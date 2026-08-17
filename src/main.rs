// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! Runebender GPUI: a font editor built on [GPUI](https://gpui.rs/),
//! started as a point of comparison against
//! [runebender-xilem](https://github.com/eliheuer/runebender-xilem).

mod glyph_path;
mod theme;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gpui::{
    canvas, div, prelude::*, px, size, App, Application, Bounds, Context, MouseButton,
    PathBuilder, Point, SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions,
};
use kurbo::{Affine, BezPath, PathEl};

use theme as t;

// ============================================================================
// FONT MODEL
// ============================================================================

/// One control point of a contour, in font units, with its identity
/// inside the glyph so edits can address it.
#[derive(Clone, Copy)]
struct GlyphPoint {
    x: f64,
    y: f64,
    on_curve: bool,
    contour: usize,
    index: usize,
}

/// One glyph, ready to paint: outline in font units (Y-up), advance
/// width, and identifying info.
struct GlyphEntry {
    name: SharedString,
    codepoint: Option<char>,
    path: Arc<BezPath>,
    points: Arc<Vec<GlyphPoint>>,
    advance: f64,
}

struct FontModel {
    font: norad::Font,
    family_name: SharedString,
    source_path: PathBuf,
    units_per_em: f64,
    ascender: f64,
    descender: f64,
    glyphs: Vec<GlyphEntry>,
    dirty: bool,
}

fn extract_points(glyph: &norad::Glyph) -> Vec<GlyphPoint> {
    glyph
        .contours
        .iter()
        .enumerate()
        .flat_map(|(ci, c)| {
            c.points.iter().enumerate().map(move |(pi, p)| GlyphPoint {
                x: p.x,
                y: p.y,
                on_curve: p.typ != norad::PointType::OffCurve,
                contour: ci,
                index: pi,
            })
        })
        .collect()
}

