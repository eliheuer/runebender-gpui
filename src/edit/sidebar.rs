// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! The left sidebar: categories, filters, search, and its small tools.
//!
//! The filter and search caches, saved filters, and the rows and tiles
//! the sidebar is built from.

use crate::*;

impl Workspace {
    /// Left sidebar tile: search plus the category filter list,
    /// like runebender-web's CategorySidebar.
    /// All codepoints of a glyph in the active master (norad keeps
    /// the full list; GlyphEntry only caches the first).
    pub(crate) fn glyph_codepoints(font: &Master, name: &str) -> Vec<u32> {
        font.font
            .get_glyph(name)
            .map(|g| g.codepoints.iter().map(|c| c as u32).collect())
            .unwrap_or_default()
    }

    /// Does a glyph pass the given sidebar filter?
    pub(crate) fn glyph_passes_filter(
        &self,
        font: &Master,
        name: &str,
        codepoint: Option<char>,
        filter: &SidebarFilter,
    ) -> bool {
        use runebender_core::analysis::category::GlyphCategory as GC;
        use runebender_core::ui::sidebar as sb;
        let category = codepoint.map(GC::from_codepoint).unwrap_or(GC::Other);
        match filter {
            SidebarFilter::All => true,
            SidebarFilter::Saved(si) => {
                let saved = read_saved_filters(&font.font);
                let Some((_, query)) = saved.get(*si) else {
                    return false;
                };
                match parse_search_predicates(query) {
                    Some(preds) => Self::glyph_matches_preds(font, name, codepoint, &preds),
                    None => name.contains(query.trim()),
                }
            }
            SidebarFilter::Category(c) => category == *c,
            SidebarFilter::Subfilter(c, sub) => {
                category == *c
                    && sb::glyph_matches_subfilter(name, &Self::glyph_codepoints(font, name), sub)
            }
            SidebarFilter::LanguageGroup(gi) => {
                sb::language_groups().get(*gi).is_some_and(|group| {
                    sb::glyph_matches_language_group(
                        name,
                        &Self::glyph_codepoints(font, name),
                        group,
                    )
                })
            }
            SidebarFilter::Language(gi, fi) => sb::language_groups()
                .get(*gi)
                .and_then(|group| group.filters.get(*fi))
                .is_some_and(|f| {
                    sb::glyph_matches_character_filter(name, &Self::glyph_codepoints(font, name), f)
                }),
            SidebarFilter::Builtin(bi) => {
                let Some(builtin) = sb::builtin_filters().get(*bi) else {
                    return false;
                };
                match &builtin.glyphset {
                    Some(set) => sb::glyph_matches_character_filter(
                        name,
                        &Self::glyph_codepoints(font, name),
                        set,
                    ),
                    // Runebender builtins: exporting = everything;
                    // incompatible = glyphs whose masters disagree.
                    None => match builtin.id.as_str() {
                        "incompatible" => self
                            .project
                            .as_ref()
                            .and_then(|p| p.compat.get(name))
                            .is_some_and(|ok| !ok),
                        _ => true,
                    },
                }
            }
        }
    }

