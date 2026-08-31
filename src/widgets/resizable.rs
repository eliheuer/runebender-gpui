// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Resizable panel groups: a row or a column of panels with a
//! draggable divider between them.
//!
//! Replaces `gpui_component::resizable`. Sizes live in a global keyed
//! by the group's id, so a drag survives the next render.
//!
//! A panel given a size keeps it and is resized by the dividers
//! beside it. A panel without one takes whatever is left.

use std::collections::HashMap;
use std::ops::Range;

use gpui::{
    AnyElement, App, AppContext as _, Axis, Bounds, Global, InteractiveElement as _, IntoElement,
    MouseButton, MouseDownEvent, ParentElement as _, Pixels, SharedString,
    StatefulInteractiveElement as _, Styled as _, canvas, div, px,
};

use crate::view::theme as t;

/// How thick the divider is, and how much of it answers to the mouse.
const DIVIDER: f32 = 1.0;
/// The width of the invisible strip over the divider that catches the mouse.
const GRIP: f32 = 6.0;

#[derive(Default)]
/// Every group's sizes and painted bounds, held as a global.
struct ResizableState {
    /// Panel sizes, per group id, indexed by panel position.
    sizes: HashMap<SharedString, Vec<Option<Pixels>>>,
    /// Where each panel painted, so a drag can measure against it.
    bounds: HashMap<(SharedString, usize), Bounds<Pixels>>,
}

impl Global for ResizableState {}

/// The size stored for one panel, if a drag or builder has set one.
fn stored_size(cx: &App, group: &SharedString, index: usize) -> Option<Pixels> {
    cx.try_global::<ResizableState>()?
        .sizes
        .get(group)?
        .get(index)
        .copied()
        .flatten()
}

/// Record a panel's size, growing the group's list to reach it.
fn store_size(cx: &mut App, group: &SharedString, index: usize, size: Pixels) {
    let state = cx.default_global::<ResizableState>();
    let sizes = state.sizes.entry(group.clone()).or_default();
    if sizes.len() <= index {
        sizes.resize(index + 1, None);
    }
    sizes[index] = Some(size);
}

/// Record where a panel painted.
fn store_bounds(cx: &mut App, group: &SharedString, index: usize, bounds: Bounds<Pixels>) {
    cx.default_global::<ResizableState>()
        .bounds
        .insert((group.clone(), index), bounds);
}

/// Where a panel last painted, if it has.
fn panel_bounds(cx: &App, group: &SharedString, index: usize) -> Option<Bounds<Pixels>> {
    cx.try_global::<ResizableState>()?
        .bounds
        .get(&(group.clone(), index))
        .copied()
}

/// One panel in a group.
pub struct ResizablePanel {
    /// The panel's size. `None` means take the space the sized panels leave.
    size: Option<Pixels>,
    /// The limits a drag clamps to, when set.
    range: Option<Range<Pixels>>,
    /// Whether the panel takes space and grows a divider.
    visible: bool,
    /// The panel's content.
    child: Option<AnyElement>,
}

/// An empty, visible panel with no size of its own.
pub fn resizable_panel() -> ResizablePanel {
    ResizablePanel {
        size: None,
        range: None,
        visible: true,
        child: None,
    }
}

impl ResizablePanel {
    /// The size this panel starts at. Without one it takes the space
    /// the sized panels leave.
    pub fn size(mut self, size: Pixels) -> Self {
        self.size = Some(size);
        self
    }

    /// How far a drag may take it.
    pub fn size_range(mut self, range: Range<Pixels>) -> Self {
        self.range = Some(range);
        self
    }

    /// A hidden panel takes no space and grows no divider.
    pub fn visible(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }

    /// Set the panel's content.
    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.child = Some(child.into_any_element());
        self
    }
}

/// A row or column of panels.
#[derive(gpui::IntoElement)]
pub struct ResizableGroup {
    /// Keys the group's stored sizes and bounds.
    id: SharedString,
    /// Whether the panels run in a row or a column.
    axis: Axis,
    /// The panels, in order.
    panels: Vec<ResizablePanel>,
}

/// Panels side by side, dividers running vertically.
pub fn h_resizable(id: impl Into<SharedString>) -> ResizableGroup {
    ResizableGroup {
        id: id.into(),
        axis: Axis::Horizontal,
        panels: Vec::new(),
    }
}

/// Panels stacked, dividers running horizontally.
pub fn v_resizable(id: impl Into<SharedString>) -> ResizableGroup {
    ResizableGroup {
        id: id.into(),
        axis: Axis::Vertical,
        panels: Vec::new(),
    }
}

impl ResizableGroup {
    /// Append a panel to the group.
    pub fn child(mut self, panel: ResizablePanel) -> Self {
        self.panels.push(panel);
        self
    }

