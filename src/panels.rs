// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! The panels either side of the canvas.
//!
//! Each function builds one region and reads the workspace rather than
//! holding state of its own, so a panel can be moved or removed
//! without untangling it from the editing model.

use super::*;

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
        let mut ink: std::collections::HashMap<
            usize,
            (Arc<BezPath>, kurbo::Rect, f64, gpui::Rgba),
        > = std::collections::HashMap::new();
        for &(glyph, _) in rows.iter().flatten() {
            let Some(entry) = font.glyphs.get(glyph) else {
                continue;
            };
            if entry.path.elements().is_empty() {
                continue;
            }
            let selected =
                self.selected == Some(glyph) || self.multi_selected.contains(entry.name.as_ref());
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

    pub(crate) fn category_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        use runebender_core::sidebar as sb;
        let counts = self.sidebar_counts.as_ref();

        // Categories: expandable rows with the web's subfilters.
        let mut categories = div().flex().flex_col();
        for (ci, (category, label)) in SIDEBAR_CATEGORIES.iter().enumerate() {
            let subs = sb::category_subfilters(label);
            let count = counts.map(|c| c.categories[ci]).unwrap_or(0);
            let expanded = self.expanded_categories.contains(&ci);
            let mut row = self
                .sidebar_row(
                    ("category", ci),
                    false,
                    (!subs.is_empty()).then_some(expanded),
                    None,
                    SharedString::from(*label),
                    format!("{count}").into(),
                    if ci == 0 {
                        SidebarFilter::All
                    } else {
                        SidebarFilter::Category(*category)
                    },
                    cx,
                )
                .into_any_element();
            if !subs.is_empty() {
                // A separate click target for the chevron would fight
                // the row click; double-purpose: clicking an already
                // selected row toggles expansion instead.
                let category = *category;
                let selected = self.sidebar_filter == SidebarFilter::Category(category)
                    || subs.iter().any(|(sub, _)| {
                        self.sidebar_filter == SidebarFilter::Subfilter(category, sub)
                    });
                row = self
                    .sidebar_row(
                        ("category", ci),
                        false,
                        Some(expanded),
                        None,
                        SharedString::from(*label),
                        format!("{count}").into(),
                        SidebarFilter::Category(category),
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if selected {
                            if !this.expanded_categories.remove(&ci) {
                                this.expanded_categories.insert(ci);
                            }
                        }
                        this.set_sidebar_filter(SidebarFilter::Category(category));
                        cx.notify();
                    }))
                    .into_any_element();
            }
            categories = categories.child(row);
            if expanded {
                for (si, (sub, sub_label)) in subs.iter().enumerate() {
                    let count = counts
                        .and_then(|c| c.subfilters.get(&(ci, si)).copied())
                        .unwrap_or(0);
                    categories = categories.child(self.sidebar_row(
                        ("subfilter", ci * 100 + si),
                        true,
                        None,
                        None,
                        SharedString::from(*sub_label),
                        format!("{count}").into(),
                        SidebarFilter::Subfilter(*category, sub),
                        cx,
                    ));
                }
            }
        }

        // Languages: script groups with per-set coverage, like the
        // web sidebar and Glyphs.
        let mut languages = div().flex().flex_col();
        for (gi, group) in sb::language_groups().iter().enumerate() {
            let count = counts.map(|c| c.groups[gi]).unwrap_or(0);
            let expanded = self.expanded_scripts.contains(&gi);
            let selected = self.sidebar_filter == SidebarFilter::LanguageGroup(gi)
                || (0..group.filters.len())
                    .any(|fi| self.sidebar_filter == SidebarFilter::Language(gi, fi));
            languages = languages.child(
                self.sidebar_row(
                    ("script", gi),
                    false,
                    Some(expanded),
                    Some(group.icon.clone().into()),
                    group.label.clone().into(),
                    format!("{count}").into(),
                    SidebarFilter::LanguageGroup(gi),
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if selected {
                        if !this.expanded_scripts.remove(&gi) {
                            this.expanded_scripts.insert(gi);
                        }
                    } else {
                        this.expanded_scripts.insert(gi);
                    }
                    this.set_sidebar_filter(SidebarFilter::LanguageGroup(gi));
                    cx.notify();
                })),
            );
            if expanded {
                for (fi, filter) in group.filters.iter().enumerate() {
                    let count = counts.map(|c| c.languages[gi][fi]).unwrap_or(0);
                    let missing = counts.map(|c| c.missing[gi][fi]).unwrap_or(0);
                    let count_text = match filter.expected_count {
                        Some(expected) => format!("{count}/{expected}"),
                        None => format!("{count}"),
                    };
                    let row = self.sidebar_row(
                        ("language", gi * 100 + fi),
                        true,
                        None,
                        None,
                        filter.label.clone().into(),
                        count_text.into(),
                        SidebarFilter::Language(gi, fi),
                        cx,
                    );
                    if missing > 0 {
                        // "+" generates the filter's missing glyphs.
                        languages = languages.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(div().flex_1().child(row))
                                .child(
                                    div()
                                        .id(("gen-missing", gi * 100 + fi))
                                        .w(px(18.0))
                                        .h(px(18.0))
                                        .rounded(t::radius())
                                        .border(t::stroke())
                                        .border_color(t::cell_border())
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_sm()
                                        .text_color(t::text_muted())
                                        .cursor_pointer()
                                        .child("+")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.command_generate_missing(gi, fi);
                                            cx.notify();
                                        })),
                                ),
                        );
                    } else {
                        languages = languages.child(row);
                    }
                }
            }
        }

        // Filters: the Runebender builtins plus headline GF sets.
        let mut filters = div().flex().flex_col();
        for (bi, builtin) in sb::builtin_filters().iter().enumerate() {
            let count = counts.map(|c| c.builtins[bi]).unwrap_or(0);
            let count_text = match builtin.glyphset.as_ref().and_then(|set| set.expected_count) {
                Some(expected) => format!("{count}/{expected}"),
                None => format!("{count}"),
            };
            filters = filters.child(self.sidebar_row(
                ("builtin", bi),
                false,
                None,
                None,
                builtin.label.clone().into(),
                count_text.into(),
                SidebarFilter::Builtin(bi),
                cx,
            ));
        }
        // Saved searches (Glyphs' smart filters): pinned queries from
        // the search field, stored in the font lib.
        let saved_defs = self
            .font()
            .map(|f| read_saved_filters(&f.font))
            .unwrap_or_default();
        for (si, (label, _)) in saved_defs.iter().enumerate() {
            let count = counts.and_then(|c| c.saved.get(si).copied()).unwrap_or(0);
            let active = self.sidebar_filter == SidebarFilter::Saved(si);
            filters = filters.child(
                div()
                    .id(("saved-filter", si))
                    .group("saved-filter")
                    .h(px(20.0))
                    .px_2()
                    .rounded(t::radius())
                    .text_sm()
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap_1()
                    .when(active, |el| {
                        el.border(t::stroke())
                            .border_color(t::accent())
                            .text_color(t::accent())
                    })
                    .when(!active, |el| el.text_color(t::text()))
                    .child(
                        div()
                            .w(px(16.0))
                            .text_color(if active { t::accent() } else { t::text_muted() })
                            .child("⌕"),
                    )
                    .child(div().flex_1().child(SharedString::from(label.clone())))
                    .child(
                        div()
                            .id(("saved-filter-del", si))
                            .text_color(t::text_muted())
                            .invisible()
                            .group_hover("saved-filter", |el| el.visible())
                            .child("×")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.delete_saved_filter(si);
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .text_color(if active { t::accent() } else { t::text_muted() })
                            .child(SharedString::from(format!("{count}"))),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_sidebar_filter(SidebarFilter::Saved(si));
                        cx.notify();
                    })),
            );
        }
        let pending_query = self.search_query.trim().to_string();
        if !pending_query.is_empty() && !saved_defs.iter().any(|(_, q)| *q == pending_query) {
            filters = filters.child(
                div()
                    .id("save-search-filter")
                    .h(px(20.0))
                    .px_2()
                    .rounded(t::radius())
                    .text_sm()
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_color(t::text_muted())
                    .child(div().w(px(16.0)).child("+"))
                    .child(div().flex_1().child(SharedString::from(format!(
                        "Save \u{201c}{pending_query}\u{201d}"
                    ))))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.save_current_search_as_filter();
                        cx.notify();
                    })),
            );
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
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
                            .child(widgets::input::Input::new(&self.search)),
                    )
                    .child(self.search_toggle(
                        "search-mode",
                        match self.search_mode {
                            1 => "N",
                            2 => "U",
                            _ => "A",
                        },
                        self.search_mode != 0,
                        |this| this.search_mode = (this.search_mode + 1) % 3,
                        cx,
                    ))
                    .child(self.search_toggle(
                        "search-regex",
                        ".*",
                        self.search_regex,
                        |this| {
                            this.search_regex = !this.search_regex;
                            this.rebuild_search_regex();
                        },
                        cx,
                    ))
                    .child(self.search_toggle(
                        "search-case",
                        "Aa",
                        self.search_case,
                        |this| {
                            this.search_case = !this.search_case;
                            this.rebuild_search_regex();
                        },
                        cx,
                    )),
            )
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .child(self.section(cx, "Categories", categories))
                    .child(self.section(cx, "Languages", languages))
                    .child(self.section(cx, "Filters", filters)),
            )
            // Mark colours sit at the foot of the sidebar, beside the
            // glyphs they apply to, the way the web places them.
            .child(self.mark_colors_panel(cx))
    }

    /// Right tile: details of the selected glyph, like
    /// runebender-web's GlyphInfoSidebar.
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
            .child(row("Master", master))
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
            .child(input_row("Glyph Name", &self.glyph_inputs.name))
            .when(in_editor, |el| {
                el.child(row("Width", format!("{:.0}", entry.advance).into()))
            })
            .when(!in_editor, |el| {
                el.child(
                    div()
                        .flex()
                        .gap_1()
                        .child(metric_field("Width", &self.metric_inputs.width))
                        .child(metric_field("LSB", &self.metric_inputs.lsb))
                        .child(metric_field("RSB", &self.metric_inputs.rsb)),
                )
            })
            .child(pair_row(
                "Kerning Groups (L · R)",
                &self.glyph_inputs.group_l,
                &self.glyph_inputs.group_r,
            ))
            .child(pair_row(
                "Metrics Keys (L · R)",
                &self.glyph_inputs.lsb_key,
                &self.glyph_inputs.rsb_key,
            ))
            .child(input_row("Unicode", &self.glyph_inputs.unicode))
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
            .child(input_row("Production Name", &self.glyph_inputs.production))
            .child(input_row("Note", &self.glyph_inputs.note))
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
                        .child(widgets::input::Input::new(&self.glyph_inputs.switch_at)),
                }
            });
        self.section(cx, "Glyph", panel)
    }

    /// Colors panel: mark-color swatches for the selected glyph, like
    /// the web grid's bottom-right panel.
    /// Right-panel live preview of the selected glyph: outline plus
    /// control points, the way runebender-web fills the space between
    /// the info sections and the colors.
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

    /// Editor sidebar: search + scrollable mini glyph grid, so glyph
    /// switching doesn't require leaving the editor.
    pub(crate) fn editor_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let _query = self.search_query.clone();
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
                let start = self.sidebar_scroll_row.min(rows_total.saturating_sub(1));
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
            let active = self.sidebar_tab == which;
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
                    this.sidebar_tab = which;
                    cx.notify();
                }))
        };
        // An axis-less font has no Axes tab, so a stale selection
        // falls back to the glyph list.
        let tab_now = if !has_axes && self.sidebar_tab == 2 {
            0
        } else {
            self.sidebar_tab
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
                                .child(widgets::input::Input::new(&self.search)),
                        )
                        .child(self.search_toggle(
                            "search-mode",
                            match self.search_mode {
                                1 => "N",
                                2 => "U",
                                _ => "A",
                            },
                            self.search_mode != 0,
                            |this| this.search_mode = (this.search_mode + 1) % 3,
                            cx,
                        ))
                        .child(self.search_toggle(
                            "search-regex",
                            ".*",
                            self.search_regex,
                            |this| {
                                this.search_regex = !this.search_regex;
                                this.rebuild_search_regex();
                            },
                            cx,
                        ))
                        .child(self.search_toggle(
                            "search-case",
                            "Aa",
                            self.search_case,
                            |this| {
                                this.search_case = !this.search_case;
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
                                        if this.sidebar_viewport != bounds.size {
                                            this.sidebar_viewport = bounds.size;
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
                                            &mut this.sidebar_scroll_row,
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
                            self.sidebar_slider
                                .as_ref()
                                .map(|slider| div().w(px(96.0)).child(flat_slider(slider, cx))),
                        ),
                )
            })
            // Colours stay put whichever tab is up.
            .child(self.mark_colors_panel(cx))
    }

    /// Related Glyphs section (Fontra's panel): the glyph's base,
    /// its suffix siblings (name.*), its components, and every
    /// composite using it — one click away.
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
    /// logical order against the shaped glyphs, cluster-linked —
    /// Fontra's inspector, on the shared text engine. Click a chip
    /// to cross-highlight its cluster; double-click a glyph chip to
    /// open that glyph for editing inside the shaped run.
    pub(crate) fn shaping_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        use runebender_core::text::{TextDirection, TextSortKind};
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
        for i in 0..count {
            let Some(sort) = self.edit_buffer.sort(i) else {
                continue;
            };
            let TextSortKind::Glyph { codepoint, .. } = &sort.kind else {
                continue;
            };
            let carrier = carrier_of[i];
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
                                                runebender_core::glyph_ops::reverse_contours(
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
                                .child(widgets::input::Input::new(&self.slant_input)),
                        )
                        .child(div().text_xs().text_color(t::text_muted()).child("Stroke"))
                        .child(
                            div()
                                .w(px(64.0))
                                .child(widgets::input::Input::new(&self.stroke_input)),
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
                                .child(widgets::input::Input::new(&self.offset_input)),
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
                                .child(widgets::input::Input::new(&self.extrude_input)),
                        )
                        .child(div().text_xs().text_color(t::text_muted()).child("Roughen"))
                        .child(
                            div()
                                .w(px(64.0))
                                .child(widgets::input::Input::new(&self.roughen_input)),
                        ),
                ),
        )
    }

    /// Curves section: comb + continuity toggles (web CurvePanel).
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
                        .child(widgets::input::Input::new(&self.fit_input)),
                ),
        );
        self.section(cx, "Curves", body)
    }

    /// Background section: show/send/swap/clear plus the reference
    /// glyph (web's Background block).
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
                            .child(widgets::input::Input::new(&self.reference_glyph_input)),
                    ),
            );
        self.section(cx, "Background", body)
    }

    /// Layers section: one row per master, the active one highlighted.
    pub(crate) fn layers_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let (names, active): (Vec<SharedString>, usize) = match &self.project {
            Some(p) => (p.master_names.clone(), p.active),
            None => (Vec::new(), 0),
        };
        let reference = self.reference_layers.clone();
        // A thumbnail of the current glyph in each master, the web
        // MasterToolbar's glyph buttons relocated into this section.
        let glyph_name: Option<String> = self
            .selected
            .and_then(|i| self.font().map(|f| f.glyphs[i].name.to_string()));
        let thumbs: Vec<Option<(Arc<BezPath>, f64, f64, f64)>> = match (&self.project, &glyph_name)
        {
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
                    .child(widgets::input::Input::new(&self.component_name_input)),
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
                    .child(widgets::input::Input::new(&self.corner_name_input)),
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
                .and_then(|i| self.font().map(|f| (i, f)))
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
                    .child(widgets::input::Input::new(&self.annotation_input)),
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
        let group_l = runebender_core::glyph_ops::kern_group(&font.font, entry.name.as_ref(), true)
            .map(|g| g.as_str().replace("public.kern1.", ""))
            .unwrap_or_default();
        let group_r =
            runebender_core::glyph_ops::kern_group(&font.font, entry.name.as_ref(), false)
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
                            .child(metric(&self.metric_inputs.lsb))
                            .child(metric(&self.metric_inputs.width))
                            .child(metric(&self.metric_inputs.rsb))
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
                    .child(widgets::input::Input::new(&self.features_input).h_full()),
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
            .child(widgets::input::Input::new(&self.group_name_input))
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
            .kern_inputs
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
            .child(field(&self.kern_inputs.first))
            .child(field(&self.kern_inputs.second))
            .child(field(&self.kern_inputs.value));
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
                            (&this.kern_inputs.first, f3.clone()),
                            (&this.kern_inputs.second, s3.clone()),
                            (&this.kern_inputs.value, format!("{v3}")),
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
            .child(widgets::input::Input::new(&self.kern_inputs.filter))
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
                .child(widgets::input::Input::new(&self.color_hex_input)),
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
            let pair_count = |m: &FontModel| {
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
            .child(field("Family Name", &self.font_info_inputs.family))
            .child(field("Style Name", &self.font_info_inputs.style))
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(field("UPM", &self.font_info_inputs.upm))
                    .child(field("Italic Angle", &self.font_info_inputs.italic_angle)),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(field("Ascender", &self.font_info_inputs.ascender))
                    .child(field("Descender", &self.font_info_inputs.descender)),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(field("x-Height", &self.font_info_inputs.x_height))
                    .child(field("Cap Height", &self.font_info_inputs.cap_height)),
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
                    .child(field("typoAsc", &self.font_info_inputs.typo_asc))
                    .child(field("typoDesc", &self.font_info_inputs.typo_desc))
                    .child(field("typoGap", &self.font_info_inputs.typo_gap)),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(field("hheaAsc", &self.font_info_inputs.hhea_asc))
                    .child(field("hheaDesc", &self.font_info_inputs.hhea_desc))
                    .child(field("hheaGap", &self.font_info_inputs.hhea_gap)),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(field("winAsc", &self.font_info_inputs.win_asc))
                    .child(field("winDesc", &self.font_info_inputs.win_desc)),
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
            .child(field("Blue Values", &self.font_info_inputs.blue_values))
            .child(field("Other Blues", &self.font_info_inputs.other_blues))
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(field("Stems H", &self.font_info_inputs.stems_h))
                    .child(field("Stems V", &self.font_info_inputs.stems_v)),
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
            use runebender_core::path::Quadrant;
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
                            .child(field("X", &self.metric_inputs.x))
                            .child(field("Y", &self.metric_inputs.y)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(field("W", &self.metric_inputs.w))
                            .child(field("H", &self.metric_inputs.h)),
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
                            .child(widgets::input::Input::new(&self.anchor_name_input)),
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
                        !runebender_core::composites::component_alignment_disabled(c),
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
                                    .child(widgets::input::Input::new(&self.smart_value_input)),
                            ),
                    );
                }
            }
        }
        self.section(cx, "Coordinates", body)
    }

    /// The Local AI section: choose a model, run it, and see how the
    /// result scores against a master already drawn.
    ///
    /// Both halves matter. Running a model is easy to offer and easy
    /// to trust too far; scoring it against work done by hand is what
    /// says whether the proposal was worth having.
    pub(crate) fn local_ai_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        let body = div().flex().flex_col().gap_1p5();

        // Which model, and a way to change it.
        let label: SharedString = self
            .model_summary
            .clone()
            .unwrap_or_else(|| "No model chosen".into());
        let body = body.child(
            div()
                .id("ai-model")
                .px_1()
                .py_0p5()
                .border(t::stroke())
                .border_color(t::panel_outline())
                .cursor_pointer()
                .text_xs()
                .text_color(t::text())
                .child(label)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.command_choose_model(cx);
                })),
        );

        if self.model_dir.is_none() {
            return body.child(div().text_xs().text_color(t::text_muted()).child(
                "A model is a folder holding config.json, \
                         weights.safetensors and vocab.txt. Nothing is \
                         downloaded.",
            ));
        }

        // Strength, because a model can be right about direction and
        // short on distance.
        let body = match &self.model_strength_slider {
            Some(slider) => body.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .w(px(58.0))
                            .text_xs()
                            .text_color(t::text_muted())
                            .child(format!("{:.2}x", self.model_strength)),
                    )
                    .child(div().flex_1().child(flat_slider(slider, cx))),
            ),
            None => body,
        };

        let in_editor = matches!(self.mode, Mode::Editor(_));
        let body = body.child(
            div()
                .id("ai-run")
                .px_1()
                .py_0p5()
                .border(t::stroke())
                .border_color(if in_editor {
                    t::accent()
                } else {
                    t::panel_outline()
                })
                .cursor_pointer()
                .text_xs()
                .text_color(if in_editor {
                    t::text()
                } else {
                    t::text_muted()
                })
                .child(if in_editor {
                    "Bolden this glyph"
                } else {
                    "Open a glyph to run"
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Mode::Editor(index) = this.mode {
                        let dir = this.model_dir.clone();
                        if let Some(dir) = dir {
                            this.apply_bolden(index, &dir);
                            cx.notify();
                        }
                    }
                })),
        );

        // The judgement, when there is another master to judge against.
        let body = body.child(
            div()
                .id("ai-score")
                .px_1()
                .py_0p5()
                .border(t::stroke())
                .border_color(t::panel_outline())
                .cursor_pointer()
                .text_xs()
                .text_color(t::text_muted())
                .child("Score against the other master")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.command_score_model();
                    cx.notify();
                })),
        );

        let body = match &self.model_score {
            Some((glyph, model, baseline)) => {
                let better = model < baseline;
                body.child(
                    div()
                        .text_xs()
                        .text_color(if better { t::accent() } else { t::text_muted() })
                        .child(format!(
                            "{glyph}: model {model:.1}, mean-shift {baseline:.1}"
                        )),
                )
            }
            None => body,
        };
        body
    }

    /// The Glyphs-style tab strip under the header: a Font tab that
    /// returns to the full glyph overview, plus one tab per edit
    /// session, titled with the session's text.
    pub(crate) fn tab_strip(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        if self.project.is_none() {
            return div().into_any_element();
        }
        let in_editor = matches!(self.mode, Mode::Editor(_));
        let tab = |id: gpui::ElementId, label: SharedString, active: bool| {
            div()
                .id(id)
                .h(px(TAB_H))
                .px_2()
                .flex()
                .items_center()
                .rounded(t::radius())
                .text_sm()
                .cursor_pointer()
                .when(active, |el| {
                    el.border(t::stroke())
                        .border_color(t::accent())
                        .text_color(t::accent())
                })
                .when(!active, |el| {
                    el.border(t::stroke())
                        .border_color(t::cell_border())
                        .text_color(t::text_muted())
                })
                .child(label)
        };
        // Each session tab reads like Glyphs: the buffer's text, with
        // /name for unencoded glyphs, trimmed to fit.
        let session_label =
            |buffer: &runebender_core::text::TextBuffer, fallback: &str| -> SharedString {
                let mut label = String::new();
                for i in 0..buffer.len() {
                    let Some(sort) = buffer.sort(i) else {
                        continue;
                    };
                    if sort.is_absorbed() {
                        continue;
                    }
                    match &sort.kind {
                        runebender_core::text::TextSortKind::Glyph {
                            codepoint, name, ..
                        } => match codepoint {
                            Some(c) => label.push(*c),
                            None => {
                                label.push('/');
                                label.push_str(name);
                            }
                        },
                        _ => label.push(' '),
                    }
                    if label.chars().count() > 24 {
                        label.truncate(
                            label
                                .char_indices()
                                .nth(24)
                                .map(|(i, _)| i)
                                .unwrap_or(label.len()),
                        );
                        label.push('…');
                        break;
                    }
                }
                if label.is_empty() {
                    label = fallback.to_string();
                }
                label.into()
            };
        let labels: Vec<SharedString> = self
            .sessions
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let fallback: String = if i == self.active_session {
                    match self.mode {
                        Mode::Editor(index) => self
                            .font()
                            .map(|f| f.glyphs[index].name.to_string())
                            .unwrap_or_default(),
                        Mode::Grid => s.glyph_name.clone(),
                    }
                } else {
                    s.glyph_name.clone()
                };
                if i == self.active_session {
                    session_label(&self.edit_buffer, &fallback)
                } else {
                    session_label(&s.buffer, &fallback)
                }
            })
            .collect();
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                tab("tab-font".into(), "Font".into(), !in_editor).on_click(cx.listener(
                    |this, _, _, cx| {
                        if let Mode::Editor(index) = this.mode {
                            this.last_editor = Some(index);
                            let name = this.font().map(|f| f.glyphs[index].name.to_string());
                            if let (Some(name), Some(project)) = (name, this.project.as_mut()) {
                                project.recheck_compat(&name);
                            }
                            this.mode = Mode::Grid;
                            this.status_note = None;
                            cx.notify();
                        }
                    },
                )),
            )
            .children(labels.into_iter().enumerate().map(|(i, label)| {
                let active = in_editor && i == self.active_session;
                tab(("tab-session", i).into(), label, active)
                    .flex()
                    .items_center()
                    .gap_1()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // Return to the session as it was left: same
                        // buffer, tool, undo stack.
                        this.activate_session(i);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .id(("tab-close", i))
                            .px_0p5()
                            .rounded(t::radius())
                            .text_color(t::text_muted())
                            .hover(|el| el.text_color(t::text()))
                            .child("×")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_session(i);
                                cx.notify();
                            })),
                    )
            }))
            .child(
                tab("tab-new".into(), "+".into(), false)
                    .w(px(TAB_H))
                    .justify_center()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.command_new_session();
                        cx.notify();
                    })),
            )
            .into_any_element()
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
                            .child(axis.tag.clone()),
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
                        .child(div().flex_1().min_w(px(0.0)).truncate().child(name.clone()))
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
            body = body.child(widgets::input::Input::new(&self.instance_name_input));
        }
        // Axis mappings (avar): user → design pairs on the first
        // axis, the Glyphs Axis Mappings story. "400,430" adds or
        // replaces the pair at that input; × removes.
        if let Some(doc) = project.ds_doc.as_ref() {
            if let Some(axis) = doc.axes.first() {
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
                        .child(widgets::input::Input::new(&self.axis_map_input)),
                );
            }
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
                        .child(widgets::input::Input::new(&self.ease_input)),
                ),
        );
        Some(self.section(cx, "Axes", body))
    }

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

        let blur = self.preview_blur;
        let blur_cache = self.preview_blur_cache.clone();
        let invert = self.preview_invert;

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
                        line.extend((transform * path.as_ref().clone()).into_iter());
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
