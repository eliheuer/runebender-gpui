// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! Runebender GPUI: a font editor built on [GPUI](https://gpui.rs/),
//! started as a point of comparison against
//! [runebender-xilem](https://github.com/eliheuer/runebender-xilem).

mod blur;
mod canvas;
mod chrome;
mod commands;
mod config;
mod editing;
mod grid;
mod host;
mod input;
mod inspector;
mod journal;
mod local_ai;
mod panels;
mod session;
mod sidebar;
mod startup;
#[cfg(test)]
mod tests;
mod text_tool;
mod theme;
#[cfg(target_family = "wasm")]
mod web_host;
mod widgets;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gpui::{
    App, Bounds, Context, MouseButton, PathBuilder, Point, SharedString, TitlebarOptions, Window,
    WindowBounds, WindowOptions, canvas, div, prelude::*, px, size,
};
use kurbo::{Affine, BezPath, PathEl};

use runebender_core::analysis::measure::joining_band;
use runebender_core::analysis::search::{SearchPred, parse_search_predicates};
use runebender_core::document::project::{BraceSource, GlyphPoint, Master, Project};
use runebender_core::formats::color_font::{
    COLOR_LAYERS_EXPLICIT_KEY, has_v1_entry, linear_gradient_paint, paint_glyph_layer, paint_solid,
    parse_hex_color, read_color_mapping, read_color_palette, write_color_mapping,
    write_color_palette,
};
use runebender_core::formats::lib_keys::{
    Annotation, bake_masks, hoi_quad_at, read_annotations, read_hoi_intermediates, read_masks,
    read_production_name, read_saved_filters, write_annotations, write_hoi_intermediates,
    write_masks, write_production_name, write_saved_filters,
};
use runebender_core::formats::metrics_keys::{
    MetricsFormula, parse_metrics_key, read_metrics_key, write_metrics_key,
};
use runebender_core::formats::svg::{glyph_svg, svg_to_contours};
use runebender_core::outline::cleanup::{
    add_extreme_points, correct_path_directions, fit_curve_handles, round_glyph_coordinates,
    tidy_contours, toggle_contour_open,
};
use runebender_core::outline::convert::{cubics_to_quads, quads_to_cubics};
use runebender_core::outline::effects::{
    apply_corner_at, bolden_contours, expand_stroke_contours, extrude_glyph_contours,
    offset_glyph_contours, roughen_glyph_contours,
};
use runebender_core::outline::glyph_ops::{CurveOp, GlyphSnapshot};
use runebender_core::ui::editing::ViewPort;

use startup::keymap;
#[cfg(not(target_family = "wasm"))]
use startup::{open_from_args, print_font_families};
use theme as t;

// App-level commands, reachable from the native menu bar and the
// keymap. GPUI does not populate the macOS menu bar on its own; the
// menus declared in `main` dispatch these.
gpui::actions!(
    runebender,
    [
        OpenFont,
        NewFont,
        SaveFont,
        SaveFontAs,
        ExportFont,
        Undo,
        Redo,
        CopyContours,
        PasteContours,
        CopySelectedGlyphs,
        MeasureColorize,
        MeasureHandles,
        MeasureSegments,
        MeasureSpans,
        MeasureSideBearings,
        MeasureSizes,
        MeasurePopcount,
        MeasureAllOn,
        MeasureAllOff,
        SetThemeDark,
        SetThemeMidnight,
        SetThemeGray,
        SetThemeLight,
        RemoveOverlap,
        Decompose,
        FlipHorizontal,
        FlipVertical,
        RotateLeft,
        RotateRight,
        ReverseContours,
        BooleanUnion,
        BooleanSubtract,
        BooleanIntersect,
        BooleanExclude,
        SetStartPoint,
        DuplicateSelection,
        DuplicateRepeat,
        Rotate180,
        RoundCorners,
        AddExtremes,
        Reinterpolate,
        ExportGlyphSvg,
        TidyPaths,
        CorrectPathDirection,
        RoundCoordinates,
        SelectAllPoints,
        DeselectAllPoints,
        InvertPointSelection,
        NewGlyph,
        DuplicateGlyph,
        RemoveGlyphCmd,
        FilterOffsetCurve,
        FilterExtrude,
        FilterRoughen,
        FilterSlant,
        SyncMetrics,
        ShowAllMasters,
        BakeMasks,
        CheckJoining,
        NextSampleString,
        PreviousSampleString,
        HyperToCubic,
        QuadsToCubics,
        CubicsToQuads,
        TraceImage,
        BoldenWithModel,
        PlaceImage,
        ImportSvg,
        RemoveImage,
        Harmonize,
        Balance,
        Optimize,
        ZoomToFit,
        SortByName,
        SortByUnicode,
        NextMaster,
        PreviousMaster,
        Quit
    ]
);

/// The label a sidebar tab shows on hover, now that the tabs are
/// icons. Placeholder icons for the two that have none of their own.
struct TabTooltip {
    label: &'static str,
}

impl Render for TabTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_1p5()
            .py_0p5()
            .bg(t::panel_bg())
            .border(t::stroke())
            .border_color(t::panel_outline())
            .rounded(t::radius())
            .text_xs()
            .text_color(t::text())
            .child(self.label)
    }
}

/// The interface font, resolved once against what the platform
/// actually has. A name gpui cannot resolve shapes to nothing and no
/// text draws at all, so the preferences are tried in order and the
/// first family the text system reports wins.
fn ui_font_family(cx: &gpui::App) -> gpui::SharedString {
    // Cached: asking the platform for its font list takes about 140ms,
    // and this is read once per frame. Uncached it capped the whole
    // editor at roughly seven frames a second.
    static RESOLVED: std::sync::OnceLock<gpui::SharedString> = std::sync::OnceLock::new();
    if let Some(name) = RESOLVED.get() {
        return name.clone();
    }
    let name = resolve_ui_font_family(cx);
    RESOLVED.set(name.clone()).ok();
    name
}

/// The uncached lookup. Runs once.
fn resolve_ui_font_family(cx: &gpui::App) -> gpui::SharedString {
    // Each platform's own interface font first, then the families that
    // are actually installed on that platform, then a last-resort
    // shared one. Ordered per platform rather than in one list, or a
    // Linux session walks past four macOS families it will never have
    // before reaching anything it does.
    #[cfg(target_os = "macos")]
    const PREFERRED: &[&str] = &[
        ".SystemUIFont",
        "SF Pro Text",
        "SF Pro Display",
        "Helvetica Neue",
        "Helvetica",
        "Arial",
    ];
    #[cfg(target_os = "windows")]
    const PREFERRED: &[&str] = &["Segoe UI Variable Text", "Segoe UI", "Inter", "Arial"];
    // Cantarell ships with GNOME, Noto Sans with most distributions,
    // DejaVu Sans is the long-standing fallback, and Liberation Sans
    // is the metric-compatible stand-in for Arial.
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    const PREFERRED: &[&str] = &[
        "Inter",
        "Cantarell",
        "Noto Sans",
        "DejaVu Sans",
        "Liberation Sans",
        "Arial",
    ];
    let available = cx.text_system().all_font_names();
    // A handful of families means gpui is on its embedded fallback
    // list rather than the platform's fonts, which is what happens if
    // gpui_platform loses the font-kit feature. Text then shapes and
    // paints without ever reaching the screen, so say so here rather
    // than leave a wordless window to be puzzled over.
    static REPORTED: std::sync::Once = std::sync::Once::new();
    if available.len() < 50 {
        REPORTED.call_once(|| {
            eprintln!(
                "warning: only {} font families visible, so text may not \
                 render. Check that gpui_platform still has the font-kit \
                 feature; --fonts lists what it can see.",
                available.len()
            );
        });
    }
    for name in PREFERRED {
        if available.iter().any(|f| f == name) {
            return (*name).into();
        }
    }
    // Nothing preferred is installed: take whatever there is rather
    // than render an empty window.
    available
        .into_iter()
        .next()
        .map(Into::into)
        .unwrap_or_else(|| "Helvetica".into())
}

/// The application menu, used three ways: the native macOS menu bar,
/// the stored menu Windows/Linux expose to `get_menus`, and the
/// in-window menu bar drawn on every platform that has no native bar,
/// the browser included.
/// The action that switches to a theme. `None` means the token file
/// gained a theme that nothing here can reach: a fallback arm would
/// silently hand it Dark's action and the menu item would do the wrong
/// thing, so callers are made to notice instead.
fn theme_action(id: &str) -> Option<Box<dyn gpui::Action>> {
    Some(match id {
        "dark" => Box::new(SetThemeDark),
        "midnight" => Box::new(SetThemeMidnight),
        "gray" => Box::new(SetThemeGray),
        "light" => Box::new(SetThemeLight),
        _ => return None,
    })
}

/// One item per theme, with the active one checked. The menus are
/// rebuilt on a switch so the tick follows.
fn theme_menu_items() -> Vec<gpui::MenuItem> {
    use gpui::MenuItem;
    let current = t::current_theme();
    t::THEMES
        .iter()
        .map(|(id, label)| {
            let action = theme_action(id).expect("every theme has an action");
            MenuItem::Action {
                name: (*label).into(),
                action,
                os_action: None,
                checked: *id == current,
                disabled: false,
            }
        })
        .collect()
}

/// The Measure overlays, as a menu of toggles. They are view options,
/// so they live beside the other view settings rather than taking a
/// panel's worth of sidebar.
fn measure_menu_items() -> Vec<gpui::MenuItem> {
    use gpui::MenuItem;
    let o = *MEASURE_MENU.lock().expect("measure menu");
    let item =
        |name: &'static str, action: Box<dyn gpui::Action>, checked: bool| MenuItem::Action {
            name: name.into(),
            action,
            os_action: None,
            checked,
            disabled: false,
        };
    vec![
        item("Colorize Outline", Box::new(MeasureColorize), o.colorize),
        item("Handle Lengths", Box::new(MeasureHandles), o.handles),
        item("Segment Lengths", Box::new(MeasureSegments), o.segments),
        item("Segment Sizes", Box::new(MeasureSizes), o.sizes),
        item("Stems & Counters", Box::new(MeasureSpans), o.spans),
        item(
            "Side Bearings",
            Box::new(MeasureSideBearings),
            o.sidebearings,
        ),
        MenuItem::separator(),
        // Not a layer: how the labels that are on get written.
        item("Popcount Sums", Box::new(MeasurePopcount), o.popcount),
        MenuItem::separator(),
        MenuItem::action("All On", MeasureAllOn),
        MenuItem::action("All Off", MeasureAllOff),
    ]
}

