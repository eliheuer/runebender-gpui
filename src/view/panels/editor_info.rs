// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! The editor view's right panel: one section per group of fields, from features to axes.

use super::*;

impl Workspace {
    /// The floating info panel Glyphs puts at the bottom of the edit
    /// view: the glyph's name and codepoint, its sidebearings and
    /// width, its kerning groups, and — while something is selected —
    /// the selection's position and size.
    pub(crate) fn editor_info_panel(
        &self,
        index: usize,
        _cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        if self.editor.tool == Tool::Preview {
            return div().into_any_element();
        }
        let Some(font) = self.font() else {
            return div().into_any_element();
        };
        let entry = &font.glyphs[index];
        let name: SharedString = entry.name.to_string().into();
        let unicode: SharedString = entry
            .codepoint
            .map(|c| format!("{:04X}", c as u32))
            .unwrap_or_default()
            .into();
        let group_l =
            runebender_core::document::font_ops::kern_group(&font.font, entry.name.as_ref(), true)
                .map(|g| g.as_str().replace("public.kern1.", ""))
                .unwrap_or_default();
        let group_r =
            runebender_core::document::font_ops::kern_group(&font.font, entry.name.as_ref(), false)
                .map(|g| g.as_str().replace("public.kern2.", ""))
                .unwrap_or_default();

        // One card, built on a 6px rhythm: an 8px inset on every side,
        // 6px between rows, and a header band the same height as the
        // fields under it.
        const CARD_PAD: f32 = 8.0;
        const CARD_GAP: f32 = 6.0;
        const CARD_RADIUS: f32 = 6.0;
        const HEADER_H: f32 = 22.0;
        let card = || {
            div()
                .rounded(px(CARD_RADIUS))
                .border(t::stroke())
                .border_color(t::panel_outline())
                .bg(t::panel_bg())
                .flex()
                .flex_col()
        };
        let label = |text: SharedString| div().text_xs().text_color(t::text_muted()).child(text);
        let metric = |input: &gpui::Entity<widgets::input::InputState>| {
            div()
                .w(px(64.0))
                .child(widgets::input::Input::new(input).small())
        };

        let metrics = card()
            .child(
                // Header: the glyph on the left, its codepoint on the
                // right. A quiet band, not a colour statement — the
                // corners follow the card's radius so nothing pokes
                // out past the border.
                div()
                    .h(px(HEADER_H))
                    .px(px(CARD_PAD))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .rounded_t(px(CARD_RADIUS - 1.0))
                    .bg(t::cell_selected_bg())
                    .border_b_1()
                    .border_color(t::panel_outline())
                    .text_sm()
                    .text_color(t::text())
                    .child(name)
                    .child(div().text_color(t::text_muted()).child(unicode)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(CARD_GAP))
                    .p(px(CARD_PAD))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(CARD_GAP))
                            .child(label("LSB".into()))
                            .child(metric(&self.inputs.metric.lsb))
                            .child(metric(&self.inputs.metric.width))
                            .child(metric(&self.inputs.metric.rsb))
                            .child(label("RSB".into())),
                    )
                    .child(
                        // Kerning groups sit under the sidebearing they
                        // apply to, the way Glyphs stacks them.
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(label(SharedString::from(group_l)))
                            .child(label(SharedString::from(group_r))),
                    ),
            );

        let selection = self.selection_bounds().map(|r| {
            let readout = |name: &'static str, value: f64| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(CARD_GAP))
                    .child(
                        div()
                            .w(px(10.0))
                            .text_xs()
                            .text_color(t::text_muted())
                            .child(name),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(t::text())
                            .child(SharedString::from(format!("{value:.0}"))),
                    )
            };
            card()
                .child(
                    div()
                        .h(px(HEADER_H))
                        .px(px(CARD_PAD))
                        .flex()
                        .items_center()
                        .rounded_t(px(CARD_RADIUS - 1.0))
                        .bg(t::cell_selected_bg())
                        .border_b_1()
                        .border_color(t::panel_outline())
                        .text_sm()
                        .text_color(t::text_muted())
                        .child("Selection"),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(CARD_PAD * 2.0))
                        .p(px(CARD_PAD))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(CARD_GAP))
                                .child(readout("X", r.x0))
                                .child(readout("Y", r.y0)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(CARD_GAP))
                                .child(readout("W", r.width()))
                                .child(readout("H", r.height())),
                        ),
                )
        });

        div()
            .absolute()
            .bottom(px(12.0))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .items_end()
            .gap_2()
            .child(metrics)
            .children(selection)
            .into_any_element()
    }

    /// The Features section (grid mode): the active master's
    /// features.fea in a plain editor, Apply and Revert below, and
    /// the compile verdict. Glyphs' Features tab, one file at a time
    /// (UFO keeps prefixes, classes, and features in features.fea).
    pub(crate) fn features_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        if self.project.is_none() {
            return self.section(cx, "Features", div());
        }
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_2()
                .py_0p5()
                .rounded(t::radius())
                .text_sm()
                .cursor_pointer()
                .border(t::stroke())
                .border_color(t::cell_border())
                .text_color(t::text())
                .child(label)
        };
        let body = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .h(px(260.0))
                    .child(widgets::input::Input::new(&self.inputs.features).h_full()),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .items_center()
                    .child(button("features-apply", "Apply").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.command_apply_features(cx);
                            cx.notify();
                        },
                    )))
                    .child(button("features-revert", "Revert").on_click(cx.listener(
                        |this, _, window, cx| {
                            this.features_edited = false;
                            this.features_status = None;
                            this.refresh_features_input(true, window, cx);
                            cx.notify();
                        },
                    )))
                    .child(
                        button("features-generate", "Generate").on_click(cx.listener(
                            |this, _, window, cx| {
                                this.command_generate_features(window, cx);
                                cx.notify();
                            },
                        )),
                    )
                    .when(self.features_edited, |el| {
                        el.child(
                            div()
                                .text_xs()
                                .text_color(t::status_yellow())
                                .child("edited"),
                        )
                    }),
            )
            .children(
                self.features_status
                    .clone()
                    .map(|status| div().text_xs().text_color(t::text_muted()).child(status)),
            );
        self.section(cx, "Features", body)
    }

    /// Groups section (grid mode): the kerning groups as shelves —
    /// members as chips with removal, and '+ sel' adds the grid
    /// selection (the Glyphs 4 visual groups shelf, click-to-assign
    /// instead of drag for now). The field creates a group from the
    /// selection: 'o' for kern1, '|o' for kern2.
    pub(crate) fn groups_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let Some(font) = self.font() else {
            return self.section(cx, "Groups", div());
        };
        let mut rows = div().flex().flex_col().gap_1();
        let mut shown = 0usize;
        for (full, members) in font.font.groups.iter() {
            let name = full.as_str();
            let (side, short) = if let Some(s) = name.strip_prefix("public.kern1.") {
                ("L", s)
            } else if let Some(s) = name.strip_prefix("public.kern2.") {
                ("R", s)
            } else {
                continue;
            };
            shown += 1;
            if shown > 40 {
                break;
            }
            let full_owned = name.to_string();
            let short_owned = short.to_string();
            let side_first = side == "L";
            let mut chips = div().flex().flex_wrap().gap_1();
            for member in members.iter().take(24) {
                let member_owned = member.to_string();
                let full_for_chip = full_owned.clone();
                chips = chips.child(
                    div()
                        .id(gpui::SharedString::from(format!("grp-{name}-{member}")))
                        .px_1()
                        .rounded(t::radius())
                        .border(t::stroke())
                        .border_color(t::cell_border())
                        .text_xs()
                        .text_color(t::text())
                        .cursor_pointer()
                        .child(member.to_string())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.command_remove_from_group(&full_for_chip, &member_owned);
                            cx.notify();
                        })),
                );
            }
            if members.len() > 24 {
                chips = chips.child(
                    div()
                        .text_xs()
                        .text_color(t::text_muted())
                        .child(format!("+{}", members.len() - 24)),
                );
            }
            rows = rows.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t::accent())
                                    .child(format!("@{short} · {side}")),
                            )
                            .child(
                                div()
                                    .id(gpui::SharedString::from(format!("grp-add-{name}")))
                                    .px_1()
                                    .rounded(t::radius())
                                    .text_xs()
                                    .cursor_pointer()
                                    .border(t::stroke())
                                    .border_color(t::cell_border())
                                    .text_color(t::text_muted())
                                    .child("+ sel")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.command_add_selection_to_group(
                                            side_first,
                                            &short_owned,
                                        );
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(chips),
            );
        }
        let body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(widgets::input::Input::new(&self.inputs.group_name))
            .child(rows)
            .child(
                div()
                    .text_xs()
                    .text_color(t::text_muted())
                    .child("Chip removes · + sel adds the grid selection"),
            );
        self.section(cx, "Groups", body)
    }

    /// Kerning section (grid mode): every pair on the active master,
    /// filtered by the search field, with an editor row that commits
    /// on Enter. Glyphs keeps this in its kerning window; the drag
    /// workflow in text mode stays the fast path.
    pub(crate) fn kerning_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let Some(font) = self.font() else {
            return self.section(cx, "Kerning", div());
        };
        let filter = self
            .inputs
            .kern
            .filter
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        let mut pairs: Vec<(String, String, f64)> = Vec::new();
        let mut hidden = 0usize;
        const CAP: usize = 200;
        for (first, seconds) in font.font.kerning.iter() {
            for (second, value) in seconds.iter() {
                if !filter.is_empty()
                    && !first.as_str().to_lowercase().contains(&filter)
                    && !second.as_str().to_lowercase().contains(&filter)
                {
                    continue;
                }
                if pairs.len() >= CAP {
                    hidden += 1;
                    continue;
                }
                pairs.push((first.to_string(), second.to_string(), *value));
            }
        }
        let total = pairs.len() + hidden;
        let field = |input: &gpui::Entity<widgets::input::InputState>| {
            div().flex_1().child(widgets::input::Input::new(input))
        };
        let editor_row = div()
            .flex()
            .gap_1()
            .child(field(&self.inputs.kern.first))
            .child(field(&self.inputs.kern.second))
            .child(field(&self.inputs.kern.value));
        let mut list = div()
            .id("kerning-pairs")
            .max_h(px(220.0))
            .overflow_y_scroll()
            .flex()
            .flex_col();
        for (i, (first, second, value)) in pairs.iter().enumerate() {
            let (f2, s2) = (first.clone(), second.clone());
            let (f3, s3, v3) = (first.clone(), second.clone(), *value);
            list = list.child(
                div()
                    .id(("kern-pair", i))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1()
                    .py_0p5()
                    .text_xs()
                    .cursor_pointer()
                    .hover(|el| el.bg(t::cell_selected_bg()))
                    // Clicking a row loads it into the editor row, so
                    // adjusting an existing pair is click, type, Enter.
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let sets = [
                            (&this.inputs.kern.first, f3.clone()),
                            (&this.inputs.kern.second, s3.clone()),
                            (&this.inputs.kern.value, format!("{v3}")),
                        ];
                        for (entity, value) in sets {
                            entity.clone().update(cx, |st, cx| {
                                st.set_value(value, window, cx);
                            });
                        }
                    }))
                    .child({
                        // Groups read as @name in the accent; raw
                        // glyph pairs — exceptions — in the warning
                        // yellow, the Glyphs kerning window's code.
                        let is_group = |name: &str| {
                            name.starts_with("public.kern1.") || name.starts_with("public.kern2.")
                        };
                        let short = |name: &str| {
                            name.strip_prefix("public.kern1.")
                                .or_else(|| name.strip_prefix("public.kern2."))
                                .map(|g| format!("@{g}"))
                                .unwrap_or_else(|| name.to_string())
                        };
                        let exception = !is_group(first) || !is_group(second);
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .text_color(if exception {
                                t::status_yellow()
                            } else {
                                t::text()
                            })
                            .child(format!("{} · {}", short(first), short(second)))
                    })
                    .child(
                        div()
                            .text_color(t::text_muted())
                            .child(format!("{value:.0}")),
                    )
                    .child(
                        div()
                            .id(("kern-del", i))
                            .px_1()
                            .text_color(t::text_muted())
                            .cursor_pointer()
                            .hover(|el| el.text_color(t::text()))
                            .child("×")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.delete_kern_pair(&f2, &s2);
                                cx.notify();
                            })),
                    ),
            );
        }
        let body = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(widgets::input::Input::new(&self.inputs.kern.filter))
            .child(editor_row)
            .child(list)
            .child(
                div()
                    .text_xs()
                    .text_color(t::text_muted())
                    .child(if hidden > 0 {
                        format!("{total} pairs · showing {CAP}")
                    } else {
                        format!("{total} pairs")
                    }),
            );
        self.section(cx, "Kerning", body)
    }

    /// Color section: the CPAL palette, the layer mapping, and the
    /// stacked-preview toggle (COLRv0 through the ufo2ft lib keys).
    pub(crate) fn color_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let Some(font) = self.font() else {
            return self.section(cx, "Color", div());
        };
        let palette = read_color_palette(&font.font);
        let mapping = read_color_mapping(&font.font);
        let swatch_color = |c: &[f64; 4]| gpui::Rgba {
            r: c[0] as f32,
            g: c[1] as f32,
            b: c[2] as f32,
            a: c[3] as f32,
        };
        let mut swatches = div().flex().flex_wrap().gap_1().items_center();
        for (i, c) in palette.iter().enumerate() {
            let selected = i == self.color_selected;
            swatches = swatches.child(
                div()
                    .id(("cpal-swatch", i))
                    .w(px(18.0))
                    .h(px(18.0))
                    .rounded(t::radius())
                    .bg(swatch_color(c))
                    .border(t::stroke_emphasis())
                    .border_color(if selected {
                        t::accent()
                    } else {
                        t::cell_border()
                    })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.color_selected = i;
                        cx.notify();
                    })),
            );
        }
        swatches = swatches.child(
            div()
                .w(px(96.0))
                .child(widgets::input::Input::new(&self.inputs.color_hex)),
        );
        if !palette.is_empty() {
            let selected = self.color_selected;
            swatches = swatches.child(
                div()
                    .id("cpal-remove")
                    .px_1()
                    .text_sm()
                    .cursor_pointer()
                    .text_color(t::text_muted())
                    .hover(|el| el.text_color(t::text()))
                    .child("×")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.command_remove_palette_color(selected);
                        cx.notify();
                    })),
            );
        }
        let mut rows = div().flex().flex_col().gap_0p5();
        for (i, (layer, color)) in mapping.iter().enumerate() {
            let dot = palette
                .get(*color)
                .map(swatch_color)
                .unwrap_or(t::text_muted());
            rows = rows.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_xs()
                    .child(div().w(px(10.0)).h(px(10.0)).rounded_full().bg(dot))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .text_color(t::text())
                            .child(layer.clone()),
                    )
                    .child(
                        div()
                            .id(("color-layer-grad", i))
                            .px_1()
                            .text_xs()
                            .cursor_pointer()
                            .text_color(t::text_muted())
                            .hover(|el| el.text_color(t::text()))
                            .child("◐")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.command_layer_gradient(i);
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id(("color-layer-del", i))
                            .px_1()
                            .cursor_pointer()
                            .text_color(t::text_muted())
                            .hover(|el| el.text_color(t::text()))
                            .child("×")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.command_remove_color_layer(i);
                                cx.notify();
                            })),
                    ),
            );
        }
        let toggle_on = self.show_color_preview;
        let body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(swatches)
            .child(rows)
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(
                        div()
                            .id("color-layer-add")
                            .px_2()
                            .py_0p5()
                            .rounded(t::radius())
                            .text_sm()
                            .cursor_pointer()
                            .border(t::stroke())
                            .border_color(t::cell_border())
                            .text_color(t::text())
                            .child("+ Color Layer")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.command_add_color_layer();
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("color-to-v1")
                            .px_2()
                            .py_0p5()
                            .rounded(t::radius())
                            .text_sm()
                            .cursor_pointer()
                            .border(t::stroke())
                            .border_color(t::cell_border())
                            .text_color(t::text())
                            .child("To v1")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.command_convert_to_colrv1();
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("color-preview-toggle")
                            .px_2()
                            .py_0p5()
                            .rounded(t::radius())
                            .text_sm()
                            .cursor_pointer()
                            .border(t::stroke())
                            .when(toggle_on, |el| {
                                el.border_color(t::accent()).text_color(t::accent())
                            })
                            .when(!toggle_on, |el| {
                                el.border_color(t::cell_border()).text_color(t::text())
                            })
                            .child("Preview")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_color_preview = !this.show_color_preview;
                                cx.notify();
                            })),
                    ),
            );
        self.section(cx, "Color", body)
    }

    /// Compare section (grid mode): every master against the active
    /// one — glyph count, structural incompatibilities, differing
    /// advances, kerning pair count, and the vertical metrics that
    /// disagree. The Glyphs Compare Fonts window's job, inside one
    /// project.
    pub(crate) fn compare_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let Some(project) = self.project.as_ref() else {
            return self.section(cx, "Compare", div());
        };
        if project.masters.len() < 2 {
            return self.section(
                cx,
                "Compare",
                div()
                    .text_xs()
                    .text_color(t::text_muted())
                    .child("One master · nothing to compare"),
            );
        }
        let active = project.active;
        let reference = &project.masters[active];
        let incompatible = project.compat.values().filter(|ok| !**ok).count();
        let mut rows = div().flex().flex_col().gap_1();
        for (i, master) in project.masters.iter().enumerate() {
            if i == active {
                continue;
            }
            let missing = reference
                .glyphs
                .iter()
                .filter(|g| !master.name_map.contains_key(g.name.as_ref()))
                .count();
            let advance_diffs = reference
                .glyphs
                .iter()
                .filter(|g| {
                    master
                        .name_map
                        .get(g.name.as_ref())
                        .map(|&j| (master.glyphs[j].advance - g.advance).abs() > 0.5)
                        .unwrap_or(false)
                })
                .count();
            let pair_count = |m: &Master| {
                m.font
                    .kerning
                    .values()
                    .map(|seconds| seconds.len())
                    .sum::<usize>()
            };
            let metrics_diff = {
                let mut diffs: Vec<&str> = Vec::new();
                if (master.ascender - reference.ascender).abs() > 0.5 {
                    diffs.push("asc");
                }
                if (master.descender - reference.descender).abs() > 0.5 {
                    diffs.push("desc");
                }
                if master.x_height != reference.x_height {
                    diffs.push("xh");
                }
                if master.cap_height != reference.cap_height {
                    diffs.push("cap");
                }
                diffs
            };
            rows = rows.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(div().text_sm().text_color(t::text()).child(format!(
                        "{} vs {}",
                        project.master_names[i], project.master_names[active]
                    )))
                    .child(div().text_xs().text_color(t::text_muted()).child(format!(
                        "{} glyphs · {} missing · {} advance diffs · kerning {} vs {}{}",
                        master.glyphs.len(),
                        missing,
                        advance_diffs,
                        pair_count(master),
                        pair_count(reference),
                        if metrics_diff.is_empty() {
                            String::new()
                        } else {
                            format!(" · metrics differ: {}", metrics_diff.join(", "))
                        },
                    ))),
            );
        }
        rows = rows.child(
            div()
                .text_xs()
                .text_color(if incompatible == 0 {
                    t::text_muted()
                } else {
                    t::status_yellow()
                })
                .child(format!("{incompatible} structurally incompatible glyph(s)")),
        );
        self.section(cx, "Compare", rows)
    }

    /// Dimensions section (grid mode): measured stems and bars for
    /// the reference glyphs, per master. Glyphs' Dimensions palette
    /// is hand-typed; these are measured from the outlines.
    pub(crate) fn dimensions_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        if self.project.is_none() {
            return self.section(cx, "Dimensions", div());
        }
        let mut rows = div().flex().flex_col().gap_0p5();
        let mut shown = 0usize;
        for name in ["H", "O", "n", "o", "t", "v"] {
            let (stem, bar) = self.measured_dimensions(name);
            if stem.is_none() && bar.is_none() {
                continue;
            }
            shown += 1;
            let fmt = |v: Option<i64>| v.map(|v| v.to_string()).unwrap_or_else(|| "–".into());
            rows = rows.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .child(
                        div()
                            .w(px(16.0))
                            .text_sm()
                            .text_color(t::text())
                            .child(name),
                    )
                    .child(
                        div()
                            .text_color(t::text_muted())
                            .child(format!("stem {}", fmt(stem))),
                    )
                    .child(
                        div()
                            .text_color(t::text_muted())
                            .child(format!("bar {}", fmt(bar))),
                    ),
            );
        }
        if shown == 0 {
            return self.section(
                cx,
                "Dimensions",
                div()
                    .text_xs()
                    .text_color(t::text_muted())
                    .child("No reference glyphs with straight stems"),
            );
        }
        self.section(cx, "Dimensions", rows)
    }

    /// Font Info section (grid mode): names and vertical metrics of
    /// the active master, saved to fontinfo.plist. The first slice of
    /// Glyphs' Font Info window; axes and instances come later.
    pub(crate) fn font_info_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        if self.project.is_none() {
            return self.section(cx, "Font Info", div());
        }
        let field = |header: &'static str, input: &gpui::Entity<widgets::input::InputState>| {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(div().text_xs().text_color(t::text_muted()).child(header))
                .child(widgets::input::Input::new(input))
        };
        let body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(field("Family Name", &self.inputs.font_info.family))
            .child(field("Style Name", &self.inputs.font_info.style))
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(field("UPM", &self.inputs.font_info.upm))
                    .child(field("Italic Angle", &self.inputs.font_info.italic_angle)),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(field("Ascender", &self.inputs.font_info.ascender))
                    .child(field("Descender", &self.inputs.font_info.descender)),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(field("x-Height", &self.inputs.font_info.x_height))
                    .child(field("Cap Height", &self.inputs.font_info.cap_height)),
            )
            // The vertical-metrics parameters (typo/hhea/win), kept
            // together the way the Glyphs Masters tab carries them.
            .child(
                div()
                    .text_xs()
                    .text_color(t::text_muted())
                    .child("Vertical Metrics"),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(field("typoAsc", &self.inputs.font_info.typo_asc))
                    .child(field("typoDesc", &self.inputs.font_info.typo_desc))
                    .child(field("typoGap", &self.inputs.font_info.typo_gap)),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(field("hheaAsc", &self.inputs.font_info.hhea_asc))
                    .child(field("hheaDesc", &self.inputs.font_info.hhea_desc))
                    .child(field("hheaGap", &self.inputs.font_info.hhea_gap)),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(field("winAsc", &self.inputs.font_info.win_asc))
                    .child(field("winDesc", &self.inputs.font_info.win_desc)),
            )
            // PostScript hinting data: alignment zones (pairs of
            // position, position+size) and standard stems, the
            // Glyphs Masters-tab Metrics/Stems story. The zones
            // also draw as bands in the editor.
            .child(
                div()
                    .text_xs()
                    .text_color(t::text_muted())
                    .child("Zones & Stems"),
            )
            .child(field("Blue Values", &self.inputs.font_info.blue_values))
            .child(field("Other Blues", &self.inputs.font_info.other_blues))
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(field("Stems H", &self.inputs.font_info.stems_h))
                    .child(field("Stems V", &self.inputs.font_info.stems_v)),
            );
        self.section(cx, "Font Info", body)
    }

    /// Selection section: count plus editable X/Y for a single point.
    pub(crate) fn selection_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let count = self.editor.selected.len();
        let single = self.single_selected_point();
        // A quiet count line rather than a heading: the fields below
        // say what they are.
        let mut body = div().flex().flex_col().gap_2().child(
            div()
                .text_xs()
                .text_color(t::text_muted())
                .child(match count {
                    0 => "nothing selected".to_string(),
                    1 => "1 point".to_string(),
                    n => format!("{n} points"),
                }),
        );
        let _ = single;
        // A whole segment selected: report the curve's real size, which
        // is what you compare when matching one curve to another.
        if let Some((segments, r)) = self.selected_segment_bounds() {
            let label = if segments == 1 {
                "Segment".to_string()
            } else {
                format!("{segments} segments")
            };
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .text_sm()
                    .child(div().text_color(t::text_muted()).child(label))
                    .child(
                        div()
                            .text_color(t::text())
                            .child(SharedString::from(format!(
                                "{:.0} × {:.0}",
                                r.width(),
                                r.height()
                            ))),
                    ),
            );
        }
        // The picker and the fields are always up, the way the web's
        // CoordinatePanel is: the reference point is a setting you
        // choose before selecting, and an empty panel that appears and
        // disappears makes the sidebar jump.
        {
            use runebender_core::outline::path::Quadrant;
            let field = |label: &'static str, input: &gpui::Entity<widgets::input::InputState>| {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(14.0))
                            .text_sm()
                            .text_color(t::text_muted())
                            .child(label),
                    )
                    .child(div().flex_1().child(widgets::input::Input::new(input)))
            };
            // The 9-point reference picker (web coordinate quadrant):
            // numeric X/Y and W/H act about the chosen corner.
            const QUADRANTS: [[Quadrant; 3]; 3] = [
                [Quadrant::TopLeft, Quadrant::Top, Quadrant::TopRight],
                [Quadrant::Left, Quadrant::Center, Quadrant::Right],
                [
                    Quadrant::BottomLeft,
                    Quadrant::Bottom,
                    Quadrant::BottomRight,
                ],
            ];
            let mut picker = div()
                .w(px(52.0))
                .h(px(52.0))
                .flex()
                .flex_col()
                .justify_between()
                .border(t::stroke())
                .border_color(t::panel_outline())
                .p(px(3.0));
            for (ri, row_quads) in QUADRANTS.iter().enumerate() {
                let mut row_el = div().flex().justify_between().w_full();
                for (qi, quadrant) in row_quads.iter().enumerate() {
                    let quadrant = *quadrant;
                    let active = self.coord_quadrant == quadrant;
                    row_el = row_el.child(
                        div()
                            .id(("quadrant", ri * 3 + qi))
                            .w(px(10.0))
                            .h(px(10.0))
                            .rounded_full()
                            .cursor_pointer()
                            .border(t::stroke())
                            .when(active, |el| el.bg(t::accent()).border_color(t::accent()))
                            .when(!active, |el| el.border_color(t::cell_border()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.coord_quadrant = quadrant;
                                cx.notify();
                            })),
                    );
                }
                picker = picker.child(row_el);
            }
            body = body.child(
                div()
                    .flex()
                    .gap_3()
                    .items_center()
                    .child(picker)
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(field("X", &self.inputs.metric.x))
                            .child(field("Y", &self.inputs.metric.y)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(field("W", &self.inputs.metric.w))
                            .child(field("H", &self.inputs.metric.h)),
                    ),
            );
        }
        // Selected anchor: editable name (web AnchorPanel).
        if !self.editor.selected_anchors.is_empty() {
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().text_sm().text_color(t::text_muted()).child("Anchor"))
                    .child(
                        div()
                            .flex_1()
                            .child(widgets::input::Input::new(&self.inputs.anchor_name)),
                    ),
            );
        }
        // Selected component: name plus the anchor lock, the Glyphs
        // contract — locked follows its anchor, free is draggable.
        if let (Mode::Editor(index), Some(ci)) = (&self.mode, self.editor.selected_component) {
            let index = *index;
            let info = self
                .font()
                .and_then(|f| f.font.get_glyph(f.glyphs[index].name.as_ref()))
                .and_then(|g| g.components.get(ci))
                .map(|c| {
                    (
                        c.base.to_string(),
                        !runebender_core::document::composites::component_alignment_disabled(c),
                    )
                });
            if let Some((base, aligned)) = info {
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .text_color(t::text_muted())
                                .child(format!("Component /{base}")),
                        )
                        .child(
                            div()
                                .id("component-lock")
                                .px_2()
                                .py_0p5()
                                .rounded(t::radius())
                                .text_sm()
                                .cursor_pointer()
                                .border(t::stroke())
                                .when(aligned, |el| {
                                    el.border_color(t::accent()).text_color(t::accent())
                                })
                                .when(!aligned, |el| {
                                    el.border_color(t::cell_border()).text_color(t::text())
                                })
                                .child(if aligned { "Locked" } else { "Free" })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_component_alignment(index, ci);
                                    cx.notify();
                                })),
                        ),
                );
                // A smart part gets its value field: Enter re-places
                // it at the typed position — a bare number moves the
                // first axis, "Height=30" names one.
                let smart_axis = self.font().and_then(|f| {
                    let glyph = f.font.get_glyph(f.glyphs[index].name.as_ref())?;
                    let base = f.font.get_glyph(glyph.components.get(ci)?.base.as_str())?;
                    let names: Vec<&str> = base
                        .lib
                        .get("com.schriftgestaltung.Glyphs.smartComponentAxes")?
                        .as_array()?
                        .iter()
                        .filter_map(|a| a.as_dictionary()?.get("name")?.as_string())
                        .collect();
                    (!names.is_empty()).then(|| names.join(" · "))
                });
                if let Some(axis) = smart_axis {
                    body = body.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t::text_muted())
                                    .child(format!("Smart {axis}")),
                            )
                            .child(
                                div()
                                    .w(px(64.0))
                                    .child(widgets::input::Input::new(&self.inputs.smart_value)),
                            ),
                    );
                }
            }
        }
        self.section(cx, "Coordinates", body)
    }

    /// Axis slider row (designspaces only).
    /// Axes section for a sidebar: one labeled slider per designspace
    /// axis (the web/Glyphs place these in a pane, not a full-width
    /// strip).
    pub(crate) fn axes_section(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let project = self.project.as_ref()?;
        if self.axis_sliders.is_empty() {
            return None;
        }
        let mut rows = div().flex().flex_col().gap_2();
        for (axis_index, slider) in &self.axis_sliders {
            let Some(axis) = project.axes.get(*axis_index) else {
                continue;
            };
            rows = rows.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(t::text_muted())
                            .child(SharedString::from(axis.tag.clone())),
                    )
                    .child(div().flex_1().child(flat_slider(slider, cx))),
            );
        }
        let mut body = div().flex().flex_col().gap_2().child(rows);
        if project.ds_doc.is_some() {
            // Named designspace instances: one row each; clicking
            // parks the sliders and the preview on that instance,
            // × drops it. The field below renames the instance at
            // the current location on Enter, or adds one there.
            let mut list = div().flex().flex_col();
            let here = &project.location;
            for (i, (name, location)) in project.instances.iter().enumerate() {
                let at_instance = project.axes.iter().all(|a| {
                    let want = location.get(&a.name).copied().unwrap_or(0.0);
                    let got = here.get(&a.name).copied().unwrap_or(0.0);
                    (want - got).abs() < 1e-6
                });
                let target = location.clone();
                list = list.child(
                    div()
                        .id(("instance-row", i))
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_1()
                        .py_0p5()
                        .text_sm()
                        .cursor_pointer()
                        .text_color(if at_instance { t::accent() } else { t::text() })
                        .hover(|el| el.bg(t::cell_selected_bg()))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .truncate()
                                .child(SharedString::from(name.clone())),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.go_to_location(&target, window, cx);
                        }))
                        .child(
                            div()
                                .id(("instance-del", i))
                                .px_1()
                                .text_color(t::text_muted())
                                .hover(|el| el.text_color(t::text()))
                                .child("×")
                                .on_click(cx.listener(
                                    move |this, ev: &gpui::ClickEvent, _, cx| {
                                        let _ = ev;
                                        cx.stop_propagation();
                                        this.command_instance_delete(i);
                                        cx.notify();
                                    },
                                )),
                        ),
                );
            }
            body = body.child(
                div()
                    .text_xs()
                    .text_color(t::text_muted())
                    .child("Instances"),
            );
            body = body.child(list);
            body = body.child(widgets::input::Input::new(&self.inputs.instance_name));
        }
        // Axis mappings (avar): user → design pairs on the first
        // axis, the Glyphs Axis Mappings story. "400,430" adds or
        // replaces the pair at that input; × removes.
        if let Some(doc) = project.ds_doc.as_ref()
            && let Some(axis) = doc.axes.first()
        {
            body = body.child(
                div()
                    .text_xs()
                    .text_color(t::text_muted())
                    .child(format!("Mappings ({} user → design)", axis.tag)),
            );
            if let Some(map) = axis.map.as_ref() {
                let mut rows = div().flex().flex_wrap().gap_1();
                for (i, m) in map.iter().enumerate() {
                    rows = rows.child(
                        div()
                            .id(("axis-map", i))
                            .px_1()
                            .rounded(t::radius())
                            .border(t::stroke())
                            .border_color(t::cell_border())
                            .text_xs()
                            .text_color(t::text())
                            .cursor_pointer()
                            .child(format!("{:.0}→{:.0} ×", m.input, m.output))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.command_remove_axis_mapping(i);
                                cx.notify();
                            })),
                    );
                }
                body = body.child(rows);
            }
            body = body.child(
                div()
                    .w(px(110.0))
                    .child(widgets::input::Input::new(&self.inputs.axis_map)),
            );
        }
        // HOI: the trajectory view and the timing ease, the
        // higher-order interpolation corner of the panel.
        body = body.child(
            div()
                .text_xs()
                .text_color(t::text_muted())
                .child("Interpolation"),
        );
        let on = self.show_trajectories;
        body = body.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .id("hoi-trajectories")
                        .px_2()
                        .py_0p5()
                        .rounded(t::radius())
                        .text_sm()
                        .cursor_pointer()
                        .border(t::stroke())
                        .when(on, |el| {
                            el.border_color(t::accent()).text_color(t::accent())
                        })
                        .when(!on, |el| {
                            el.border_color(t::cell_border()).text_color(t::text())
                        })
                        .child("Trajectories")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_trajectories = !this.show_trajectories;
                            cx.notify();
                        })),
                )
                .child(div().text_xs().text_color(t::text_muted()).child("Ease"))
                .child(
                    div()
                        .w(px(64.0))
                        .child(widgets::input::Input::new(&self.inputs.ease)),
                ),
        );
        Some(self.section(cx, "Axes", body))
    }
}
