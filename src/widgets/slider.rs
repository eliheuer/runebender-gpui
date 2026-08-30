//! A slider, in the shape this editor uses one.
//!
//! Replaces `gpui_component::slider`, whose version carries range
//! sliders, orientation, disabled and release events that nothing
//! here asks for. This one holds a value between a minimum and a
//! maximum, snaps it to a step, and says so when it changes.
//!
//! Dropping the dependency is the point: gpui-component pulls `gpui`
//! from a bare git URL, which is what forces `cargo install --locked`
//! and what turns gpui's `profiler` feature on, and the profiler
//! panics on wasm.

use gpui::{
    AppContext as _, Bounds, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    MouseButton, MouseDownEvent, ParentElement as _, Pixels, Point,
    StatefulInteractiveElement as _, Styled as _, Window, canvas, div, px,
};

/// Emitted while the value is being changed by the user.
pub enum SliderEvent {
    Change(f32),
}

/// A value between `min` and `max`, snapped to `step`.
pub struct SliderState {
    min: f32,
    max: f32,
    step: f32,
    value: f32,
    /// The track's bounds, recorded when it paints. A drag has to turn
    /// a window position into a value, which needs them.
    bounds: Bounds<Pixels>,
}

impl SliderState {
    pub fn new() -> Self {
        Self {
            min: 0.0,
            max: 100.0,
            step: 1.0,
            value: 0.0,
            bounds: Bounds::default(),
        }
    }

    pub fn min(mut self, min: f32) -> Self {
        self.min = min;
        self.clamp_value();
        self
    }

    pub fn max(mut self, max: f32) -> Self {
        self.max = max;
        self.clamp_value();
        self
    }

    pub fn step(mut self, step: f32) -> Self {
        self.step = if step > 0.0 { step } else { 1.0 };
        self
    }

    pub fn default_value(mut self, value: f32) -> Self {
        self.value = value;
        self.clamp_value();
        self
    }


    /// Where the thumb sits, 0.0 at the minimum and 1.0 at the maximum.
    pub fn percentage(&self) -> f32 {
        let span = self.max - self.min;
        if span.abs() < f32::EPSILON {
            return 0.0;
        }
        ((self.value - self.min) / span).clamp(0.0, 1.0)
    }

    /// Set the value from code. Silent: only a drag reports a change,
    /// so a slider following the state it displays cannot feed itself.
    pub fn set_value(&mut self, value: f32, _window: &mut Window, cx: &mut Context<Self>) {
        self.value = value;
        self.clamp_value();
        cx.notify();
    }

    fn clamp_value(&mut self) {
        let (lo, hi) = if self.min <= self.max {
            (self.min, self.max)
        } else {
            (self.max, self.min)
        };
        self.value = self.value.clamp(lo, hi);
    }

    fn set_bounds(&mut self, bounds: Bounds<Pixels>) {
        self.bounds = bounds;
    }

    /// Turn a window position into a value, snap it, and report it.
    fn drag_to(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let width = self.bounds.size.width;
        if width <= px(0.0) {
            return;
        }
        let along = (position.x - self.bounds.left()).clamp(px(0.0), width);
        let fraction = f32::from(along) / f32::from(width);
        let raw = self.min + fraction * (self.max - self.min);
        let snapped = (raw / self.step).round() * self.step;
        let previous = self.value;
        self.value = snapped;
        self.clamp_value();
        if (self.value - previous).abs() > f32::EPSILON {
            cx.emit(SliderEvent::Change(self.value));
        }
        cx.notify();
    }

    #[cfg(test)]
    pub fn value(&self) -> f32 {
        self.value
    }
}

impl Default for SliderState {
    fn default() -> Self {
        Self::new()
    }
}

impl EventEmitter<SliderEvent> for SliderState {}

/// The thing that gets dragged. gpui routes drag moves by the value
/// carried here, so each slider only answers to its own.
#[derive(Clone)]
struct DragSlider(gpui::EntityId);

impl gpui::Render for DragSlider {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

/// Build the track. `fill` draws the bar and the knob, so the caller
/// keeps control of the look.
///
/// Handlers update the state entity directly rather than through
/// `window.listener_for`, so this can be called from a render path
/// that only has an `App`.
pub fn track(
    state: &Entity<SliderState>,
    height: Pixels,
    fill: impl IntoElement,
) -> impl IntoElement {
    let id = state.entity_id();
    let recorder = state.clone();
    let on_down = state.clone();
    let on_move = state.clone();
    div()
        .id("slider-track")
        .relative()
        .w_full()
        .h(height)
        .flex()
        .items_center()
        .child(
            canvas(
                move |bounds, _, cx| {
                    recorder.update(cx, |state, _| state.set_bounds(bounds));
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
        .child(fill)
        .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, _, cx| {
            let position = event.position;
            on_down.update(cx, |state, cx| state.drag_to(position, cx));
        })
        .on_drag(DragSlider(id), |drag, _, _, cx| {
            cx.stop_propagation();
            cx.new(|_| drag.clone())
        })
        .on_drag_move(move |event: &gpui::DragMoveEvent<DragSlider>, _, cx| {
            let DragSlider(dragged) = event.drag(cx);
            if *dragged != id {
                return;
            }
            let position = event.event.position;
            on_move.update(cx, |state, cx| state.drag_to(position, cx));
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_clamps_and_reports_percentage() {
        let state = SliderState::new()
            .min(100.0)
            .max(900.0)
            .default_value(400.0);
        assert_eq!(state.value(), 400.0);
        assert!((state.percentage() - 0.375).abs() < 1e-6);

        // Out of range in either direction lands on the nearer end.
        let low = SliderState::new().min(100.0).max(900.0).default_value(0.0);
        assert_eq!(low.value(), 100.0);
        let high = SliderState::new()
            .min(100.0)
            .max(900.0)
            .default_value(9000.0);
        assert_eq!(high.value(), 900.0);
    }

    #[test]
    fn builder_order_does_not_matter() {
        // gpui-component's slider needed .max() before .min(); this one
        // clamps after each, so either order lands in the same place.
        let a = SliderState::new()
            .max(900.0)
            .min(100.0)
            .default_value(400.0);
        let b = SliderState::new()
            .min(100.0)
            .max(900.0)
            .default_value(400.0);
        assert_eq!(a.value(), b.value());
    }

    #[test]
    fn zero_span_does_not_divide_by_zero() {
        let state = SliderState::new().min(5.0).max(5.0).default_value(5.0);
        assert_eq!(state.percentage(), 0.0);
    }
}