fn app_menus() -> Vec<gpui::Menu> {
    use gpui::{Menu, MenuItem};
    vec![
        Menu {
            name: "Runebender".into(),
            items: vec![MenuItem::action("Quit Runebender", Quit)],
            disabled: false,
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Font", NewFont),
                MenuItem::action("Open…", OpenFont),
                MenuItem::separator(),
                MenuItem::action("Save", SaveFont),
                MenuItem::action("Save As…", SaveFontAs),
                MenuItem::separator(),
                MenuItem::action("Export…", ExportFont),
            ],
            disabled: false,
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::action("Undo", Undo),
                MenuItem::action("Redo", Redo),
                MenuItem::separator(),
                MenuItem::action("Copy", CopyContours),
                MenuItem::action("Paste", PasteContours),
                MenuItem::action("Copy Selected Glyphs as Text", CopySelectedGlyphs),
                MenuItem::separator(),
                MenuItem::action("Select All", SelectAllPoints),
                MenuItem::action("Deselect All", DeselectAllPoints),
                MenuItem::action("Invert Selection", InvertPointSelection),
            ],
            disabled: false,
        },
        // The Glyph / Path / Filter split mirrors Glyphs 4: Glyph
        // manages the glyph set, Path holds outline commands, Filter
        // the parameterized effects.
        Menu {
            name: "Glyph".into(),
            items: vec![
                MenuItem::action("New Glyph", NewGlyph),
                MenuItem::action("Duplicate Glyph", DuplicateGlyph),
                MenuItem::action("Remove Glyph", RemoveGlyphCmd),
                MenuItem::separator(),
                MenuItem::action("Update Metrics", SyncMetrics),
                MenuItem::action("Reinterpolate", Reinterpolate),
                MenuItem::action("Decompose Components", Decompose),
                MenuItem::separator(),
                MenuItem::action("Check Joining", CheckJoining),
                MenuItem::action("Bake Masks", BakeMasks),
                MenuItem::action("Export Glyph as SVG", ExportGlyphSvg),
                MenuItem::separator(),
                MenuItem::action("Trace Image…", TraceImage),
                MenuItem::action("Bolden With Model…", BoldenWithModel),
                MenuItem::action("Place Image…", PlaceImage),
                MenuItem::action("Import SVG…", ImportSvg),
                MenuItem::action("Remove Image", RemoveImage),
            ],
            disabled: false,
        },
        Menu {
            name: "Path".into(),
            items: vec![
                MenuItem::action("Tidy Up Paths", TidyPaths),
                MenuItem::action("Add Extremes", AddExtremes),
                MenuItem::action("Round Coordinates", RoundCoordinates),
                MenuItem::separator(),
                MenuItem::action("Correct Path Direction", CorrectPathDirection),
                MenuItem::action("Reverse Contours", ReverseContours),
                MenuItem::action("Set Start Point", SetStartPoint),
                MenuItem::separator(),
                MenuItem::action("Remove Overlap", RemoveOverlap),
                MenuItem::action("Union", BooleanUnion),
                MenuItem::action("Subtract", BooleanSubtract),
                MenuItem::action("Intersect", BooleanIntersect),
                MenuItem::action("Exclude", BooleanExclude),
                MenuItem::separator(),
                MenuItem::action("Flip Horizontal", FlipHorizontal),
                MenuItem::action("Flip Vertical", FlipVertical),
                MenuItem::action("Rotate 90° Left", RotateLeft),
                MenuItem::action("Rotate 90° Right", RotateRight),
                MenuItem::action("Rotate 180°", Rotate180),
                MenuItem::action("Duplicate Selection", DuplicateSelection),
                MenuItem::action("Duplicate + Repeat", DuplicateRepeat),
                MenuItem::separator(),
                MenuItem::action("Harmonize", Harmonize),
                MenuItem::action("Balance", Balance),
                MenuItem::action("Optimize", Optimize),
                MenuItem::separator(),
                MenuItem::action("Hyperbezier to Cubic", HyperToCubic),
                MenuItem::action("Quadratic to Cubic", QuadsToCubics),
                MenuItem::action("Cubic to Quadratic", CubicsToQuads),
            ],
            disabled: false,
        },
        // Parameterized filters take their values from the matching
        // grid-side section (Offset, Extrude, Roughen, Slanter).
        Menu {
            name: "Filter".into(),
            items: vec![
                MenuItem::action("Offset Curve", FilterOffsetCurve),
                MenuItem::action("Extrude", FilterExtrude),
                MenuItem::action("Roughen", FilterRoughen),
                MenuItem::action("Round Corners", RoundCorners),
                MenuItem::action("Slanter", FilterSlant),
                MenuItem::separator(),
                MenuItem::action("Add Extremes", AddExtremes),
                MenuItem::action("Remove Overlap", RemoveOverlap),
            ],
            disabled: false,
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Zoom to Fit", ZoomToFit),
                MenuItem::separator(),
                MenuItem::action("Show All Masters", ShowAllMasters),
                MenuItem::separator(),
                MenuItem::action("Sort Glyphs by Name", SortByName),
                MenuItem::action("Sort Glyphs by Unicode", SortByUnicode),
                MenuItem::separator(),
                MenuItem::action("Next Master", NextMaster),
                MenuItem::action("Previous Master", PreviousMaster),
                MenuItem::separator(),
                MenuItem::action("Next Sample String", NextSampleString),
                MenuItem::action("Previous Sample String", PreviousSampleString),
                MenuItem::separator(),
                MenuItem::Submenu(Menu {
                    name: "Measure".into(),
                    items: measure_menu_items(),
                    disabled: false,
                }),
                MenuItem::Submenu(Menu {
                    name: "Theme".into(),
                    items: theme_menu_items(),
                    disabled: false,
                }),
            ],
            disabled: false,
        },
    ]
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

/// A shared toolbar icon painted to fit its element, centered with
/// padding. Icon geometry comes from runebender-core (the same icon
/// UFO the web toolbar uses).
fn icon_svg(name: &'static str, color: gpui::Rgba) -> impl IntoElement {
    canvas(
        move |bounds, _, _| bounds,
        move |_, bounds: Bounds<gpui::Pixels>, window, _| {
            let Some(icon) = runebender_core::ui::theme_oklch::toolbar_icons().get(name) else {
                return;
            };
            let w: f32 = bounds.size.width.into();
            let h: f32 = bounds.size.height.into();
            // Proportional inset: a fixed one shrank the mark inside
            // bigger tiles and crowded it in small ones.
            let pad = (w.min(h) as f64) * 0.12;
            let vb = icon.view_box;
            let scale =
                ((w as f64 - pad * 2.0) / vb.width()).min((h as f64 - pad * 2.0) / vb.height());
            let dx = (w as f64 - vb.width() * scale) / 2.0;
            let dy = (h as f64 - vb.height() * scale) / 2.0;
            // Icon space is Y-down SVG, same as gpui.
            let transform = Affine::translate((dx, dy))
                * Affine::scale(scale)
                * Affine::translate((-vb.x0, -vb.y0));
            if let Some(path) = build_fill_path(&icon.path, transform, bounds.origin) {
                window.paint_path(path, color);
            }
        },
    )
    .size_full()
}

/// Comparable key for a segment (PathSeg has no Eq).
fn seg_key(seg: kurbo::PathSeg) -> [u64; 8] {
    let p = |pt: kurbo::Point| [pt.x.to_bits(), pt.y.to_bits()];
    match seg {
        kurbo::PathSeg::Line(l) => {
            let [a, b] = [p(l.p0), p(l.p1)];
            [a[0], a[1], b[0], b[1], 0, 0, 0, 0]
        }
        kurbo::PathSeg::Quad(q) => {
            let [a, b, c] = [p(q.p0), p(q.p1), p(q.p2)];
            [a[0], a[1], b[0], b[1], c[0], c[1], 0, 1]
        }
        kurbo::PathSeg::Cubic(c) => {
            let [a, b, cc, d] = [p(c.p0), p(c.p1), p(c.p2), p(c.p3)];
            [a[0], a[1], b[0], b[1], cc[0], cc[1], d[0], d[1]]
        }
    }
}

fn build_fill_path(
    outline: &BezPath,
    transform: Affine,
    origin: Point<gpui::Pixels>,
) -> Option<gpui::Path<gpui::Pixels>> {
    build_path(outline, transform, origin, PathBuilder::fill())
}

// ---- joining QA (Arabic connecting-stroke bands) ----

// ---- COLRv1 (paint graphs through the ufo2ft colorLayers key) ----

// ---- masks (subtracting contours, the Glyphs path attribute) ----

// ---- annotations (canvas notes, arrows, circles) ----

// ---- SVG outline import ----

// ---- cubic <-> quadratic conversion ----

// ---- compiled-font import (TTF/OTF via skrifa) ----

/// Font View's three modes (Glyphs 4): grid, detail, list.
#[derive(Clone, Copy, PartialEq)]
enum FontViewMode {
    Grid,
    Detail,
    List,
    /// The positional-forms matrix: Arabic review, isol/init/
    /// medi/fina as columns per base letter.
    Matrix,
}

/// Built-in sample strings (View > Next Sample String): spacing
/// control strings and kern words, cycled around the open glyph.
const SAMPLE_STRINGS: &[&str] = &[
    "HHOHOHOO",
    "nnonoonoo",
    "hamburgefonstiv",
    "HAMBURGEFONSTIV",
    "0123456789",
    "AVATAR Wave Toy Vy",
    "((\"quoted\")) [j] {f}!?",
];

// ---- color fonts (COLRv0 via the ufo2ft lib keys) ----
//
// The build contract is ufo2ft's: `colorPalettes` in the font lib is a
// list of palettes of [r, g, b, a] floats, `colorLayerMapping` is an
// ordered list of [layerName, paletteIndex] pairs (bottom first), and
// at build time same-named glyphs in those UFO layers become the COLR
// layers. Fontra edits the same keys.

// ============================================================================
// EDITOR VIEWPORT
// ============================================================================

/// Which metric field is being edited.
#[derive(Clone, Copy)]
enum MetricField {
    Width,
    Lsb,
    Rsb,
}

/// The active editor tool.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Select,
    Pen,
    Shapes,
    Text,
    Knife,
    Preview,
    HyperPen,
    Measure,
}

/// Pen-tool drawing state: the open contour and the outgoing handle
/// of the previously placed point (set by click-dragging it).
struct PenState {
    contour: usize,
    prev_out_handle: Option<(f64, f64)>,
    /// While the mouse is down on a fresh point: its position, used
    /// to mirror the dragged handle.
    placing: Option<(f64, f64)>,
}