    /// Rebuild the per-row counts and the current filter's match set.
    /// Called lazily from render after anything font-shaped changes.
    pub(crate) fn rebuild_sidebar_cache(&mut self) {
        use runebender_core::analysis::category::GlyphCategory as GC;
        use runebender_core::ui::sidebar as sb;
        let Some(font) = self.font() else {
            self.sidebar.counts = None;
            self.sidebar.matches = None;
            return;
        };
        let glyphs: Vec<(String, Option<char>, Vec<u32>)> = font
            .glyphs
            .iter()
            .map(|entry| {
                (
                    entry.name.to_string(),
                    entry.codepoint,
                    Self::glyph_codepoints(font, entry.name.as_ref()),
                )
            })
            .collect();
        let categories = SIDEBAR_CATEGORIES
            .iter()
            .map(|(category, _)| {
                if *category == GC::All {
                    glyphs.len()
                } else {
                    glyphs
                        .iter()
                        .filter(|(_, cp, _)| {
                            cp.map(GC::from_codepoint).unwrap_or(GC::Other) == *category
                        })
                        .count()
                }
            })
            .collect();
        let mut subfilters = std::collections::HashMap::new();
        for (ci, (category, label)) in SIDEBAR_CATEGORIES.iter().enumerate() {
            for (si, (sub, _)) in sb::category_subfilters(label).iter().enumerate() {
                let count = glyphs
                    .iter()
                    .filter(|(name, cp, cps)| {
                        cp.map(GC::from_codepoint).unwrap_or(GC::Other) == *category
                            && sb::glyph_matches_subfilter(name, cps, sub)
                    })
                    .count();
                subfilters.insert((ci, si), count);
            }
        }
        let name_cps: Vec<(String, Vec<u32>)> = glyphs
            .iter()
            .map(|(name, _, cps)| (name.clone(), cps.clone()))
            .collect();
        let mut groups = Vec::new();
        let mut languages = Vec::new();
        let mut missing = Vec::new();
        for group in sb::language_groups() {
            groups.push(
                glyphs
                    .iter()
                    .filter(|(name, _, cps)| sb::glyph_matches_language_group(name, cps, group))
                    .count(),
            );
            languages.push(
                group
                    .filters
                    .iter()
                    .map(|filter| {
                        glyphs
                            .iter()
                            .filter(|(name, _, cps)| {
                                sb::glyph_matches_character_filter(name, cps, filter)
                            })
                            .count()
                    })
                    .collect(),
            );
            missing.push(
                group
                    .filters
                    .iter()
                    .map(|filter| sb::missing_targets(&name_cps, filter).len())
                    .collect(),
            );
        }
        let builtins = sb::builtin_filters()
            .iter()
            .map(|builtin| match &builtin.glyphset {
                Some(set) => glyphs
                    .iter()
                    .filter(|(name, _, cps)| sb::glyph_matches_character_filter(name, cps, set))
                    .count(),
                None => match builtin.id.as_str() {
                    "incompatible" => self
                        .project
                        .as_ref()
                        .map(|p| p.compat.values().filter(|ok| !**ok).count())
                        .unwrap_or(0),
                    _ => glyphs.len(),
                },
            })
            .collect();
        let saved = self
            .font()
            .map(|font| {
                read_saved_filters(&font.font)
                    .iter()
                    .map(|(_, query)| {
                        let preds = parse_search_predicates(query);
                        glyphs
                            .iter()
                            .filter(|(name, cp, _)| match &preds {
                                Some(preds) => Self::glyph_matches_preds(font, name, *cp, preds),
                                None => name.contains(query.trim()),
                            })
                            .count()
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.sidebar.counts = Some(SidebarCounts {
            total: glyphs.len(),
            categories,
            subfilters,
            groups,
            languages,
            missing,
            builtins,
            saved,
        });
        self.rebuild_sidebar_matches();
    }

    /// Recompute the current filter's match set only (filter clicks).
    pub(crate) fn rebuild_sidebar_matches(&mut self) {
        let filter = self.sidebar.filter.clone();
        if filter == SidebarFilter::All {
            self.sidebar.matches = None;
            return;
        }
        let Some(font) = self.font() else {
            self.sidebar.matches = None;
            return;
        };
        let matches: std::collections::HashSet<String> = font
            .glyphs
            .iter()
            .filter(|entry| {
                self.glyph_passes_filter(font, entry.name.as_ref(), entry.codepoint, &filter)
            })
            .map(|entry| entry.name.to_string())
            .collect();
        self.sidebar.matches = Some(matches);
    }

    /// Does a glyph match the sidebar search, honoring scope, regex,
    /// and case options (web glyphMatchesSidebarSearch)?
    /// Evaluate a parsed predicate list against one glyph. Shared by
    /// the search field and saved sidebar filters.
    pub(crate) fn glyph_matches_preds(
        font: &Master,
        name: &str,
        codepoint: Option<char>,
        preds: &[SearchPred],
    ) -> bool {
        let Some(&index) = font.name_map.get(name) else {
            return false;
        };
        let entry = &font.glyphs[index];
        preds.iter().all(|pred| match pred {
            SearchPred::Width(order, value) => {
                let diff = entry.advance - value;
                match order {
                    std::cmp::Ordering::Greater => diff > 0.5,
                    std::cmp::Ordering::Less => diff < -0.5,
                    std::cmp::Ordering::Equal => diff.abs() <= 0.5,
                }
            }
            SearchPred::Category(want) => codepoint
                .map(|c| {
                    runebender_core::analysis::category::GlyphCategory::from_codepoint(c)
                        .display_name()
                        .to_lowercase()
                        .starts_with(want.as_str())
                })
                .unwrap_or(want == "unencoded"),
            SearchPred::MarkLabel(want) => match entry.mark.as_deref() {
                Some(label) => label.to_lowercase() == *want,
                None => want == "none",
            },
            SearchPred::Encoded(want) => codepoint.is_some() == *want,
            SearchPred::UsesComponent(base) => font
                .font
                .get_glyph(name)
                .is_some_and(|g| g.components.iter().any(|c| c.base.as_str() == base)),
            SearchPred::Has(what) => {
                font.font
                    .get_glyph(name)
                    .is_some_and(|g| match what.as_str() {
                        "contours" => !g.contours.is_empty(),
                        "components" => !g.components.is_empty(),
                        "anchors" => !g.anchors.is_empty(),
                        "note" => g.note.is_some(),
                        _ => false,
                    })
            }
        })
    }

    pub(crate) fn search_matches(&self, name: &str, codepoint: Option<char>) -> bool {
        let query = self.sidebar.search_query.trim();
        if query.is_empty() {
            return true;
        }
        // Predicate queries filter on glyph data (all terms must
        // hold); anything else falls through to text search.
        if let Some(preds) = &self.sidebar.search_predicates {
            let Some(font) = self.font() else { return true };
            return Self::glyph_matches_preds(font, name, codepoint, preds);
        }
        // Only build the codepoint haystacks the mode actually reads.
        let hex;
        let chars;
        let haystacks: [&str; 3] = match self.sidebar.search_mode {
            1 => [name, "", ""],
            2 => {
                hex = codepoint
                    .map(|c| format!("{:04X}", c as u32))
                    .unwrap_or_default();
                chars = codepoint.map(String::from).unwrap_or_default();
                ["", hex.as_str(), chars.as_str()]
            }
            _ => {
                hex = codepoint
                    .map(|c| format!("{:04X}", c as u32))
                    .unwrap_or_default();
                chars = codepoint.map(String::from).unwrap_or_default();
                [name, hex.as_str(), chars.as_str()]
            }
        };
        let any = |f: &dyn Fn(&str) -> bool| haystacks.iter().any(|h| !h.is_empty() && f(h));
        if self.sidebar.search_regex {
            // Compiled once when the query changed, not per glyph: a
            // font-wide filter used to build 862 regexes a frame.
            return match &self.sidebar.search_re {
                Some(re) => any(&|h| re.is_match(h)),
                // A half-typed pattern matches everything, like the web.
                None => true,
            };
        }
        if self.sidebar.search_case {
            any(&|h| h.contains(query))
        } else {
            let needle = query.to_lowercase();
            any(&|h| h.to_lowercase().contains(&needle))
        }
    }

    /// Recompile the search pattern. Called when the query or the
    /// case flag changes.
    pub(crate) fn rebuild_search_regex(&mut self) {
        self.sidebar.search_re = None;
        let query = self.sidebar.search_query.trim();
        self.sidebar.search_predicates = parse_search_predicates(query);
        if !self.sidebar.search_regex || query.is_empty() {
            return;
        }
        let pattern = if self.sidebar.search_case {
            query.to_string()
        } else {
            format!("(?i){query}")
        };
        self.sidebar.search_re = regex::Regex::new(&pattern).ok();
    }

    /// Pin the current search query as a saved filter in the font lib.
    pub(crate) fn save_current_search_as_filter(&mut self) {
        let query = self.sidebar.search_query.trim().to_string();
        if query.is_empty() {
            return;
        }
        let Some(font) = self.font_mut() else { return };
        let mut saved = read_saved_filters(&font.font);
        if saved.iter().any(|(_, q)| *q == query) {
            return;
        }
        saved.push((query.clone(), query));
        write_saved_filters(&mut font.font, &saved);
        font.dirty = true;
        let index = saved.len() - 1;
        self.sidebar.counts = None;
        self.set_sidebar_filter(SidebarFilter::Saved(index));
    }

    /// Remove one saved filter, keeping the selection sensible.
    pub(crate) fn delete_saved_filter(&mut self, si: usize) {
        let Some(font) = self.font_mut() else { return };
        let mut saved = read_saved_filters(&font.font);
        if si >= saved.len() {
            return;
        }
        saved.remove(si);
        write_saved_filters(&mut font.font, &saved);
        font.dirty = true;
        self.sidebar.counts = None;
        match self.sidebar.filter {
            SidebarFilter::Saved(active) if active == si => {
                self.set_sidebar_filter(SidebarFilter::All);
            }
            SidebarFilter::Saved(active) if active > si => {
                self.set_sidebar_filter(SidebarFilter::Saved(active - 1));
            }
            _ => self.rebuild_sidebar_matches(),
        }
    }

    /// Select a sidebar row.
    pub(crate) fn set_sidebar_filter(&mut self, filter: SidebarFilter) {
        self.sidebar.filter = filter;
        // A different set of glyphs starts at the top.
        self.grid.scroll_row = 0;
        self.rebuild_sidebar_matches();
    }

    /// A small disclosure triangle for expandable sidebar rows
    /// (painted: IBM Plex has no triangle codepoints).
    pub(crate) fn row_chevron(expanded: bool) -> impl IntoElement {
        canvas(
            move |bounds, _, _| bounds,
            move |_, bounds: Bounds<gpui::Pixels>, window, _| {
                let o = bounds.origin;
                let w: f32 = bounds.size.width.into();
                let h: f32 = bounds.size.height.into();
                let (cx_, cy) = (w / 2.0, h / 2.0);
                let mut path = gpui::PathBuilder::fill();
                let pt = |dx: f32, dy: f32| gpui::point(o.x + px(cx_ + dx), o.y + px(cy + dy));
                if expanded {
                    path.move_to(pt(-3.5, -1.5));
                    path.line_to(pt(3.5, -1.5));
                    path.line_to(pt(0.0, 2.5));
                } else {
                    path.move_to(pt(-1.5, -3.5));
                    path.line_to(pt(2.5, 0.0));
                    path.line_to(pt(-1.5, 3.5));
                }
                if let Ok(p) = path.build() {
                    window.paint_path(p, t::text_muted());
                }
            },
        )
        .w(px(10.0))
        .h(px(10.0))
    }

    /// One sidebar row: optional chevron, optional icon, label, and a
    /// right-aligned count ("n" or "n/m" coverage).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn sidebar_row(
        &self,
        id: (&'static str, usize),
        indent: bool,
        chevron: Option<bool>,
        icon: Option<SharedString>,
        label: SharedString,
        count: SharedString,
        filter: SidebarFilter,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let active = self.sidebar.filter == filter;
        div()
            .id(id)
            // Fixed row height, no vertical padding: Glyphs' sidebar
            // packs its rows tight, and leading is what made ours look
            // twice as tall as it needed to be.
            .h(px(20.0))
            .px_2()
            .when(indent, |el| el.ml_3())
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
            .when_some(chevron, |el, expanded| {
                el.child(Self::row_chevron(expanded))
            })
            .when_some(icon, |el, icon| {
                el.child(
                    div()
                        .w(px(16.0))
                        .text_color(if active { t::accent() } else { t::text_muted() })
                        .child(icon),
                )
            })
            .child(div().flex_1().child(label))
            .child(
                div()
                    .text_color(if active { t::accent() } else { t::text_muted() })
                    .child(count),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_sidebar_filter(filter.clone());
                cx.notify();
            }))
    }

    /// A tiny toggle beside the search box (scope / regex / case).
    pub(crate) fn search_toggle(
        &self,
        id: &'static str,
        label: &'static str,
        active: bool,
        on: fn(&mut Self),
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .w(px(24.0))
            // No fixed height: the row stretches these to the search
            // input's height so the whole strip lines up.
            .rounded(t::radius())
            .border(t::stroke())
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .cursor_pointer()
            .when(active, |el| {
                el.border_color(t::accent()).text_color(t::accent())
            })
            .when(!active, |el| {
                el.border_color(t::cell_border())
                    .text_color(t::text_muted())
            })
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                on(this);
                cx.notify();
            }))
    }

    /// Set or clear the mark color on every selected glyph.
    pub(crate) fn set_selected_mark(&mut self, label: Option<String>) {
        let names = self.selection_names();
        if names.is_empty() {
            return;
        }
        let Some(font) = self.font_mut() else { return };
        for name in names {
            if let Some(&index) = font.name_map.get(&name) {
                font.edit_glyph(index, |glyph| {
                    runebender_core::ui::theme_oklch::set_glyph_mark(glyph, label.as_deref());
                });
            }
        }
    }

    /// The Shapes tab: one row per contour and per component in the
    /// open glyph, like the web's sidebar. A row selects what it names.
    pub(crate) fn sidebar_shapes(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut list = div().flex().flex_col().gap_1().p_2();
        let (Mode::Editor(index), Some(font)) = (&self.mode, self.font()) else {
            return list.child(
                div()
                    .text_xs()
                    .text_color(t::text_muted())
                    .child("No glyph open."),
            );
        };
        let index = *index;
        let entry = &font.glyphs[index];
        let Some(glyph) = font.font.get_glyph(entry.name.as_ref()) else {
            return list;
        };
        let row = |id: (&'static str, usize),
                   mark: &'static str,
                   label: SharedString,
                   detail: SharedString,
                   active: bool| {
            div()
                .id(id)
                .h(px(20.0))
                .px_1()
                .flex()
                .items_center()
                .gap_2()
                .rounded(t::radius())
                .text_xs()
                .cursor_pointer()
                .when(active, |el| {
                    el.bg(t::cell_selected_bg()).text_color(t::text())
                })
                .when(!active, |el| el.text_color(t::text_muted()))
                .child(div().w(px(10.0)).child(mark))
                .child(div().flex_1().child(label))
                .child(div().text_color(t::text_muted()).child(detail))
        };

        let counts: Vec<usize> = glyph.contours.iter().map(|c| c.points.len()).collect();
        for (ci, points) in counts.iter().copied().enumerate() {
            let selected = self
                .editor
                .selected
                .iter()
                .any(|(contour, _)| *contour == ci);
            list = list.child(
                row(
                    ("shape-contour", ci),
                    "◌",
                    format!("contour {}", ci + 1).into(),
                    format!("{points} nodes").into(),
                    selected,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let Mode::Editor(index) = this.mode else {
                        return;
                    };
                    this.editor.selected_component = None;
                    this.editor.selected = this
                        .font()
                        .map(|f| {
                            f.glyphs[index]
                                .points
                                .iter()
                                .filter(|p| p.contour == ci)
                                .map(|p| (p.contour, p.index))
                                .collect()
                        })
                        .unwrap_or_default();
                    cx.notify();
                })),
            );
        }
        let bases: Vec<String> = glyph
            .components
            .iter()
            .map(|c| c.base.to_string())
            .collect();
        for (i, base) in bases.into_iter().enumerate() {
            let selected = self.editor.selected_component == Some(i);
            list = list.child(
                row(
                    ("shape-component", i),
                    "◇",
                    base.into(),
                    "component".into(),
                    selected,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.editor.selected.clear();
                    this.editor.selected_component = Some(i);
                    cx.notify();
                })),
            );
        }
        if counts.is_empty() && glyph.components.is_empty() {
            list = list.child(
                div()
                    .text_xs()
                    .text_color(t::text_muted())
                    .child("No shapes in this glyph yet."),
            );
        }
        list
    }

    /// The glyph editor: metrics lines, stroked outline over a dim
    /// fill, draggable control points, wheel pan, Cmd+wheel zoom.
    /// A flat docked sidebar section: small muted header with a
    /// disclosure triangle, hairline divider below (Glyphs-style, no
    /// floating container). Clicking the header folds the body.
    pub(crate) fn section(
        &self,
        cx: &mut Context<Self>,
        title: &'static str,
        body: impl IntoElement,
    ) -> gpui::Div {
        let collapsed = self.collapsed_sections.contains(title);
        div()
            .flex()
            .flex_col()
            .gap_1()
            .px_2()
            .py_1p5()
            .border_b_1()
            .border_color(t::panel_outline())
            .child(
                div()
                    .id(gpui::SharedString::from(format!("section-{title}")))
                    .flex()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .text_xs()
                    .text_color(t::text_muted())
                    .child(
                        canvas(
                            move |bounds, _, _| bounds,
                            move |_, bounds: Bounds<gpui::Pixels>, window, _| {
                                let o = bounds.origin;
                                let w: f32 = bounds.size.width.into();
                                let h: f32 = bounds.size.height.into();
                                let (cx_, cy) = (w / 2.0, h / 2.0);
                                let mut path = gpui::PathBuilder::fill();
                                let pt = |dx: f32, dy: f32| {
                                    gpui::point(o.x + px(cx_ + dx), o.y + px(cy + dy))
                                };
                                if collapsed {
                                    path.move_to(pt(-1.5, -3.5));
                                    path.line_to(pt(2.5, 0.0));
                                    path.line_to(pt(-1.5, 3.5));
                                } else {
                                    path.move_to(pt(-3.5, -1.5));
                                    path.line_to(pt(3.5, -1.5));
                                    path.line_to(pt(0.0, 2.5));
                                }
                                if let Ok(p) = path.build() {
                                    window.paint_path(p, t::text_muted());
                                }
                            },
                        )
                        .w(px(10.0))
                        .h(px(10.0)),
                    )
                    .child(title)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.collapsed_sections.remove(title) {
                            this.collapsed_sections.insert(title);
                        }
                        cx.notify();
                    })),
            )
            .when(!collapsed, |el| el.child(body))
    }

    /// A 30px icon tile (header tools, transform section).
    pub(crate) fn icon_tile(
        id: &'static str,
        icon: &'static str,
        active: bool,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .w(px(30.0))
            .h(px(30.0))
            .rounded(t::radius_control())
            .cursor_pointer()
            .when(active, |el| el.bg(t::cell_selected_bg()))
            .child(icon_svg(icon, if active { t::accent() } else { t::text() }))
    }

    /// Tool icons for the header bar (editor mode only).
    pub(crate) fn header_tools(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let tool = self.editor.tool;
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                Self::icon_tile("tool-select", "select", tool == Tool::Select).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.pen_finish();
                        this.editor.tool = Tool::Select;
                        cx.notify();
                    }),
                ),
            )
            .child(
                Self::icon_tile("tool-pen", "pen", tool == Tool::Pen).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.editor.tool = Tool::Pen;
                        cx.notify();
                    },
                )),
            )
            .child(
                Self::icon_tile(
                    "tool-shapes",
                    if self.editor.shape_ellipse {
                        "shape-ellipse"
                    } else {
                        "shape-rectangle"
                    },
                    tool == Tool::Shapes,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.editor.tool == Tool::Shapes {
                        this.editor.shape_ellipse = !this.editor.shape_ellipse;
                    }
                    this.pen_finish();
                    this.editor.tool = Tool::Shapes;
                    cx.notify();
                })),
            )
            .child(
                Self::icon_tile("tool-measure", "measure", tool == Tool::Measure).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.pen_finish();
                        this.editor.tool = Tool::Measure;
                        cx.notify();
                    }),
                ),
            )
            .child(
                Self::icon_tile("tool-text", "text", tool == Tool::Text).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.pen_finish();
                        this.editor.tool = Tool::Text;
                        cx.notify();
                    },
                )),
            )
            .child(
                Self::icon_tile("tool-knife", "knife", tool == Tool::Knife).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.pen_finish();
                        this.editor.tool = Tool::Knife;
                        cx.notify();
                    },
                )),
            )
            .child(
                Self::icon_tile("tool-hyperpen", "hyperpen", tool == Tool::HyperPen).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.pen_finish();
                        this.editor.tool = Tool::HyperPen;
                        cx.notify();
                    }),
                ),
            )
            .child(
                Self::icon_tile("tool-preview", "preview", tool == Tool::Preview).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.pen_finish();
                        if this.editor.tool == Tool::Preview {
                            this.editor.tool = this.editor.previous_tool;
                        } else {
                            this.editor.previous_tool = this.editor.tool;
                            this.editor.tool = Tool::Preview;
                        }
                        cx.notify();
                    }),
                ),
            )
    }

    /// Text direction control (text tool): LTR / RTL / Auto, like
    /// the web editor's TextDirectionToolbar.
    pub(crate) fn direction_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        use runebender_core::text::buffer::TextDirection;
        let auto = self.edit_buffer.direction_is_auto();
        let dir = self.edit_buffer.direction();
        let button = |id: &'static str, label: &'static str, active: bool| {
            div()
                .id(id)
                .px_2()
                .py_0p5()
                .rounded(t::radius())
                .border(t::stroke())
                .border_color(if active {
                    t::accent()
                } else {
                    t::cell_border()
                })
                .text_sm()
                .text_color(if active { t::accent() } else { t::text_muted() })
                .cursor_pointer()
                .child(label)
        };
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                button("dir-ltr", "LTR", !auto && dir == TextDirection::LeftToRight).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.edit_buffer.set_direction(
                            runebender_core::text::buffer::TextDirection::LeftToRight,
                        );
                        this.edit_buffer.shape_arabic_if_rtl();
                        this.sync_sort_offset();
                        cx.notify();
                    }),
                ),
            )
            .child(
                button("dir-rtl", "RTL", !auto && dir == TextDirection::RightToLeft).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.edit_buffer.set_direction(
                            runebender_core::text::buffer::TextDirection::RightToLeft,
                        );
                        this.edit_buffer.shape_arabic_if_rtl();
                        this.sync_sort_offset();
                        cx.notify();
                    }),
                ),
            )
            .child(
                button("dir-auto", "Auto", auto).on_click(cx.listener(|this, _, _, cx| {
                    this.edit_buffer.set_auto_direction();
                    this.edit_buffer.shape_arabic_if_rtl();
                    this.sync_sort_offset();
                    cx.notify();
                })),
            )
    }
}
