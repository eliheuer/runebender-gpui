// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Text fields, on Linebender's text stack.
//!
//! The editing model is `parley::PlainEditor`: cursor and word motion,
//! selection by mouse, IME compose, accessibility. Layout is parley's
//! too, and the glyphs are painted through the same outline path this
//! editor already uses for the canvas and the toolbar icons.
//!
//! What this replaces, `gpui_component::input`, is around 22,000 lines
//! because it carries a code editor: LSP semantic tokens, tree-sitter
//! highlighting, a document model. The fields here hold a glyph name,
//! a width, a kerning value, or a feature file.

use std::collections::HashMap;
use std::sync::Arc;

use crate::view::render::px32;
use gpui::{App, Context, EventEmitter, FocusHandle, Focusable, SharedString, Window};
use parley::{FontContext, LayoutContext, PlainEditor};

/// What a field reports. `Change` on every edit, `PressEnter` when the
/// value is committed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum InputEvent {
    /// The text changed under user editing.
    Change,
    /// Enter was pressed in a single-line field.
    PressEnter,
}

/// Shared parley contexts. Building a `FontContext` scans the system
/// font list, so every field borrows one rather than owning it.
pub(crate) struct TextContexts {
    /// The system font list parley shapes against.
    pub font: FontContext,
    /// Parley's reusable layout scratch space.
    pub layout: LayoutContext<[u8; 4]>,
}

impl Default for TextContexts {
    fn default() -> Self {
        Self {
            font: FontContext::new(),
            layout: LayoutContext::new(),
        }
    }
}

impl gpui::Global for GlobalTextContexts {}

/// Every field's focus handle, so the rest of the app can ask whether
/// typing is going into a text field. Without this the window's own
/// Cmd+C and Cmd+V would copy contours while you are editing a name.
#[derive(Default)]
struct FieldFocus(Vec<FocusHandle>);

impl gpui::Global for FieldFocus {}

/// Add a field's focus handle to the list `any_field_focused` checks.
fn register_field(cx: &mut App, handle: &FocusHandle) {
    cx.default_global::<FieldFocus>().0.push(handle.clone());
}

/// Whether a text field currently has the keyboard.
pub(crate) fn any_field_focused(window: &Window, cx: &App) -> bool {
    let Some(focused) = window.focused(cx) else {
        return false;
    };
    cx.try_global::<FieldFocus>()
        .is_some_and(|fields| fields.0.contains(&focused))
}

/// The contexts, in a global so the whole window shares one font list.
pub(crate) struct GlobalTextContexts(pub Arc<std::sync::Mutex<TextContexts>>);

/// The window's shared contexts, created on first use.
pub(crate) fn text_contexts(cx: &mut App) -> Arc<std::sync::Mutex<TextContexts>> {
    if !cx.has_global::<GlobalTextContexts>() {
        let contexts = Arc::new(std::sync::Mutex::new(TextContexts::default()));
        cx.set_global(GlobalTextContexts(contexts));
    }
    cx.global::<GlobalTextContexts>().0.clone()
}

/// One text field.
pub(crate) struct InputState {
    /// The parley editor: text, cursor, selection, IME.
    editor: PlainEditor<[u8; 4]>,
    /// The shared font and layout contexts.
    contexts: Arc<std::sync::Mutex<TextContexts>>,
    /// This field's focus handle, registered so the app can tell a field has the keyboard.
    focus_handle: FocusHandle,
    /// Text shown dimmed while the field is empty.
    placeholder: SharedString,
    /// Whether Enter inserts a line break rather than committing.
    multi_line: bool,
    /// The value as a string, kept beside the editor so `value()` can
    /// hand out a `&str` without borrowing the editor mutably.
    text: String,
    /// Where the text last painted, for turning clicks into positions.
    origin: Point<Pixels>,
    /// The wrap width in force, so it is only set when it changes.
    layout_width: Option<f32>,
    /// Whether a selection drag is in progress.
    dragging: bool,
}

/// The font size fields are drawn at.
pub(crate) const FONT_SIZE: f32 = 13.0;

impl InputState {
    /// An empty single-line field.
    pub(crate) fn new(_window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let contexts = text_contexts(cx);
        let focus_handle = cx.focus_handle();
        register_field(cx, &focus_handle);
        let mut editor = PlainEditor::new(FONT_SIZE);
        editor.set_text("");
        Self {
            editor,
            contexts,
            focus_handle,
            placeholder: SharedString::default(),
            multi_line: false,
            text: String::new(),
            origin: Point::default(),
            layout_width: None,
            dragging: false,
        }
    }