/// An in-progress mouse gesture on the editor canvas.
enum Drag {
    /// Moving the selected points: gesture start in design space and
    /// every point's position when the gesture began. Handles that
    /// travel with a selected on-curve point need their start
    /// positions too, so this covers the whole glyph rather than just
    /// the selection.
    Points {
        start: (f64, f64),
        originals: std::collections::HashMap<(usize, usize), (f64, f64)>,
        /// Selected anchors travel with the points (web moves points
        /// and anchors on one delta): index and start position each.
        anchor: Vec<(usize, (f64, f64))>,
    },
    /// Manual kerning drag in the text buffer (engine session).
    TextKern,
    /// Alt-drag pans the viewport (select tool). Window-space anchor.
    Pan { last: (f64, f64) },
    /// Dragging a sidebearing edge (false = left, true = right).
    Sidebearing {
        right: bool,
        start_x: f64,
        applied: f64,
        start_width: f64,
    },
    /// Free transform from the selection bounding box: a handle
    /// scales about the opposite handle, the ring outside a corner
    /// rotates about the box centre. Shift constrains (proportional
    /// scale, 15-degree rotation steps).
    FreeTransform {
        /// The fixed point of the gesture, in design space.
        anchor: (f64, f64),
        /// Where the gesture began, in design space.
        start: (f64, f64),
        /// Rotation instead of scaling.
        rotate: bool,
        /// Which axes a scale handle drives (edge handles pin one).
        scale_x: bool,
        scale_y: bool,
        /// Every point's position when the gesture began.
        originals: std::collections::HashMap<(usize, usize), (f64, f64)>,
    },
    /// Dragging a node's HOI intermediate knob: the point id and
    /// the node's positions in the axis-end masters.
    HoiKnob {
        id: (usize, usize),
        a: (f64, f64),
        b: (f64, f64),
    },
    /// Dragging a guide: `local` picks the open glyph's guidelines
    /// over the master's fontinfo ones. Guides move live; the
    /// master is marked dirty as they move.
    Guide { local: bool, index: usize },
    /// Dragging the selected component.
    Component {
        index: usize,
        start: (f64, f64),
        orig: (f64, f64),
    },
    /// Knife line, in design space.
    Knife {
        start: (f64, f64),
        current: (f64, f64),
    },
    /// Rubber-band selection rectangle, in design space. `base` is
    /// what was selected when the drag began: the live selection is
    /// always that plus whatever the box now encloses.
    Marquee {
        start: (f64, f64),
        current: (f64, f64),
        base: std::collections::HashSet<(usize, usize)>,
        base_anchors: Vec<usize>,
    },
    /// Dragging out a rectangle/ellipse (shapes tool).
    Shape {
        start: (f64, f64),
        current: (f64, f64),
    },
    /// Measuring (measure tool).
    Measure {
        start: (f64, f64),
        current: (f64, f64),
    },
}

/// Editor viewport and interaction state, on the shared
/// `runebender_core` viewport (design Y-up ↔ screen Y-down).
/// A right-click context menu over the editor canvas (web
/// contourContextMenu).
struct ContextMenu {
    /// Position inside the canvas, in canvas-local pixels.
    at: Point<gpui::Pixels>,
    design: (f64, f64),
    contour: Option<usize>,
    contour_count: usize,
    start_point: Option<(usize, usize)>,
    anchor: Option<usize>,
    component: Option<(usize, bool)>,
    has_components: bool,
    adding_component: bool,
    /// Inline corner-name input mode (Apply Corner…).
    applying_corner: bool,
    /// Inline note-text input mode (Annotate: Note…).
    adding_note: bool,
    /// Annotation under the click.
    annotation: Option<usize>,
    /// Guide under the click: (local, index).
    guide: Option<(bool, usize)>,
}

struct EditorState {
    /// The active text-buffer sort's layout position (design units);
    /// (0,0) when the glyph is alone in the editor.
    sort_offset: (f64, f64),
    /// The tool to return to when space-hold preview ends.
    previous_tool: Tool,
    /// The hyper pen's open contour, if drawing.
    hyper_contour: Option<usize>,
    /// Alt-hover segment preview (select tool), in glyph space.
    segment_hover: Option<kurbo::PathSeg>,
    /// The last flip/rotate, re-applied by duplicate-repeat.
    last_transform: Option<Affine>,
    /// The selected component of the open glyph, if any.
    selected_component: Option<usize>,
    /// Sidebearing edge under the cursor (false = left, true = right).
    sidebearing_hover: Option<bool>,
    /// Guide under the cursor: (local, index).
    guide_hover: Option<(bool, usize)>,
    /// Locked nodes (session-scoped): unselectable and undraggable
    /// until unlocked — Glyphs' node locking.
    locked_points: std::collections::HashSet<(usize, usize)>,
    /// Mouse position in window coords, for pen previews.
    pointer: Option<Point<gpui::Pixels>>,
    viewport: ViewPort,
    initialized: bool,
    tool: Tool,
    pen: Option<PenState>,
    /// Shapes tool draws ellipses instead of rectangles.
    shape_ellipse: bool,
    selected: std::collections::HashSet<(usize, usize)>,
    /// Selected anchors, in the order they were picked. A selection
    /// may hold points and anchors at once (web keeps both in one
    /// selection); the last one is the "primary" the panels read.
    selected_anchors: Vec<usize>,
    /// Last cursor position in design space (for A = add anchor).
    cursor: (f64, f64),
    drag: Option<Drag>,
    /// Undo/redo stacks of glyph snapshots for the open glyph.
    undo: Vec<GlyphSnapshot>,
    redo: Vec<GlyphSnapshot>,
    /// Canvas bounds in window coordinates, written during paint so
    /// mouse handlers can map window→design coordinates.
    bounds: Arc<Mutex<Bounds<gpui::Pixels>>>,
}

impl EditorState {
    /// The anchor the side panels edit: the last one picked.
    fn selected_anchor(&self) -> Option<usize> {
        self.selected_anchors.last().copied()
    }

    fn new() -> Self {
        Self {
            sort_offset: (0.0, 0.0),
            previous_tool: Tool::Select,
            hyper_contour: None,
            segment_hover: None,
            last_transform: None,
            selected_component: None,
            sidebearing_hover: None,
            guide_hover: None,
            locked_points: std::collections::HashSet::new(),
            pointer: None,
            viewport: ViewPort::new(),
            initialized: false,
            tool: Tool::Select,
            pen: None,
            shape_ellipse: false,
            selected: std::collections::HashSet::new(),
            selected_anchors: Vec::new(),
            cursor: (0.0, 0.0),
            drag: None,
            undo: Vec::new(),
            redo: Vec::new(),
            bounds: Arc::new(Mutex::new(Bounds::default())),
        }
    }

    /// design → local pixels, in the active sort's glyph space.
    /// When the text tool has other sorts in the buffer, the open
    /// glyph sits at its layout position; the offset keeps every
    /// tool (points, pen, shapes, marquee) working in glyph-local
    /// coordinates.
    fn transform(&self) -> Affine {
        self.viewport.affine() * Affine::translate(self.sort_offset)
    }

    fn zoom(&self) -> f64 {
        self.viewport.zoom
    }

    /// window position → local canvas pixels
    fn window_to_local(&self, pos: Point<gpui::Pixels>) -> kurbo::Point {
        let origin = self.bounds.lock().unwrap().origin;
        let lx: f32 = (pos.x - origin.x).into();
        let ly: f32 = (pos.y - origin.y).into();
        kurbo::Point::new(lx as f64, ly as f64)
    }

    /// window position → design coordinates
    fn window_to_design(&self, pos: Point<gpui::Pixels>) -> (f64, f64) {
        let p = self.viewport.screen_to_design(self.window_to_local(pos));
        (p.x - self.sort_offset.0, p.y - self.sort_offset.1)
    }

