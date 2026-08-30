// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! Runebender GPUI: a font editor built on [GPUI](https://gpui.rs/),
//! started as a point of comparison against
//! [runebender-xilem](https://github.com/eliheuer/runebender-xilem).

mod actions;
mod edit;
mod platform;
mod startup;
#[cfg(test)]
mod tests;
mod view;
mod widgets;
mod workspace;

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
#[cfg(not(target_family = "wasm"))]
use runebender_core::formats::svg::glyph_svg;
use runebender_core::formats::svg::svg_to_contours;
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

use actions::*;
#[cfg(not(target_family = "wasm"))]
use platform::host::*;
use platform::*;
use startup::keymap;
use startup::*;
#[cfg(not(target_family = "wasm"))]
use startup::{open_from_args, print_font_families};
use view::grid::*;
use view::paint::*;
use view::render::TabTooltip;
use view::theme as t;
use view::*;
use workspace::*;

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

// ============================================================================
// GLYPH PAINTING
// ============================================================================

// ---- joining QA (Arabic connecting-stroke bands) ----

// ---- COLRv1 (paint graphs through the ufo2ft colorLayers key) ----

// ---- masks (subtracting contours, the Glyphs path attribute) ----

// ---- annotations (canvas notes, arrows, circles) ----

// ---- SVG outline import ----

// ---- cubic <-> quadratic conversion ----

// ---- compiled-font import (TTF/OTF via skrifa) ----

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

// ============================================================================
// WORKSPACE VIEW
// ============================================================================

// ============================================================================
// ENTRY
// ============================================================================

#[cfg(target_family = "wasm")]
thread_local! {
    /// Keeps the gpui application alive: on the web the event loop
    /// belongs to the browser, so run_embedded returns a handle that
    /// must not drop.
    static APPLICATION: std::cell::RefCell<Option<gpui::ApplicationHandle>> =
        const { std::cell::RefCell::new(None) };
}

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
