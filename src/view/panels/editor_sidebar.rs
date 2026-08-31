// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! The editor view's left panel: related glyphs, shaping, transforms, curves, background, layers, and the context menu.

use crate::Mode;
use crate::Workspace;
use crate::view::grid::glyph_column_span;
use crate::view::grid::pack_spans;
use crate::view::paint::build_fill_path;
use crate::view::paint::flat_slider;
use crate::view::paint::icon_svg;
use crate::view::panels::Thumb;
use crate::view::render::TabTooltip;
use crate::view::theme as t;
use crate::widgets;
use crate::workspace::BOTTOM_BAR_H;
use crate::workspace::GRID_GAP;
use gpui::AppContext;
use gpui::Bounds;
use gpui::Context;
use gpui::InteractiveElement;
use gpui::IntoElement;
use gpui::MouseButton;
use gpui::ParentElement;
use gpui::SharedString;
use gpui::StatefulInteractiveElement;
use gpui::Styled;
use gpui::canvas;
use gpui::div;
use gpui::prelude::FluentBuilder;
use gpui::px;
use kurbo::Affine;
use runebender_core::formats::lib_keys::read_masks;
use runebender_core::outline::glyph_ops::CurveOp;
impl Workspace {
    /// Editor sidebar: search + scrollable mini glyph grid, so glyph
    /// switching doesn't require leaving the editor.
    pub(crate) fn editor_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let _query = self.sidebar.search_query.clone();
        let fit = self.sidebar_cell_metrics();
        let mut rows_total = 0usize;
        let mut shown = 0usize;
        let matched = self.glyph_order();
        let mut visible_rows: Vec<Vec<(usize, usize)>> = Vec::new();
        let cells: Vec<_> = match self.font() {
            Some(font) => {
                shown = matched.len();
                let upm = font.units_per_em;
                let spans: Vec<(usize, usize)> = matched
                    .iter()
                    .map(|&i| {
                        (
                            i,
                            glyph_column_span(
                                font.glyphs[i].name.as_ref(),
                                font.glyphs[i].advance,
                                upm,
                            ),
                        )
                    })
                    .collect();
                let packed = pack_spans(&spans, fit.cols);
                rows_total = packed.len();
                let start = self.sidebar.scroll_row.min(rows_total.saturating_sub(1));
                visible_rows = packed.iter().skip(start).take(fit.rows).cloned().collect();
                packed
                    .into_iter()
                    .skip(start)
                    .take(fit.rows)
                    .flatten()
                    .map(|(i, span)| {
                        let w = fit.cell_w * span as f32 + GRID_GAP * (span - 1) as f32;
                        self.glyph_cell_sized(i, w, fit.cell_h, true, cx)
                            .into_any_element()
                    })
                    .collect()
            }
            None => Vec::new(),
        };
        // The sidebar's own tabs, like the web's editor sidebar: the
        // glyph list, and the designspace axes.
        let has_axes = !self.axis_sliders.is_empty();
        // Icons, not words: four labels overflowed a narrow sidebar
        // and the last one was clipped. The name comes back on hover.
        let tab = |id: &'static str,
                   label: &'static str,
                   icon: &'static str,
                   which: u8,
                   cx: &mut Context<Self>| {
            let active = self.sidebar.tab == which;
            // Same treatment as the edit-mode toolbar: a filled tile
            // when active, no outline either way.
            div()
                .id(id)
                .size(px(26.0))
                .flex()
                .items_center()
                .justify_center()
                .flex_shrink_0()
                .rounded(t::radius_control())
                .cursor_pointer()
                .when(active, |el| el.bg(t::cell_selected_bg()))
                .child(icon_svg(
                    icon,
                    if active { t::accent() } else { t::text_muted() },
                ))
                .tooltip(move |_, cx| cx.new(|_| TabTooltip { label }).into())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.sidebar.tab = which;
                    cx.notify();
                }))
        };
        // An axis-less font has no Axes tab, so a stale selection
        // falls back to the glyph list.
        let tab_now = if !has_axes && self.sidebar.tab == 2 {
            0
        } else {
            self.sidebar.tab
        };
        let on_glyphs = tab_now == 0;
        div()
            .size_full()
            .flex()
            .flex_col()
            .min_h(px(0.0))
            .child(
                div()
                    .px_2()
                    .pt_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(tab("sidebar-tab-glyphs", "Glyphs", "glyph-grid", 0, cx))
                    .child(tab("sidebar-tab-shapes", "Shapes", "shapes", 1, cx))
                    .when(has_axes, |el| {
                        el.child(tab("sidebar-tab-axes", "Axes", "measure", 2, cx))
                    })
                    .child(tab("sidebar-tab-ai", "Local AI", "preview", 3, cx)),
            )
            .when(tab_now == 1, |el| {
                el.child(
                    div()
                        .id("sidebar-shapes")
                        .flex_1()
                        .min_h(px(0.0))
                        .overflow_y_scroll()
                        .child(self.sidebar_shapes(cx)),
                )
            })
            .when(tab_now == 2, |el| {
                el.child(
                    div()
                        .flex_1()
                        .min_h(px(0.0))
                        .p_2()
                        .children(self.axes_section(cx)),
                )
            })
            .when(tab_now == 3, |el| {
                el.child(
                    div()
                        .id("sidebar-ai")
                        .flex_1()
                        .min_h(px(0.0))
                        .overflow_y_scroll()
                        .p_2()
                        .child(self.local_ai_panel(cx)),
                )
            })
            .when(on_glyphs, |el| {
                el.child(
                    div()
                        .p_2()
                        .flex()
                        .items_stretch()
                        .gap_1()
                        .border_b_1()
                        .border_color(t::panel_outline())
                        .child(
                            div()
                                .flex_1()
                                .child(widgets::input::Input::new(&self.sidebar.search_input)),
                        )
                        .child(self.search_toggle(
                            "search-mode",
                            match self.sidebar.search_mode {
                                1 => "N",
                                2 => "U",
                                _ => "A",
                            },
                            self.sidebar.search_mode != 0,
                            |this| this.sidebar.search_mode = (this.sidebar.search_mode + 1) % 3,
                            cx,
                        ))
                        .child(self.search_toggle(
                            "search-regex",
                            ".*",
                            self.sidebar.search_regex,
                            |this| {
                                this.sidebar.search_regex = !this.sidebar.search_regex;
                                this.rebuild_search_regex();
                            },
                            cx,
                        ))
                        .child(self.search_toggle(
                            "search-case",
                            "Aa",
                            self.sidebar.search_case,
                            |this| {
                                this.sidebar.search_case = !this.sidebar.search_case;
                                this.rebuild_search_regex();
                            },
                            cx,
                        )),
                )
                .child(
                    // Measured the same way the main grid is, so the mini
                    // cells stretch to fill the pane and a row is never
                    // left half-showing at the bottom.
                    div()
                        .flex_1()
                        .min_h(px(0.0))
                        .relative()
                        .child({
                            let this = cx.entity().downgrade();
                            canvas(
                                move |bounds: Bounds<gpui::Pixels>, _, app: &mut gpui::App| {
                                    this.update(app, |this, cx| {
                                        if this.sidebar.viewport != bounds.size {
                                            this.sidebar.viewport = bounds.size;
                                            cx.notify();
                                        }
                                    })
                                    .ok();
                                },
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                        })
                        .child(
                            div()
                                .id("editor-sidebar-grid")
                                .size_full()
                                .min_h(px(0.0))
                                // The outline overlay is absolute: this is
                                // the box it pins to.
                                .relative()
                                .overflow_hidden()
                                .child(
                                    div()
                                        .size_full()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(
                                            div()
                                                .w(px(fit.content_w()))
                                                .flex()
                                                .flex_wrap()
                                                .gap(px(GRID_GAP))
                                                .children(cells),
                                        ),
                                )
                                .children(self.glyph_overlay(visible_rows, fit, cx))
                                .on_scroll_wheel(cx.listener(
                                    move |this, ev: &gpui::ScrollWheelEvent, _, cx| {
                                        let dy = match ev.delta {
                                            gpui::ScrollDelta::Pixels(p) => f32::from(p.y),
                                            gpui::ScrollDelta::Lines(p) => p.y * 24.0,
                                        };
                                        if Self::scroll_grid_rows(
                                            &mut this.sidebar.scroll_row,
                                            dy,
                                            fit.cell_h + GRID_GAP,
                                            fit.rows,
                                            rows_total,
                                        ) {
                                            cx.notify();
                                        }
                                    },
                                )),
                        ),
                )
                .child(
                    // Same bar the main grid has, and the same height, so
                    // the two line up across the divider.
                    div()
                        .h(px(BOTTOM_BAR_H))
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .border_t_1()
                        .border_color(t::panel_outline())
                        .child(
                            div()
                                .flex_1()
                                .text_xs()
                                .text_color(t::text_muted())
                                .child(SharedString::from(format!("{} glyphs", shown))),
                        )
                        .children(
                            self.sidebar
                                .slider
                                .as_ref()
                                .map(|slider| div().w(px(96.0)).child(flat_slider(slider, cx))),
                        ),
                )
            })
            // Colours stay put whichever tab is up.
            .child(self.mark_colors_panel(cx))
    }

    /// Related Glyphs section: the glyph's base, its suffix siblings
    /// (name.*), its components, and every composite using it, each
    /// one click away.
    ///
    /// This is the Related Glyphs panel in Fontra.
    pub(crate) fn related_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let Some(index) = self.current_glyph_index() else {
            return self.section(cx, "Related", div());
        };
        let Some(font) = self.font() else {
            return self.section(cx, "Related", div());
        };
        let name = font.glyphs[index].name.to_string();
        let stem = name.split('.').next().unwrap_or(&name).to_string();
        let mut rows: Vec<(&'static str, Vec<String>)> = Vec::new();
        // Components of this glyph.
        let components: Vec<String> = font
            .font
            .get_glyph(name.as_str())
            .map(|g| g.components.iter().map(|c| c.base.to_string()).collect())
            .unwrap_or_default();
        if !components.is_empty() {
            rows.push(("Components", components));
        }
        // Suffix siblings sharing the stem.
        let siblings: Vec<String> = font
            .glyphs
            .iter()
            .map(|g| g.name.to_string())
            .filter(|other| *other != name && other.split('.').next() == Some(stem.as_str()))
            .take(24)
            .collect();
        if !siblings.is_empty() {
            rows.push(("Siblings", siblings));
        }
        // Composites that place this glyph.
        let used_by: Vec<String> = font
            .glyphs
            .iter()
            .filter(|g| {
                font.font
                    .get_glyph(g.name.as_ref())
                    .is_some_and(|norad_glyph| {
                        norad_glyph
                            .components
                            .iter()
                            .any(|c| c.base.as_str() == name)
                    })
            })
            .map(|g| g.name.to_string())
            .take(24)
            .collect();
        if !used_by.is_empty() {
            rows.push(("Used by", used_by));
        }
        if rows.is_empty() {
            return self.section(
                cx,
                "Related",
                div()
                    .text_xs()
                    .text_color(t::text_muted())
                    .child("No related glyphs"),
            );
        }
        let mut body = div().flex().flex_col().gap_1();
        for (label, names) in rows {
            let mut chips = div().flex().flex_wrap().gap_1();
            for related in names {
                let target = font.name_map.get(related.as_str()).copied();
                chips = chips.child(
                    div()
                        .id(gpui::SharedString::from(format!("rel-{label}-{related}")))
                        .px_1()
                        .rounded(t::radius())
                        .border(t::stroke())
                        .border_color(t::cell_border())
                        .text_xs()
                        .text_color(t::text())
                        .cursor_pointer()
                        .child(related.clone())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(target) = target {
                                this.open_editor(target);
                            }
                            cx.notify();
                        })),
                );
            }
            body = body
                .child(div().text_xs().text_color(t::text_muted()).child(label))
                .child(chips);
        }
        self.section(cx, "Related", body)
    }

    /// Shaping section (editor mode): the buffer's characters in
    /// logical order against the shaped glyphs, cluster-linked.
    ///
    /// Click a chip to cross-highlight its cluster. Double-click a
    /// glyph chip to open that glyph for editing inside the shaped
    /// run. This is Fontra's shaping inspector, on the shared text
    /// engine.
    pub(crate) fn shaping_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        use runebender_core::text::buffer::{TextDirection, TextSortKind};
        let count = self.edit_buffer.len();
        if count < 2 {
            return self.section(
                cx,
                "Shaping",
                div()
                    .text_xs()
                    .text_color(t::text_muted())
                    .child("Type around the glyph to inspect shaping"),
            );
        }
        // Carrier per sort: an absorbed sort (eaten by a ligature)
        // belongs to the last unabsorbed sort before it.
        let mut carrier_of: Vec<usize> = Vec::with_capacity(count);
        let mut last_carrier = 0usize;
        for i in 0..count {
            let absorbed = self.edit_buffer.sort(i).is_some_and(|s| s.is_absorbed());
            if !absorbed {
                last_carrier = i;
            }
            carrier_of.push(last_carrier);
        }
        let focus = self.shaping_focus;
        let chip = |id: (&'static str, usize),
                    label: SharedString,
                    sub: SharedString,
                    lit: bool,
                    dim: bool,
                    cx: &mut Context<Self>,
                    carrier: usize,
                    open_on_double: bool| {
            div()
                .id(id)
                .px_1()
                .py_0p5()
                .rounded(t::radius())
                .border(t::stroke())
                .border_color(if lit { t::accent() } else { t::cell_border() })
                .flex()
                .flex_col()
                .items_center()
                .cursor_pointer()
                .child(
                    div()
                        .text_sm()
                        .text_color(if dim {
                            t::text_muted()
                        } else if lit {
                            t::accent()
                        } else {
                            t::text()
                        })
                        .child(label),
                )
                .child(div().text_xs().text_color(t::text_muted()).child(sub))
                .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, _, cx| {
                    this.shaping_focus = Some(carrier);
                    if open_on_double && ev.click_count() >= 2 {
                        this.edit_buffer.activate_sort(carrier);
                        let name = this
                            .edit_buffer
                            .sort(carrier)
                            .and_then(|s| s.glyph_name())
                            .map(str::to_string);
                        if let Some(glyph) =
                            name.and_then(|n| this.font().and_then(|f| f.name_map.get(&n).copied()))
                        {
                            this.mode = Mode::Editor(glyph);
                            this.selected = Some(glyph);
                            this.editor.selected.clear();
                            this.editor.selected_anchors.clear();
                        }
                        this.sync_sort_offset();
                    }
                    cx.notify();
                }))
        };
        // Characters, logical order.
        let mut chars_row = div().flex().flex_wrap().gap_1();
        for (i, &carrier) in carrier_of.iter().enumerate().take(count) {
            let Some(sort) = self.edit_buffer.sort(i) else {
                continue;
            };
            let TextSortKind::Glyph { codepoint, .. } = &sort.kind else {
                continue;
            };
            let lit = focus == Some(carrier);
            let (label, sub): (SharedString, SharedString) = match codepoint {
                Some(c) => (c.to_string().into(), format!("{:04X}", *c as u32).into()),
                None => ("·".into(), "—".into()),
            };
            chars_row = chars_row.child(chip(
                ("shape-char", i),
                label,
                sub,
                lit,
                sort.is_absorbed(),
                cx,
                carrier,
                false,
            ));
        }
        // Glyphs: the unabsorbed sorts, visually ordered for a
        // single RTL line (Fontra shows output left-to-right).
        let mut glyph_indices: Vec<usize> = (0..count)
            .filter(|&i| {
                self.edit_buffer
                    .sort(i)
                    .is_some_and(|s| !s.is_absorbed() && s.glyph_name().is_some())
            })
            .collect();
        if self.edit_buffer.line_count() == 1
            && self.edit_buffer.resolved_line_direction(0) == TextDirection::RightToLeft
        {
            glyph_indices.reverse();
        }
        let mut glyphs_row = div().flex().flex_wrap().gap_1();
        for &i in &glyph_indices {
            let Some(sort) = self.edit_buffer.sort(i) else {
                continue;
            };
            let TextSortKind::Glyph {
                name,
                advance_width,
                ..
            } = &sort.kind
            else {
                continue;
            };
            let lit = focus == Some(i);
            glyphs_row = glyphs_row.child(chip(
                ("shape-glyph", i),
                name.clone().into(),
                format!("{advance_width:.0}").into(),
                lit,
                false,
                cx,
                i,
                true,
            ));
        }
        // Feature toggles: every feature tag in features.fea, each
        // cycling default → off → on (Fontra's preview switches).
        let mut tags: Vec<String> = self
            .font()
            .map(|f| {
                let mut found = Vec::new();
                let fea = &f.font.features;
                let mut rest = fea.as_str();
                while let Some(at) = rest.find("feature ") {
                    let tail = &rest[at + 8..];
                    let tag: String = tail
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric())
                        .take(4)
                        .collect();
                    if tag.len() == 4 && !found.contains(&tag) {
                        found.push(tag);
                    }
                    rest = tail;
                }
                found
            })
            .unwrap_or_default();
        tags.sort();
        let mut toggles = div().flex().flex_wrap().gap_1();
        for tag in &tags {
            let state = self.feature_overrides.get(tag.as_str()).copied();
            let tag_owned = tag.clone();
            toggles = toggles.child(
                div()
                    .id(gpui::SharedString::from(format!("fea-{tag}")))
                    .px_1p5()
                    .py_0p5()
                    .rounded(t::radius())
                    .border(t::stroke())
                    .text_xs()
                    .cursor_pointer()
                    .border_color(match state {
                        Some(true) => t::accent(),
                        Some(false) => t::annotation(),
                        None => t::cell_border(),
                    })
                    .text_color(match state {
                        Some(true) => t::accent(),
                        Some(false) => t::annotation(),
                        None => t::text_muted(),
                    })
                    .child(match state {
                        Some(false) => format!("{tag} ×"),
                        Some(true) => format!("{tag} ✓"),
                        None => tag.clone(),
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // default → off → on → default.
                        let next = match this.feature_overrides.get(tag_owned.as_str()) {
                            None => Some(false),
                            Some(false) => Some(true),
                            Some(true) => None,
                        };
                        match next {
                            Some(v) => {
                                this.feature_overrides.insert(tag_owned.clone(), v);
                            }
                            None => {
                                this.feature_overrides.remove(tag_owned.as_str());
                            }
                        }
                        let overrides: Vec<(String, bool)> = this
                            .feature_overrides
                            .iter()
                            .map(|(k, v)| (k.clone(), *v))
                            .collect();
                        this.edit_buffer.set_feature_overrides(overrides);
                        this.edit_buffer.shape_arabic_if_rtl();
                        this.sync_sort_offset();
                        cx.notify();
                    })),
            );
        }
        // Language presets: languagesystem-specific rules (Urdu or
        // Sindhi locl, Turkish i) only fire with a language set.
        const LOCALES: [(&str, &str, &str); 8] = [
            ("Urdu", "arab", "ur"),
            ("Sindhi", "arab", "sd"),
            ("Farsi", "arab", "fa"),
            ("Kashmiri", "arab", "ks"),
            ("Turkish", "latn", "tr"),
            ("Dutch", "latn", "nl"),
            ("Romanian", "latn", "ro"),
            ("Vietnamese", "latn", "vi"),
        ];
        let mut locale_chips = div().flex().flex_wrap().gap_1();
        for (li, (label, script, lang)) in LOCALES.iter().enumerate() {
            let lit = self
                .shaping_locale
                .as_ref()
                .is_some_and(|(s, l)| s == script && l == lang);
            locale_chips = locale_chips.child(
                div()
                    .id(("shaping-locale", li))
                    .px_1p5()
                    .py_0p5()
                    .rounded(t::radius())
                    .border(t::stroke())
                    .text_xs()
                    .cursor_pointer()
                    .border_color(if lit { t::accent() } else { t::cell_border() })
                    .text_color(if lit { t::accent() } else { t::text_muted() })
                    .child(*label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let (script, lang) = (LOCALES[li].1.to_string(), LOCALES[li].2.to_string());
                        let already = this
                            .shaping_locale
                            .as_ref()
                            .is_some_and(|(s, l)| *s == script && *l == lang);
                        this.shaping_locale = (!already).then_some((script, lang));
                        let (script, lang) = match &this.shaping_locale {
                            Some((s, l)) => (Some(s.clone()), Some(l.clone())),
                            None => (None, None),
                        };
                        this.edit_buffer.set_shaping_locale(script, lang);
                        this.edit_buffer.shape_arabic_if_rtl();
                        this.sync_sort_offset();
                        cx.notify();
                    })),
            );
        }
        let body = div()
            .flex()
            .flex_col()
            .gap_2()
            .when(!tags.is_empty(), |el| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(t::text_muted())
                        .child("Features (click: default → off → on)"),
                )
                .child(toggles)
            })
            .child(
                div()
                    .text_xs()
                    .text_color(t::text_muted())
                    .child("Language"),
            )
            .child(locale_chips)
            .child(
                div()
                    .text_xs()
                    .text_color(t::text_muted())
                    .child("Characters (logical)"),
            )
            .child(chars_row)
            .child(
                div()
                    .text_xs()
                    .text_color(t::text_muted())
                    .child("Glyphs (visual)"),
            )
            .child(glyphs_row);
        self.section(cx, "Shaping", body)
    }

    /// Transformations section for the right sidebar (editor mode).
    pub(crate) fn transform_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let text_op = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_2()
                .py_0p5()
                .rounded(t::radius())
                .text_sm()
                .text_color(t::text())
                .cursor_pointer()
                .border(t::stroke())
                .border_color(t::cell_border())
                .child(label)
        };
        self.section(
            cx,
            "Transformations",
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child(
                            Self::icon_tile("op-flip-h", "flip-h", false).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.apply_transform(Affine::scale_non_uniform(-1.0, 1.0));
                                    cx.notify();
                                },
                            )),
                        )
                        .child(
                            Self::icon_tile("op-flip-v", "flip-v", false).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.apply_transform(Affine::scale_non_uniform(1.0, -1.0));
                                    cx.notify();
                                },
                            )),
                        )
                        .child(Self::icon_tile("op-rot-ccw", "rot-ccw", false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.apply_transform(Affine::rotate(std::f64::consts::FRAC_PI_2));
                                cx.notify();
                            }),
                        ))
                        .child(
                            Self::icon_tile("op-rot-cw", "rot-cw", false).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.apply_transform(Affine::rotate(
                                        -std::f64::consts::FRAC_PI_2,
                                    ));
                                    cx.notify();
                                },
                            )),
                        )
                        .child(
                            Self::icon_tile("op-duplicate", "duplicate", false).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.command_duplicate();
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(
                            Self::icon_tile("op-duplicate-repeat", "duplicate-repeat", false)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.command_duplicate_repeat();
                                    cx.notify();
                                })),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child(
                            Self::icon_tile("op-union", "union", false).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.command_boolean(linesweeper::BinaryOp::Union);
                                    cx.notify();
                                },
                            )),
                        )
                        .child(Self::icon_tile("op-subtract", "subtract", false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.command_boolean(linesweeper::BinaryOp::Difference);
                                cx.notify();
                            }),
                        ))
                        .child(
                            Self::icon_tile("op-intersect", "intersect", false).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.command_boolean(linesweeper::BinaryOp::Intersection);
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(Self::icon_tile("op-exclude", "exclude", false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.command_boolean(linesweeper::BinaryOp::Xor);
                                cx.notify();
                            }),
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .child(text_op("op-harmonize", "Harmonize").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.apply_curve_op(CurveOp::Harmonize);
                                cx.notify();
                            },
                        )))
                        .child(text_op("op-balance", "Balance").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.apply_curve_op(CurveOp::Balance);
                                cx.notify();
                            },
                        )))
                        .child(text_op("op-optimize", "Optimize").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.apply_curve_op(CurveOp::Optimize(0.12));
                                cx.notify();
                            },
                        )))
                        .child(text_op("op-extremes", "Extremes").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.command_add_extremes();
                                cx.notify();
                            },
                        )))
                        .child(text_op("op-round", "Round").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.command_round_corners();
                                cx.notify();
                            },
                        )))
                        .child(text_op("op-reverse", "Reverse").on_click(cx.listener(
                            |this, _, _, cx| {
                                if let Mode::Editor(index) = this.mode {
                                    this.push_undo_snapshot(index);
                                    let selected = this.editor.selected.clone();
                                    let changed = this
                                        .font_mut()
                                        .and_then(|f| {
                                            f.edit_glyph(index, |g| {
                                                runebender_core::outline::glyph_ops::reverse_contours(
                                                    g, &selected,
                                                )
                                            })
                                        })
                                        .unwrap_or(false);
                                    if !changed {
                                        this.editor.undo.pop();
                                    } else {
                                        this.editor.selected.clear();
                                    }
                                }
                                cx.notify();
                            },
                        ))),
                )
                // Slanter: shear the selection (or the whole glyph)
                // by an angle typed in degrees. Enter applies;
                // positive leans right, the italic convention.
                // Stroke: expand the selected contours (or all) into
                // stroked outlines of the typed width.
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().text_xs().text_color(t::text_muted()).child("Slant"))
                        .child(
                            div()
                                .w(px(64.0))
                                .child(widgets::input::Input::new(&self.inputs.slant)),
                        )
                        .child(div().text_xs().text_color(t::text_muted()).child("Stroke"))
                        .child(
                            div()
                                .w(px(64.0))
                                .child(widgets::input::Input::new(&self.inputs.stroke)),
                        ),
                )
                // Offset: the whole glyph bolder (+) or lighter (−).
                // Extrude sweeps a shadow ("offset,angle"); Roughen
                // flattens and jitters ("segment,h,v").
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().text_xs().text_color(t::text_muted()).child("Offset"))
                        .child(
                            div()
                                .w(px(64.0))
                                .child(widgets::input::Input::new(&self.inputs.offset)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().text_xs().text_color(t::text_muted()).child("Extrude"))
                        .child(
                            div()
                                .w(px(64.0))
                                .child(widgets::input::Input::new(&self.inputs.extrude)),
                        )
                        .child(div().text_xs().text_color(t::text_muted()).child("Roughen"))
                        .child(
                            div()
                                .w(px(64.0))
                                .child(widgets::input::Input::new(&self.inputs.roughen)),
                        ),
                ),
        )
    }

    /// Curves section: the comb and continuity toggles. This is the
    /// web editor's `CurvePanel`.
    pub(crate) fn curves_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let toggle = |id: &'static str,
                      label: &'static str,
                      active: bool,
                      cx: &mut Context<Self>,
                      on: fn(&mut Self)| {
            div()
                .id(id)
                .px_2()
                .py_0p5()
                .rounded(t::radius())
                .text_sm()
                .cursor_pointer()
                .border(t::stroke())
                .when(active, |el| {
                    el.border_color(t::accent()).text_color(t::accent())
                })
                .when(!active, |el| {
                    el.border_color(t::cell_border()).text_color(t::text())
                })
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    on(this);
                    cx.notify();
                }))
        };
        let body = div()
            .flex()
            .gap_1()
            .child(toggle(
                "curve-comb",
                "Curvature comb",
                self.curve_comb,
                cx,
                |this| this.curve_comb = !this.curve_comb,
            ))
            .child(toggle(
                "curve-continuity",
                "Continuity G0–G3",
                self.curve_continuity,
                cx,
                |this| this.curve_continuity = !this.curve_continuity,
            ));
        // Fit Curve: type a percentage, Enter sets the selected
        // segments' handles to that fraction of their maximum (100 =
        // handles at the tangent intersection), Glyphs' scale.
        let body = div().flex().flex_col().gap_2().child(body).child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(t::text_muted())
                        .child("Fit Curve"),
                )
                .child(
                    div()
                        .w(px(64.0))
                        .child(widgets::input::Input::new(&self.inputs.fit)),
                ),
        );
        self.section(cx, "Curves", body)
    }

    /// Background section: show/send/swap/clear plus the reference
    /// glyph. This is the web editor's Background block.
    pub(crate) fn background_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let button = |id: &'static str,
                      label: &'static str,
                      active: bool,
                      cx: &mut Context<Self>,
                      on: fn(&mut Self)| {
            div()
                .id(id)
                .px_2()
                .py_0p5()
                .rounded(t::radius())
                .text_sm()
                .cursor_pointer()
                .border(t::stroke())
                .when(active, |el| {
                    el.border_color(t::accent()).text_color(t::accent())
                })
                .when(!active, |el| {
                    el.border_color(t::cell_border()).text_color(t::text())
                })
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    on(this);
                    cx.notify();
                }))
        };
        let body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .child(button(
                        "bg-show",
                        "Show background",
                        self.show_background,
                        cx,
                        |this| this.show_background = !this.show_background,
                    ))
                    .child(button(
                        "mark-cloud",
                        "Mark cloud",
                        self.show_mark_cloud,
                        cx,
                        |this| this.show_mark_cloud = !this.show_mark_cloud,
                    ))
                    .child(button("bg-send", "Send to background", false, cx, |this| {
                        this.command_send_to_background()
                    }))
                    .child(button("bg-swap", "Swap", false, cx, |this| {
                        this.command_swap_background()
                    }))
                    .child(button("bg-clear", "Clear", false, cx, |this| {
                        this.command_clear_background()
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(t::text_muted())
                            .child("Reference"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(widgets::input::Input::new(&self.inputs.reference_glyph)),
                    ),
            );
        self.section(cx, "Background", body)
    }

    /// Layers section: one row per master, the active one highlighted.
    pub(crate) fn layers_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let (names, active): (Vec<SharedString>, usize) = match &self.project {
            Some(p) => (
                p.master_names
                    .iter()
                    .cloned()
                    .map(SharedString::from)
                    .collect(),
                p.active,
            ),
            None => (Vec::new(), 0),
        };
        let reference = self.reference_layers.clone();
        // A thumbnail of the current glyph in each master, the web
        // MasterToolbar's glyph buttons relocated into this section.
        let glyph_name: Option<String> = self
            .selected
            .and_then(|i| self.font().map(|f| f.glyphs[i].name.to_string()));
        let thumbs: Vec<Option<Thumb>> = match (&self.project, &glyph_name) {
            (Some(p), Some(name)) => p
                .masters
                .iter()
                .map(|m| {
                    m.name_map.get(name).map(|&g| {
                        (
                            m.glyphs[g].path.clone(),
                            m.glyphs[g].advance,
                            m.ascender,
                            m.descender,
                        )
                    })
                })
                .collect(),
            _ => Vec::new(),
        };
        let rows: Vec<_> = names
            .into_iter()
            .enumerate()
            .map(|(i, name)| {
                let is_active = i == active;
                let eye_on = reference.contains(&i);
                let thumb = thumbs.get(i).cloned().flatten();
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .children(thumb.map(|(path, advance, asc, desc)| {
                        div()
                            .id(("layer-thumb", i))
                            .w(px(22.0))
                            .h(px(22.0))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.reference_layers.remove(&i) {
                                    this.reference_layers.insert(i);
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
                    }))
                    .child(
                        // The active master reads in the accent, like a
                        // picked category or tab. Clicking the
                        // thumbnail beside it toggles that master as a
                        // dim reference underlay — the dot that used to
                        // carry that is gone.
                        div()
                            .id(("layer", i))
                            .h(px(20.0))
                            .flex_1()
                            .px_1()
                            .flex()
                            .items_center()
                            .rounded(t::radius())
                            .text_sm()
                            .cursor_pointer()
                            .when(is_active, |el| {
                                el.border(t::stroke())
                                    .border_color(t::accent())
                                    .text_color(t::accent())
                            })
                            .when(!is_active && eye_on, |el| el.text_color(t::text()))
                            .when(!is_active && !eye_on, |el| el.text_color(t::text_muted()))
                            .child(name)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.switch_master(i);
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        let mut body = div().flex().flex_col().children(rows);
        // Per-glyph layers: any other UFO layer holding this glyph.
        // Eye = underlay, arrows = swap with the drawing, × = drop.
        if let (Some(font), Some(name)) = (
            self.font(),
            self.current_glyph_index()
                .and_then(|i| self.font().map(|f| f.glyphs[i].name.to_string())),
        ) {
            let layers = Self::glyph_layer_names(&font.font, &name);
            if !layers.is_empty() {
                body = body.child(
                    div()
                        .mt_1()
                        .text_xs()
                        .text_color(t::text_muted())
                        .child("Glyph Layers"),
                );
            }
            for (i, layer) in layers.into_iter().enumerate() {
                let eye_on = self.visible_glyph_layers.contains(&layer);
                let (l_eye, l_swap, l_del) = (layer.clone(), layer.clone(), layer.clone());
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .id(("glyph-layer-eye", i))
                                .w(px(20.0))
                                .text_sm()
                                .cursor_pointer()
                                .text_color(if eye_on { t::accent() } else { t::text_muted() })
                                .child("◉")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if !this.visible_glyph_layers.remove(&l_eye) {
                                        this.visible_glyph_layers.insert(l_eye.clone());
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .truncate()
                                .text_sm()
                                .text_color(t::text())
                                .child(layer.clone()),
                        )
                        .child(
                            div()
                                .id(("glyph-layer-swap", i))
                                .px_1()
                                .text_sm()
                                .cursor_pointer()
                                .text_color(t::text_muted())
                                .hover(|el| el.text_color(t::text()))
                                .child("⇅")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.command_swap_layer(&l_swap);
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .id(("glyph-layer-del", i))
                                .px_1()
                                .text_sm()
                                .cursor_pointer()
                                .text_color(t::text_muted())
                                .hover(|el| el.text_color(t::text()))
                                .child("×")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.command_delete_layer_glyph(&l_del);
                                    cx.notify();
                                })),
                        ),
                );
            }
            body = body.child(
                div()
                    .flex()
                    .gap_1()
                    .child(
                        div()
                            .id("glyph-layer-backup")
                            .mt_1()
                            .px_2()
                            .py_0p5()
                            .rounded(t::radius())
                            .text_sm()
                            .cursor_pointer()
                            .border(t::stroke())
                            .border_color(t::cell_border())
                            .text_color(t::text())
                            .child("+ Backup")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.command_backup_layer();
                                cx.notify();
                            })),
                    )
                    .child(
                        // A brace layer: freeze the interpolation at
                        // the preview location as a sparse master.
                        div()
                            .id("glyph-layer-brace")
                            .mt_1()
                            .px_2()
                            .py_0p5()
                            .rounded(t::radius())
                            .text_sm()
                            .cursor_pointer()
                            .border(t::stroke())
                            .border_color(t::cell_border())
                            .text_color(t::text())
                            .child("+ Intermediate")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.command_brace_layer();
                                cx.notify();
                            })),
                    ),
            );
        }
        self.section(cx, "Masters", body)
    }

    /// The context-menu overlay, absolutely positioned inside the
    /// editor container.
    pub(crate) fn context_menu_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<gpui::Div>> {
        let menu = self.context_menu.as_ref()?;
        let item = |id: (&'static str, usize),
                    label: SharedString,
                    action: &'static str,
                    cx: &mut Context<Self>| {
            div()
                .id(id)
                .px_3()
                .py_1()
                .text_sm()
                .text_color(t::text())
                .cursor_pointer()
                .hover(|el| el.bg(t::cell_selected_bg()))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.context_menu_action(action);
                    cx.notify();
                }))
        };
        let mut list = div().flex().flex_col().py_1();
        // Component items first: when you right-click a component,
        // that is what you meant, and the lock is the thing you reach
        // for most while placing marks.
        match menu.component {
            Some((_, true)) => {
                list = list.child(item(
                    ("cm", 0),
                    "Unlock from Anchor".into(),
                    "unlock-component",
                    cx,
                ));
            }
            Some((_, false)) => {
                list = list.child(item(
                    ("cm", 0),
                    "Lock to Anchor".into(),
                    "lock-component",
                    cx,
                ));
            }
            None => {}
        }
        if menu.component.is_some() {
            list = list.child(item(
                ("cm", 1),
                "Decompose Component".into(),
                "decompose-component",
                cx,
            ));
        } else if menu.has_components {
            list = list.child(item(
                ("cm", 1),
                "Decompose Components".into(),
                "decompose-all",
                cx,
            ));
        }
        if menu.adding_component {
            list = list.child(
                div()
                    .px_3()
                    .py_1()
                    .w(px(180.0))
                    .child(widgets::input::Input::new(&self.inputs.component_name)),
            );
        } else {
            list = list.child(item(
                ("cm", 2),
                "Add Component…".into(),
                "add-component",
                cx,
            ));
        }
        if menu.applying_corner {
            list = list.child(
                div()
                    .px_3()
                    .py_1()
                    .w(px(180.0))
                    .child(widgets::input::Input::new(&self.inputs.corner_name)),
            );
        } else if menu.start_point.is_some() {
            list = list.child(item(("cm", 14), "Apply Corner…".into(), "apply-corner", cx));
        }
        if menu.contour.is_some() {
            list = list.child(item(
                ("cm", 21),
                "Insert Node Here".into(),
                "node-insert",
                cx,
            ));
        }
        if let Some((ci, _)) = menu.start_point {
            let open_contour = self
                .current_glyph_index()
                .zip(self.font())
                .and_then(|(i, f)| {
                    f.font
                        .get_glyph(f.glyphs[i].name.as_ref())?
                        .contours
                        .get(ci)
                        .map(|c| {
                            c.points
                                .first()
                                .is_some_and(|p| p.typ == norad::PointType::Move)
                        })
                })
                .unwrap_or(false);
            list = list.child(item(
                ("cm", 22),
                if open_contour {
                    "Close Contour"
                } else {
                    "Open Contour Here"
                }
                .into(),
                "contour-open-close",
                cx,
            ));
        }
        if let Some(node) = menu.start_point {
            let locked = self.editor.locked_points.contains(&node);
            list = list.child(item(
                ("cm", 19),
                if locked { "Unlock Node" } else { "Lock Node" }.into(),
                "node-lock",
                cx,
            ));
        }
        if !self.editor.locked_points.is_empty() {
            list = list.child(item(
                ("cm", 20),
                "Unlock All Nodes".into(),
                "node-unlock-all",
                cx,
            ));
        }
        if menu.contour.is_some() {
            let is_mask = menu
                .contour
                .zip(self.font())
                .and_then(|(ci, f)| {
                    let g = f
                        .font
                        .get_glyph(f.glyphs[self.current_glyph_index()?].name.as_ref())?;
                    Some(read_masks(g).contains(&ci))
                })
                .unwrap_or(false);
            list = list.child(item(
                ("cm", 18),
                if is_mask { "Remove Mask" } else { "Make Mask" }.into(),
                "mask-toggle",
                cx,
            ));
        }
        if menu.start_point.is_some() {
            list = list.child(item(("cm", 3), "Set Start Point".into(), "set-start", cx));
        }
        if menu.contour.is_some() {
            list = list.child(item(("cm", 4), "Reverse Contour".into(), "reverse", cx));
        }
        if !self.editor.selected.is_empty() {
            list = list.child(item(("cm", 5), "Round Corners".into(), "round-corners", cx));
        }
        if let Some(ci) = menu.contour {
            if ci > 0 {
                list = list.child(item(
                    ("cm", 6),
                    format!("Move Contour Up ({ci} → {})", ci - 1).into(),
                    "move-up",
                    cx,
                ));
            }
            if ci + 1 < menu.contour_count {
                list = list.child(item(
                    ("cm", 7),
                    format!("Move Contour Down ({ci} → {})", ci + 1).into(),
                    "move-down",
                    cx,
                ));
            }
        }
        list = list.child(item(("cm", 8), "Add Anchor Here".into(), "add-anchor", cx));
        if menu.anchor.is_some() {
            list = list.child(item(("cm", 9), "Delete Anchor".into(), "delete-anchor", cx));
        }
        if menu.adding_note {
            list = list.child(
                div()
                    .px_3()
                    .py_1()
                    .w(px(200.0))
                    .child(widgets::input::Input::new(&self.inputs.annotation)),
            );
        } else if menu.annotation.is_some() {
            list = list.child(item(
                ("cm", 15),
                "Delete Annotation".into(),
                "annotation-delete",
                cx,
            ));
        } else {
            list = list.child(item(
                ("cm", 15),
                "Annotate: Arrow".into(),
                "annotation-arrow",
                cx,
            ));
            list = list.child(item(
                ("cm", 16),
                "Annotate: Circle".into(),
                "annotation-circle",
                cx,
            ));
            list = list.child(item(
                ("cm", 17),
                "Annotate: Note…".into(),
                "annotation-note",
                cx,
            ));
        }
        match menu.guide {
            Some((true, _)) => {
                list = list.child(item(
                    ("cm", 10),
                    "Delete Local Guide".into(),
                    "guide-delete",
                    cx,
                ));
            }
            Some((false, _)) => {
                list = list.child(item(("cm", 10), "Delete Guide".into(), "guide-delete", cx));
            }
            None => {
                list = list.child(item(("cm", 10), "Add Guide Here".into(), "guide-add-h", cx));
                list = list.child(item(
                    ("cm", 11),
                    "Add Vertical Guide Here".into(),
                    "guide-add-v",
                    cx,
                ));
                list = list.child(item(
                    ("cm", 12),
                    "Add Local Guide Here".into(),
                    "guide-add-local-h",
                    cx,
                ));
                list = list.child(item(
                    ("cm", 13),
                    "Add Local Vertical Guide Here".into(),
                    "guide-add-local-v",
                    cx,
                ));
            }
        }
        Some(
            div()
                .id("context-menu")
                .absolute()
                .left(menu.at.x)
                .top(menu.at.y)
                // Clicks inside the menu must not reach the canvas:
                // its mouse-down would dismiss the menu before the
                // item's click fires (and start a marquee besides).
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _, _, cx| {
                        cx.stop_propagation();
                    }),
                )
                .bg(t::panel_bg())
                .border(t::stroke())
                .border_color(t::panel_outline())
                .rounded(t::radius_control())
                .shadow_md()
                .min_w(px(180.0))
                .child(list),
        )
    }
}
