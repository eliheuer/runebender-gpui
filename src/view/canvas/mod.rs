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
use crate::view::theme as t;
use gpui::Context;
use gpui::InteractiveElement;
use gpui::IntoElement;
use gpui::ParentElement;
use gpui::SharedString;
use gpui::StatefulInteractiveElement;
use gpui::Styled;
use gpui::div;
use gpui::prelude::FluentBuilder;
use gpui::px;

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
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement + use<> {
        let font = self
            .font()
            .expect("a cell is only built while a font is open");
        let entry = &font.glyphs[index];
        let name = entry.name.clone();
        let unicode_label: Option<SharedString> = entry
            .codepoint
            .map(|c| format!("U+{:04X}", c as u32).into());
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
                                    t::cell_selected_ink()
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
                                        t::cell_selected_ink()
                                    } else {
                                        mark.unwrap_or_else(t::text_muted)
                                    })
                                    .child(unicode_label.unwrap_or_else(|| "".into())),
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
    pub(crate) fn glyph_list_view(&self, cx: &mut Context<'_, Self>) -> gpui::AnyElement {
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
            .track_scroll(&self.grid.list_scroll)
            .flex()
            .flex_col();
        for &index in order.iter() {
            let entry = &font.glyphs[index];
            let name = entry.name.clone();
            let selected =
                self.selected == Some(index) || self.grid.multi_selected.contains(name.as_ref());
            let mark = t::mark_paint(entry.mark.as_deref());
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
                    // The mark, painted the way the grid paints it: the
                    // same fill and the same keyline, on a small cell.
                    .child(
                        div().w(px(14.0)).flex_shrink_0().child(
                            div()
                                .w(px(12.0))
                                .h(px(12.0))
                                .rounded(t::radius())
                                .when_some(mark.as_ref(), |el, p| {
                                    el.bg(p.bg.unwrap_or(p.ink))
                                        .border(t::stroke())
                                        .border_color(p.border)
                                }),
                        ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(80.0))
                            .text_color(if selected {
                                t::cell_selected_ink()
                            } else {
                                t::text()
                            })
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
}
