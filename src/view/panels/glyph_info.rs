// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The grid view's right panel: glyph info, the preview, mark colours, and the hover overlay.

use crate::Arc;
use crate::Mode;
use crate::Workspace;
use crate::view::grid::cell_glyph_transform;
use crate::view::grid::cell_label_metrics;
use crate::view::grid::place_cells;
use crate::view::paint::IconMark;
use crate::view::paint::build_fill_path;
use crate::view::paint::build_path;
use crate::view::paint::glyph_free_icon;
use crate::view::paint::paint_batched;
use crate::view::theme as t;
use crate::widgets;
use crate::workspace::BOTTOM_BAR_H;
use crate::workspace::GridFit;
use gpui::Bounds;
use gpui::Context;
use gpui::InteractiveElement;
use gpui::IntoElement;
use gpui::ParentElement;
use gpui::PathBuilder;
use gpui::Point;
use gpui::SharedString;
use gpui::StatefulInteractiveElement;
use gpui::Styled;
use gpui::Window;
use gpui::canvas;
use gpui::div;
use gpui::prelude::FluentBuilder;
use gpui::px;
use kurbo::Affine;
use kurbo::BezPath;
use runebender_core::document::project::GlyphPoint;
use std::collections::HashMap;
impl Workspace {
    /// One canvas for every glyph in a grid, batched by colour. The
    /// cells themselves are plain divs: gpui breaks its render pass at
    /// each run of paths, so a canvas per cell cost a pass per cell.
    ///
    /// `rows` is the packed rows that are on screen. Where each cell
    /// lands is worked out in the paint closure, against the bounds
    /// the overlay was actually given, so the outlines follow the
    /// cells through a resize instead of trailing the probe.
    pub(crate) fn glyph_overlay(
        &self,
        rows: Vec<Vec<(usize, usize)>>,
        fit: GridFit,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let _ = cx;
        let font = self.font()?;
        let upm = font.units_per_em;
        // Everything the paint closure needs about a glyph, pulled out
        // here because it cannot borrow the font.
        let mut ink: HashMap<usize, (Arc<BezPath>, kurbo::Rect, f64, gpui::Rgba)> = HashMap::new();
        for &(glyph, _) in rows.iter().flatten() {
            let Some(entry) = font.glyphs.get(glyph) else {
                continue;
            };
            if entry.path.elements().is_empty() {
                continue;
            }
            let selected = self.selected == Some(glyph)
                || self.grid.multi_selected.contains(entry.name.as_ref());
            let color = if selected {
                t::cell_selected_ring()
            } else {
                t::mark_paint(entry.mark.as_deref())
                    .map(|p| p.ink)
                    .unwrap_or_else(t::glyph_fill)
            };
            ink.insert(glyph, (entry.path.clone(), entry.ink, entry.advance, color));
        }
        if ink.is_empty() {
            return None;
        }
        Some(
            canvas(
                move |bounds, _, _| bounds,
                move |_, bounds: Bounds<gpui::Pixels>, window, _| {
                    // One path per ink colour, so the whole grid is a
                    // handful of draws however many cells are on screen.
                    let mut batches: std::collections::BTreeMap<u32, (gpui::Rgba, Vec<BezPath>)> =
                        std::collections::BTreeMap::new();
                    for cell in place_cells(&rows, fit, bounds.size, 0) {
                        let Some((path, bbox, advance, color)) = ink.get(&cell.glyph) else {
                            continue;
                        };
                        // The cell sizes its own label block from its
                        // own width, so a cell spanning two columns
                        // gets a taller one: ask the same question.
                        let label_h = cell_label_metrics(cell.w).height;
                        let transform = cell_glyph_transform(
                            *bbox,
                            false,
                            *advance,
                            upm,
                            cell.w as f64,
                            (cell.h - label_h) as f64,
                        );
                        let place = Affine::translate((cell.x as f64, cell.y as f64)) * transform;
                        let key = u32::from_be_bytes([
                            (color.r * 255.0) as u8,
                            (color.g * 255.0) as u8,
                            (color.b * 255.0) as u8,
                            (color.a * 255.0) as u8,
                        ]);
                        batches
                            .entry(key)
                            .or_insert_with(|| (*color, Vec::new()))
                            .1
                            .push(place * path.as_ref().clone());
                    }
                    for (color, paths) in batches.values() {
                        paint_batched(window, bounds.origin, *color, paths, None);
                    }
                },
            )
            // An absolute element with no inset lands at its static
            // position, which for the last child of a column is below
            // everything before it: without this the whole grid was
            // painted a viewport lower and clipped away.
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
    }

    /// Right tile: details of the selected glyph. This is
    /// `GlyphInfoSidebar` in runebender-web.
    pub(crate) fn glyph_info_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        // Read-only facts read as one line each, label left and value
        // right. A stack of big accent-green headings for one-word
        // values was most of what made this panel shout.
        let row = |header: &'static str, value: SharedString| {
            div()
                .h(px(18.0))
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .text_xs()
                .child(div().text_color(t::text_muted()).child(header))
                .child(div().text_sm().text_color(t::text()).child(value))
        };
        let mut panel = div().flex().flex_col().gap_2();
        let (Some(project), Some(index)) = (self.project.as_ref(), self.selected) else {
            return self.section(
                cx,
                "Glyph",
                div()
                    .text_sm()
                    .text_color(t::text_muted())
                    .child("Select a glyph"),
            );
        };
        let font = project.active_font();
        let Some(entry) = font.glyphs.get(index) else {
            return self.section(cx, "Glyph", div());
        };
        let name = entry.name.to_string();
        let master = project.master_names[project.active].clone();
        let _ = name;
        // Editable fields commit on Enter (rename, unicode, kerning
        // groups); the rest stay read-only rows.
        let input_row = |header: &'static str, input: &gpui::Entity<widgets::input::InputState>| {
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(div().text_xs().text_color(t::text_muted()).child(header))
                .child(widgets::input::Input::new(input))
        };
        let pair_row = |header: &'static str,
                        a: &gpui::Entity<widgets::input::InputState>,
                        b: &gpui::Entity<widgets::input::InputState>| {
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(div().text_xs().text_color(t::text_muted()).child(header))
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child(div().flex_1().child(widgets::input::Input::new(a)))
                        .child(div().flex_1().child(widgets::input::Input::new(b))),
                )
        };
        // Width and the sidebearings are edited here, beside the name
        // and the kerning groups, the way the web keeps a glyph's
        // metrics in one panel. Enter commits each field.
        let metric_field =
            |label_text: &'static str, input: &gpui::Entity<widgets::input::InputState>| {
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .text_xs()
                            .text_color(t::text_muted())
                            .child(label_text),
                    )
                    .child(widgets::input::Input::new(input))
            };
        // In the editor the metric fields live in the floating panel
        // over the canvas (Glyphs-style), so they appear here only in
        // the grid: one input entity, one place on screen.
        let in_editor = matches!(self.mode, Mode::Editor(_));
        panel = panel
            .child(row("Master", SharedString::from(master)))
            // Why the glyph is not interpolating, when it is not: the
            // grid dot says that something is wrong, this says what.
            .when_some(
                self.project
                    .as_ref()
                    .and_then(|p| p.compat_detail(entry.name.as_ref())),
                |el, detail| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(t::status_yellow())
                            .child(format!("Not interpolating: {detail}")),
                    )
                },
            )
            .child(input_row("Glyph Name", &self.inputs.glyph.name))
            .when(in_editor, |el| {
                el.child(row("Width", format!("{:.0}", entry.advance).into()))
            })
            .when(!in_editor, |el| {
                el.child(
                    div()
                        .flex()
                        .gap_1()
                        .child(metric_field("Width", &self.inputs.metric.width))
                        .child(metric_field("LSB", &self.inputs.metric.lsb))
                        .child(metric_field("RSB", &self.inputs.metric.rsb)),
                )
            })
            .child(pair_row(
                "Kerning Groups (L · R)",
                &self.inputs.glyph.group_l,
                &self.inputs.glyph.group_r,
            ))
            .child(pair_row(
                "Metrics Keys (L · R)",
                &self.inputs.glyph.lsb_key,
                &self.inputs.glyph.rsb_key,
            ))
            .child(input_row("Unicode", &self.inputs.glyph.unicode))
            // The character's Unicode name, the Glyph Info window's
            // headline fact, quietly under the code point.
            .when_some(
                entry
                    .codepoint
                    .and_then(unicode_names2::name)
                    .map(|n| n.to_string()),
                |el, uni_name| {
                    el.child(div().text_xs().text_color(t::text_muted()).child(uni_name))
                },
            )
            .child(input_row("Production Name", &self.inputs.glyph.production))
            .child(input_row("Note", &self.inputs.glyph.note))
            // A part glyph's smart axis ("Width,0,100"): defines the
            // axis and seeds the top pole layer.
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .text_xs()
                            .text_color(t::text_muted())
                            .child("Smart Axis (name,min,max)"),
                    )
                    .child(widgets::input::Input::new(&self.glyph_smart_axis_ref())),
            )
            // Bracket layers: the shape switch on this glyph, or the
            // field that creates one at a typed axis value.
            .child({
                let switch = self.project.as_ref().and_then(|p| {
                    p.ds_doc.as_ref()?.rules.rules.iter().find_map(|rule| {
                        let sub = rule
                            .substitutions
                            .iter()
                            .find(|sub| sub.name.as_ref() == entry.name.as_ref())?;
                        let cond = rule
                            .condition_sets
                            .first()
                            .and_then(|set| set.conditions.first());
                        Some((
                            sub.with.to_string(),
                            cond.and_then(|c| c.minimum),
                            cond.map(|c| c.name.clone()),
                        ))
                    })
                });
                match switch {
                    Some((with, min, axis)) => div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_xs()
                        .child(div().text_color(t::text_muted()).child(format!(
                            "→ {with} at {} ≥ {}",
                            axis.unwrap_or_else(|| "axis".into()),
                            min.map(|v| format!("{v:.0}")).unwrap_or_else(|| "?".into()),
                        )))
                        .child(
                            div()
                                .id("switch-remove")
                                .px_1()
                                .cursor_pointer()
                                .text_color(t::text_muted())
                                .hover(|el| el.text_color(t::text()))
                                .child("×")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.command_remove_shape_switch();
                                    cx.notify();
                                })),
                        ),
                    None => div()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(
                            div()
                                .text_xs()
                                .text_color(t::text_muted())
                                .child("Switch At (axis value)"),
                        )
                        .child(widgets::input::Input::new(&self.inputs.glyph.switch_at)),
                }
            });
        self.section(cx, "Glyph", panel)
    }

    /// Right-panel live preview of the selected glyph: outline plus
    /// control points.
    ///
    /// It fills the space between the info sections and the colors,
    /// the way runebender-web does.
    pub(crate) fn glyph_preview_panel(&self) -> gpui::Div {
        let data = self.selected.and_then(|index| {
            let font = self.font()?;
            let entry = &font.glyphs[index];
            Some((
                entry.contour_path.clone(),
                entry.component_path.clone(),
                entry.points.clone(),
                entry.advance,
                font.ascender,
                font.descender,
            ))
        });
        let body: gpui::AnyElement = match data {
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(t::text_muted())
                .into_any_element(),
            Some((outline, components, points, advance, ascender, descender)) => {
                canvas(
                    move |bounds, _, _| bounds,
                    move |_, bounds: Bounds<gpui::Pixels>, window, _| {
                        let w: f32 = bounds.size.width.into();
                        let h: f32 = bounds.size.height.into();
                        if w < 40.0 || h < 40.0 {
                            return;
                        }
                        // Fit the em box (with the glyph's actual
                        // advance) into the panel with padding.
                        use kurbo::Shape as _;
                        let ink = {
                            let mut b = outline.bounding_box();
                            if !components.elements().is_empty() {
                                b = b.union(components.bounding_box());
                            }
                            b
                        };
                        let (ink_w, ink_h, ink_cx, ink_cy) =
                            if ink.width() > 0.0 && ink.height() > 0.0 {
                                (ink.width(), ink.height(), ink.center().x, ink.center().y)
                            } else {
                                // A blank glyph still needs a box: use
                                // its advance and the em.
                                let em_h = (ascender - descender).max(1.0);
                                let em_w = advance.max(em_h * 0.3);
                                (em_w, em_h, em_w / 2.0, (ascender + descender) / 2.0)
                            };
                        let scale = ((w as f64 * 0.88) / ink_w).min((h as f64 * 0.88) / ink_h);
                        let origin_x = w as f64 / 2.0 - ink_cx * scale;
                        let baseline = h as f64 / 2.0 + ink_cy * scale;
                        let view = Affine::translate((origin_x, baseline))
                            * Affine::scale_non_uniform(scale, -scale);
                        let to_screen = |x: f64, y: f64| {
                            let p = view * kurbo::Point::new(x, y);
                            gpui::point(
                                bounds.origin.x + px(p.x as f32),
                                bounds.origin.y + px(p.y as f32),
                            )
                        };
                        // No metric frame here: this tile is a shape
                        // preview, and the box only ate the space the
                        // outline could use.
                        if !components.elements().is_empty()
                            && let Some(p) = build_fill_path(&components, view, bounds.origin)
                        {
                            window.paint_path(p, t::component_fill());
                        }
                        if let Some(p) =
                            build_path(&outline, view, bounds.origin, PathBuilder::stroke(px(1.0)))
                        {
                            window.paint_path(p, t::path_stroke());
                        }
                        // Handle lines then points, editor-style but
                        // small.
                        let mut handles = PathBuilder::stroke(px(1.0));
                        let mut any_handles = false;
                        for p in points.iter() {
                            if p.on_curve {
                                continue;
                            }
                            let contour_pts: Vec<&GlyphPoint> =
                                points.iter().filter(|q| q.contour == p.contour).collect();
                            let n = contour_pts.len();
                            let Some(pos) = contour_pts.iter().position(|q| q.index == p.index)
                            else {
                                continue;
                            };
                            let prev = contour_pts[(pos + n - 1) % n];
                            let next = contour_pts[(pos + 1) % n];
                            let anchor = if prev.on_curve {
                                prev
                            } else if next.on_curve {
                                next
                            } else {
                                continue;
                            };
                            handles.move_to(to_screen(p.x, p.y));
                            handles.line_to(to_screen(anchor.x, anchor.y));
                            any_handles = true;
                        }
                        if any_handles && let Ok(p) = handles.build() {
                            window.paint_path(p, t::handle_line());
                        }
                        let ring = |center: Point<gpui::Pixels>,
                                    r: f32,
                                    color: gpui::Rgba,
                                    window: &mut Window| {
                            let cx_: f32 = center.x.into();
                            let cy_: f32 = center.y.into();
                            let shape = kurbo::Circle::new((cx_ as f64, cy_ as f64), r as f64)
                                .to_path(0.25);
                            if let Some(p) = build_fill_path(
                                &shape,
                                Affine::IDENTITY,
                                gpui::point(px(0.0), px(0.0)),
                            ) {
                                window.paint_path(p, t::point_inner());
                            }
                            if let Some(p) = build_path(
                                &shape,
                                Affine::IDENTITY,
                                gpui::point(px(0.0), px(0.0)),
                                PathBuilder::stroke(px(1.0)),
                            ) {
                                window.paint_path(p, color);
                            }
                        };
                        for p in points.iter() {
                            let center = to_screen(p.x, p.y);
                            if !p.on_curve {
                                ring(center, 2.0, t::point_offcurve_outer(), window);
                            } else if p.smooth {
                                ring(center, 3.0, t::point_smooth_outer(), window);
                            } else if p.hyper {
                                ring(center, 3.0, t::point_hyper_outer(), window);
                            } else {
                                window.paint_quad(gpui::fill(
                                    Bounds::from_corners(
                                        gpui::point(center.x - px(2.5), center.y - px(2.5)),
                                        gpui::point(center.x + px(2.5), center.y + px(2.5)),
                                    ),
                                    t::point_corner_outer(),
                                ));
                            }
                        }
                    },
                )
                .size_full()
                .into_any_element()
            }
        };
        div().flex_1().min_h(px(200.0)).p_1().child(body)
    }

    /// The row of mark colour swatches: clicking one sets the selected
    /// glyph's mark, and the last swatch clears it.
    pub(crate) fn mark_colors_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        let current = self
            .selected
            .and_then(|i| self.font().and_then(|f| f.glyphs.get(i)))
            .and_then(|e| e.mark.clone());
        // One row, always: each swatch sits in an equal-width column,
        // so the spacing between them and the margin at both ends stay
        // the same at any panel width.
        const SWATCH: f32 = 14.0;
        // (bar height - swatch) / 2, so the ring of space around the
        // row is the same on every side.
        const INSET: f32 = (BOTTOM_BAR_H - (SWATCH + 6.0)) / 2.0;
        let slot = |child: gpui::Stateful<gpui::Div>| child;
        let mut swatches = div().flex().items_center().justify_between().w_full();
        for (index, (label, color)) in t::mark_palette().into_iter().enumerate() {
            let is_current = current.as_deref() == Some(label.as_str());
            // Selected reads as a ring in the swatch's own colour with
            // a dark gap inside it, rather than a white outline drawn
            // over the colour: the colour stays the thing you see.
            swatches = swatches.child(slot(
                div()
                    .id(("mark-swatch", index))
                    .w(px(SWATCH + 6.0))
                    .h(px(SWATCH + 6.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .border(t::stroke())
                    .border_color(if is_current {
                        color
                    } else {
                        gpui::Rgba {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 0.0,
                        }
                    })
                    .cursor_pointer()
                    .child(div().w(px(SWATCH)).h(px(SWATCH)).rounded_full().bg(color))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_selected_mark(Some(label.clone()));
                        cx.notify();
                    })),
            ));
        }
        // "No colour" is a swatch like the others: same ring when it is
        // the one in force, drawn in the muted grey it stands for.
        swatches = swatches.child(slot(
            div()
                .id("mark-clear")
                .w(px(SWATCH + 6.0))
                .h(px(SWATCH + 6.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .border(t::stroke())
                .border_color(if current.is_none() {
                    t::text_muted()
                } else {
                    gpui::Rgba {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 0.0,
                    }
                })
                .cursor_pointer()
                .child(
                    div()
                        .w(px(SWATCH))
                        .h(px(SWATCH))
                        .rounded_full()
                        .border(t::stroke())
                        .border_color(t::text_muted())
                        .child(glyph_free_icon(t::text_muted(), IconMark::Cross)),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.set_selected_mark(None);
                    cx.notify();
                })),
        ));
        // No header, no collapse: it is one row of swatches that is
        // always up. It carries its own top rule since it is the last
        // thing in the sidebar.
        div()
            .h(px(BOTTOM_BAR_H))
            .flex()
            .items_center()
            .border_t_1()
            .border_color(t::panel_outline())
            .px(px(INSET))
            .child(swatches)
    }
}
