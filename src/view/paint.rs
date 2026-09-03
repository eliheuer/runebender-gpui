// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Painting helpers shared by the canvas, the grid, and the panels:
//! kurbo paths into gpui paths, icons drawn from paths, batched fills,
//! and the blur cache key.

use crate::view::render::px32;
use crate::view::theme as t;
use crate::widgets;
use gpui::Bounds;
use gpui::IntoElement;
use gpui::ParentElement;
use gpui::PathBuilder;
use gpui::Point;
use gpui::Styled;
use gpui::Window;
use gpui::canvas;
use gpui::div;
use gpui::px;
use kurbo::Affine;
use kurbo::BezPath;
use kurbo::PathEl;

/// Build a gpui path from `outline` through `transform`, offset by
/// `origin`.
///
/// The affine maps design space (font units, Y-up) into local pixels
/// (Y-down). Returns `None` for an empty outline or a failed build.
pub(crate) fn build_path(
    outline: &BezPath,
    transform: Affine,
    origin: Point<gpui::Pixels>,
    mut builder: PathBuilder,
) -> Option<gpui::Path<gpui::Pixels>> {
    let mut any = false;
    let gp = |p: kurbo::Point| gpui::point(origin.x + px(px32(p.x)), origin.y + px(px32(p.y)));
    for el in transform * outline {
        match el {
            PathEl::MoveTo(p) => builder.move_to(gp(p)),
            PathEl::LineTo(p) => builder.line_to(gp(p)),
            PathEl::QuadTo(c, p) => builder.curve_to(gp(p), gp(c)),
            PathEl::CurveTo(c1, c2, p) => builder.cubic_bezier_to(gp(p), gp(c1), gp(c2)),
            PathEl::ClosePath => builder.close(),
        }
        any = true;
    }
    if !any {
        return None;
    }
    builder.build().ok()
}

/// A shared toolbar icon painted to fit its element, centered with
/// padding.
///
/// Icon geometry comes from runebender-core, from the same icon UFO
/// the web toolbar uses.
pub(crate) fn icon_svg(name: &'static str, color: gpui::Rgba) -> impl IntoElement {
    canvas(
        move |bounds, _, _| bounds,
        move |_, bounds: Bounds<gpui::Pixels>, window, _| {
            let Some(icon) = runebender_core::ui::theme::toolbar_icons().get(name) else {
                return;
            };
            let w: f32 = bounds.size.width.into();
            let h: f32 = bounds.size.height.into();
            // Proportional inset: a fixed one shrank the mark inside
            // bigger tiles and crowded it in small ones.
            let pad = (w.min(h) as f64) * 0.12;
            let vb = icon.view_box;
            let scale =
                ((w as f64 - pad * 2.0) / vb.width()).min((h as f64 - pad * 2.0) / vb.height());
            let dx = (w as f64 - vb.width() * scale) / 2.0;
            let dy = (h as f64 - vb.height() * scale) / 2.0;
            // Icon space is Y-down SVG, same as gpui.
            let transform = Affine::translate((dx, dy))
                * Affine::scale(scale)
                * Affine::translate((-vb.x0, -vb.y0));
            if let Some(path) = build_fill_path(&icon.path, transform, bounds.origin) {
                window.paint_path(path, color);
            }
        },
    )
    .size_full()
}

/// Comparable key for a segment. `PathSeg` has no `Eq`.
pub(crate) fn seg_key(seg: kurbo::PathSeg) -> [u64; 8] {
    let p = |pt: kurbo::Point| [pt.x.to_bits(), pt.y.to_bits()];
    match seg {
        kurbo::PathSeg::Line(l) => {
            let [a, b] = [p(l.p0), p(l.p1)];
            [a[0], a[1], b[0], b[1], 0, 0, 0, 0]
        }
        kurbo::PathSeg::Quad(q) => {
            let [a, b, c] = [p(q.p0), p(q.p1), p(q.p2)];
            [a[0], a[1], b[0], b[1], c[0], c[1], 0, 1]
        }
        kurbo::PathSeg::Cubic(c) => {
            let [a, b, cc, d] = [p(c.p0), p(c.p1), p(c.p2), p(c.p3)];
            [a[0], a[1], b[0], b[1], cc[0], cc[1], d[0], d[1]]
        }
    }
}

/// `build_path` with a fill builder.
pub(crate) fn build_fill_path(
    outline: &BezPath,
    transform: Affine,
    origin: Point<gpui::Pixels>,
) -> Option<gpui::Path<gpui::Pixels>> {
    build_path(outline, transform, origin, PathBuilder::fill())
}