    fn fit(&mut self, advance: f64, ascender: f64, descender: f64) {
        let bounds = *self.bounds.lock().unwrap();
        let w: f32 = bounds.size.width.into();
        let h: f32 = bounds.size.height.into();
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        self.viewport
            .fit_to_canvas(w as f64, h as f64, advance, ascender, descender, 0.6);
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

/// The category rows, in web order. Labels double as the keys for
/// core's category_subfilters.
const SIDEBAR_CATEGORIES: [(runebender_core::analysis::category::GlyphCategory, &str); 8] = {
    use runebender_core::analysis::category::GlyphCategory as GC;
    [
        (GC::All, "All"),
        (GC::Letter, "Letter"),
        (GC::Number, "Number"),
        (GC::Punctuation, "Punctuation"),
        (GC::Symbol, "Symbol"),
        (GC::Mark, "Mark"),
        (GC::Separator, "Separator"),
        (GC::Other, "Other"),
    ]
};

/// What the sidebar has selected (web GlyphSidebarFilter).
#[derive(Clone, PartialEq, Eq)]
enum SidebarFilter {
    All,
    Category(runebender_core::analysis::category::GlyphCategory),
    Subfilter(
        runebender_core::analysis::category::GlyphCategory,
        &'static str,
    ),
    LanguageGroup(usize),
    Language(usize, usize),
    Builtin(usize),
    /// A user-saved search (index into the font's saved-filter list).
    Saved(usize),
}

/// Glyph counts for every sidebar row, computed once per font state.
struct SidebarCounts {
    #[allow(dead_code)]
    total: usize,
    categories: Vec<usize>,
    subfilters: std::collections::HashMap<(usize, usize), usize>,
    groups: Vec<usize>,
    languages: Vec<Vec<usize>>,
    /// Missing-target counts per (group, filter); 0 = complete or
    /// not target-bearing.
    missing: Vec<Vec<usize>>,
    builtins: Vec<usize>,
    saved: Vec<usize>,
}

/// One edit tab: the open glyph (by name, so it survives renames
/// and master switches), plus the parked editor state and text
/// buffer. The ACTIVE tab's live state lives in `Workspace::editor`
/// and `edit_buffer`; its slot here is stale until the next switch
/// parks it back.
struct EditSession {
    glyph_name: String,
    editor: EditorState,
    buffer: runebender_core::text::buffer::TextBuffer,
}

/// Every glyph carrying one anchor name: marks (name, x, y), bases,
/// and mark carriers.
type AnchorFamily = (
    Vec<(String, f64, f64)>,
    Vec<(String, f64, f64)>,
    Vec<(String, f64, f64)>,
);

/// A blurred preview frame with the key it was rendered for.
type BlurFrame = (u64, Arc<gpui::RenderImage>);

struct Workspace {
    project: Option<Project>,
    load_error: Option<SharedString>,
    selected: Option<usize>,
    /// The glyph whose edit session the tab strip returns to after
    /// the Font tab switched back to the overview.
    last_editor: Option<usize>,
    /// Edit tabs, Glyphs-style. Empty until a glyph is first opened.
    sessions: Vec<EditSession>,
    active_session: usize,
    sidebar_filter: SidebarFilter,
    /// Names matched by the current sidebar filter (None = all).
    sidebar_matches: Option<std::collections::HashSet<String>>,
    /// Per-row glyph counts, rebuilt on load/reload/master switch.
    sidebar_counts: Option<SidebarCounts>,
    expanded_scripts: std::collections::HashSet<usize>,
    expanded_categories: std::collections::HashSet<usize>,
    /// Grid sort: false = by name, true = by unicode (web default).
    sort_unicode: bool,
    /// A run of arrow-key nudges is in progress: they share one undo
    /// step until something else happens.
    nudging: bool,
    /// Text preview strip under the editor: whether it is showing, its
    /// type size in pixels, how far it is blurred (a spacing check),
    /// whether the colors are flipped, and how the line is aligned.
    preview_visible: bool,
    preview_blur: f32,
    /// The last blurred frame, kept so dragging a point does not
    /// re-rasterize the preview on every mouse move.
    preview_blur_cache: Arc<Mutex<Option<BlurFrame>>>,
    /// Decoded glyph background images from the UFO images store,
    /// keyed by file name; None caches a failed decode. Behind a
    /// mutex because rendering (which fills it) holds &self.
    glyph_image_cache:
        Arc<Mutex<std::collections::HashMap<String, Option<Arc<gpui::RenderImage>>>>>,
    preview_invert: bool,
    preview_blur_slider: Option<gpui::Entity<widgets::slider::SliderState>>,
    /// Grid cell size in px, driven by the bottom bar's zoom slider.
    /// This is the *target*: cells stretch from it to fill the row.
    grid_cell_size: f32,
    /// Measured size of the glyph grid's scroll viewport. Columns and
    /// row height are solved against it so rows fill the width and
    /// divide the height evenly (no half row at the bottom edge).
    grid_viewport: gpui::Size<gpui::Pixels>,
    /// The same, for the editor sidebar's mini glyph grid.
    sidebar_viewport: gpui::Size<gpui::Pixels>,
    /// The glyphs the filters and the search leave, in display order.
    /// Rebuilt when the inputs change rather than on every frame: it
    /// filters and sorts the whole font, which is far too much work to
    /// repeat for a mouse move.
    glyph_order: Option<Arc<Vec<usize>>>,
    /// What `glyph_order` was built from.
    order_key: Option<OrderKey>,
    /// The search pattern, compiled once instead of per glyph.
    search_re: Option<regex::Regex>,
    /// First visible row of each grid. Scrolling moves whole rows.
    grid_scroll_row: usize,
    sidebar_scroll_row: usize,
    /// Which editor-sidebar tab is up: 0 glyphs, 1 shapes, 2 axes,
    /// 3 chat.
    sidebar_tab: u8,
    /// Target cell size for the editor sidebar's mini grid.
    sidebar_cell_size: f32,
    sidebar_slider: Option<gpui::Entity<widgets::slider::SliderState>>,
    cell_slider: Option<gpui::Entity<widgets::slider::SliderState>>,
    mode: Mode,
    editor: EditorState,
    /// The editor's text buffer (the text tool): the open glyph is
    /// the active sort; other sorts render as filled context around
    /// it, exactly the web and xilem model.
    edit_buffer: runebender_core::text::buffer::TextBuffer,
    /// Keys route to the preview buffer (click the strip to focus,
    /// Escape to leave).
    /// Folded sidebar sections (by title).
    collapsed_sections: std::collections::HashSet<&'static str>,
    /// Masters drawn as dim reference underlays in the editor
    /// (the layer rows' eye toggles).
    reference_layers: std::collections::HashSet<usize>,
    /// Edit > Show All Masters: every master overlaid in the edit
    /// view, any master's node clickable (the click switches to that
    /// master with the node selected).
    show_all_masters: bool,
    /// Left sidebar hidden (header toggle, like the Glyphs one).
    left_collapsed: bool,
    /// In-window menu bar for platforms without a native one
    /// (Windows, Linux, the browser).
    #[cfg(not(target_os = "macos"))]
    app_menu_bar: gpui::Entity<widgets::menu_bar::MenuBar>,
    focus_handle: gpui::FocusHandle,
    /// Scales what a model predicts. A model can be right about which
    /// way a point moves and short on how far, which looks like a
    /// prediction that is too light.
    model_strength: f64,
    /// The chosen model directory, kept so a run does not re-ask.
    model_dir: Option<PathBuf>,
    /// What the directory says it is, for the panel.
    model_summary: Option<SharedString>,
    /// Loaded weights. Cached: reading them is the slow part.
    model_loaded: Option<std::rc::Rc<font_ml::outline::OutlineModel>>,
    /// Last judgement: glyph, model error, baseline error.
    model_score: Option<(SharedString, f64, f64)>,
    model_strength_slider: Option<gpui::Entity<widgets::slider::SliderState>>,
    status_note: Option<SharedString>,
    search: gpui::Entity<widgets::input::InputState>,
    search_query: String,
    /// Search scope: 0 = all, 1 = name, 2 = unicode.
    search_mode: u8,
    /// Wall-clock time of the last save, for the header.
    last_save_label: Option<SharedString>,
    /// Multi-selected glyph names (grid cmd/shift-click); `selected`
    /// stays the primary.
    multi_selected: std::collections::HashSet<String>,
    search_regex: bool,
    search_case: bool,
    metric_inputs: MetricInputs,
    font_info_inputs: FontInfoInputs,
    kern_inputs: KernInputs,
    /// Slant angle field in the Transformations section (degrees).
    slant_input: gpui::Entity<widgets::input::InputState>,
    /// Stroke width field in the Transformations section (units).
    stroke_input: gpui::Entity<widgets::input::InputState>,
    /// Offset field: bolder (positive) or lighter (negative) units.
    offset_input: gpui::Entity<widgets::input::InputState>,
    /// Fit Curve percentage field in the Curves section.
    fit_input: gpui::Entity<widgets::input::InputState>,
    /// Hex field that appends a color to the CPAL palette.
    color_hex_input: gpui::Entity<widgets::input::InputState>,
    /// Palette index the next color layer is assigned.
    color_selected: usize,
    /// Paint the color layers stacked in the editor.
    show_color_preview: bool,
    /// Which built-in sample string the buffer shows.
    sample_index: usize,
    /// Font view mode: the classic grid, the Glyphs 4 detail grid
    /// (info beside every glyph), or the property-table list.
    font_view_mode: FontViewMode,
    /// Draw node trajectories + velocity dots across the first axis
    /// (higher-order interpolation view).
    show_trajectories: bool,
    /// The intermediate point being dragged right now (id, Q),
    /// painted live and committed + baked on mouse-up.
    hoi_live: Option<((usize, usize), (f64, f64))>,
    /// The shaping inspector's focused cluster (carrier sort index).
    shaping_focus: Option<usize>,
    /// Ghost every attachable mark on the open glyph's anchors
    /// (Glyphs' mark cloud).
    show_mark_cloud: bool,
    /// Preview feature overrides: tag → forced on/off. Absent tags
    /// keep the shaper's defaults.
    feature_overrides: std::collections::HashMap<String, bool>,
    /// Preview shaping locale: (script tag, BCP 47 language), e.g.
    /// ("arab", "ur"). None = direction-derived defaults.
    shaping_locale: Option<(String, String)>,
    /// Ease amount field: Enter bakes interpolation timing into a
    /// brace layer at the preview location.
    ease_input: gpui::Entity<widgets::input::InputState>,
    /// Extrude field ("offset,angle"; k-prefix keeps the front).
    extrude_input: gpui::Entity<widgets::input::InputState>,
    /// Roughen field ("segment,h,v"); reseeded per apply.
    roughen_input: gpui::Entity<widgets::input::InputState>,
    roughen_seed: u64,
    /// The Instances editor field under the axis sliders: Enter
    /// renames the instance at the preview location, or adds one.
    instance_name_input: gpui::Entity<widgets::input::InputState>,
    /// The Features section's features.fea editor (grid mode).
    features_input: gpui::Entity<widgets::input::InputState>,
    /// Unapplied edits in the features editor: the refresh keeps its
    /// hands off until Apply or Revert.
    features_edited: bool,
    /// The last Apply's compile verdict, shown under the editor.
    features_status: Option<SharedString>,
    glyph_inputs: GlyphInputs,
    context_menu: Option<ContextMenu>,
    /// The Selection panel's 9-point reference for numeric move and
    /// scale (web coordinate quadrant).
    coord_quadrant: runebender_core::outline::path::Quadrant,
    /// Curve overlays (web CurvePanel).
    curve_comb: bool,
    curve_continuity: bool,
    /// Measure-tool HUD layers (web SelectPanel / MeasureOptions).
    measure_opts: MeasureOpts,
    /// Show the UFO background layer as a quiet outline.
    show_background: bool,
    /// Per-glyph UFO layers drawn as underlays (layer names with the
    /// eye on), beyond the default and background layers.
    visible_glyph_layers: std::collections::HashSet<String>,
    /// Another glyph ghosted behind the drawing for comparison.
    reference_glyph: Option<String>,
    reference_glyph_input: gpui::Entity<widgets::input::InputState>,
    component_name_input: gpui::Entity<widgets::input::InputState>,
    /// Corner-glyph name typed in the context menu (Apply Corner…).
    corner_name_input: gpui::Entity<widgets::input::InputState>,
    /// Note text typed in the context menu (Annotate: Note…).
    annotation_input: gpui::Entity<widgets::input::InputState>,
    /// Smart-axis definition on the open part glyph ("Width,0,100").
    smart_axis_input: gpui::Entity<widgets::input::InputState>,
    /// New kerning group from the grid selection: "o" (kern1) or
    /// "|o" (kern2).
    group_name_input: gpui::Entity<widgets::input::InputState>,
    /// New avar pair on the first axis: "user,design".
    axis_map_input: gpui::Entity<widgets::input::InputState>,
    /// Parsed predicate query, rebuilt when the search changes.
    search_predicates: Option<Vec<SearchPred>>,
    /// The selected smart component's value on its first axis.
    smart_value_input: gpui::Entity<widgets::input::InputState>,
    anchor_name_input: gpui::Entity<widgets::input::InputState>,
    /// Sliders for non-degenerate designspace axes: (axis index,
    /// slider), created lazily in render.
    axis_sliders: Vec<(usize, gpui::Entity<widgets::slider::SliderState>)>,
    /// Internal outline clipboard: whole contours.
    clipboard: Vec<norad::Contour>,
    /// Web host connection (server base + file ETags), when the page
    /// was opened with ?server=.
    #[cfg(target_family = "wasm")]
    web_host: Option<web_host::WebHost>,
    /// Filesystem watcher over the open masters' UFO directories.
    _watcher: Option<notify::RecommendedWatcher>,
    /// Set at save time so the watcher ignores our own writes.
    last_save: Arc<Mutex<web_time::Instant>>,
    /// A selected kern pair in the preview strip: indices into the
    /// resolved preview line (glyph indices of the pair).
    _subscriptions: Vec<gpui::Subscription>,
}

/// The editor's Width / LSB / RSB / X / Y fields.
/// Which measurement-HUD layers the Measure tool draws (web
/// MeasureOptions). Every layer off returns the plain editor; the
/// panel is purely additive.
#[derive(Clone, Copy)]
struct MeasureOpts {
    /// Tint outline segments, curves, and handles by popcount.
    colorize: bool,
    /// Label Bézier handle lengths.
    handles: bool,
    /// Label straight outline segment lengths.
    segments: bool,
    /// Draw + label stem/counter/height spans (dimension lines).
    spans: bool,
    /// Draw + label left/right side bearings.
    sidebearings: bool,
    /// Label every curve segment with the size of its own bounding
    /// box, so a glyph's curves can be compared at a glance.
    sizes: bool,
    /// Spell lengths as sums of powers of two (`96 = 64+32`).
    popcount: bool,
}

impl Default for MeasureOpts {
    fn default() -> Self {
        Self {
            colorize: false,
            handles: false,
            segments: false,
            spans: false,
            sidebearings: false,
            sizes: false,
            popcount: true,
        }
    }
}

/// What the Measure menu shows as ticked. The menu is built outside
/// the view, so the live options are mirrored here whenever they
/// change.
static MEASURE_MENU: std::sync::Mutex<MeasureOpts> = std::sync::Mutex::new(MeasureOpts {
    colorize: false,
    handles: false,
    segments: false,
    spans: false,
    sidebearings: false,
    sizes: false,
    popcount: true,
});

impl MeasureOpts {
    fn any(&self) -> bool {
        self.colorize
            || self.handles
            || self.segments
            || self.spans
            || self.sidebearings
            || self.sizes
    }

    fn label(&self, value: i64) -> String {
        if self.popcount {
            runebender_core::analysis::measure::label(value)
        } else {
            value.to_string()
        }
    }
}

/// Editable glyph-data fields in the Glyph panel.
struct GlyphInputs {
    name: gpui::Entity<widgets::input::InputState>,
    unicode: gpui::Entity<widgets::input::InputState>,
    group_l: gpui::Entity<widgets::input::InputState>,
    group_r: gpui::Entity<widgets::input::InputState>,
    /// Free-text glyph note (UFO glif note element), like Glyphs'
    /// note field; shows as a tooltip in its font view.
    note: gpui::Entity<widgets::input::InputState>,
    /// Shape-switch point: Enter creates the .bold alternate and the
    /// designspace rule at this axis value (bracket layer).
    switch_at: gpui::Entity<widgets::input::InputState>,
    /// Metrics keys ("=n", "=|o", "=n+10"): linked sidebearings,
    /// synced across every master.
    lsb_key: gpui::Entity<widgets::input::InputState>,
    rsb_key: gpui::Entity<widgets::input::InputState>,
    /// Export (production) name, written to public.postscriptNames
    /// in every master's lib; ufo2ft renames on compile.
    production: gpui::Entity<widgets::input::InputState>,
}

struct MetricInputs {
    width: gpui::Entity<widgets::input::InputState>,
    lsb: gpui::Entity<widgets::input::InputState>,
    rsb: gpui::Entity<widgets::input::InputState>,
    /// Selection reference coordinates and size (Selection section).
    x: gpui::Entity<widgets::input::InputState>,
    y: gpui::Entity<widgets::input::InputState>,
    w: gpui::Entity<widgets::input::InputState>,
    h: gpui::Entity<widgets::input::InputState>,
}

/// Editable fields in the Font Info section (grid mode). Each commits
/// on Enter and writes fontinfo.plist through the normal save path.
struct FontInfoInputs {
    family: gpui::Entity<widgets::input::InputState>,
    style: gpui::Entity<widgets::input::InputState>,
    upm: gpui::Entity<widgets::input::InputState>,
    italic_angle: gpui::Entity<widgets::input::InputState>,
    ascender: gpui::Entity<widgets::input::InputState>,
    descender: gpui::Entity<widgets::input::InputState>,
    x_height: gpui::Entity<widgets::input::InputState>,
    cap_height: gpui::Entity<widgets::input::InputState>,
    /// PostScript hinting data per master: alignment zones (blue
    /// values in pairs) and standard stems, comma-separated lists.
    blue_values: gpui::Entity<widgets::input::InputState>,
    other_blues: gpui::Entity<widgets::input::InputState>,
    stems_h: gpui::Entity<widgets::input::InputState>,
    stems_v: gpui::Entity<widgets::input::InputState>,
    /// The OS/2 and hhea vertical metrics (typo/hhea/win), the
    /// parameters the Google Fonts vertical-metrics checks read.
    typo_asc: gpui::Entity<widgets::input::InputState>,
    typo_desc: gpui::Entity<widgets::input::InputState>,
    typo_gap: gpui::Entity<widgets::input::InputState>,
    hhea_asc: gpui::Entity<widgets::input::InputState>,
    hhea_desc: gpui::Entity<widgets::input::InputState>,
    hhea_gap: gpui::Entity<widgets::input::InputState>,
    win_asc: gpui::Entity<widgets::input::InputState>,
    win_desc: gpui::Entity<widgets::input::InputState>,
}

/// The Kerning section's inputs: a live filter over the pair list,
/// and a first/second/value editor that commits on Enter.
struct KernInputs {
    filter: gpui::Entity<widgets::input::InputState>,
    first: gpui::Entity<widgets::input::InputState>,
    second: gpui::Entity<widgets::input::InputState>,
    value: gpui::Entity<widgets::input::InputState>,
}

/// Which Font Info field an input commits to.
#[derive(Clone, Copy, PartialEq)]
enum FontInfoField {
    Family,
    Style,
    Upm,
    ItalicAngle,
    Ascender,
    Descender,
    XHeight,
    CapHeight,
    TypoAscender,
    TypoDescender,
    TypoLineGap,
    HheaAscender,
    HheaDescender,
    HheaLineGap,
    WinAscent,
    WinDescent,
    BlueValues,
    OtherBlues,
    StemsH,
    StemsV,
}

/// A flat slider: a thin, evenly colored track (the library's own
/// styling tints the unfilled side with the bar color, which reads as
/// a dark stripe on one side) and a ring thumb that fills solid while
/// it is grabbed, instead of growing a translucent halo.
fn flat_slider(
    state: &gpui::Entity<widgets::slider::SliderState>,
    cx: &gpui::App,
) -> gpui::AnyElement {
    const TRACK: f32 = 3.0;
    const THUMB: f32 = 12.0;

    let pct = state.read(cx).percentage();
    let bar = div()
        .relative()
        .w_full()
        .h(px(TRACK))
        .rounded_full()
        // One colour end to end: this reports a value, it is not a
        // progress bar.
        .bg(t::accent())
        .child(
            div()
                .absolute()
                .top(px((TRACK - THUMB) / 2.0))
                .left(gpui::relative(pct))
                .ml(px(-THUMB / 2.0))
                .w(px(THUMB))
                .h(px(THUMB))
                .flex_shrink_0()
                .rounded_full()
                .border(t::stroke_emphasis())
                .border_color(t::accent())
                .bg(t::panel_bg()),
        );
    widgets::slider::track(state, px(THUMB), bar).into_any_element()
}

/// Everything the blurred preview image depends on, hashed: the line
/// itself, the pane size, the radius and the two colours.
fn blur_key(line: &BezPath, w: f64, h: f64, blur: f32, ink: gpui::Rgba, ground: gpui::Rgba) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for element in line.elements() {
        match element {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => {
                (p.x.to_bits(), p.y.to_bits()).hash(&mut hasher)
            }
            PathEl::QuadTo(a, b) => {
                (a.x.to_bits(), a.y.to_bits(), b.x.to_bits(), b.y.to_bits()).hash(&mut hasher)
            }
            PathEl::CurveTo(a, b, c) => (
                a.x.to_bits(),
                a.y.to_bits(),
                b.x.to_bits(),
                b.y.to_bits(),
                c.x.to_bits(),
                c.y.to_bits(),
            )
                .hash(&mut hasher),
            PathEl::ClosePath => 0u8.hash(&mut hasher),
        }
    }
    (
        w.to_bits(),
        h.to_bits(),
        blur.to_bits(),
        ink.r.to_bits(),
        ink.g.to_bits(),
        ink.b.to_bits(),
        ground.r.to_bits(),
        ground.g.to_bits(),
        ground.b.to_bits(),
    )
        .hash(&mut hasher);
    hasher.finish()
}

/// A drawn eye, for the preview's show/hide switch. The icon set has
/// no eye, and the "preview" icon in it is a hand, which reads as a
/// pan tool.
fn eye_icon(color: gpui::Rgba, open: bool) -> impl IntoElement {
    canvas(
        move |bounds, _, _| bounds,
        move |_, bounds: Bounds<gpui::Pixels>, window, _| {
            let w = f32::from(bounds.size.width) as f64;
            let h = f32::from(bounds.size.height) as f64;
            let o = bounds.origin;
            let (cx_, cy_) = (w / 2.0, h / 2.0);
            let rx = w * 0.40;
            let ry = h * 0.30;
            let pt = |x: f64, y: f64| gpui::point(o.x + px(x as f32), o.y + px(y as f32));
            let mut pb = PathBuilder::stroke(px(1.2));
            // The almond: one curve over, one curve back.
            pb.move_to(pt(cx_ - rx, cy_));
            pb.curve_to(pt(cx_ + rx, cy_), pt(cx_, cy_ - ry * 2.2));
            pb.move_to(pt(cx_ - rx, cy_));
            pb.curve_to(pt(cx_ + rx, cy_), pt(cx_, cy_ + ry * 2.2));
            if !open {
                pb.move_to(pt(cx_ - rx, cy_ + ry));
                pb.line_to(pt(cx_ + rx, cy_ - ry));
            }
            if let Ok(p) = pb.build() {
                window.paint_path(p, color);
            }
            if open {
                use kurbo::Shape as _;
                let pupil = kurbo::Circle::new((cx_, cy_), ry * 0.62).to_path(0.1);
                if let Some(p) = build_fill_path(&pupil, Affine::IDENTITY, o) {
                    window.paint_path(p, color);
                }
            }
        },
    )
    .w(px(16.0))
    .h(px(16.0))
}

/// A drawn plus, minus or cross. Set as text these sit visibly
/// off-centre — a "×" carries its own side bearings and a "−" rides
/// above the middle — so they are stroked instead.
fn glyph_free_icon(color: gpui::Rgba, kind: IconMark) -> impl IntoElement {
    canvas(
        move |bounds, _, _| bounds,
        move |_, bounds: Bounds<gpui::Pixels>, window, _| {
            let w = f32::from(bounds.size.width) as f64;
            let h = f32::from(bounds.size.height) as f64;
            let o = bounds.origin;
            let (cx_, cy_) = (w / 2.0, h / 2.0);
            let r = (w.min(h) / 2.0) * 0.42;
            let pt = |x: f64, y: f64| gpui::point(o.x + px(x as f32), o.y + px(y as f32));
            let mut pb = PathBuilder::stroke(px(1.3));
            match kind {
                IconMark::Plus | IconMark::Minus => {
                    pb.move_to(pt(cx_ - r, cy_));
                    pb.line_to(pt(cx_ + r, cy_));
                    if matches!(kind, IconMark::Plus) {
                        pb.move_to(pt(cx_, cy_ - r));
                        pb.line_to(pt(cx_, cy_ + r));
                    }
                }
                IconMark::Cross => {
                    let d = r * 0.78;
                    pb.move_to(pt(cx_ - d, cy_ - d));
                    pb.line_to(pt(cx_ + d, cy_ + d));
                    pb.move_to(pt(cx_ + d, cy_ - d));
                    pb.line_to(pt(cx_ - d, cy_ + d));
                }
            }
            if let Ok(p) = pb.build() {
                window.paint_path(p, color);
            }
        },
    )
    .size_full()
}

#[derive(Clone, Copy)]
enum IconMark {
    Plus,
    Minus,
    Cross,
}

/// A circle filled on one half: the ink/ground flip.
fn invert_icon(color: gpui::Rgba) -> impl IntoElement {
    canvas(
        move |bounds, _, _| bounds,
        move |_, bounds: Bounds<gpui::Pixels>, window, _| {
            use kurbo::Shape as _;
            let w = f32::from(bounds.size.width) as f64;
            let h = f32::from(bounds.size.height) as f64;
            let o = bounds.origin;
            let r = (w.min(h) / 2.0) - 1.5;
            let center = (w / 2.0, h / 2.0);
            let ring = kurbo::Circle::new(center, r).to_path(0.1);
            if let Some(p) = build_path(&ring, Affine::IDENTITY, o, PathBuilder::stroke(px(1.2))) {
                window.paint_path(p, color);
            }
            // The filled half, as a half-turn arc closed across the
            // diameter.
            let half = kurbo::Arc {
                center: center.into(),
                radii: (r, r).into(),
                start_angle: std::f64::consts::FRAC_PI_2,
                sweep_angle: std::f64::consts::PI,
                x_rotation: 0.0,
            }
            .to_path(0.1);
            if let Some(p) = build_fill_path(&half, Affine::IDENTITY, o) {
                window.paint_path(p, color);
            }
        },
    )
    .w(px(16.0))
    .h(px(16.0))
}

/// Paint many subpaths as few draws without overflowing gpui's
/// tessellator, which indexes vertices with a `u16`: merging a whole
/// screen of glyph outlines into one path exceeds 65,535 vertices,
/// `build` fails, and nothing is drawn at all. Batches are flushed
/// every `CHUNK` subpaths, and a batch that still fails is halved
/// until it builds.
fn paint_batched(
    window: &mut Window,
    origin: Point<gpui::Pixels>,
    color: gpui::Rgba,
    subpaths: &[BezPath],
    stroke: Option<f32>,
) {
    const CHUNK: usize = 12;
    fn paint_chunk(
        window: &mut Window,
        origin: Point<gpui::Pixels>,
        color: gpui::Rgba,
        chunk: &[BezPath],
        stroke: Option<f32>,
    ) {
        if chunk.is_empty() {
            return;
        }
        let mut merged = BezPath::new();
        for path in chunk {
            merged.extend(path.iter());
        }
        let built = match stroke {
            Some(width) => build_path(
                &merged,
                Affine::IDENTITY,
                origin,
                PathBuilder::stroke(px(width)),
            ),
            None => build_fill_path(&merged, Affine::IDENTITY, origin),
        };
        match built {
            Some(path) => window.paint_path(path, color),
            // Too much geometry for one path: split and retry, down to
            // a single subpath.
            None if chunk.len() > 1 => {
                let mid = chunk.len() / 2;
                paint_chunk(window, origin, color, &chunk[..mid], stroke);
                paint_chunk(window, origin, color, &chunk[mid..], stroke);
            }
            None => {}
        }
    }
    for chunk in subpaths.chunks(CHUNK) {
        paint_chunk(window, origin, color, chunk, stroke);
    }
}

/// One cell placed by the packer: which glyph, and the rectangle it
/// occupies inside the grid's viewport.
#[derive(Clone, Copy)]
struct PlacedCell {
    glyph: usize,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

/// Lay the packed rows out exactly as the wrapping flex will: the
/// block is centred, cells run left to right with one gap between,
/// and rows stack by the cell height.
///
/// `viewport` has to be the box the cells are actually being laid out
/// in, measured this frame — not the probe's stored size. The probe
/// lags the layout by a frame (longer, if the browser coalesces the
/// redraw), and a viewport a column narrower than the real one puts
/// every outline a column away from its cell.
fn place_cells(
    packed: &[Vec<(usize, usize)>],
    fit: GridFit,
    viewport: gpui::Size<gpui::Pixels>,
    start_row: usize,
) -> Vec<PlacedCell> {
    let rows: Vec<&Vec<(usize, usize)>> = packed.iter().skip(start_row).take(fit.rows).collect();
    if rows.is_empty() {
        return Vec::new();
    }
    let content_w = fit.content_w();
    let block_h = fit.cell_h * rows.len() as f32 + GRID_GAP * (rows.len() - 1) as f32;
    let vw: f32 = viewport.width.into();
    let vh: f32 = viewport.height.into();
    let x0 = ((vw - content_w) / 2.0).max(0.0);
    let y0 = ((vh - block_h) / 2.0).max(0.0);
    let mut out = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        let mut x = x0;
        let y = y0 + r as f32 * (fit.cell_h + GRID_GAP);
        for &(glyph, span) in row.iter() {
            let w = fit.cell_w * span as f32 + GRID_GAP * (span - 1) as f32;
            out.push(PlacedCell {
                glyph,
                x,
                y,
                w,
                h: fit.cell_h,
            });
            x += w + GRID_GAP;
        }
    }
    out
}

/// Where a glyph's outline sits inside a cell, as an affine from
/// design space to the cell's local pixels. Ported from the web's
/// grid thumbnail box (`glyph_svg.rs`): one vertical scale for every
/// glyph so a period stays a dot and an M stays tall, each centred on
/// its own ink, and the em window grows rather than cropping ink that
/// runs past it.
fn cell_glyph_transform(
    ink: kurbo::Rect,
    empty: bool,
    advance: f64,
    upm: f64,
    w: f64,
    h: f64,
) -> Affine {
    const EM_FILL: f64 = 0.65;
    const BASELINE_FROM_TOP: f64 = 0.8;
    let (ink_x0, ink_w) = if empty || ink.width() <= 0.0 {
        (0.0, advance.max(1.0))
    } else {
        (ink.x0, ink.width())
    };
    let em_height = upm.max(1.0) / EM_FILL;
    let em_top = -BASELINE_FROM_TOP * em_height;
    let (top, bottom) = if empty {
        (em_top, em_top + em_height)
    } else {
        (em_top.min(-ink.y1), (em_top + em_height).max(-ink.y0))
    };
    let box_h = (bottom - top).max(1.0);
    let scale = (w / ink_w).min(h / box_h);
    let x_offset = (w - ink_w * scale) / 2.0 - ink_x0 * scale;
    let baseline = (h - box_h * scale) / 2.0 - top * scale;
    Affine::translate((x_offset, baseline)) * Affine::scale_non_uniform(scale, -scale)
}

/// A cell's label block: whether it shows at all, its type size, and
/// the height it takes. Mirrors the web's cell-labels box — 8px sides
/// and bottom, a 2px gap, both lines the same size.
fn cell_label_metrics(cell_w: f32) -> CellLabels {
    // gpui's default line box is much taller than the type size, which
    // clipped the first line and pushed the two apart. The line height
    // is stated here and the block's height is derived from it, so the
    // box always holds exactly what it draws.
    const PAD_TOP: f32 = 4.0;
    const PAD_BOTTOM: f32 = 8.0;
    const GAP: f32 = 2.0;
    let build = |size: f32, lines: usize| {
        let line = (size * 1.25).ceil();
        CellLabels {
            show: true,
            size,
            line,
            height: PAD_TOP
                + line * lines as f32
                + GAP * (lines.saturating_sub(1)) as f32
                + PAD_BOTTOM,
        }
    };
    if cell_w < 34.0 {
        // Too small to carry text: a pure thumbnail.
        CellLabels {
            show: false,
            size: 0.0,
            line: 0.0,
            height: 0.0,
        }
    } else if cell_w < 90.0 {
        // Name only.
        build(10.0, 1)
    } else {
        build(12.0, 2)
    }
}

/// Everything that decides which glyphs show and in what order. When
/// this is unchanged, the order is too.
#[derive(Clone, PartialEq)]
struct OrderKey {
    query: String,
    mode: u8,
    regex: bool,
    case: bool,
    sort_unicode: bool,
    filter: SidebarFilter,
    /// Structural changes to the font (a glyph added, removed or
    /// renamed) bump this.
    revision: u64,
    /// Masters can differ in what they contain.
    master: usize,
}

/// The label block's type size, line height and total height.
#[derive(Clone, Copy)]
struct CellLabels {
    show: bool,
    size: f32,
    line: f32,
    height: f32,
}

/// How many columns a glyph should take, ported from the web's
/// `computeGlyphColumnSpan`: a long name or a wide advance gets more
/// room instead of being cut off.
fn glyph_column_span(name: &str, advance: f64, upm: f64) -> usize {
    let name_span = match name.chars().count() {
        0..=14 => 1,
        15..=26 => 2,
        _ => 3,
    };
    let ratio = if upm > 0.0 { advance / upm } else { 0.0 };
    let width_span = if ratio <= 1.5 {
        1
    } else if ratio <= 2.8 {
        2
    } else if ratio <= 4.0 {
        3
    } else {
        4
    };
    name_span.max(width_span)
}

/// Pack spanned cells into rows that each fill the width exactly: when
/// the next cell will not fit, the last one on the row grows into the
/// gap (the web's `gridGlyphItems`). Returns one vector per row of
/// (item index, span).
fn pack_spans(spans: &[(usize, usize)], cols: usize) -> Vec<Vec<(usize, usize)>> {
    let cols = cols.max(1);
    let mut rows: Vec<Vec<(usize, usize)>> = Vec::new();
    let mut row: Vec<(usize, usize)> = Vec::new();
    let mut used = 0usize;
    for &(item, span) in spans {
        let span = span.clamp(1, cols);
        if used + span > cols && !row.is_empty() {
            if let Some(last) = row.last_mut() {
                last.1 += cols - used;
            }
            rows.push(std::mem::take(&mut row));
            used = 0;
        }
        row.push((item, span));
        used += span;
        if used == cols {
            rows.push(std::mem::take(&mut row));
            used = 0;
        }
    }
    if !row.is_empty() {
        if let Some(last) = row.last_mut() {
            last.1 += cols - used;
        }
        rows.push(row);
    }
    rows
}

/// How a grid of glyph cells fits its pane: cell size, and how many
/// columns and whole rows are on screen.
#[derive(Clone, Copy)]
struct GridFit {
    cell_w: f32,
    cell_h: f32,
    cols: usize,
    rows: usize,
}

impl GridFit {
    /// Exact width of a full row of cells, gaps included.
    fn content_w(&self) -> f32 {
        self.cell_w * self.cols as f32 + GRID_GAP * (self.cols - 1) as f32
    }
}

const CELL: f32 = 96.0;
/// Target cell size for the editor sidebar's mini grid.
const MINI_CELL: f32 = 44.0;
/// Height of every bottom bar, so the ones in neighbouring columns
/// line up across the divider.
const BOTTOM_BAR_H: f32 = 28.0;
/// Square buttons in a bottom bar, sized so the space above, below and
/// beside them is the same.
const BAR_BUTTON: f32 = 20.0;
/// Wheel zoom response and limits, matching the web editor.
const ZOOM_PER_PIXEL: f64 = 0.0015;
const ZOOM_MIN: f64 = 1e-3;
const ZOOM_MAX: f64 = 1e4;
/// One press of the zoom keys.
const ZOOM_KEY_STEP: f64 = 1.1;
/// Height of a header tab, and the side of the square icon buttons
/// that sit beside tabs in the header and the status bar.
const TAB_H: f32 = 24.0;
/// Gap between grid cells, and the grid's inner padding.
const GRID_GAP: f32 = 8.0;
const GRID_PAD: f32 = 12.0;
const GRID_PAD_Y: f32 = 8.0;
/// The sidebar's mini grid is narrow: it spares less padding, but the
/// fit is solved the same way.
const GRID_PAD_SM: f32 = 6.0;
const HIT_RADIUS_PX: f64 = 10.0;
/// Points are easier to grab than segments: the web select tool gives
/// them a wider radius (SELECT_POINT_HIT_DISTANCE) than the 10px it
/// uses for segments, metric edges and components.
const POINT_HIT_RADIUS_PX: f64 = 16.0;

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Claim focus only when nothing else has it, so text inputs
        // (the search box) keep theirs while focused.
        if window.focused(cx).is_none() {
            window.focus(&self.focus_handle, cx);
        }

