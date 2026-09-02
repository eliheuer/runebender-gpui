// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An in-window menu bar, for the platforms with no native one:
//! Windows, Linux, and the browser.
//!
//! Replaces `gpui_component::menu::AppMenuBar`. It reads the same
//! `gpui::Menu` list the native bar is built from, so there is one
//! description of the menus and the two bars cannot drift.

use gpui::{
    Action, Context, InteractiveElement as _, IntoElement, Menu, MenuItem, MouseButton,
    ParentElement as _, Render, SharedString, Styled as _, Window, div, px,
};

use crate::view::theme as t;

/// One menu's title and its flattened items.
struct MenuEntry {
    /// The title shown in the bar.
    title: SharedString,
    /// The menu's items, submenus flattened to headings.
    items: Vec<Entry>,
}

/// One row of an open dropdown.
enum Entry {
    /// A clickable item that dispatches its action on the window.
    Action {
        /// The item's title.
        name: SharedString,
        /// The action the click dispatches.
        action: Box<dyn Action>,
    },
    /// A thin rule between groups of items.
    Separator,
    /// A submenu is shown inline under a heading: this bar is a
    /// fallback, and a nested popup is more machinery than the case
    /// needs.
    Heading(SharedString),
}

/// The in-window menu bar: one row of titles, one open dropdown at
/// a time.
pub(crate) struct MenuBar {
    /// The menus, in bar order.
    menus: Vec<MenuEntry>,
    /// Which title is open, if any.
    open: Option<usize>,
}

impl MenuBar {
    /// Build the bar from the same `gpui::Menu` list the native bar
    /// uses.
    pub(crate) fn new(menus: Vec<Menu>, cx: &mut Context<'_, Self>) -> Self {
        let _ = cx;
        Self {
            menus: menus.into_iter().map(convert).collect(),
            open: None,
        }
    }
}

/// Convert one `gpui::Menu` into a bar entry.
fn convert(menu: Menu) -> MenuEntry {
    let mut items = Vec::new();
    flatten(menu.items, &mut items);
    MenuEntry {
        title: menu.name,
        items,
    }
}

/// Flatten menu items into rows, turning a submenu into a heading
/// followed by its items.
fn flatten(source: Vec<MenuItem>, out: &mut Vec<Entry>) {
    for item in source {
        match item {
            MenuItem::Separator => out.push(Entry::Separator),
            MenuItem::Action { name, action, .. } => out.push(Entry::Action { name, action }),
            MenuItem::Submenu(sub) => {
                out.push(Entry::Separator);
                out.push(Entry::Heading(sub.name.clone()));
                flatten(sub.items, out);
            }
            MenuItem::SystemMenu(_) => {}
        }
    }
}

impl Render for MenuBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let open = self.open;
        let mut bar = div()
            .flex()
            .items_center()
            .h(px(24.0))
            .w_full()
            .bg(t::panel_bg())
            .border_b_1()
            .border_color(t::panel_outline());

        for (index, menu) in self.menus.iter().enumerate() {
            let is_open = open == Some(index);
            let title = div()
                .id(("menu-title", index))
                .px(px(10.0))
                .h_full()
                .flex()
                .items_center()
                .text_color(t::text())
                .when(is_open, |el| {
                    el.bg(t::selected_bg()).text_color(t::selected_ink())
                })
                .child(menu.title.clone())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this: &mut Self, _, _, cx| {
                        this.open = if this.open == Some(index) {
                            None
                        } else {
                            Some(index)
                        };
                        cx.notify();
                    }),
                );

            let mut cell = div().relative().h_full().child(title);
            if is_open {
                cell = cell.child(self.dropdown(index, cx));
            }
            bar = bar.child(cell);
        }
        bar
    }
}

use gpui::prelude::FluentBuilder as _;

impl MenuBar {
    /// The open menu's dropdown: its rows, and the click handlers
    /// that dispatch an action and close the menu.
    fn dropdown(&self, index: usize, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let mut list = div()
            .absolute()
            .top(px(24.0))
            .left(px(0.0))
            .min_w(px(200.0))
            .py(px(4.0))
            .bg(t::panel_bg())
            .border(t::stroke())
            .border_color(t::panel_outline())
            .flex()
            .flex_col();

        for (row, entry) in self.menus[index].items.iter().enumerate() {
            match entry {
                Entry::Separator => {
                    list = list.child(div().my(px(4.0)).h(px(1.0)).w_full().bg(t::panel_outline()));
                }
                Entry::Heading(name) => {
                    list = list.child(
                        div()
                            .px(px(10.0))
                            .py(px(2.0))
                            .text_color(t::text_muted())
                            .child(name.clone()),
                    );
                }
                Entry::Action { name, action } => {
                    let action = action.boxed_clone();
                    list = list.child(
                        div()
                            .id(("menu-item", index * 1000 + row))
                            .px(px(10.0))
                            .py(px(3.0))
                            .text_color(t::text())
                            .hover(|el| el.bg(t::selected_bg()).text_color(t::selected_ink()))
                            .child(name.clone())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this: &mut Self, _, window, cx| {
                                    this.open = None;
                                    window.dispatch_action(action.boxed_clone(), cx);
                                    cx.notify();
                                }),
                            ),
                    );
                }
            }
        }
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_submenu_is_flattened_under_a_heading() {
        let menu = Menu {
            name: "View".into(),
            disabled: false,
            items: vec![
                MenuItem::Separator,
                MenuItem::Submenu(Menu {
                    name: "Theme".into(),
                    disabled: false,
                    items: vec![MenuItem::Separator],
                }),
            ],
        };
        let entry = convert(menu);
        assert_eq!(entry.title, SharedString::from("View"));
        // separator, then the submenu's own separator + heading, then
        // its contents.
        assert!(matches!(entry.items[0], Entry::Separator));
        assert!(matches!(entry.items[1], Entry::Separator));
        assert!(matches!(entry.items[2], Entry::Heading(_)));
        assert!(matches!(entry.items[3], Entry::Separator));
    }
}