impl FontModel {
    fn load(path: &std::path::Path) -> Result<Self, norad::error::FontLoadError> {
        let font = norad::Font::load(path)?;
        let info = &font.font_info;
        let units_per_em = info.units_per_em.map(|v| v.as_f64()).unwrap_or(1000.0);
        let ascender = info.ascender.unwrap_or(units_per_em * 0.8);
        let descender = info.descender.unwrap_or(-(units_per_em * 0.2));
        let family_name = info.family_name.clone().unwrap_or_else(|| "Untitled".into());

        let mut glyphs: Vec<GlyphEntry> = font
            .default_layer()
            .iter()
            .map(|glyph| GlyphEntry {
                name: glyph.name().to_string().into(),
                codepoint: glyph.codepoints.iter().next(),
                path: Arc::new(glyph_path::glyph_to_bezpath(glyph, &font)),
                points: Arc::new(extract_points(glyph)),
                advance: glyph.width,
            })
            .collect();
        // Unicode order, unencoded glyphs after, each group by name.
        glyphs.sort_by(|a, b| match (a.codepoint, b.codepoint) {
            (Some(x), Some(y)) => x.cmp(&y).then_with(|| a.name.cmp(&b.name)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.name.cmp(&b.name),
        });

        Ok(Self {
            font,
            family_name: family_name.into(),
            source_path: path.to_path_buf(),
            units_per_em,
            ascender,
            descender,
            glyphs,
            dirty: false,
        })
    }

    /// Move one control point to a new design-space position and
    /// rebuild the glyph's cached outline.
    fn move_point_to(&mut self, glyph_index: usize, contour: usize, index: usize, x: f64, y: f64) {
        let name = self.glyphs[glyph_index].name.to_string();
        let Some(glyph) = self.font.default_layer_mut().get_glyph_mut(name.as_str()) else {
            return;
        };
        let Some(point) = glyph
            .contours
            .get_mut(contour)
            .and_then(|c| c.points.get_mut(index))
        else {
            return;
        };
        point.x = x;
        point.y = y;
        self.dirty = true;
        self.rebuild_entry(glyph_index);
    }

    fn rebuild_entry(&mut self, glyph_index: usize) {
        let name = self.glyphs[glyph_index].name.to_string();
        let Some(glyph) = self.font.get_glyph(name.as_str()) else {
            return;
        };
        let path = Arc::new(glyph_path::glyph_to_bezpath(glyph, &self.font));
        let points = Arc::new(extract_points(glyph));
        let entry = &mut self.glyphs[glyph_index];
        entry.path = path;
        entry.points = points;
    }

    fn save(&mut self) -> Result<(), norad::error::FontWriteError> {
        self.font.save(&self.source_path)?;
        self.dirty = false;
        Ok(())
    }
}

// ============================================================================
// GLYPH PAINTING
// ============================================================================

/// Convert a kurbo path (font units, Y-up) into a gpui path mapped
/// into `bounds` (pixels, Y-down) with the given design→local affine.
fn build_path(
    outline: &BezPath,
    transform: Affine,
    origin: Point<gpui::Pixels>,
    mut builder: PathBuilder,
) -> Option<gpui::Path<gpui::Pixels>> {
    let mut any = false;
    let gp = |p: kurbo::Point| gpui::point(origin.x + px(p.x as f32), origin.y + px(p.y as f32));
    for el in transform * outline {
        match el {
            PathEl::MoveTo(p) => builder.move_to(gp(p)),
            PathEl::LineTo(p) => builder.line_to(gp(p)),
            PathEl::QuadTo(c, p) => builder.curve_to(gp(p), gp(c)),
            PathEl::CurveTo(c1, c2, p) => builder.cubic_bezier_to(gp(p), gp(c1), gp(c2)),
            PathEl::ClosePath => builder.close(),
        }
        any = true;
    }
    if !any {
        return None;
    }
    builder.build().ok()
}

fn build_fill_path(
    outline: &BezPath,
    transform: Affine,
    origin: Point<gpui::Pixels>,
) -> Option<gpui::Path<gpui::Pixels>> {
    build_path(outline, transform, origin, PathBuilder::fill())
}

// ============================================================================
// EDITOR VIEWPORT
// ============================================================================

/// Editor viewport and interaction state. `zoom` is pixels per font
/// unit; `pan` is the local-pixel position of the design origin
/// (glyph left sidebearing at baseline).
struct EditorState {
    zoom: f64,
    pan: (f64, f64),
    initialized: bool,
    selected: Option<(usize, usize)>,
    dragging: bool,
    /// Canvas bounds in window coordinates, written during paint so
    /// mouse handlers can map window→design coordinates.
    bounds: Arc<Mutex<Bounds<gpui::Pixels>>>,
}

impl EditorState {
    fn new() -> Self {
        Self {
            zoom: 1.0,
            pan: (0.0, 0.0),
            initialized: false,
            selected: None,
            dragging: false,
            bounds: Arc::new(Mutex::new(Bounds::default())),
        }
    }

    /// design → local pixels
    fn transform(&self) -> Affine {
        Affine::translate((self.pan.0, self.pan.1))
            * Affine::scale_non_uniform(self.zoom, -self.zoom)
    }

    /// window position → design coordinates
    fn window_to_design(&self, pos: Point<gpui::Pixels>) -> (f64, f64) {
        let origin = self.bounds.lock().unwrap().origin;
        let lx: f32 = (pos.x - origin.x).into();
        let ly: f32 = (pos.y - origin.y).into();
        (
            (lx as f64 - self.pan.0) / self.zoom,
            (self.pan.1 - ly as f64) / self.zoom,
        )
    }

    fn fit(&mut self, advance: f64, ascender: f64, descender: f64) {
        let bounds = *self.bounds.lock().unwrap();
        let w: f32 = bounds.size.width.into();
        let h: f32 = bounds.size.height.into();
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let zoom = (h as f64 * 0.62) / (ascender - descender);
        self.zoom = zoom;
        self.pan = (
            (w as f64 - advance * zoom) / 2.0,
            h as f64 * 0.80 + descender * zoom,
        );
        self.initialized = true;
    }
}

// ============================================================================
// WORKSPACE VIEW
// ============================================================================

enum Mode {
    Grid,
    Editor(usize),
}

struct Workspace {
    font: Option<FontModel>,
    load_error: Option<SharedString>,
    selected: Option<usize>,
    mode: Mode,
    editor: EditorState,
    focus_handle: gpui::FocusHandle,
    status_note: Option<SharedString>,
}

const CELL: f32 = 96.0;
const HIT_RADIUS_PX: f64 = 8.0;

impl Workspace {
    fn open_editor(&mut self, index: usize) {
        self.mode = Mode::Editor(index);
        self.editor.initialized = false;
        self.editor.selected = None;
        self.editor.dragging = false;
    }