/// A flat slider: a thin, evenly colored track and a ring thumb.
///
/// The library's own styling tints the unfilled side of the track
/// with the bar color, which reads as a dark stripe on one side; this
/// track is one color. The thumb fills solid while it is grabbed
/// instead of growing a translucent halo.
pub(crate) fn flat_slider(
    state: &gpui::Entity<widgets::slider::SliderState>,
    cx: &gpui::App,
) -> gpui::AnyElement {
    const TRACK: f32 = 3.0;
    const THUMB: f32 = 12.0;

    let pct = state.read(cx).percentage();
    let bar = div()
        .relative()
        .w_full()
        .h(px(TRACK))
        .rounded_full()
        // One colour end to end: this reports a value, it is not a
        // progress bar. The rule colour, so it sits with the panel's
        // other lines.
        .bg(t::cell_border())
        .child(
            div()
                .absolute()
                .top(px((TRACK - THUMB) / 2.0))
                .left(gpui::relative(pct))
                .ml(px(-THUMB / 2.0))
                .w(px(THUMB))
                .h(px(THUMB))
                .flex_shrink_0()
                .rounded_full()
                .border(t::stroke_emphasis())
                .border_color(t::text())
                .bg(t::panel_bg()),
        );
    widgets::slider::track(state, px(THUMB), bar).into_any_element()
}

/// Everything the blurred preview image depends on, hashed: the line
/// itself, the pane size, the radius and the two colours.
pub(crate) fn blur_key(
    line: &BezPath,
    w: f64,
    h: f64,
    blur: f32,
    ink: gpui::Rgba,
    ground: gpui::Rgba,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for element in line.elements() {
        match element {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => {
                (p.x.to_bits(), p.y.to_bits()).hash(&mut hasher);
            }
            PathEl::QuadTo(a, b) => {
                (a.x.to_bits(), a.y.to_bits(), b.x.to_bits(), b.y.to_bits()).hash(&mut hasher);
            }
            PathEl::CurveTo(a, b, c) => (
                a.x.to_bits(),
                a.y.to_bits(),
                b.x.to_bits(),
                b.y.to_bits(),
                c.x.to_bits(),
                c.y.to_bits(),
            )
                .hash(&mut hasher),
            PathEl::ClosePath => 0_u8.hash(&mut hasher),
        }
    }
    (
        w.to_bits(),
        h.to_bits(),
        blur.to_bits(),
        ink.r.to_bits(),
        ink.g.to_bits(),
        ink.b.to_bits(),
        ground.r.to_bits(),
        ground.g.to_bits(),
        ground.b.to_bits(),
    )
        .hash(&mut hasher);
    hasher.finish()
}

/// A drawn eye, for the preview's show/hide switch. The icon set has
/// no eye, and the "preview" icon in it is a hand, which reads as a
/// pan tool.
pub(crate) fn eye_icon(color: gpui::Rgba, open: bool) -> impl IntoElement {
    canvas(
        move |bounds, _, _| bounds,
        move |_, bounds: Bounds<gpui::Pixels>, window, _| {
            let w = f32::from(bounds.size.width) as f64;
            let h = f32::from(bounds.size.height) as f64;
            let o = bounds.origin;
            let (cx_, cy_) = (w / 2.0, h / 2.0);
            let rx = w * 0.40;
            let ry = h * 0.30;
            let pt = |x: f64, y: f64| gpui::point(o.x + px(px32(x)), o.y + px(px32(y)));
            let mut pb = PathBuilder::stroke(px(1.2));
            // The almond: one curve over, one curve back.
            pb.move_to(pt(cx_ - rx, cy_));
            pb.curve_to(pt(cx_ + rx, cy_), pt(cx_, cy_ - ry * 2.2));
            pb.move_to(pt(cx_ - rx, cy_));
            pb.curve_to(pt(cx_ + rx, cy_), pt(cx_, cy_ + ry * 2.2));
            if !open {
                pb.move_to(pt(cx_ - rx, cy_ + ry));
                pb.line_to(pt(cx_ + rx, cy_ - ry));
            }
            if let Ok(p) = pb.build() {
                window.paint_path(p, color);
            }
            if open {
                use kurbo::Shape as _;
                let pupil = kurbo::Circle::new((cx_, cy_), ry * 0.62).to_path(0.1);
                if let Some(p) = build_fill_path(&pupil, Affine::IDENTITY, o) {
                    window.paint_path(p, color);
                }
            }
        },
    )
    .w(px(16.0))
    .h(px(16.0))
}

