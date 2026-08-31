// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The canvas: the glyph grid and the editing view.
//!
//! These build what fills the middle of the window. This file holds
//! the glyph grid; `editor` holds the editing view. Outlines are
//! painted through one canvas element over the whole grid rather than
//! one per cell, because gpui ends its render pass at every run of
//! paths and a canvas per cell meant a pass switch per cell.

use crate::Mode;
use crate::Workspace;
use crate::view::grid::cell_label_metrics;
use crate::view::paint::build_fill_path;
use crate::view::theme as t;
use crate::workspace::FontViewMode;
use gpui::Bounds;
use gpui::Context;
use gpui::InteractiveElement;
use gpui::IntoElement;
use gpui::ParentElement;
use gpui::SharedString;
use gpui::StatefulInteractiveElement;
use gpui::Styled;
use gpui::canvas;
use gpui::div;
use gpui::prelude::FluentBuilder;
use gpui::px;
use kurbo::Affine;

/// The glyph editing canvas: the scene it gathers and the layers it
/// paints.
mod editor;

impl Workspace {
    /// Builds one grid cell for the glyph at `index`, `cell` by
    /// `cell_h` pixels.
    ///
    /// With `jump_on_click`, a single click opens the glyph instead
    /// of selecting it. The editor sidebar's mini grid sets it.
    pub(crate) fn glyph_cell_sized(
        &self,
        index: usize,
        cell: f32,
        cell_h: f32,
        jump_on_click: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let font = self.font().unwrap();
        let entry = &font.glyphs[index];
        let name = entry.name.clone();
        let unicode_label: Option<SharedString> = entry
            .codepoint
            .map(|c| format!("U+{:04X}", c as u32).into());
        let detail_info: Option<SharedString> =
            (self.grid.view_mode == FontViewMode::Detail && !jump_on_click).then(|| {
                let category = entry
                    .codepoint
                    .map(|c| {
                        runebender_core::analysis::category::GlyphCategory::from_codepoint(c)
                            .display_name()
                    })
                    .unwrap_or("Unencoded");
                format!("{category} · {:.0}", entry.advance).into()
            });
        let selected = if jump_on_click {
            matches!(self.mode, Mode::Editor(i) if i == index)
        } else {
            self.selected == Some(index) || self.grid.multi_selected.contains(name.as_ref())
        };
        let labels = cell_label_metrics(cell);
        let (show_labels, label_px, label_h) = (labels.show, labels.size, labels.height);
        let incompatible = self
            .project
            .as_ref()
            .and_then(|p| p.compat.get(entry.name.as_ref()))
            .is_some_and(|ok| !ok);

        let paint = t::mark_paint(entry.mark.as_deref());
        let mark = paint.as_ref().map(|p| p.ink);
        let _ = font;
        div()
            .id(index)
            .w(px(cell))
            .h(px(cell_h))
            .flex()
            .flex_col()
            .bg(match (selected, paint.as_ref().and_then(|p| p.bg)) {
                (true, _) => t::cell_selected_bg(),
                (false, Some(fill)) => fill,
                (false, None) => t::cell_bg(),
            })
            .border(t::stroke())
            .border_color(if selected {
                t::cell_selected_ring()
            } else {
                paint
                    .as_ref()
                    .map(|p| p.border)
                    .unwrap_or_else(t::cell_border)
            })
            .rounded(t::radius_control())
            .cursor_pointer()
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                // Notes are transient: picking a glyph clears them so
                // the bottom bar's count shows again.
                this.status_note = None;
                if jump_on_click {
                    this.open_editor(index);
                } else {
                    let modifiers = event.modifiers();
                    if modifiers.platform {
                        // Cmd-click toggles membership.
                        this.grid_toggle_multi(index);
                    } else if modifiers.shift {
                        this.grid_extend_multi(index);
                    } else {
                        this.selected = Some(index);
                        this.grid.multi_selected.clear();
                    }
                    if event.click_count() >= 2 {
                        this.open_editor(index);
                    }
                }
                cx.notify();
            }))
            // The outline itself is painted by one canvas over the
            // whole grid, not per cell: gpui ends its render pass at
            // every run of paths, so a canvas per cell meant a pass
            // switch per cell.
            .child(div().flex_1())
            .when(show_labels, |el| {
                el.child(
                    // Same inset left, right and bottom, a little air above,
                    // and the two lines close together (the web's
                    // cell-labels box).
                    div()
                        .h(px(label_h))
                        .pl(px(8.0))
                        .pr(px(8.0))
                        .pb(px(8.0))
                        .pt(px(4.0))
                        .flex()
                        .flex_col()
                        .justify_end()
                        .gap(px(2.0))
                        .text_size(px(label_px))
                        .line_height(px(labels.line))
                        .overflow_hidden()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_color(if selected {
                                    t::cell_selected_ring()
                                } else {
                                    mark.unwrap_or_else(t::text)
                                })
                                .when(incompatible, |el| {
                                    el.child(
                                        div().w(px(6.0)).h(px(6.0)).rounded_full().bg(t::anchor()),
                                    )
                                })
                                .child(SharedString::from(name)),
                        )
                        .when(labels.height >= 40.0, |el| {
                            el.child(
                                div()
                                    .text_color(if selected {
                                        t::cell_selected_ring()
                                    } else {
                                        mark.unwrap_or_else(t::text_muted)
                                    })
                                    .child(unicode_label.unwrap_or_else(|| "".into())),
                            )
                        })
                        // Detail mode's extra line: category and advance,
                        // the Glyphs 4 detail-grid info.
                        .when(detail_info.is_some(), |el| {
                            el.child(
                                div()
                                    .text_color(t::text_muted())
                                    .child(detail_info.clone().unwrap_or_default()),
                            )
                        }),
                )
            })
    }

    /// The List view: one row per glyph, one column per property.
    ///
    /// Click selects, cmd toggles, shift extends, and double-click
    /// opens the editor. Values are the active master's, edited
    /// through the Glyph panel, which already batch-edits a
    /// multi-selection. This is the list view in Glyphs.
    pub(crate) fn glyph_list_view(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(font) = self.font() else {
            return div().into_any_element();
        };
        let order = self.glyph_order();
        const W_UNI: f32 = 68.0;
        const W_NUM: f32 = 52.0;
        const W_GROUP: f32 = 84.0;
        const W_CAT: f32 = 92.0;
        let head = |label: &'static str, w: f32| {
            div()
                .w(px(w))
                .flex_shrink_0()
                .text_xs()
                .text_color(t::text_muted())
                .child(label)
        };
        let list = div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .px_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .py_1()
                    .border_b_1()
                    .border_color(t::panel_outline())
                    .child(div().w(px(14.0)).flex_shrink_0())
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(80.0))
                            .text_xs()
                            .text_color(t::text_muted())
                            .child("Name"),
                    )
                    .child(head("Unicode", W_UNI))
                    .child(head("Width", W_NUM))
                    .child(head("LSB", W_NUM))
                    .child(head("RSB", W_NUM))
                    .child(head("Group L", W_GROUP))
                    .child(head("Group R", W_GROUP))
                    .child(head("Category", W_CAT)),
            );
        let mut rows = div()
            .id("glyph-list")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .flex()
            .flex_col();
        for &index in order.iter() {
            let entry = &font.glyphs[index];
            let name = entry.name.clone();
            let selected =
                self.selected == Some(index) || self.grid.multi_selected.contains(name.as_ref());
            let mark = t::mark_paint(entry.mark.as_deref()).map(|p| p.ink);
            let ink = font.ink_bounds(index);
            let (lsb, rsb) = match ink {
                Some(r) => (
                    format!("{:.0}", r.x0),
                    format!("{:.0}", entry.advance - r.x1),
                ),
                None => (String::new(), String::new()),
            };
            let group = |left: bool| {
                runebender_core::document::font_ops::kern_group(&font.font, name.as_ref(), left)
                    .map(|g| {
                        g.as_str()
                            .replace("public.kern1.", "")
                            .replace("public.kern2.", "")
                    })
                    .unwrap_or_default()
            };
            let category = entry
                .codepoint
                .map(|c| {
                    runebender_core::analysis::category::GlyphCategory::from_codepoint(c)
                        .display_name()
                        .to_string()
                })
                .unwrap_or_else(|| "Unencoded".into());
            let text_color = if selected { t::text() } else { t::text_muted() };
            let cell = |value: String, w: f32| {
                div()
                    .w(px(w))
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(text_color)
                    .overflow_hidden()
                    .child(value)
            };
            rows = rows.child(
                div()
                    .id(("glyph-row", index))
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(24.0))
                    .px_0p5()
                    .rounded(t::radius())
                    .when(selected, |el| el.bg(t::cell_selected_bg()))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                        this.status_note = None;
                        let modifiers = event.modifiers();
                        if modifiers.platform {
                            this.grid_toggle_multi(index);
                        } else if modifiers.shift {
                            this.grid_extend_multi(index);
                        } else {
                            this.selected = Some(index);
                            this.grid.multi_selected.clear();
                        }
                        if event.click_count() >= 2 {
                            this.open_editor(index);
                        }
                        cx.notify();
                    }))
                    .child(
                        div().w(px(14.0)).flex_shrink_0().child(
                            div()
                                .w(px(9.0))
                                .h(px(9.0))
                                .rounded_full()
                                .bg(mark.unwrap_or(gpui::Rgba {
                                    r: 0.0,
                                    g: 0.0,
                                    b: 0.0,
                                    a: 0.0,
                                })),
                        ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(80.0))
                            .text_sm()
                            .text_color(if selected { t::accent() } else { t::text() })
                            .overflow_hidden()
                            .child(SharedString::from(name.clone())),
                    )
                    .child(cell(
                        entry
                            .codepoint
                            .map(|c| format!("U+{:04X}", c as u32))
                            .unwrap_or_default(),
                        W_UNI,
                    ))
                    .child(cell(format!("{:.0}", entry.advance), W_NUM))
                    .child(cell(lsb, W_NUM))
                    .child(cell(rsb, W_NUM))
                    .child(cell(group(true), W_GROUP))
                    .child(cell(group(false), W_GROUP))
                    .child(cell(category, W_CAT)),
            );
        }
        list.child(rows).into_any_element()
    }

    /// The positional-forms matrix, the Arabic review surface.
    ///
    /// One row per base letter that carries positional variants, with
    /// isol/init/medi/fina as columns, each a live thumbnail. Click a
    /// form to open it. A dash marks a missing form. This is Matrix
    /// Mode in Counterpunch.
    pub(crate) fn glyph_matrix_view(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(font) = self.font() else {
            return div().into_any_element();
        };
        // Families: base name → indices of [isol, init, medi, fina].
        let mut families: std::collections::BTreeMap<String, [Option<usize>; 4]> =
            std::collections::BTreeMap::new();
        for (i, entry) in font.glyphs.iter().enumerate() {
            let name = entry.name.as_ref();
            let (base, slot) = if let Some(b) = name.strip_suffix(".init") {
                (b, 1)
            } else if let Some(b) = name.strip_suffix(".medi") {
                (b, 2)
            } else if let Some(b) = name.strip_suffix(".fina") {
                (b, 3)
            } else {
                (name, 0)
            };
            let family = families.entry(base.to_string()).or_default();
            family[slot] = Some(i);
        }
        families.retain(|_, forms| forms[1..].iter().any(Option::is_some));
        if families.is_empty() {
            return div()
                .p_4()
                .text_sm()
                .text_color(t::text_muted())
                .child("No positional forms (.init/.medi/.fina) in this font")
                .into_any_element();
        }
        const THUMB: f32 = 56.0;
        let header = |label: &'static str| {
            div()
                .w(px(THUMB))
                .flex_shrink_0()
                .text_xs()
                .text_color(t::text_muted())
                .child(label)
        };
        let mut rows = div()
            .id("glyph-matrix")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .px_2();
        rows = rows.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .py_1()
                .border_b_1()
                .border_color(t::panel_outline())
                .child(
                    div()
                        .w(px(140.0))
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(t::text_muted())
                        .child("Base"),
                )
                // RTL reading order: isolated at the right end would
                // be truer, but columns read left-to-right here with
                // the joining flow explicit in the labels.
                .child(header("isol"))
                .child(header("init"))
                .child(header("medi"))
                .child(header("fina")),
        );
        for (base, forms) in &families {
            let mut row = div().flex().items_center().gap_2().py_0p5().child(
                div()
                    .w(px(140.0))
                    .flex_shrink_0()
                    .text_sm()
                    .text_color(t::text())
                    .overflow_hidden()
                    .child(base.clone()),
            );
            for (slot, form) in forms.iter().enumerate() {
                row = row.child(match *form {
                    Some(index) => {
                        let entry = &font.glyphs[index];
                        let (path, advance, asc, desc) = (
                            entry.path.clone(),
                            entry.advance,
                            font.ascender,
                            font.descender,
                        );
                        let selected = self.selected == Some(index);
                        div()
                            .id(("matrix-cell", index * 4 + slot))
                            .w(px(THUMB))
                            .h(px(THUMB))
                            .flex_shrink_0()
                            .rounded(t::radius())
                            .border(t::stroke())
                            .border_color(if selected {
                                t::cell_selected_ring()
                            } else {
                                t::cell_border()
                            })
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, _, cx| {
                                this.selected = Some(index);
                                this.grid.multi_selected.clear();
                                if ev.click_count() >= 2 {
                                    this.open_editor(index);
                                }
                                cx.notify();
                            }))
                            .child(
                                canvas(
                                    move |bounds, _, _| bounds,
                                    move |_, bounds: Bounds<gpui::Pixels>, window, _| {
                                        let h: f32 = bounds.size.height.into();
                                        let w: f32 = bounds.size.width.into();
                                        let em = (asc - desc).max(1.0);
                                        let scale =
                                            (h as f64 / em).min(w as f64 / advance.max(1.0));
                                        let ox = (w as f64 - advance * scale) / 2.0;
                                        let baseline = h as f64 + desc * scale;
                                        let view = Affine::translate((ox, baseline))
                                            * Affine::scale_non_uniform(scale, -scale);
                                        if let Some(p) = build_fill_path(&path, view, bounds.origin)
                                        {
                                            window.paint_path(p, t::glyph_fill());
                                        }
                                    },
                                )
                                .size_full(),
                            )
                            .into_any_element()
                    }
                    None => div()
                        .w(px(THUMB))
                        .h(px(THUMB))
                        .flex_shrink_0()
                        .rounded(t::radius())
                        .border(t::stroke())
                        .border_color(t::panel_outline())
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(t::text_muted())
                        .child("–")
                        .into_any_element(),
                });
            }
            rows = rows.child(row);
        }
        rows.into_any_element()
    }
}
