// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The inspector's controls, built in one place.
//!
//! A panel is a list of rows. A row holds buttons, toggles, or a
//! label with a field, and every one of them stands on the same
//! height, reads at the same size, and fills the row instead of
//! sitting at whatever width its label happens to need. Nothing here
//! names a colour or a measurement: the theme and the constants do.

use crate::view::theme as t;
use crate::widgets::input::Input;
use gpui::ElementId;
use gpui::InteractiveElement;
use gpui::ParentElement;
use gpui::SharedString;
use gpui::Stateful;
use gpui::Styled;
use gpui::div;
use gpui::px;

/// The height of every control in a panel: a button, a toggle, a
/// field, an icon tile. One number, so a row of them lines up.
pub(crate) const CONTROL_H: f32 = 28.0;

/// The width a row label takes, so fields in different rows start on
/// the same line.
pub(crate) const LABEL_W: f32 = 88.0;

/// A row of controls that share the width equally.
pub(crate) fn row() -> gpui::Div {
    div().flex().items_center().gap_1().w_full()
}

/// A column of rows.
pub(crate) fn column() -> gpui::Div {
    div().flex().flex_col().gap_1().w_full()
}

/// What every pressable control shares: the height, the type size,
/// the rule, and filling its share of the row.
fn base(id: impl Into<ElementId>) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_1()
        .min_w_0()
        .h(px(CONTROL_H))
        .px_2()
        .flex()
        .items_center()
        .justify_center()
        .overflow_hidden()
        .whitespace_nowrap()
        .rounded(t::radius())
        .border(t::stroke())
        .text_xs()
        .cursor_pointer()
}

/// A command: press it and something happens.
pub(crate) fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
) -> Stateful<gpui::Div> {
    base(id)
        .border_color(t::cell_border())
        .text_color(t::text())
        .child(label.into())
}

/// A state: press it and it stays on, in the accent.
pub(crate) fn toggle(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
) -> Stateful<gpui::Div> {
    let el = base(id).child(label.into());
    if active {
        el.bg(t::selected_bg())
            .border_color(t::selected_bg())
            .text_color(t::selected_ink())
    } else {
        el.border_color(t::cell_border()).text_color(t::text())
    }
}

/// A row label: the noun for the field beside it, with its unit.
pub(crate) fn label(text: impl Into<SharedString>) -> gpui::Div {
    div()
        .flex_none()
        .w(px(LABEL_W))
        .text_xs()
        .text_color(t::text_muted())
        .overflow_hidden()
        .whitespace_nowrap()
        .child(text.into())
}

/// A label and its field on one row. The field takes what the label
/// leaves.
pub(crate) fn field(text: impl Into<SharedString>, input: Input) -> gpui::Div {
    row()
        .child(label(text))
        .child(div().flex_1().min_w_0().child(input))
}
