// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! Runebender GPUI: a font editor built on [GPUI](https://gpui.rs/),
//! started as a point of comparison against
//! [runebender-xilem](https://github.com/eliheuer/runebender-xilem).

mod blur;
mod canvas;
#[cfg(test)]
mod tests;
mod commands;
mod glyph_path;
mod input;
mod panels;
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

use runebender_core::editing::ViewPort;
use runebender_core::glyph_ops::{self as ops, CurveOp, GlyphSnapshot};

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
    smooth: bool,
    /// Point in a hyperbezier contour (drawn in its own color).
    hyper: bool,
    contour: usize,
    index: usize,
}

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
    const PREFERRED: &[&str] = &[
        ".SystemUIFont",
        "SF Pro Text",
        "SF Pro Display",
        "Helvetica Neue",
        "Helvetica",
        "Segoe UI",
        "Inter",
        "DejaVu Sans",
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

/// One glyph, ready to paint: outline in font units (Y-up), advance
/// width, and identifying info.
struct GlyphEntry {
    name: SharedString,
    codepoint: Option<char>,
    /// Contours + components combined (grid, preview).
    path: Arc<BezPath>,
    /// The glyph's own contours only (editor fill).
    contour_path: Arc<BezPath>,
    /// Resolved components only (editor, distinct color).
    component_path: Arc<BezPath>,
    points: Arc<Vec<GlyphPoint>>,
    anchors: Arc<Vec<(SharedString, f64, f64)>>,
    advance: f64,
    component_names: Arc<Vec<SharedString>>,
    /// Mark label ("red", "green", …) from the glyph lib, if any.
    mark: Option<SharedString>,
    /// The outline's bounding box, kept so the grid does not walk every
    /// path element again on every frame.
    ink: kurbo::Rect,
}

struct FontModel {
    font: norad::Font,
    /// Names of glyphs edited since load/save (partial saves).
    modified_glyphs: std::collections::HashSet<String>,
    /// glyph name → glif path relative to the UFO root (memory hosts).
    glif_paths: std::collections::HashMap<String, String>,
    /// Kerning changed since load/save.
    kerning_dirty: bool,
    /// codepoint → index into `glyphs`, for the text preview.
    /// glyph name → index into `glyphs` (text buffer sorts carry
    /// names, including unencoded ligature glyphs from shaping).
    name_map: std::collections::HashMap<String, usize>,
    source_path: PathBuf,
    units_per_em: f64,
    ascender: f64,
    descender: f64,
    /// Optional guides: drawn only when fontinfo defines them, like
    /// the web's metric guides.
    x_height: Option<f64>,
    cap_height: Option<f64>,
    glyphs: Vec<GlyphEntry>,
    /// Bumped when the glyph list itself changes (added, removed,
    /// renamed), so caches keyed on the list can tell.
    revision: u64,
    dirty: bool,
}

fn extract_anchors(glyph: &norad::Glyph) -> Vec<(SharedString, f64, f64)> {
    glyph
        .anchors
        .iter()
        .map(|a| {
            (
                a.name
                    .as_ref()
                    .map(|n| n.to_string())
                    .unwrap_or_default()
                    .into(),
                a.x,
                a.y,
            )
        })
        .collect()
}

fn extract_points(glyph: &norad::Glyph) -> Vec<GlyphPoint> {
    glyph
        .contours
        .iter()
        .enumerate()
        .flat_map(|(ci, c)| {
            let hyper = runebender_core::model::workspace::norad_contour_is_hyper(c);
            c.points.iter().enumerate().map(move |(pi, p)| GlyphPoint {
                x: p.x,
                y: p.y,
                on_curve: p.typ != norad::PointType::OffCurve,
                smooth: p.smooth,
                hyper,
                contour: ci,
                index: pi,
            })
        })
        .collect()
}

impl FontModel {
    /// Run an op on the named glyph's norad data, then rebuild caches.
    fn edit_glyph<R>(
        &mut self,
        glyph_index: usize,
        op: impl FnOnce(&mut norad::Glyph) -> R,
    ) -> Option<R> {
        let name = self.glyphs[glyph_index].name.to_string();
        let result = self
            .font
            .default_layer_mut()
            .get_glyph_mut(name.as_str())
            .map(op)?;
        self.dirty = true;
        self.modified_glyphs.insert(name.clone());
        self.rebuild_entry(glyph_index);
        self.realign_after_edit(&name);
        Some(result)
    }

    /// After any glyph edit: re-place anchor-locked components — the
    /// edited glyph's own (its anchors may have moved; its own
    /// anchors seed, the open-glyph behavior) and every composite
    /// that places it, so accents follow their base live.
    fn realign_after_edit(&mut self, edited: &str) {
        use runebender_core::composites as comp;
        let mut targets: Vec<(String, bool)> = vec![(edited.to_string(), true)];
        for user in comp::composites_using(&self.font, edited) {
            if user != edited {
                targets.push((user, false));
            }
        }
        for (name, seed_own) in targets {
            let Some(glyph) = self.font.get_glyph(name.as_str()) else {
                continue;
            };
            if glyph.components.is_empty() {
                continue;
            }
            let mut copy = glyph.clone();
            if comp::realign_glyph(&self.font, &mut copy, seed_own) {
                if let Some(slot) = self.font.default_layer_mut().get_glyph_mut(name.as_str()) {
                    *slot = copy;
                }
                self.modified_glyphs.insert(name.clone());
                self.dirty = true;
                if let Some(&i) = self.name_map.get(&name) {
                    self.rebuild_entry(i);
                }
            }
        }
    }

    /// Rebuild every cache from the norad font (glyph added or
    /// removed); bookkeeping fields survive.
    fn refresh_from_font(&mut self) {
        let font = std::mem::replace(&mut self.font, norad::Font::new());
        let mut fresh = Self::from_font(font, self.source_path.clone());
        // The glyph list has been rebuilt: anything cached against it
        // (the grid's order, for one) has to notice.
        fresh.revision = self.revision.wrapping_add(1);
        fresh.dirty = self.dirty;
        fresh.kerning_dirty = self.kerning_dirty;
        fresh.modified_glyphs = std::mem::take(&mut self.modified_glyphs);
        fresh.glif_paths = std::mem::take(&mut self.glif_paths);
        *self = fresh;
    }

    /// Add an empty glyph. Returns its index in the sorted list.
    fn add_glyph(&mut self, name: &str, width: f64) -> Option<usize> {
        if self.name_map.contains_key(name) {
            return None;
        }
        let mut glyph = norad::Glyph::new(name);
        glyph.width = width;
        self.font.default_layer_mut().insert_glyph(glyph);
        self.dirty = true;
        self.modified_glyphs.insert(name.to_string());
        self.refresh_from_font();
        self.name_map.get(name).copied()
    }

    /// Remove a glyph outright.
    fn remove_glyph(&mut self, name: &str) -> bool {
        if self.font.default_layer_mut().remove_glyph(name).is_none() {
            return false;
        }
        self.dirty = true;
        self.modified_glyphs.remove(name);
        self.refresh_from_font();
        true
    }

    fn load(path: &std::path::Path) -> Result<Self, norad::error::FontLoadError> {
        let font = norad::Font::load(path)?;
        Ok(Self::from_font(font, path.to_path_buf()))
    }

    /// Build the model from an already-assembled font (in-memory
    /// hosts: web builds, embedded demo data).
    fn from_font(font: norad::Font, source_path: PathBuf) -> Self {
        let info = &font.font_info;
        let units_per_em = info.units_per_em.map(|v| v.as_f64()).unwrap_or(1000.0);
        let ascender = info.ascender.unwrap_or(units_per_em * 0.8);
        let descender = info.descender.unwrap_or(-(units_per_em * 0.2));
        let x_height = info.x_height;
        let cap_height = info.cap_height;

        let mut glyphs: Vec<GlyphEntry> = font
            .default_layer()
            .iter()
            .map(|glyph| {
                let path = Arc::new(glyph_path::glyph_to_bezpath(glyph, &font));
                GlyphEntry {
                    name: glyph.name().to_string().into(),
                    codepoint: glyph.codepoints.iter().next(),
                    ink: {
                        use kurbo::Shape as _;
                        path.bounding_box()
                    },
                    path: path.clone(),
                    contour_path: Arc::new(glyph_path::contours_to_bezpath(glyph)),
                    component_path: Arc::new(glyph_path::components_to_bezpath(glyph, &font)),
                    points: Arc::new(extract_points(glyph)),
                    anchors: Arc::new(extract_anchors(glyph)),
                    advance: glyph.width,
                    component_names: Arc::new(
                        glyph
                            .components
                            .iter()
                            .map(|c| c.base.to_string().into())
                            .collect(),
                    ),
                    mark: t::mark_label(glyph).map(SharedString::from),
                }
            })
            .collect();
        // Unicode order, unencoded glyphs after, each group by name.
        glyphs.sort_by(|a, b| match (a.codepoint, b.codepoint) {
            (Some(x), Some(y)) => x.cmp(&y).then_with(|| a.name.cmp(&b.name)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.name.cmp(&b.name),
        });

        let name_map = glyphs
            .iter()
            .enumerate()
            .map(|(i, g)| (g.name.to_string(), i))
            .collect();

        Self {
            font,
            modified_glyphs: std::collections::HashSet::new(),
            glif_paths: std::collections::HashMap::new(),
            kerning_dirty: false,
            name_map,
            source_path,
            units_per_em,
            ascender,
            descender,
            x_height,
            cap_height,
            glyphs,
            revision: 0,
            dirty: false,
        }
    }

    fn rebuild_entry(&mut self, glyph_index: usize) {
        let name = self.glyphs[glyph_index].name.to_string();
        let Some(glyph) = self.font.get_glyph(name.as_str()) else {
            return;
        };
        let glyph_advance = glyph.width;
        let path = Arc::new(glyph_path::glyph_to_bezpath(glyph, &self.font));
        let contour_path = Arc::new(glyph_path::contours_to_bezpath(glyph));
        let component_path = Arc::new(glyph_path::components_to_bezpath(glyph, &self.font));
        let component_names: Arc<Vec<SharedString>> = Arc::new(
            glyph
                .components
                .iter()
                .map(|c| c.base.to_string().into())
                .collect(),
        );
        let points = Arc::new(extract_points(glyph));
        let anchors = Arc::new(extract_anchors(glyph));
        let ink = {
            use kurbo::Shape as _;
            path.bounding_box()
        };
        let entry = &mut self.glyphs[glyph_index];
        entry.ink = ink;
        entry.path = path;
        entry.contour_path = contour_path;
        entry.component_path = component_path;
        entry.component_names = component_names;
        entry.points = points;
        entry.anchors = anchors;
        entry.advance = glyph_advance;
        entry.mark = t::mark_label(glyph).map(SharedString::from);
    }

    /// Clone a glyph's editable state for undo snapshots.
    fn snapshot_contours(&self, glyph_index: usize) -> Option<GlyphSnapshot> {
        let name = self.glyphs[glyph_index].name.to_string();
        self.font.get_glyph(name.as_str()).map(ops::snapshot)
    }

    /// Replace a glyph's editable state (undo/redo) and rebuild caches.
    fn restore_contours(&mut self, glyph_index: usize, snapshot: GlyphSnapshot) {
        self.edit_glyph(glyph_index, |g| ops::restore(g, snapshot));
    }

    fn set_anchor(&mut self, glyph_index: usize, anchor: usize, x: f64, y: f64) {
        self.edit_glyph(glyph_index, |g| {
            if let Some(a) = g.anchors.get_mut(anchor) {
                a.x = x;
                a.y = y;
            }
        });
    }

    fn add_anchor(&mut self, glyph_index: usize, x: f64, y: f64) {
        self.edit_glyph(glyph_index, |g| {
            let n = g.anchors.len();
            let name = norad::Name::new(&format!("anchor.{n}")).ok();
            g.anchors.push(norad::Anchor::new(x, y, name, None, None));
        });
    }

    fn delete_anchor(&mut self, glyph_index: usize, anchor: usize) {
        self.edit_glyph(glyph_index, |g| {
            if anchor < g.anchors.len() {
                g.anchors.remove(anchor);
            }
        });
    }

    /// Set several points at once (multi-point drag).
    fn set_points(&mut self, glyph_index: usize, updates: &ops::PointUpdates) {
        self.edit_glyph(glyph_index, |g| ops::set_points(g, updates));
    }

    /// Start a new open contour at (x, y). Returns its index.
    fn start_hyper_contour(&mut self, glyph_index: usize, x: f64, y: f64) -> Option<usize> {
        self.edit_glyph(glyph_index, |g| {
            runebender_core::glyph_ops::start_hyper_contour(g, x, y)
        })
    }

    fn append_hyper_point(
        &mut self,
        glyph_index: usize,
        contour: usize,
        x: f64,
        y: f64,
        corner: bool,
    ) {
        self.edit_glyph(glyph_index, |g| {
            runebender_core::glyph_ops::append_hyper_point(g, contour, x, y, corner)
        });
    }

    fn close_hyper_contour(&mut self, glyph_index: usize, contour: usize) {
        self.edit_glyph(glyph_index, |g| {
            runebender_core::glyph_ops::close_hyper_contour(g, contour)
        });
    }

    fn start_contour(&mut self, glyph_index: usize, x: f64, y: f64) -> Option<usize> {
        self.edit_glyph(glyph_index, |g| ops::start_contour(g, x, y))
    }

    /// Append a segment to an open contour (pen tool).
    fn append_segment(
        &mut self,
        glyph_index: usize,
        contour: usize,
        controls: Option<((f64, f64), (f64, f64))>,
        x: f64,
        y: f64,
        smooth: bool,
    ) {
        self.edit_glyph(glyph_index, |g| {
            ops::append_segment(g, contour, controls, x, y, smooth)
        });
    }

    /// Close an open contour.
    fn close_contour(
        &mut self,
        glyph_index: usize,
        contour: usize,
        controls: Option<((f64, f64), (f64, f64))>,
    ) {
        self.edit_glyph(glyph_index, |g| ops::close_contour(g, contour, controls));
    }

    /// Delete an unfinished pen contour (single stray point).
    fn remove_contour_if_degenerate(&mut self, glyph_index: usize, contour: usize) {
        self.edit_glyph(glyph_index, |g| {
            ops::remove_contour_if_degenerate(g, contour)
        });
    }

    /// Delete points (see `runebender_core::glyph_ops`).
    fn delete_points(
        &mut self,
        glyph_index: usize,
        selected: &std::collections::HashSet<(usize, usize)>,
    ) -> bool {
        self.edit_glyph(glyph_index, |g| ops::delete_points(g, selected))
            .unwrap_or(false)
    }

    /// Toggle smooth/corner on the selected on-curve points.
    fn toggle_smooth(
        &mut self,
        glyph_index: usize,
        selected: &std::collections::HashSet<(usize, usize)>,
    ) -> bool {
        self.edit_glyph(glyph_index, |g| ops::toggle_smooth(g, selected))
            .unwrap_or(false)
    }

    /// Apply a curve-quality op to the selection or whole glyph.
    fn curve_op(
        &mut self,
        glyph_index: usize,
        selected: &std::collections::HashSet<(usize, usize)>,
        op: CurveOp,
    ) -> bool {
        self.edit_glyph(glyph_index, |g| ops::curve_op(g, selected, op))
            .unwrap_or(false)
    }

    /// Ink bounds of a glyph in design units, None when empty.
    fn ink_bounds(&self, glyph_index: usize) -> Option<kurbo::Rect> {
        use kurbo::Shape;
        let path = &self.glyphs[glyph_index].path;
        if path.elements().is_empty() {
            None
        } else {
            Some(path.bounding_box())
        }
    }

    fn set_advance(&mut self, glyph_index: usize, width: f64) {
        let name = self.glyphs[glyph_index].name.to_string();
        if let Some(glyph) = self.font.default_layer_mut().get_glyph_mut(name.as_str()) {
            glyph.width = width;
            self.dirty = true;
        }
        self.rebuild_metrics(glyph_index);
    }

    /// Shift a glyph's ink horizontally (LSB edits).
    fn shift_ink(&mut self, glyph_index: usize, dx: f64) {
        self.edit_glyph(glyph_index, |g| ops::shift_ink(g, dx));
    }

    fn rebuild_metrics(&mut self, glyph_index: usize) {
        let name = self.glyphs[glyph_index].name.to_string();
        if let Some(glyph) = self.font.get_glyph(name.as_str()) {
            self.glyphs[glyph_index].advance = glyph.width;
        }
    }

    /// Keep the sibling handle collinear through a smooth point.

    /// Kerning between two glyphs with UFO group fallback.

    /// Set an exception-level (glyph-to-glyph) kern pair.

    /// Replace a glyph's components with their resolved contours.
    fn decompose(&mut self, glyph_index: usize) -> bool {
        let name = self.glyphs[glyph_index].name.to_string();
        let Some(glyph) = self.font.get_glyph(name.as_str()) else {
            return false;
        };
        if glyph.components.is_empty() {
            return false;
        }
        let resolved = ops::resolved_component_contours(&self.font, glyph);
        self.edit_glyph(glyph_index, |g| {
            g.contours.extend(resolved);
            g.components.clear();
        });
        true
    }

    /// Contours that contain any selected point; all contours when
    /// the selection is empty.
    fn contours_for_copy(
        &self,
        glyph_index: usize,
        selected: &std::collections::HashSet<(usize, usize)>,
    ) -> Vec<norad::Contour> {
        let name = self.glyphs[glyph_index].name.to_string();
        let Some(glyph) = self.font.get_glyph(name.as_str()) else {
            return Vec::new();
        };
        if selected.is_empty() {
            return glyph.contours.clone();
        }
        glyph
            .contours
            .iter()
            .enumerate()
            .filter(|(ci, _)| selected.iter().any(|(c, _)| c == ci))
            .map(|(_, c)| c.clone())
            .collect()
    }

    fn paste_contours(&mut self, glyph_index: usize, contours: &[norad::Contour]) {
        if contours.is_empty() {
            return;
        }
        let name = self.glyphs[glyph_index].name.to_string();
        if let Some(glyph) = self.font.default_layer_mut().get_glyph_mut(name.as_str()) {
            glyph.contours.extend(contours.iter().cloned());
            self.dirty = true;
        }
        self.rebuild_entry(glyph_index);
    }

    /// Union all contours (remove overlap). Returns false when
    /// nothing changed.
    fn remove_overlap(&mut self, glyph_index: usize) -> bool {
        let name = self.glyphs[glyph_index].name.to_string();
        let Some(unioned) = self
            .font
            .get_glyph(name.as_str())
            .and_then(ops::remove_overlap)
        else {
            return false;
        };
        self.edit_glyph(glyph_index, |g| g.contours = unioned);
        true
    }

    /// Insert a rectangle or ellipse contour spanning `rect`.
    fn add_shape_contour(&mut self, glyph_index: usize, rect: kurbo::Rect, ellipse: bool) {
        self.edit_glyph(glyph_index, |g| ops::add_shape_contour(g, rect, ellipse));
    }

    fn save(&mut self) -> Result<(), norad::error::FontWriteError> {
        self.font.save(&self.source_path)?;
        self.dirty = false;
        self.modified_glyphs.clear();
        self.kerning_dirty = false;
        Ok(())
    }
}

// ============================================================================
// PROJECT (designspace or single UFO)
// ============================================================================

/// One designspace axis, in design coordinates.
#[derive(Clone)]
struct AxisInfo {
    name: String,
    tag: SharedString,
    min: f64,
    default: f64,
    max: f64,
}

/// An open project: one or more master UFOs, optionally tied together
/// by a designspace document.
struct Project {
    masters: Vec<FontModel>,
    active: usize,
    /// Style names for the master switcher, one per master.
    master_names: Vec<SharedString>,
    axes: Vec<AxisInfo>,
    /// Normalized (-1..1) location of each master, by axis name.
    master_locations: Vec<runebender_core::var_model::Location>,
    model: Option<runebender_core::var_model::VariationModel>,
    /// Current preview location, normalized, by axis name.
    location: runebender_core::var_model::Location,
    /// Per-glyph master point-compatibility (designspaces only).
    compat: std::collections::HashMap<String, bool>,
    /// What fontc compiles on File > Export: the designspace the
    /// project was opened from, or the single UFO. `None` until the
    /// project has a home on disk (File > New before Save As).
    export_source: Option<PathBuf>,
    /// Named designspace instances: style name and normalized
    /// location, for the Instances rows under the axis sliders.
    instances: Vec<(SharedString, runebender_core::var_model::Location)>,
    /// The loaded designspace document, kept so instance (and later
    /// axis) edits can be written back. None for single-UFO projects.
    ds_doc: Option<norad::designspace::DesignSpaceDocument>,
    /// Instance edits not yet written to the designspace file.
    ds_dirty: bool,
    /// Sparse "brace" sources: per-glyph intermediate masters living
    /// in a named layer of a master UFO at their own location
    /// (designspace sources with a `layer` attribute).
    brace: Vec<BraceSource>,
}

/// One sparse intermediate source (a Glyphs brace layer).
struct BraceSource {
    /// Index into `masters`: the UFO holding the layer.
    master: usize,
    /// The UFO layer name (Glyphs writes "{500}").
    layer: String,
    /// Normalized location.
    location: runebender_core::var_model::Location,
}

/// Which Glyphs form a path names, if either.
#[derive(Clone, Copy, PartialEq)]
enum GlyphsSource {
    File,
    Package,
    Neither,
}

/// Read a `.glyphspackage` into the entries the importer wants: paths
/// relative to the package root, so `glyphs/A.glyph` stays
/// `glyphs/A.glyph`.
fn read_glyphspackage(
    root: &std::path::Path,
) -> Result<std::collections::HashMap<String, String>, String> {
    fn walk(
        dir: &std::path::Path,
        root: &std::path::Path,
        out: &mut std::collections::HashMap<String, String>,
    ) -> Result<(), String> {
        for entry in std::fs::read_dir(dir).map_err(|e| format!("{e}"))? {
            let path = entry.map_err(|e| format!("{e}"))?.path();
            if path.is_dir() {
                walk(&path, root, out)?;
            } else if let Ok(text) = std::fs::read_to_string(&path) {
                // Anything that is not UTF-8 is not part of the
                // source; skip it rather than failing the open.
                let rel = path
                    .strip_prefix(root)
                    .map_err(|e| format!("{e}"))?
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(rel, text);
            }
        }
        Ok(())
    }
    let mut out = std::collections::HashMap::new();
    walk(root, root, &mut out)?;
    if out.is_empty() {
        return Err(format!("{} is empty", root.display()));
    }
    Ok(out)
}

impl Project {
    /// The master sitting exactly at `location`, if any. Landing on a
    /// master is a master switch, not an interpolation: the web treats
    /// it that way so the outline stays editable.
    fn master_at_location(&self) -> Option<usize> {
        if self.axes.is_empty() {
            return None;
        }
        self.master_locations.iter().position(|there| {
            self.axes.iter().all(|axis| {
                let a = there.get(&axis.name).copied().unwrap_or(0.0);
                let b = self.location.get(&axis.name).copied().unwrap_or(0.0);
                (a - b).abs() < 1e-6
            })
        })
    }

    /// True while the sliders sit between masters: what the canvas
    /// shows is an interpolated instance, and nothing there is
    /// editable.
    fn showing_instance(&self) -> bool {
        self.model.is_some() && !self.axes.is_empty() && self.master_at_location().is_none()
    }

    /// Put `location` back on a master, for a master switch.
    fn snap_location_to_master(&mut self, master: usize) {
        if let Some(there) = self.master_locations.get(master) {
            self.location = there.clone();
        }
    }

    /// File → New Font: one master from the GF-shaped template. The
    /// source path is where Save will write; Save As picks it.
    fn new_font(path: PathBuf) -> Self {
        let font = runebender_core::new_font::new_font("Untitled", "Regular", 400);
        let mut model = FontModel::from_font(font, path);
        model.dirty = true;
        let mut project = Self {
            masters: vec![model],
            active: 0,
            master_names: vec!["Regular".into()],
            axes: Vec::new(),
            master_locations: Vec::new(),
            model: None,
            location: runebender_core::var_model::Location::new(),
            compat: std::collections::HashMap::new(),
            export_source: None,
            instances: Vec::new(),
            ds_doc: None,
            ds_dirty: false,
            brace: Vec::new(),
        };
        project.compute_compat();
        project
    }

    fn load(path: &std::path::Path) -> Result<Self, String> {
        let mut project = Self::load_inner(path)?;
        if project.export_source.is_none() {
            project.export_source = Some(path.to_path_buf());
        }
        project.compute_compat();
        Ok(project)
    }

    fn load_inner(path: &std::path::Path) -> Result<Self, String> {
        let glyphs_ext = path.extension().and_then(|e| e.to_str()).map(|e| {
            if e.eq_ignore_ascii_case("glyphspackage") {
                GlyphsSource::Package
            } else if e.eq_ignore_ascii_case("glyphs") {
                GlyphsSource::File
            } else {
                GlyphsSource::Neither
            }
        });
        if let Some(kind @ (GlyphsSource::File | GlyphsSource::Package)) = glyphs_ext {
            // Convert the Glyphs source to UFO + designspace files in
            // a sibling directory, then open the converted project.
            let result = match kind {
                GlyphsSource::Package => {
                    let entries = read_glyphspackage(path)?;
                    runebender_core::glyphs_import::glyphs_package_to_ufo_files(&entries)?
                }
                _ => {
                    let text = std::fs::read_to_string(path).map_err(|e| format!("{e}"))?;
                    runebender_core::glyphs_import::glyphs_to_ufo_files(&text)?
                }
            };
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "glyphs-import".into());
            let out_dir = path
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join(format!("{stem}-ufo"));
            let mut designspace: Option<std::path::PathBuf> = None;
            let mut first_ufo: Option<std::path::PathBuf> = None;
            for file in &result.files {
                let target = out_dir.join(&file.path);
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| format!("{e}"))?;
                }
                std::fs::write(&target, &file.text).map_err(|e| format!("{e}"))?;
                if file.path.ends_with(".designspace") {
                    designspace = Some(target);
                } else if first_ufo.is_none() && file.path.ends_with("fontinfo.plist") {
                    first_ufo = target.parent().map(|p| p.to_path_buf());
                }
            }
            let open = designspace
                .or(first_ufo)
                .ok_or_else(|| "conversion produced no font".to_string())?;
            // Export compiles the converted files, not the .glyphs.
            let mut project = Self::load_inner(&open)?;
            project.export_source = Some(open);
            return Ok(project);
        }
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("ttf") || e.eq_ignore_ascii_case("otf"))
        {
            // A compiled font opens as an editable in-memory UFO.
            // Save writes that UFO next to the binary — never over
            // it — and Export compiles from the UFO.
            let font = import_binary_font(path)?;
            let name: SharedString = font
                .font_info
                .style_name
                .clone()
                .unwrap_or_else(|| "Regular".into())
                .into();
            let ufo_path = path.with_extension("ufo");
            let mut model = FontModel::from_font(font, ufo_path.clone());
            model.dirty = true;
            let mut project = Self {
                masters: vec![model],
                active: 0,
                master_names: vec![name],
                axes: Vec::new(),
                master_locations: Vec::new(),
                model: None,
                location: runebender_core::var_model::Location::new(),
                compat: std::collections::HashMap::new(),
                export_source: Some(ufo_path),
                instances: Vec::new(),
                ds_doc: None,
                ds_dirty: false,
                brace: Vec::new(),
            };
            project.compute_compat();
            return Ok(project);
        }
        if path.extension().is_some_and(|e| e == "designspace") {
            let doc = norad::designspace::DesignSpaceDocument::load(path)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            let dir = path
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .to_path_buf();
            return Self::from_designspace(doc, move |filename| {
                let ufo_path = dir.join(filename);
                FontModel::load(&ufo_path).map_err(|e| format!("{}: {e}", ufo_path.display()))
            });
        }
        {
            let model = FontModel::load(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let name: SharedString = model
                .font
                .font_info
                .style_name
                .clone()
                .unwrap_or_else(|| "Regular".into())
                .into();
            Ok(Self {
                masters: vec![model],
                active: 0,
                master_names: vec![name],
                axes: Vec::new(),
                master_locations: Vec::new(),
                model: None,
                location: runebender_core::var_model::Location::new(),
                compat: std::collections::HashMap::new(),
                export_source: None,
                instances: Vec::new(),
                ds_doc: None,
                ds_dirty: false,
                brace: Vec::new(),
            })
        }
    }

    /// Assemble a designspace project; `load_master` maps a source
    /// filename to its font model (filesystem or in-memory host).
    fn from_designspace(
        doc: norad::designspace::DesignSpaceDocument,
        mut load_master: impl FnMut(&str) -> Result<FontModel, String>,
    ) -> Result<Self, String> {
        {
            let mut seen = std::collections::HashSet::new();
            let mut masters = Vec::new();
            let mut master_names = Vec::new();
            let mut default_index = 0usize;
            // The source whose location matches every axis default is
            // the default master; open on that one.
            let defaults: std::collections::HashMap<&str, f32> = doc
                .axes
                .iter()
                .map(|a| (a.name.as_str(), a.default))
                .collect();
            // Axis metadata (design coordinates; avar maps ignored
            // for now, which matches sources that don't use them).
            let axes: Vec<AxisInfo> = doc
                .axes
                .iter()
                .map(|a| AxisInfo {
                    name: a.name.clone(),
                    tag: a.tag.clone().into(),
                    min: a.minimum.unwrap_or(a.default) as f64,
                    default: a.default as f64,
                    max: a.maximum.unwrap_or(a.default) as f64,
                })
                .collect();
            let mut master_locations = Vec::new();
            let mut master_files: Vec<String> = Vec::new();
            // Sparse sources (a `layer` attribute) are brace layers:
            // per-glyph intermediates, resolved after the masters.
            let normalize_loc = |dims: &[norad::designspace::Dimension]| {
                let mut location = runebender_core::var_model::Location::new();
                for axis in &axes {
                    let raw = dims
                        .iter()
                        .find(|d| d.name == axis.name)
                        .and_then(|d| d.xvalue.or(d.uservalue))
                        .map(|v| v as f64)
                        .unwrap_or(axis.default);
                    location.insert(
                        axis.name.clone(),
                        runebender_core::var_model::normalize_value(
                            raw,
                            axis.min,
                            axis.default,
                            axis.max,
                        ),
                    );
                }
                location
            };
            let mut layer_sources: Vec<(String, String, runebender_core::var_model::Location)> =
                Vec::new();
            for source in &doc.sources {
                if let Some(layer) = &source.layer {
                    layer_sources.push((
                        source.filename.clone(),
                        layer.clone(),
                        normalize_loc(&source.location),
                    ));
                    continue;
                }
                if !seen.insert(source.filename.clone()) {
                    continue; // duplicate full-source entries
                }
                let model = load_master(&source.filename)?;
                let is_default = source.location.iter().all(|d| {
                    let value = d.xvalue.or(d.uservalue).unwrap_or(0.0);
                    defaults
                        .get(d.name.as_str())
                        .is_some_and(|v| (*v - value).abs() < f32::EPSILON)
                });
                if is_default {
                    default_index = masters.len();
                }
                // Normalized location for the interpolation model.
                let mut location = runebender_core::var_model::Location::new();
                for axis in &axes {
                    let raw = source
                        .location
                        .iter()
                        .find(|d| d.name == axis.name)
                        .and_then(|d| d.xvalue.or(d.uservalue))
                        .map(|v| v as f64)
                        .unwrap_or(axis.default);
                    location.insert(
                        axis.name.clone(),
                        runebender_core::var_model::normalize_value(
                            raw,
                            axis.min,
                            axis.default,
                            axis.max,
                        ),
                    );
                }
                master_locations.push(location);
                let name = source
                    .stylename
                    .clone()
                    .unwrap_or_else(|| source.filename.clone());
                masters.push(model);
                master_names.push(name.into());
                master_files.push(source.filename.clone());
            }
            if masters.is_empty() {
                return Err("designspace has no sources".into());
            }
            let model = (masters.len() > 1)
                .then(|| runebender_core::var_model::VariationModel::new(&master_locations));
            let location = axes.iter().map(|a| (a.name.clone(), 0.0)).collect();
            let brace: Vec<BraceSource> = layer_sources
                .into_iter()
                .filter_map(|(filename, layer, location)| {
                    let master = master_files.iter().position(|f| *f == filename)?;
                    Some(BraceSource {
                        master,
                        layer,
                        location,
                    })
                })
                .collect();
            let mut project = Self {
                active: default_index,
                masters,
                master_names,
                axes,
                master_locations,
                model,
                location,
                compat: std::collections::HashMap::new(),
                export_source: None,
                instances: Vec::new(),
                ds_doc: Some(doc),
                ds_dirty: false,
                brace,
            };
            project.refresh_instances_from_doc();
            Ok(project)
        }
    }

    /// Assemble a project from a fetched workspace (web host).
    /// Returns the project plus per-master UFO path prefixes
    /// (workspace-root relative), aligned with `masters`.
    #[cfg(target_family = "wasm")]
    fn from_fetched(fetched: &web_host::FetchedWorkspace) -> Result<(Self, Vec<String>), String> {
        use std::cell::RefCell;
        let prefixes: RefCell<Vec<String>> = RefCell::new(Vec::new());
        let build_master = |prefix: String| -> Result<FontModel, String> {
            let files: Vec<(&str, &[u8])> = fetched
                .files
                .iter()
                .filter_map(|(path, bytes)| {
                    path.strip_prefix(&prefix)
                        .map(|rel| (rel, bytes.as_slice()))
                })
                .collect();
            if files.is_empty() {
                return Err(format!("no files under {prefix}"));
            }
            let ufo =
                runebender_core::font_memory::ufo_from_files(files.iter().map(|(p, b)| (*p, *b)))?;
            let mut model =
                FontModel::from_font(ufo.font, PathBuf::from(prefix.trim_end_matches('/')));
            model.glif_paths = ufo.glif_paths;
            prefixes.borrow_mut().push(prefix);
            Ok(model)
        };

        let project = if let Some(ds_text) = &fetched.designspace_text {
            let doc = runebender_core::font_memory::designspace_from_str(ds_text)?;
            let ds_dir = match fetched.entry.rfind('/') {
                Some(i) => &fetched.entry[..=i],
                None => "",
            };
            Self::from_designspace(doc, |filename| build_master(format!("{ds_dir}{filename}/")))?
        } else {
            // Bare UFO entry.
            let model = build_master(format!("{}/", fetched.entry.trim_end_matches('/')))?;
            let name: SharedString = model
                .font
                .font_info
                .style_name
                .clone()
                .unwrap_or_else(|| "Regular".into())
                .into();
            Self {
                masters: vec![model],
                active: 0,
                master_names: vec![name],
                axes: Vec::new(),
                master_locations: Vec::new(),
                model: None,
                location: runebender_core::var_model::Location::new(),
                compat: std::collections::HashMap::new(),
                export_source: None,
                instances: Vec::new(),
                ds_doc: None,
                ds_dirty: false,
                brace: Vec::new(),
            }
        };
        let mut project = project;
        project.compute_compat();
        Ok((project, prefixes.into_inner()))
    }

    /// The embedded demo project for hosts without a filesystem
    /// (web builds): the Virtua Grotesk designspace and both master
    /// UFOs compiled into the binary.
    #[cfg(target_family = "wasm")]
    fn demo_embedded() -> Result<Self, String> {
        static DEMO: include_dir::Dir<'_> =
            include_dir::include_dir!("$CARGO_MANIFEST_DIR/../runebender-web/assets/test-fonts");
        let ds_text = DEMO
            .get_file("VirtuaGrotesk.designspace")
            .and_then(|f| f.contents_utf8())
            .ok_or("embedded designspace missing")?;
        let doc = runebender_core::font_memory::designspace_from_str(ds_text)?;
        let mut project = Self::from_designspace(doc, |filename| {
            let ufo = DEMO
                .get_dir(filename)
                .ok_or_else(|| format!("embedded UFO missing: {filename}"))?;
            let mut files: Vec<(String, &[u8])> = Vec::new();
            fn walk<'a>(
                dir: &'a include_dir::Dir<'a>,
                prefix: &str,
                out: &mut Vec<(String, &'a [u8])>,
            ) {
                for file in dir.files() {
                    let name = file.path().file_name().unwrap().to_string_lossy();
                    out.push((format!("{prefix}{name}"), file.contents()));
                }
                for sub in dir.dirs() {
                    let name = sub.path().file_name().unwrap().to_string_lossy();
                    walk(sub, &format!("{prefix}{name}/"), out);
                }
            }
            walk(ufo, "", &mut files);
            let font = runebender_core::font_memory::font_from_files(
                files.iter().map(|(p, b)| (p.as_str(), *b)),
            )?;
            Ok(FontModel::from_font(font, PathBuf::from(filename)))
        })?;
        project.compute_compat();
        Ok(project)
    }

    /// Structural signature used for interpolation compatibility:
    /// per contour, the ordered list of point types.
    fn glyph_signature(font: &FontModel, name: &str) -> Option<Vec<Vec<norad::PointType>>> {
        font.font.get_glyph(name).map(ops::glyph_signature)
    }

    /// Why a glyph does not interpolate: the first master pair whose
    /// structure disagrees, with contour and point counts. None when
    /// compatible or single-master.
    fn compat_detail(&self, name: &str) -> Option<String> {
        if self.masters.len() < 2 || self.compat.get(name).copied().unwrap_or(true) {
            return None;
        }
        let first_sig = Self::glyph_signature(&self.masters[0], name);
        let first_name = &self.master_names[0];
        let describe = |sig: &Option<Vec<Vec<norad::PointType>>>| match sig {
            None => "missing".to_string(),
            Some(contours) => {
                let points: usize = contours.iter().map(|c| c.len()).sum();
                format!("{}c · {}pt", contours.len(), points)
            }
        };
        for (master, master_name) in self.masters.iter().zip(&self.master_names).skip(1) {
            let sig = Self::glyph_signature(master, name);
            if sig == first_sig {
                continue;
            }
            return Some(format!(
                "{first_name} {} · {master_name} {}",
                describe(&first_sig),
                describe(&sig),
            ));
        }
        // Same counts everywhere: the disagreement is point types
        // (a curve against a line somewhere).
        Some("point types differ between masters".into())
    }

    /// Rebuild the Instances display rows (name + normalized
    /// location) from the designspace document.
    fn refresh_instances_from_doc(&mut self) {
        let Some(doc) = self.ds_doc.as_ref() else {
            return;
        };
        self.instances = doc
            .instances
            .iter()
            .map(|inst| {
                let name: SharedString = inst
                    .stylename
                    .clone()
                    .or_else(|| inst.name.clone())
                    .unwrap_or_else(|| "Instance".into())
                    .into();
                let mut location = runebender_core::var_model::Location::new();
                for axis in &self.axes {
                    let raw = inst
                        .location
                        .iter()
                        .find(|d| d.name == axis.name)
                        .and_then(|d| d.xvalue.or(d.uservalue))
                        .map(|v| v as f64)
                        .unwrap_or(axis.default);
                    location.insert(
                        axis.name.clone(),
                        runebender_core::var_model::normalize_value(
                            raw,
                            axis.min,
                            axis.default,
                            axis.max,
                        ),
                    );
                }
                (name, location)
            })
            .collect();
    }

    /// Check one glyph's compatibility across all masters.
    fn check_compat(&self, name: &str) -> bool {
        let mut signatures = self.masters.iter().map(|m| Self::glyph_signature(m, name));
        let Some(first) = signatures.next().flatten() else {
            return false;
        };
        signatures.all(|s| s.as_ref() == Some(&first))
    }

    /// Recompute the whole compatibility map (load / reload).
    fn compute_compat(&mut self) {
        self.compat.clear();
        if self.masters.len() < 2 {
            return;
        }
        let names: Vec<String> = self.masters[self.active]
            .glyphs
            .iter()
            .map(|g| g.name.to_string())
            .collect();
        for name in names {
            let ok = self.check_compat(&name);
            self.compat.insert(name, ok);
        }
    }

    /// Recheck one glyph after editing.
    fn recheck_compat(&mut self, name: &str) {
        if self.masters.len() < 2 {
            return;
        }
        let ok = self.check_compat(name);
        self.compat.insert(name.to_string(), ok);
    }

    /// Interpolated outline + advance of a glyph at the current
    /// location. None when at the default location, when masters are
    /// point-incompatible, or when there is no model.
    /// The glyph rebuilt from every source EXCEPT the active
    /// master, evaluated at the active master's own location —
    /// Glyphs' Re-Interpolate, for repairing one broken master from
    /// the others. With one other source this is a straight copy.
    fn reinterpolated_from_others(&self, glyph_name: &str) -> Result<norad::Glyph, String> {
        let flatten = |glyph: &norad::Glyph| {
            let mut v = vec![glyph.width];
            for contour in &glyph.contours {
                for p in &contour.points {
                    v.push(p.x);
                    v.push(p.y);
                }
            }
            v
        };
        let mut values: Vec<Vec<f64>> = Vec::new();
        let mut locations: Vec<runebender_core::var_model::Location> = Vec::new();
        let mut template: Option<norad::Glyph> = None;
        for (mi, master) in self.masters.iter().enumerate() {
            if mi == self.active {
                continue;
            }
            let Some(glyph) = master.font.get_glyph(glyph_name) else {
                continue;
            };
            values.push(flatten(glyph));
            locations.push(self.master_locations[mi].clone());
            if template.is_none() {
                template = Some(glyph.clone());
            }
        }
        for b in &self.brace {
            if b.master == self.active {
                continue;
            }
            let Some(glyph) = self
                .masters
                .get(b.master)
                .and_then(|m| m.font.layers.get(&b.layer))
                .and_then(|l| l.get_glyph(glyph_name))
            else {
                continue;
            };
            values.push(flatten(glyph));
            locations.push(b.location.clone());
        }
        let Some(mut template) = template else {
            return Err("No other master holds this glyph".into());
        };
        let len = values[0].len();
        if values.iter().any(|v| v.len() != len) {
            return Err("Other masters are not point-compatible".into());
        }
        let out = if values.len() == 1 {
            values.remove(0)
        } else {
            runebender_core::var_model::VariationModel::new(&locations)
                .interpolate(&values, &self.master_locations[self.active])
        };
        let mut it = out.iter().copied();
        template.width = it.next().unwrap_or(template.width);
        for contour in template.contours.iter_mut() {
            for p in contour.points.iter_mut() {
                p.x = it.next().unwrap_or(p.x);
                p.y = it.next().unwrap_or(p.y);
            }
        }
        Ok(template)
    }

    fn interpolated_glyph(&self, glyph_name: &str) -> Option<(BezPath, f64)> {
        let glyph = self.interpolated_norad_glyph(glyph_name)?;
        let advance = glyph.width;
        let base = &self.masters[self.active];
        Some((glyph_path::glyph_to_bezpath(&glyph, &base.font), advance))
    }

    /// The interpolation at the current location as a norad glyph
    /// (point structure kept): the working form for the ghost, the
    /// strip, and for freezing into a brace layer.
    fn interpolated_norad_glyph(&self, glyph_name: &str) -> Option<norad::Glyph> {
        if self.location.values().all(|v| v.abs() < 1e-9) {
            return None;
        }
        self.interpolated_at(glyph_name, &self.location)
    }

    /// The interpolation at an arbitrary normalized location — the
    /// default location included, where it returns the default
    /// master's own coordinates (trajectory sampling needs the whole
    /// axis, ends included).
    fn interpolated_at(
        &self,
        glyph_name: &str,
        location: &runebender_core::var_model::Location,
    ) -> Option<norad::Glyph> {
        self.model.as_ref()?;
        let flatten = |glyph: &norad::Glyph| {
            let mut v = vec![glyph.width];
            for contour in &glyph.contours {
                for p in &contour.points {
                    v.push(p.x);
                    v.push(p.y);
                }
            }
            v
        };
        // Flatten [advance, x0, y0, x1, y1, ...] per master.
        let mut values: Vec<Vec<f64>> = Vec::with_capacity(self.masters.len());
        for master in &self.masters {
            values.push(flatten(master.font.get_glyph(glyph_name)?));
        }
        // Brace layers holding this glyph join the master set: the
        // model grows their locations, per glyph (Glyphs' intermediate
        // layers).
        let mut brace_locations: Vec<runebender_core::var_model::Location> = Vec::new();
        for b in &self.brace {
            let Some(glyph) = self
                .masters
                .get(b.master)
                .and_then(|m| m.font.layers.get(&b.layer))
                .and_then(|l| l.get_glyph(glyph_name))
            else {
                continue;
            };
            values.push(flatten(glyph));
            brace_locations.push(b.location.clone());
        }
        let len = values[0].len();
        if values.iter().any(|v| v.len() != len) {
            return None; // point-incompatible sources
        }
        let out = if brace_locations.is_empty() {
            self.model.as_ref()?.interpolate(&values, location)
        } else {
            let mut locations = self.master_locations.clone();
            locations.extend(brace_locations);
            runebender_core::var_model::VariationModel::new(&locations)
                .interpolate(&values, location)
        };
        // Rebuild on the default master's structure.
        let base = &self.masters[self.active];
        let mut glyph = base.font.get_glyph(glyph_name)?.clone();
        let mut it = out.iter().copied();
        let advance = it.next()?;
        for contour in glyph.contours.iter_mut() {
            for p in contour.points.iter_mut() {
                p.x = it.next()?;
                p.y = it.next()?;
            }
        }
        glyph.width = advance;
        // HOI: nodes with an intermediate point follow their exact
        // quadratic, overriding the piecewise answer the baked brace
        // layers gave the model — the bake stays for compilers, the
        // preview is exact.
        if let (Some(axis), Some((lo, hi))) = (self.axes.first(), self.axis_end_masters()) {
            let curves = self.masters[lo]
                .font
                .get_glyph(glyph_name)
                .map(read_hoi_intermediates)
                .unwrap_or_default();
            if !curves.is_empty() {
                let normalized = location.get(&axis.name).copied().unwrap_or(0.0);
                let design = runebender_core::var_model::denormalize_value(
                    normalized,
                    axis.min,
                    axis.default,
                    axis.max,
                );
                let t01 = ((design - axis.min) / (axis.max - axis.min)).clamp(0.0, 1.0);
                let (a_glyph, b_glyph) = (
                    self.masters[lo].font.get_glyph(glyph_name),
                    self.masters[hi].font.get_glyph(glyph_name),
                );
                if let (Some(a_glyph), Some(b_glyph)) = (a_glyph, b_glyph) {
                    for (&(ci, pi), &q) in &curves {
                        let (Some(pa), Some(pb)) = (
                            a_glyph.contours.get(ci).and_then(|c| c.points.get(pi)),
                            b_glyph.contours.get(ci).and_then(|c| c.points.get(pi)),
                        ) else {
                            continue;
                        };
                        let pos = hoi_quad_at((pa.x, pa.y), (pb.x, pb.y), q, t01);
                        if let Some(point) = glyph
                            .contours
                            .get_mut(ci)
                            .and_then(|c| c.points.get_mut(pi))
                        {
                            point.x = pos.0;
                            point.y = pos.1;
                        }
                    }
                }
            }
        }
        Some(glyph)
    }

    /// The masters at the low and high end of the first axis (by
    /// normalized location), for HOI endpoints.
    fn axis_end_masters(&self) -> Option<(usize, usize)> {
        let axis = self.axes.first()?;
        if self.masters.len() < 2 {
            return None;
        }
        let value = |i: usize| {
            self.master_locations
                .get(i)
                .and_then(|l| l.get(&axis.name).copied())
                .unwrap_or(0.0)
        };
        let lo = (0..self.masters.len()).min_by(|&a, &b| value(a).total_cmp(&value(b)))?;
        let hi = (0..self.masters.len()).max_by(|&a, &b| value(a).total_cmp(&value(b)))?;
        (lo != hi).then_some((lo, hi))
    }

    /// Sample every point's position at `steps + 1` equal stops
    /// along the first axis (min to max), through the same per-glyph
    /// model the ghost uses — brace layers bend the trajectories.
    /// Outer index: point (flattened contour order); inner: stop.
    fn trajectory_samples(&self, glyph_name: &str, steps: usize) -> Option<Vec<Vec<kurbo::Point>>> {
        self.model.as_ref()?;
        let axis = self.axes.first()?;
        let mut per_point: Vec<Vec<kurbo::Point>> = Vec::new();
        for step in 0..=steps {
            let t = step as f64 / steps as f64;
            let design = axis.min + (axis.max - axis.min) * t;
            let mut location = self.location.clone();
            location.insert(
                axis.name.clone(),
                runebender_core::var_model::normalize_value(
                    design,
                    axis.min,
                    axis.default,
                    axis.max,
                ),
            );
            let glyph = self.interpolated_at(glyph_name, &location)?;
            let mut flat = Vec::new();
            for contour in &glyph.contours {
                for p in &contour.points {
                    flat.push(kurbo::Point::new(p.x, p.y));
                }
            }
            if per_point.is_empty() {
                per_point = flat.into_iter().map(|p| vec![p]).collect();
            } else {
                if flat.len() != per_point.len() {
                    return None;
                }
                for (track, p) in per_point.iter_mut().zip(flat) {
                    track.push(p);
                }
            }
        }
        Some(per_point)
    }

    /// The glyph a designspace rule shows at the current preview
    /// location, if any (bracket layers / shape switches). Rules
    /// apply when every condition of any condition set holds; an
    /// empty condition set always holds.
    fn rule_substitute(&self, glyph_name: &str) -> Option<String> {
        let doc = self.ds_doc.as_ref()?;
        // Current location in design coordinates.
        let design: std::collections::HashMap<&str, f64> = self
            .axes
            .iter()
            .map(|axis| {
                let normalized = self.location.get(&axis.name).copied().unwrap_or(0.0);
                (
                    axis.name.as_str(),
                    runebender_core::var_model::denormalize_value(
                        normalized,
                        axis.min,
                        axis.default,
                        axis.max,
                    ),
                )
            })
            .collect();
        for rule in &doc.rules.rules {
            let applies = rule.condition_sets.is_empty()
                || rule.condition_sets.iter().any(|set| {
                    set.conditions.iter().all(|c| {
                        let Some(&value) = design.get(c.name.as_str()) else {
                            return false;
                        };
                        c.minimum.is_none_or(|min| value >= min as f64 - 1e-6)
                            && c.maximum.is_none_or(|max| value <= max as f64 + 1e-6)
                    })
                });
            if !applies {
                continue;
            }
            for sub in &rule.substitutions {
                if sub.name.as_str() == glyph_name {
                    return Some(sub.with.to_string());
                }
            }
        }
        None
    }

    fn active_font(&self) -> &FontModel {
        &self.masters[self.active]
    }

    fn active_font_mut(&mut self) -> &mut FontModel {
        &mut self.masters[self.active]
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

/// A shared toolbar icon painted to fit its element, centered with
/// padding. Icon geometry comes from runebender-core (the same icon
/// UFO the web toolbar uses).
fn icon_svg(name: &'static str, color: gpui::Rgba) -> impl IntoElement {
    canvas(
        move |bounds, _, _| bounds,
        move |_, bounds: Bounds<gpui::Pixels>, window, _| {
            let Some(icon) = runebender_core::theme_oklch::toolbar_icons().get(name) else {
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

/// Replace targeted contours with the outline of a stroke of the
/// given width (round joins and caps), the Make Stroke half of
/// Glyphs' Offset Curve. An empty `selected` set targets every
/// contour. Returns false when nothing changed.
fn expand_stroke_contours(
    glyph: &mut norad::Glyph,
    selected: &std::collections::HashSet<usize>,
    width: f64,
) -> bool {
    let style = kurbo::Stroke::new(width);
    let opts = kurbo::StrokeOpts::default();
    let empty = std::collections::HashMap::new();
    let mut out: Vec<norad::Contour> = Vec::new();
    let mut any = false;
    for (ci, contour) in glyph.contours.iter().enumerate() {
        let targeted = selected.is_empty() || selected.contains(&ci);
        if !targeted {
            out.push(contour.clone());
            continue;
        }
        let path = runebender_core::glyph_paths::contour_to_bezpath(contour);
        let stroked = kurbo::stroke(path.elements().iter().copied(), &style, &opts, 0.25);
        // One stroked outline can be several subpaths (a closed
        // skeleton keeps its counter).
        let mut sub = BezPath::new();
        let mut made = false;
        for el in stroked.elements() {
            if matches!(el, PathEl::MoveTo(_)) && !sub.elements().is_empty() {
                if let Some(c) = runebender_core::glyph_ops::bezpath_to_contour(&sub, &empty) {
                    out.push(c);
                    made = true;
                }
                sub = BezPath::new();
            }
            sub.push(*el);
        }
        if !sub.elements().is_empty() {
            if let Some(c) = runebender_core::glyph_ops::bezpath_to_contour(&sub, &empty) {
                out.push(c);
                made = true;
            }
        }
        if made {
            any = true;
        } else {
            out.push(contour.clone());
        }
    }
    if any {
        glyph.contours = out;
    }
    any
}

/// Offset every contour outward (positive `delta`, bolder) or inward
/// (negative, lighter): the whole glyph is unioned with — or cut by —
/// a stroke band of width 2·delta around its own outline, which moves
/// counters the opposite way automatically. The bolder/lighter half
/// of Glyphs' Offset Curve. Returns false when nothing changed.
fn offset_glyph_contours(glyph: &mut norad::Glyph, delta: f64) -> bool {
    if delta == 0.0 || glyph.contours.is_empty() {
        return false;
    }
    let mut combined = BezPath::new();
    let mut band = BezPath::new();
    let style = kurbo::Stroke::new(delta.abs() * 2.0);
    let opts = kurbo::StrokeOpts::default();
    for contour in &glyph.contours {
        let path = runebender_core::glyph_paths::contour_to_bezpath(contour);
        band.extend(
            kurbo::stroke(path.elements().iter().copied(), &style, &opts, 0.25)
                .elements()
                .iter()
                .copied(),
        );
        combined.extend(path.elements().iter().copied());
    }
    let op = if delta > 0.0 {
        linesweeper::BinaryOp::Union
    } else {
        linesweeper::BinaryOp::Difference
    };
    let Ok(result) = linesweeper::binary_op(&combined, &band, linesweeper::FillRule::NonZero, op)
    else {
        return false;
    };
    let smooth_at: std::collections::HashMap<(i64, i64), bool> = glyph
        .contours
        .iter()
        .flat_map(|c| c.points.iter())
        .filter(|p| p.typ != norad::PointType::OffCurve)
        .map(|p| ((p.x.round() as i64, p.y.round() as i64), p.smooth))
        .collect();
    let mut contours: Vec<norad::Contour> = Vec::new();
    for contour in result.contours() {
        if let Some(c) = runebender_core::glyph_ops::bezpath_to_contour(&contour.path, &smooth_at) {
            contours.push(c);
        }
    }
    if contours.is_empty() {
        return false;
    }
    glyph.contours = contours;
    true
}

/// Set curve handles to a fraction of their maximum: 100% puts each
/// handle at the intersection of the segment's end tangents (the
/// longest the curve can be without a kink), Glyphs' Fit Curve
/// scale. Applies to segments with a selected point, or the whole
/// glyph when the selection is empty. Tangent directions are kept;
/// only lengths change. Returns true if anything moved.
fn fit_curve_handles(
    glyph: &mut norad::Glyph,
    selected: &std::collections::HashSet<(usize, usize)>,
    fraction: f64,
) -> bool {
    use kurbo::{Point, Vec2};
    if !(0.01..=1.5).contains(&fraction) {
        return false;
    }
    let all = selected.is_empty();
    let mut changed = false;
    for (ci, contour) in glyph.contours.iter_mut().enumerate() {
        let pts = &mut contour.points;
        let n = pts.len();
        if n < 4 {
            continue;
        }
        // Walk cubic segments: offcurve, offcurve, curve on-point,
        // with the previous on-point before them.
        for i in 0..n {
            if pts[i].typ != norad::PointType::Curve {
                continue;
            }
            let c2i = (i + n - 1) % n;
            let c1i = (i + n - 2) % n;
            let p0i = (i + n - 3) % n;
            if pts[c1i].typ != norad::PointType::OffCurve
                || pts[c2i].typ != norad::PointType::OffCurve
                || pts[p0i].typ == norad::PointType::OffCurve
            {
                continue;
            }
            let in_scope = all
                || [p0i, c1i, c2i, i]
                    .iter()
                    .any(|&k| selected.contains(&(ci, k)));
            if !in_scope {
                continue;
            }
            let p0 = Point::new(pts[p0i].x, pts[p0i].y);
            let c1 = Point::new(pts[c1i].x, pts[c1i].y);
            let c2 = Point::new(pts[c2i].x, pts[c2i].y);
            let p3 = Point::new(pts[i].x, pts[i].y);
            let d0 = c1 - p0;
            let d3 = c2 - p3;
            if d0.hypot() < 1e-9 || d3.hypot() < 1e-9 {
                continue;
            }
            let (d0, d3) = (d0 / d0.hypot(), d3 / d3.hypot());
            // Ray intersection p0 + s·d0 = p3 + u·d3.
            let cross = |a: Vec2, b: Vec2| a.x * b.y - a.y * b.x;
            let denom = cross(d0, d3);
            if denom.abs() < 1e-9 {
                continue; // parallel tangents: no finite maximum
            }
            let w = p3 - p0;
            let s_max = cross(w, d3) / denom;
            let u_max = cross(w, d0) / denom;
            if s_max <= 0.0 || u_max <= 0.0 {
                continue; // tangents meet behind the points
            }
            let nc1 = p0 + d0 * (s_max * fraction);
            let nc2 = p3 + d3 * (u_max * fraction);
            let write = |pt: &mut norad::ContourPoint, p: Point| {
                let (nx, ny) = (p.x.round(), p.y.round());
                let moved = pt.x != nx || pt.y != ny;
                pt.x = nx;
                pt.y = ny;
                moved
            };
            changed |= write(&mut pts[c1i], nc1);
            changed |= write(&mut pts[c2i], nc2);
        }
    }
    changed
}

/// Insert on-curve points at every curve extremum (horizontal and
/// vertical tangents), Glyphs' Add Extremes. Targets segments with a
/// selected point, or the whole glyph when the selection is empty.
/// Returns true if any point was added.
fn add_extreme_points(
    glyph: &mut norad::Glyph,
    selected: &std::collections::HashSet<(usize, usize)>,
) -> bool {
    use kurbo::ParamCurveExtrema as _;
    let mut changed = false;
    // One insertion per scan: a split invalidates the segment list.
    let mut guard = 0;
    'outer: loop {
        guard += 1;
        if guard > 300 {
            break;
        }
        for hit in runebender_core::segment_ops::segments(glyph) {
            let kurbo::PathSeg::Cubic(cubic) = hit.seg else {
                continue;
            };
            let in_scope =
                selected.is_empty() || hit.point_ids().iter().any(|id| selected.contains(id));
            if !in_scope {
                continue;
            }
            for t in cubic.extrema() {
                // Extrema at (or rounding onto) the endpoints are
                // already nodes; skipping them also terminates the
                // rescan loop, because subsegments keep their
                // extrema at the ends.
                if !(0.02..=0.98).contains(&t) {
                    continue;
                }
                if runebender_core::segment_ops::insert_point_on_segment(glyph, &hit, t).is_some() {
                    changed = true;
                    continue 'outer;
                }
            }
        }
        break;
    }
    changed
}

/// Extrude (Glyphs' filter): sweep the glyph along `angle` by
/// `offset` units — the union of the shape, its translated copy,
/// and a wall quad per segment — then cut the front face away
/// unless `keep_front`. Angle 0 extrudes right; 30 is the Glyphs
/// default's downward-right shadow.
fn extrude_glyph_contours(
    glyph: &mut norad::Glyph,
    offset: f64,
    angle_degrees: f64,
    keep_front: bool,
) -> bool {
    if offset <= 0.0 || glyph.contours.is_empty() {
        return false;
    }
    let (sin, cos) = (-angle_degrees).to_radians().sin_cos();
    let d = kurbo::Vec2::new(offset * cos, offset * sin);
    let mut combined = BezPath::new();
    let mut front = BezPath::new();
    for contour in &glyph.contours {
        let path = runebender_core::glyph_paths::contour_to_bezpath(contour);
        front.extend(path.elements().iter().copied());
        combined.extend(path.elements().iter().copied());
        combined.extend((Affine::translate(d) * &path).elements().iter().copied());
        // Wall quads, each wound positive so the nonzero union eats
        // them all the same way.
        let mut walls = BezPath::new();
        for seg in path.segments() {
            use kurbo::ParamCurve as _;
            let (a, b) = (seg.eval(0.0), seg.eval(1.0));
            let (a2, b2) = (a + d, b + d);
            let area = (b.x - a.x) * (b2.y - a.y) - (b2.x - a.x) * (b.y - a.y);
            let quad = if area >= 0.0 {
                [a, b, b2, a2]
            } else {
                [a, a2, b2, b]
            };
            walls.move_to(quad[0]);
            walls.line_to(quad[1]);
            walls.line_to(quad[2]);
            walls.line_to(quad[3]);
            walls.close_path();
        }
        combined.extend(walls.elements().iter().copied());
    }
    let empty = BezPath::new();
    let Ok(silhouette) = linesweeper::binary_op(
        &combined,
        &empty,
        linesweeper::FillRule::NonZero,
        linesweeper::BinaryOp::Union,
    ) else {
        return false;
    };
    let mut merged = BezPath::new();
    for contour in silhouette.contours() {
        merged.extend(contour.path.elements().iter().copied());
    }
    let result = if keep_front {
        merged
    } else {
        let Ok(cut) = linesweeper::binary_op(
            &merged,
            &front,
            linesweeper::FillRule::NonZero,
            linesweeper::BinaryOp::Difference,
        ) else {
            return false;
        };
        let mut out = BezPath::new();
        for contour in cut.contours() {
            out.extend(contour.path.elements().iter().copied());
        }
        out
    };
    let empty_map = std::collections::HashMap::new();
    let mut contours: Vec<norad::Contour> = Vec::new();
    let mut sub = BezPath::new();
    for el in result.elements() {
        if matches!(el, PathEl::MoveTo(_)) && !sub.elements().is_empty() {
            if let Some(c) = runebender_core::glyph_ops::bezpath_to_contour(&sub, &empty_map) {
                contours.push(c);
            }
            sub = BezPath::new();
        }
        sub.push(*el);
    }
    if !sub.elements().is_empty() {
        if let Some(c) = runebender_core::glyph_ops::bezpath_to_contour(&sub, &empty_map) {
            contours.push(c);
        }
    }
    if contours.is_empty() {
        return false;
    }
    glyph.contours = contours;
    true
}

/// Roughen (Glyphs' filter): flatten each targeted contour into
/// straight segments of roughly `segment_length`, then jitter every
/// point by up to ±h/±v. `seed` varies run to run so Apply twice
/// gives a different rough.
fn roughen_glyph_contours(
    glyph: &mut norad::Glyph,
    selected: &std::collections::HashSet<usize>,
    segment_length: f64,
    h: f64,
    v: f64,
    seed: u64,
) -> bool {
    use kurbo::ParamCurve as _;
    use kurbo::ParamCurveArclen as _;
    if segment_length < 1.0 {
        return false;
    }
    // A tiny LCG: deterministic per seed, no clock, no dependency.
    let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    let mut jitter = |amount: f64| {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let unit = (state >> 11) as f64 / (1u64 << 53) as f64;
        (unit * 2.0 - 1.0) * amount
    };
    let mut changed = false;
    for (ci, contour) in glyph.contours.iter_mut().enumerate() {
        if !(selected.is_empty() || selected.contains(&ci)) {
            continue;
        }
        let path = runebender_core::glyph_paths::contour_to_bezpath(&*contour);
        let mut points: Vec<norad::ContourPoint> = Vec::new();
        for seg in path.segments() {
            let len = seg.arclen(0.5);
            let steps = (len / segment_length).ceil().max(1.0) as usize;
            for step in 0..steps {
                let t = step as f64 / steps as f64;
                let p = seg.eval(t);
                points.push(norad::ContourPoint::new(
                    (p.x + jitter(h)).round(),
                    (p.y + jitter(v)).round(),
                    norad::PointType::Line,
                    false,
                    None,
                    None,
                ));
            }
        }
        if points.len() >= 3 {
            *contour = norad::Contour::new(points, None);
            changed = true;
        }
    }
    changed
}

/// Open a closed contour at an on-curve point (it becomes the new
/// start, typed Move), or close an open contour again (the Move
/// start becomes a Line). Glyphs' opening and closing paths.
fn toggle_contour_open(glyph: &mut norad::Glyph, ci: usize, pi: usize) -> bool {
    use norad::PointType;
    let Some(contour) = glyph.contours.get_mut(ci) else {
        return false;
    };
    let n = contour.points.len();
    if n < 2 || pi >= n {
        return false;
    }
    let is_open = contour
        .points
        .first()
        .is_some_and(|p| p.typ == PointType::Move);
    if is_open {
        // Close: the Move start becomes an ordinary point. If the
        // start needs a curve type it stays Line; the designer
        // redraws the closing segment as needed.
        contour.points[0].typ = PointType::Line;
        return true;
    }
    if contour.points[pi].typ == PointType::OffCurve {
        return false;
    }
    contour.points.rotate_left(pi);
    contour.points[0].typ = PointType::Move;
    true
}

/// A standalone SVG document for one glyph: the outline in font
/// units, y flipped into SVG space, the viewBox spanning the em
/// (ascender down to descender) across the advance.
fn glyph_svg(path: &BezPath, advance: f64, ascender: f64, descender: f64) -> String {
    let height = ascender - descender;
    format!(
        concat!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" ",
            "viewBox=\"0 0 {w} {h}\">\n",
            "  <path transform=\"translate(0,{asc}) scale(1,-1)\" ",
            "d=\"{d}\"/>\n",
            "</svg>\n"
        ),
        w = advance,
        h = height,
        asc = ascender,
        d = path.to_svg(),
    )
}

/// Path > Tidy up Paths: drop on-curve points that duplicate the
/// previous on-curve point (zero-length line segments), including
/// the closing wrap of a closed contour. Conservative on purpose —
/// curve simplification is Simplify's job, not Tidy's. Returns how
/// many points were removed.
fn tidy_contours(glyph: &mut norad::Glyph) -> usize {
    use norad::PointType;
    let mut removed = 0usize;
    for contour in glyph.contours.iter_mut() {
        let closed = contour
            .points
            .first()
            .is_none_or(|p| p.typ != PointType::Move);
        let mut i = 1;
        while i < contour.points.len() {
            let dup = {
                let prev = &contour.points[i - 1];
                let here = &contour.points[i];
                here.typ == PointType::Line
                    && prev.typ != PointType::OffCurve
                    && (here.x - prev.x).abs() < 0.01
                    && (here.y - prev.y).abs() < 0.01
            };
            if dup {
                contour.points.remove(i);
                removed += 1;
            } else {
                i += 1;
            }
        }
        // A closed contour's last Line landing on the first point is
        // the same zero-length segment, wrapped.
        if closed && contour.points.len() > 2 {
            let first = contour.points[0].clone();
            let last = contour.points.last().unwrap().clone();
            if last.typ == PointType::Line
                && first.typ != PointType::OffCurve
                && (last.x - first.x).abs() < 0.01
                && (last.y - first.y).abs() < 0.01
            {
                contour.points.pop();
                removed += 1;
            }
        }
    }
    removed
}

/// Path > Correct Path Direction: outer contours counter-clockwise,
/// holes clockwise (the PostScript/UFO cubic convention, and what
/// remove-overlap expects). Depth = how many other contours contain
/// the contour's first on-curve point; even is outer. Returns how
/// many contours were reversed.
fn correct_path_directions(glyph: &mut norad::Glyph) -> usize {
    use kurbo::Shape as _;
    let paths: Vec<BezPath> = glyph
        .contours
        .iter()
        .map(runebender_core::glyph_paths::contour_to_bezpath)
        .collect();
    let mut flip: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
    let mut flipped = 0usize;
    for (ci, contour) in glyph.contours.iter().enumerate() {
        let Some(probe) = contour
            .points
            .iter()
            .find(|p| p.typ != norad::PointType::OffCurve)
        else {
            continue;
        };
        let pt = kurbo::Point::new(probe.x, probe.y);
        let depth = paths
            .iter()
            .enumerate()
            .filter(|(oi, path)| *oi != ci && path.contains(pt))
            .count();
        let area = paths[ci].area();
        let want_ccw = depth % 2 == 0;
        if (want_ccw && area < 0.0) || (!want_ccw && area > 0.0) {
            flip.insert((ci, 0));
            flipped += 1;
        }
    }
    if !flip.is_empty() {
        runebender_core::glyph_ops::reverse_contours(glyph, &flip);
    }
    flipped
}

/// Path > Round Coordinates: every point onto the integer grid.
/// Returns how many points moved.
fn round_glyph_coordinates(glyph: &mut norad::Glyph) -> usize {
    let mut moved = 0usize;
    for contour in glyph.contours.iter_mut() {
        for p in contour.points.iter_mut() {
            let (rx, ry) = (p.x.round(), p.y.round());
            if rx != p.x || ry != p.y {
                p.x = rx;
                p.y = ry;
                moved += 1;
            }
        }
    }
    moved
}

// ---- joining QA (Arabic connecting-stroke bands) ----

/// The y-extent of a glyph's ink at one joining edge: outline
/// points (components resolved) at or past x = 0 going left, or at
/// or past x = advance going right — joining strokes overlap the
/// edge on purpose (the anti-seam tongue), so the test is
/// one-sided. None when nothing reaches the edge — for a form that
/// should join, that is itself the defect.
fn joining_band(outline: &BezPath, advance: f64, left: bool, tolerance: f64) -> Option<(f64, f64)> {
    let mut band: Option<(f64, f64)> = None;
    let mut visit = |p: kurbo::Point| {
        let reaches = if left {
            p.x <= tolerance
        } else {
            p.x >= advance - tolerance
        };
        if !reaches {
            return;
        }
        band = Some(match band {
            Some((lo, hi)) => (lo.min(p.y), hi.max(p.y)),
            None => (p.y, p.y),
        });
    };
    for el in outline.elements() {
        match el {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => visit(*p),
            PathEl::QuadTo(c, p) => {
                visit(*c);
                visit(*p);
            }
            PathEl::CurveTo(c1, c2, p) => {
                visit(*c1);
                visit(*c2);
                visit(*p);
            }
            PathEl::ClosePath => {}
        }
    }
    band
}

// ---- COLRv1 (paint graphs through the ufo2ft colorLayers key) ----

/// The explicit color-layers key, fontTools buildCOLR's input.
/// Once present, ufo2ft skips its own layer exploding — so writing
/// any v1 entry means exploding every color glyph ourselves.
const COLOR_LAYERS_EXPLICIT_KEY: &str = "com.github.googlei18n.ufo2ft.colorLayers";

/// A COLRv1 linear-gradient paint dict in fontTools' unbuilt form:
/// two palette stops, running from `p0` to `p1` (x2/y2 is the
/// required rotation vector, perpendicular to the gradient).
fn linear_gradient_paint(
    stop0: usize,
    stop1: usize,
    p0: (f64, f64),
    p1: (f64, f64),
) -> plist::Value {
    let stop = |offset: f64, palette: usize| {
        let mut dict = plist::Dictionary::new();
        dict.insert("StopOffset".into(), plist::Value::Real(offset));
        dict.insert(
            "PaletteIndex".into(),
            plist::Value::Integer((palette as u64).into()),
        );
        dict.insert("Alpha".into(), plist::Value::Real(1.0));
        plist::Value::Dictionary(dict)
    };
    let mut color_line = plist::Dictionary::new();
    color_line.insert(
        "ColorStop".into(),
        plist::Value::Array(vec![stop(0.0, stop0), stop(1.0, stop1)]),
    );
    color_line.insert("Extend".into(), plist::Value::String("pad".into()));
    let mut paint = plist::Dictionary::new();
    // PaintLinearGradient.
    paint.insert("Format".into(), plist::Value::Integer(4u64.into()));
    paint.insert("ColorLine".into(), plist::Value::Dictionary(color_line));
    paint.insert("x0".into(), plist::Value::Real(p0.0));
    paint.insert("y0".into(), plist::Value::Real(p0.1));
    paint.insert("x1".into(), plist::Value::Real(p1.0));
    paint.insert("y1".into(), plist::Value::Real(p1.1));
    // Rotation vector: perpendicular to p0->p1.
    paint.insert("x2".into(), plist::Value::Real(p0.0 + (p1.1 - p0.1)));
    paint.insert("y2".into(), plist::Value::Real(p0.1 - (p1.0 - p0.0)));
    plist::Value::Dictionary(paint)
}

/// A PaintGlyph layer wrapping a child paint (Format 10), and the
/// solid child (Format 2) — the shapes verified through ufo2ft's
/// buildCOLR: the glyph's root is PaintColrLayers (Format 1) with
/// these as Layers.
fn paint_glyph_layer(glyph: &str, child: plist::Value) -> plist::Value {
    let mut dict = plist::Dictionary::new();
    dict.insert("Format".into(), plist::Value::Integer(10u64.into()));
    dict.insert("Glyph".into(), plist::Value::String(glyph.into()));
    dict.insert("Paint".into(), child);
    plist::Value::Dictionary(dict)
}

fn paint_solid(palette: usize) -> plist::Value {
    let mut dict = plist::Dictionary::new();
    dict.insert("Format".into(), plist::Value::Integer(2u64.into()));
    dict.insert(
        "PaletteIndex".into(),
        plist::Value::Integer((palette as u64).into()),
    );
    dict.insert("Alpha".into(), plist::Value::Real(1.0));
    plist::Value::Dictionary(dict)
}

/// Does this font carry explicit (v1) color layers for the glyph?
fn has_v1_entry(font: &norad::Font, glyph: &str) -> bool {
    font.lib
        .get(COLOR_LAYERS_EXPLICIT_KEY)
        .and_then(|v| v.as_dictionary())
        .is_some_and(|d| d.contains_key(glyph))
}

// ---- masks (subtracting contours, the Glyphs path attribute) ----

/// Contour indices marked as masks: shapes that cut away from the
/// rest of the glyph. Live in a lib key; previews subtract them,
/// Bake Masks makes the subtraction real (external compilers only
/// see baked outlines).
const MASKS_KEY: &str = "com.runebender.masks";

fn read_masks(glyph: &norad::Glyph) -> std::collections::HashSet<usize> {
    glyph
        .lib
        .get(MASKS_KEY)
        .and_then(|v| v.as_array())
        .map(|rows| {
            rows.iter()
                .filter_map(|v| v.as_signed_integer())
                .filter(|&i| i >= 0)
                .map(|i| i as usize)
                .collect()
        })
        .unwrap_or_default()
}

fn write_masks(glyph: &mut norad::Glyph, masks: &std::collections::HashSet<usize>) {
    if masks.is_empty() {
        glyph.lib.remove(MASKS_KEY);
        return;
    }
    let mut sorted: Vec<usize> = masks.iter().copied().collect();
    sorted.sort_unstable();
    glyph.lib.insert(
        MASKS_KEY.into(),
        plist::Value::Array(
            sorted
                .into_iter()
                .map(|i| plist::Value::Integer((i as u64).into()))
                .collect(),
        ),
    );
}

/// Cut the mask contours out of the rest and drop them: the final
/// outline every compiler understands. Returns false when the glyph
/// has no masks or the boolean fails.
fn bake_masks(glyph: &mut norad::Glyph) -> bool {
    let masks = read_masks(glyph);
    if masks.is_empty() || masks.len() >= glyph.contours.len() {
        return false;
    }
    let mut keep = BezPath::new();
    let mut cut = BezPath::new();
    for (ci, contour) in glyph.contours.iter().enumerate() {
        let path = runebender_core::glyph_paths::contour_to_bezpath(contour);
        let target = if masks.contains(&ci) {
            &mut cut
        } else {
            &mut keep
        };
        target.extend(path.elements().iter().copied());
    }
    let Ok(result) = linesweeper::binary_op(
        &keep,
        &cut,
        linesweeper::FillRule::NonZero,
        linesweeper::BinaryOp::Difference,
    ) else {
        return false;
    };
    let empty = std::collections::HashMap::new();
    let mut contours = Vec::new();
    for contour in result.contours() {
        if let Some(c) = runebender_core::glyph_ops::bezpath_to_contour(&contour.path, &empty) {
            contours.push(c);
        }
    }
    if contours.is_empty() {
        return false;
    }
    glyph.contours = contours;
    write_masks(glyph, &std::collections::HashSet::new());
    true
}

// ---- annotations (canvas notes, arrows, circles) ----

/// Editor annotations, the Glyphs annotation tool's marks: arrows,
/// circles, plus/minus, and text notes pinned to design-space
/// points. Stored in a glyph lib key; never exported.
/// Saved sidebar filters: searches the user pinned, stored in the
/// font lib as an array of {name, query} dicts. Glyphs calls these
/// smart filters; ours reuse the search-field predicate language.
const SAVED_FILTERS_KEY: &str = "com.runebender.savedFilters";

/// UFO-standard glyph name -> production name mapping (consumed by
/// ufo2ft/fontc at compile time).
const PSNAMES_KEY: &str = "public.postscriptNames";

fn read_production_name(font: &norad::Font, glyph: &str) -> Option<String> {
    match font.lib.get(PSNAMES_KEY)? {
        plist::Value::Dictionary(d) => d.get(glyph)?.as_string().map(str::to_string),
        _ => None,
    }
}

fn read_saved_filters(font: &norad::Font) -> Vec<(String, String)> {
    let Some(plist::Value::Array(rows)) = font.lib.get(SAVED_FILTERS_KEY) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            let dict = row.as_dictionary()?;
            let name = dict.get("name")?.as_string()?.to_string();
            let query = dict.get("query")?.as_string()?.to_string();
            Some((name, query))
        })
        .collect()
}

fn write_saved_filters(font: &mut norad::Font, filters: &[(String, String)]) {
    if filters.is_empty() {
        font.lib.remove(SAVED_FILTERS_KEY);
        return;
    }
    let rows = filters
        .iter()
        .map(|(name, query)| {
            let mut dict = plist::Dictionary::new();
            dict.insert("name".into(), plist::Value::String(name.clone()));
            dict.insert("query".into(), plist::Value::String(query.clone()));
            plist::Value::Dictionary(dict)
        })
        .collect();
    font.lib
        .insert(SAVED_FILTERS_KEY.into(), plist::Value::Array(rows));
}

const ANNOTATIONS_KEY: &str = "com.runebender.annotations";

#[derive(Clone, Debug, PartialEq)]
struct Annotation {
    kind: String,
    x: f64,
    y: f64,
    text: String,
}

fn read_annotations(glyph: &norad::Glyph) -> Vec<Annotation> {
    glyph
        .lib
        .get(ANNOTATIONS_KEY)
        .and_then(|v| v.as_array())
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let dict = row.as_dictionary()?;
                    Some(Annotation {
                        kind: dict.get("kind")?.as_string()?.to_string(),
                        x: dict.get("x")?.as_real()?,
                        y: dict.get("y")?.as_real()?,
                        text: dict
                            .get("text")
                            .and_then(|t| t.as_string())
                            .unwrap_or_default()
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn write_annotations(glyph: &mut norad::Glyph, notes: &[Annotation]) {
    if notes.is_empty() {
        glyph.lib.remove(ANNOTATIONS_KEY);
        return;
    }
    let rows = notes
        .iter()
        .map(|a| {
            let mut dict = plist::Dictionary::new();
            dict.insert("kind".into(), plist::Value::String(a.kind.clone()));
            dict.insert("x".into(), plist::Value::Real(a.x));
            dict.insert("y".into(), plist::Value::Real(a.y));
            if !a.text.is_empty() {
                dict.insert("text".into(), plist::Value::String(a.text.clone()));
            }
            plist::Value::Dictionary(dict)
        })
        .collect();
    glyph
        .lib
        .insert(ANNOTATIONS_KEY.into(), plist::Value::Array(rows));
}

// ---- SVG outline import ----

/// Pull every path's `d` attribute out of an SVG document, parse
/// with kurbo, flip to font coordinates (SVG runs y-down), and fit
/// the whole drawing between `descender` and `ascender`. Fills,
/// strokes, groups, and transforms are ignored: this is the
/// Illustrator-outline paste, not a renderer.
fn svg_to_contours(
    svg_text: &str,
    ascender: f64,
    descender: f64,
) -> Result<Vec<norad::Contour>, String> {
    let mut combined = BezPath::new();
    let mut rest = svg_text;
    while let Some(at) = rest.find(" d=") {
        let after = &rest[at + 3..];
        let Some(quote) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            rest = after;
            continue;
        };
        let body = &after[1..];
        let Some(end) = body.find(quote) else { break };
        let data = &body[..end];
        let path = BezPath::from_svg(data).map_err(|e| format!("SVG path: {e}"))?;
        combined.extend(path.elements().iter().copied());
        rest = &body[end..];
    }
    if combined.elements().is_empty() {
        return Err("no <path d=\"…\"> outlines in the SVG".into());
    }
    use kurbo::Shape as _;
    let bbox = combined.bounding_box();
    if bbox.height() < 1e-6 {
        return Err("SVG outlines have no height".into());
    }
    let scale = (ascender - descender) / bbox.height();
    // Flip and fit: SVG top lands on the ascender.
    let fitted = Affine::translate((0.0, ascender))
        * Affine::scale_non_uniform(scale, -scale)
        * Affine::translate((-bbox.x0, -bbox.y0))
        * combined;
    let empty = std::collections::HashMap::new();
    let mut contours = Vec::new();
    let mut sub = BezPath::new();
    for el in fitted.elements() {
        if matches!(el, PathEl::MoveTo(_)) && !sub.elements().is_empty() {
            if let Some(c) = runebender_core::glyph_ops::bezpath_to_contour(&sub, &empty) {
                contours.push(c);
            }
            sub = BezPath::new();
        }
        sub.push(*el);
    }
    if !sub.elements().is_empty() {
        if let Some(c) = runebender_core::glyph_ops::bezpath_to_contour(&sub, &empty) {
            contours.push(c);
        }
    }
    (!contours.is_empty())
        .then_some(contours)
        .ok_or_else(|| "SVG outlines did not convert".into())
}

// ---- cubic <-> quadratic conversion ----

/// Rewrite quadratic segments as exact cubics: each offcurve+qcurve
/// pair (P0, C, P1) becomes the identical cubic with controls at
/// P0 + 2/3(C-P0) and P1 + 2/3(C-P1). Lossless.
fn quads_to_cubics(glyph: &mut norad::Glyph) -> bool {
    use norad::PointType;
    let mut changed = false;
    for contour in glyph.contours.iter_mut() {
        let n = contour.points.len();
        if n < 3 {
            continue;
        }
        let has_quads = contour.points.iter().any(|p| p.typ == PointType::QCurve);
        if !has_quads {
            continue;
        }
        let old = contour.points.clone();
        let mut points: Vec<norad::ContourPoint> = Vec::with_capacity(n + 4);
        for (i, p) in old.iter().enumerate() {
            if p.typ != PointType::QCurve {
                points.push(p.clone());
                continue;
            }
            // The single offcurve before this qcurve, and the on-point
            // before that.
            let ci = (i + n - 1) % n;
            let oi = (i + n - 2) % n;
            let (c, p0) = (&old[ci], &old[oi]);
            if c.typ != PointType::OffCurve || p0.typ == PointType::OffCurve {
                points.push(p.clone());
                continue;
            }
            // Replace the emitted offcurve with the two cubic ones.
            let popped = points.pop();
            debug_assert!(popped.is_some_and(|q| q.typ == PointType::OffCurve));
            let c1 = (
                p0.x + (c.x - p0.x) * 2.0 / 3.0,
                p0.y + (c.y - p0.y) * 2.0 / 3.0,
            );
            let c2 = (p.x + (c.x - p.x) * 2.0 / 3.0, p.y + (c.y - p.y) * 2.0 / 3.0);
            let off = |x: f64, y: f64| {
                norad::ContourPoint::new(
                    x.round(),
                    y.round(),
                    PointType::OffCurve,
                    false,
                    None,
                    None,
                )
            };
            points.push(off(c1.0, c1.1));
            points.push(off(c2.0, c2.1));
            points.push(norad::ContourPoint::new(
                p.x,
                p.y,
                PointType::Curve,
                p.smooth,
                None,
                None,
            ));
            changed = true;
        }
        if changed {
            contour.points = points;
        }
    }
    changed
}

/// Approximate cubic segments with quadratics: each cubic splits in
/// halves until one quad (control from the 3/4 rule) sits within
/// `tolerance` of it, then the quads replace the cubic. The reverse
/// of quads_to_cubics, lossy by nature — the same trade every
/// cubic-to-TrueType compiler makes.
fn cubics_to_quads(glyph: &mut norad::Glyph, tolerance: f64) -> bool {
    use kurbo::{CubicBez, ParamCurve as _, Point};
    use norad::PointType;
    fn approx(cubic: CubicBez, tolerance: f64, out: &mut Vec<(Point, Point)>) {
        // One-quad candidate: Q = (3(c1+c2) − (p0+p3)) / 4.
        let q = Point::new(
            (3.0 * (cubic.p1.x + cubic.p2.x) - (cubic.p0.x + cubic.p3.x)) / 4.0,
            (3.0 * (cubic.p1.y + cubic.p2.y) - (cubic.p0.y + cubic.p3.y)) / 4.0,
        );
        let quad = kurbo::QuadBez::new(cubic.p0, q, cubic.p3);
        let err = [0.25, 0.5, 0.75]
            .iter()
            .map(|&t| cubic.eval(t).distance(quad.eval(t)))
            .fold(0.0_f64, f64::max);
        if err <= tolerance || out.len() > 64 {
            out.push((q, cubic.p3));
        } else {
            let (a, b) = cubic.subdivide();
            approx(a, tolerance, out);
            approx(b, tolerance, out);
        }
    }
    let mut changed = false;
    for contour in glyph.contours.iter_mut() {
        let n = contour.points.len();
        if n < 4 {
            continue;
        }
        let has_cubics = contour.points.iter().any(|p| p.typ == PointType::Curve);
        if !has_cubics {
            continue;
        }
        let old = contour.points.clone();
        let mut points: Vec<norad::ContourPoint> = Vec::new();
        for (i, p) in old.iter().enumerate() {
            if p.typ != PointType::Curve {
                points.push(p.clone());
                continue;
            }
            let c2i = (i + n - 1) % n;
            let c1i = (i + n - 2) % n;
            let p0i = (i + n - 3) % n;
            let (c2, c1, p0) = (&old[c2i], &old[c1i], &old[p0i]);
            if c1.typ != PointType::OffCurve
                || c2.typ != PointType::OffCurve
                || p0.typ == PointType::OffCurve
            {
                points.push(p.clone());
                continue;
            }
            // Drop the two emitted cubic offcurves.
            points.pop();
            points.pop();
            let cubic = CubicBez::new(
                Point::new(p0.x, p0.y),
                Point::new(c1.x, c1.y),
                Point::new(c2.x, c2.y),
                Point::new(p.x, p.y),
            );
            let mut quads = Vec::new();
            approx(cubic, tolerance, &mut quads);
            for (k, (control, end)) in quads.iter().enumerate() {
                points.push(norad::ContourPoint::new(
                    control.x.round(),
                    control.y.round(),
                    PointType::OffCurve,
                    false,
                    None,
                    None,
                ));
                let last = k + 1 == quads.len();
                points.push(norad::ContourPoint::new(
                    if last { p.x } else { end.x.round() },
                    if last { p.y } else { end.y.round() },
                    PointType::QCurve,
                    if last { p.smooth } else { true },
                    None,
                    None,
                ));
            }
            changed = true;
        }
        if changed {
            contour.points = points;
        }
    }
    changed
}

// ---- compiled-font import (TTF/OTF via skrifa) ----

/// A pen that collects skrifa outline callbacks into UFO contours.
/// Quadratics stay quadratic (offcurve + qcurve points), cubics stay
/// cubic; every binary contour is closed.
#[derive(Default)]
struct BinaryImportPen {
    contours: Vec<norad::Contour>,
    current: Vec<norad::ContourPoint>,
}

impl BinaryImportPen {
    fn point(x: f32, y: f32, typ: norad::PointType) -> norad::ContourPoint {
        norad::ContourPoint::new(
            (x as f64).round(),
            (y as f64).round(),
            typ,
            false,
            None,
            None,
        )
    }

    fn finish_contour(&mut self) {
        if self.current.is_empty() {
            return;
        }
        let points = std::mem::take(&mut self.current);
        // Closed contour: the leading Move either duplicates the
        // final on-point (drop it) or becomes an ordinary point.
        let mut points = points;
        if points.len() >= 2 && points[0].typ == norad::PointType::Move {
            let (fx, fy) = (points[0].x, points[0].y);
            let last_matches = points
                .last()
                .is_some_and(|l| l.typ != norad::PointType::OffCurve && l.x == fx && l.y == fy);
            if last_matches {
                points.remove(0);
            } else {
                points[0].typ = norad::PointType::Line;
            }
        }
        if points.len() >= 2 {
            self.contours.push(norad::Contour::new(points, None));
        }
    }
}

impl skrifa::outline::OutlinePen for BinaryImportPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.finish_contour();
        self.current.push(Self::point(x, y, norad::PointType::Move));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.current.push(Self::point(x, y, norad::PointType::Line));
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.current
            .push(Self::point(cx0, cy0, norad::PointType::OffCurve));
        self.current
            .push(Self::point(x, y, norad::PointType::QCurve));
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.current
            .push(Self::point(cx0, cy0, norad::PointType::OffCurve));
        self.current
            .push(Self::point(cx1, cy1, norad::PointType::OffCurve));
        self.current
            .push(Self::point(x, y, norad::PointType::Curve));
    }
    fn close(&mut self) {
        self.finish_contour();
    }
}

/// Open a compiled TTF or OTF as an editable in-memory UFO: names,
/// metrics, encodings, and outlines (glyf quadratics kept as UFO
/// qcurves, CFF cubics kept cubic). Kerning and features are not
/// decompiled in this slice.
fn import_binary_font(path: &std::path::Path) -> Result<norad::Font, String> {
    use skrifa::MetadataProvider as _;
    use skrifa::raw::TableProvider as _;
    let bytes = std::fs::read(path).map_err(|e| format!("{e}"))?;
    let font_ref = skrifa::FontRef::new(&bytes).map_err(|e| format!("{e}"))?;
    let size = skrifa::instance::Size::unscaled();
    let location = skrifa::instance::LocationRef::default();
    let metrics = font_ref.metrics(size, location);
    let glyph_metrics = font_ref.glyph_metrics(size, location);
    let english = |id: skrifa::string::StringId| {
        font_ref
            .localized_strings(id)
            .english_or_first()
            .map(|s| s.chars().collect::<String>())
    };
    let mut font = norad::Font::default();
    let info = &mut font.font_info;
    info.family_name = english(skrifa::string::StringId::FAMILY_NAME);
    info.style_name = english(skrifa::string::StringId::SUBFAMILY_NAME);
    info.units_per_em =
        norad::fontinfo::NonNegativeIntegerOrFloat::try_from(metrics.units_per_em as f64).ok();
    info.ascender = Some(metrics.ascent as f64);
    // skrifa's descent is signed; UFO wants it below zero.
    let descent = metrics.descent as f64;
    info.descender = Some(if descent > 0.0 { -descent } else { descent });
    info.x_height = metrics.x_height.map(|v| v as f64);
    info.cap_height = metrics.cap_height.map(|v| v as f64);
    // gid → codepoints.
    let mut encodings: std::collections::HashMap<u32, Vec<char>> = std::collections::HashMap::new();
    for (codepoint, gid) in font_ref.charmap().mappings() {
        if let Some(c) = char::from_u32(codepoint) {
            encodings.entry(gid.to_u32()).or_default().push(c);
        }
    }
    let names = font_ref.glyph_names();
    let outlines = font_ref.outline_glyphs();
    let count = font_ref
        .maxp()
        .map(|maxp| maxp.num_glyphs() as u32)
        .map_err(|e| format!("{e}"))?;
    let mut seen = std::collections::HashSet::new();
    for raw_gid in 0..count {
        let gid = skrifa::GlyphId::new(raw_gid);
        let mut name = names
            .get(gid)
            .map(|n| n.to_string())
            .unwrap_or_else(|| format!("glyph{raw_gid:05}"));
        if !seen.insert(name.clone()) {
            name = format!("{name}.gid{raw_gid}");
            seen.insert(name.clone());
        }
        let mut pen = BinaryImportPen::default();
        if let Some(outline) = outlines.get(gid) {
            let _ = outline.draw(
                skrifa::outline::DrawSettings::unhinted(size, location),
                &mut pen,
            );
            pen.finish_contour();
        }
        let Ok(glyph_name) = norad::Name::new(&name) else {
            continue;
        };
        let mut glyph = norad::Glyph::new(glyph_name.as_str());
        glyph.contours = pen.contours;
        glyph.width = glyph_metrics.advance_width(gid).unwrap_or(0.0) as f64;
        if let Some(codepoints) = encodings.get(&raw_gid) {
            glyph.codepoints = norad::Codepoints::new(codepoints.iter().copied());
        }
        font.default_layer_mut().insert_glyph(glyph);
    }
    Ok(font)
}

/// Apply a corner glyph to one on-curve node: the corner's open
/// path, drawn around its origin, is mapped into the node's frame —
/// corner-space x runs back along the incoming segment, y forward
/// along the outgoing one (Glyphs' fit, which shears the corner to
/// unequal angles) — and spliced in place of the node. Both
/// neighbors must be on-curve (line corners) in this first slice.
/// The result is a plain outline: pipelines see baked points.
fn apply_corner_at(glyph: &mut norad::Glyph, corner: &norad::Glyph, ci: usize, pi: usize) -> bool {
    use norad::PointType;
    let Some(corner_contour) = corner.contours.first() else {
        return false;
    };
    if corner_contour.points.len() < 2 {
        return false;
    }
    let Some(contour) = glyph.contours.get(ci) else {
        return false;
    };
    let n = contour.points.len();
    if n < 3 || pi >= n {
        return false;
    }
    let point = &contour.points[pi];
    if point.typ == PointType::OffCurve {
        return false;
    }
    let prev = &contour.points[(pi + n - 1) % n];
    let next = &contour.points[(pi + 1) % n];
    if prev.typ == PointType::OffCurve || next.typ == PointType::OffCurve {
        return false; // curve corners come later
    }
    let node = (point.x, point.y);
    let len_in = ((node.0 - prev.x).powi(2) + (node.1 - prev.y).powi(2)).sqrt();
    let len_out = ((next.x - node.0).powi(2) + (next.y - node.1).powi(2)).sqrt();
    if len_in < 1e-6 || len_out < 1e-6 {
        return false;
    }
    let u = ((node.0 - prev.x) / len_in, (node.1 - prev.y) / len_in);
    let v = ((next.x - node.0) / len_out, (next.y - node.1) / len_out);
    let mapped: Vec<norad::ContourPoint> = corner_contour
        .points
        .iter()
        .map(|p| {
            let (x, y) = (
                node.0 + p.x * u.0 + p.y * v.0,
                node.1 + p.x * u.1 + p.y * v.1,
            );
            let typ = match p.typ {
                PointType::Move => PointType::Line,
                other => other,
            };
            norad::ContourPoint::new(x.round(), y.round(), typ, p.smooth, None, None)
        })
        .collect();
    let contour = glyph.contours.get_mut(ci).expect("checked");
    contour.points.splice(pi..=pi, mapped);
    true
}

/// Metrics keys, the Glyphs spacing formulas, stored in the lib
/// keys glyphsLib round-trips ("com.schriftgestaltung.Glyphs.
/// glyph.leftMetricsKey" / rightMetricsKey). "=n" copies n's same
/// sidebearing, "=|o" the opposite one, "=n+10" and "=n*1.1" add
/// arithmetic, "=50" is a constant.
const LEFT_METRICS_KEY: &str = "com.schriftgestaltung.Glyphs.glyph.leftMetricsKey";
const RIGHT_METRICS_KEY: &str = "com.schriftgestaltung.Glyphs.glyph.rightMetricsKey";

/// A parsed metrics-key formula.
#[derive(Clone, Debug, PartialEq)]
enum MetricsFormula {
    Constant(f64),
    Reference {
        glyph: String,
        /// Read the opposite sidebearing of the referenced glyph.
        mirror: bool,
        /// Trailing arithmetic: ('+' | '-' | '*', value).
        op: Option<(char, f64)>,
    },
}

fn parse_metrics_key(text: &str) -> Option<MetricsFormula> {
    let body = text.trim().trim_start_matches('=').trim();
    if body.is_empty() {
        return None;
    }
    if let Ok(v) = body.parse::<f64>() {
        return Some(MetricsFormula::Constant(v));
    }
    let (mirror, body) = match body.strip_prefix('|') {
        Some(rest) => (true, rest.trim()),
        None => (false, body),
    };
    let split = body.find(['+', '-', '*']).filter(|&i| i > 0);
    let (name, op) = match split {
        Some(i) => {
            let sign = body.as_bytes()[i] as char;
            let value = body[i + 1..].trim().parse::<f64>().ok()?;
            (body[..i].trim(), Some((sign, value)))
        }
        None => (body, None),
    };
    (!name.is_empty()).then(|| MetricsFormula::Reference {
        glyph: name.to_string(),
        mirror,
        op,
    })
}

fn read_metrics_key(glyph: &norad::Glyph, left: bool) -> Option<String> {
    let key = if left {
        LEFT_METRICS_KEY
    } else {
        RIGHT_METRICS_KEY
    };
    glyph
        .lib
        .get(key)
        .and_then(|v| v.as_string())
        .map(|v| v.to_string())
}

fn write_metrics_key(glyph: &mut norad::Glyph, left: bool, value: &str) {
    let key = if left {
        LEFT_METRICS_KEY
    } else {
        RIGHT_METRICS_KEY
    };
    let value = value.trim();
    if value.is_empty() {
        glyph.lib.remove(key);
    } else {
        glyph
            .lib
            .insert(key.into(), plist::Value::String(value.into()));
    }
}

/// Per-node HOI intermediate points (the Glyphs "Intermediate
/// Point": the node's interpolation path curves through it at the
/// axis middle). Stored on the axis-min master's glyph, absolute
/// design coordinates, keyed "contour,point". Source of truth for
/// re-editing; the baked brace layers are what compilers consume.
const HOI_INTERMEDIATE_KEY: &str = "com.runebender.hoiIntermediate";

fn read_hoi_intermediates(
    glyph: &norad::Glyph,
) -> std::collections::HashMap<(usize, usize), (f64, f64)> {
    glyph
        .lib
        .get(HOI_INTERMEDIATE_KEY)
        .and_then(|v| v.as_dictionary())
        .map(|dict| {
            dict.iter()
                .filter_map(|(key, value)| {
                    let (c, p) = key.split_once(',')?;
                    let arr = value.as_array()?;
                    let x = arr.first()?.as_real()?;
                    let y = arr.get(1)?.as_real()?;
                    Some(((c.parse().ok()?, p.parse().ok()?), (x, y)))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn write_hoi_intermediates(
    glyph: &mut norad::Glyph,
    map: &std::collections::HashMap<(usize, usize), (f64, f64)>,
) {
    if map.is_empty() {
        glyph.lib.remove(HOI_INTERMEDIATE_KEY);
        return;
    }
    let mut dict = plist::Dictionary::new();
    for ((c, p), (x, y)) in map {
        dict.insert(
            format!("{c},{p}"),
            plist::Value::Array(vec![plist::Value::Real(*x), plist::Value::Real(*y)]),
        );
    }
    glyph
        .lib
        .insert(HOI_INTERMEDIATE_KEY.into(), plist::Value::Dictionary(dict));
}

/// Quadratic through Q at the middle: position at `t` between `a`
/// and `b` when the path must pass through `q` at t = 0.5.
fn hoi_quad_at(a: (f64, f64), b: (f64, f64), q: (f64, f64), t: f64) -> (f64, f64) {
    // Control C with (1-t)²A + 2(1-t)tC + t²B passing Q at 0.5:
    // Q = A/4 + C/2 + B/4  =>  C = 2Q - (A+B)/2.
    let c = (2.0 * q.0 - (a.0 + b.0) / 2.0, 2.0 * q.1 - (a.1 + b.1) / 2.0);
    let u = 1.0 - t;
    (
        u * u * a.0 + 2.0 * u * t * c.0 + t * t * b.0,
        u * u * a.1 + 2.0 * u * t * c.1 + t * t * b.1,
    )
}

/// One parsed search predicate (the Counterpunch dynamic-filter
/// idea as search syntax): `w>600`, `cat:Mark`, `mark:red`,
/// `enc:no`, `comp:beh-ar`, `has:anchors`.
#[derive(Clone, Debug, PartialEq)]
enum SearchPred {
    Width(std::cmp::Ordering, f64),
    Category(String),
    MarkLabel(String),
    Encoded(bool),
    UsesComponent(String),
    Has(String),
}

fn parse_search_predicates(query: &str) -> Option<Vec<SearchPred>> {
    let mut preds = Vec::new();
    for term in query.split_whitespace() {
        let pred = if let Some(rest) = term.strip_prefix("w>") {
            SearchPred::Width(std::cmp::Ordering::Greater, rest.parse().ok()?)
        } else if let Some(rest) = term.strip_prefix("w<") {
            SearchPred::Width(std::cmp::Ordering::Less, rest.parse().ok()?)
        } else if let Some(rest) = term.strip_prefix("w=") {
            SearchPred::Width(std::cmp::Ordering::Equal, rest.parse().ok()?)
        } else if let Some(rest) = term.strip_prefix("cat:") {
            SearchPred::Category(rest.to_lowercase())
        } else if let Some(rest) = term.strip_prefix("mark:") {
            SearchPred::MarkLabel(rest.to_lowercase())
        } else if let Some(rest) = term.strip_prefix("enc:") {
            SearchPred::Encoded(matches!(rest, "yes" | "y" | "true"))
        } else if let Some(rest) = term.strip_prefix("comp:") {
            SearchPred::UsesComponent(rest.to_string())
        } else if let Some(rest) = term.strip_prefix("has:") {
            SearchPred::Has(rest.to_lowercase())
        } else {
            return None; // any plain term: not a predicate query
        };
        preds.push(pred);
    }
    (!preds.is_empty()).then_some(preds)
}

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

const COLOR_PALETTES_KEY: &str = "com.github.googlei18n.ufo2ft.colorPalettes";
const COLOR_LAYER_MAPPING_KEY: &str = "com.github.googlei18n.ufo2ft.colorLayerMapping";

/// The first palette: [r, g, b, a] float rows.
fn read_color_palette(font: &norad::Font) -> Vec<[f64; 4]> {
    let number = |v: &plist::Value| {
        v.as_real()
            .or_else(|| v.as_signed_integer().map(|n| n as f64))
    };
    font.lib
        .get(COLOR_PALETTES_KEY)
        .and_then(|v| v.as_array())
        .and_then(|palettes| palettes.first())
        .and_then(|p| p.as_array())
        .map(|colors| {
            colors
                .iter()
                .filter_map(|c| {
                    let arr = c.as_array()?;
                    let mut out = [0.0, 0.0, 0.0, 1.0];
                    for (i, v) in arr.iter().take(4).enumerate() {
                        out[i] = number(v)?;
                    }
                    Some(out)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn write_color_palette(font: &mut norad::Font, palette: &[[f64; 4]]) {
    let value = plist::Value::Array(vec![plist::Value::Array(
        palette
            .iter()
            .map(|c| plist::Value::Array(c.iter().map(|&v| plist::Value::Real(v)).collect()))
            .collect(),
    )]);
    font.lib.insert(COLOR_PALETTES_KEY.into(), value);
}

/// The font-level layer mapping: (layer name, palette index), bottom
/// layer first.
fn read_color_mapping(font: &norad::Font) -> Vec<(String, usize)> {
    font.lib
        .get(COLOR_LAYER_MAPPING_KEY)
        .and_then(|v| v.as_array())
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let arr = row.as_array()?;
                    let layer = arr.first()?.as_string()?.to_string();
                    let color = arr
                        .get(1)?
                        .as_signed_integer()
                        .or_else(|| arr.get(1)?.as_real().map(|v| v as i64))?;
                    Some((layer, color.max(0) as usize))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn write_color_mapping(font: &mut norad::Font, mapping: &[(String, usize)]) {
    if mapping.is_empty() {
        font.lib.remove(COLOR_LAYER_MAPPING_KEY);
        return;
    }
    let value = plist::Value::Array(
        mapping
            .iter()
            .map(|(layer, color)| {
                plist::Value::Array(vec![
                    plist::Value::String(layer.clone()),
                    plist::Value::Integer((*color as u64).into()),
                ])
            })
            .collect(),
    );
    font.lib.insert(COLOR_LAYER_MAPPING_KEY.into(), value);
}

/// Parse #RRGGBB or #RRGGBBAA (the # optional).
fn parse_hex_color(text: &str) -> Option<[f64; 4]> {
    let hex = text.trim().trim_start_matches('#');
    if hex.len() != 6 && hex.len() != 8 {
        return None;
    }
    let byte = |i: usize| {
        u8::from_str_radix(&hex[i..i + 2], 16)
            .ok()
            .map(|v| v as f64 / 255.0)
    };
    Some([
        byte(0)?,
        byte(2)?,
        byte(4)?,
        if hex.len() == 8 { byte(6)? } else { 1.0 },
    ])
}

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
const SIDEBAR_CATEGORIES: [(runebender_core::category::GlyphCategory, &str); 8] = {
    use runebender_core::category::GlyphCategory as GC;
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
    Category(runebender_core::category::GlyphCategory),
    Subfilter(runebender_core::category::GlyphCategory, &'static str),
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
    buffer: runebender_core::text::TextBuffer,
}

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
    preview_blur_cache: Arc<Mutex<Option<(u64, Arc<gpui::RenderImage>)>>>,
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
    edit_buffer: runebender_core::text::TextBuffer,
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
    coord_quadrant: runebender_core::path::Quadrant,
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
            runebender_core::measure::label(value)
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

impl Workspace {
    fn font(&self) -> Option<&FontModel> {
        self.project.as_ref().map(|p| p.active_font())
    }

    fn font_mut(&mut self) -> Option<&mut FontModel> {
        self.project.as_mut().map(|p| p.active_font_mut())
    }

    /// Switch the active master, keeping the open glyph (by name)
    /// when it exists in the target master.
    fn switch_master(&mut self, master: usize) {
        self.sidebar_counts = None;
        let Some(project) = self.project.as_mut() else {
            return;
        };
        if master >= project.masters.len() || master == project.active {
            return;
        }
        let open_glyph_name = match self.mode {
            Mode::Editor(i) => Some(project.active_font().glyphs[i].name.clone()),
            Mode::Grid => None,
        };
        project.active = master;
        project.snap_location_to_master(master);
        if let Some(name) = open_glyph_name {
            match project
                .active_font()
                .glyphs
                .iter()
                .position(|g| g.name == name)
            {
                Some(index) => self.open_editor(index),
                None => self.mode = Mode::Grid,
            }
        }
        self.rebuild_text_models();
    }

    /// Write the live editor state back into the active session's
    /// slot before switching to another tab.
    fn park_active_session(&mut self) {
        let glyph = match self.mode {
            Mode::Editor(i) => Some(i),
            Mode::Grid => self.last_editor,
        };
        let name = glyph
            .and_then(|i| self.font().and_then(|f| f.glyphs.get(i)))
            .map(|g| g.name.to_string());
        let Some(slot) = self.sessions.get_mut(self.active_session) else {
            return;
        };
        if let Some(name) = name {
            slot.glyph_name = name;
        }
        slot.editor = std::mem::replace(&mut self.editor, EditorState::new());
        slot.buffer = std::mem::replace(
            &mut self.edit_buffer,
            runebender_core::text::TextBuffer::new(),
        );
    }

    /// Switch to another edit tab, restoring its buffer, tool,
    /// selection, viewport, and undo stack as they were left.
    fn activate_session(&mut self, target: usize) {
        if target >= self.sessions.len() {
            return;
        }
        let switching = target != self.active_session;
        if switching {
            self.park_active_session();
            let slot = &mut self.sessions[target];
            self.editor = std::mem::replace(&mut slot.editor, EditorState::new());
            self.edit_buffer =
                std::mem::replace(&mut slot.buffer, runebender_core::text::TextBuffer::new());
            self.active_session = target;
        }
        let name = self.sessions[target].glyph_name.clone();
        let Some(&index) = self.font().and_then(|f| f.name_map.get(name.as_str())) else {
            // The glyph is gone (removed, or absent from this
            // master): drop the dead tab.
            self.close_session(target);
            return;
        };
        self.mode = Mode::Editor(index);
        self.selected = Some(index);
        self.last_editor = Some(index);
        self.status_note = None;
    }

    /// Close an edit tab. Closing the active one activates its
    /// neighbor; closing the last returns to the overview.
    fn close_session(&mut self, target: usize) {
        if target >= self.sessions.len() {
            return;
        }
        self.sessions.remove(target);
        if self.sessions.is_empty() {
            self.active_session = 0;
            self.editor = EditorState::new();
            self.edit_buffer = runebender_core::text::TextBuffer::new();
            self.last_editor = None;
            self.mode = Mode::Grid;
            return;
        }
        match target.cmp(&self.active_session) {
            std::cmp::Ordering::Less => self.active_session -= 1,
            std::cmp::Ordering::Equal => {
                // The live state belonged to the removed tab: load the
                // neighbor without parking.
                let next = target.min(self.sessions.len() - 1);
                let slot = &mut self.sessions[next];
                self.editor = std::mem::replace(&mut slot.editor, EditorState::new());
                self.edit_buffer =
                    std::mem::replace(&mut slot.buffer, runebender_core::text::TextBuffer::new());
                self.active_session = next;
                let name = self.sessions[next].glyph_name.clone();
                match self
                    .font()
                    .and_then(|f| f.name_map.get(name.as_str()))
                    .copied()
                {
                    Some(index) => {
                        if matches!(self.mode, Mode::Editor(_)) {
                            self.mode = Mode::Editor(index);
                        }
                        self.selected = Some(index);
                        self.last_editor = Some(index);
                    }
                    None => self.close_session(next),
                }
            }
            std::cmp::Ordering::Greater => {}
        }
    }

    fn open_editor(&mut self, index: usize) {
        // Opening from the grid lands in the active tab; the first
        // open creates it.
        if self.sessions.is_empty() {
            self.sessions.push(EditSession {
                glyph_name: String::new(),
                editor: EditorState::new(),
                buffer: runebender_core::text::TextBuffer::new(),
            });
            self.active_session = 0;
        }
        if let Some(name) = self
            .font()
            .and_then(|f| f.glyphs.get(index))
            .map(|g| g.name.to_string())
        {
            if let Some(slot) = self.sessions.get_mut(self.active_session) {
                slot.glyph_name = name;
            }
        }
        self.mode = Mode::Editor(index);
        // The info and colors sections follow the open glyph.
        self.selected = Some(index);
        self.seed_edit_buffer(index);
        self.editor.initialized = false;
        self.editor.selected.clear();
        self.editor.drag = None;
        self.editor.undo.clear();
        self.editor.redo.clear();
        self.editor.tool = Tool::Select;
        self.editor.pen = None;
        self.editor.hyper_contour = None;
        self.editor.selected_anchors.clear();
    }

    /// Solve the grid's cell size against the measured viewport, the
    /// way the web editor does: the zoom slider sets a *target* size,
    /// then columns are chosen to fill the width exactly and the row
    /// height divides the visible height evenly, so no row is left
    /// sliced in half at the bottom edge.
    fn grid_cell_metrics(&self) -> GridFit {
        // Detail mode needs room for the info lines: the cell floor
        // rises, whatever the zoom slider says.
        let size = if self.font_view_mode == FontViewMode::Detail {
            self.grid_cell_size.max(148.0)
        } else {
            self.grid_cell_size
        };
        Self::solve_grid(self.grid_viewport, size, GRID_PAD)
    }

    /// Same solve for the editor sidebar's mini grid, against its own
    /// narrower viewport.
    fn sidebar_cell_metrics(&self) -> GridFit {
        Self::solve_grid(self.sidebar_viewport, self.sidebar_cell_size, GRID_PAD_SM)
    }

    /// Scroll a row-quantized grid by a wheel delta. The offset is
    /// kept in whole rows, so a row is never left sliced at the top or
    /// bottom edge — the web got this from `scroll-snap-type`, which
    /// gpui has no equivalent for.
    fn scroll_grid_rows(
        offset: &mut usize,
        delta_y: f32,
        row_h: f32,
        rows_visible: usize,
        rows_total: usize,
    ) -> bool {
        let max = rows_total.saturating_sub(rows_visible);
        let step = (delta_y / row_h.max(1.0)).abs().ceil() as usize;
        let step = step.clamp(1, rows_visible.max(1));
        let next = if delta_y > 0.0 {
            offset.saturating_sub(step)
        } else {
            (*offset + step).min(max)
        };
        let changed = next != *offset;
        *offset = next;
        changed
    }

    fn solve_grid(viewport: gpui::Size<gpui::Pixels>, target: f32, pad: f32) -> GridFit {
        let label_h = |w: f32| cell_label_metrics(w).height;
        let target = target.max(24.0);
        let vw: f32 = viewport.width.into();
        let vh: f32 = viewport.height.into();
        if vw <= 0.0 || vh <= 0.0 {
            // First frame, before the probe reports: fall back to the
            // target size.
            return GridFit {
                cell_w: target,
                cell_h: target + label_h(target),
                cols: 1,
                rows: 1,
            };
        }
        let usable_w = (vw - pad * 2.0).max(target);
        let cols = (((usable_w + GRID_GAP) / (target + GRID_GAP)).floor() as usize).max(1);
        let cell_w = ((usable_w - GRID_GAP * (cols - 1) as f32) / cols as f32).floor();

        let target_row = cell_w + label_h(cell_w);
        let usable_h = (vh - pad.min(GRID_PAD_Y) * 2.0).max(target_row);
        let rows = (((usable_h + GRID_GAP) / (target_row + GRID_GAP)).round() as usize).max(1);
        let cell_h = ((usable_h - GRID_GAP * (rows - 1) as f32) / rows as f32).floor();
        GridFit {
            cell_w,
            cell_h,
            cols,
            rows,
        }
    }

    /// Left sidebar tile: search plus the category filter list,
    /// like runebender-web's CategorySidebar.
    /// All codepoints of a glyph in the active master (norad keeps
    /// the full list; GlyphEntry only caches the first).
    fn glyph_codepoints(font: &FontModel, name: &str) -> Vec<u32> {
        font.font
            .get_glyph(name)
            .map(|g| g.codepoints.iter().map(|c| c as u32).collect())
            .unwrap_or_default()
    }

    /// Does a glyph pass the given sidebar filter?
    fn glyph_passes_filter(
        &self,
        font: &FontModel,
        name: &str,
        codepoint: Option<char>,
        filter: &SidebarFilter,
    ) -> bool {
        use runebender_core::category::GlyphCategory as GC;
        use runebender_core::sidebar as sb;
        let category = codepoint.map(GC::from_codepoint).unwrap_or(GC::Other);
        match filter {
            SidebarFilter::All => true,
            SidebarFilter::Saved(si) => {
                let saved = read_saved_filters(&font.font);
                let Some((_, query)) = saved.get(*si) else {
                    return false;
                };
                match parse_search_predicates(query) {
                    Some(preds) => Self::glyph_matches_preds(font, name, codepoint, &preds),
                    None => name.contains(query.trim()),
                }
            }
            SidebarFilter::Category(c) => category == *c,
            SidebarFilter::Subfilter(c, sub) => {
                category == *c
                    && sb::glyph_matches_subfilter(name, &Self::glyph_codepoints(font, name), sub)
            }
            SidebarFilter::LanguageGroup(gi) => {
                sb::language_groups().get(*gi).is_some_and(|group| {
                    sb::glyph_matches_language_group(
                        name,
                        &Self::glyph_codepoints(font, name),
                        group,
                    )
                })
            }
            SidebarFilter::Language(gi, fi) => sb::language_groups()
                .get(*gi)
                .and_then(|group| group.filters.get(*fi))
                .is_some_and(|f| {
                    sb::glyph_matches_character_filter(name, &Self::glyph_codepoints(font, name), f)
                }),
            SidebarFilter::Builtin(bi) => {
                let Some(builtin) = sb::builtin_filters().get(*bi) else {
                    return false;
                };
                match &builtin.glyphset {
                    Some(set) => sb::glyph_matches_character_filter(
                        name,
                        &Self::glyph_codepoints(font, name),
                        set,
                    ),
                    // Runebender builtins: exporting = everything;
                    // incompatible = glyphs whose masters disagree.
                    None => match builtin.id.as_str() {
                        "incompatible" => self
                            .project
                            .as_ref()
                            .and_then(|p| p.compat.get(name))
                            .is_some_and(|ok| !ok),
                        _ => true,
                    },
                }
            }
        }
    }

    /// Rebuild the per-row counts and the current filter's match set.
    /// Called lazily from render after anything font-shaped changes.
    fn rebuild_sidebar_cache(&mut self) {
        use runebender_core::category::GlyphCategory as GC;
        use runebender_core::sidebar as sb;
        let Some(font) = self.font() else {
            self.sidebar_counts = None;
            self.sidebar_matches = None;
            return;
        };
        let glyphs: Vec<(String, Option<char>, Vec<u32>)> = font
            .glyphs
            .iter()
            .map(|entry| {
                (
                    entry.name.to_string(),
                    entry.codepoint,
                    Self::glyph_codepoints(font, entry.name.as_ref()),
                )
            })
            .collect();
        let categories = SIDEBAR_CATEGORIES
            .iter()
            .map(|(category, _)| {
                if *category == GC::All {
                    glyphs.len()
                } else {
                    glyphs
                        .iter()
                        .filter(|(_, cp, _)| {
                            cp.map(GC::from_codepoint).unwrap_or(GC::Other) == *category
                        })
                        .count()
                }
            })
            .collect();
        let mut subfilters = std::collections::HashMap::new();
        for (ci, (category, label)) in SIDEBAR_CATEGORIES.iter().enumerate() {
            for (si, (sub, _)) in sb::category_subfilters(label).iter().enumerate() {
                let count = glyphs
                    .iter()
                    .filter(|(name, cp, cps)| {
                        cp.map(GC::from_codepoint).unwrap_or(GC::Other) == *category
                            && sb::glyph_matches_subfilter(name, cps, sub)
                    })
                    .count();
                subfilters.insert((ci, si), count);
            }
        }
        let name_cps: Vec<(String, Vec<u32>)> = glyphs
            .iter()
            .map(|(name, _, cps)| (name.clone(), cps.clone()))
            .collect();
        let mut groups = Vec::new();
        let mut languages = Vec::new();
        let mut missing = Vec::new();
        for group in sb::language_groups() {
            groups.push(
                glyphs
                    .iter()
                    .filter(|(name, _, cps)| sb::glyph_matches_language_group(name, cps, group))
                    .count(),
            );
            languages.push(
                group
                    .filters
                    .iter()
                    .map(|filter| {
                        glyphs
                            .iter()
                            .filter(|(name, _, cps)| {
                                sb::glyph_matches_character_filter(name, cps, filter)
                            })
                            .count()
                    })
                    .collect(),
            );
            missing.push(
                group
                    .filters
                    .iter()
                    .map(|filter| sb::missing_targets(&name_cps, filter).len())
                    .collect(),
            );
        }
        let builtins = sb::builtin_filters()
            .iter()
            .map(|builtin| match &builtin.glyphset {
                Some(set) => glyphs
                    .iter()
                    .filter(|(name, _, cps)| sb::glyph_matches_character_filter(name, cps, set))
                    .count(),
                None => match builtin.id.as_str() {
                    "incompatible" => self
                        .project
                        .as_ref()
                        .map(|p| p.compat.values().filter(|ok| !**ok).count())
                        .unwrap_or(0),
                    _ => glyphs.len(),
                },
            })
            .collect();
        let saved = self
            .font()
            .map(|font| {
                read_saved_filters(&font.font)
                    .iter()
                    .map(|(_, query)| {
                        let preds = parse_search_predicates(query);
                        glyphs
                            .iter()
                            .filter(|(name, cp, _)| match &preds {
                                Some(preds) => Self::glyph_matches_preds(font, name, *cp, preds),
                                None => name.contains(query.trim()),
                            })
                            .count()
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.sidebar_counts = Some(SidebarCounts {
            total: glyphs.len(),
            categories,
            subfilters,
            groups,
            languages,
            missing,
            builtins,
            saved,
        });
        self.rebuild_sidebar_matches();
    }

    /// Recompute the current filter's match set only (filter clicks).
    fn rebuild_sidebar_matches(&mut self) {
        let filter = self.sidebar_filter.clone();
        if filter == SidebarFilter::All {
            self.sidebar_matches = None;
            return;
        }
        let Some(font) = self.font() else {
            self.sidebar_matches = None;
            return;
        };
        let matches: std::collections::HashSet<String> = font
            .glyphs
            .iter()
            .filter(|entry| {
                self.glyph_passes_filter(font, entry.name.as_ref(), entry.codepoint, &filter)
            })
            .map(|entry| entry.name.to_string())
            .collect();
        self.sidebar_matches = Some(matches);
    }

    /// The grid's visible order (same filter + sort the grid draws).
    fn visible_grid_indices(&self) -> Vec<usize> {
        let Some(font) = self.font() else {
            return Vec::new();
        };
        let mut indices: Vec<usize> = (0..font.glyphs.len())
            .filter(|&i| {
                let entry = &font.glyphs[i];
                self.sidebar_matches
                    .as_ref()
                    .is_none_or(|m| m.contains(entry.name.as_ref()))
                    && self.search_matches(entry.name.as_ref(), entry.codepoint)
            })
            .collect();
        if !self.sort_unicode {
            indices.sort_by_key(|&i| font.glyphs[i].name.clone());
        }
        indices
    }

    /// Cmd-click: toggle a glyph in the multi-selection.
    fn grid_toggle_multi(&mut self, index: usize) {
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        if let Some(primary) = self.selected {
            if let Some(primary_name) = self.font().map(|f| f.glyphs[primary].name.to_string()) {
                self.multi_selected.insert(primary_name);
            }
        }
        if !self.multi_selected.remove(&name) {
            self.multi_selected.insert(name);
        }
        self.selected = Some(index);
    }

    /// Shift-click: extend from the primary through the visible order.
    fn grid_extend_multi(&mut self, index: usize) {
        let order = self.visible_grid_indices();
        let Some(primary) = self.selected else {
            self.selected = Some(index);
            return;
        };
        let (Some(a), Some(b)) = (
            order.iter().position(|&i| i == primary),
            order.iter().position(|&i| i == index),
        ) else {
            self.selected = Some(index);
            return;
        };
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let names: Vec<String> = self
            .font()
            .map(|font| {
                order[lo..=hi]
                    .iter()
                    .map(|&i| font.glyphs[i].name.to_string())
                    .collect()
            })
            .unwrap_or_default();
        self.multi_selected.extend(names);
    }

    /// Every selected glyph name (primary plus multi), in font order.
    fn selection_names(&self) -> Vec<String> {
        let Some(font) = self.font() else {
            return Vec::new();
        };
        let mut names: Vec<String> = font
            .glyphs
            .iter()
            .filter(|entry| {
                self.multi_selected.contains(entry.name.as_ref())
                    || self
                        .selected
                        .is_some_and(|i| font.glyphs[i].name == entry.name)
            })
            .map(|entry| entry.name.to_string())
            .collect();
        names.dedup();
        names
    }

    /// Does a glyph match the sidebar search, honoring scope, regex,
    /// and case options (web glyphMatchesSidebarSearch)?
    /// Evaluate a parsed predicate list against one glyph. Shared by
    /// the search field and saved sidebar filters.
    fn glyph_matches_preds(
        font: &FontModel,
        name: &str,
        codepoint: Option<char>,
        preds: &[SearchPred],
    ) -> bool {
        let Some(&index) = font.name_map.get(name) else {
            return false;
        };
        let entry = &font.glyphs[index];
        preds.iter().all(|pred| match pred {
            SearchPred::Width(order, value) => {
                let diff = entry.advance - value;
                match order {
                    std::cmp::Ordering::Greater => diff > 0.5,
                    std::cmp::Ordering::Less => diff < -0.5,
                    std::cmp::Ordering::Equal => diff.abs() <= 0.5,
                }
            }
            SearchPred::Category(want) => codepoint
                .map(|c| {
                    runebender_core::category::GlyphCategory::from_codepoint(c)
                        .display_name()
                        .to_lowercase()
                        .starts_with(want.as_str())
                })
                .unwrap_or(want == "unencoded"),
            SearchPred::MarkLabel(want) => match entry.mark.as_deref() {
                Some(label) => label.to_lowercase() == *want,
                None => want == "none",
            },
            SearchPred::Encoded(want) => codepoint.is_some() == *want,
            SearchPred::UsesComponent(base) => font
                .font
                .get_glyph(name)
                .is_some_and(|g| g.components.iter().any(|c| c.base.as_str() == base)),
            SearchPred::Has(what) => {
                font.font
                    .get_glyph(name)
                    .is_some_and(|g| match what.as_str() {
                        "contours" => !g.contours.is_empty(),
                        "components" => !g.components.is_empty(),
                        "anchors" => !g.anchors.is_empty(),
                        "note" => g.note.is_some(),
                        _ => false,
                    })
            }
        })
    }

    fn search_matches(&self, name: &str, codepoint: Option<char>) -> bool {
        let query = self.search_query.trim();
        if query.is_empty() {
            return true;
        }
        // Predicate queries filter on glyph data (all terms must
        // hold); anything else falls through to text search.
        if let Some(preds) = &self.search_predicates {
            let Some(font) = self.font() else { return true };
            return Self::glyph_matches_preds(font, name, codepoint, preds);
        }
        // Only build the codepoint haystacks the mode actually reads.
        let hex;
        let chars;
        let haystacks: [&str; 3] = match self.search_mode {
            1 => [name, "", ""],
            2 => {
                hex = codepoint
                    .map(|c| format!("{:04X}", c as u32))
                    .unwrap_or_default();
                chars = codepoint.map(String::from).unwrap_or_default();
                ["", hex.as_str(), chars.as_str()]
            }
            _ => {
                hex = codepoint
                    .map(|c| format!("{:04X}", c as u32))
                    .unwrap_or_default();
                chars = codepoint.map(String::from).unwrap_or_default();
                [name, hex.as_str(), chars.as_str()]
            }
        };
        let any = |f: &dyn Fn(&str) -> bool| haystacks.iter().any(|h| !h.is_empty() && f(h));
        if self.search_regex {
            // Compiled once when the query changed, not per glyph: a
            // font-wide filter used to build 862 regexes a frame.
            return match &self.search_re {
                Some(re) => any(&|h| re.is_match(h)),
                // A half-typed pattern matches everything, like the web.
                None => true,
            };
        }
        if self.search_case {
            any(&|h| h.contains(query))
        } else {
            let needle = query.to_lowercase();
            any(&|h| h.to_lowercase().contains(&needle))
        }
    }

    /// The glyphs to show, filtered and sorted, from cache when the
    /// inputs have not moved.
    fn visible_glyphs(&mut self) -> Arc<Vec<usize>> {
        let key = OrderKey {
            query: self.search_query.clone(),
            mode: self.search_mode,
            regex: self.search_regex,
            case: self.search_case,
            sort_unicode: self.sort_unicode,
            filter: self.sidebar_filter.clone(),
            revision: self.font().map(|f| f.revision).unwrap_or(0),
            master: self.project.as_ref().map(|p| p.active).unwrap_or(0),
        };
        if self.order_key.as_ref() == Some(&key)
            && let Some(order) = &self.glyph_order
        {
            return order.clone();
        }
        let matches = self.sidebar_matches.clone();
        let order: Vec<usize> = match self.font() {
            Some(font) => {
                let mut indices: Vec<usize> = (0..font.glyphs.len())
                    .filter(|&i| {
                        let entry = &font.glyphs[i];
                        matches
                            .as_ref()
                            .is_none_or(|m| m.contains(entry.name.as_ref()))
                            && self.search_matches(entry.name.as_ref(), entry.codepoint)
                    })
                    .collect();
                if !self.sort_unicode {
                    // Font order is already unicode order, so the Name
                    // toggle sorts alphabetically.
                    indices.sort_by(|a, b| font.glyphs[*a].name.cmp(&font.glyphs[*b].name));
                }
                indices
            }
            None => Vec::new(),
        };
        let order = Arc::new(order);
        self.glyph_order = Some(order.clone());
        self.order_key = Some(key);
        order
    }

    /// The cached order, for the panels that only hold `&self`.
    /// `render` refreshes it once a frame before any of them run.
    fn glyph_order(&self) -> Arc<Vec<usize>> {
        self.glyph_order.clone().unwrap_or_default()
    }

    /// Recompile the search pattern. Called when the query or the
    /// case flag changes.
    fn rebuild_search_regex(&mut self) {
        self.search_re = None;
        let query = self.search_query.trim();
        self.search_predicates = parse_search_predicates(query);
        if !self.search_regex || query.is_empty() {
            return;
        }
        let pattern = if self.search_case {
            query.to_string()
        } else {
            format!("(?i){query}")
        };
        self.search_re = regex::Regex::new(&pattern).ok();
    }

    /// Pin the current search query as a saved filter in the font lib.
    fn save_current_search_as_filter(&mut self) {
        let query = self.search_query.trim().to_string();
        if query.is_empty() {
            return;
        }
        let Some(font) = self.font_mut() else { return };
        let mut saved = read_saved_filters(&font.font);
        if saved.iter().any(|(_, q)| *q == query) {
            return;
        }
        saved.push((query.clone(), query));
        write_saved_filters(&mut font.font, &saved);
        font.dirty = true;
        let index = saved.len() - 1;
        self.sidebar_counts = None;
        self.set_sidebar_filter(SidebarFilter::Saved(index));
    }

    /// Remove one saved filter, keeping the selection sensible.
    fn delete_saved_filter(&mut self, si: usize) {
        let Some(font) = self.font_mut() else { return };
        let mut saved = read_saved_filters(&font.font);
        if si >= saved.len() {
            return;
        }
        saved.remove(si);
        write_saved_filters(&mut font.font, &saved);
        font.dirty = true;
        self.sidebar_counts = None;
        match self.sidebar_filter {
            SidebarFilter::Saved(active) if active == si => {
                self.set_sidebar_filter(SidebarFilter::All);
            }
            SidebarFilter::Saved(active) if active > si => {
                self.set_sidebar_filter(SidebarFilter::Saved(active - 1));
            }
            _ => self.rebuild_sidebar_matches(),
        }
    }

    /// Select a sidebar row.
    fn set_sidebar_filter(&mut self, filter: SidebarFilter) {
        self.sidebar_filter = filter;
        // A different set of glyphs starts at the top.
        self.grid_scroll_row = 0;
        self.rebuild_sidebar_matches();
    }

    /// A small disclosure triangle for expandable sidebar rows
    /// (painted: IBM Plex has no triangle codepoints).
    fn row_chevron(expanded: bool) -> impl IntoElement {
        canvas(
            move |bounds, _, _| bounds,
            move |_, bounds: Bounds<gpui::Pixels>, window, _| {
                let o = bounds.origin;
                let w: f32 = bounds.size.width.into();
                let h: f32 = bounds.size.height.into();
                let (cx_, cy) = (w / 2.0, h / 2.0);
                let mut path = gpui::PathBuilder::fill();
                let pt = |dx: f32, dy: f32| gpui::point(o.x + px(cx_ + dx), o.y + px(cy + dy));
                if expanded {
                    path.move_to(pt(-3.5, -1.5));
                    path.line_to(pt(3.5, -1.5));
                    path.line_to(pt(0.0, 2.5));
                } else {
                    path.move_to(pt(-1.5, -3.5));
                    path.line_to(pt(2.5, 0.0));
                    path.line_to(pt(-1.5, 3.5));
                }
                if let Ok(p) = path.build() {
                    window.paint_path(p, t::text_muted());
                }
            },
        )
        .w(px(10.0))
        .h(px(10.0))
    }

    /// One sidebar row: optional chevron, optional icon, label, and a
    /// right-aligned count ("n" or "n/m" coverage).
    #[allow(clippy::too_many_arguments)]
    fn sidebar_row(
        &self,
        id: (&'static str, usize),
        indent: bool,
        chevron: Option<bool>,
        icon: Option<SharedString>,
        label: SharedString,
        count: SharedString,
        filter: SidebarFilter,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let active = self.sidebar_filter == filter;
        div()
            .id(id)
            // Fixed row height, no vertical padding: Glyphs' sidebar
            // packs its rows tight, and leading is what made ours look
            // twice as tall as it needed to be.
            .h(px(20.0))
            .px_2()
            .when(indent, |el| el.ml_3())
            .rounded(t::radius())
            .text_sm()
            .cursor_pointer()
            .flex()
            .items_center()
            .gap_1()
            .when(active, |el| {
                el.border(t::stroke())
                    .border_color(t::accent())
                    .text_color(t::accent())
            })
            .when(!active, |el| el.text_color(t::text()))
            .when_some(chevron, |el, expanded| {
                el.child(Self::row_chevron(expanded))
            })
            .when_some(icon, |el, icon| {
                el.child(
                    div()
                        .w(px(16.0))
                        .text_color(if active { t::accent() } else { t::text_muted() })
                        .child(icon),
                )
            })
            .child(div().flex_1().child(label))
            .child(
                div()
                    .text_color(if active { t::accent() } else { t::text_muted() })
                    .child(count),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_sidebar_filter(filter.clone());
                cx.notify();
            }))
    }

    /// A tiny toggle beside the search box (scope / regex / case).
    fn search_toggle(
        &self,
        id: &'static str,
        label: &'static str,
        active: bool,
        on: fn(&mut Self),
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .w(px(24.0))
            // No fixed height: the row stretches these to the search
            // input's height so the whole strip lines up.
            .rounded(t::radius())
            .border(t::stroke())
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .cursor_pointer()
            .when(active, |el| {
                el.border_color(t::accent()).text_color(t::accent())
            })
            .when(!active, |el| {
                el.border_color(t::cell_border())
                    .text_color(t::text_muted())
            })
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                on(this);
                cx.notify();
            }))
    }

    /// Set or clear the mark color on every selected glyph.
    fn set_selected_mark(&mut self, label: Option<String>) {
        let names = self.selection_names();
        if names.is_empty() {
            return;
        }
        let Some(font) = self.font_mut() else { return };
        for name in names {
            if let Some(&index) = font.name_map.get(&name) {
                font.edit_glyph(index, |glyph| {
                    runebender_core::theme_oklch::set_glyph_mark(glyph, label.as_deref());
                });
            }
        }
    }

    /// The Shapes tab: one row per contour and per component in the
    /// open glyph, like the web's sidebar. A row selects what it names.
    fn sidebar_shapes(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut list = div().flex().flex_col().gap_1().p_2();
        let (Mode::Editor(index), Some(font)) = (&self.mode, self.font()) else {
            return list.child(
                div()
                    .text_xs()
                    .text_color(t::text_muted())
                    .child("No glyph open."),
            );
        };
        let index = *index;
        let entry = &font.glyphs[index];
        let Some(glyph) = font.font.get_glyph(entry.name.as_ref()) else {
            return list;
        };
        let row = |id: (&'static str, usize),
                   mark: &'static str,
                   label: SharedString,
                   detail: SharedString,
                   active: bool| {
            div()
                .id(id)
                .h(px(20.0))
                .px_1()
                .flex()
                .items_center()
                .gap_2()
                .rounded(t::radius())
                .text_xs()
                .cursor_pointer()
                .when(active, |el| {
                    el.bg(t::cell_selected_bg()).text_color(t::text())
                })
                .when(!active, |el| el.text_color(t::text_muted()))
                .child(div().w(px(10.0)).child(mark))
                .child(div().flex_1().child(label))
                .child(div().text_color(t::text_muted()).child(detail))
        };

        let counts: Vec<usize> = glyph.contours.iter().map(|c| c.points.len()).collect();
        for (ci, points) in counts.iter().copied().enumerate() {
            let selected = self
                .editor
                .selected
                .iter()
                .any(|(contour, _)| *contour == ci);
            list = list.child(
                row(
                    ("shape-contour", ci),
                    "◌",
                    format!("contour {}", ci + 1).into(),
                    format!("{points} nodes").into(),
                    selected,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let Mode::Editor(index) = this.mode else {
                        return;
                    };
                    this.editor.selected_component = None;
                    this.editor.selected = this
                        .font()
                        .map(|f| {
                            f.glyphs[index]
                                .points
                                .iter()
                                .filter(|p| p.contour == ci)
                                .map(|p| (p.contour, p.index))
                                .collect()
                        })
                        .unwrap_or_default();
                    cx.notify();
                })),
            );
        }
        let bases: Vec<String> = glyph
            .components
            .iter()
            .map(|c| c.base.to_string())
            .collect();
        for (i, base) in bases.into_iter().enumerate() {
            let selected = self.editor.selected_component == Some(i);
            list = list.child(
                row(
                    ("shape-component", i),
                    "◇",
                    base.into(),
                    "component".into(),
                    selected,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.editor.selected.clear();
                    this.editor.selected_component = Some(i);
                    cx.notify();
                })),
            );
        }
        if counts.is_empty() && glyph.components.is_empty() {
            list = list.child(
                div()
                    .text_xs()
                    .text_color(t::text_muted())
                    .child("No shapes in this glyph yet."),
            );
        }
        list
    }

    /// The glyph editor: metrics lines, stroked outline over a dim
    /// fill, draggable control points, wheel pan, Cmd+wheel zoom.
    /// A flat docked sidebar section: small muted header with a
    /// disclosure triangle, hairline divider below (Glyphs-style, no
    /// floating container). Clicking the header folds the body.
    fn section(
        &self,
        cx: &mut Context<Self>,
        title: &'static str,
        body: impl IntoElement,
    ) -> gpui::Div {
        let collapsed = self.collapsed_sections.contains(title);
        div()
            .flex()
            .flex_col()
            .gap_1()
            .px_2()
            .py_1p5()
            .border_b_1()
            .border_color(t::panel_outline())
            .child(
                div()
                    .id(gpui::SharedString::from(format!("section-{title}")))
                    .flex()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .text_xs()
                    .text_color(t::text_muted())
                    .child(
                        canvas(
                            move |bounds, _, _| bounds,
                            move |_, bounds: Bounds<gpui::Pixels>, window, _| {
                                let o = bounds.origin;
                                let w: f32 = bounds.size.width.into();
                                let h: f32 = bounds.size.height.into();
                                let (cx_, cy) = (w / 2.0, h / 2.0);
                                let mut path = gpui::PathBuilder::fill();
                                let pt = |dx: f32, dy: f32| {
                                    gpui::point(o.x + px(cx_ + dx), o.y + px(cy + dy))
                                };
                                if collapsed {
                                    path.move_to(pt(-1.5, -3.5));
                                    path.line_to(pt(2.5, 0.0));
                                    path.line_to(pt(-1.5, 3.5));
                                } else {
                                    path.move_to(pt(-3.5, -1.5));
                                    path.line_to(pt(3.5, -1.5));
                                    path.line_to(pt(0.0, 2.5));
                                }
                                if let Ok(p) = path.build() {
                                    window.paint_path(p, t::text_muted());
                                }
                            },
                        )
                        .w(px(10.0))
                        .h(px(10.0)),
                    )
                    .child(title)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.collapsed_sections.remove(title) {
                            this.collapsed_sections.insert(title);
                        }
                        cx.notify();
                    })),
            )
            .when(!collapsed, |el| el.child(body))
    }

    /// A 30px icon tile (header tools, transform section).
    fn icon_tile(id: &'static str, icon: &'static str, active: bool) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .w(px(30.0))
            .h(px(30.0))
            .rounded(t::radius_control())
            .cursor_pointer()
            .when(active, |el| el.bg(t::cell_selected_bg()))
            .child(icon_svg(icon, if active { t::accent() } else { t::text() }))
    }

    /// Tool icons for the header bar (editor mode only).
    fn header_tools(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let tool = self.editor.tool;
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                Self::icon_tile("tool-select", "select", tool == Tool::Select).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.pen_finish();
                        this.editor.tool = Tool::Select;
                        cx.notify();
                    }),
                ),
            )
            .child(
                Self::icon_tile("tool-pen", "pen", tool == Tool::Pen).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.editor.tool = Tool::Pen;
                        cx.notify();
                    },
                )),
            )
            .child(
                Self::icon_tile(
                    "tool-shapes",
                    if self.editor.shape_ellipse {
                        "shape-ellipse"
                    } else {
                        "shape-rectangle"
                    },
                    tool == Tool::Shapes,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.editor.tool == Tool::Shapes {
                        this.editor.shape_ellipse = !this.editor.shape_ellipse;
                    }
                    this.pen_finish();
                    this.editor.tool = Tool::Shapes;
                    cx.notify();
                })),
            )
            .child(
                Self::icon_tile("tool-measure", "measure", tool == Tool::Measure).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.pen_finish();
                        this.editor.tool = Tool::Measure;
                        cx.notify();
                    }),
                ),
            )
            .child(
                Self::icon_tile("tool-text", "text", tool == Tool::Text).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.pen_finish();
                        this.editor.tool = Tool::Text;
                        cx.notify();
                    },
                )),
            )
            .child(
                Self::icon_tile("tool-knife", "knife", tool == Tool::Knife).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.pen_finish();
                        this.editor.tool = Tool::Knife;
                        cx.notify();
                    },
                )),
            )
            .child(
                Self::icon_tile("tool-hyperpen", "hyperpen", tool == Tool::HyperPen).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.pen_finish();
                        this.editor.tool = Tool::HyperPen;
                        cx.notify();
                    }),
                ),
            )
            .child(
                Self::icon_tile("tool-preview", "preview", tool == Tool::Preview).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.pen_finish();
                        if this.editor.tool == Tool::Preview {
                            this.editor.tool = this.editor.previous_tool;
                        } else {
                            this.editor.previous_tool = this.editor.tool;
                            this.editor.tool = Tool::Preview;
                        }
                        cx.notify();
                    }),
                ),
            )
    }

    /// Text direction control (text tool): LTR / RTL / Auto, like
    /// the web editor's TextDirectionToolbar.
    fn direction_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        use runebender_core::text::TextDirection;
        let auto = self.edit_buffer.direction_is_auto();
        let dir = self.edit_buffer.direction();
        let button = |id: &'static str, label: &'static str, active: bool| {
            div()
                .id(id)
                .px_2()
                .py_0p5()
                .rounded(t::radius())
                .border(t::stroke())
                .border_color(if active {
                    t::accent()
                } else {
                    t::cell_border()
                })
                .text_sm()
                .text_color(if active { t::accent() } else { t::text_muted() })
                .cursor_pointer()
                .child(label)
        };
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                button("dir-ltr", "LTR", !auto && dir == TextDirection::LeftToRight).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.edit_buffer
                            .set_direction(runebender_core::text::TextDirection::LeftToRight);
                        this.edit_buffer.shape_arabic_if_rtl();
                        this.sync_sort_offset();
                        cx.notify();
                    }),
                ),
            )
            .child(
                button("dir-rtl", "RTL", !auto && dir == TextDirection::RightToLeft).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.edit_buffer
                            .set_direction(runebender_core::text::TextDirection::RightToLeft);
                        this.edit_buffer.shape_arabic_if_rtl();
                        this.sync_sort_offset();
                        cx.notify();
                    }),
                ),
            )
            .child(
                button("dir-auto", "Auto", auto).on_click(cx.listener(|this, _, _, cx| {
                    this.edit_buffer.set_auto_direction();
                    this.edit_buffer.shape_arabic_if_rtl();
                    this.sync_sort_offset();
                    cx.notify();
                })),
            )
    }

    /// The open glyph in the editor, or the grid selection.
    fn current_glyph_index(&self) -> Option<usize> {
        match self.mode {
            Mode::Editor(index) => Some(index),
            Mode::Grid => self.selected,
        }
    }

    /// Non-default, non-background layers of the active master that
    /// hold a copy of `name`.
    fn glyph_layer_names(font: &norad::Font, name: &str) -> Vec<String> {
        font.layers
            .iter()
            .filter(|l| !l.is_default())
            .filter(|l| {
                let ln = l.name().as_str();
                ln != "public.background" && ln != "background"
            })
            .filter(|l| l.contains_glyph(name))
            .map(|l| l.name().to_string())
            .collect()
    }

    /// Set a metrics key on the selected glyph (every master keeps
    /// the same key; the values differ per master when synced).
    fn apply_metrics_key(&mut self, left: bool, text: &str) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let name = project.active_font().glyphs[index].name.to_string();
        let text = text.trim().to_string();
        if !text.is_empty() && parse_metrics_key(&text).is_none() {
            self.status_note = Some("Metrics key: =glyph, =|glyph, =glyph+10, or =50".into());
            return;
        }
        for master in project.masters.iter_mut() {
            if let Some(glyph) = master.font.get_glyph_mut(name.as_str()) {
                write_metrics_key(glyph, left, &text);
                master.dirty = true;
                master.modified_glyphs.insert(name.clone());
            }
        }
        self.command_sync_metrics();
    }

    /// Commit a dragged intermediate point: store it in the glyph's
    /// HOI lib key (dragging back onto the linear middle clears it),
    /// then rebake the brace layers so every consumer follows.
    fn commit_hoi_intermediate(&mut self, id: (usize, usize), q: (f64, f64)) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let Some((lo, hi)) = project.axis_end_masters() else {
            return;
        };
        let name = project.active_font().glyphs[index].name.to_string();
        let linear_mid = {
            let a = project.masters[lo]
                .font
                .get_glyph(name.as_str())
                .and_then(|g| g.contours.get(id.0))
                .and_then(|c| c.points.get(id.1))
                .map(|p| (p.x, p.y));
            let b = project.masters[hi]
                .font
                .get_glyph(name.as_str())
                .and_then(|g| g.contours.get(id.0))
                .and_then(|c| c.points.get(id.1))
                .map(|p| (p.x, p.y));
            match (a, b) {
                (Some(a), Some(b)) => ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0),
                _ => return,
            }
        };
        {
            let master = &mut project.masters[lo];
            let Some(glyph) = master.font.get_glyph_mut(name.as_str()) else {
                return;
            };
            let mut map = read_hoi_intermediates(glyph);
            let back_to_linear =
                ((q.0 - linear_mid.0).powi(2) + (q.1 - linear_mid.1).powi(2)).sqrt() < 3.0;
            if back_to_linear {
                map.remove(&id);
            } else {
                map.insert(id, (q.0.round(), q.1.round()));
            }
            write_hoi_intermediates(glyph, &map);
            master.dirty = true;
            master.modified_glyphs.insert(name.clone());
        }
        self.bake_hoi();
    }

    /// Rebake the HOI brace layers for the open glyph: stops at
    /// t = 0.25 / 0.5 / 0.75 of the first axis, curved nodes on
    /// their quadratic, the rest linear — standard sparse sources
    /// out, so fontc and fontmake follow the curves exactly enough.
    fn bake_hoi(&mut self) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let Some((lo, hi)) = project.axis_end_masters() else {
            return;
        };
        let Some(axis) = project.axes.first().cloned() else {
            return;
        };
        let name = project.active_font().glyphs[index].name.to_string();
        let (lo_glyph, hi_glyph, curves) = {
            let a = project.masters[lo].font.get_glyph(name.as_str()).cloned();
            let b = project.masters[hi].font.get_glyph(name.as_str()).cloned();
            let (Some(a), Some(b)) = (a, b) else { return };
            let curves = read_hoi_intermediates(&a);
            (a, b, curves)
        };
        let filename = project.masters[lo]
            .source_path
            .file_name()
            .map(|f| f.to_string_lossy().to_string());
        let Some(filename) = filename else { return };
        for &t in &[0.25_f64, 0.5, 0.75] {
            let design = (axis.min + (axis.max - axis.min) * t).round();
            let layer_name = format!("{{{design:.0}}}");
            if curves.is_empty() {
                // Cleared: drop our baked copies.
                if let Some(layer) = project.masters[lo].font.layers.get_mut(&layer_name) {
                    layer.remove_glyph(name.as_str());
                }
                project.masters[lo].dirty = true;
                continue;
            }
            let mut baked = lo_glyph.clone();
            baked.width = lo_glyph.width + (hi_glyph.width - lo_glyph.width) * t;
            for (ci, contour) in baked.contours.iter_mut().enumerate() {
                for (pi, point) in contour.points.iter_mut().enumerate() {
                    let Some(pb) = hi_glyph.contours.get(ci).and_then(|c| c.points.get(pi)) else {
                        continue;
                    };
                    let a = (point.x, point.y);
                    let b = (pb.x, pb.y);
                    let pos = match curves.get(&(ci, pi)) {
                        Some(&q) => hoi_quad_at(a, b, q, t),
                        None => (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t),
                    };
                    point.x = pos.0.round();
                    point.y = pos.1.round();
                }
            }
            let master = &mut project.masters[lo];
            if let Ok(layer) = master.font.layers.get_or_create_layer(&layer_name) {
                layer.insert_glyph(baked);
                master.dirty = true;
                master.modified_glyphs.insert(name.clone());
            }
            // Register the sparse source once.
            let registered = project.ds_doc.as_ref().is_some_and(|doc| {
                doc.sources.iter().any(|src| {
                    src.layer.as_deref() == Some(layer_name.as_str()) && src.filename == filename
                })
            });
            if !registered {
                if let Some(doc) = project.ds_doc.as_mut() {
                    doc.sources.push(norad::designspace::Source {
                        name: Some(format!("hoi {layer_name}")),
                        filename: filename.clone(),
                        layer: Some(layer_name.clone()),
                        location: vec![norad::designspace::Dimension {
                            name: axis.name.clone(),
                            xvalue: Some(design as f32),
                            ..Default::default()
                        }],
                        ..Default::default()
                    });
                    project.ds_dirty = true;
                }
                let mut location = runebender_core::var_model::Location::new();
                location.insert(
                    axis.name.clone(),
                    runebender_core::var_model::normalize_value(
                        design,
                        axis.min,
                        axis.default,
                        axis.max,
                    ),
                );
                project.brace.push(BraceSource {
                    master: lo,
                    layer: layer_name.clone(),
                    location,
                });
            }
        }
        self.status_note = Some(
            if curves.is_empty() {
                format!("{name}: interpolation back to linear")
            } else {
                format!(
                    "{name}: {} curved node path{} baked",
                    curves.len(),
                    if curves.len() == 1 { "" } else { "s" }
                )
            }
            .into(),
        );
    }

    /// The background layer we read: public.background first, then
    /// RoboFont's conventional plain "background".
    fn background_layer_name(font: &norad::Font) -> Option<String> {
        for candidate in ["public.background", "background"] {
            if font.layers.get(candidate).is_some() {
                return Some(candidate.to_string());
            }
        }
        None
    }

    /// Measure-tool HUD layer toggles (web SelectPanel): only shown
    /// while the Measure tool is active.

    fn ensure_editor_fit(&mut self) {
        if self.editor.initialized {
            return;
        }
        let Mode::Editor(index) = self.mode else {
            return;
        };
        let Some(font) = self.font() else {
            return;
        };
        let entry = &font.glyphs[index];
        let (advance, asc, desc) = (entry.advance, font.ascender, font.descender);
        self.editor.fit(advance, asc, desc);
    }

    /// Right-click on the canvas: build the web-style context menu
    /// for whatever is under the cursor.
    fn editor_context_menu(&mut self, pos: Point<gpui::Pixels>) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        let Some(font) = self.font() else { return };
        let (dx, dy) = self.editor.window_to_design(pos);
        let tolerance = 16.0 / self.editor.zoom().max(1e-6);
        let entry = &font.glyphs[index];
        let anchor = entry
            .anchors
            .iter()
            .enumerate()
            .map(|(i, (_, x, y))| (((x - dx).powi(2) + (y - dy).powi(2)).sqrt(), i))
            .filter(|(dist, _)| *dist <= tolerance)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, i)| i);
        let norad_glyph = font.font.get_glyph(entry.name.as_ref());
        let component = if anchor.is_none() {
            norad_glyph.and_then(|g| {
                runebender_core::glyph_ops::component_at(&font.font, g, kurbo::Point::new(dx, dy))
                    .map(|ci| {
                        let aligned = !runebender_core::composites::component_alignment_disabled(
                            &g.components[ci],
                        );
                        (ci, aligned)
                    })
            })
        } else {
            None
        };
        // The nearest on-curve point (for Set Start Point) and its
        // contour; a segment hit supplies the contour otherwise.
        let start_point = entry
            .points
            .iter()
            .filter(|p| p.on_curve)
            .map(|p| {
                (
                    ((p.x - dx).powi(2) + (p.y - dy).powi(2)).sqrt(),
                    (p.contour, p.index),
                )
            })
            .filter(|(dist, _)| *dist <= tolerance)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id);
        let contour = start_point.map(|(ci, _)| ci).or_else(|| {
            norad_glyph
                .and_then(|g| {
                    runebender_core::segment_ops::nearest_segment_with_t(
                        g,
                        kurbo::Point::new(dx, dy),
                        tolerance,
                    )
                })
                .map(|(hit, _)| hit.contour)
        });
        let contour_count = norad_glyph.map(|g| g.contours.len()).unwrap_or(0);
        let has_components = !entry.component_names.is_empty();
        if let Some((ci, _)) = component {
            self.editor.selected_component = Some(ci);
            self.editor.selected.clear();
        }
        let bounds = *self.editor.bounds.lock().unwrap();
        self.context_menu = Some(ContextMenu {
            at: gpui::point(pos.x - bounds.origin.x, pos.y - bounds.origin.y),
            design: (dx, dy),
            contour,
            contour_count,
            start_point,
            anchor,
            component,
            has_components,
            adding_component: false,
            applying_corner: false,
            adding_note: false,
            annotation: self.font().and_then(|f| {
                let glyph = f.font.get_glyph(f.glyphs[index].name.as_ref())?;
                read_annotations(glyph)
                    .iter()
                    .enumerate()
                    .map(|(i, a)| (((a.x - dx).powi(2) + (a.y - dy).powi(2)).sqrt(), i))
                    .filter(|(dist, _)| *dist <= tolerance * 2.0)
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, i)| i)
            }),
            guide: self.guide_hit(dx, dy, tolerance),
        });
    }

    /// Run one context-menu action and close the menu.
    fn context_menu_action(&mut self, action: &'static str) {
        let Some(menu) = self.context_menu.take() else {
            return;
        };
        let Mode::Editor(index) = self.mode else {
            return;
        };
        match action {
            "guide-delete" => {
                if let Some((local, gi)) = menu.guide {
                    if local {
                        let name = self.font().map(|f| f.glyphs[index].name.to_string());
                        if let (Some(name), Some(f)) = (name, self.font_mut()) {
                            if let Some(g) = f.font.get_glyph_mut(name.as_str()) {
                                if gi < g.guidelines.len() {
                                    g.guidelines.remove(gi);
                                    f.dirty = true;
                                    f.modified_glyphs.insert(name);
                                }
                            }
                        }
                    } else if let Some(f) = self.font_mut() {
                        if let Some(gs) = f.font.font_info.guidelines.as_mut() {
                            if gi < gs.len() {
                                gs.remove(gi);
                                f.dirty = true;
                            }
                        }
                    }
                }
            }
            "guide-add-h" | "guide-add-v" | "guide-add-local-h" | "guide-add-local-v" => {
                let (dx, dy) = menu.design;
                let vertical = action.ends_with("-v");
                let line = if vertical {
                    norad::Line::Vertical(dx.round())
                } else {
                    norad::Line::Horizontal(dy.round())
                };
                let guide = norad::Guideline::new(line, None, None, None);
                if action.contains("local") {
                    // A local guide belongs to the open glyph.
                    let name = self.font().map(|f| f.glyphs[index].name.to_string());
                    if let (Some(name), Some(f)) = (name, self.font_mut()) {
                        if let Some(g) = f.font.get_glyph_mut(name.as_str()) {
                            g.guidelines.push(guide);
                            f.dirty = true;
                            f.modified_glyphs.insert(name);
                        }
                    }
                } else if let Some(f) = self.font_mut() {
                    f.font
                        .font_info
                        .guidelines
                        .get_or_insert_with(Vec::new)
                        .push(guide);
                    f.dirty = true;
                }
            }
            "lock-component" | "unlock-component" => {
                if let Some((ci, _)) = menu.component {
                    self.toggle_component_alignment(index, ci);
                }
            }
            "decompose-component" => {
                if let Some((ci, _)) = menu.component {
                    self.push_undo_snapshot(index);
                    let ok = self
                        .font_mut()
                        .and_then(|f| {
                            let font_clone = f.font.clone();
                            f.edit_glyph(index, |g| {
                                runebender_core::glyph_ops::decompose_single_component(
                                    &font_clone,
                                    g,
                                    ci,
                                )
                            })
                        })
                        .unwrap_or(false);
                    if !ok {
                        self.editor.undo.pop();
                    }
                    self.editor.selected_component = None;
                }
            }
            "decompose-all" => {
                self.push_undo_snapshot(index);
                let ok = self.font_mut().is_some_and(|f| f.decompose(index));
                if !ok {
                    self.editor.undo.pop();
                }
                self.editor.selected_component = None;
            }
            "add-component" => {
                // Reopen in input mode; commit happens on Enter in
                // the name field.
                self.context_menu = Some(ContextMenu {
                    adding_component: true,
                    ..menu
                });
            }
            "apply-corner" => {
                self.context_menu = Some(ContextMenu {
                    applying_corner: true,
                    ..menu
                });
            }
            "annotation-note" => {
                self.context_menu = Some(ContextMenu {
                    adding_note: true,
                    ..menu
                });
            }
            "annotation-arrow" => {
                self.command_add_annotation(menu.design, "arrow", "");
            }
            "annotation-circle" => {
                self.command_add_annotation(menu.design, "circle", "");
            }
            "annotation-delete" => {
                if let Some(i) = menu.annotation {
                    self.command_delete_annotation(i);
                }
            }
            "mask-toggle" => {
                if let Some(ci) = menu.contour {
                    self.command_toggle_mask(ci);
                }
            }
            "node-insert" => {
                let (dx, dy) = menu.design;
                self.push_undo_snapshot(index);
                let inserted = self
                    .font_mut()
                    .and_then(|f| {
                        f.edit_glyph(index, |g| {
                            runebender_core::segment_ops::nearest_segment_with_t(
                                g,
                                kurbo::Point::new(dx, dy),
                                24.0,
                            )
                            .and_then(|(hit, t)| {
                                runebender_core::segment_ops::insert_point_on_segment(g, &hit, t)
                            })
                        })
                    })
                    .flatten();
                match inserted {
                    Some(id) => {
                        self.editor.selected.clear();
                        self.editor.selected.insert(id);
                    }
                    None => {
                        self.editor.undo.pop();
                    }
                }
            }
            "contour-open-close" => {
                if let Some((ci, pi)) = menu.start_point {
                    self.push_undo_snapshot(index);
                    let changed = self
                        .font_mut()
                        .and_then(|f| f.edit_glyph(index, |g| toggle_contour_open(g, ci, pi)))
                        .unwrap_or(false);
                    if !changed {
                        self.editor.undo.pop();
                    } else {
                        self.editor.selected.clear();
                    }
                }
            }
            "node-lock" => {
                if let Some(node) = menu.start_point {
                    if !self.editor.locked_points.remove(&node) {
                        self.editor.locked_points.insert(node);
                        self.editor.selected.remove(&node);
                    }
                }
            }
            "node-unlock-all" => {
                self.editor.locked_points.clear();
            }
            "set-start" => {
                if let Some((ci, pi)) = menu.start_point {
                    self.push_undo_snapshot(index);
                    let ok = self
                        .font_mut()
                        .and_then(|f| {
                            f.edit_glyph(index, |g| {
                                runebender_core::glyph_ops::set_contour_start(g, ci, pi)
                            })
                        })
                        .unwrap_or(false);
                    if !ok {
                        self.editor.undo.pop();
                    }
                }
            }
            "reverse" => {
                if let Some(ci) = menu.contour {
                    self.push_undo_snapshot(index);
                    let target: std::collections::HashSet<(usize, usize)> = [(ci, 0)].into();
                    let ok = self
                        .font_mut()
                        .and_then(|f| {
                            f.edit_glyph(index, |g| {
                                runebender_core::glyph_ops::reverse_contours(g, &target)
                            })
                        })
                        .unwrap_or(false);
                    if !ok {
                        self.editor.undo.pop();
                    }
                }
            }
            "round-corners" => self.command_round_corners(),
            "move-up" | "move-down" => {
                if let Some(ci) = menu.contour {
                    self.push_undo_snapshot(index);
                    let up = action == "move-up";
                    let ok = self
                        .font_mut()
                        .and_then(|f| {
                            f.edit_glyph(index, |g| {
                                runebender_core::glyph_ops::move_contour(g, ci, up)
                            })
                        })
                        .unwrap_or(false);
                    if !ok {
                        self.editor.undo.pop();
                    } else {
                        self.editor.selected.clear();
                    }
                }
            }
            "add-anchor" => {
                self.push_undo_snapshot(index);
                if let Some(font) = self.font_mut() {
                    font.add_anchor(index, menu.design.0.round(), menu.design.1.round());
                }
            }
            "delete-anchor" => {
                if let Some(ai) = menu.anchor {
                    self.push_undo_snapshot(index);
                    if let Some(font) = self.font_mut() {
                        font.delete_anchor(index, ai);
                    }
                    self.editor.selected_anchors.clear();
                }
            }
            _ => {}
        }
    }

    /// Commit the Add Component name field.
    fn commit_add_component(&mut self, base: &str) {
        self.context_menu = None;
        let Mode::Editor(index) = self.mode else {
            return;
        };
        let base = base.trim().to_string();
        if base.is_empty() {
            return;
        }
        self.push_undo_snapshot(index);
        let ok = self
            .font_mut()
            .and_then(|f| {
                let font_clone = f.font.clone();
                f.edit_glyph(index, |g| {
                    runebender_core::glyph_ops::add_component(&font_clone, g, &base)
                })
            })
            .unwrap_or(false);
        if !ok {
            self.editor.undo.pop();
            self.status_note = Some(format!("No glyph named {base}").into());
        } else {
            self.status_note = Some(format!("Added component {base}").into());
        }
    }

    /// Bounding box of the selected points, in design space. None
    /// when it has no extent (a single point, or none).
    fn selection_bbox(&self, index: usize) -> Option<kurbo::Rect> {
        if self.editor.selected.len() < 2 {
            return None;
        }
        let font = self.font()?;
        let mut min = (f64::INFINITY, f64::INFINITY);
        let mut max = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        for p in font.glyphs[index].points.iter() {
            if !self.editor.selected.contains(&(p.contour, p.index)) {
                continue;
            }
            min = (min.0.min(p.x), min.1.min(p.y));
            max = (max.0.max(p.x), max.1.max(p.y));
        }
        (min.0.is_finite() && (max.0 - min.0 > 1e-9 || max.1 - min.1 > 1e-9))
            .then(|| kurbo::Rect::new(min.0, min.1, max.0, max.1))
    }

    /// Distance from (dx, dy) to a guideline.
    fn guide_distance(line: &norad::Line, dx: f64, dy: f64) -> f64 {
        match *line {
            norad::Line::Vertical(x) => (dx - x).abs(),
            norad::Line::Horizontal(y) => (dy - y).abs(),
            norad::Line::Angle { x, y, degrees } => {
                // Distance to the infinite line through (x, y).
                let (sin, cos) = degrees.to_radians().sin_cos();
                ((dy - y) * cos - (dx - x) * sin).abs()
            }
        }
    }

    /// The nearest guide within `tolerance` design units of (dx, dy):
    /// (local, index). Local guides (the open glyph's own) win ties
    /// over the master's global fontinfo guidelines.
    fn guide_hit(&self, dx: f64, dy: f64, tolerance: f64) -> Option<(bool, usize)> {
        let font = self.font()?;
        let local = self
            .current_glyph_index()
            .and_then(|i| font.font.get_glyph(font.glyphs[i].name.as_ref()))
            .into_iter()
            .flat_map(|g| g.guidelines.iter().enumerate())
            .map(|(i, g)| (Self::guide_distance(&g.line, dx, dy), (true, i)));
        let global = font
            .font
            .font_info
            .guidelines
            .iter()
            .flatten()
            .enumerate()
            .map(|(i, g)| (Self::guide_distance(&g.line, dx, dy), (false, i)));
        local
            .chain(global)
            .filter(|(dist, _)| *dist <= tolerance)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id)
    }

    /// Every selected anchor's index and current position, for drags
    /// that carry them along with the point selection.
    fn selected_anchor_origin(&self, index: usize) -> Vec<(usize, (f64, f64))> {
        let Some(font) = self.font() else {
            return Vec::new();
        };
        self.editor
            .selected_anchors
            .iter()
            .filter_map(|&ai| {
                let (_, x, y) = font.glyphs[index].anchors.get(ai)?;
                Some((ai, (*x, *y)))
            })
            .collect()
    }

    /// Idle mouse move over the canvas: track the pointer for pen
    /// previews, and alt-hover highlights the nearest segment
    /// (select tool), like the web editor.
    fn editor_hover(&mut self, pos: Point<gpui::Pixels>, alt: bool) -> bool {
        let Mode::Editor(index) = self.mode else {
            return false;
        };
        let mut changed = false;
        let track_pointer = matches!(self.editor.tool, Tool::Pen | Tool::HyperPen | Tool::Select);
        if track_pointer {
            let moved = self.editor.pointer.is_none_or(|p| p != pos);
            self.editor.pointer = Some(pos);
            // Re-render for the pen rubber band only while drawing.
            if moved && (self.editor.pen.is_some() || self.editor.hyper_contour.is_some()) {
                changed = true;
            }
        }
        if self.editor.tool == Tool::Select && self.editor.drag.is_none() {
            let (dx, dy) = self.editor.window_to_design(pos);
            let tolerance = HIT_RADIUS_PX / self.editor.zoom();
            let (top_b, bottom_b) = self.text_sort_bounds();
            let advance = self.font().map(|f| f.glyphs[index].advance).unwrap_or(0.0);
            let edge = if dy >= bottom_b - tolerance && dy <= top_b + tolerance {
                if (dx - advance).abs() <= tolerance {
                    Some(true)
                } else if dx.abs() <= tolerance {
                    Some(false)
                } else {
                    None
                }
            } else {
                None
            };
            if self.editor.sidebearing_hover != edge {
                self.editor.sidebearing_hover = edge;
                changed = true;
            }
            // Guides light up under the cursor so their knob reads
            // as grabbable before anything is clicked.
            let guide = self.guide_hit(dx, dy, tolerance);
            if self.editor.guide_hover != guide {
                self.editor.guide_hover = guide;
                changed = true;
            }
        }
        let hover = if alt && self.editor.tool == Tool::Select {
            let (dx, dy) = self.editor.window_to_design(pos);
            let radius = HIT_RADIUS_PX / self.editor.zoom();
            self.font()
                .and_then(|f| f.font.get_glyph(f.glyphs[index].name.as_ref()))
                .and_then(|g| {
                    runebender_core::segment_ops::nearest_segment_with_t(
                        g,
                        kurbo::Point::new(dx, dy),
                        radius,
                    )
                })
                .map(|(hit, _)| hit.seg)
        } else {
            None
        };
        if self.editor.segment_hover.map(seg_key) != hover.map(seg_key) {
            self.editor.segment_hover = hover;
            changed = true;
        }
        changed
    }

    /// Selection for a marquee: whatever it started from, plus every
    /// point and anchor the box encloses. Recomputed on every drag
    /// step, so pulling the box back in gives entities up again (web
    /// `select_in_screen_rect`).
    fn select_in_rect(
        &mut self,
        index: usize,
        start: (f64, f64),
        current: (f64, f64),
        base: &std::collections::HashSet<(usize, usize)>,
        base_anchors: &[usize],
    ) {
        let (x0, x1) = (start.0.min(current.0), start.0.max(current.0));
        let (y0, y1) = (start.1.min(current.1), start.1.max(current.1));
        let inside = |x: f64, y: f64| x >= x0 && x <= x1 && y >= y0 && y <= y1;
        let Some(font) = self.font() else { return };
        let entry = &font.glyphs[index];
        let mut selected = base.clone();
        selected.extend(
            entry
                .points
                .iter()
                .filter(|p| inside(p.x, p.y))
                .map(|p| (p.contour, p.index))
                .filter(|id| !self.editor.locked_points.contains(id)),
        );
        let mut anchors = base_anchors.to_vec();
        for (i, (_, x, y)) in entry.anchors.iter().enumerate() {
            if inside(*x, *y) && !anchors.contains(&i) {
                anchors.push(i);
            }
        }
        self.editor.selected = selected;
        self.editor.selected_anchors = anchors;
    }

    /// End the open hyper contour (Enter/Escape/tool switch), leaving
    /// it open like an unfinished pen path; degenerate ones vanish.
    fn hyper_pen_finish(&mut self) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        if let Some(contour) = self.editor.hyper_contour.take()
            && let Some(font) = self.font_mut()
        {
            font.remove_contour_if_degenerate(index, contour);
        }
    }

    fn pen_finish(&mut self) {
        self.hyper_pen_finish();
        let Mode::Editor(index) = self.mode else {
            return;
        };
        if let Some(pen) = self.editor.pen.take()
            && let Some(font) = self.font_mut()
        {
            font.remove_contour_if_degenerate(index, pen.contour);
        }
    }

    fn apply_metric(&mut self, which: MetricField, value: f64) {
        // A grid multi-selection batch-edits: the typed value lands
        // on every selected glyph, the Glyphs list-edit behavior.
        // No undo for the batch yet — undo is single-glyph.
        let batch: Vec<usize> = if matches!(self.mode, Mode::Grid) && self.multi_selected.len() > 1
        {
            let Some(font) = self.font() else { return };
            self.multi_selected
                .iter()
                .filter_map(|name| font.name_map.get(name).copied())
                .collect()
        } else {
            let Some(index) = (match self.mode {
                Mode::Editor(index) => Some(index),
                Mode::Grid => self.selected,
            }) else {
                return;
            };
            self.push_undo_snapshot(index);
            vec![index]
        };
        let count = batch.len();
        let Some(font) = self.font_mut() else {
            return;
        };
        for index in batch {
            let ink = font.ink_bounds(index);
            let advance = font.glyphs[index].advance;
            match which {
                MetricField::Width => font.set_advance(index, value.round()),
                MetricField::Lsb => {
                    if let Some(ink) = ink {
                        // Move the ink; the right sidebearing absorbs it.
                        font.shift_ink(index, (value - ink.x0).round());
                    }
                }
                MetricField::Rsb => {
                    if let Some(ink) = ink {
                        font.set_advance(index, (ink.x1 + value).round());
                    } else {
                        font.set_advance(index, (advance + value).round());
                    }
                }
            }
        }
        if count > 1 {
            self.status_note = Some(
                format!(
                    "{} set on {count} glyphs",
                    match which {
                        MetricField::Width => "Width",
                        MetricField::Lsb => "LSB",
                        MetricField::Rsb => "RSB",
                    }
                )
                .into(),
            );
        }
    }

    /// Push current glyph metrics into the input fields. Skipped when
    /// an input has focus (unless forced) so typing is not clobbered.
    /// After a rename or unicode change reorders the glyph list,
    /// re-point selection, the open editor, and the parked session at
    /// the glyph by name.
    fn remap_glyph_indices(&mut self, name: &str) {
        let Some(&index) = self.font().and_then(|f| f.name_map.get(name)) else {
            return;
        };
        if self.selected.is_some() {
            self.selected = Some(index);
        }
        if matches!(self.mode, Mode::Editor(_)) {
            self.mode = Mode::Editor(index);
        }
        if self.last_editor.is_some() {
            self.last_editor = Some(index);
        }
    }

    /// Set (or clear) the selected glyph's note in the active master.
    fn apply_glyph_note(&mut self, text: &str) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        let text = text.trim();
        if let Some(font) = self.font_mut()
            && let Some(glyph) = font.font.get_glyph_mut(name.as_str())
        {
            let new = (!text.is_empty()).then(|| text.to_string());
            if glyph.note != new {
                glyph.note = new;
                font.dirty = true;
                font.modified_glyphs.insert(name);
            }
        }
    }

    /// Set or clear the glyph's production (export) name in every
    /// master's public.postscriptNames mapping.
    fn apply_glyph_production(&mut self, text: &str) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let name = project.active_font().glyphs[index].name.to_string();
        let text = text.trim().to_string();
        for master in project.masters.iter_mut() {
            let dict = match master.font.lib.get_mut(PSNAMES_KEY) {
                Some(plist::Value::Dictionary(d)) => d,
                _ => {
                    if text.is_empty() {
                        continue;
                    }
                    master.font.lib.insert(
                        PSNAMES_KEY.into(),
                        plist::Value::Dictionary(plist::Dictionary::new()),
                    );
                    match master.font.lib.get_mut(PSNAMES_KEY) {
                        Some(plist::Value::Dictionary(d)) => d,
                        _ => continue,
                    }
                }
            };
            let before = dict.get(&name).and_then(|v| v.as_string());
            if text.is_empty() {
                if before.is_some() {
                    dict.remove(&name);
                    if dict.is_empty() {
                        master.font.lib.remove(PSNAMES_KEY);
                    }
                    master.dirty = true;
                }
            } else if before != Some(text.as_str()) {
                dict.insert(name.clone(), plist::Value::String(text.clone()));
                master.dirty = true;
            }
        }
    }

    /// Rename the selected glyph in every master, updating components,
    /// groups, kerning, and the open text session.
    fn apply_glyph_rename(&mut self, new_name: &str) {
        let Some(index) = self.selected else { return };
        let Some(old) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        let new_name = new_name.trim().to_string();
        if new_name.is_empty() || new_name == old {
            return;
        }
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let mut renamed = false;
        for master in project.masters.iter_mut() {
            if runebender_core::glyph_ops::rename_glyph(&mut master.font, &old, &new_name) {
                master.dirty = true;
                master.kerning_dirty = true;
                master.modified_glyphs.remove(&old);
                master.modified_glyphs.insert(new_name.clone());
                master.refresh_from_font();
                renamed = true;
            }
        }
        if !renamed {
            self.status_note = Some(format!("Cannot rename {old} to {new_name}").into());
            return;
        }
        project.compat.remove(&old);
        let recheck = new_name.clone();
        project.recheck_compat(&recheck);
        // Parked tabs on the renamed glyph follow it.
        for slot in &mut self.sessions {
            if slot.glyph_name == old {
                slot.glyph_name = new_name.clone();
            }
        }
        // The open text session keeps working under the new name.
        for i in 0..self.edit_buffer.len() {
            let matches_old = self
                .edit_buffer
                .sort(i)
                .and_then(|s| s.glyph_name())
                .is_some_and(|n| n == old);
            if matches_old {
                let (codepoint, advance) = self
                    .font()
                    .and_then(|f| f.name_map.get(&new_name).copied())
                    .and_then(|g| {
                        self.font()
                            .map(|f| (f.glyphs[g].codepoint, f.glyphs[g].advance))
                    })
                    .unwrap_or((None, 0.0));
                self.edit_buffer
                    .update_glyph(i, new_name.clone(), codepoint, advance);
            }
        }
        self.sidebar_counts = None;
        self.remap_glyph_indices(&new_name);
        self.status_note = Some(format!("Renamed {old} → {new_name}").into());
    }

    /// Set the selected glyph's unicode in every master ("0041",
    /// "U+0041", "0x41"; empty clears).
    fn apply_glyph_unicode(&mut self, text: &str) {
        let Some(index) = self.selected else { return };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let mut ok = false;
        for master in project.masters.iter_mut() {
            if let Some(glyph_index) = master.name_map.get(&name).copied() {
                let changed = master
                    .edit_glyph(glyph_index, |g| {
                        runebender_core::glyph_ops::set_glyph_unicode(g, text)
                    })
                    .unwrap_or(false);
                if changed {
                    master.refresh_from_font();
                    ok = true;
                }
            }
        }
        if !ok {
            self.status_note = Some(format!("Bad unicode: {text}").into());
            return;
        }
        self.sidebar_counts = None;
        self.rebuild_text_models();
        self.remap_glyph_indices(&name);
    }

    /// Set the selected glyph's kerning group on one side, in every
    /// master (groups.plist; empty clears).
    fn apply_kern_group(&mut self, first_side: bool, text: &str) {
        let Some(index) = self.selected else { return };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        let Some(project) = self.project.as_mut() else {
            return;
        };
        for master in project.masters.iter_mut() {
            if runebender_core::glyph_ops::set_kern_group(&mut master.font, &name, first_side, text)
            {
                master.dirty = true;
                master.kerning_dirty = true;
            }
        }
        self.rebuild_text_models();
    }

    /// Fill the Glyph panel's editable fields from the selected glyph
    /// unless one of them is being typed in.
    fn refresh_glyph_inputs(&mut self, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !force && window.focused(cx).is_some_and(|f| f != self.focus_handle) {
            return;
        }
        let Some(index) = self.selected else { return };
        let Some(font) = self.font() else { return };
        let Some(entry) = font.glyphs.get(index) else {
            return;
        };
        let name = entry.name.to_string();
        let unicode = entry
            .codepoint
            .map(|c| format!("{:04X}", c as u32))
            .unwrap_or_default();
        let group_l = runebender_core::glyph_ops::kern_group(&font.font, &name, true)
            .map(|g| g.as_str().replace("public.kern1.", ""))
            .unwrap_or_default();
        let group_r = runebender_core::glyph_ops::kern_group(&font.font, &name, false)
            .map(|g| g.as_str().replace("public.kern2.", ""))
            .unwrap_or_default();
        let set = |entity: &gpui::Entity<widgets::input::InputState>,
                   value: String,
                   window: &mut Window,
                   cx: &mut Context<Self>| {
            entity.update(cx, |st, cx| {
                if st.value() != value.as_str() {
                    st.set_value(value, window, cx);
                }
            });
        };
        let note = font
            .font
            .get_glyph(name.as_str())
            .and_then(|g| g.note.clone())
            .unwrap_or_default();
        let (lkey, rkey) = font
            .font
            .get_glyph(name.as_str())
            .map(|g| {
                (
                    read_metrics_key(g, true).unwrap_or_default(),
                    read_metrics_key(g, false).unwrap_or_default(),
                )
            })
            .unwrap_or_default();
        let name_input = self.glyph_inputs.name.clone();
        let unicode_input = self.glyph_inputs.unicode.clone();
        let l_input = self.glyph_inputs.group_l.clone();
        let r_input = self.glyph_inputs.group_r.clone();
        let note_input = self.glyph_inputs.note.clone();
        let lkey_input = self.glyph_inputs.lsb_key.clone();
        let rkey_input = self.glyph_inputs.rsb_key.clone();
        let production = read_production_name(&font.font, name.as_str()).unwrap_or_default();
        let production_input = self.glyph_inputs.production.clone();
        set(&name_input, name, window, cx);
        set(&unicode_input, unicode, window, cx);
        set(&l_input, group_l, window, cx);
        set(&r_input, group_r, window, cx);
        set(&note_input, note, window, cx);
        set(&lkey_input, lkey, window, cx);
        set(&rkey_input, rkey, window, cx);
        set(&production_input, production, window, cx);
    }

    /// Auto-generated feature blocks from glyph names, the Glyphs
    /// conventions: `.init`/`.medi`/`.fina` suffixes feed the
    /// positional features, and underscore names (f_i, beh-ar_lam-ar)
    /// whose parts all exist feed liga. Returns (tag, body) pairs;
    /// tags with nothing to say are omitted.
    fn generated_feature_blocks(font: &norad::Font) -> Vec<(String, String)> {
        let names: std::collections::BTreeSet<&str> = font
            .default_layer()
            .iter()
            .map(|g| g.name().as_str())
            .collect();
        let mut blocks: Vec<(String, String)> = Vec::new();
        for tag in ["init", "medi", "fina"] {
            let suffix = format!(".{tag}");
            let mut rules = String::new();
            for name in &names {
                let Some(base) = name.strip_suffix(suffix.as_str()) else {
                    continue;
                };
                if names.contains(base) {
                    rules.push_str(&format!("    sub {base} by {name};\n"));
                }
            }
            if !rules.is_empty() {
                blocks.push((tag.to_string(), rules));
            }
        }
        // Cursive attachment: glyphs carrying entry/exit anchors
        // (the Glyphs cascade workflow) feed a curs feature —
        // position cursive <glyph> <entry> <exit>, NULL where a
        // side is missing.
        {
            let mut rules = String::new();
            for glyph in font.default_layer().iter() {
                let mut entry: Option<(f64, f64)> = None;
                let mut exit: Option<(f64, f64)> = None;
                for anchor in &glyph.anchors {
                    match anchor.name.as_ref().map(|n| n.as_str()) {
                        Some("entry") => entry = Some((anchor.x, anchor.y)),
                        Some("exit") => exit = Some((anchor.x, anchor.y)),
                        _ => {}
                    }
                }
                if entry.is_none() && exit.is_none() {
                    continue;
                }
                let fmt = |a: Option<(f64, f64)>| match a {
                    Some((x, y)) => format!("<anchor {x:.0} {y:.0}>"),
                    None => "<anchor NULL>".to_string(),
                };
                rules.push_str(&format!(
                    "    position cursive {} {} {};\n",
                    glyph.name(),
                    fmt(entry),
                    fmt(exit),
                ));
            }
            if !rules.is_empty() {
                let body = format!("    lookupflag RightToLeft IgnoreMarks;\n{rules}");
                blocks.push(("curs".to_string(), body));
            }
        }
        // Mark positioning (mark + mkmk) from anchors, the way
        // Fontra emulates it live: every anchor family X with marks
        // carrying _X gets a markClass; bases with X position them,
        // marks that also carry X stack them. The shaped preview
        // then places vowel marks exactly as the compiled font will.
        {
            use std::collections::BTreeMap;
            // anchor name -> (marks: name, _X pos), (bases: name, X pos),
            // (mark carriers: name, X pos).
            let mut families: BTreeMap<
                String,
                (
                    Vec<(String, f64, f64)>,
                    Vec<(String, f64, f64)>,
                    Vec<(String, f64, f64)>,
                ),
            > = BTreeMap::new();
            for glyph in font.default_layer().iter() {
                let is_mark_glyph = glyph
                    .anchors
                    .iter()
                    .any(|a| a.name.as_ref().is_some_and(|n| n.as_str().starts_with('_')));
                for anchor in &glyph.anchors {
                    let Some(name) = anchor.name.as_ref().map(|n| n.as_str()) else {
                        continue;
                    };
                    if name == "entry" || name == "exit" {
                        continue;
                    }
                    if let Some(base_name) = name.strip_prefix('_') {
                        families.entry(base_name.to_string()).or_default().0.push((
                            glyph.name().to_string(),
                            anchor.x,
                            anchor.y,
                        ));
                    } else {
                        let entry = families.entry(name.to_string()).or_default();
                        let record = (glyph.name().to_string(), anchor.x, anchor.y);
                        if is_mark_glyph {
                            entry.2.push(record);
                        } else {
                            entry.1.push(record);
                        }
                    }
                }
            }
            let mut mark_rules = String::new();
            let mut mkmk_rules = String::new();
            let mut classes = String::new();
            for (family, (marks, bases, carriers)) in &families {
                if marks.is_empty() || (bases.is_empty() && carriers.is_empty()) {
                    continue;
                }
                for (mark, x, y) in marks {
                    classes.push_str(&format!(
                        "    markClass {mark} <anchor {x:.0} {y:.0}> @MC_{family};\n"
                    ));
                }
                for (base, x, y) in bases {
                    mark_rules.push_str(&format!(
                        "    pos base {base} <anchor {x:.0} {y:.0}> mark @MC_{family};\n"
                    ));
                }
                for (carrier, x, y) in carriers {
                    mkmk_rules.push_str(&format!(
                        "    pos mark {carrier} <anchor {x:.0} {y:.0}> mark @MC_{family};\n"
                    ));
                }
            }
            if !mark_rules.is_empty() {
                blocks.push(("mark".to_string(), format!("{classes}{mark_rules}")));
            }
            if !mkmk_rules.is_empty() {
                let body = if mark_rules.is_empty() {
                    format!("{classes}{mkmk_rules}")
                } else {
                    // Classes already defined in the mark block.
                    mkmk_rules.clone()
                };
                blocks.push(("mkmk".to_string(), body));
            }
        }
        // Composition (ccmp): a composite-only glyph whose
        // components all exist, with at least one combining mark
        // after the base, substitutes from its parts — edit the base
        // and the mark once, the composed form follows (the
        // composition-first workflow). Longest sequences first.
        {
            let is_mark = |name: &str| {
                font.default_layer()
                    .get_glyph(name)
                    .and_then(|g| g.codepoints.iter().next())
                    .is_some_and(|c| {
                        matches!(
                            runebender_core::category::GlyphCategory::from_codepoint(c),
                            runebender_core::category::GlyphCategory::Mark
                        )
                    })
            };
            let mut compositions: Vec<(String, Vec<String>)> = font
                .default_layer()
                .iter()
                .filter(|g| {
                    g.contours.is_empty() && g.components.len() >= 2 && !g.name().contains('.')
                })
                .filter_map(|g| {
                    let parts: Vec<String> =
                        g.components.iter().map(|c| c.base.to_string()).collect();
                    (parts.iter().all(|p| names.contains(p.as_str()))
                        && parts[1..].iter().any(|p| is_mark(p)))
                    .then(|| (g.name().to_string(), parts))
                })
                .collect();
            compositions.sort_by_key(|(_, parts)| std::cmp::Reverse(parts.len()));
            if !compositions.is_empty() {
                let mut rules = String::new();
                for (name, parts) in compositions {
                    rules.push_str(&format!("    sub {} by {name};\n", parts.join(" ")));
                }
                blocks.push(("ccmp".to_string(), rules));
            }
        }
        // Ligatures: longest first, so f_f_i wins over f_f.
        let mut ligatures: Vec<(&str, Vec<&str>)> = names
            .iter()
            .filter(|name| name.contains('_') && !name.contains('.'))
            .filter_map(|name| {
                let parts: Vec<&str> = name.split('_').collect();
                (parts.len() >= 2 && parts.iter().all(|part| names.contains(part)))
                    .then(|| (*name, parts))
            })
            .collect();
        ligatures.sort_by_key(|(_, parts)| std::cmp::Reverse(parts.len()));
        if !ligatures.is_empty() {
            let mut rules = String::new();
            for (name, parts) in ligatures {
                rules.push_str(&format!("    sub {} by {name};\n", parts.join(" ")));
            }
            blocks.push(("liga".to_string(), rules));
        }
        // Ligature caret positions: caret_1, caret_2... anchors on a
        // ligature give editing carets between its parts (GDEF
        // LigatureCaretByPos), the Glyphs anchor convention.
        let mut caret_rules = String::new();
        for glyph in font.default_layer().iter() {
            let mut carets: Vec<(u32, f64)> = glyph
                .anchors
                .iter()
                .filter_map(|a| {
                    let n = a
                        .name
                        .as_ref()?
                        .as_str()
                        .strip_prefix("caret_")?
                        .parse::<u32>()
                        .ok()?;
                    Some((n, a.x))
                })
                .collect();
            if carets.is_empty() {
                continue;
            }
            carets.sort_by_key(|(n, _)| *n);
            let positions: Vec<String> = carets.iter().map(|(_, x)| format!("{x:.0}")).collect();
            caret_rules.push_str(&format!(
                "    LigatureCaretByPos {} {};
",
                glyph.name(),
                positions.join(" ")
            ));
        }
        if !caret_rules.is_empty() {
            blocks.push(("table GDEF".to_string(), caret_rules));
        }
        blocks
    }

    /// Replace (or append) one `feature X { … } X;` block in a fea
    /// source. The terminator `} X;` is required syntax, so the block
    /// span is found textually.
    fn replace_feature_block(fea: &str, tag: &str, body: &str) -> String {
        // A tag of "table GDEF" replaces a table block instead;
        // both share the `} NAME;` terminator grammar.
        let (open, close, block) = match tag.strip_prefix("table ") {
            Some(name) => (
                format!("table {name} "),
                format!("}} {name};"),
                format!("table {name} {{\n{body}}} {name};\n"),
            ),
            None => (
                format!("feature {tag} "),
                format!("}} {tag};"),
                format!("feature {tag} {{\n{body}}} {tag};\n"),
            ),
        };
        if let (Some(start), Some(end)) = (fea.find(&open), fea.find(&close)) {
            if end > start {
                let mut out = String::with_capacity(fea.len());
                out.push_str(&fea[..start]);
                out.push_str(block.trim_end());
                out.push_str(&fea[end + close.len()..]);
                return out;
            }
        }
        // New block. An insertion marker (Fontra's convention, one
        // line reading "# Automatic Code") controls where generated
        // code lands among hand-written blocks: each new block goes
        // in just above the marker, so call order is kept and the
        // marker stays for the next Generate.
        for (offset, line) in fea.lines().map({
            let mut pos = 0usize;
            move |line| {
                let at = pos;
                pos += line.len() + 1;
                (at, line)
            }
        }) {
            if line.trim() == "# Automatic Code" {
                let mut out = String::with_capacity(fea.len() + block.len());
                out.push_str(&fea[..offset]);
                out.push_str(&block);
                out.push('\n');
                out.push_str(&fea[offset..]);
                return out;
            }
        }
        let mut out = fea.trim_end().to_string();
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&block);
        out
    }

    /// Compile-check a features.fea against the active master's
    /// glyph set, the same build the text engine shapes with.
    fn check_features_compile(font: &FontModel, fea: &str) -> Result<(), String> {
        use runebender_core::shape::{ShapingFont, ShapingGlyph, ShapingSource};
        let glyphs: Vec<ShapingGlyph> = std::iter::once(ShapingGlyph {
            name: ".notdef".into(),
            advance: 0.0,
            unicodes: Vec::new(),
        })
        .chain(
            font.glyphs
                .iter()
                .filter(|g| g.name.as_ref() != ".notdef")
                .map(|g| ShapingGlyph {
                    name: g.name.to_string(),
                    advance: g.advance,
                    unicodes: g.codepoint.map(|c| c as u32).into_iter().collect(),
                }),
        )
        .collect();
        ShapingFont::build(&ShapingSource {
            units_per_em: font.units_per_em,
            glyphs,
            features: fea.to_string(),
        })
        .map(|_| ())
    }

    /// Push the active master's features.fea into the editor. Hands
    /// off while it holds unapplied edits or focus, unless forced.
    fn refresh_features_input(&mut self, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !force
            && (self.features_edited || window.focused(cx).is_some_and(|f| f != self.focus_handle))
        {
            return;
        }
        let Some(font) = self.font() else { return };
        let value = font.font.features.clone();
        self.features_input.update(cx, |st, cx| {
            if st.value() != value.as_str() {
                st.set_value(value, window, cx);
            }
        });
    }

    /// Commit the Kerning section's editor row: set (or update) the
    /// pair on the active master. First and second may be glyph names
    /// or group names (public.kern1./public.kern2.).
    fn apply_kern_pair(&mut self, first: &str, second: &str, value: f64) {
        let (Ok(first), Ok(second)) = (norad::Name::new(first), norad::Name::new(second)) else {
            self.status_note = Some("Kerning: invalid name".into());
            return;
        };
        if let Some(font) = self.font_mut() {
            font.font
                .kerning
                .entry(first)
                .or_default()
                .insert(second, value);
            font.kerning_dirty = true;
            font.dirty = true;
        }
        self.rebuild_text_models();
    }

    /// Remove one kerning pair from the active master.
    fn delete_kern_pair(&mut self, first: &str, second: &str) {
        if let Some(font) = self.font_mut() {
            let mut emptied = false;
            if let Some(seconds) = font.font.kerning.get_mut(first) {
                seconds.retain(|name, _| name.as_str() != second);
                emptied = seconds.is_empty();
            }
            if emptied {
                font.font.kerning.retain(|name, _| name.as_str() != first);
            }
            font.kerning_dirty = true;
            font.dirty = true;
        }
        self.rebuild_text_models();
    }

    /// Commit one Font Info field (Enter in the Font Info section).
    /// The family name is font-wide and lands on every master; style
    /// and the metrics belong to the active master.
    fn apply_font_info(&mut self, field: FontInfoField, text: &str) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let text = text.trim();
        match field {
            FontInfoField::Family => {
                if text.is_empty() {
                    return;
                }
                for master in project.masters.iter_mut() {
                    master.font.font_info.family_name = Some(text.to_string());
                    master.dirty = true;
                }
            }
            FontInfoField::Style => {
                if text.is_empty() {
                    return;
                }
                let active = project.active;
                let master = &mut project.masters[active];
                master.font.font_info.style_name = Some(text.to_string());
                master.dirty = true;
                project.master_names[active] = text.to_string().into();
            }
            FontInfoField::BlueValues
            | FontInfoField::OtherBlues
            | FontInfoField::StemsH
            | FontInfoField::StemsV => {
                // Comma or space separated numbers; empty clears.
                let values: Vec<f64> = text
                    .split([',', ' '])
                    .filter(|part| !part.trim().is_empty())
                    .filter_map(|part| part.trim().parse::<f64>().ok())
                    .collect();
                let stored = (!values.is_empty()).then_some(values);
                let master = &mut project.masters[project.active];
                let info = &mut master.font.font_info;
                match field {
                    FontInfoField::BlueValues => info.postscript_blue_values = stored,
                    FontInfoField::OtherBlues => info.postscript_other_blues = stored,
                    FontInfoField::StemsH => info.postscript_stem_snap_h = stored,
                    _ => info.postscript_stem_snap_v = stored,
                }
                master.dirty = true;
            }
            _ => {
                let Ok(v) = text.parse::<f64>() else { return };
                let master = &mut project.masters[project.active];
                let info = &mut master.font.font_info;
                match field {
                    FontInfoField::Upm => {
                        let Ok(upm) = norad::fontinfo::NonNegativeIntegerOrFloat::try_from(v)
                        else {
                            return;
                        };
                        info.units_per_em = Some(upm);
                        master.units_per_em = v;
                    }
                    FontInfoField::ItalicAngle => info.italic_angle = Some(v),
                    FontInfoField::Ascender => {
                        info.ascender = Some(v);
                        master.ascender = v;
                    }
                    FontInfoField::Descender => {
                        info.descender = Some(v);
                        master.descender = v;
                    }
                    FontInfoField::XHeight => {
                        info.x_height = Some(v);
                        master.x_height = Some(v);
                    }
                    FontInfoField::CapHeight => {
                        info.cap_height = Some(v);
                        master.cap_height = Some(v);
                    }
                    FontInfoField::TypoAscender => {
                        info.open_type_os2_typo_ascender = Some(v as i32)
                    }
                    FontInfoField::TypoDescender => {
                        info.open_type_os2_typo_descender = Some(v as i32)
                    }
                    FontInfoField::TypoLineGap => info.open_type_os2_typo_line_gap = Some(v as i32),
                    FontInfoField::HheaAscender => info.open_type_hhea_ascender = Some(v as i32),
                    FontInfoField::HheaDescender => info.open_type_hhea_descender = Some(v as i32),
                    FontInfoField::HheaLineGap => info.open_type_hhea_line_gap = Some(v as i32),
                    FontInfoField::WinAscent => {
                        if v >= 0.0 {
                            info.open_type_os2_win_ascent = Some(v as u32)
                        }
                    }
                    FontInfoField::WinDescent => {
                        // winDescent is stored positive.
                        if v >= 0.0 {
                            info.open_type_os2_win_descent = Some(v as u32)
                        }
                    }
                    FontInfoField::Family
                    | FontInfoField::Style
                    | FontInfoField::BlueValues
                    | FontInfoField::OtherBlues
                    | FontInfoField::StemsH
                    | FontInfoField::StemsV => unreachable!(),
                }
                master.dirty = true;
            }
        }
    }

    /// Push the active master's font info into the section's inputs.
    /// Skipped while any input is focused, unless `force`, the same
    /// contract as `refresh_metric_inputs`.
    fn refresh_font_info_inputs(
        &mut self,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !force && window.focused(cx).is_some_and(|f| f != self.focus_handle) {
            return;
        }
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let master = &project.masters[project.active];
        let info = &master.font.font_info;
        let opt = |v: Option<f64>| v.map(|v| format!("{v:.0}")).unwrap_or_default();
        let list = |v: &Option<Vec<f64>>| {
            v.as_ref()
                .map(|values| {
                    values
                        .iter()
                        .map(|n| format!("{n:.0}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default()
        };
        let values = [
            (
                &self.font_info_inputs.family,
                info.family_name.clone().unwrap_or_default(),
            ),
            (
                &self.font_info_inputs.style,
                info.style_name.clone().unwrap_or_default(),
            ),
            (
                &self.font_info_inputs.upm,
                format!("{:.0}", master.units_per_em),
            ),
            (
                &self.font_info_inputs.italic_angle,
                info.italic_angle
                    .map(|v| format!("{v}"))
                    .unwrap_or_default(),
            ),
            (
                &self.font_info_inputs.ascender,
                format!("{:.0}", master.ascender),
            ),
            (
                &self.font_info_inputs.descender,
                format!("{:.0}", master.descender),
            ),
            (&self.font_info_inputs.x_height, opt(master.x_height)),
            (&self.font_info_inputs.cap_height, opt(master.cap_height)),
            (
                &self.font_info_inputs.typo_asc,
                info.open_type_os2_typo_ascender
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            ),
            (
                &self.font_info_inputs.typo_desc,
                info.open_type_os2_typo_descender
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            ),
            (
                &self.font_info_inputs.typo_gap,
                info.open_type_os2_typo_line_gap
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            ),
            (
                &self.font_info_inputs.hhea_asc,
                info.open_type_hhea_ascender
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            ),
            (
                &self.font_info_inputs.hhea_desc,
                info.open_type_hhea_descender
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            ),
            (
                &self.font_info_inputs.hhea_gap,
                info.open_type_hhea_line_gap
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            ),
            (
                &self.font_info_inputs.win_asc,
                info.open_type_os2_win_ascent
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            ),
            (
                &self.font_info_inputs.win_desc,
                info.open_type_os2_win_descent
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            ),
            (
                &self.font_info_inputs.blue_values,
                list(&info.postscript_blue_values),
            ),
            (
                &self.font_info_inputs.other_blues,
                list(&info.postscript_other_blues),
            ),
            (
                &self.font_info_inputs.stems_h,
                list(&info.postscript_stem_snap_h),
            ),
            (
                &self.font_info_inputs.stems_v,
                list(&info.postscript_stem_snap_v),
            ),
        ];
        for (entity, value) in values {
            entity.update(cx, |st, cx| {
                if st.value() != value.as_str() {
                    st.set_value(value, window, cx);
                }
            });
        }
    }

    /// Measured stem and bar of a glyph: the narrowest horizontal
    /// and vertical black spans between facing straight edges.
    /// (Counters are white spans; the midpoint containment test
    /// keeps only ink.)
    fn measured_dimensions(&self, name: &str) -> (Option<i64>, Option<i64>) {
        use kurbo::Shape as _;
        use runebender_core::measure::{self, MeasureKind};
        use runebender_core::model::workspace::Contour as WContour;
        let Some(font) = self.font() else {
            return (None, None);
        };
        let Some(g) = font.font.get_glyph(name) else {
            return (None, None);
        };
        if g.contours.is_empty() {
            return (None, None);
        }
        let paths: Vec<runebender_core::path::Path> = g
            .contours
            .iter()
            .map(|c| runebender_core::path::Path::from_contour(&WContour::from_norad(c)))
            .collect();
        let filled = runebender_core::glyph_paths::glyph_to_bezpath(g, &font.font);
        let black = |m: &measure::Measurement| {
            let mid = kurbo::Point::new((m.a.x + m.b.x) / 2.0, (m.a.y + m.b.y) / 2.0);
            filled.contains(mid)
        };
        let measurements = measure::glyph_measurements(&paths);
        let narrowest = |kind: MeasureKind| {
            measurements
                .iter()
                .filter(|m| m.kind == kind)
                .filter(|m| black(m))
                .map(|m| m.length)
                .min()
        };
        (
            narrowest(MeasureKind::Horizontal),
            narrowest(MeasureKind::Vertical),
        )
    }

    fn refresh_metric_inputs(&mut self, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        // The metric fields live in the Glyph panel, which is up in
        // both modes: in the grid they follow the selected cell.
        let Some(index) = (match self.mode {
            Mode::Editor(index) => Some(index),
            Mode::Grid => self.selected,
        }) else {
            return;
        };
        if !force {
            // Any focused element other than the workspace canvas
            // means an input might be active: leave the text alone.
            if window.focused(cx).is_some_and(|f| f != self.focus_handle) {
                return;
            }
        }
        let Some(font) = self.font() else {
            return;
        };
        let advance = font.glyphs[index].advance;
        let ink = font.ink_bounds(index);
        let (lsb, rsb) = match ink {
            Some(r) => (format!("{:.0}", r.x0), format!("{:.0}", advance - r.x1)),
            None => (String::new(), String::new()),
        };
        let width = format!("{advance:.0}");
        let set = |entity: &gpui::Entity<widgets::input::InputState>,
                   value: String,
                   window: &mut Window,
                   cx: &mut Context<Self>| {
            entity.update(cx, |st, cx| {
                if st.value() != value.as_str() {
                    st.set_value(value, window, cx);
                }
            });
        };
        set(&self.metric_inputs.width, width, window, cx);
        set(&self.metric_inputs.lsb, lsb, window, cx);
        set(&self.metric_inputs.rsb, rsb, window, cx);
    }

    /// The single selected point, if exactly one point is selected.
    fn single_selected_point(&self) -> Option<GlyphPoint> {
        let Mode::Editor(index) = self.mode else {
            return None;
        };
        if self.editor.selected.len() != 1 {
            return None;
        }
        let &(contour, point) = self.editor.selected.iter().next()?;
        self.font()?.glyphs[index]
            .points
            .iter()
            .find(|p| p.contour == contour && p.index == point)
            .copied()
    }

    /// Set one coordinate of the single selected point (Selection
    /// section X/Y inputs), with an undo snapshot.
    /// Rename the selected anchor (Enter in the Selection panel).
    fn apply_anchor_name(&mut self, text: &str) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        let Some(ai) = self.editor.selected_anchor() else {
            return;
        };
        let name = text.trim();
        if name.is_empty() {
            return;
        }
        let Ok(name) = norad::Name::new(name) else {
            self.status_note = Some(format!("Bad anchor name: {text}").into());
            return;
        };
        self.push_undo_snapshot(index);
        self.font_mut().and_then(|f| {
            f.edit_glyph(index, |g| {
                if let Some(anchor) = g.anchors.get_mut(ai) {
                    anchor.name = Some(name);
                }
            })
        });
    }

    /// Bounds of whatever is selected: points, else the component,
    /// else the anchor.
    /// The true bounding box of the selected segments: the curve's own
    /// extrema, not the box around its control points. A cubic's
    /// handles usually sit outside the ink, so the point box overstates
    /// how tall or wide a curve actually is — this is the number you
    /// want when matching one curve's size against another.
    ///
    /// `None` unless the selection covers whole segments.
    fn selected_segment_bounds(&self) -> Option<(usize, kurbo::Rect)> {
        use kurbo::Shape as _;
        let Mode::Editor(index) = self.mode else {
            return None;
        };
        if self.editor.selected.is_empty() {
            return None;
        }
        let font = self.font()?;
        let glyph = font.font.get_glyph(font.glyphs[index].name.as_ref())?;
        let mut bounds: Option<kurbo::Rect> = None;
        let mut count = 0usize;
        for hit in runebender_core::segment_ops::segments(glyph) {
            if !hit
                .point_ids()
                .iter()
                .all(|id| self.editor.selected.contains(id))
            {
                continue;
            }
            count += 1;
            let b = hit.seg.bounding_box();
            bounds = Some(match bounds {
                Some(acc) => acc.union(b),
                None => b,
            });
        }
        bounds.map(|b| (count, b))
    }

    fn selection_bounds(&self) -> Option<kurbo::Rect> {
        let Mode::Editor(index) = self.mode else {
            return None;
        };
        let font = self.font()?;
        let entry = &font.glyphs[index];
        if !self.editor.selected.is_empty() {
            let mut bounds: Option<kurbo::Rect> = None;
            for p in entry.points.iter() {
                if self.editor.selected.contains(&(p.contour, p.index)) {
                    let r = kurbo::Rect::new(p.x, p.y, p.x, p.y);
                    bounds = Some(match bounds {
                        Some(b) => b.union(r),
                        None => r,
                    });
                }
            }
            return bounds;
        }
        if let Some(ci) = self.editor.selected_component {
            use kurbo::Shape as _;
            let glyph = font.font.get_glyph(entry.name.as_ref())?;
            let component = glyph.components.get(ci)?;
            let base = font.font.get_glyph(component.base.as_str())?;
            let transform = runebender_core::glyph_paths::component_affine(&component.transform);
            let path =
                transform * &runebender_core::glyph_paths::glyph_to_bezpath(base, &font.font);
            return Some(path.bounding_box());
        }
        if let Some(ai) = self.editor.selected_anchor() {
            let (_, x, y) = entry.anchors.get(ai)?;
            return Some(kurbo::Rect::new(*x, *y, *x, *y));
        }
        None
    }

    /// Move whatever is selected so the quadrant reference lands on
    /// `value` along one axis (web move_selection_reference).
    fn apply_coord(&mut self, is_x: bool, value: f64) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        if !value.is_finite() {
            return;
        }
        let Some(bounds) = self.selection_bounds() else {
            return;
        };
        let reference = self.coord_quadrant.point_in_dspace_rect(bounds);
        let delta = if is_x {
            kurbo::Vec2::new(value - reference.x, 0.0)
        } else {
            kurbo::Vec2::new(0.0, value - reference.y)
        };
        if delta.hypot() < 1e-9 {
            return;
        }
        self.push_undo_snapshot(index);
        let changed = self.translate_selected(index, delta);
        if !changed {
            self.editor.undo.pop();
        }
    }

    /// Scale whatever is selected about the quadrant reference so its
    /// bounds reach `value` along one axis (web
    /// resize_selection_reference).
    fn apply_size(&mut self, is_width: bool, value: f64) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        if !value.is_finite() || value <= 0.0 {
            return;
        }
        let Some(bounds) = self.selection_bounds() else {
            return;
        };
        let current = if is_width {
            bounds.width()
        } else {
            bounds.height()
        };
        if current.abs() < 1e-9 {
            return;
        }
        let reference = self.coord_quadrant.point_in_dspace_rect(bounds);
        let scale = value / current;
        if (scale - 1.0).abs() < 1e-9 {
            return;
        }
        let (sx, sy) = if is_width { (scale, 1.0) } else { (1.0, scale) };
        let transform = Affine::translate(-reference.to_vec2())
            .then_scale_non_uniform(sx, sy)
            .then_translate(reference.to_vec2());
        self.editor.last_transform = Some(transform);
        self.push_undo_snapshot(index);
        let changed = self.transform_selected(index, transform);
        if !changed {
            self.editor.undo.pop();
        }
    }

    /// Translate the active selection (points, component, or anchor).
    fn translate_selected(&mut self, index: usize, delta: kurbo::Vec2) -> bool {
        if let Some(ci) = self.editor.selected_component {
            return self
                .font_mut()
                .and_then(|f| {
                    f.edit_glyph(index, |g| {
                        runebender_core::glyph_ops::translate_component(g, ci, delta.x, delta.y)
                    })
                })
                .unwrap_or(false);
        }
        if let Some(ai) = self.editor.selected_anchor() {
            let target = self.font().and_then(|f| {
                f.glyphs[index]
                    .anchors
                    .get(ai)
                    .map(|(_, x, y)| (x + delta.x, y + delta.y))
            });
            if let Some((x, y)) = target {
                if let Some(font) = self.font_mut() {
                    font.set_anchor(index, ai, x.round(), y.round());
                    return true;
                }
            }
            return false;
        }
        let selected = self.editor.selected.clone();
        if selected.is_empty() {
            return false;
        }
        self.font_mut()
            .and_then(|f| {
                f.edit_glyph(index, |g| {
                    runebender_core::glyph_ops::transform_selection(
                        g,
                        &selected,
                        Affine::translate(delta),
                    )
                })
            })
            .unwrap_or(false)
    }

    /// Transform the active selection (points, component, or anchor).
    fn transform_selected(&mut self, index: usize, transform: Affine) -> bool {
        if let Some(ci) = self.editor.selected_component {
            // Bake the scale into the component transform.
            return self
                .font_mut()
                .and_then(|f| {
                    f.edit_glyph(index, |g| {
                        let Some(component) = g.components.get_mut(ci) else {
                            return false;
                        };
                        let current =
                            runebender_core::glyph_paths::component_affine(&component.transform);
                        let combined = transform * current;
                        let c = combined.as_coeffs();
                        component.transform = norad::AffineTransform {
                            x_scale: c[0],
                            xy_scale: c[1],
                            yx_scale: c[2],
                            y_scale: c[3],
                            x_offset: c[4],
                            y_offset: c[5],
                        };
                        true
                    })
                })
                .unwrap_or(false);
        }
        if let Some(ai) = self.editor.selected_anchor() {
            let target = self.font().and_then(|f| {
                f.glyphs[index].anchors.get(ai).map(|(_, x, y)| {
                    let p = transform * kurbo::Point::new(*x, *y);
                    (p.x, p.y)
                })
            });
            if let Some((x, y)) = target {
                if let Some(font) = self.font_mut() {
                    font.set_anchor(index, ai, x.round(), y.round());
                    return true;
                }
            }
            return false;
        }
        let selected = self.editor.selected.clone();
        if selected.is_empty() {
            return false;
        }
        self.font_mut()
            .and_then(|f| {
                f.edit_glyph(index, |g| {
                    runebender_core::glyph_ops::transform_selection(g, &selected, transform)
                })
            })
            .unwrap_or(false)
    }

    /// Keep the Selection X/Y inputs showing the selected point.
    fn refresh_coord_inputs(&mut self, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !force && window.focused(cx).is_some_and(|f| f != self.focus_handle) {
            return;
        }
        let (x, y, w, h) = match self.selection_bounds() {
            Some(bounds) => {
                let reference = self.coord_quadrant.point_in_dspace_rect(bounds);
                (
                    format!("{:.0}", reference.x),
                    format!("{:.0}", reference.y),
                    format!("{:.0}", bounds.width()),
                    format!("{:.0}", bounds.height()),
                )
            }
            None => Default::default(),
        };
        let anchor_name = self
            .editor
            .selected_anchor()
            .and_then(|ai| {
                let Mode::Editor(index) = self.mode else {
                    return None;
                };
                self.font()
                    .and_then(|f| f.glyphs[index].anchors.get(ai).cloned())
            })
            .map(|(name, _, _)| name.to_string())
            .unwrap_or_default();
        for (entity, value) in [
            (self.metric_inputs.x.clone(), x),
            (self.metric_inputs.y.clone(), y),
            (self.metric_inputs.w.clone(), w),
            (self.metric_inputs.h.clone(), h),
            (self.anchor_name_input.clone(), anchor_name),
        ] {
            entity.update(cx, |st, cx| {
                if st.value() != value.as_str() {
                    st.set_value(value, window, cx);
                }
            });
        }
    }

    /// Lock the selected component back onto its anchor, or cut it
    /// loose. Unlocking leaves it exactly where it sits; locking
    /// snaps it home (the realign hook runs on the edit).
    fn toggle_component_alignment(&mut self, index: usize, ci: usize) {
        let currently_aligned = self
            .font()
            .and_then(|f| f.font.get_glyph(f.glyphs[index].name.as_ref()))
            .and_then(|g| g.components.get(ci))
            .map(|c| !runebender_core::composites::component_alignment_disabled(c));
        let Some(aligned) = currently_aligned else {
            return;
        };
        self.push_undo_snapshot(index);
        self.font_mut().and_then(|f| {
            f.edit_glyph(index, |g| {
                if let Some(component) = g.components.get_mut(ci) {
                    runebender_core::composites::set_component_alignment_disabled(
                        component, aligned,
                    );
                }
            })
        });
    }

    /// Read a model directory and cache the weights.
    /// Where a model is looked for when nobody points at one.
    ///
    /// `$RUNEBENDER_MODELS`, else `~/.runebender/models`. A model is a
    /// directory holding `config.json`, so dropping one in is the whole
    /// installation step: no rebuild, no account, no file picker.
    pub(crate) fn models_dir() -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os("RUNEBENDER_MODELS") {
            return Some(PathBuf::from(dir));
        }
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".runebender/models"))
    }

    /// Every model directory under `models_dir`, by name.
    ///
    /// Sorted, so the list does not reshuffle between launches on
    /// whatever order the filesystem hands back.
    pub(crate) fn installed_models() -> Vec<(String, PathBuf)> {
        let Some(root) = Self::models_dir() else {
            return Vec::new();
        };
        let Ok(entries) = std::fs::read_dir(&root) else {
            return Vec::new();
        };
        let mut found: Vec<(String, PathBuf)> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join("config.json").is_file())
            .filter_map(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| (n.to_string(), p.clone()))
            })
            .collect();
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found
    }

    fn load_model(&mut self, dir: &std::path::Path) {
        let checkpoint = match font_ml::Checkpoint::open(dir) {
            Ok(c) => c,
            Err(e) => {
                self.status_note = Some(format!("Model: {e}").into());
                return;
            }
        };
        match font_ml::outline::OutlineModel::load(&checkpoint) {
            Ok(model) => {
                self.model_summary = Some(checkpoint.summary().into());
                self.model_loaded = Some(std::rc::Rc::new(model));
                self.model_dir = Some(dir.to_path_buf());
                self.model_score = None;
                self.status_note = Some("Model loaded".into());
            }
            Err(e) => self.status_note = Some(format!("Model: {e}").into()),
        }
    }

    /// Run the model over the open glyph and install what it predicts.
    fn apply_bolden(&mut self, index: usize, dir: &std::path::Path) {
        let checkpoint = match font_ml::Checkpoint::open(dir) {
            Ok(c) => c,
            Err(e) => {
                self.status_note = Some(format!("Model: {e}").into());
                return;
            }
        };
        if self.model_loaded.is_none() {
            self.load_model(dir);
        }
        let Some(model) = self.model_loaded.clone() else {
            return;
        };
        let Some(font) = self.font() else { return };
        let Some(entry) = font.glyphs.get(index) else {
            return;
        };
        let name = entry.name.to_string();
        let advance = entry.advance;
        let unicode = entry.codepoint.map(|c| c as u32);
        let Some(glyph) = font.font.get_glyph(name.as_str()) else {
            return;
        };
        let Some(ops) = font_ml::ufo::glyph_ops(glyph) else {
            self.status_note =
                Some("Nothing to bolden: this glyph is built from components".into());
            return;
        };

        let center = checkpoint
            .config
            .delta_center
            .map(|c| (c[0], c[1]))
            .unwrap_or((0, 0));
        let result = match font_ml::bolden::bolden(
            model.as_ref(),
            &name,
            unicode,
            advance,
            &ops,
            center,
            checkpoint.config.trim_close,
            self.model_strength,
        ) {
            Ok(r) => r,
            Err(e) => {
                self.status_note = Some(format!("Bolden: {e}").into());
                return;
            }
        };
        // The encoding guarantees this; assert it before writing to a
        // font rather than take it on trust.
        if !result.is_compatible() {
            self.status_note = Some("Refused: the prediction changed the point structure".into());
            return;
        }

        let expected = glyph
            .contours
            .iter()
            .map(|c| c.points.len() + 1)
            .sum::<usize>();
        if result.deltas.len() != expected {
            self.status_note = Some(
                format!(
                    "Refused: model returned {} offsets for {expected} points",
                    result.deltas.len()
                )
                .into(),
            );
            return;
        }
        let contours = bolden_contours(glyph, &result.deltas, center);
        let moved = result
            .deltas
            .iter()
            .filter(|(x, y)| *x != 0 || *y != 0)
            .count();
        self.push_undo_snapshot(index);
        self.font_mut().and_then(|f| {
            f.edit_glyph(index, |g| {
                g.contours = contours.clone();
            })
        });
        self.editor.selected.clear();
        self.status_note = Some(
            format!(
                "Boldened {name}: {moved}/{} points moved, advance {:+}. Undo to reject.",
                result.deltas.len(),
                result.advance_delta
            )
            .into(),
        );
    }

    fn apply_place_image(&mut self, index: usize, path: &std::path::Path, bytes: Vec<u8>) {
        // Decode first: a file the editor cannot draw is refused
        // rather than silently written into the font.
        let decoded = match image::load_from_memory(&bytes) {
            Ok(img) => img,
            Err(e) => {
                self.status_note = Some(format!("Place image: {e}").into());
                return;
            }
        };
        let (img_w, img_h) = (decoded.width() as f64, decoded.height() as f64);
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "image.png".into());
        let Some(font) = self.font() else { return };
        let (ascender, descender) = (font.ascender, font.descender);
        let name = font.glyphs[index].name.to_string();
        let scale = ((ascender - descender) / img_h.max(1.0)).max(1e-6);
        let transform = norad::AffineTransform {
            x_scale: scale,
            xy_scale: 0.0,
            yx_scale: 0.0,
            y_scale: scale,
            x_offset: 0.0,
            y_offset: descender,
        };
        let image = match norad::Image::new(std::path::PathBuf::from(&file_name), None, transform) {
            Ok(image) => image,
            Err(e) => {
                self.status_note = Some(format!("Place image: {e}").into());
                return;
            }
        };
        if let Some(font) = self.font_mut() {
            // An existing entry under the same name is replaced.
            let _ = font
                .font
                .images
                .insert(std::path::PathBuf::from(&file_name), bytes);
            if let Some(glyph) = font.font.get_glyph_mut(name.as_str()) {
                glyph.image = Some(image);
            }
            font.dirty = true;
            font.modified_glyphs.insert(name);
        }
        // The cache entry is rebuilt from the store on next paint.
        self.glyph_image_cache.lock().unwrap().remove(&file_name);
        self.show_background = true;
        self.status_note = Some(format!("Placed {file_name} · {img_w:.0}×{img_h:.0}px").into());
    }

    fn glyph_smart_axis_ref(&self) -> gpui::Entity<widgets::input::InputState> {
        self.smart_axis_input.clone()
    }

    /// The decoded background image for a file in the UFO images
    /// store, cached. gpui's RenderImage wants premultiplied BGRA.
    fn glyph_image(&self, file_name: &str) -> Option<Arc<gpui::RenderImage>> {
        if let Some(cached) = self.glyph_image_cache.lock().unwrap().get(file_name) {
            return cached.clone();
        }
        let decoded = self
            .font()
            .and_then(|f| {
                f.font
                    .images
                    .get(std::path::Path::new(file_name))
                    .and_then(|r| r.ok())
            })
            .and_then(|bytes| image::load_from_memory(&bytes).ok())
            .map(|img| {
                let rgba = img.to_rgba8();
                let (w, h) = (rgba.width(), rgba.height());
                let mut bytes = rgba.into_raw();
                for px in bytes.chunks_exact_mut(4) {
                    let a = px[3] as u32;
                    // Swap to BGRA and premultiply in one pass.
                    let (r, g, b) = (px[0] as u32, px[1] as u32, px[2] as u32);
                    px[0] = ((b * a) / 255) as u8;
                    px[1] = ((g * a) / 255) as u8;
                    px[2] = ((r * a) / 255) as u8;
                }
                let buffer = image::RgbaImage::from_raw(w, h, bytes).expect("same-size buffer");
                Arc::new(gpui::RenderImage::new(vec![image::Frame::new(buffer)]))
            });
        self.glyph_image_cache
            .lock()
            .unwrap()
            .insert(file_name.to_string(), decoded.clone());
        decoded
    }

    fn apply_image_trace(&mut self, index: usize, bytes: &[u8]) {
        let Some(font) = self.font() else { return };
        let (ascender, descender) = (font.ascender, font.descender);
        let advance = font
            .glyphs
            .get(index)
            .map(|g| g.advance)
            .unwrap_or(runebender_core::new_font::DEFAULT_WIDTH);
        let config = runebender_core::image_trace::TraceConfig {
            target_height: (ascender - descender).max(1.0),
            y_offset: descender,
            advance: advance.max(1.0),
            ..Default::default()
        };
        match runebender_core::image_trace::trace_image(bytes, &config) {
            Ok(traced) => {
                self.push_undo_snapshot(index);
                let count = traced.contours.len();
                self.font_mut().and_then(|f| {
                    f.edit_glyph(index, |g| {
                        g.contours = traced.contours.clone();
                    })
                });
                self.editor.selected.clear();
                self.status_note = Some(
                    format!(
                        "Traced {count} contour{}",
                        if count == 1 { "" } else { "s" }
                    )
                    .into(),
                );
            }
            Err(e) => {
                self.status_note = Some(format!("Trace failed: {e}").into());
            }
        }
    }

    /// Flip/rotate the selection (whole glyph when nothing selected)
    /// about its bbox center, with an undo snapshot.
    fn apply_transform(&mut self, transform: Affine) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        self.push_undo_snapshot(index);
        self.editor.last_transform = Some(transform);
        let selected = self.editor.selected.clone();
        let changed = self
            .font_mut()
            .and_then(|f| {
                f.edit_glyph(index, |g| {
                    runebender_core::glyph_ops::transform_selection(g, &selected, transform)
                })
            })
            .unwrap_or(false);
        if !changed {
            self.editor.undo.pop();
        }
    }

    fn apply_curve_op(&mut self, op: CurveOp) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        self.push_undo_snapshot(index);
        let selected = self.editor.selected.clone();
        let changed = self
            .font_mut()
            .is_some_and(|f| f.curve_op(index, &selected, op));
        if !changed {
            // Nothing moved: drop the useless snapshot.
            self.editor.undo.pop();
        }
    }

    /// Push the open glyph's contours onto the undo stack and clear
    /// the redo tail. Called at the start of every mutating gesture.
    /// Apply a change to the measure options, mirror it for the menu,
    /// and rebuild the menus so the ticks follow.
    fn toggle_measure(&mut self, change: impl FnOnce(&mut MeasureOpts), cx: &mut Context<Self>) {
        change(&mut self.measure_opts);
        *MEASURE_MENU.lock().expect("measure menu") = self.measure_opts;
        cx.set_menus(app_menus());
        cx.notify();
    }

    fn push_undo_snapshot(&mut self, index: usize) {
        // Any other edit ends a nudge burst, so the next arrow press
        // opens a fresh undo group.
        self.nudging = false;
        if let Some(snapshot) = self.font().and_then(|f| f.snapshot_contours(index)) {
            self.editor.undo.push(snapshot);
            self.editor.redo.clear();
        }
    }

    /// Snapshot for a nudge: a run of arrow presses is one undo step,
    /// the way the web commits one group per burst
    /// (`finishNudgeSelection` on key-up).
    fn push_nudge_snapshot(&mut self, index: usize) {
        if self.nudging {
            return;
        }
        self.push_undo_snapshot(index);
        self.nudging = true;
    }

    // ---- app commands (menu bar + keymap) ----

    /// The repo's own Google Fonts build script above the source
    /// (build-fontc.sh preferred, then build.sh), with the directory
    /// to run it from. A repo pipeline carries the gftools fixes,
    /// STAT, and statics that a raw compile does not.
    #[cfg(not(target_family = "wasm"))]
    fn gf_build_script(source: &std::path::Path) -> Option<(PathBuf, PathBuf)> {
        let mut dir = source.parent()?;
        for _ in 0..4 {
            for name in ["build-fontc.sh", "build.sh"] {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some((candidate, dir.to_path_buf()));
                }
            }
            if dir.join(".git").exists() {
                break;
            }
            dir = dir.parent()?;
        }
        None
    }

    /// PATH for export child processes. The app may have been
    /// launched from the Dock with the minimal system PATH, so the
    /// places build scripts expect (cargo bin, Homebrew, the repo
    /// venv) are put back in front.
    #[cfg(not(target_family = "wasm"))]
    fn export_path_env(workdir: Option<&std::path::Path>) -> std::ffi::OsString {
        let mut parts: Vec<PathBuf> = Vec::new();
        if let Some(workdir) = workdir {
            parts.push(workdir.join(".venv/bin"));
        }
        if let Some(home) = std::env::var_os("HOME") {
            parts.push(PathBuf::from(&home).join(".cargo/bin"));
        }
        parts.push(PathBuf::from("/opt/homebrew/bin"));
        parts.push(PathBuf::from("/usr/local/bin"));
        if let Some(path) = std::env::var_os("PATH") {
            parts.extend(std::env::split_paths(&path));
        }
        std::env::join_paths(parts.into_iter().filter(|p| p.exists()))
            .unwrap_or_else(|_| std::env::var_os("PATH").unwrap_or_default())
    }

    /// Paste the system clipboard's text into the editor's buffer,
    /// character by character (web pasteTextIntoBuffer): switches to
    /// the Text tool, line breaks for newlines, characters with no
    /// glyph skipped.
    fn paste_text_into_buffer(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        if text.is_empty() {
            return;
        }
        if self.editor.tool != Tool::Text {
            self.editor.previous_tool = self.editor.tool;
            self.editor.tool = Tool::Text;
        }
        let mut inserted = 0usize;
        let mut skipped = 0usize;
        for c in text.chars() {
            if c == '\r' {
                continue;
            }
            if c == '\n' {
                self.edit_buffer.insert_line_break();
                inserted += 1;
                continue;
            }
            if self.edit_buffer.insert_character(c) {
                inserted += 1;
            } else {
                skipped += 1;
            }
        }
        if inserted == 0 && skipped == 0 {
            return;
        }
        self.edit_buffer.shape_arabic_if_rtl();
        self.sync_sort_offset();
        self.status_note = Some(
            if skipped > 0 {
                format!(
                    "pasted {inserted} character{} ({skipped} with no glyph skipped)",
                    if inserted == 1 { "" } else { "s" }
                )
            } else {
                format!(
                    "pasted {inserted} character{}",
                    if inserted == 1 { "" } else { "s" }
                )
            }
            .into(),
        );
    }

    fn undo(&mut self) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        let Some(previous) = self.editor.undo.pop() else {
            return;
        };
        if let Some(font) = self.font_mut() {
            let current = font.snapshot_contours(index);
            font.restore_contours(index, previous);
            if let Some(current) = current {
                self.editor.redo.push(current);
            }
        }
    }

    fn redo(&mut self) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        let Some(next) = self.editor.redo.pop() else {
            return;
        };
        if let Some(font) = self.font_mut() {
            let current = font.snapshot_contours(index);
            font.restore_contours(index, next);
            if let Some(current) = current {
                self.editor.undo.push(current);
            }
        }
    }

    /// Nudge the selected points by (dx, dy) design units.
    /// Arrow-key nudge, with the web's routing: a selected component
    /// moves alone; with no points selected an anchor moves; otherwise
    /// points move, carrying any selected anchors with them. Alt makes
    /// the move independent — selected points travel without their
    /// handles.
    fn nudge_selection(&mut self, dx: f64, dy: f64, independent: bool) -> bool {
        let Mode::Editor(index) = self.mode else {
            return false;
        };
        let selected = self.editor.selected.clone();
        let anchor = self.editor.selected_anchor();
        if let Some(ci) = self.editor.selected_component {
            self.push_nudge_snapshot(index);
            let changed = self
                .font_mut()
                .and_then(|f| {
                    f.edit_glyph(index, |g| {
                        runebender_core::glyph_ops::translate_component(g, ci, dx, dy)
                    })
                })
                .unwrap_or(false);
            if !changed {
                self.editor.undo.pop();
                self.nudging = false;
            }
            return changed;
        }
        if selected.is_empty() && anchor.is_none() {
            return false;
        }
        self.push_nudge_snapshot(index);
        let mut changed = false;
        if let Some(ai) = anchor
            && let Some(font) = self.font_mut()
            && let Some((x, y)) = font.glyphs[index].anchors.get(ai).map(|(_, x, y)| (*x, *y))
        {
            font.set_anchor(index, ai, x + dx, y + dy);
            changed = true;
        }
        if !selected.is_empty()
            && let Some(font) = self.font_mut()
        {
            changed |= font
                .edit_glyph(index, |g| {
                    runebender_core::point_ops::translate_points(
                        g,
                        &selected,
                        &std::collections::HashMap::new(),
                        (dx, dy),
                        independent,
                    )
                })
                .unwrap_or(false);
        }
        if !changed {
            self.editor.undo.pop();
            self.nudging = false;
        }
        changed
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
        // The wheel zooms about the cursor, always — the web editor's
        // `wheel()`, same 0.0015-per-pixel response and the same
        // limits, so a notch wheel and a trackpad flick both feel like
        // they do there. Panning is alt-drag, as it is in the web.
        let local = self.editor.window_to_local(event.position);
        let factor = (delta.1 * ZOOM_PER_PIXEL).exp();
        self.editor
            .viewport
            .zoom_about(local, factor, ZOOM_MIN, ZOOM_MAX);
        let _ = delta.0;
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let (title, status): (SharedString, SharedString) = match (self.font(), &self.load_error) {
            (Some(font), _) => (
                // Just the file name, like Glyphs' title. The glyph
                // count lives in the status bar; upm belongs to font
                // info, not the chrome.
                font.source_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| font.source_path.display().to_string())
                    .into(),
                if font.dirty {
                    "Not saved".into()
                } else {
                    match &self.last_save_label {
                        Some(at) => format!("Saved {at}").into(),
                        None => "Saved".into(),
                    }
                },
            ),
            (None, Some(err)) => ("Load failed".into(), err.clone()),
            (None, None) => ("Runebender".into(), "No font loaded".into()),
        };
        let in_editor = matches!(self.mode, Mode::Editor(_));
        div()
            .flex()
            .items_center()
            // The same 6px everywhere: from the window's edges to the
            // icon, and from the icon to the title.
            .gap_1p5()
            .px_1p5()
            .py_1p5()
            .bg(t::panel_bg())
            .border_b_1()
            .border_color(t::panel_outline())
            .child(
                div()
                    .id("toggle-left")
                    .w(px(TAB_H))
                    .h(px(TAB_H))
                    .rounded(t::radius_control())
                    .cursor_pointer()
                    .child(icon_svg(
                        "glyph-grid",
                        if self.left_collapsed {
                            t::text_muted()
                        } else {
                            t::text()
                        },
                    ))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.left_collapsed = !this.left_collapsed;
                        cx.notify();
                    })),
            )
            .when(cfg!(not(target_os = "macos")), |el| {
                #[cfg(not(target_os = "macos"))]
                let el = el.child(div().flex_none().child(self.app_menu_bar.clone()));
                el
            })
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .overflow_hidden()
                    .child(div().text_sm().text_color(t::text()).child(title))
                    .child(div().text_sm().text_color(t::status_yellow()).child(status)),
            )
            .when(
                // Always up in the editor, the Glyphs bottom-corner
                // toggle: direction is a property of the review, not
                // of the text tool.
                in_editor,
                |el| el.child(self.direction_toolbar(cx)),
            )
            .when(in_editor, |el| el.child(self.header_tools(cx)))
            .child(self.tab_strip(cx))
    }

    /// Create the axis sliders once a project with axes exists.
    fn ensure_axis_sliders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        if !self.axis_sliders.is_empty() || project.axes.is_empty() || project.model.is_none() {
            return;
        }
        let axes = project.axes.clone();
        for (i, axis) in axes.iter().enumerate() {
            if axis.max <= axis.min {
                continue; // degenerate axis: nothing to slide
            }
            // Start where the active master sits, not at the axis
            // default: opening a Bold master with the handle parked on
            // Regular means the first touch jumps the design.
            let here = project
                .master_locations
                .get(project.active)
                .and_then(|loc| loc.get(&axis.name).copied())
                .map(|normalized| {
                    runebender_core::var_model::denormalize_value(
                        normalized,
                        axis.min,
                        axis.default,
                        axis.max,
                    )
                })
                .unwrap_or(axis.default);
            let slider = cx.new(|_| {
                widgets::slider::SliderState::new()
                    .max(axis.max as f32)
                    .min(axis.min as f32)
                    .step(1.0)
                    .default_value(here as f32)
            });
            let axis_info = axis.clone();
            let sub = cx.subscribe_in(&slider, window, {
                move |this: &mut Workspace, _, event: &widgets::slider::SliderEvent, _window, cx| {
                    let widgets::slider::SliderEvent::Change(value) = event else {
                        return;
                    };
                    let raw = *value as f64;
                    let landed = {
                        let Some(project) = this.project.as_mut() else {
                            return;
                        };
                        project.location.insert(
                            axis_info.name.clone(),
                            runebender_core::var_model::normalize_value(
                                raw,
                                axis_info.min,
                                axis_info.default,
                                axis_info.max,
                            ),
                        );
                        project.master_at_location()
                    };
                    // Landing on a master hands editing back to it;
                    // anywhere else the canvas shows an instance.
                    if let Some(master) = landed {
                        this.switch_master(master);
                    }
                    cx.notify();
                }
            });
            self._subscriptions.push(sub);
            self.axis_sliders.push((i, slider));
        }
    }

    /// Google Fonts style linking for an instance name: RIBBI styles
    /// link inside the family; anything else becomes its own
    /// stylemap family with regular/italic, the shape gftools
    /// expects (Medium → "Family Medium" + regular).
    fn style_linking(family: &str, style: &str) -> (String, String) {
        match style.to_lowercase().as_str() {
            "regular" | "bold" | "italic" | "bold italic" => {
                (family.to_string(), style.to_lowercase())
            }
            lower => {
                if let Some(base) = lower.strip_suffix(" italic").map(|b| b.len()) {
                    (
                        format!("{family} {}", style[..base].trim()),
                        "italic".to_string(),
                    )
                } else {
                    (format!("{family} {style}"), "regular".to_string())
                }
            }
        }
    }

    /// Park the preview (and the sliders) on a normalized location.
    /// Landing exactly on a master switches to it, the same contract
    /// as dragging a slider there.
    fn go_to_location(
        &mut self,
        target: &runebender_core::var_model::Location,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let landed = {
            let Some(project) = self.project.as_mut() else {
                return;
            };
            project.location = target.clone();
            project.master_at_location()
        };
        // Sliders show design coordinates; the location is normalized.
        let slider_values: Vec<(gpui::Entity<widgets::slider::SliderState>, f32)> = {
            let Some(project) = self.project.as_ref() else {
                return;
            };
            self.axis_sliders
                .iter()
                .filter_map(|(axis_index, slider)| {
                    let axis = project.axes.get(*axis_index)?;
                    let normalized = target.get(&axis.name).copied().unwrap_or(0.0);
                    let raw = runebender_core::var_model::denormalize_value(
                        normalized,
                        axis.min,
                        axis.default,
                        axis.max,
                    );
                    Some((slider.clone(), raw as f32))
                })
                .collect()
        };
        for (slider, value) in slider_values {
            slider.update(cx, |st, cx| {
                st.set_value(value, window, cx);
            });
        }
        if let Some(master) = landed {
            self.switch_master(master);
        }
        cx.notify();
    }

    /// Bottom bar in editor mode: Width / LSB / RSB fields.
    /// The resolved preview line: glyph index, pen x position (font
    /// units, kerning applied), and advance.
    /// The text sort metric box bounds: top = max(upm, ascender),
    /// bottom = descender — the web editor's text_sort_metric_bounds.
    fn text_sort_bounds(&self) -> (f64, f64) {
        let Some(font) = self.font() else {
            return (1000.0, -200.0);
        };
        (font.units_per_em.max(font.ascender), font.descender)
    }

    /// Line height for the text buffers: the sort box height, so a
    /// second line's box top sits exactly on the first line's bottom.
    fn text_line_height(&self) -> f64 {
        let (top, bottom) = self.text_sort_bounds();
        (top - bottom).max(1.0)
    }

    /// Rebuild the text engine's font models from the active master
    /// (glyph advances, unicode map, kerning with groups, features
    /// for shaping), and refresh the advances of sorts already in
    /// the buffer.
    fn rebuild_text_models(&mut self) {
        let Some(font) = self.project.as_ref().map(|p| p.active_font()) else {
            return;
        };
        let inventory = runebender_core::text::TextGlyphInventory::from_font(&font.font);
        let kerning = runebender_core::text::TextKerningModel::from_font(&font.font);
        let edit_widths: Vec<(usize, String, Option<char>, f64)> = (0..self.edit_buffer.len())
            .filter_map(|i| {
                let sort = self.edit_buffer.sort(i)?;
                let name = sort.glyph_name()?.to_string();
                let index = *font.name_map.get(&name)?;
                Some((
                    i,
                    name,
                    font.glyphs[index].codepoint,
                    font.glyphs[index].advance,
                ))
            })
            .collect();
        self.edit_buffer.set_glyph_inventory(inventory);
        self.edit_buffer.set_kerning_model(kerning);
        for (i, name, codepoint, advance) in edit_widths {
            self.edit_buffer.update_glyph(i, name, codepoint, advance);
        }
        self.sync_sort_offset();
    }

    /// Keep the editor's glyph-space offset in step with the active
    /// sort's layout position.
    fn sync_sort_offset(&mut self) {
        if self.font().is_none() {
            return;
        }
        let line_height = self.text_line_height();
        let offset = self
            .edit_buffer
            .active_sort()
            .and_then(|active| {
                let layout = self.edit_buffer.layout(line_height);
                layout
                    .items
                    .iter()
                    .find(|item| item.index == active)
                    .map(|item| (item.x, item.y))
            })
            .unwrap_or((0.0, 0.0));
        self.editor.sort_offset = offset;
    }

    /// Text tool click: place the caret (like the web editor). A
    /// shift-click on a sort begins a manual kerning drag instead.
    fn text_tool_click(&mut self, pos: Point<gpui::Pixels>, shift: bool) {
        if self.font().is_none() {
            return;
        }
        let line_height = self.text_line_height();
        let (top, bottom) = self.text_sort_bounds();
        let (dx, dy) = self.editor.window_to_design(pos);
        // window_to_design is glyph-local; the buffer wants buffer space.
        let bx = dx + self.editor.sort_offset.0;
        let by = dy + self.editor.sort_offset.1;
        if shift {
            let hit = self.edit_buffer.hit_test(bx, by, line_height, top, bottom);
            if let Some(index) = hit.active_sort {
                if self.edit_buffer.begin_manual_kerning(index, bx) {
                    self.editor.drag = Some(Drag::TextKern);
                    self.sync_sort_offset();
                    return;
                }
            }
        }
        self.edit_buffer
            .place_cursor_at(bx, by, line_height, top, bottom);
    }

    /// Double-click editing, in the web's priority order: toggle the
    /// point type under the cursor, else select its whole contour.
    fn double_click_edit(&mut self, pos: Point<gpui::Pixels>) -> bool {
        let Mode::Editor(index) = self.mode else {
            return false;
        };
        let Some(font) = self.font() else {
            return false;
        };
        let (dx, dy) = self.editor.window_to_design(pos);
        let tolerance = HIT_RADIUS_PX / self.editor.zoom();
        // On-curve point under the cursor: toggle smooth/corner.
        let point_hit = font.glyphs[index]
            .points
            .iter()
            .filter(|p| p.on_curve)
            .map(|p| {
                let dist = ((p.x - dx).powi(2) + (p.y - dy).powi(2)).sqrt();
                (dist, (p.contour, p.index))
            })
            .filter(|(dist, _)| *dist <= tolerance)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id);
        if let Some(id) = point_hit {
            self.push_undo_snapshot(index);
            let set: std::collections::HashSet<_> = [id].into();
            let changed = self
                .font_mut()
                .is_some_and(|f| f.toggle_smooth(index, &set));
            if !changed {
                self.editor.undo.pop();
            }
            return changed;
        }
        // A segment under the cursor: select its whole contour.
        let seg = font
            .font
            .get_glyph(font.glyphs[index].name.as_ref())
            .and_then(|g| {
                runebender_core::segment_ops::nearest_segment_with_t(
                    g,
                    kurbo::Point::new(dx, dy),
                    tolerance,
                )
            });
        if let Some((seg_hit, _)) = seg {
            let contour = seg_hit.contour;
            self.editor.selected = font.glyphs[index]
                .points
                .iter()
                .filter(|p| p.contour == contour)
                .map(|p| (p.contour, p.index))
                .collect();
            return true;
        }
        // A component under the cursor: open its base glyph beside
        // the sort being edited (web openTextGlyphBesideActive) — the
        // base belongs next to the glyph that uses it, not wherever
        // the cursor was left.
        let base = font
            .font
            .get_glyph(font.glyphs[index].name.as_ref())
            .and_then(|g| {
                runebender_core::glyph_ops::component_at(&font.font, g, kurbo::Point::new(dx, dy))
                    .map(|ci| g.components[ci].base.to_string())
            });
        if let Some(base_name) = base
            && let Some(&target) = font.name_map.get(&base_name)
        {
            let codepoint = font.glyphs[target].codepoint;
            let advance = font.glyphs[target].advance;
            self.edit_buffer
                .insert_glyph_after_active(base_name, codepoint, advance);
            self.edit_buffer.shape_arabic_if_rtl();
            self.mode = Mode::Editor(target);
            self.selected = Some(target);
            self.editor.selected.clear();
            self.editor.selected_anchors.clear();
            self.editor.selected_component = None;
            self.editor.drag = None;
            self.editor.undo.clear();
            self.editor.redo.clear();
            self.sync_sort_offset();
            return true;
        }
        false
    }

    /// Double-click on a sort (any tool): activate it and follow it
    /// in the editor, keeping the buffer.
    fn activate_sort_at_pos(&mut self, pos: Point<gpui::Pixels>) -> bool {
        if self.font().is_none() {
            return false;
        }
        let line_height = self.text_line_height();
        let (top, bottom) = self.text_sort_bounds();
        let (dx, dy) = self.editor.window_to_design(pos);
        let bx = dx + self.editor.sort_offset.0;
        let by = dy + self.editor.sort_offset.1;
        let Some(activation) = self
            .edit_buffer
            .activate_sort_at(bx, by, line_height, top, bottom)
        else {
            return false;
        };
        let name = self
            .edit_buffer
            .sort(activation.index)
            .and_then(|s| s.glyph_name())
            .map(str::to_string);
        let target = name.and_then(|n| self.font().and_then(|f| f.name_map.get(&n).copied()));
        if let Some(glyph) = target {
            if !matches!(self.mode, Mode::Editor(i) if i == glyph) {
                self.mode = Mode::Editor(glyph);
                self.selected = Some(glyph);
                self.editor.selected.clear();
                self.editor.selected_anchors.clear();
                self.editor.drag = None;
                self.editor.undo.clear();
                self.editor.redo.clear();
            }
        }
        self.sync_sort_offset();
        true
    }

    /// Write the buffer's kerning (updated by a manual kern drag)
    /// back into the font, wholesale like the web editor does.
    fn sync_kerning_from_buffer(&mut self) {
        let pairs = self.edit_buffer.kerning_model().pairs().clone();
        if let Some(font) = self.font_mut() {
            font.font.kerning = pairs
                .into_iter()
                .map(|(first, seconds)| {
                    (
                        norad::Name::new(&first).expect("kerning key name"),
                        seconds
                            .into_iter()
                            .filter_map(|(second, v)| {
                                norad::Name::new(&second).ok().map(|n| (n, v))
                            })
                            .collect(),
                    )
                })
                .filter_map(
                    |(first, seconds): (
                        norad::Name,
                        std::collections::BTreeMap<norad::Name, f64>,
                    )| { Some((first, seconds)) },
                )
                .collect();
            font.kerning_dirty = true;
            font.dirty = true;
        }
        self.rebuild_text_models();
    }

    /// Seed the editor's text buffer for an opened glyph    /// Seed the editor's text buffer for an opened glyph: keep the
    /// buffer when the glyph is already a sort in it (the text tool
    /// walking between sorts), otherwise start fresh with this glyph
    /// as the single active sort.
    fn seed_edit_buffer(&mut self, index: usize) {
        let Some((name, codepoint, advance)) = self.font().map(|font| {
            let entry = &font.glyphs[index];
            (entry.name.to_string(), entry.codepoint, entry.advance)
        }) else {
            return;
        };
        let existing = (0..self.edit_buffer.len()).find(|&i| {
            self.edit_buffer
                .sort(i)
                .and_then(|s| s.glyph_name())
                .is_some_and(|n| n == name)
        });
        match existing {
            Some(i) => {
                self.edit_buffer.activate_sort(i);
            }
            None => {
                self.edit_buffer.clear();
                self.edit_buffer.insert_glyph(name, codepoint, advance);
                self.edit_buffer.activate_sort(0);
            }
        }
        self.sync_sort_offset();
    }

    /// The preview's on/off switch, in the bottom bar's left corner
    /// where the tool hints used to be.
    fn preview_toggle(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .flex()
            .items_center()
            .gap_1()
            .flex_none()
            .child(
                div()
                    .id("preview-eye")
                    .flex_none()
                    .cursor_pointer()
                    .child(eye_icon(
                        if self.preview_visible {
                            t::accent()
                        } else {
                            t::text_muted()
                        },
                        self.preview_visible,
                    ))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.preview_visible = !this.preview_visible;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("preview-invert")
                    .flex_none()
                    .cursor_pointer()
                    .child(invert_icon(if self.preview_invert {
                        t::accent()
                    } else {
                        t::text_muted()
                    }))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.preview_invert = !this.preview_invert;
                        cx.notify();
                    })),
            )
    }

    /// What is left on the right of the bar: the blur, which is a
    /// spacing check. Show/hide and the ink flip live in the left
    /// corner beside each other.
    fn preview_controls(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .flex()
            .items_center()
            .gap_2()
            .flex_none()
            .child(div().text_xs().text_color(t::text_muted()).child("blur"))
            .children(self.preview_blur_slider.as_ref().map(|slider| {
                // The thumb hangs past both ends of the track, so the
                // slider gets its own room rather than sitting on the
                // label.
                div().w(px(90.0)).mr_1().child(flat_slider(slider, cx))
            }))
    }

    /// Create the bottom bar's cell-size slider once a window exists.
    fn ensure_preview_slider(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.preview_blur_slider.is_some() {
            return;
        }
        let slider = cx.new(|_| {
            widgets::slider::SliderState::new()
                .max(12.0)
                .min(0.0)
                .step(0.5)
                .default_value(0.0)
        });
        let sub = cx.subscribe_in(&slider, window, {
            move |this: &mut Workspace, _, event: &widgets::slider::SliderEvent, _window, cx| {
                let widgets::slider::SliderEvent::Change(value) = event else {
                    return;
                };
                this.preview_blur = *value;
                cx.notify();
            }
        });
        self._subscriptions.push(sub);
        self.preview_blur_slider = Some(slider);
    }

    /// The strength control for model predictions.
    fn ensure_model_strength_slider(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.model_strength_slider.is_some() {
            return;
        }
        let slider = cx.new(|_| {
            widgets::slider::SliderState::new()
                .min(0.25)
                .max(3.0)
                .step(0.05)
                .default_value(1.0)
        });
        let sub = cx.subscribe_in(&slider, window, {
            move |this: &mut Workspace, _, event: &widgets::slider::SliderEvent, _window, cx| {
                let widgets::slider::SliderEvent::Change(value) = event else {
                    return;
                };
                this.model_strength = *value as f64;
                // The last judgement was made at the old strength.
                this.model_score = None;
                cx.notify();
            }
        });
        self._subscriptions.push(sub);
        self.model_strength_slider = Some(slider);
    }

    fn ensure_sidebar_slider(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sidebar_slider.is_some() {
            return;
        }
        let slider = cx.new(|_| {
            widgets::slider::SliderState::new()
                .max(120.0)
                .min(24.0)
                .step(2.0)
                .default_value(MINI_CELL)
        });
        let sub = cx.subscribe_in(&slider, window, {
            move |this: &mut Workspace, _, event: &widgets::slider::SliderEvent, _window, cx| {
                let widgets::slider::SliderEvent::Change(value) = event else {
                    return;
                };
                this.sidebar_cell_size = *value;
                this.sidebar_scroll_row = 0;
                cx.notify();
            }
        });
        self._subscriptions.push(sub);
        self.sidebar_slider = Some(slider);
    }

    fn ensure_cell_slider(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cell_slider.is_some() {
            return;
        }
        let slider = cx.new(|_| {
            widgets::slider::SliderState::new()
                .max(200.0)
                .min(48.0)
                .step(4.0)
                .default_value(CELL)
        });
        let sub = cx.subscribe_in(&slider, window, {
            move |this: &mut Workspace, _, event: &widgets::slider::SliderEvent, _window, cx| {
                let widgets::slider::SliderEvent::Change(value) = event else {
                    return;
                };
                this.grid_cell_size = *value;
                cx.notify();
            }
        });
        self._subscriptions.push(sub);
        self.cell_slider = Some(slider);
    }

    fn status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        // Grid mode gets the Glyphs bottom bar: add/remove glyph on
        // the left, the selection count centered, cell zoom on the
        // right.
        if !matches!(self.mode, Mode::Editor(_)) && self.project.is_some() {
            let total = self.font().map(|f| f.glyphs.len()).unwrap_or(0);
            // The same list the grid draws, already filtered: counting
            // it again meant another pass over the whole font per frame.
            let shown = self.glyph_order().len();
            let center: SharedString = match &self.status_note {
                Some(note) => note.clone(),
                None => format!(
                    "{} selected · {shown}/{total} glyphs",
                    usize::from(self.selected.is_some())
                )
                .into(),
            };
            let bar_button = |id: &'static str, mark: IconMark| {
                div()
                    .id(id)
                    .w(px(BAR_BUTTON))
                    .h(px(BAR_BUTTON))
                    .rounded(t::radius())
                    .border(t::stroke())
                    .border_color(t::cell_border())
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(glyph_free_icon(t::text(), mark))
            };
            return div()
                .h(px(BOTTOM_BAR_H))
                .flex()
                .items_center()
                .gap_1()
                .px(px((BOTTOM_BAR_H - BAR_BUTTON) / 2.0))
                .bg(t::panel_bg())
                .border_t_1()
                .border_color(t::cell_border())
                .child(
                    bar_button("add-glyph", IconMark::Plus).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.command_add_glyph();
                            cx.notify();
                        },
                    )),
                )
                .child(
                    bar_button("remove-glyph", IconMark::Minus).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.command_remove_glyph();
                            cx.notify();
                        },
                    )),
                )
                .child(
                    div()
                        .flex_1()
                        .text_center()
                        .text_sm()
                        .text_color(t::text_muted())
                        .child(center),
                )
                .child({
                    // Grid · Detail · List, the Glyphs 4 view modes,
                    // beside the cell zoom.
                    let mode_button =
                        |id: &'static str,
                         label: &'static str,
                         mode: FontViewMode,
                         current: FontViewMode,
                         cx: &mut Context<Self>| {
                            div()
                                .id(id)
                                .px_1p5()
                                .rounded(t::radius())
                                .text_xs()
                                .cursor_pointer()
                                .text_color(if mode == current {
                                    t::accent()
                                } else {
                                    t::text_muted()
                                })
                                .child(label)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.font_view_mode = mode;
                                    cx.notify();
                                }))
                        };
                    let current = self.font_view_mode;
                    div()
                        .flex()
                        .items_center()
                        .gap_0p5()
                        .mr_2()
                        .child(mode_button(
                            "view-grid",
                            "Grid",
                            FontViewMode::Grid,
                            current,
                            cx,
                        ))
                        .child(mode_button(
                            "view-detail",
                            "Detail",
                            FontViewMode::Detail,
                            current,
                            cx,
                        ))
                        .child(mode_button(
                            "view-list",
                            "List",
                            FontViewMode::List,
                            current,
                            cx,
                        ))
                        .child(mode_button(
                            "view-matrix",
                            "Forms",
                            FontViewMode::Matrix,
                            current,
                            cx,
                        ))
                })
                .children(
                    self.cell_slider
                        .as_ref()
                        .map(|slider| div().w(px(140.0)).child(flat_slider(slider, cx))),
                );
        }
        let text: SharedString = if let Some(note) = &self.status_note {
            note.clone()
        } else {
            match (&self.mode, self.selected, self.font()) {
                (Mode::Editor(_), _, Some(_)) => {
                    // No standing hint text here: the tool cheatsheet
                    // was permanent clutter. Only live readouts and
                    // transient notes speak.
                    if let Some(Drag::Measure { start, current }) = &self.editor.drag {
                        let (dx, dy) = (current.0 - start.0, current.1 - start.1);
                        let len = (dx * dx + dy * dy).sqrt();
                        let angle = dy.atan2(dx).to_degrees();
                        return div()
                            .px_4()
                            .py_1()
                            .bg(t::panel_bg())
                            .border_t_1()
                            .border_color(t::cell_border())
                            .text_sm()
                            .text_color(t::text_muted())
                            .child(SharedString::from(format!(
                                "dx {dx:.0} · dy {dy:.0} · length {len:.1} · angle {angle:.1}°"
                            )));
                    }
                    SharedString::default()
                }
                (_, Some(i), Some(font)) => {
                    let g = &font.glyphs[i];
                    match g.codepoint {
                        Some(c) => {
                            format!("{} · U+{:04X} · advance {}", g.name, c as u32, g.advance)
                                .into()
                        }
                        None => format!("{} · unencoded · advance {}", g.name, g.advance).into(),
                    }
                }
                _ => "Click a glyph; double-click to edit · Cmd+O opens a font".into(),
            }
        };
        div()
            .h(px(BOTTOM_BAR_H))
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .bg(t::panel_bg())
            .border_t_1()
            .border_color(t::cell_border())
            .children(matches!(self.mode, Mode::Editor(_)).then(|| self.preview_toggle(cx)))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .text_sm()
                    .text_color(t::text_muted())
                    .child(text),
            )
            .children(matches!(self.mode, Mode::Editor(_)).then(|| self.preview_controls(cx)))
    }

    /// Watch every master's UFO directory; external changes reload
    /// the affected masters (in-memory edits are never clobbered:
    /// dirty masters skip the reload with a status note). Our own
    /// saves are suppressed via the last_save timestamp.
    #[cfg(target_family = "wasm")]
    fn start_watching(&mut self, _cx: &mut Context<Self>) {
        // No filesystem on the web: live reload will ride the host
        // data layer instead.
    }

    #[cfg(not(target_family = "wasm"))]
    fn start_watching(&mut self, cx: &mut Context<Self>) {
        use futures::StreamExt;
        self._watcher = None;
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
        let mut watcher =
            match notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                if res.is_ok() {
                    let _ = tx.unbounded_send(());
                }
            }) {
                Ok(w) => w,
                Err(_) => return,
            };
        for master in &project.masters {
            let _ = notify::Watcher::watch(
                &mut watcher,
                &master.source_path,
                notify::RecursiveMode::Recursive,
            );
        }
        self._watcher = Some(watcher);
        let last_save = self.last_save.clone();
        cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                // Debounce: drain everything arriving in the next
                // half second into one reload.
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(500))
                    .await;
                while rx.try_recv().is_ok() {}
                if last_save.lock().unwrap().elapsed() < std::time::Duration::from_secs(2) {
                    continue;
                }
                if this
                    .update(cx, |workspace, cx| {
                        workspace.reload_from_disk();
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// Re-read every clean master from disk, keeping the open glyph.
    fn reload_from_disk(&mut self) {
        self.sidebar_counts = None;
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let open_glyph_name = match self.mode {
            Mode::Editor(i) => Some(project.active_font().glyphs[i].name.clone()),
            Mode::Grid => None,
        };
        let mut skipped_dirty = false;
        for master in project.masters.iter_mut() {
            if master.dirty {
                skipped_dirty = true;
                continue;
            }
            if let Ok(fresh) = FontModel::load(&master.source_path) {
                *master = fresh;
            }
        }
        if let Some(name) = open_glyph_name {
            match project
                .active_font()
                .glyphs
                .iter()
                .position(|g| g.name == name)
            {
                Some(index) => {
                    self.mode = Mode::Editor(index);
                    self.editor.selected.clear();
                    self.editor.selected_anchors.clear();
                    self.editor.drag = None;
                }
                None => self.mode = Mode::Grid,
            }
        }
        self.status_note = Some(if skipped_dirty {
            "Changed on disk · dirty masters kept your unsaved edits".into()
        } else {
            "Reloaded from disk".into()
        });
    }

    /// Connect to the workspace server named by ?server= and load
    /// its fonts (web builds).
    #[cfg(target_family = "wasm")]
    fn connect_web_host(&mut self, base: String, cx: &mut Context<Self>) {
        self.status_note = Some(format!("Connecting to {base}…").into());
        let client = cx.http_client();
        cx.spawn(async move |this, cx| {
            let fetched = web_host::fetch_workspace(client, base.clone()).await;
            this.update(cx, |workspace, cx| {
                match fetched.and_then(|fetched| {
                    Project::from_fetched(&fetched).map(|built| (fetched, built))
                }) {
                    Ok((fetched, (project, ufo_prefixes))) => {
                        let n = project.masters.len();
                        workspace.axis_sliders.clear();
                        workspace.sessions.clear();
                        workspace.active_session = 0;
                        workspace.last_editor = None;
                        workspace.project = Some(project);
                        workspace.sidebar_counts = None;
                        workspace.load_error = None;
                        workspace.mode = Mode::Grid;
                        workspace.selected = None;
                        workspace.web_host = Some(web_host::WebHost {
                            base,
                            etags: fetched.etags,
                            ufo_prefixes,
                        });
                        workspace.status_note = Some(
                            format!("Connected · {n} masters · Cmd+S saves to the server").into(),
                        );
                    }
                    Err(e) => {
                        workspace.load_error = Some(format!("{e}").into());
                        workspace.status_note = None;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Save dirty masters to the workspace server (web builds):
    /// modified glifs and kerning, each PUT with its If-Match ETag.
    #[cfg(target_family = "wasm")]
    fn save_to_web_host(&mut self, cx: &mut Context<Self>) {
        let Some(host) = self.web_host.as_ref() else {
            self.status_note = Some("No server connected: open with ?server=http://…".into());
            return;
        };
        let Some(project) = self.project.as_ref() else {
            return;
        };
        // Collect the files to write while we hold &self.
        let mut to_save: Vec<web_host::SaveFile> = Vec::new();
        let mut saved_masters: Vec<usize> = Vec::new();
        for (mi, master) in project.masters.iter().enumerate() {
            if !master.dirty {
                continue;
            }
            let Some(prefix) = host.ufo_prefixes.get(mi) else {
                continue;
            };
            for name in &master.modified_glyphs {
                let Some(glyph) = master.font.get_glyph(name.as_str()) else {
                    continue;
                };
                let Some(rel) = master.glif_paths.get(name) else {
                    continue;
                };
                match runebender_core::font_memory::glif_bytes(glyph) {
                    Ok(bytes) => to_save.push(web_host::SaveFile {
                        path: format!("{prefix}{rel}"),
                        bytes,
                    }),
                    Err(e) => {
                        self.status_note = Some(format!("{e}").into());
                        return;
                    }
                }
            }
            if master.kerning_dirty {
                match runebender_core::font_memory::kerning_plist_bytes(&master.font) {
                    Ok(bytes) => to_save.push(web_host::SaveFile {
                        path: format!("{prefix}kerning.plist"),
                        bytes,
                    }),
                    Err(e) => {
                        self.status_note = Some(format!("{e}").into());
                        return;
                    }
                }
            }
            saved_masters.push(mi);
        }
        if to_save.is_empty() {
            self.status_note = Some("Nothing to save".into());
            return;
        }
        let base = host.base.clone();
        let etags: std::collections::HashMap<String, String> = host.etags.clone();
        let client = cx.http_client();
        let count = to_save.len();
        self.status_note = Some(format!("Saving {count} files…").into());
        cx.spawn(async move |this, cx| {
            let mut new_etags: Vec<(String, String)> = Vec::new();
            let mut failure: Option<String> = None;
            for file in &to_save {
                match web_host::put_file(
                    &client,
                    &base,
                    file,
                    etags.get(&file.path).map(|s| s.as_str()),
                )
                .await
                {
                    Ok(etag) => new_etags.push((file.path.clone(), etag)),
                    Err(e) => {
                        failure = Some(e);
                        break;
                    }
                }
            }
            this.update(cx, |workspace, cx| {
                if let Some(host) = workspace.web_host.as_mut() {
                    for (path, etag) in new_etags {
                        host.etags.insert(path, etag);
                    }
                }
                workspace.status_note = Some(match failure {
                    Some(e) => format!("Save failed: {e}").into(),
                    None => {
                        if let Some(project) = workspace.project.as_mut() {
                            for mi in saved_masters {
                                if let Some(master) = project.masters.get_mut(mi) {
                                    master.dirty = false;
                                    master.modified_glyphs.clear();
                                    master.kerning_dirty = false;
                                }
                            }
                        }
                        format!("Saved {count} files to the server").into()
                    }
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Cmd+O: native open dialog. Directories are selectable, so a
    /// .ufo and a .glyphspackage come through the same way a
    /// .designspace does.
    fn open_dialog(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let loaded = Project::load(&path);
            this.update(cx, |workspace, cx| {
                match loaded {
                    Ok(project) => {
                        workspace.axis_sliders.clear();
                        workspace.sessions.clear();
                        workspace.active_session = 0;
                        workspace.last_editor = None;
                        workspace.project = Some(project);
                        workspace.sidebar_counts = None;
                        workspace.load_error = None;
                        workspace.mode = Mode::Grid;
                        workspace.selected = None;
                        workspace.status_note = None;
                        workspace.search_query.clear();
                        workspace.rebuild_text_models();
                        workspace.start_watching(cx);
                    }
                    Err(e) => workspace.load_error = Some(e.into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

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
                                runebender_core::glyph_ops::convert_hyper_to_cubic(g, &selected)
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

fn main() {
    #[cfg(target_family = "wasm")]
    gpui_platform::web_init();

    // `runebender-gpui --fonts` lists the families gpui can resolve
    // and exits without opening a window. A family it cannot resolve
    // shapes to nothing and the interface comes up wordless, so this
    // is the first thing to check when that happens.
    #[cfg(not(target_family = "wasm"))]
    if std::env::args().any(|a| a == "--fonts") {
        gpui_platform::application().run(|cx: &mut App| {
            let names = cx.text_system().all_font_names();
            println!("{} families", names.len());
            for name in names {
                // Listed is not the same as loadable: report which
                // families actually resolve, and to what.
                let font = gpui::font(name.as_str());
                let resolved = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    cx.text_system().resolve_font(&font)
                }));
                match resolved {
                    Ok(id) => {
                        let got = cx.text_system().get_font_for_id(id);
                        let same = got.as_ref().is_some_and(|f| f.family == name.as_str());
                        println!("{name}\t{}", if same { "ok" } else { "FELL BACK" });
                    }
                    Err(_) => println!("{name}\tPANICKED"),
                }
            }
            // Can the resolved family actually measure a glyph? If
            // this errors the pipeline is broken; if it works the
            // fault is in the style, not the font.
            let font = gpui::font(ui_font_family(cx).as_ref());
            let id = cx.text_system().resolve_font(&font);
            println!(
                "chosen: {} em_width={:?} advance={:?}",
                ui_font_family(cx),
                cx.text_system().em_width(id, gpui::px(13.0)),
                cx.text_system().advance(id, gpui::px(13.0), 'A'),
            );
            cx.quit();
        });
        return;
    }

    #[cfg(not(target_family = "wasm"))]
    let (project, load_error) = {
        let font_path = std::env::args()
            .nth(1)
            .map(PathBuf::from)
            .unwrap_or_else(default_font_path);
        match Project::load(&font_path) {
            Ok(p) => (Some(p), None),
            Err(e) => (None, Some(e.into())),
        }
    };
    // The web build has no filesystem: open the embedded demo
    // designspace (a host data layer over fetch comes later).
    #[cfg(target_family = "wasm")]
    let (project, load_error): (Option<Project>, Option<SharedString>) =
        match Project::demo_embedded() {
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
        cx.bind_keys([
            gpui::KeyBinding::new("cmd-o", OpenFont, None),
            gpui::KeyBinding::new("cmd-n", NewFont, None),
            gpui::KeyBinding::new("cmd-shift-s", SaveFontAs, None),
            gpui::KeyBinding::new("cmd-e", ExportFont, None),
            gpui::KeyBinding::new("cmd-s", SaveFont, None),
            gpui::KeyBinding::new("cmd-z", Undo, None),
            gpui::KeyBinding::new("cmd-shift-z", Redo, None),
            gpui::KeyBinding::new("cmd-c", CopyContours, None),
            gpui::KeyBinding::new("cmd-v", PasteContours, None),
            gpui::KeyBinding::new("cmd-shift-o", RemoveOverlap, None),
            gpui::KeyBinding::new("cmd-shift-d", Decompose, None),
            gpui::KeyBinding::new("cmd-q", Quit, None),
            gpui::KeyBinding::new("cmd-shift-h", FlipHorizontal, None),
            gpui::KeyBinding::new("cmd-shift-v", FlipVertical, None),
            // Glyphs' shortcuts where they translate: Cmd-Shift-R
            // corrects direction, Cmd-Shift-T tidies (reverse and
            // duplicate-repeat move to Opt variants).
            gpui::KeyBinding::new("cmd-shift-r", CorrectPathDirection, None),
            gpui::KeyBinding::new("cmd-alt-shift-r", ReverseContours, None),
            gpui::KeyBinding::new("cmd-0", ZoomToFit, None),
            gpui::KeyBinding::new("cmd-d", DuplicateSelection, None),
            gpui::KeyBinding::new("cmd-shift-t", TidyPaths, None),
            gpui::KeyBinding::new("cmd-alt-shift-t", DuplicateRepeat, None),
            gpui::KeyBinding::new("cmd-ctrl-m", SyncMetrics, None),
            gpui::KeyBinding::new("cmd-a", SelectAllPoints, None),
            gpui::KeyBinding::new("cmd-alt-a", DeselectAllPoints, None),
            gpui::KeyBinding::new("cmd-alt-shift-i", InvertPointSelection, None),
            gpui::KeyBinding::new("cmd-alt-shift-n", NewGlyph, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());

        // One menu definition, three consumers: the macOS native bar
        // (set_menus), the stored menus on Windows/Linux, and the
        // in-window bar drawn where no native bar exists.
        #[cfg(not(target_family = "wasm"))]
        cx.set_menus(app_menus());
        #[cfg(not(target_os = "macos"))]
        let app_menu_bar = cx.new(|cx| widgets::menu_bar::MenuBar::new(app_menus(), cx));

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
                let workspace = cx.new(|cx| {
                    let search = cx.new(|cx| {
                        widgets::input::InputState::new(window, cx)
                            .placeholder("Search glyphs")
                    });
                    let metric = |cx: &mut Context<Workspace>, window: &mut Window| {
                        cx.new(|cx| widgets::input::InputState::new(window, cx))
                    };
                    let width_input = metric(cx, window);
                    let lsb_input = metric(cx, window);
                    let rsb_input = metric(cx, window);
                    let x_input = metric(cx, window);
                    let y_input = metric(cx, window);
                    let w_input = metric(cx, window);
                    let h_input = metric(cx, window);
                    let fi_family = metric(cx, window);
                    let fi_style = metric(cx, window);
                    let fi_upm = metric(cx, window);
                    let fi_angle = metric(cx, window);
                    let fi_asc = metric(cx, window);
                    let fi_desc = metric(cx, window);
                    let fi_xh = metric(cx, window);
                    let fi_ch = metric(cx, window);
                    let font_info_sub = |cx: &mut Context<Workspace>,
                                         window: &mut Window,
                                         state: &gpui::Entity<
                        widgets::input::InputState,
                    >,
                                         which: FontInfoField| {
                        let state = state.clone();
                        cx.subscribe_in(&state, window, {
                            let state = state.clone();
                            move |this: &mut Workspace,
                                  _,
                                  ev: &widgets::input::InputEvent,
                                  window,
                                  cx| {
                                if matches!(
                                    ev,
                                    widgets::input::InputEvent::PressEnter { .. }
                                ) {
                                    let text = state.read(cx).value().to_string();
                                    this.apply_font_info(which, &text);
                                    this.rebuild_text_models();
                                    this.refresh_font_info_inputs(true, window, cx);
                                    cx.notify();
                                }
                            }
                        })
                    };
                    let sub_fi_family =
                        font_info_sub(cx, window, &fi_family, FontInfoField::Family);
                    let sub_fi_style =
                        font_info_sub(cx, window, &fi_style, FontInfoField::Style);
                    let sub_fi_upm =
                        font_info_sub(cx, window, &fi_upm, FontInfoField::Upm);
                    let sub_fi_angle =
                        font_info_sub(cx, window, &fi_angle, FontInfoField::ItalicAngle);
                    let sub_fi_asc =
                        font_info_sub(cx, window, &fi_asc, FontInfoField::Ascender);
                    let sub_fi_desc =
                        font_info_sub(cx, window, &fi_desc, FontInfoField::Descender);
                    let sub_fi_xh =
                        font_info_sub(cx, window, &fi_xh, FontInfoField::XHeight);
                    let sub_fi_ch =
                        font_info_sub(cx, window, &fi_ch, FontInfoField::CapHeight);
                    let fi_blues = metric(cx, window);
                    let fi_oblues = metric(cx, window);
                    let fi_stems_h = metric(cx, window);
                    let fi_stems_v = metric(cx, window);
                    let sub_fi_bv = font_info_sub(
                        cx, window, &fi_blues, FontInfoField::BlueValues,
                    );
                    let sub_fi_ob = font_info_sub(
                        cx, window, &fi_oblues, FontInfoField::OtherBlues,
                    );
                    let sub_fi_sh = font_info_sub(
                        cx, window, &fi_stems_h, FontInfoField::StemsH,
                    );
                    let sub_fi_sv = font_info_sub(
                        cx, window, &fi_stems_v, FontInfoField::StemsV,
                    );
                    let fi_typo_asc = metric(cx, window);
                    let fi_typo_desc = metric(cx, window);
                    let fi_typo_gap = metric(cx, window);
                    let fi_hhea_asc = metric(cx, window);
                    let fi_hhea_desc = metric(cx, window);
                    let fi_hhea_gap = metric(cx, window);
                    let fi_win_asc = metric(cx, window);
                    let fi_win_desc = metric(cx, window);
                    let sub_fi_ta = font_info_sub(
                        cx, window, &fi_typo_asc, FontInfoField::TypoAscender,
                    );
                    let sub_fi_td = font_info_sub(
                        cx, window, &fi_typo_desc, FontInfoField::TypoDescender,
                    );
                    let sub_fi_tg = font_info_sub(
                        cx, window, &fi_typo_gap, FontInfoField::TypoLineGap,
                    );
                    let sub_fi_ha = font_info_sub(
                        cx, window, &fi_hhea_asc, FontInfoField::HheaAscender,
                    );
                    let sub_fi_hd = font_info_sub(
                        cx, window, &fi_hhea_desc, FontInfoField::HheaDescender,
                    );
                    let sub_fi_hg = font_info_sub(
                        cx, window, &fi_hhea_gap, FontInfoField::HheaLineGap,
                    );
                    let sub_fi_wa = font_info_sub(
                        cx, window, &fi_win_asc, FontInfoField::WinAscent,
                    );
                    let sub_fi_wd = font_info_sub(
                        cx, window, &fi_win_desc, FontInfoField::WinDescent,
                    );
                    let kern_filter = cx.new(|cx| {
                        widgets::input::InputState::new(window, cx)
                            .placeholder("Filter pairs")
                    });
                    let kern_first = cx.new(|cx| {
                        widgets::input::InputState::new(window, cx)
                            .placeholder("First")
                    });
                    let kern_second = cx.new(|cx| {
                        widgets::input::InputState::new(window, cx)
                            .placeholder("Second")
                    });
                    let kern_value = cx.new(|cx| {
                        widgets::input::InputState::new(window, cx)
                            .placeholder("Value")
                    });
                    // The filter redraws the list as it changes; the
                    // three editor fields commit together on Enter.
                    let sub_kern_filter = cx.subscribe_in(
                        &kern_filter,
                        window,
                        |_: &mut Workspace,
                         _,
                         ev: &widgets::input::InputEvent,
                         _,
                         cx| {
                            if matches!(
                                ev,
                                widgets::input::InputEvent::Change
                            ) {
                                cx.notify();
                            }
                        },
                    );
                    let kern_commit = |cx: &mut Context<Workspace>,
                                       window: &mut Window,
                                       state: &gpui::Entity<
                        widgets::input::InputState,
                    >| {
                        let state = state.clone();
                        cx.subscribe_in(&state, window, {
                            move |this: &mut Workspace,
                                  _,
                                  ev: &widgets::input::InputEvent,
                                  _,
                                  cx| {
                                if matches!(
                                    ev,
                                    widgets::input::InputEvent::PressEnter { .. }
                                ) {
                                    let first = this
                                        .kern_inputs
                                        .first
                                        .read(cx)
                                        .value()
                                        .trim()
                                        .to_string();
                                    let second = this
                                        .kern_inputs
                                        .second
                                        .read(cx)
                                        .value()
                                        .trim()
                                        .to_string();
                                    let value = this
                                        .kern_inputs
                                        .value
                                        .read(cx)
                                        .value()
                                        .trim()
                                        .parse::<f64>();
                                    if let (false, false, Ok(value)) =
                                        (first.is_empty(), second.is_empty(), value)
                                    {
                                        this.apply_kern_pair(&first, &second, value);
                                        cx.notify();
                                    }
                                }
                            }
                        })
                    };
                    let sub_kern_first = kern_commit(cx, window, &kern_first);
                    let sub_kern_second = kern_commit(cx, window, &kern_second);
                    let sub_kern_value = kern_commit(cx, window, &kern_value);
                    let slant_input = cx.new(|cx| {
                        widgets::input::InputState::new(window, cx)
                            .placeholder("Angle°")
                    });
                    let stroke_input = cx.new(|cx| {
                        widgets::input::InputState::new(window, cx)
                            .placeholder("Width")
                    });
                    let sub_stroke = cx.subscribe_in(&stroke_input, window, {
                        let state = stroke_input.clone();
                        move |this: &mut Workspace,
                              _,
                              ev: &widgets::input::InputEvent,
                              _,
                              cx| {
                            if matches!(
                                ev,
                                widgets::input::InputEvent::PressEnter { .. }
                            ) {
                                if let Ok(width) = state
                                    .read(cx)
                                    .value()
                                    .trim()
                                    .parse::<f64>()
                                {
                                    this.command_expand_stroke(width);
                                    cx.notify();
                                }
                            }
                        }
                    });
                    let offset_input = cx.new(|cx| {
                        widgets::input::InputState::new(window, cx)
                            .placeholder("±Units")
                    });
                    let sub_offset = cx.subscribe_in(&offset_input, window, {
                        let state = offset_input.clone();
                        move |this: &mut Workspace,
                              _,
                              ev: &widgets::input::InputEvent,
                              _,
                              cx| {
                            if matches!(
                                ev,
                                widgets::input::InputEvent::PressEnter { .. }
                            ) {
                                if let Ok(delta) = state
                                    .read(cx)
                                    .value()
                                    .trim()
                                    .parse::<f64>()
                                {
                                    this.command_offset(delta);
                                    cx.notify();
                                }
                            }
                        }
                    });
                    let fit_input = cx.new(|cx| {
                        widgets::input::InputState::new(window, cx)
                            .placeholder("%")
                    });
                    let sub_fit = cx.subscribe_in(&fit_input, window, {
                        let state = fit_input.clone();
                        move |this: &mut Workspace,
                              _,
                              ev: &widgets::input::InputEvent,
                              _,
                              cx| {
                            if matches!(
                                ev,
                                widgets::input::InputEvent::PressEnter { .. }
                            ) {
                                if let Ok(pct) = state
                                    .read(cx)
                                    .value()
                                    .trim()
                                    .trim_end_matches('%')
                                    .parse::<f64>()
                                {
                                    this.command_fit_curve(pct / 100.0);
                                    cx.notify();
                                }
                            }
                        }
                    });
                    let color_hex_input = cx.new(|cx| {
                        widgets::input::InputState::new(window, cx)
                            .placeholder("#RRGGBB")
                    });
                    let sub_color_hex = cx.subscribe_in(&color_hex_input, window, {
                        let state = color_hex_input.clone();
                        move |this: &mut Workspace,
                              _,
                              ev: &widgets::input::InputEvent,
                              window,
                              cx| {
                            if matches!(
                                ev,
                                widgets::input::InputEvent::PressEnter { .. }
                            ) {
                                let text = state.read(cx).value().to_string();
                                if this.command_add_palette_color(&text) {
                                    state.update(cx, |st, cx| {
                                        st.set_value(String::new(), window, cx);
                                    });
                                }
                                cx.notify();
                            }
                        }
                    });
                    let ease_input = cx.new(|cx| {
                        widgets::input::InputState::new(window, cx)
                            .placeholder("±50")
                    });
                    let sub_ease = cx.subscribe_in(&ease_input, window, {
                        let state = ease_input.clone();
                        move |this: &mut Workspace,
                              _,
                              ev: &widgets::input::InputEvent,
                              _,
                              cx| {
                            if matches!(
                                ev,
                                widgets::input::InputEvent::PressEnter { .. }
                            ) {
                                if let Ok(ease) = state
                                    .read(cx)
                                    .value()
                                    .trim()
                                    .parse::<f64>()
                                {
                                    this.command_ease_interpolation(ease);
                                    cx.notify();
                                }
                            }
                        }
                    });
                    let extrude_input = cx.new(|cx| {
                        widgets::input::InputState::new(window, cx)
                            .placeholder("15,30")
                    });
                    let sub_extrude = cx.subscribe_in(&extrude_input, window, {
                        let state = extrude_input.clone();
                        move |this: &mut Workspace,
                              _,
                              ev: &widgets::input::InputEvent,
                              _,
                              cx| {
                            if matches!(
                                ev,
                                widgets::input::InputEvent::PressEnter { .. }
                            ) {
                                let text = state.read(cx).value().to_string();
                                this.command_extrude(&text);
                                cx.notify();
                            }
                        }
                    });
                    let roughen_input = cx.new(|cx| {
                        widgets::input::InputState::new(window, cx)
                            .placeholder("15,15,10")
                    });
                    let sub_roughen = cx.subscribe_in(&roughen_input, window, {
                        let state = roughen_input.clone();
                        move |this: &mut Workspace,
                              _,
                              ev: &widgets::input::InputEvent,
                              _,
                              cx| {
                            if matches!(
                                ev,
                                widgets::input::InputEvent::PressEnter { .. }
                            ) {
                                let text = state.read(cx).value().to_string();
                                this.command_roughen(&text);
                                cx.notify();
                            }
                        }
                    });
                    let instance_name_input = cx.new(|cx| {
                        widgets::input::InputState::new(window, cx)
                            .placeholder("Instance name")
                    });
                    let sub_instance_name = cx.subscribe_in(
                        &instance_name_input,
                        window,
                        {
                            let state = instance_name_input.clone();
                            move |this: &mut Workspace,
                                  _,
                                  ev: &widgets::input::InputEvent,
                                  window,
                                  cx| {
                                if matches!(
                                    ev,
                                    widgets::input::InputEvent::PressEnter { .. }
                                ) {
                                    let name = state.read(cx).value().to_string();
                                    this.command_instance_upsert(&name);
                                    state.update(cx, |st, cx| {
                                        st.set_value(String::new(), window, cx);
                                    });
                                    cx.notify();
                                }
                            }
                        },
                    );
                    let features_input = cx.new(|cx| {
                        widgets::input::InputState::new(window, cx).multi_line()
                    });
                    let sub_features = cx.subscribe_in(
                        &features_input,
                        window,
                        |this: &mut Workspace,
                         _,
                         ev: &widgets::input::InputEvent,
                         _,
                         cx| {
                            if matches!(
                                ev,
                                widgets::input::InputEvent::Change
                            ) {
                                this.features_edited = true;
                                cx.notify();
                            }
                        },
                    );
                    let sub_slant = cx.subscribe_in(&slant_input, window, {
                        let state = slant_input.clone();
                        move |this: &mut Workspace,
                              _,
                              ev: &widgets::input::InputEvent,
                              _,
                              cx| {
                            if matches!(
                                ev,
                                widgets::input::InputEvent::PressEnter { .. }
                            ) {
                                let Ok(angle) = state
                                    .read(cx)
                                    .value()
                                    .trim()
                                    .parse::<f64>()
                                else {
                                    return;
                                };
                                if angle == 0.0 || angle.abs() >= 89.0 {
                                    return;
                                }
                                // Positive leans right, the italic
                                // convention (Glyphs' Slant filter).
                                this.apply_transform(Affine::skew(
                                    angle.to_radians().tan(),
                                    0.0,
                                ));
                                cx.notify();
                            }
                        }
                    });
                    let metric_sub = |cx: &mut Context<Workspace>,
                                      window: &mut Window,
                                      state: &gpui::Entity<widgets::input::InputState>,
                                      which: MetricField| {
                        let state = state.clone();
                        cx.subscribe_in(&state, window, {
                            let state = state.clone();
                            move |this: &mut Workspace,
                                  _,
                                  ev: &widgets::input::InputEvent,
                                  window,
                                  cx| {
                                if matches!(
                                    ev,
                                    widgets::input::InputEvent::PressEnter { .. }
                                ) {
                                    let text = state.read(cx).value().to_string();
                                    if let Ok(v) = text.trim().parse::<f64>() {
                                        this.apply_metric(which, v);
                                        this.rebuild_text_models();
                                    }
                                    this.refresh_metric_inputs(true, window, cx);
                                    cx.notify();
                                }
                            }
                        })
                    };
                    let sub_w = metric_sub(cx, window, &width_input, MetricField::Width);
                    let sub_l = metric_sub(cx, window, &lsb_input, MetricField::Lsb);
                    let sub_r = metric_sub(cx, window, &rsb_input, MetricField::Rsb);
                    let coord_sub = |cx: &mut Context<Workspace>,
                                     window: &mut Window,
                                     state: &gpui::Entity<widgets::input::InputState>,
                                     is_x: bool| {
                        let state = state.clone();
                        cx.subscribe_in(&state, window, {
                            let state = state.clone();
                            move |this: &mut Workspace,
                                  _,
                                  ev: &widgets::input::InputEvent,
                                  window,
                                  cx| {
                                if matches!(
                                    ev,
                                    widgets::input::InputEvent::PressEnter { .. }
                                ) {
                                    let text = state.read(cx).value().to_string();
                                    if let Ok(v) = text.trim().parse::<f64>() {
                                        this.apply_coord(is_x, v);
                                    }
                                    this.refresh_coord_inputs(true, window, cx);
                                    cx.notify();
                                }
                            }
                        })
                    };
                    let sub_x = coord_sub(cx, window, &x_input, true);
                    let sub_y = coord_sub(cx, window, &y_input, false);
                    let size_sub = |cx: &mut Context<Workspace>,
                                    window: &mut Window,
                                    state: &gpui::Entity<
                        widgets::input::InputState,
                    >,
                                    is_width: bool| {
                        let state = state.clone();
                        cx.subscribe_in(&state, window, {
                            let state = state.clone();
                            move |this: &mut Workspace,
                                  _,
                                  ev: &widgets::input::InputEvent,
                                  window,
                                  cx| {
                                if matches!(
                                    ev,
                                    widgets::input::InputEvent::PressEnter { .. }
                                ) {
                                    let text = state.read(cx).value().to_string();
                                    if let Ok(v) = text.trim().parse::<f64>() {
                                        this.apply_size(is_width, v);
                                    }
                                    this.refresh_coord_inputs(true, window, cx);
                                    cx.notify();
                                }
                            }
                        })
                    };
                    let sub_sw = size_sub(cx, window, &w_input, true);
                    let sub_sh = size_sub(cx, window, &h_input, false);
                    let name_input = metric(cx, window);
                    let unicode_input = metric(cx, window);
                    let group_l_input = metric(cx, window);
                    let group_r_input = metric(cx, window);
                    // 0=name, 1=unicode, 2=left group, 3=right group.
                    let glyph_sub = |cx: &mut Context<Workspace>,
                                     window: &mut Window,
                                     state: &gpui::Entity<
                        widgets::input::InputState,
                    >,
                                     which: u8| {
                        let state = state.clone();
                        cx.subscribe_in(&state, window, {
                            let state = state.clone();
                            move |this: &mut Workspace,
                                  _,
                                  ev: &widgets::input::InputEvent,
                                  window,
                                  cx| {
                                if matches!(
                                    ev,
                                    widgets::input::InputEvent::PressEnter { .. }
                                ) {
                                    let text =
                                        state.read(cx).value().to_string();
                                    match which {
                                        0 => this.apply_glyph_rename(&text),
                                        1 => this.apply_glyph_unicode(&text),
                                        2 => this.apply_kern_group(true, &text),
                                        4 => this.apply_glyph_note(&text),
                                        5 => {
                                            if let Ok(at) =
                                                text.trim().parse::<f64>()
                                            {
                                                this.command_add_shape_switch(at);
                                            }
                                        }
                                        6 => this.apply_metrics_key(true, &text),
                                        7 => this.apply_metrics_key(false, &text),
                                        8 => this.apply_glyph_production(&text),
                                        _ => this.apply_kern_group(false, &text),
                                    }
                                    this.refresh_glyph_inputs(true, window, cx);
                                    cx.notify();
                                }
                            }
                        })
                    };
                    let component_name_input = cx.new(|cx| {
                        widgets::input::InputState::new(window, cx)
                            .placeholder("glyph name")
                    });
                    let reference_glyph_input = cx.new(|cx| {
                        widgets::input::InputState::new(window, cx)
                            .placeholder("glyph name")
                    });
                    let sub_ref = cx.subscribe_in(&reference_glyph_input, window, {
                        let state = reference_glyph_input.clone();
                        move |this: &mut Workspace,
                              _,
                              ev: &widgets::input::InputEvent,
                              _window,
                              cx| {
                            if matches!(
                                ev,
                                widgets::input::InputEvent::PressEnter { .. }
                            ) {
                                let text =
                                    state.read(cx).value().trim().to_string();
                                this.reference_glyph =
                                    (!text.is_empty()).then_some(text);
                                cx.notify();
                            }
                        }
                    });
                    let anchor_name_input = cx.new(|cx| {
                        widgets::input::InputState::new(window, cx)
                            .placeholder("anchor name")
                    });
                    let sub_anchor = cx.subscribe_in(&anchor_name_input, window, {
                        let state = anchor_name_input.clone();
                        move |this: &mut Workspace,
                              _,
                              ev: &widgets::input::InputEvent,
                              _window,
                              cx| {
                            if matches!(
                                ev,
                                widgets::input::InputEvent::PressEnter { .. }
                            ) {
                                let text = state.read(cx).value().to_string();
                                this.apply_anchor_name(&text);
                                cx.notify();
                            }
                        }
                    });
                    let corner_name_input = cx.new(|cx| {
                        widgets::input::InputState::new(window, cx)
                            .placeholder("corner name")
                    });
                    let sub_corner = cx.subscribe_in(&corner_name_input, window, {
                        let state = corner_name_input.clone();
                        move |this: &mut Workspace,
                              _,
                              ev: &widgets::input::InputEvent,
                              window,
                              cx| {
                            if matches!(
                                ev,
                                widgets::input::InputEvent::PressEnter { .. }
                            ) {
                                let text = state.read(cx).value().to_string();
                                let node = this
                                    .context_menu
                                    .as_ref()
                                    .and_then(|m| m.start_point);
                                this.context_menu = None;
                                if let Some(node) = node {
                                    this.command_apply_corner(node, text.trim());
                                }
                                state.update(cx, |st, cx| {
                                    st.set_value(String::new(), window, cx);
                                });
                                cx.notify();
                            }
                        }
                    });
                    let smart_axis_input = cx.new(|cx| {
                        widgets::input::InputState::new(window, cx)
                            .placeholder("Width,0,100")
                    });
                    let sub_smart_axis = cx.subscribe_in(&smart_axis_input, window, {
                        let state = smart_axis_input.clone();
                        move |this: &mut Workspace,
                              _,
                              ev: &widgets::input::InputEvent,
                              _,
                              cx| {
                            if matches!(
                                ev,
                                widgets::input::InputEvent::PressEnter { .. }
                            ) {
                                let text = state.read(cx).value().to_string();
                                this.command_make_smart_axis(&text);
                                cx.notify();
                            }
                        }
                    });
                    let group_name_input = cx.new(|cx| {
                        widgets::input::InputState::new(window, cx)
                            .placeholder("new group · o or |o")
                    });
                    let sub_group_name = cx.subscribe_in(&group_name_input, window, {
                        let state = group_name_input.clone();
                        move |this: &mut Workspace,
                              _,
                              ev: &widgets::input::InputEvent,
                              window,
                              cx| {
                            if matches!(
                                ev,
                                widgets::input::InputEvent::PressEnter { .. }
                            ) {
                                let text = state.read(cx).value().to_string();
                                let trimmed = text.trim();
                                let (first_side, name) =
                                    match trimmed.strip_prefix('|') {
                                        Some(rest) => (false, rest.trim()),
                                        None => (true, trimmed),
                                    };
                                if !name.is_empty() {
                                    this.command_add_selection_to_group(
                                        first_side, name,
                                    );
                                    state.update(cx, |st, cx| {
                                        st.set_value(String::new(), window, cx);
                                    });
                                }
                                cx.notify();
                            }
                        }
                    });
                    let axis_map_input = cx.new(|cx| {
                        widgets::input::InputState::new(window, cx)
                            .placeholder("400,430")
                    });
                    let sub_axis_map = cx.subscribe_in(&axis_map_input, window, {
                        let state = axis_map_input.clone();
                        move |this: &mut Workspace,
                              _,
                              ev: &widgets::input::InputEvent,
                              window,
                              cx| {
                            if matches!(
                                ev,
                                widgets::input::InputEvent::PressEnter { .. }
                            ) {
                                let text = state.read(cx).value().to_string();
                                let mut parts =
                                    text.split(',').map(str::trim);
                                if let (Some(Ok(input)), Some(Ok(output))) = (
                                    parts.next().map(str::parse::<f32>),
                                    parts.next().map(str::parse::<f32>),
                                ) {
                                    this.command_add_axis_mapping(
                                        input, output,
                                    );
                                    state.update(cx, |st, cx| {
                                        st.set_value(String::new(), window, cx);
                                    });
                                }
                                cx.notify();
                            }
                        }
                    });
                    let smart_value_input = cx.new(|cx| {
                        widgets::input::InputState::new(window, cx)
                            .placeholder("value")
                    });
                    let sub_smart_value = cx.subscribe_in(&smart_value_input, window, {
                        let state = smart_value_input.clone();
                        move |this: &mut Workspace,
                              _,
                              ev: &widgets::input::InputEvent,
                              _,
                              cx| {
                            if matches!(
                                ev,
                                widgets::input::InputEvent::PressEnter { .. }
                            ) {
                                let text =
                                    state.read(cx).value().trim().to_string();
                                if !text.is_empty() {
                                    this.command_set_smart_value(&text);
                                    cx.notify();
                                }
                            }
                        }
                    });
                    let annotation_input = cx.new(|cx| {
                        widgets::input::InputState::new(window, cx)
                            .placeholder("note text")
                    });
                    let sub_note = cx.subscribe_in(&annotation_input, window, {
                        let state = annotation_input.clone();
                        move |this: &mut Workspace,
                              _,
                              ev: &widgets::input::InputEvent,
                              window,
                              cx| {
                            if matches!(
                                ev,
                                widgets::input::InputEvent::PressEnter { .. }
                            ) {
                                let text = state.read(cx).value().to_string();
                                let at = this
                                    .context_menu
                                    .as_ref()
                                    .map(|m| m.design);
                                this.context_menu = None;
                                if let (Some(at), false) =
                                    (at, text.trim().is_empty())
                                {
                                    this.command_add_annotation(
                                        at,
                                        "note",
                                        text.trim(),
                                    );
                                }
                                state.update(cx, |st, cx| {
                                    st.set_value(String::new(), window, cx);
                                });
                                cx.notify();
                            }
                        }
                    });
                    let sub_comp = cx.subscribe_in(&component_name_input, window, {
                        let state = component_name_input.clone();
                        move |this: &mut Workspace,
                              _,
                              ev: &widgets::input::InputEvent,
                              window,
                              cx| {
                            if matches!(
                                ev,
                                widgets::input::InputEvent::PressEnter { .. }
                            ) {
                                let text = state.read(cx).value().to_string();
                                this.commit_add_component(&text);
                                state.update(cx, |st, cx| {
                                    st.set_value(String::new(), window, cx);
                                });
                                cx.notify();
                            }
                        }
                    });
                    let note_input = metric(cx, window);
                    let switch_input = metric(cx, window);
                    let lsb_key_input = metric(cx, window);
                    let rsb_key_input = metric(cx, window);
                    let sub_gn = glyph_sub(cx, window, &name_input, 0);
                    let sub_gu = glyph_sub(cx, window, &unicode_input, 1);
                    let sub_gl = glyph_sub(cx, window, &group_l_input, 2);
                    let sub_gr = glyph_sub(cx, window, &group_r_input, 3);
                    let sub_gnote = glyph_sub(cx, window, &note_input, 4);
                    let sub_gswitch = glyph_sub(cx, window, &switch_input, 5);
                    let sub_glk = glyph_sub(cx, window, &lsb_key_input, 6);
                    let sub_grk = glyph_sub(cx, window, &rsb_key_input, 7);
                    let production_input = metric(cx, window);
                    let sub_gprod =
                        glyph_sub(cx, window, &production_input, 8);
                    let subscription = cx.subscribe_in(&search, window, {
                        let search = search.clone();
                        move |this: &mut Workspace,
                              _,
                              ev: &widgets::input::InputEvent,
                              _window,
                              cx| {
                            if matches!(ev, widgets::input::InputEvent::Change) {
                                this.search_query =
                                    search.read(cx).value().to_string().to_lowercase();
                                this.rebuild_search_regex();
                                // Fewer matches: start both grids at
                                // the top rather than past the end.
                                this.grid_scroll_row = 0;
                                this.sidebar_scroll_row = 0;
                                cx.notify();
                            }
                        }
                    });
                    let mut workspace = Workspace {
                        project,
                        load_error,
                        selected: None,
                        last_editor: None,
                        sessions: Vec::new(),
                        active_session: 0,
                        sidebar_filter: SidebarFilter::All,
                        sidebar_matches: None,
                        sidebar_counts: None,
                        expanded_scripts: std::collections::HashSet::new(),
                        expanded_categories: std::collections::HashSet::new(),
                        sort_unicode: true,
                        nudging: false,
                        preview_visible: true,
                        preview_blur: 0.0,
                        preview_blur_cache: Arc::new(Mutex::new(None)),
                        preview_invert: false,
                        preview_blur_slider: None,
                        grid_cell_size: CELL,
                        grid_viewport: gpui::size(px(0.0), px(0.0)),
                        sidebar_viewport: gpui::size(px(0.0), px(0.0)),
                        glyph_order: None,
                        order_key: None,
                        search_re: None,
                        grid_scroll_row: 0,
                        sidebar_scroll_row: 0,
                        sidebar_tab: 0,
                        sidebar_cell_size: MINI_CELL,
                        sidebar_slider: None,
                        cell_slider: None,
                        mode: start_mode,
                        editor: EditorState::new(),
                        edit_buffer: runebender_core::text::TextBuffer::new(),
                        collapsed_sections: std::collections::HashSet::new(),
                        reference_layers: std::collections::HashSet::new(),
                        show_all_masters: false,
                        left_collapsed: false,
                        #[cfg(not(target_os = "macos"))]
                        app_menu_bar: app_menu_bar.clone(),
                        focus_handle: cx.focus_handle(),
                        model_strength: 1.0,
                        model_dir: None,
                        model_summary: None,
                        model_loaded: None,
                        model_score: None,
                        model_strength_slider: None,
                        status_note: None,
                        search,
                        search_query: String::new(),
                        search_mode: 0,
                        last_save_label: None,
                        multi_selected: std::collections::HashSet::new(),
                        search_regex: false,
                        search_case: false,
                        context_menu: None,
                        coord_quadrant: Default::default(),
                        curve_comb: false,
                        curve_continuity: false,
                        measure_opts: MeasureOpts::default(),
                        show_background: true,
                        visible_glyph_layers: Default::default(),
                        reference_glyph: None,
                        reference_glyph_input: reference_glyph_input.clone(),
                        component_name_input: component_name_input.clone(),
                        corner_name_input: corner_name_input.clone(),
                        annotation_input: annotation_input.clone(),
                        smart_axis_input,
                        smart_value_input,
                        group_name_input,
                        axis_map_input,
                        anchor_name_input: anchor_name_input.clone(),
                        glyph_image_cache: Default::default(),
                        glyph_inputs: GlyphInputs {
                            name: name_input,
                            unicode: unicode_input,
                            group_l: group_l_input,
                            group_r: group_r_input,
                            note: note_input,
                            switch_at: switch_input,
                            lsb_key: lsb_key_input,
                            rsb_key: rsb_key_input,
                            production: production_input,
                        },
                        metric_inputs: MetricInputs {
                            width: width_input,
                            lsb: lsb_input,
                            rsb: rsb_input,
                            x: x_input,
                            y: y_input,
                            w: w_input,
                            h: h_input,
                        },
                        font_info_inputs: FontInfoInputs {
                            family: fi_family,
                            style: fi_style,
                            upm: fi_upm,
                            italic_angle: fi_angle,
                            ascender: fi_asc,
                            descender: fi_desc,
                            x_height: fi_xh,
                            cap_height: fi_ch,
                            typo_asc: fi_typo_asc,
                            typo_desc: fi_typo_desc,
                            typo_gap: fi_typo_gap,
                            hhea_asc: fi_hhea_asc,
                            hhea_desc: fi_hhea_desc,
                            hhea_gap: fi_hhea_gap,
                            win_asc: fi_win_asc,
                            win_desc: fi_win_desc,
                            blue_values: fi_blues,
                            other_blues: fi_oblues,
                            stems_h: fi_stems_h,
                            stems_v: fi_stems_v,
                        },
                        kern_inputs: KernInputs {
                            filter: kern_filter,
                            first: kern_first,
                            second: kern_second,
                            value: kern_value,
                        },
                        slant_input,
                        stroke_input,
                        offset_input,
                        fit_input,
                        color_hex_input,
                        color_selected: 0,
                        show_color_preview: true,
                        sample_index: 0,
                        font_view_mode: FontViewMode::Grid,
                        search_predicates: None,
                        show_trajectories: false,
                        hoi_live: None,
                        shaping_focus: None,
                        show_mark_cloud: false,
                        feature_overrides: Default::default(),
                        shaping_locale: None,
                        ease_input,
                        extrude_input,
                        roughen_input,
                        roughen_seed: 0,
                        instance_name_input,
                        features_input,
                        features_edited: false,
                        features_status: None,
                        axis_sliders: Vec::new(),
                        clipboard: Vec::new(),
                        #[cfg(target_family = "wasm")]
                        web_host: None,
                        _watcher: None,
                        last_save: Arc::new(Mutex::new(web_time::Instant::now())),
                        _subscriptions: vec![
                            subscription, sub_w, sub_l, sub_r, sub_x, sub_y,
                            sub_gn, sub_gu, sub_gl, sub_gr, sub_gnote,
                            sub_gswitch, sub_glk, sub_grk, sub_gprod,
                            sub_comp,
                            sub_corner, sub_note, sub_smart_axis,
                            sub_smart_value, sub_group_name, sub_axis_map,
                            sub_sw, sub_sh, sub_anchor, sub_ref,
                            sub_fi_family, sub_fi_style, sub_fi_upm,
                            sub_fi_angle, sub_fi_asc, sub_fi_desc,
                            sub_fi_xh, sub_fi_ch, sub_fi_ta, sub_fi_td,
                            sub_fi_tg, sub_fi_ha, sub_fi_hd, sub_fi_hg,
                            sub_fi_wa, sub_fi_wd, sub_fi_bv, sub_fi_ob,
                            sub_fi_sh, sub_fi_sv,
                            sub_kern_filter, sub_kern_first,
                            sub_kern_second, sub_kern_value, sub_slant,
                            sub_features, sub_instance_name, sub_stroke,
                            sub_offset, sub_fit, sub_color_hex, sub_ease,
                            sub_extrude, sub_roughen,
                        ],
                    };
                    workspace.rebuild_text_models();
                    workspace.start_watching(cx);
                    #[cfg(target_family = "wasm")]
                    if let Some(base) = web_host::server_from_location() {
                        workspace.connect_web_host(base, cx);
                    } else {
                        workspace.status_note = Some(
                            "Embedded demo font (read-only) · open with ?server=http://… to edit real fonts"
                                .into(),
                        );
                    }
                    workspace
                });
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
                let shortcut_target = workspace.clone();
                cx.intercept_keystrokes(move |event, window, cx| {
                    let ks = &event.keystroke;
                    let cmd = ks.modifiers.platform;
                    let shift = ks.modifiers.shift;
                    if ks.modifiers.control || ks.modifiers.alt {
                        return;
                    }
                    // A focused text field owns its keystrokes,
                    // including Tab and the clipboard.
                    if widgets::input::any_field_focused(window, cx) {
                        return;
                    }
                    if ks.key == "tab" && !cmd {
                        cx.stop_propagation();
                        shortcut_target.update(cx, |this, cx| {
                            if this.command_cycle_point(shift) {
                                cx.notify();
                            }
                        });
                        return;
                    }
                    if !cfg!(target_family = "wasm") || !cmd {
                        return;
                    }
                    let handled = shortcut_target.update(cx, |this, cx| {
                        match (ks.key.as_str(), shift) {
                            ("s", false) => this.command_save(cx),
                            ("n", false) => this.command_new_font(),
                            ("z", false) => {
                                this.undo();
                                this.rebuild_text_models();
                            }
                            ("z", true) => {
                                this.redo();
                                this.rebuild_text_models();
                            }
                            ("c", false) => this.command_copy(),
                            ("v", false) => this.command_paste_routed(cx),
                            ("o", true) => this.command_remove_overlap(),
                            ("d", true) => this.command_decompose(),
                            ("d", false) => this.command_duplicate(),
                            ("t", true) => this.command_duplicate_repeat(),
                            ("h", true) => {
                                this.apply_transform(Affine::scale_non_uniform(
                                    -1.0, 1.0,
                                ));
                            }
                            ("v", true) => {
                                this.apply_transform(Affine::scale_non_uniform(
                                    1.0, -1.0,
                                ));
                            }
                            ("r", true) => this.command_reverse(),
                            ("0", false) => {
                                if matches!(this.mode, Mode::Editor(_)) {
                                    this.editor.initialized = false;
                                    this.ensure_editor_fit();
                                }
                            }
                            _ => return false,
                        }
                        cx.notify();
                        true
                    });
                    if handled {
                        cx.stop_propagation();
                    }
                })
                .detach();
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


/// Move a glyph's points by the model's offsets, in the order the
/// outline reader produced them.
///
/// Walks the same contours in the same rotation `font_ml::ufo` uses,
/// so offset *n* lands on the point it was predicted for. Point types
/// and smooth flags are left alone: this moves points and nothing
/// else.
fn bolden_contours(
    glyph: &norad::Glyph,
    deltas: &[(i32, i32)],
    center: (i32, i32),
) -> Vec<norad::Contour> {
    let mut next = deltas.iter();
    let mut out = Vec::with_capacity(glyph.contours.len());
    for contour in &glyph.contours {
        let points = &contour.points;
        let start = points
            .iter()
            .position(|p| p.typ != norad::PointType::OffCurve)
            .unwrap_or(0);
        let n = points.len();
        let mut moved = points.clone();
        // Visit in pen order, starting where the reader started.
        for step in 0..n {
            let i = (start + step) % n;
            let Some((dx, dy)) = next.next().copied() else {
                break;
            };
            moved[i].x += (dx + center.0) as f64;
            moved[i].y += (dy + center.1) as f64;
        }
        // The reader ends a closed contour by returning to its start,
        // so it yields one offset more than the contour has points.
        // Drop it, or every later contour is shifted by one point.
        next.next();
        out.push(norad::Contour::new(moved, contour.identifier().cloned()));
    }
    out
}