    fn glyph_cell(&self, index: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let font = self.font.as_ref().unwrap();
        let entry = &font.glyphs[index];
        let name = entry.name.clone();
        let selected = self.selected == Some(index);
        let outline = entry.path.clone();
        let advance = entry.advance;
        let ascender = font.ascender;
        let descender = font.descender;

        div()
            .id(index)
            .w(px(CELL))
            .h(px(CELL + 20.0))
            .flex()
            .flex_col()
            .bg(if selected { t::cell_selected_bg() } else { t::cell_bg() })
            .border_1()
            .border_color(if selected { t::accent() } else { t::cell_border() })
            .rounded_md()
            .cursor_pointer()
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                this.selected = Some(index);
                if event.click_count() >= 2 {
                    this.open_editor(index);
                }
                cx.notify();
            }))
            .child(
                div().flex_1().child(
                    canvas(
                        move |bounds, _, _| bounds,
                        move |_, bounds: Bounds<gpui::Pixels>, window, _| {
                            let h: f32 = bounds.size.height.into();
                            let w: f32 = bounds.size.width.into();
                            let scale = (h * 0.72) / (ascender - descender) as f32;
                            let baseline_y = h * 0.86 + (descender as f32 * scale);
                            let x_offset = (w - advance as f32 * scale) / 2.0;
                            let transform =
                                Affine::translate((x_offset as f64, baseline_y as f64))
                                    * Affine::scale_non_uniform(scale as f64, -(scale as f64));
                            if let Some(path) = build_fill_path(&outline, transform, bounds.origin)
                            {
                                window.paint_path(path, t::glyph_fill());
                            }
                        },
                    )
                    // A canvas has no intrinsic size; without this it
                    // lays out at 0x0 and paints nothing.
                    .size_full(),
                ),
            )
            .child(
                div()
                    .h(px(20.0))
                    .px_1()
                    .text_size(px(10.0))
                    .text_color(t::text_muted())
                    .overflow_hidden()
                    .child(name),
            )
    }

    /// The glyph editor: metrics lines, stroked outline over a dim
    /// fill, draggable control points, wheel pan, Cmd+wheel zoom.
    fn editor_view(&self, index: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let font = self.font.as_ref().unwrap();
        let entry = &font.glyphs[index];
        let outline = entry.path.clone();
        let points = entry.points.clone();
        let advance = entry.advance;
        let ascender = font.ascender;
        let descender = font.descender;

        let transform = self.editor.transform();
        let zoom = self.editor.zoom;
        let selected_point = self.editor.selected;
        let bounds_slot = self.editor.bounds.clone();
        let needs_fit = !self.editor.initialized;

        div()
            .flex_1()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    this.editor_mouse_down(event.position);
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |this, event: &gpui::MouseMoveEvent, _, cx| {
                if event.pressed_button == Some(MouseButton::Left)
                    && this.editor_mouse_drag(event.position)
                {
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _: &gpui::MouseUpEvent, _, cx| {
                    this.editor.dragging = false;
                    cx.notify();
                }),
            )
            .on_scroll_wheel(cx.listener(move |this, event: &gpui::ScrollWheelEvent, _, cx| {
                this.editor_scroll(event);
                cx.notify();
            }))
            .child(
                canvas(
                    move |bounds, _, _| bounds,
                    move |_, bounds: Bounds<gpui::Pixels>, window, cx| {
                        *bounds_slot.lock().unwrap() = bounds;
                        let mut transform = transform;
                        let mut zoom = zoom;
                        if needs_fit {
                            // First paint after opening: fit the glyph.
                            // Recompute locally; the entity state is
                            // fitted on the next mouse interaction via
                            // the same bounds slot.
                            let h: f32 = bounds.size.height.into();
                            let w: f32 = bounds.size.width.into();
                            let z = (h as f64 * 0.62) / (ascender - descender);
                            let pan = (
                                (w as f64 - advance * z) / 2.0,
                                h as f64 * 0.80 + descender * z,
                            );
                            transform = Affine::translate(pan)
                                * Affine::scale_non_uniform(z, -z);
                            zoom = z;
                        }
                        let _ = cx;
                        let origin = bounds.origin;
                        let to_screen = |x: f64, y: f64| {
                            let p = transform * kurbo::Point::new(x, y);
                            gpui::point(origin.x + px(p.x as f32), origin.y + px(p.y as f32))
                        };

                        // Metrics: baseline, ascender, descender,
                        // sidebearings.
                        let hline = |y: f64, window: &mut Window| {
                            let a = to_screen(0.0, y);
                            let b = to_screen(advance, y);
                            window.paint_quad(gpui::fill(
                                Bounds::from_corners(a, gpui::point(b.x, b.y + px(1.0))),
                                t::metrics_line(),
                            ));
                        };
                        hline(0.0, window);
                        hline(ascender, window);
                        hline(descender, window);
                        for x in [0.0, advance] {
                            let a = to_screen(x, ascender);
                            let b = to_screen(x, descender);
                            window.paint_quad(gpui::fill(
                                Bounds::from_corners(a, gpui::point(a.x + px(1.0), b.y)),
                                t::metrics_line(),
                            ));
                        }

                        if let Some(path) = build_fill_path(&outline, transform, origin) {
                            window.paint_path(path, t::editor_fill());
                        }
                        if let Some(path) =
                            build_path(&outline, transform, origin, PathBuilder::stroke(px(1.5)))
                        {
                            window.paint_path(path, t::accent());
                        }

                        for p in points.iter() {
                            let c = to_screen(p.x, p.y);
                            let is_selected =
                                selected_point == Some((p.contour, p.index));
                            let r = if is_selected {
                                px(4.5)
                            } else if p.on_curve {
                                px(3.0)
                            } else {
                                px(2.0)
                            };
                            let color = if is_selected {
                                t::accent()
                            } else if p.on_curve {
                                t::text()
                            } else {
                                t::text_muted()
                            };
                            window.paint_quad(gpui::fill(
                                Bounds::from_corners(
                                    gpui::point(c.x - r, c.y - r),
                                    gpui::point(c.x + r, c.y + r),
                                ),
                                color,
                            ));
                        }
                        let _ = zoom;
                    },
                )
                .size_full(),
            )
    }

    fn ensure_editor_fit(&mut self) {
        if self.editor.initialized {
            return;
        }
        let Mode::Editor(index) = self.mode else {
            return;
        };
        let Some(font) = self.font.as_ref() else {
            return;
        };
        let entry = &font.glyphs[index];
        let (advance, asc, desc) = (entry.advance, font.ascender, font.descender);
        self.editor.fit(advance, asc, desc);
    }

    fn editor_mouse_down(&mut self, pos: Point<gpui::Pixels>) {
        self.ensure_editor_fit();
        let Mode::Editor(index) = self.mode else {
            return;
        };
        let Some(font) = self.font.as_ref() else {
            return;
        };
        let (dx, dy) = self.editor.window_to_design(pos);
        let tolerance = HIT_RADIUS_PX / self.editor.zoom;
        let hit = font.glyphs[index]
            .points
            .iter()
            .map(|p| {
                let dist = ((p.x - dx).powi(2) + (p.y - dy).powi(2)).sqrt();
                (dist, (p.contour, p.index))
            })
            .filter(|(dist, _)| *dist <= tolerance)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id);
        self.editor.selected = hit;
        self.editor.dragging = hit.is_some();
    }

    fn editor_mouse_drag(&mut self, pos: Point<gpui::Pixels>) -> bool {
        if !self.editor.dragging {
            return false;
        }
        let (Mode::Editor(index), Some((contour, point_index))) =
            (&self.mode, self.editor.selected)
        else {
            return false;
        };
        let index = *index;
        let (dx, dy) = self.editor.window_to_design(pos);
        if let Some(font) = self.font.as_mut() {
            font.move_point_to(index, contour, point_index, dx.round(), dy.round());
            return true;
        }
        false
    }

    fn editor_scroll(&mut self, event: &gpui::ScrollWheelEvent) {
        self.ensure_editor_fit();
        let delta = match event.delta {
            gpui::ScrollDelta::Pixels(p) => {
                let x: f32 = p.x.into();
                let y: f32 = p.y.into();
                (x as f64, y as f64)
            }
            gpui::ScrollDelta::Lines(p) => ((p.x * 24.0) as f64, (p.y * 24.0) as f64),
        };
        if event.modifiers.platform {
            // Cmd+wheel: zoom about the cursor.
            let (dx, dy) = self.editor.window_to_design(event.position);
            let factor = (delta.1 * 0.01).exp();
            self.editor.zoom = (self.editor.zoom * factor).clamp(0.01, 100.0);
            let origin = self.editor.bounds.lock().unwrap().origin;
            let lx: f32 = (event.position.x - origin.x).into();
            let ly: f32 = (event.position.y - origin.y).into();
            self.editor.pan = (
                lx as f64 - dx * self.editor.zoom,
                ly as f64 + dy * self.editor.zoom,
            );
        } else {
            self.editor.pan.0 += delta.0;
            self.editor.pan.1 += delta.1;
        }
    }

    fn header(&self) -> impl IntoElement + use<> {
        let (title, subtitle) = match (&self.font, &self.load_error) {
            (Some(font), _) => (
                font.family_name.clone(),
                SharedString::from(format!(
                    "{} · {} glyphs · {} upm{}",
                    font.source_path.display(),
                    font.glyphs.len(),
                    font.units_per_em,
                    if font.dirty { " · edited" } else { "" }
                )),
            ),
            (None, Some(err)) => ("Load failed".into(), err.clone()),
            (None, None) => ("Runebender GPUI".into(), "No font loaded".into()),
        };
        div()
            .flex()
            .items_baseline()
            .gap_3()
            .px_4()
            .py_2()
            .bg(t::panel_bg())
            .border_b_1()
            .border_color(t::cell_border())
            .child(div().text_lg().text_color(t::text()).child(title))
            .child(div().text_sm().text_color(t::text_muted()).child(subtitle))
    }

    fn status_bar(&self) -> impl IntoElement + use<> {
        let text: SharedString = if let Some(note) = &self.status_note {
            note.clone()
        } else {
            match (&self.mode, self.selected, &self.font) {
                (Mode::Editor(i), _, Some(font)) => {
                    let g = &font.glyphs[*i];
                    let sel = match self.editor.selected {
                        Some((c, p)) => format!(" · point {c}:{p}"),
                        None => String::new(),
                    };
                    format!(
                        "{}{} · wheel pans, Cmd+wheel zooms, drag points, Cmd+S saves, Esc exits",
                        g.name, sel
                    )
                    .into()
                }
                (_, Some(i), Some(font)) => {
                    let g = &font.glyphs[i];
                    match g.codepoint {
                        Some(c) => {
                            format!("{} · U+{:04X} · advance {}", g.name, c as u32, g.advance)
                                .into()
                        }
                        None => {
                            format!("{} · unencoded · advance {}", g.name, g.advance).into()
                        }
                    }
                }
                _ => "Click a glyph; double-click to edit".into(),
            }
        };
        div()
            .px_4()
            .py_1()
            .bg(t::panel_bg())
            .border_t_1()
            .border_color(t::cell_border())
            .text_sm()
            .text_color(t::text_muted())
            .child(text)
    }

    fn handle_key(&mut self, event: &gpui::KeyDownEvent) -> bool {
        let key = event.keystroke.key.as_str();
        let cmd = event.keystroke.modifiers.platform;
        match (key, cmd) {
            ("escape", _) if matches!(self.mode, Mode::Editor(_)) => {
                self.mode = Mode::Grid;
                self.status_note = None;
                true
            }
            ("s", true) => {
                if let Some(font) = self.font.as_mut() {
                    self.status_note = Some(match font.save() {
                        Ok(()) => format!("Saved {}", font.source_path.display()).into(),
                        Err(e) => format!("Save failed: {e}").into(),
                    });
                }
                true
            }
            ("0", _) if matches!(self.mode, Mode::Editor(_)) => {
                self.editor.initialized = false;
                self.ensure_editor_fit();
                true
            }
            _ => false,
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // No text inputs yet, so the workspace can hold focus for
        // keyboard shortcuts unconditionally.
        window.focus(&self.focus_handle);

        let content = match self.mode {
            Mode::Editor(index) if self.font.is_some() => {
                self.editor_view(index, cx).into_any_element()
            }
            _ => {
                let grid: Vec<_> = match &self.font {
                    Some(font) => (0..font.glyphs.len())
                        .map(|i| self.glyph_cell(i, cx).into_any_element())
                        .collect(),
                    None => Vec::new(),
                };
                div()
                    .id("glyph-grid")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(div().flex().flex_wrap().gap_2().p_4().children(grid))
                    .into_any_element()
            }
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(t::window_bg())
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if this.handle_key(event) {
                    cx.notify();
                }
            }))
            .child(self.header())
            .child(content)
            .child(self.status_bar())
    }
}

