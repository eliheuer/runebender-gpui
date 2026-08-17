// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! Runebender GPUI: a font editor built on [GPUI](https://gpui.rs/),
//! started as a point of comparison against
//! [runebender-xilem](https://github.com/eliheuer/runebender-xilem).

mod glyph_path;
mod theme;

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    canvas, div, prelude::*, px, size, App, Application, Bounds, Context, PathBuilder, Point,
    SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions,
};
use kurbo::{Affine, BezPath, PathEl};

use theme as t;

// ============================================================================
// FONT MODEL
// ============================================================================

/// One control point of a contour, in font units.
struct GlyphPoint {
    x: f64,
    y: f64,
    on_curve: bool,
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
    family_name: SharedString,
    source_path: SharedString,
    units_per_em: f64,
    ascender: f64,
    descender: f64,
    glyphs: Vec<GlyphEntry>,
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
                points: Arc::new(
                    glyph
                        .contours
                        .iter()
                        .flat_map(|c| c.points.iter())
                        .map(|p| GlyphPoint {
                            x: p.x,
                            y: p.y,
                            on_curve: p.typ != norad::PointType::OffCurve,
                        })
                        .collect(),
                ),
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
            family_name: family_name.into(),
            source_path: path.display().to_string().into(),
            units_per_em,
            ascender,
            descender,
            glyphs,
        })
    }
}

// ============================================================================
// GLYPH PAINTING
// ============================================================================

/// Convert a kurbo path (font units, Y-up) into a gpui fill path
/// mapped into `bounds` (pixels, Y-down).
fn build_fill_path(
    outline: &BezPath,
    transform: Affine,
    origin: Point<gpui::Pixels>,
) -> Option<gpui::Path<gpui::Pixels>> {
    build_path(outline, transform, origin, PathBuilder::fill())
}