        self.ensure_axis_sliders(window, cx);
        self.ensure_cell_slider(window, cx);
        self.ensure_sidebar_slider(window, cx);
        self.ensure_preview_slider(window, cx);
        self.ensure_model_strength_slider(window, cx);
        if self.sidebar_counts.is_none() && self.project.is_some() {
            self.rebuild_sidebar_cache();
        }
        // One filter-and-sort pass per frame at most, and none at all
        // when nothing that decides the order has changed.
        self.visible_glyphs();
        self.refresh_metric_inputs(false, window, cx);
        self.refresh_font_info_inputs(false, window, cx);
        self.refresh_features_input(false, window, cx);
        if matches!(self.mode, Mode::Editor(_)) {
            self.refresh_coord_inputs(false, window, cx);
        }
        self.refresh_glyph_inputs(false, window, cx);
        use widgets::resizable::{h_resizable, resizable_panel, v_resizable};

        // Glyphs-style docked layout: left sidebar | center | right
        // sidebar as flat resizable panels, no floating containers.
        let (left, center): (gpui::AnyElement, gpui::AnyElement) = match self.mode {
            Mode::Editor(index) if self.project.is_some() => (
                self.editor_sidebar(cx).into_any_element(),
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .min_h(px(0.0))
                    // Canvas over preview, with a draggable divider:
                    // the preview takes whatever height it is given and
                    // fits its type to it. The bottom bar belongs to
                    // this column too, so the side panels keep the
                    // window's full height and the dividers between
                    // the three columns run all the way down.
                    .child(
                        div().flex_1().min_h(px(0.0)).child(
                            v_resizable("editor-column")
                                .child(
                                    resizable_panel().child(
                                        // A flex column: the canvas
                                        // grows into the panel. In a
                                        // plain block its flex_1 is
                                        // ignored and it lays out at
                                        // zero height.
                                        div()
                                            .size_full()
                                            .min_h(px(0.0))
                                            .flex()
                                            .flex_col()
                                            .child(self.editor_view(index, cx)),
                                    ),
                                )
                                .child(
                                    resizable_panel()
                                        .size(px(140.0))
                                        .size_range(px(0.0)..px(720.0))
                                        .visible(self.preview_visible)
                                        .child(self.preview_strip(cx)),
                                ),
                        ),
                    )
                    .child(self.status_bar(cx))
                    .into_any_element(),
            ),
            _ => {
                let _query = self.search_query.clone();
                let fit = self.grid_cell_metrics();
                let (cell_w, cell_h) = (fit.cell_w, fit.cell_h);
                let mut rows_total = 0usize;
                let indices = self.glyph_order();
                let mut visible_rows: Vec<Vec<(usize, usize)>> = Vec::new();
                let grid: Vec<_> = match self.font() {
                    Some(font) => {
                        // A wide advance or a long name takes more
                        // than one column, and the last cell on a row
                        // grows into whatever is left, so every row
                        // fills the width and no name is cut off.
                        let upm = font.units_per_em;
                        let spans: Vec<(usize, usize)> = indices
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
                        // Only the rows on screen are built: the view
                        // starts at a row boundary and holds exactly
                        // the rows that fit, so nothing is ever half
                        // drawn at either edge.
                        let start = self.grid_scroll_row.min(rows_total.saturating_sub(1));
                        visible_rows = packed.iter().skip(start).take(fit.rows).cloned().collect();
                        packed
                            .into_iter()
                            .skip(start)
                            .take(fit.rows)
                            .flatten()
                            .map(|(i, span)| {
                                let w = cell_w * span as f32 + GRID_GAP * (span - 1) as f32;
                                self.glyph_cell_sized(i, w, cell_h, false, cx)
                                    .into_any_element()
                            })
                            .collect()
                    }
                    None => Vec::new(),
                };
                // The grid solves its own layout against the viewport,
                // so it needs the viewport measured. An inert canvas
                // laid over the scroll area reports its bounds back.
                let this = cx.entity().downgrade();
                let probe = canvas(
                    move |bounds: Bounds<gpui::Pixels>, _, app: &mut gpui::App| {
                        this.update(app, |this, cx| {
                            if this.grid_viewport != bounds.size {
                                this.grid_viewport = bounds.size;
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
                .size_full();
                (
                    self.category_sidebar(cx).into_any_element(),
                    div()
                        .size_full()
                        .min_h(px(0.0))
                        .flex()
                        .flex_col()
                        .child({
                            let grid_block = div()
                                .id("glyph-grid")
                                .flex_1()
                                .min_h(px(0.0))
                                .relative()
                                .overflow_hidden()
                                .child(probe)
                                .child(
                                    // The block is sized to exactly
                                    // cols x rows and centred, so the
                                    // pixels left over by rounding the
                                    // cell size split evenly between
                                    // the margins instead of piling up
                                    // on the right and bottom.
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
                                                .children(grid),
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
                                            &mut this.grid_scroll_row,
                                            dy,
                                            fit.cell_h + GRID_GAP,
                                            fit.rows,
                                            rows_total,
                                        ) {
                                            cx.notify();
                                        }
                                    },
                                ));
                            // List swaps the whole grid for the
                            // property table; Grid and Detail share
                            // the cell pipeline.
                            match self.font_view_mode {
                                FontViewMode::List => self.glyph_list_view(cx),
                                FontViewMode::Matrix => self.glyph_matrix_view(cx),
                                _ => grid_block.into_any_element(),
                            }
                        })
                        // One bar per column bottom: the grid's lives
                        // here so the sidebars run past it.
                        .child(self.status_bar(cx))
                        .into_any_element(),
                )
            }
        };
        let in_editor = matches!(self.mode, Mode::Editor(_)) && self.project.is_some();
        let right = div()
            .id("right-sidebar")
            .size_full()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .when(in_editor, |el| {
                el.child(self.glyph_info_panel(cx))
                    .child(self.selection_section(cx))
                    .child(self.transform_section(cx))
                    .child(self.curves_section(cx))
                    .child(self.background_section(cx))
                    .child(self.color_section(cx))
                    .child(self.shaping_section(cx))
                    .child(self.related_section(cx))
                    .child(self.layers_section(cx))
                    .children(self.axes_section(cx))
            })
            .when(!in_editor, |el| {
                el.child(self.glyph_info_panel(cx))
                    .child(self.font_info_section(cx))
                    .child(self.dimensions_section(cx))
                    .child(self.kerning_section(cx))
                    .child(self.groups_section(cx))
                    .child(self.compare_section(cx))
                    .child(self.features_section(cx))
                    .child(self.layers_section(cx))
                    .child(self.glyph_preview_panel())
            });
        let content = div()
            .flex_1()
            .min_h(px(0.0))
            .child(
                h_resizable("workspace")
                    .child(
                        resizable_panel()
                            .size(px(224.0))
                            .size_range(px(140.0)..px(440.0))
                            .visible(!self.left_collapsed)
                            .child(
                                // No border here: the resize handle
                                // already paints a 1px divider in the
                                // same color, and the two together
                                // read as a thick line.
                                div().size_full().bg(t::panel_bg()).child(left),
                            ),
                    )
                    .child(resizable_panel().child(center))
                    .child(
                        resizable_panel()
                            .size(px(230.0))
                            .size_range(px(170.0)..px(440.0))
                            .child(div().size_full().bg(t::panel_bg()).child(right)),
                    ),
            )
            .into_any_element();

        // The window's text style. This used to come from the
        // wrapper the app was mounted in; without it gpui has no
        // family to shape with and no UI text draws at all.
        window.set_rem_size(px(16.0));

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(t::window_bg())
            .font_family(ui_font_family(cx))
            .text_color(t::text())
            .text_size(px(13.0))
            .key_context("Workspace")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &OpenFont, _, cx| {
                this.open_dialog(cx);
            }))
            .on_action(cx.listener(|this, _: &NewFont, _, cx| {
                this.command_new_font();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SaveFontAs, _, cx| {
                this.command_save_as(cx);
            }))
            .on_action(cx.listener(|this, _: &SaveFont, _, cx| {
                this.command_save(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ExportFont, _, cx| {
                this.command_export(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Undo, _, cx| {
                this.undo();
                this.rebuild_text_models();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Redo, _, cx| {
                this.redo();
                this.rebuild_text_models();
                cx.notify();
            }))
            .on_action(
                cx.listener(|this, _: &CopyContours, window: &mut Window, cx| {
                    // A focused field handles its own clipboard.
                    if widgets::input::any_field_focused(window, cx) {
                        return;
                    }
                    this.command_copy();
                    cx.notify();
                }),
            )
            .on_action(
                cx.listener(|this, _: &PasteContours, window: &mut Window, cx| {
                    if widgets::input::any_field_focused(window, cx) {
                        return;
                    }
                    this.command_paste_routed(cx);
                    cx.notify();
                }),
            )
            .on_action(cx.listener(|this, _: &CopySelectedGlyphs, _, cx| {
                this.command_copy_selection_text(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &MeasureColorize, _, cx| {
                this.toggle_measure(|o| o.colorize = !o.colorize, cx);
            }))
            .on_action(cx.listener(|this, _: &MeasureHandles, _, cx| {
                this.toggle_measure(|o| o.handles = !o.handles, cx);
            }))
            .on_action(cx.listener(|this, _: &MeasureSegments, _, cx| {
                this.toggle_measure(|o| o.segments = !o.segments, cx);
            }))
            .on_action(cx.listener(|this, _: &MeasureSizes, _, cx| {
                this.toggle_measure(|o| o.sizes = !o.sizes, cx);
            }))
            .on_action(cx.listener(|this, _: &MeasureSpans, _, cx| {
                this.toggle_measure(|o| o.spans = !o.spans, cx);
            }))
            .on_action(cx.listener(|this, _: &MeasureSideBearings, _, cx| {
                this.toggle_measure(|o| o.sidebearings = !o.sidebearings, cx);
            }))
            .on_action(cx.listener(|this, _: &MeasurePopcount, _, cx| {
                this.toggle_measure(|o| o.popcount = !o.popcount, cx);
            }))
            .on_action(cx.listener(|this, _: &MeasureAllOn, _, cx| {
                this.toggle_measure(
                    |o| {
                        o.colorize = true;
                        o.handles = true;
                        o.segments = true;
                        o.spans = true;
                        o.sidebearings = true;
                        o.sizes = true;
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &MeasureAllOff, _, cx| {
                this.toggle_measure(
                    |o| {
                        o.colorize = false;
                        o.handles = false;
                        o.segments = false;
                        o.spans = false;
                        o.sidebearings = false;
                        o.sizes = false;
                    },
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &SetThemeDark, window, cx| {
                this.command_set_theme("dark", window, cx);
            }))
            .on_action(cx.listener(|this, _: &SetThemeMidnight, window, cx| {
                this.command_set_theme("midnight", window, cx);
            }))
            .on_action(cx.listener(|this, _: &SetThemeGray, window, cx| {
                this.command_set_theme("gray", window, cx);
            }))
            .on_action(cx.listener(|this, _: &SetThemeLight, window, cx| {
                this.command_set_theme("light", window, cx);
            }))
            .on_action(cx.listener(|this, _: &RemoveOverlap, _, cx| {
                this.command_remove_overlap();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Decompose, _, cx| {
                this.command_decompose();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FlipHorizontal, _, cx| {
                this.apply_transform(Affine::scale_non_uniform(-1.0, 1.0));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FlipVertical, _, cx| {
                this.apply_transform(Affine::scale_non_uniform(1.0, -1.0));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &RotateLeft, _, cx| {
                this.apply_transform(Affine::rotate(std::f64::consts::FRAC_PI_2));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &RotateRight, _, cx| {
                this.apply_transform(Affine::rotate(-std::f64::consts::FRAC_PI_2));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ReverseContours, _, cx| {
                this.command_reverse();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &BooleanUnion, _, cx| {
                this.command_boolean(linesweeper::BinaryOp::Union);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &BooleanSubtract, _, cx| {
                this.command_boolean(linesweeper::BinaryOp::Difference);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &BooleanIntersect, _, cx| {
                this.command_boolean(linesweeper::BinaryOp::Intersection);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &BooleanExclude, _, cx| {
                this.command_boolean(linesweeper::BinaryOp::Xor);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &DuplicateSelection, _, cx| {
                this.command_duplicate();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &DuplicateRepeat, _, cx| {
                this.command_duplicate_repeat();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &HyperToCubic, _, cx| {
                if let Mode::Editor(index) = this.mode {
                    this.push_undo_snapshot(index);
                    let selected = this.editor.selected.clone();
                    let ok = this
                        .font_mut()
                        .and_then(|f| {
                            f.edit_glyph(index, |g| {
                                runebender_core::outline::glyph_ops::convert_hyper_to_cubic(
                                    g, &selected,
                                )
                            })
                        })
                        .unwrap_or(false);
                    if !ok {
                        this.editor.undo.pop();
                    } else {
                        this.editor.selected.clear();
                    }
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &AddExtremes, _, cx| {
                this.command_add_extremes();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &TidyPaths, _, cx| {
                this.command_tidy_paths();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &CorrectPathDirection, _, cx| {
                this.command_correct_path_direction();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &RoundCoordinates, _, cx| {
                this.command_round_coordinates();
                cx.notify();
            }))
            .on_action(
                cx.listener(|this, _: &SelectAllPoints, window: &mut Window, cx| {
                    if widgets::input::any_field_focused(window, cx) {
                        return;
                    }
                    this.command_select_points(0);
                    cx.notify();
                }),
            )
            .on_action(cx.listener(|this, _: &DeselectAllPoints, _, cx| {
                this.command_select_points(1);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &InvertPointSelection, _, cx| {
                this.command_select_points(2);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &NewGlyph, _, cx| {
                this.command_add_glyph();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &DuplicateGlyph, _, cx| {
                this.command_duplicate_glyph();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &RemoveGlyphCmd, _, cx| {
                this.command_remove_glyph();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FilterOffsetCurve, _, cx| {
                if let Ok(delta) = this.offset_input.read(cx).value().trim().parse::<f64>() {
                    this.command_offset(delta);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FilterExtrude, _, cx| {
                let text = this.extrude_input.read(cx).value().to_string();
                this.command_extrude(&text);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FilterRoughen, _, cx| {
                let text = this.roughen_input.read(cx).value().to_string();
                this.command_roughen(&text);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FilterSlant, _, cx| {
                if let Ok(deg) = this.slant_input.read(cx).value().trim().parse::<f64>()
                    && deg != 0.0
                    && deg.abs() < 89.0
                {
                    // Positive leans right, the italic convention.
                    this.apply_transform(Affine::skew(deg.to_radians().tan(), 0.0));
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ExportGlyphSvg, _, cx| {
                this.command_export_glyph_svg();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Reinterpolate, _, cx| {
                this.command_reinterpolate();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SyncMetrics, _, cx| {
                this.command_sync_metrics();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &BakeMasks, _, cx| {
                this.command_bake_masks();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &CheckJoining, _, cx| {
                this.command_check_joining();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &QuadsToCubics, _, cx| {
                this.command_convert_curves(true);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &CubicsToQuads, _, cx| {
                this.command_convert_curves(false);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ShowAllMasters, _, cx| {
                this.show_all_masters = !this.show_all_masters;
                this.status_note = Some(
                    if this.show_all_masters {
                        "All masters shown · click any master's node to edit it"
                    } else {
                        "Showing the active master"
                    }
                    .into(),
                );
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &NextSampleString, _, cx| {
                this.command_sample_string(1);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &PreviousSampleString, _, cx| {
                this.command_sample_string(-1);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &RoundCorners, _, cx| {
                this.command_round_corners();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &PlaceImage, _, cx| {
                this.command_place_image(cx);
            }))
            .on_action(cx.listener(|this, _: &ImportSvg, _, cx| {
                this.command_import_svg(cx);
            }))
            .on_action(cx.listener(|this, _: &RemoveImage, _, cx| {
                this.command_remove_image();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &BoldenWithModel, _, cx| {
                this.command_bolden_with_model(cx);
            }))
            .on_action(cx.listener(|this, _: &TraceImage, _, cx| {
                this.command_trace_image(cx);
            }))
            .on_action(cx.listener(|this, _: &Rotate180, _, cx| {
                this.apply_transform(Affine::rotate(std::f64::consts::PI));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SetStartPoint, _, cx| {
                this.command_set_start_point();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Harmonize, _, cx| {
                this.apply_curve_op(CurveOp::Harmonize);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Balance, _, cx| {
                this.apply_curve_op(CurveOp::Balance);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Optimize, _, cx| {
                this.apply_curve_op(CurveOp::Optimize(0.12));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SortByName, _, cx| {
                this.sort_unicode = false;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SortByUnicode, _, cx| {
                this.sort_unicode = true;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ZoomToFit, _, cx| {
                if matches!(this.mode, Mode::Editor(_)) {
                    this.editor.initialized = false;
                    this.ensure_editor_fit();
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &NextMaster, _, cx| {
                this.command_step_master(1);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &PreviousMaster, _, cx| {
                this.command_step_master(-1);
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if this.handle_key(event, window, cx) {
                    cx.notify();
                }
            }))
            .on_key_up(cx.listener(|this, event: &gpui::KeyUpEvent, _, cx| {
                if matches!(
                    event.keystroke.key.as_str(),
                    "left" | "right" | "up" | "down"
                ) {
                    this.nudging = false;
                }
                if event.keystroke.key.as_str() == "space" && this.editor.tool == Tool::Preview {
                    this.editor.tool = this.editor.previous_tool;
                    cx.notify();
                }
            }))
            .child(self.header(cx))
            .child(content)
    }
}

// ============================================================================
// ENTRY
// ============================================================================

/// Find the fontc compiler: PATH first, then the default cargo
/// install location, because an app launched from the Dock does not
/// inherit a shell PATH.
#[cfg(not(target_family = "wasm"))]
fn fontc_binary() -> Option<PathBuf> {
    if std::process::Command::new("fontc")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
    {
        return Some(PathBuf::from("fontc"));
    }
    let home = std::env::var_os("HOME")?;
    let cargo_bin = PathBuf::from(home).join(".cargo/bin/fontc");
    cargo_bin.exists().then_some(cargo_bin)
}

fn default_font_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../runebender-web/assets/test-fonts/VirtuaGrotesk.designspace")
}

#[cfg(target_family = "wasm")]
thread_local! {
    /// Keeps the gpui application alive: on the web the event loop
    /// belongs to the browser, so run_embedded returns a handle that
    /// must not drop.
    static APPLICATION: std::cell::RefCell<Option<gpui::ApplicationHandle>> =
        const { std::cell::RefCell::new(None) };
}

/// The config file's contents, read once before the window opens.
///
/// A `OnceLock` rather than a re-read per call: the file is read at
/// startup and changing it means restarting, which is the same promise
/// the theme menu makes.
static CONFIG: std::sync::OnceLock<config::Config> = std::sync::OnceLock::new();

fn main() {
    #[cfg(target_family = "wasm")]
    gpui_platform::web_init();

    // Settings before anything draws, so the first frame is already in
    // the chosen theme rather than flashing the default first.
    #[cfg(not(target_family = "wasm"))]
    {
        let config = config::load();
        if let Some(theme) = config.theme.as_deref()
            && !t::set_theme(theme)
        {
            eprintln!(
                "runebender: config names theme {theme:?}, which does not exist; \
                     using the default"
            );
        }
        CONFIG.set(config).ok();
    }

    // `runebender-gpui --fonts` lists the families gpui can resolve
    // and exits without opening a window. A family it cannot resolve
    // shapes to nothing and the interface comes up wordless, so this
    // is the first thing to check when that happens.
    #[cfg(not(target_family = "wasm"))]
    if std::env::args().any(|a| a == "--fonts") {
        gpui_platform::application().run(|cx: &mut App| {
            print_font_families(cx);
            cx.quit();
        });
        return;
    }

    #[cfg(not(target_family = "wasm"))]
    let (project, load_error) = open_from_args();
    // The web build has no filesystem: open the embedded demo
    // designspace (a host data layer over fetch comes later).
    #[cfg(target_family = "wasm")]
    let (project, load_error): (Option<Project>, Option<SharedString>) =
        match web_host::demo_project() {
            Ok(p) => (Some(p), None),
            Err(e) => (None, Some(e.into())),
        };

    // QA hook: RB_OPEN_GLYPH=<name> starts in the editor on that
    // glyph, so agent screenshots can reach it without clicks.
    let start_mode = std::env::var("RB_OPEN_GLYPH")
        .ok()
        .and_then(|name| {
            let p = project.as_ref()?;
            p.active_font()
                .glyphs
                .iter()
                .position(|g| g.name.as_ref() == name)
        })
        .map(Mode::Editor)
        .unwrap_or(Mode::Grid);

    #[cfg(not(target_family = "wasm"))]
    let app = gpui_platform::application();
    #[cfg(target_family = "wasm")]
    let app = gpui_platform::single_threaded_web();
    let launch = move |cx: &mut App| {
        // The keymap for app commands; menu items show these as their
        // key equivalents.
        cx.bind_keys(keymap());
        cx.on_action(|_: &Quit, cx| cx.quit());

        // One menu definition, three consumers: the macOS native bar
        // (set_menus), the stored menus on Windows/Linux, and the
        // in-window bar drawn where no native bar exists.
        #[cfg(not(target_family = "wasm"))]
        cx.set_menus(app_menus());

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
            |window, cx| {
                let workspace =
                    cx.new(|cx| Workspace::new(window, cx, project, load_error, start_mode));
                // Handle shortcuts before any binding runs, so Tab
                // cycles the point selection (the web behavior)
                // rather than walking tab stops.
                //
                // On wasm this also stands in for action dispatch,
                // which used to panic: gpui-component force-enabled
                // gpui's "profiler" feature, whose action timing calls
                // std::time::Instant::now, unsupported there. That
                // dependency is gone, so the wasm arm below should be
                // removable once a browser build confirms actions
                // dispatch cleanly.
                Workspace::install_shortcuts(&workspace, cx);
                // The workspace is the window root: nothing here used
                // the dialog, sheet, notification, and tooltip layers
                // the old wrapper existed to provide.
                workspace
            },
        )
        .unwrap();
        cx.activate(true);
    };
    #[cfg(target_family = "wasm")]
    APPLICATION.with(|application| {
        *application.borrow_mut() = Some(app.run_embedded(launch));
    });
    #[cfg(not(target_family = "wasm"))]
    app.run(launch);
}
