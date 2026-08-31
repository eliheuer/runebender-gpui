// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A real blur for the text preview.
//!
//! Blurring type is how you check spacing. Out-of-focus text turns
//! the rhythm of the line into light and dark bands. A loose or tight
//! join shows up as a gap or a clot. That only works with an actual
//! blur. gpui blurs box shadows and nothing else, so the line is
//! rasterized here, blurred, and handed back as an image.
//!
//! The blur is three box passes, the standard cheap stand-in for a
//! gaussian. By the third pass the kernel is a piecewise cubic close
//! enough to a gaussian that the difference is invisible at these
//! radii.

use std::sync::Arc;

use gpui::{RenderImage, Rgba};
use kurbo::{BezPath, PathEl};

/// Rasterize `path` over `ground`, blur it, and return an image ready
/// for `paint_image`.
///
/// `path` is already in the pane's own pixel coordinates. `radius` is
/// in logical pixels. `scale` is device pixels per logical pixel.
/// Returns `None` for a degenerate size or a path with nothing in it.
pub fn blurred_line(
    path: &BezPath,
    width: f32,
    height: f32,
    scale: f32,
    ink: Rgba,
    ground: Rgba,
    radius: f32,
) -> Option<Arc<RenderImage>> {
    let scale = scale.max(1.0);
    let w = (width * scale).round() as u32;
    let h = (height * scale).round() as u32;
    if w == 0 || h == 0 || w > 8192 || h > 8192 || path.elements().is_empty() {
        return None;
    }

    let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
    pixmap.fill(color(ground));

    let mut builder = tiny_skia::PathBuilder::new();
    let s = scale as f64;
    for element in path.elements() {
        match element {
            PathEl::MoveTo(p) => builder.move_to((p.x * s) as f32, (p.y * s) as f32),
            PathEl::LineTo(p) => builder.line_to((p.x * s) as f32, (p.y * s) as f32),
            PathEl::QuadTo(c, p) => builder.quad_to(
                (c.x * s) as f32,
                (c.y * s) as f32,
                (p.x * s) as f32,
                (p.y * s) as f32,
            ),
            PathEl::CurveTo(c1, c2, p) => builder.cubic_to(
                (c1.x * s) as f32,
                (c1.y * s) as f32,
                (c2.x * s) as f32,
                (c2.y * s) as f32,
                (p.x * s) as f32,
                (p.y * s) as f32,
            ),
            PathEl::ClosePath => builder.close(),
        }
    }
    let outline = builder.finish()?;

    let mut paint = tiny_skia::Paint::default();
    paint.set_color(color(ink));
    paint.anti_alias = true;
    pixmap.fill_path(
        &outline,
        &paint,
        tiny_skia::FillRule::Winding,
        tiny_skia::Transform::identity(),
        None,
    );

    // The pixmap is premultiplied, so the passes can average the
    // channels directly.
    let r = (radius * scale).round() as i32;
    if r > 0 {
        let data = pixmap.pixels_mut();
        let mut buffer = vec![tiny_skia::PremultipliedColorU8::TRANSPARENT; data.len()];
        for _ in 0..3 {
            box_pass(data, &mut buffer, w as i32, h as i32, r, true);
            box_pass(data, &mut buffer, w as i32, h as i32, r, false);
        }
    }

    // gpui's RenderImage holds BGRA.
    let mut bytes = Vec::with_capacity((w * h * 4) as usize);
    for pixel in pixmap.pixels() {
        bytes.extend_from_slice(&[pixel.blue(), pixel.green(), pixel.red(), pixel.alpha()]);
    }
    let buffer = image::RgbaImage::from_raw(w, h, bytes)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

/// One box-blur pass along a single axis, reading `data` and writing it
/// back through `scratch`.
fn box_pass(
    data: &mut [tiny_skia::PremultipliedColorU8],
    scratch: &mut [tiny_skia::PremultipliedColorU8],
    w: i32,
    h: i32,
    radius: i32,
    horizontal: bool,
) {
    let (outer, inner) = if horizontal { (h, w) } else { (w, h) };
    let index = |a: i32, b: i32| -> usize {
        if horizontal {
            (a * w + b) as usize
        } else {
            (b * w + a) as usize
        }
    };
    let window = (radius * 2 + 1) as u32;
    for a in 0..outer {
        for b in 0..inner {
            let (mut r, mut g, mut bl, mut al) = (0u32, 0u32, 0u32, 0u32);
            for k in -radius..=radius {
                // Clamp at the edges: the ground colour continues
                // rather than fading to nothing.
                let sample = (b + k).clamp(0, inner - 1);
                let p = data[index(a, sample)];
                r += p.red() as u32;
                g += p.green() as u32;
                bl += p.blue() as u32;
                al += p.alpha() as u32;
            }
            scratch[index(a, b)] = tiny_skia::PremultipliedColorU8::from_rgba(
                (r / window) as u8,
                (g / window) as u8,
                (bl / window) as u8,
                (al / window) as u8,
            )
            .unwrap_or(tiny_skia::PremultipliedColorU8::TRANSPARENT);
        }
    }
    data.copy_from_slice(scratch);
}

/// Converts a gpui `Rgba` to a `tiny_skia` colour; black for
/// out-of-range channels.
fn color(c: Rgba) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba(c.r, c.g, c.b, c.a).unwrap_or(tiny_skia::Color::BLACK)
}