/// A drawn plus, minus or cross.
///
/// Set as text these sit visibly off-centre: a "×" carries its own
/// side bearings and a "−" rides above the middle. So they are
/// stroked instead.
pub(crate) fn glyph_free_icon(
    color: gpui::Rgba,
    weight: gpui::Pixels,
    kind: IconMark,
) -> impl IntoElement {
    canvas(
        move |bounds, _, _| bounds,
        move |_, bounds: Bounds<gpui::Pixels>, window, _| {
            let w = f32::from(bounds.size.width) as f64;
            let h = f32::from(bounds.size.height) as f64;
            let o = bounds.origin;
            let (cx_, cy_) = (w / 2.0, h / 2.0);
            let r = (w.min(h) / 2.0) * 0.42;
            let pt = |x: f64, y: f64| gpui::point(o.x + px(px32(x)), o.y + px(px32(y)));
            let mut pb = PathBuilder::stroke(weight);
            match kind {
                IconMark::Plus | IconMark::Minus => {
                    pb.move_to(pt(cx_ - r, cy_));
                    pb.line_to(pt(cx_ + r, cy_));
                    if matches!(kind, IconMark::Plus) {
                        pb.move_to(pt(cx_, cy_ - r));
                        pb.line_to(pt(cx_, cy_ + r));
                    }
                }
            }
            if let Ok(p) = pb.build() {
                window.paint_path(p, color);
            }
        },
    )
    .size_full()
}

#[derive(Clone, Copy)]
/// The mark `glyph_free_icon` strokes.
pub(crate) enum IconMark {
    /// A plus sign.
    Plus,
    /// A horizontal minus stroke.
    Minus,
}

/// A circle filled on one half: the ink/ground flip.
pub(crate) fn invert_icon(color: gpui::Rgba) -> impl IntoElement {
    canvas(
        move |bounds, _, _| bounds,
        move |_, bounds: Bounds<gpui::Pixels>, window, _| {
            use kurbo::Shape as _;
            let w = f32::from(bounds.size.width) as f64;
            let h = f32::from(bounds.size.height) as f64;
            let o = bounds.origin;
            let r = (w.min(h) / 2.0) - 1.5;
            let center = (w / 2.0, h / 2.0);
            let ring = kurbo::Circle::new(center, r).to_path(0.1);
            if let Some(p) = build_path(&ring, Affine::IDENTITY, o, PathBuilder::stroke(px(1.2))) {
                window.paint_path(p, color);
            }
            // The filled half, as a half-turn arc closed across the
            // diameter.
            let half = kurbo::Arc {
                center: center.into(),
                radii: (r, r).into(),
                start_angle: std::f64::consts::FRAC_PI_2,
                sweep_angle: std::f64::consts::PI,
                x_rotation: 0.0,
            }
            .to_path(0.1);
            if let Some(p) = build_fill_path(&half, Affine::IDENTITY, o) {
                window.paint_path(p, color);
            }
        },
    )
    .w(px(16.0))
    .h(px(16.0))
}

/// Paint many subpaths as few draws without overflowing gpui's
/// tessellator.
///
/// The tessellator indexes vertices with a `u16`. Merging a whole
/// screen of glyph outlines into one path exceeds 65,535 vertices,
/// `build` fails, and nothing is drawn at all. So batches are flushed
/// every `CHUNK` subpaths, and a batch that still fails is halved
/// until it builds.
pub(crate) fn paint_batched(
    window: &mut Window,
    origin: Point<gpui::Pixels>,
    color: gpui::Rgba,
    subpaths: &[BezPath],
    stroke: Option<f32>,
) {
    const CHUNK: usize = 12;
    pub(crate) fn paint_chunk(
        window: &mut Window,
        origin: Point<gpui::Pixels>,
        color: gpui::Rgba,
        chunk: &[BezPath],
        stroke: Option<f32>,
    ) {
        if chunk.is_empty() {
            return;
        }
        let mut merged = BezPath::new();
        for path in chunk {
            merged.extend(path.iter());
        }
        let built = match stroke {
            Some(width) => build_path(
                &merged,
                Affine::IDENTITY,
                origin,
                PathBuilder::stroke(px(width)),
            ),
            None => build_fill_path(&merged, Affine::IDENTITY, origin),
        };
        match built {
            Some(path) => window.paint_path(path, color),
            // Too much geometry for one path: split and retry, down to
            // a single subpath.
            None if chunk.len() > 1 => {
                let mid = chunk.len() / 2;
                paint_chunk(window, origin, color, &chunk[..mid], stroke);
                paint_chunk(window, origin, color, &chunk[mid..], stroke);
            }
            None => {}
        }
    }
    for chunk in subpaths.chunks(CHUNK) {
        paint_chunk(window, origin, color, chunk, stroke);
    }
}