// ============================================================================
// ENTRY
// ============================================================================

fn default_font_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../runebender-web/assets/test-fonts/VirtuaGrotesk-Regular.ufo")
}

fn main() {
    let font_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(default_font_path);

    let (font, load_error) = match FontModel::load(&font_path) {
        Ok(f) => (Some(f), None),
        Err(e) => (None, Some(format!("{}: {e}", font_path.display()).into())),
    };

    // QA hook: RB_OPEN_GLYPH=<name> starts in the editor on that
    // glyph, so agent screenshots can reach it without clicks.
    let start_mode = std::env::var("RB_OPEN_GLYPH")
        .ok()
        .and_then(|name| {
            let f = font.as_ref()?;
            f.glyphs.iter().position(|g| g.name.as_ref() == name)
        })
        .map(Mode::Editor)
        .unwrap_or(Mode::Grid);

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Runebender".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|cx| Workspace {
                    font,
                    load_error,
                    selected: None,
                    mode: start_mode,
                    editor: EditorState::new(),
                    focus_handle: cx.focus_handle(),
                    status_note: None,
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_point_and_save_roundtrip() {
        let src = default_font_path();
        let tmp = std::env::temp_dir().join("rbg-save-test.ufo");
        if tmp.exists() {
            std::fs::remove_dir_all(&tmp).unwrap();
        }
        let copy_options = fs_extra_copy(&src, &tmp);
        assert!(copy_options, "copying test UFO failed");

        let mut model = FontModel::load(&tmp).expect("load");
        let index = model
            .glyphs
            .iter()
            .position(|g| g.name.as_ref() == "a")
            .expect("glyph a");
        let before = model.glyphs[index].points[0];
        model.move_point_to(index, before.contour, before.index, before.x + 10.0, before.y + 5.0);
        assert!(model.dirty);
        let after = model.glyphs[index].points[0];
        assert_eq!(after.x, before.x + 10.0);
        assert_eq!(after.y, before.y + 5.0);
        model.save().expect("save");
        assert!(!model.dirty);

        let reloaded = FontModel::load(&tmp).expect("reload");
        let entry = reloaded
            .glyphs
            .iter()
            .find(|g| g.name.as_ref() == "a")
            .unwrap();
        let p = entry
            .points
            .iter()
            .find(|p| p.contour == before.contour && p.index == before.index)
            .unwrap();
        assert_eq!(p.x, before.x + 10.0);
        assert_eq!(p.y, before.y + 5.0);
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// Minimal recursive dir copy (a UFO is a directory).
    fn fs_extra_copy(src: &std::path::Path, dst: &std::path::Path) -> bool {
        fn copy_dir(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
            std::fs::create_dir_all(dst)?;
            for entry in std::fs::read_dir(src)? {
                let entry = entry?;
                let target = dst.join(entry.file_name());
                if entry.file_type()?.is_dir() {
                    copy_dir(&entry.path(), &target)?;
                } else {
                    std::fs::copy(entry.path(), &target)?;
                }
            }
            Ok(())
        }
        copy_dir(src, dst).is_ok()
    }
}
