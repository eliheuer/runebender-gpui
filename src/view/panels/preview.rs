// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! The preview strip along the bottom of the editor.

use crate::Arc;
use crate::Workspace;
use crate::blur_key;
use crate::build_fill_path;
use crate::view::blur;
use crate::view::theme as t;
use gpui::Bounds;
use gpui::Context;
use gpui::IntoElement;
use gpui::ParentElement;
use gpui::Styled;
use gpui::canvas;
use gpui::div;
use gpui::px;
use kurbo::Affine;
use kurbo::BezPath;
impl Workspace {
    pub(crate) fn preview_strip(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let Some(font) = self.font() else {
            return div().into_any_element();
        };
        let ascender = font.ascender;
        let descender = font.descender;
        let upm = font.units_per_em;
        let line_height = self.text_line_height();
        let layout = self.edit_buffer.layout(line_height);
        // Each sort's outline, its pen position, and its advance, so
        // the line can be measured and centered.
        let items: Vec<(Arc<BezPath>, f64, f64, f64)> = layout
            .items
            .iter()
            .filter_map(|item| {
                let sort = self.edit_buffer.sort(item.index)?;
                if sort.is_absorbed() {
                    return None;
                }
                let name = sort.glyph_name()?;
                // Bracket rules preview: past a shape switch the strip
                // shows the substitute, like an exported instance.
                let subbed = self.project.as_ref().and_then(|p| p.rule_substitute(name));
                let name: &str = subbed.as_deref().unwrap_or(name);
                let glyph = *font.name_map.get(name)?;
                // Off the masters the strip shows the interpolation,
                // like the canvas ghost (and the Instances rows park
                // the location, so clicking Medium previews Medium).
                // Pen positions stay the buffer's: master metrics.
                let path = self
                    .project
                    .as_ref()
                    .and_then(|p| p.interpolated_glyph(name))
                    .map(|(path, _)| Arc::new(path))
                    .unwrap_or_else(|| font.glyphs[glyph].path.clone());
                Some((path, item.x, item.y, font.glyphs[glyph].advance))
            })
            .collect();
        let line_width = items
            .iter()
            .map(|(_, x, _, adv)| x + adv)
            .fold(0.0_f64, f64::max);
        // The line's ink, in design units relative to the first
        // baseline: what the preview centres on.
        let ink_extent: Option<(f64, f64)> = {
            use kurbo::Shape as _;
            let mut extent: Option<(f64, f64)> = None;
            for (path, _, y, _) in items.iter() {
                if path.elements().is_empty() {
                    continue;
                }
                let b = path.bounding_box();
                let (top, bottom) = (b.y1 + y, b.y0 + y);
                extent = Some(match extent {
                    Some((t, bo)) => (t.max(top), bo.min(bottom)),
                    None => (top, bottom),
                });
            }
            extent
        };

        let blur = self.preview.blur;
        let blur_cache = self.preview.blur_cache.clone();
        let invert = self.preview.invert;

        let body = div().size_full().min_h(px(0.0)).child(
            canvas(
                move |bounds, _, _| bounds,
                move |_, bounds: Bounds<gpui::Pixels>, window, _| {
                    let w: f64 = f32::from(bounds.size.width) as f64;
                    let h: f64 = f32::from(bounds.size.height) as f64;
                    let (ink, ground) = if invert {
                        (t::window_bg(), t::preview_glyph())
                    } else {
                        (t::preview_glyph(), t::panel_bg())
                    };
                    window.paint_quad(gpui::fill(bounds, ground));
                    // The type fits the pane, the way Glyphs and the
                    // web preview do it: one scale that fits vertically
                    // and the whole line horizontally, whichever is
                    // tighter. Drag the pane taller and the text grows
                    // with it.
                    //
                    // The em box is the wrong thing to centre on: for
                    // "8" the descender depth is empty, so centring the
                    // box leaves the ink riding high. Centre the ink
                    // the line actually has instead, which also keeps a
                    // deep Arabic descender in the middle of the pane
                    // rather than hanging off the bottom. The em box is
                    // the fallback when there is no ink at all.
                    let pad = 16.0;
                    let (ink_top, ink_bottom) = ink_extent.unwrap_or((ascender, descender));
                    let ink_h = (ink_top - ink_bottom).max(1.0);
                    let by_height = (h - pad * 2.0).max(1.0) / ink_h;
                    let by_width = if line_width > 0.0 {
                        (w - pad * 2.0).max(1.0) / line_width
                    } else {
                        by_height
                    };
                    let scale = by_height.min(by_width);
                    // Baseline placed so the ink's own middle lands on
                    // the pane's middle.
                    let baseline = h / 2.0 + (ink_top + ink_bottom) / 2.0 * scale;
                    let text_w = line_width * scale;
                    let origin_x = (w - text_w) / 2.0;
                    let _ = (upm, ascender, descender);
                    // gpui paints paths, not filters, so a blur is a
                    // stack of offset passes: one ring plus the middle,
                    // each at a fraction of the ink's alpha.
                    // One path for the whole line, in the pane's own
                    // pixel space.
                    let mut line = BezPath::new();
                    for (path, x, y, _) in items.iter() {
                        let transform =
                            Affine::translate((origin_x + x * scale, baseline - y * scale))
                                * Affine::scale_non_uniform(scale, -scale);
                        line.extend(transform * path.as_ref().clone());
                    }
                    if blur > 0.05 {
                        // Rasterized and blurred for real: gpui has no
                        // blur for paths, and stacking offset copies
                        // reads as ghosting rather than defocus.
                        let key = blur_key(&line, w, h, blur, ink, ground);
                        let cached = {
                            let slot = blur_cache.lock().unwrap();
                            slot.as_ref()
                                .filter(|(k, _)| *k == key)
                                .map(|(_, image)| image.clone())
                        };
                        let image = cached.or_else(|| {
                            let image = blur::blurred_line(
                                &line,
                                w as f32,
                                h as f32,
                                window.scale_factor(),
                                ink,
                                ground,
                                blur,
                            )?;
                            *blur_cache.lock().unwrap() = Some((key, image.clone()));
                            Some(image)
                        });
                        if let Some(image) = image {
                            let _ = window.paint_image(
                                bounds,
                                bounds,
                                gpui::Corners::default(),
                                image,
                                0,
                                false,
                            );
                            return;
                        }
                    }
                    if let Some(p) = build_fill_path(&line, Affine::IDENTITY, bounds.origin) {
                        window.paint_path(p, ink);
                    }
                },
            )
            .size_full(),
        );

        let _ = cx;
        div()
            .size_full()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .bg(t::panel_bg())
            .border_t_1()
            .border_color(t::cell_border())
            .child(body)
            .into_any_element()
    }
}