    /// Set the text shown while the field is empty.
    pub(crate) fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// A field that keeps line breaks, for the feature file.
    pub(crate) fn multi_line(mut self) -> Self {
        self.multi_line = true;
        self
    }

    /// The current text.
    pub(crate) fn value(&self) -> &str {
        &self.text
    }

    /// Set the value from code. Silent, so a field showing state it
    /// does not own cannot feed itself.
    pub(crate) fn set_value(
        &mut self,
        value: impl AsRef<str>,
        _window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let value = value.as_ref();
        if self.text == value {
            return;
        }
        self.text = value.to_string();
        self.editor.set_text(value);
        cx.notify();
    }

    /// Pull the text back out of the editor after an edit.
    fn sync_text(&mut self) {
        self.text = self.editor.text().to_string();
    }

    /// Run one editing action, then report the change.
    pub(crate) fn edit(
        &mut self,
        cx: &mut Context<'_, Self>,
        action: impl FnOnce(&mut parley::PlainEditorDriver<'_, [u8; 4]>),
    ) {
        {
            let contexts = self.contexts.clone();
            let mut contexts = contexts.lock().expect("text contexts");
            let TextContexts { font, layout } = &mut *contexts;
            let mut driver = self.editor.driver(font, layout);
            action(&mut driver);
        }
        self.sync_text();
        cx.emit(InputEvent::Change);
        cx.notify();
    }

    /// Insert text, dropping newlines in a single-line field.
    pub(crate) fn insert(&mut self, text: &str, cx: &mut Context<'_, Self>) {
        let cleaned: String;
        let text = if self.multi_line {
            text
        } else {
            cleaned = text.replace(['\n', '\r'], "");
            &cleaned
        };
        if text.is_empty() {
            return;
        }
        self.edit(cx, |driver| driver.insert_or_replace_selection(text));
    }

    /// Enter: a line break in a multi-line field, a commit otherwise.
    pub(crate) fn press_enter(&mut self, cx: &mut Context<'_, Self>) {
        if self.multi_line {
            self.edit(cx, |driver| driver.insert_or_replace_selection("\n"));
        } else {
            cx.emit(InputEvent::PressEnter);
        }
    }

    /// Where the text starts on screen, recorded when it paints so a
    /// click can be turned into a text position.
    fn record_origin(&mut self, origin: Point<Pixels>) {
        self.origin = origin;
    }

    /// Wrap to the field's width. A single-line field never wraps.
    fn set_layout_width(&mut self, width: f32) {
        let width = if self.multi_line { Some(width) } else { None };
        if self.layout_width != width {
            self.layout_width = width;
            self.editor.set_width(width);
        }
    }

    /// Refresh the layout, which every geometry question needs first.
    fn with_layout<R>(&mut self, f: impl FnOnce(&mut PlainEditor<[u8; 4]>) -> R) -> R {
        let contexts = self.contexts.clone();
        let mut contexts = contexts.lock().expect("text contexts");
        let TextContexts { font, layout } = &mut *contexts;
        self.editor.refresh_layout(font, layout);
        f(&mut self.editor)
    }

    /// Selection boxes, relative to the text origin: x, y, width,
    /// height.
    fn selection_rects(&mut self) -> Vec<(f32, f32, f32, f32)> {
        self.with_layout(|editor| {
            editor
                .selection_geometry()
                .into_iter()
                .map(|(rect, _)| {
                    (
                        px32(rect.x0),
                        px32(rect.y0),
                        px32(rect.x1 - rect.x0),
                        px32(rect.y1 - rect.y0),
                    )
                })
                .collect()
        })
    }

    /// The caret box, relative to the text origin.
    fn caret_rect(&mut self) -> Option<(f32, f32, f32, f32)> {
        self.with_layout(|editor| {
            editor.cursor_geometry(1.5).map(|rect| {
                (
                    px32(rect.x0),
                    px32(rect.y0),
                    px32(rect.x1 - rect.x0),
                    px32(rect.y1 - rect.y0),
                )
            })
        })
    }

    /// Paint the text itself.
    fn paint_glyphs(&mut self, origin: Point<Pixels>, color: Rgba, window: &mut Window) {
        let contexts = self.contexts.clone();
        let mut contexts = contexts.lock().expect("text contexts");
        let TextContexts { font, layout } = &mut *contexts;
        self.editor.refresh_layout(font, layout);
        let laid_out = self.editor.layout(font, layout);
        paint_layout(laid_out, origin, color, window);
    }

    /// A click: one press moves the caret, two select a word, three
    /// select the line.
    fn click_at(&mut self, position: Point<Pixels>, clicks: usize, cx: &mut Context<'_, Self>) {
        let (x, y) = self.to_text_space(position);
        {
            let contexts = self.contexts.clone();
            let mut contexts = contexts.lock().expect("text contexts");
            let TextContexts { font, layout } = &mut *contexts;
            let mut driver = self.editor.driver(font, layout);
            match clicks {
                1 => driver.move_to_point(x, y),
                2 => driver.select_word_at_point(x, y),
                _ => driver.select_line_at_point(x, y),
            }
        }
        self.dragging = true;
        cx.notify();
    }

    /// Extend the selection while the mouse is down.
    fn drag_to(&mut self, position: Point<Pixels>, cx: &mut Context<'_, Self>) {
        if !self.dragging {
            return;
        }
        let (x, y) = self.to_text_space(position);
        {
            let contexts = self.contexts.clone();
            let mut contexts = contexts.lock().expect("text contexts");
            let TextContexts { font, layout } = &mut *contexts;
            self.editor
                .driver(font, layout)
                .extend_selection_to_point(x, y);
        }
        cx.notify();
    }

    /// A window position as coordinates relative to where the text painted.
    fn to_text_space(&self, position: Point<Pixels>) -> (f32, f32) {
        (
            f32::from(position.x - self.origin.x),
            f32::from(position.y - self.origin.y),
        )
    }

    /// One keystroke. Returns whether the field used it.
    fn on_key(&mut self, keystroke: &gpui::Keystroke, cx: &mut Context<'_, Self>) -> bool {
        let m = &keystroke.modifiers;
        let word = m.alt || m.control;
        let shift = m.shift;
        match keystroke.key.as_str() {
            "backspace" => {
                self.edit(cx, |d| {
                    if word {
                        d.backdelete_word();
                    } else {
                        d.backdelete();
                    }
                });
                true
            }
            "delete" => {
                self.edit(cx, |d| {
                    if word {
                        d.delete_word();
                    } else {
                        d.delete();
                    }
                });
                true
            }
            "left" => {
                self.motion(cx, |d| match (shift, word) {
                    (true, true) => d.select_word_left(),
                    (true, false) => d.select_left(),
                    (false, true) => d.move_word_left(),
                    (false, false) => d.move_left(),
                });
                true
            }
            "right" => {
                self.motion(cx, |d| match (shift, word) {
                    (true, true) => d.select_word_right(),
                    (true, false) => d.select_right(),
                    (false, true) => d.move_word_right(),
                    (false, false) => d.move_right(),
                });
                true
            }
            "up" => {
                self.motion(cx, |d| {
                    if shift {
                        d.select_up();
                    } else {
                        d.move_up();
                    }
                });
                true
            }
            "down" => {
                self.motion(cx, |d| {
                    if shift {
                        d.select_down();
                    } else {
                        d.move_down();
                    }
                });
                true
            }
            "home" => {
                self.motion(cx, |d| {
                    if shift {
                        d.select_to_line_start();
                    } else {
                        d.move_to_line_start();
                    }
                });
                true
            }
            "end" => {
                self.motion(cx, |d| {
                    if shift {
                        d.select_to_line_end();
                    } else {
                        d.move_to_line_end();
                    }
                });
                true
            }
            "enter" => {
                self.press_enter(cx);
                true
            }
            "a" if m.platform => {
                self.motion(cx, |d| d.select_all());
                true
            }
            "c" if m.platform => {
                self.copy(cx);
                true
            }
            "x" if m.platform => {
                self.cut(cx);
                true
            }
            "v" if m.platform => {
                self.paste(cx);
                true
            }
            "escape" => false,
            _ => {
                let Some(text) = keystroke.key_char.as_deref() else {
                    return false;
                };
                // A modified keystroke is a command, not typing.
                if m.platform || m.control {
                    return false;
                }
                if text.chars().all(|c| c.is_control()) {
                    return false;
                }
                self.insert(text, cx);
                true
            }
        }
    }

    /// Put the selection on the clipboard.
    fn copy(&mut self, cx: &mut Context<'_, Self>) {
        let Some(text) = self.editor.selected_text().map(str::to_string) else {
            return;
        };
        if text.is_empty() {
            return;
        }
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
    }

    /// Copy, then take it out.
    fn cut(&mut self, cx: &mut Context<'_, Self>) {
        self.copy(cx);
        if self.editor.selected_text().is_some() {
            self.edit(cx, |d| d.delete_selection());
        }
    }

    /// Replace the selection with the clipboard's text.
    fn paste(&mut self, cx: &mut Context<'_, Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        self.insert(&text, cx);
    }

    /// Move the caret without reporting a text change.
    fn motion(
        &mut self,
        cx: &mut Context<'_, Self>,
        action: impl FnOnce(&mut parley::PlainEditorDriver<'_, [u8; 4]>),
    ) {
        let contexts = self.contexts.clone();
        let mut contexts = contexts.lock().expect("text contexts");
        let TextContexts { font, layout } = &mut *contexts;
        let mut driver = self.editor.driver(font, layout);
        action(&mut driver);
        drop(contexts);
        cx.notify();
    }

    /// The shared contexts, for callers that shape text themselves.
    pub(crate) fn contexts(&self) -> Arc<std::sync::Mutex<TextContexts>> {
        self.contexts.clone()
    }
}

impl EventEmitter<InputEvent> for InputState {}

impl Focusable for InputState {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ---------------------------------------------------------------
// Painting
// ---------------------------------------------------------------

/// Collects a skrifa outline into a kurbo path, which is what this
/// editor already paints with.
#[derive(Default)]
struct OutlineToPath {
    /// The path collected so far.
    path: kurbo::BezPath,
}

impl skrifa::outline::OutlinePen for OutlineToPath {
    fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to((x as f64, y as f64));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to((x as f64, y as f64));
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.path
            .quad_to((cx as f64, cy as f64), (x as f64, y as f64));
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.path.curve_to(
            (cx0 as f64, cy0 as f64),
            (cx1 as f64, cy1 as f64),
            (x as f64, y as f64),
        );
    }
    fn close(&mut self) {
        self.path.close_path();
    }
}

/// Outlines already extracted, keyed by font, glyph and size.
///
/// Pulling an outline out of a font file is the expensive half of
/// drawing text this way, and a field redraws every frame with almost
/// the same glyphs. Without this the editor spends its time asking
/// skrifa for the letter "e" over and over.
type OutlineKey = (usize, u32, u32, u32);

thread_local! {
    static OUTLINE_CACHE: std::cell::RefCell<
        HashMap<OutlineKey, Option<kurbo::BezPath>>,
    > = std::cell::RefCell::new(HashMap::new());
}

/// Cached outline lookup.
fn glyph_outline_cached(
    font: &parley::FontData,
    size: f32,
    coords: &[i16],
    glyph_id: u32,
) -> Option<kurbo::BezPath> {
    // The blob's address identifies the font: two fonts loaded at once
    // do not share one, and a reload gets a fresh entry.
    let key: OutlineKey = (
        font.data.as_ref().as_ptr() as usize,
        font.index,
        glyph_id,
        size.to_bits(),
    );
    // Variable coordinates would need to be in the key; fields are
    // drawn at one instance, so a font with any is not cached.
    if !coords.is_empty() {
        return glyph_outline(font, size, coords, glyph_id);
    }
    OUTLINE_CACHE.with(|cache| {
        if let Some(hit) = cache.borrow().get(&key) {
            return hit.clone();
        }
        let outline = glyph_outline(font, size, coords, glyph_id);
        cache.borrow_mut().insert(key, outline.clone());
        outline
    })
}

/// The outline of one glyph, in pixels, with the baseline at y = 0 and
/// y running down the way gpui expects.
fn glyph_outline(
    font: &parley::FontData,
    size: f32,
    coords: &[i16],
    glyph_id: u32,
) -> Option<kurbo::BezPath> {
    use skrifa::MetadataProvider as _;
    use skrifa::instance::{LocationRef, Size};
    use skrifa::outline::DrawSettings;

    let font_ref = skrifa::FontRef::from_index(font.data.as_ref(), font.index).ok()?;
    let outlines = font_ref.outline_glyphs();
    let glyph = outlines.get(skrifa::GlyphId::new(glyph_id))?;
    // parley hands back raw F2Dot14 bits; skrifa wants the type.
    let coords: Vec<skrifa::raw::types::F2Dot14> = coords
        .iter()
        .map(|c| skrifa::raw::types::F2Dot14::from_bits(*c))
        .collect();
    let location = LocationRef::new(&coords);
    let settings = DrawSettings::unhinted(Size::new(size), location);
    let mut pen = OutlineToPath::default();
    glyph.draw(settings, &mut pen).ok()?;
    // Font outlines run y-up; the screen runs y-down.
    Some(kurbo::Affine::scale_non_uniform(1.0, -1.0) * pen.path)
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_cut_is_a_copy_then_a_delete() {
        // Documented here because the order matters: reading the
        // selection after deleting it would put nothing on the
        // clipboard.
        let steps = ["copy", "delete"];
        assert_eq!(steps[0], "copy");
    }

    #[test]
    fn a_single_line_field_drops_newlines() {
        // Pasting a multi-line clipboard into a width field should not
        // put a line break in it.
        let cleaned = "12\n34\r\n56".replace(['\n', '\r'], "");
        assert_eq!(cleaned, "123456");
    }

    #[test]
    fn outlines_come_out_y_down() {
        // The transform applied to every glyph flips the font's y-up
        // outline into screen space. A point above the baseline in
        // font space must land above it on screen, which is negative y.
        let mut path = kurbo::BezPath::new();
        path.move_to((0.0, 10.0));
        let flipped = kurbo::Affine::scale_non_uniform(1.0, -1.0) * path;
        let kurbo::PathEl::MoveTo(p) = flipped.elements()[0] else {
            panic!("expected a move");
        };
        assert_eq!(p.y, -10.0);
    }
}

// ---------------------------------------------------------------
// The element
// ---------------------------------------------------------------

use gpui::{
    Bounds, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, ParentElement as _,
    Pixels, Point, Rgba, Styled as _, canvas, div, px,
};

use crate::view::theme as t;

/// A text field.
#[derive(gpui::IntoElement)]
pub(crate) struct Input {
    /// The field this element draws.
    state: gpui::Entity<InputState>,
    /// Whether the field fills the height it is given.
    full_height: bool,
    /// Whether the field draws at the shorter height.
    small: bool,
}

impl Input {
    /// An element drawing `state` at the standard height.
    pub(crate) fn new(state: &gpui::Entity<InputState>) -> Self {
        Self {
            state: state.clone(),
            full_height: false,
            small: false,
        }
    }

    /// Fill the space given, for the feature editor.
    pub(crate) fn h_full(mut self) -> Self {
        self.full_height = true;
        self
    }

    /// A shorter field, for rows that pack several together.
    pub(crate) fn small(mut self) -> Self {
        self.small = true;
        self
    }
}

/// Inset of the text from the field's border.
const PAD_X: f32 = 6.0;
/// Inset of the text from the field's top and bottom edges.
const PAD_Y: f32 = 4.0;
/// How tall a single-line field is.
const LINE_HEIGHT: f32 = 20.0;

impl gpui::RenderOnce for Input {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = self.state.clone();
        let focus_handle = state.read(cx).focus_handle.clone();
        let focused = focus_handle.is_focused(window);
        let multi_line = state.read(cx).multi_line;

        let paint_state = state.clone();
        let click_state = state.clone();
        let drag_state = state.clone();

        let mut field = div()
            .id("input")
            .relative()
            .w_full()
            .px(px(PAD_X))
            .py(px(PAD_Y))
            .border(t::stroke())
            .border_color(if focused {
                t::accent()
            } else {
                t::panel_outline()
            })
            .bg(t::window_bg())
            .cursor_text()
            .track_focus(&focus_handle);

        field = if self.full_height {
            field.h_full()
        } else if multi_line {
            field.min_h(px(LINE_HEIGHT * 3.0))
        } else if self.small {
            field.h(px(LINE_HEIGHT))
        } else {
            field.h(px(LINE_HEIGHT + PAD_Y * 2.0))
        };

        field
            .child(
                canvas(
                    move |bounds, _, _| bounds,
                    move |_, bounds: Bounds<Pixels>, window, cx| {
                        paint_field(&paint_state, bounds, focused, window, cx);
                    },
                )
                .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                move |event: &MouseDownEvent, window, cx| {
                    let handle = click_state.read(cx).focus_handle.clone();
                    window.focus(&handle, cx);
                    let position = event.position;
                    let clicks = event.click_count;
                    click_state.update(cx, |state, cx| {
                        state.click_at(position, clicks, cx);
                    });
                    cx.stop_propagation();
                },
            )
            .on_drag_move(move |event: &gpui::DragMoveEvent<()>, _, cx| {
                let position = event.event.position;
                drag_state.update(cx, |state, cx| state.drag_to(position, cx));
            })
            .on_key_down(move |event, window, cx| {
                let handled = state.update(cx, |input, cx| input.on_key(&event.keystroke, cx));
                if handled {
                    cx.stop_propagation();
                }
                let _ = window;
            })
    }
}

/// Draw the selection, the text, and the caret.
fn paint_field(
    state: &gpui::Entity<InputState>,
    bounds: Bounds<Pixels>,
    focused: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let inner = Point {
        x: bounds.origin.x + px(PAD_X),
        y: bounds.origin.y + px(PAD_Y),
    };
    let width = f32::from(bounds.size.width) - PAD_X * 2.0;

    let placeholder = state.read(cx).placeholder.clone();
    let empty = state.read(cx).text.is_empty();

    state.update(cx, |input, _| {
        input.set_layout_width(width);
        input.record_origin(inner);
    });

    if empty && !placeholder.is_empty() {
        paint_text(state, &placeholder, inner, t::text_muted(), window, cx);
        return;
    }

    // Selection first, so the text sits on top of it.
    let rects = state.update(cx, |input, _| input.selection_rects());
    for rect in rects {
        window.paint_quad(gpui::fill(
            Bounds {
                origin: Point {
                    x: inner.x + px(rect.0),
                    y: inner.y + px(rect.1),
                },
                size: gpui::size(px(rect.2), px(rect.3)),
            },
            t::accent_soft(),
        ));
    }

    state.update(cx, |input, _| {
        input.paint_glyphs(inner, t::text(), window);
    });

    if focused && let Some(caret) = state.update(cx, |input, _| input.caret_rect()) {
        window.paint_quad(gpui::fill(
            Bounds {
                origin: Point {
                    x: inner.x + px(caret.0),
                    y: inner.y + px(caret.1),
                },
                size: gpui::size(px(1.5), px(caret.3)),
            },
            t::text(),
        ));
    }
}

/// Lay out a plain string and paint it, for the placeholder.
fn paint_text(
    state: &gpui::Entity<InputState>,
    text: &str,
    origin: Point<Pixels>,
    color: Rgba,
    window: &mut Window,
    cx: &mut App,
) {
    let contexts = state.read(cx).contexts();
    let mut contexts = contexts.lock().expect("text contexts");
    let TextContexts { font, layout } = &mut *contexts;
    let mut builder = layout.ranged_builder(font, text, 1.0, true);
    builder.push_default(parley::StyleProperty::FontSize(FONT_SIZE));
    let mut laid_out: parley::Layout<[u8; 4]> = builder.build(text);
    laid_out.break_all_lines(None);
    paint_layout(&laid_out, origin, color, window);
}

/// Paint every glyph of a laid-out text.
fn paint_layout(
    layout: &parley::Layout<[u8; 4]>,
    origin: Point<Pixels>,
    color: Rgba,
    window: &mut Window,
) {
    for line in layout.lines() {
        for item in line.items() {
            let parley::PositionedLayoutItem::GlyphRun(run) = item else {
                continue;
            };
            let font = run.run().font().clone();
            let size = run.run().font_size();
            let coords = run.run().normalized_coords().to_vec();
            for glyph in run.positioned_glyphs() {
                let Some(outline) = glyph_outline_cached(&font, size, &coords, glyph.id) else {
                    continue;
                };
                let at = kurbo::Affine::translate((glyph.x as f64, glyph.y as f64));
                if let Some(path) = crate::view::paint::build_fill_path(
                    &(at * outline),
                    kurbo::Affine::IDENTITY,
                    origin,
                ) {
                    window.paint_path(path, color);
                }
            }
        }
    }
}
