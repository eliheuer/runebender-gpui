// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The left edge: the tab strip and the category sidebar it opens.

use crate::Mode;
use crate::Workspace;
use crate::edit::sidebar::RowMark;
use crate::view::paint::icon_svg;
use crate::view::theme as t;
use crate::widgets;
use crate::workspace::SIDEBAR_CATEGORIES;
use crate::workspace::SidebarFilter;
use crate::workspace::TAB_H;
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
use runebender_core::formats::lib_keys::read_saved_filters;
impl Workspace {
    /// The category sidebar: expandable category rows with glyph
    /// counts, plus the saved filters.
    pub(crate) fn category_sidebar(&self, cx: &mut Context<'_, Self>) -> impl IntoElement + use<> {
        use runebender_core::ui::sidebar as sb;
        let counts = self.sidebar.counts.as_ref();

        // The Font view keeps the same sidebar modes as the editor where
        // they remain useful. Grid/List already have controls in the status
        // bar, while axes, local models, and chat need a stable home here.
        let has_axes = !self.axis_sliders.is_empty();
        let tab_now = if !has_axes && self.sidebar.tab == 2 || self.sidebar.tab == 1 {
            0
        } else {
            self.sidebar.tab
        };
        let on_categories = tab_now == 0;
        let tab_rail = t::tab_rail_bg();
        let sidebar_tab =
            |id: &'static str, icon: &'static str, which: u8, cx: &mut Context<'_, Self>| {
                let active = tab_now == which;
                let inactive_ink = gpui::Rgba {
                    a: 0.42,
                    ..t::text()
                };
                let face = div()
                    .id(id)
                    .h(px(if active { 32.0 } else { 28.0 }))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(active, |el| {
                        el.rounded_t(px(6.0))
                            .border_t_1()
                            .border_l_1()
                            .border_r_1()
                            .border_color(t::cell_border())
                            .bg(t::panel_bg())
                    })
                    .when(!active, |el| {
                        el.rounded(px(6.0))
                            .border_1()
                            .border_color(t::cell_border())
                            .bg(t::inactive_tab_bg())
                    })
                    .cursor_pointer()
                    .group(id)
                    .child(if active {
                        div()
                            .size(px(18.0))
                            .relative()
                            .top(px(-2.0))
                            .child(icon_svg(icon, t::text()))
                    } else {
                        div()
                            .size(px(18.0))
                            .relative()
                            .child(icon_svg(icon, inactive_ink))
                            .child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .invisible()
                                    .group_hover(id, |el| el.visible())
                                    .child(icon_svg(icon, t::text())),
                            )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.sidebar.tab = which;
                        cx.notify();
                    }));
                div().h(px(32.0)).flex_1().relative().child(face)
            };

        // Categories: expandable rows with the web's subfilters.
        let mut categories = div().flex().flex_col();
        for (ci, (category, label)) in SIDEBAR_CATEGORIES.iter().enumerate() {
            let subs = sb::category_subfilters(label);
            let count = counts.map(|c| c.categories[ci]).unwrap_or(0);
            let expanded = self.sidebar.expanded_categories.contains(&ci);
            let mut row = self
                .sidebar_row(
                    ("category", ci),
                    false,
                    Some(if subs.is_empty() {
                        RowMark::Bullet
                    } else {
                        RowMark::Chevron(expanded)
                    }),
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
                let selected = self.sidebar.filter == SidebarFilter::Category(category)
                    || subs.iter().any(|(sub, _)| {
                        self.sidebar.filter == SidebarFilter::Subfilter(category, sub)
                    });
                row = self
                    .sidebar_row(
                        ("category", ci),
                        false,
                        Some(RowMark::Chevron(expanded)),
                        None,
                        SharedString::from(*label),
                        format!("{count}").into(),
                        SidebarFilter::Category(category),
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if selected && !this.sidebar.expanded_categories.remove(&ci) {
                            this.sidebar.expanded_categories.insert(ci);
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

        // Global Scripts: script groups with per-set coverage, like the
        // web sidebar and Glyphs.
        let mut languages = div().flex().flex_col();
        for (gi, group) in sb::language_groups().iter().enumerate() {
            let count = counts.map(|c| c.groups[gi]).unwrap_or(0);
            let expanded = self.sidebar.expanded_scripts.contains(&gi);
            let selected = self.sidebar.filter == SidebarFilter::LanguageGroup(gi)
                || (0..group.filters.len())
                    .any(|fi| self.sidebar.filter == SidebarFilter::Language(gi, fi));
            languages = languages.child(
                self.sidebar_row(
                    ("script", gi),
                    false,
                    Some(if group.filters.is_empty() {
                        RowMark::Bullet
                    } else {
                        RowMark::Chevron(expanded)
                    }),
                    None,
                    group.label.clone().into(),
                    format!("{count}").into(),
                    SidebarFilter::LanguageGroup(gi),
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if selected {
                        if !this.sidebar.expanded_scripts.remove(&gi) {
                            this.sidebar.expanded_scripts.insert(gi);
                        }
                    } else {
                        this.sidebar.expanded_scripts.insert(gi);
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
            let active = self.sidebar.filter == SidebarFilter::Saved(si);
            filters = filters.child(
                div()
                    .id(("saved-filter", si))
                    .group("saved-filter")
                    .h(px(20.0))
                    .px_2()
                    .rounded(t::radius())
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap_1()
                    .when(active, |el| {
                        el.border(t::stroke())
                            .bg(t::selected_bg())
                            .border_color(t::selected_outline())
                            .text_color(t::selected_ink())
                    })
                    .when(!active, |el| el.text_color(t::text()))
                    .child(
                        div()
                            .w(px(16.0))
                            .text_color(if active {
                                t::selected_ink()
                            } else {
                                t::text_muted()
                            })
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
                            .text_color(if active {
                                t::selected_ink()
                            } else {
                                t::text_muted()
                            })
                            .child(SharedString::from(format!("{count}"))),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_sidebar_filter(SidebarFilter::Saved(si));
                        cx.notify();
                    })),
            );
        }
        let pending_query = self.sidebar.search_query.trim().to_string();
        if !pending_query.is_empty() && !saved_defs.iter().any(|(_, q)| *q == pending_query) {
            filters = filters.child(
                div()
                    .id("save-search-filter")
                    .h(px(20.0))
                    .px_2()
                    .rounded(t::radius())
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

        let categories_pane = div()
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
                div()
                    .id("sidebar-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .child(self.section(cx, "Categories", categories))
                    .child(self.section(cx, "Global Scripts", languages))
                    .child(self.section(cx, "Filters", filters)),
            )
            // Mark colours sit at the foot of the sidebar, beside the
            // glyphs they apply to, the way the web places them.
            .child(self.mark_colors_panel(cx));

        div()
            .size_full()
            .flex()
            .flex_col()
            .min_h(px(0.0))
            .child(
                div()
                    .h(px(36.0))
                    .relative()
                    .px(px(4.0))
                    .pt(px(4.0))
                    .flex()
                    .items_start()
                    .gap(px(4.0))
                    .bg(tab_rail)
                    .child(
                        div()
                            .absolute()
                            .bottom_0()
                            .left_0()
                            .right_0()
                            .h(px(1.0))
                            .bg(t::cell_border()),
                    )
                    .child(sidebar_tab("font-tab-glyphs", "glyph-grid", 0, cx))
                    .when(has_axes, |el| {
                        el.child(sidebar_tab("font-tab-axes", "measure", 2, cx))
                    })
                    .child(sidebar_tab("font-tab-ai", "preview", 3, cx))
                    .child(sidebar_tab("font-tab-chat", "text", 4, cx)),
            )
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
                        .id("font-sidebar-ai")
                        .flex_1()
                        .min_h(px(0.0))
                        .overflow_y_scroll()
                        .p_2()
                        .child(self.local_ai_panel(cx)),
                )
            })
            .when(tab_now == 4, |el| {
                el.child(
                    div()
                        .flex_1()
                        .min_h(px(0.0))
                        .p_2()
                        .child(self.chat_panel(cx)),
                )
            })
            .when(on_categories, |el| el.child(categories_pane))
    }

    /// The Glyphs-style tab strip under the header: a Font tab that
    /// returns to the full glyph overview, plus one tab per edit
    /// session, titled with the session's text.
    pub(crate) fn tab_strip(&self, cx: &mut Context<'_, Self>) -> impl IntoElement + use<> {
        if self.project.is_none() {
            return div().into_any_element();
        }
        let in_editor = matches!(self.mode, Mode::Editor(_));
        let in_nodes = matches!(self.mode, Mode::Nodes);
        let tab = |id: gpui::ElementId, label: SharedString, active: bool| {
            div()
                .id(id)
                .h(px(TAB_H))
                .px_2()
                .flex()
                .items_center()
                .rounded(t::radius())
                .cursor_pointer()
                .when(active, |el| {
                    el.border(t::stroke())
                        .border_color(t::mark_color("orange").unwrap_or_else(t::selected_ink))
                        .text_color(t::mark_color("yellow").unwrap_or_else(t::selected_ink))
                })
                .when(!active, |el| {
                    el.border(t::stroke())
                        .border_color(gpui::Rgba {
                            a: 0.45,
                            ..t::selected_ink()
                        })
                        .text_color(gpui::Rgba {
                            a: 0.72,
                            ..t::selected_ink()
                        })
                })
                .child(label)
        };
        // Each session tab reads like Glyphs: the buffer's text, with
        // /name for unencoded glyphs, trimmed to fit.
        let session_label = |buffer: &runebender_core::text::buffer::TextBuffer,
                             fallback: &str|
         -> SharedString {
            let mut label = String::new();
            for i in 0..buffer.len() {
                let Some(sort) = buffer.sort(i) else {
                    continue;
                };
                if sort.is_absorbed() {
                    continue;
                }
                match &sort.kind {
                    runebender_core::text::buffer::TextSortKind::Glyph {
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
                        Mode::Grid | Mode::Nodes => s.glyph_name.clone(),
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
                tab("tab-font".into(), "Font".into(), !in_editor && !in_nodes).on_click(
                    cx.listener(|this, _, _, cx| {
                        if let Mode::Editor(index) = this.mode {
                            this.last_editor = Some(index);
                            let name = this.font().map(|f| f.glyphs[index].name.to_string());
                            if let (Some(name), Some(project)) = (name, this.project.as_mut()) {
                                project.recheck_compat(&name);
                            }
                        }
                        this.mode = Mode::Grid;
                        this.status_note = None;
                        cx.notify();
                    }),
                ),
            )
            // Nodes sits beside Font: the workflow over the font, as
            // boxes and wires.
            .child(
                tab("tab-nodes".into(), "Nodes".into(), in_nodes).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.enter_nodes_mode();
                        cx.notify();
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
                // Round, so it sits with the window's corner rather
                // than fighting it.
                tab("tab-new".into(), "+".into(), false)
                    .w(px(TAB_H))
                    .rounded_full()
                    .justify_center()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.command_new_session();
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
}