fn build_path(
    outline: &BezPath,
    transform: Affine,
    origin: Point<gpui::Pixels>,
    mut builder: PathBuilder,
) -> Option<gpui::Path<gpui::Pixels>> {
    let mut any = false;
    let gp = |p: kurbo::Point| {
        gpui::point(origin.x + px(p.x as f32), origin.y + px(p.y as f32))
    };
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

// ============================================================================
// WORKSPACE VIEW
// ============================================================================

enum Mode {
    Grid,
    Editor(usize),
}

struct Workspace {
    font: Option<Arc<FontModel>>,
    load_error: Option<SharedString>,
    selected: Option<usize>,
    mode: Mode,
    focus_handle: gpui::FocusHandle,
}

const CELL: f32 = 96.0;

impl Workspace {
    fn glyph_cell(&self, index: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let font = self.font.as_ref().unwrap().clone();
        let entry = &font.glyphs[index];
        let name = entry.name.clone();
        let selected = self.selected == Some(index);
        let outline = entry.path.clone();
        let advance = entry.advance;
        let upm = font.units_per_em;
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
                    this.mode = Mode::Editor(index);
                }
                cx.notify();
            }))
            .child(div().flex_1().child(canvas(
                move |bounds, _, _| bounds,
                move |_, bounds: Bounds<gpui::Pixels>, window, _| {
                    // Fit ascender..descender into the cell height,
                    // center the advance width horizontally.
                    let h: f32 = bounds.size.height.into();
                    let w: f32 = bounds.size.width.into();
                    let scale = (h * 0.72) / (ascender - descender) as f32;
                    let baseline_y = h * 0.86 + (descender as f32 * scale);
                    let x_offset = (w - advance as f32 * scale) / 2.0;
                    let transform = Affine::translate((x_offset as f64, baseline_y as f64))
                        * Affine::scale_non_uniform(scale as f64, -(scale as f64));
                    let _ = upm;
                    if let Some(path) = build_fill_path(&outline, transform, bounds.origin) {
                        window.paint_path(path, t::glyph_fill());
                    }
                },
            )))
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

    fn header(&self) -> impl IntoElement + use<> {
        let (title, subtitle) = match (&self.font, &self.load_error) {
            (Some(font), _) => (
                font.family_name.clone(),
                SharedString::from(format!(
                    "{} · {} glyphs · {} upm",
                    font.source_path,
                    font.glyphs.len(),
                    font.units_per_em
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

    /// The glyph editor: metrics lines, stroked outline over a dim
    /// fill, and control points. View-only for now (no pan/zoom, no
    /// editing). Esc returns to the grid.
    fn editor_view(&self, index: usize) -> impl IntoElement + use<> {
        let font = self.font.as_ref().unwrap().clone();
        let entry = &font.glyphs[index];
        let outline = entry.path.clone();
        let points = entry.points.clone();
        let advance = entry.advance;
        let ascender = font.ascender;
        let descender = font.descender;

        div().flex_1().child(canvas(
            move |bounds, _, _| bounds,
            move |_, bounds: Bounds<gpui::Pixels>, window, _| {
                let h: f32 = bounds.size.height.into();
                let w: f32 = bounds.size.width.into();
                let scale = (h * 0.62) / (ascender - descender) as f32;
                let baseline_y = h * 0.80 + (descender as f32 * scale);
                let x_offset = (w - advance as f32 * scale) / 2.0;
                let to_screen = |x: f64, y: f64| {
                    gpui::point(
                        bounds.origin.x + px(x_offset + x as f32 * scale),
                        bounds.origin.y + px(baseline_y - y as f32 * scale),
                    )
                };

                // Metrics lines: baseline, ascender, descender, and
                // the two sidebearings.
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

                // Dim fill, then a 1.5px stroke of the same outline.
                let transform = Affine::translate((x_offset as f64, baseline_y as f64))
                    * Affine::scale_non_uniform(scale as f64, -(scale as f64));
                if let Some(path) = build_fill_path(&outline, transform, bounds.origin) {
                    window.paint_path(path, t::editor_fill());
                }
                if let Some(path) =
                    build_path(&outline, transform, bounds.origin, PathBuilder::stroke(px(1.5)))
                {
                    window.paint_path(path, t::accent());
                }

                // Control points: squares for on-curve, smaller
                // squares for off-curve.
                for p in points.iter() {
                    let c = to_screen(p.x, p.y);
                    let r = if p.on_curve { px(3.0) } else { px(2.0) };
                    let color = if p.on_curve { t::text() } else { t::text_muted() };
                    window.paint_quad(gpui::fill(
                        Bounds::from_corners(
                            gpui::point(c.x - r, c.y - r),
                            gpui::point(c.x + r, c.y + r),
                        ),
                        color,
                    ));
                }
            },
        ))
    }

    fn status_bar(&self) -> impl IntoElement + use<> {
        let text: SharedString = match (self.selected, &self.font) {
            (Some(i), Some(font)) => {
                let g = &font.glyphs[i];
                match g.codepoint {
                    Some(c) => format!(
                        "{} · U+{:04X} · advance {}",
                        g.name, c as u32, g.advance
                    )
                    .into(),
                    None => format!("{} · unencoded · advance {}", g.name, g.advance).into(),
                }
            }
            _ => "Click a glyph".into(),
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
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // No text inputs yet, so the workspace can hold focus for
        // keyboard shortcuts unconditionally.
        window.focus(&self.focus_handle);

        let content = match self.mode {
            Mode::Editor(index) if self.font.is_some() => {
                self.editor_view(index).into_any_element()
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
                if event.keystroke.key == "escape" && matches!(this.mode, Mode::Editor(_)) {
                    this.mode = Mode::Grid;
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
        Ok(f) => (Some(Arc::new(f)), None),
        Err(e) => (None, Some(format!("{}: {e}", font_path.display()).into())),
    };

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
                    mode: Mode::Grid,
                    focus_handle: cx.focus_handle(),
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