    /// Assemble the panels and their dividers into one flex element.
    fn build(self, cx: &mut App) -> AnyElement {
        let horizontal = matches!(self.axis, Axis::Horizontal);
        let group = self.id.clone();

        // Resolve every visible panel's size before building, so a
        // divider knows which panel it moves.
        let visible: Vec<usize> = self
            .panels
            .iter()
            .enumerate()
            .filter(|(_, p)| p.visible)
            .map(|(i, _)| i)
            .collect();

        let mut container = div().flex().size_full().min_w(px(0.0)).min_h(px(0.0));
        if horizontal {
            container = container.flex_row();
        } else {
            container = container.flex_col();
        }

        let mut panels = self.panels;
        let dividers: Vec<Option<usize>> = visible
            .iter()
            .enumerate()
            .map(|(slot, _)| {
                if slot + 1 >= visible.len() {
                    return None;
                }
                let before = visible[slot];
                let after = visible[slot + 1];
                if panels.get(before).is_some_and(|p| p.size.is_some()) {
                    Some(before)
                } else if panels.get(after).is_some_and(|p| p.size.is_some()) {
                    Some(after)
                } else {
                    None
                }
            })
            .collect();

        for (slot, index) in visible.iter().copied().enumerate() {
            let panel = &mut panels[index];
            let child = panel.child.take();
            let size = panel
                .size
                .map(|initial| stored_size(cx, &group, index).unwrap_or(initial));
            let range = panel.range.clone();

            let mut cell = div().flex().min_w(px(0.0)).min_h(px(0.0));
            cell = match (size, horizontal) {
                (Some(size), true) => cell.w(size).flex_shrink_0(),
                (Some(size), false) => cell.h(size).flex_shrink_0(),
                (None, _) => cell.flex_1(),
            };

            // Record where it landed, so a drag has something to
            // measure against.
            let recorder = group.clone();
            cell = cell.child(
                canvas(
                    move |bounds, _, cx| store_bounds(cx, &recorder, index, bounds),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            );
            if let Some(child) = child {
                cell = cell.child(child);
            }
            container = container.child(cell.relative());

            let Some(Some(target)) = dividers.get(slot).copied() else {
                continue;
            };
            let target_range = panels[target].range.clone().or(range);
            container = container.child(divider(
                group.clone(),
                index,
                target,
                target_range,
                horizontal,
            ));
        }

        container.into_any_element()
    }

    #[cfg(test)]
    /// Which panel a divider resizes: the one before it when that one
    /// has a size, otherwise the one after.
    fn resized_by(&self, divider: usize) -> Option<usize> {
        if self.panels.get(divider).is_some_and(|p| p.size.is_some()) {
            return Some(divider);
        }
        let after = divider + 1;
        self.panels
            .get(after)
            .is_some_and(|p| p.size.is_some())
            .then_some(after)
    }
}

/// The draggable strip between two panels.
fn divider(
    group: SharedString,
    slot: usize,
    target: usize,
    range: Option<Range<Pixels>>,
    horizontal: bool,
) -> impl IntoElement {
    let id = SharedString::from(format!("{group}-divider-{slot}"));
    let drag_group = group.clone();
    let drag_range = range.clone();

    let mut strip = div().id(id).flex_shrink_0().bg(t::cell_border()).relative();
    strip = if horizontal {
        strip.w(px(DIVIDER)).h_full().cursor_col_resize()
    } else {
        strip.h(px(DIVIDER)).w_full().cursor_row_resize()
    };

    // A 1px line is hard to hit, so a wider invisible grip sits over
    // it, centred on the line.
    let grip = {
        let mut g = div().absolute();
        g = if horizontal {
            g.top_0()
                .bottom_0()
                .left(px(-(GRIP - DIVIDER) / 2.0))
                .w(px(GRIP))
        } else {
            g.left_0()
                .right_0()
                .top(px(-(GRIP - DIVIDER) / 2.0))
                .h(px(GRIP))
        };
        g
    };

    strip
        .child(grip)
        .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _, cx| {
            cx.stop_propagation();
        })
        .on_drag(DragDivider(slot), |drag, _, _, cx| {
            cx.stop_propagation();
            cx.new(|_| drag.clone())
        })
        .on_drag_move(move |event: &gpui::DragMoveEvent<DragDivider>, _, cx| {
            let DragDivider(dragged) = event.drag(cx);
            if *dragged != slot {
                return;
            }
            let Some(bounds) = panel_bounds(cx, &drag_group, target) else {
                return;
            };
            let position = event.event.position;
            // Dragging past a panel measures from its far edge, so the
            // panel after a divider grows as the mouse moves toward it.
            let raw = if target <= slot {
                if horizontal {
                    position.x - bounds.left()
                } else {
                    position.y - bounds.top()
                }
            } else if horizontal {
                bounds.right() - position.x
            } else {
                bounds.bottom() - position.y
            };
            let clamped = match &drag_range {
                Some(range) => raw.max(range.start).min(range.end),
                None => raw.max(px(0.0)),
            };
            store_size(cx, &drag_group, target, clamped);
            cx.refresh_windows();
        })
}

impl gpui::RenderOnce for ResizableGroup {
    fn render(self, _: &mut gpui::Window, cx: &mut App) -> impl IntoElement {
        self.build(cx)
    }
}

#[derive(Clone)]
/// The drag payload: the divider's slot, so drag moves only reach their own divider.
struct DragDivider(usize);

impl gpui::Render for DragDivider {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_divider_moves_the_sized_panel_beside_it() {
        // left is sized, center is not: the divider between them moves
        // left.
        let group = h_resizable("g")
            .child(resizable_panel().size(px(200.0)))
            .child(resizable_panel());
        assert_eq!(group.resized_by(0), Some(0));

        // center is not sized, right is: the divider moves right.
        let group = h_resizable("g")
            .child(resizable_panel())
            .child(resizable_panel().size(px(200.0)));
        assert_eq!(group.resized_by(0), Some(1));

        // neither is sized: nothing to move.
        let group = h_resizable("g")
            .child(resizable_panel())
            .child(resizable_panel());
        assert_eq!(group.resized_by(0), None);
    }
}
