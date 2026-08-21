// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! Runebender GPUI: a font editor built on [GPUI](https://gpui.rs/),
//! started as a point of comparison against
//! [runebender-xilem](https://github.com/eliheuer/runebender-xilem).

mod blur;
mod glyph_path;
mod theme;
#[cfg(target_family = "wasm")]
mod web_host;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gpui::{
    canvas, div, prelude::*, px, size, App, Bounds, Context, MouseButton,
    PathBuilder, Point, SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions,
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
        Undo,
        Redo,
        CopyContours,
        PasteContours,
        CopySelectedGlyphs,
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
        HyperToCubic,
        TraceImage,
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

/// The application menu, used three ways: the native macOS menu bar,
/// the stored menu Windows/Linux expose to `get_menus`, and the
/// in-window menu bar (gpui-component AppMenuBar) drawn on every
/// platform that has no native bar, the browser included.
/// One item per theme, with the active one checked. The menus are
/// rebuilt on a switch so the tick follows.
fn theme_menu_items() -> Vec<gpui::MenuItem> {
    use gpui::MenuItem;
    let current = t::current_theme();
    t::THEMES
        .iter()
        .map(|(id, label)| {
            let action: Box<dyn gpui::Action> = match *id {
                "midnight" => Box::new(SetThemeMidnight),
                "gray" => Box::new(SetThemeGray),
                "light" => Box::new(SetThemeLight),
                _ => Box::new(SetThemeDark),
            };
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
            ],
            disabled: false,
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::action("Undo", Undo),
                MenuItem::action("Redo", Redo),
                MenuItem::separator(),
                MenuItem::action("Copy Contours", CopyContours),
                MenuItem::action("Paste Contours", PasteContours),
                MenuItem::separator(),
                MenuItem::action(
                    "Copy Selected Glyphs as Text",
                    CopySelectedGlyphs,
                ),
            ],
            disabled: false,
        },
        Menu {
            name: "Glyph".into(),
            items: vec![
                MenuItem::action("Remove Overlap", RemoveOverlap),
                MenuItem::action("Decompose Components", Decompose),
                MenuItem::separator(),
                MenuItem::action("Flip Horizontal", FlipHorizontal),
                MenuItem::action("Flip Vertical", FlipVertical),
                MenuItem::action("Rotate 90° Left", RotateLeft),
                MenuItem::action("Rotate 90° Right", RotateRight),
                MenuItem::action("Rotate 180°", Rotate180),
                MenuItem::action("Duplicate Selection", DuplicateSelection),
                MenuItem::action("Duplicate + Repeat", DuplicateRepeat),
                MenuItem::action("Round Corners", RoundCorners),
                MenuItem::action("Hyperbezier to Cubic", HyperToCubic),
                MenuItem::action("Reverse Contours", ReverseContours),
                MenuItem::action("Set Start Point", SetStartPoint),
                MenuItem::separator(),
                MenuItem::action("Union", BooleanUnion),
                MenuItem::action("Subtract", BooleanSubtract),
                MenuItem::action("Intersect", BooleanIntersect),
                MenuItem::action("Exclude", BooleanExclude),
                MenuItem::separator(),
                MenuItem::action("Trace Image…", TraceImage),
                MenuItem::separator(),
                MenuItem::action("Harmonize", Harmonize),
                MenuItem::action("Balance", Balance),
                MenuItem::action("Optimize", Optimize),
            ],
            disabled: false,
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Zoom to Fit", ZoomToFit),
                MenuItem::separator(),
                MenuItem::action("Sort Glyphs by Name", SortByName),
                MenuItem::action("Sort Glyphs by Unicode", SortByUnicode),
                MenuItem::separator(),
                MenuItem::action("Next Master", NextMaster),
                MenuItem::action("Previous Master", PreviousMaster),
                MenuItem::separator(),
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
                if let Some(slot) =
                    self.font.default_layer_mut().get_glyph_mut(name.as_str())
                {
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
        if self
            .font
            .default_layer_mut()
            .remove_glyph(name)
            .is_none()
        {
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
            .map(|glyph| GlyphEntry {
                name: glyph.name().to_string().into(),
                codepoint: glyph.codepoints.iter().next(),
                path: Arc::new(glyph_path::glyph_to_bezpath(glyph, &font)),
                contour_path: Arc::new(glyph_path::contours_to_bezpath(glyph)),
                component_path: Arc::new(glyph_path::components_to_bezpath(glyph, &font)),
                points: Arc::new(extract_points(glyph)),
                anchors: Arc::new(extract_anchors(glyph)),
                advance: glyph.width,
                component_names: Arc::new(
                    glyph.components.iter().map(|c| c.base.to_string().into()).collect(),
                ),
                mark: t::mark_label(glyph).map(SharedString::from),
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
            glyph.components.iter().map(|c| c.base.to_string().into()).collect(),
        );
        let points = Arc::new(extract_points(glyph));
        let anchors = Arc::new(extract_anchors(glyph));
        let entry = &mut self.glyphs[glyph_index];
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
        let Some(unioned) = self.font.get_glyph(name.as_str()).and_then(ops::remove_overlap)
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
    /// (Kept for future per-master UI; the model owns a copy.)
    #[allow(dead_code)]
    master_locations: Vec<runebender_core::var_model::Location>,
    model: Option<runebender_core::var_model::VariationModel>,
    /// Current preview location, normalized, by axis name.
    location: runebender_core::var_model::Location,
    /// Per-glyph master point-compatibility (designspaces only).
    compat: std::collections::HashMap<String, bool>,
}

impl Project {
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
        };
        project.compute_compat();
        project
    }

    fn load(path: &std::path::Path) -> Result<Self, String> {
        let mut project = Self::load_inner(path)?;
        project.compute_compat();
        Ok(project)
    }

    fn load_inner(path: &std::path::Path) -> Result<Self, String> {
        if path.extension().is_some_and(|e| e == "glyphs") {
            // Convert the .glyphs source to UFO + designspace files in
            // a sibling directory, then open the converted project.
            let text = std::fs::read_to_string(path).map_err(|e| format!("{e}"))?;
            let result = runebender_core::glyphs_import::glyphs_to_ufo_files(&text)?;
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
            return Self::load_inner(&open);
        }
        if path.extension().is_some_and(|e| e == "designspace") {
            let doc = norad::designspace::DesignSpaceDocument::load(path)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            let dir = path.parent().unwrap_or(std::path::Path::new(".")).to_path_buf();
            return Self::from_designspace(doc, move |filename| {
                let ufo_path = dir.join(filename);
                FontModel::load(&ufo_path).map_err(|e| format!("{}: {e}", ufo_path.display()))
            });
        }
        {
            let model =
                FontModel::load(path).map_err(|e| format!("{}: {e}", path.display()))?;
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
            for source in &doc.sources {
                if !seen.insert(source.filename.clone()) {
                    continue; // per-layer duplicate source entries
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
                            raw, axis.min, axis.default, axis.max,
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
            }
            if masters.is_empty() {
                return Err("designspace has no sources".into());
            }
            let model = (masters.len() > 1)
                .then(|| runebender_core::var_model::VariationModel::new(&master_locations));
            let location = axes
                .iter()
                .map(|a| (a.name.clone(), 0.0))
                .collect();
            Ok(Self {
                active: default_index,
                masters,
                master_names,
                axes,
                master_locations,
                model,
                location,
                compat: std::collections::HashMap::new(),
            })
        }
    }

    /// Assemble a project from a fetched workspace (web host).
    /// Returns the project plus per-master UFO path prefixes
    /// (workspace-root relative), aligned with `masters`.
    #[cfg(target_family = "wasm")]
    fn from_fetched(
        fetched: &web_host::FetchedWorkspace,
    ) -> Result<(Self, Vec<String>), String> {
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
            let ufo = runebender_core::font_memory::ufo_from_files(
                files.iter().map(|(p, b)| (*p, *b)),
            )?;
            let mut model = FontModel::from_font(
                ufo.font,
                PathBuf::from(prefix.trim_end_matches('/')),
            );
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
            Self::from_designspace(doc, |filename| {
                build_master(format!("{ds_dir}{filename}/"))
            })?
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
        static DEMO: include_dir::Dir<'_> = include_dir::include_dir!(
            "$CARGO_MANIFEST_DIR/../runebender-web/assets/test-fonts"
        );
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
    fn interpolated_glyph(&self, glyph_name: &str) -> Option<(BezPath, f64)> {
        let model = self.model.as_ref()?;
        if self.location.values().all(|v| v.abs() < 1e-9) {
            return None;
        }
        // Flatten [advance, x0, y0, x1, y1, ...] per master.
        let mut values: Vec<Vec<f64>> = Vec::with_capacity(self.masters.len());
        for master in &self.masters {
            let glyph = master.font.get_glyph(glyph_name)?;
            let mut v = vec![glyph.width];
            for contour in &glyph.contours {
                for p in &contour.points {
                    v.push(p.x);
                    v.push(p.y);
                }
            }
            values.push(v);
        }
        let len = values[0].len();
        if values.iter().any(|v| v.len() != len) {
            return None; // point-incompatible masters
        }
        let out = model.interpolate(&values, &self.location);
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
        Some((glyph_path::glyph_to_bezpath(&glyph, &base.font), advance))
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
            let Some(icon) = runebender_core::theme_oklch::toolbar_icons().get(name)
            else {
                return;
            };
            let w: f32 = bounds.size.width.into();
            let h: f32 = bounds.size.height.into();
            // Proportional inset: a fixed one shrank the mark inside
            // bigger tiles and crowded it in small ones.
            let pad = (w.min(h) as f64) * 0.12;
            let vb = icon.view_box;
            let scale = ((w as f64 - pad * 2.0) / vb.width())
                .min((h as f64 - pad * 2.0) / vb.height());
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
    preview_invert: bool,
    preview_blur_slider: Option<gpui::Entity<gpui_component::slider::SliderState>>,
    /// Grid cell size in px, driven by the bottom bar's zoom slider.
    /// This is the *target*: cells stretch from it to fill the row.
    grid_cell_size: f32,
    /// Measured size of the glyph grid's scroll viewport. Columns and
    /// row height are solved against it so rows fill the width and
    /// divide the height evenly (no half row at the bottom edge).
    grid_viewport: gpui::Size<gpui::Pixels>,
    /// The same, for the editor sidebar's mini glyph grid.
    sidebar_viewport: gpui::Size<gpui::Pixels>,
    /// First visible row of each grid. Scrolling moves whole rows.
    grid_scroll_row: usize,
    sidebar_scroll_row: usize,
    /// Which editor-sidebar tab is up: 0 glyphs, 1 shapes, 2 axes,
    /// 3 chat.
    sidebar_tab: u8,
    /// Target cell size for the editor sidebar's mini grid.
    sidebar_cell_size: f32,
    sidebar_slider: Option<gpui::Entity<gpui_component::slider::SliderState>>,
    cell_slider: Option<gpui::Entity<gpui_component::slider::SliderState>>,
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
    /// Left sidebar hidden (header toggle, like the Glyphs one).
    left_collapsed: bool,
    /// In-window menu bar for platforms without a native one
    /// (Windows, Linux, the browser).
    #[cfg(not(target_os = "macos"))]
    app_menu_bar: gpui::Entity<gpui_component::menu::AppMenuBar>,
    focus_handle: gpui::FocusHandle,
    status_note: Option<SharedString>,
    search: gpui::Entity<gpui_component::input::InputState>,
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
    /// Another glyph ghosted behind the drawing for comparison.
    reference_glyph: Option<String>,
    reference_glyph_input: gpui::Entity<gpui_component::input::InputState>,
    component_name_input: gpui::Entity<gpui_component::input::InputState>,
    anchor_name_input: gpui::Entity<gpui_component::input::InputState>,
    /// Sliders for non-degenerate designspace axes: (axis index,
    /// slider), created lazily in render.
    axis_sliders: Vec<(usize, gpui::Entity<gpui_component::slider::SliderState>)>,
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
    name: gpui::Entity<gpui_component::input::InputState>,
    unicode: gpui::Entity<gpui_component::input::InputState>,
    group_l: gpui::Entity<gpui_component::input::InputState>,
    group_r: gpui::Entity<gpui_component::input::InputState>,
}

struct MetricInputs {
    width: gpui::Entity<gpui_component::input::InputState>,
    lsb: gpui::Entity<gpui_component::input::InputState>,
    rsb: gpui::Entity<gpui_component::input::InputState>,
    /// Selection reference coordinates and size (Selection section).
    x: gpui::Entity<gpui_component::input::InputState>,
    y: gpui::Entity<gpui_component::input::InputState>,
    w: gpui::Entity<gpui_component::input::InputState>,
    h: gpui::Entity<gpui_component::input::InputState>,
}

/// A flat slider: a thin, evenly colored track (the library's own
/// styling tints the unfilled side with the bar color, which reads as
/// a dark stripe on one side) and a ring thumb that fills solid while
/// it is grabbed, instead of growing a translucent halo.
fn flat_slider(
    state: &gpui::Entity<gpui_component::slider::SliderState>,
    cx: &gpui::App,
) -> impl IntoElement + use<> {
    use gpui::{InteractiveElement as _, StatefulInteractiveElement as _};
    use gpui_base::{Slider as BaseSlider, SliderIndicator, SliderThumb, SliderTrack};

    const TRACK: f32 = 3.0;
    const THUMB: f32 = 12.0;
    let pct = state.read(cx).percentage().end;
    let thumb = SliderThumb::new(state)
        .axis(gpui::Axis::Horizontal)
        .absolute()
        .top(px((TRACK - THUMB) / 2.0))
        .left(gpui::relative(pct))
        .ml(px(-THUMB / 2.0))
        .w(px(THUMB))
        .h(px(THUMB))
        .flex_shrink_0()
        .rounded_full()
        .border_2()
        .border_color(t::accent())
        .bg(t::panel_bg())
        .hover(|el| el.bg(t::accent()))
        .active(|el| el.bg(t::accent()));
    BaseSlider::new(state)
        .axis(gpui::Axis::Horizontal)
        .flex()
        .items_center()
        .w_full()
        .child(
            SliderTrack::new(state)
                .axis(gpui::Axis::Horizontal)
                .flex()
                .items_center()
                .h(px(THUMB))
                .w_full()
                .flex_shrink_0()
                .child(
                    SliderIndicator::new(state)
                        .relative()
                        .w_full()
                        .h(px(TRACK))
                        .rounded_full()
                        // One colour end to end: this reports a value,
                        // it is not a progress bar.
                        .bg(t::accent())
                        .child(thumb),
                ),
        )
}

/// Everything the blurred preview image depends on, hashed: the line
/// itself, the pane size, the radius and the two colours.
fn blur_key(
    line: &BezPath,
    w: f64,
    h: f64,
    blur: f32,
    ink: gpui::Rgba,
    ground: gpui::Rgba,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for element in line.elements() {
        match element {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => {
                (p.x.to_bits(), p.y.to_bits()).hash(&mut hasher)
            }
            PathEl::QuadTo(a, b) => {
                (a.x.to_bits(), a.y.to_bits(), b.x.to_bits(), b.y.to_bits())
                    .hash(&mut hasher)
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
            let pt = |x: f64, y: f64| {
                gpui::point(o.x + px(x as f32), o.y + px(y as f32))
            };
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
            if let Some(p) = build_path(
                &ring,
                Affine::IDENTITY,
                o,
                PathBuilder::stroke(px(1.2)),
            ) {
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
            self.editor =
                std::mem::replace(&mut slot.editor, EditorState::new());
            self.edit_buffer = std::mem::replace(
                &mut slot.buffer,
                runebender_core::text::TextBuffer::new(),
            );
            self.active_session = target;
        }
        let name = self.sessions[target].glyph_name.clone();
        let Some(&index) =
            self.font().and_then(|f| f.name_map.get(name.as_str()))
        else {
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
                self.editor =
                    std::mem::replace(&mut slot.editor, EditorState::new());
                self.edit_buffer = std::mem::replace(
                    &mut slot.buffer,
                    runebender_core::text::TextBuffer::new(),
                );
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

    /// The tab strip's "+": a fresh session on the current glyph.
    fn command_new_session(&mut self) {
        let glyph = match self.mode {
            Mode::Editor(i) => Some(i),
            Mode::Grid => self.last_editor.or(self.selected),
        };
        let Some(glyph) = glyph else { return };
        let Some(name) = self
            .font()
            .and_then(|f| f.glyphs.get(glyph))
            .map(|g| g.name.to_string())
        else {
            return;
        };
        self.park_active_session();
        self.sessions.push(EditSession {
            glyph_name: name,
            editor: EditorState::new(),
            buffer: runebender_core::text::TextBuffer::new(),
        });
        self.active_session = self.sessions.len() - 1;
        self.open_editor(glyph);
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
        Self::solve_grid(self.grid_viewport, self.grid_cell_size, GRID_PAD)
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

    fn solve_grid(
        viewport: gpui::Size<gpui::Pixels>,
        target: f32,
        pad: f32,
    ) -> GridFit {
        let label_h = |w: f32| if w >= 90.0 { 32.0 } else { 14.0 };
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
        let cols = (((usable_w + GRID_GAP) / (target + GRID_GAP)).floor()
            as usize)
            .max(1);
        let cell_w =
            ((usable_w - GRID_GAP * (cols - 1) as f32) / cols as f32).floor();

        let target_row = cell_w + label_h(cell_w);
        let usable_h = (vh - pad.min(GRID_PAD_Y) * 2.0).max(target_row);
        let rows = (((usable_h + GRID_GAP) / (target_row + GRID_GAP)).round()
            as usize)
            .max(1);
        let cell_h =
            ((usable_h - GRID_GAP * (rows - 1) as f32) / rows as f32).floor();
        GridFit {
            cell_w,
            cell_h,
            cols,
            rows,
        }
    }

    fn glyph_cell_sized(
        &self,
        index: usize,
        cell: f32,
        cell_h: f32,
        jump_on_click: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let font = self.font().unwrap();
        let entry = &font.glyphs[index];
        let name = entry.name.clone();
        let unicode_label: Option<SharedString> = entry
            .codepoint
            .map(|c| format!("U+{:04X}", c as u32).into());
        let selected = if jump_on_click {
            matches!(self.mode, Mode::Editor(i) if i == index)
        } else {
            self.selected == Some(index)
                || self.multi_selected.contains(name.as_ref())
        };
        let outline = entry.path.clone();
        let advance = entry.advance;
        let upm = font.units_per_em;
        // Labels are dropped once a cell is too small to carry them.
        let show_labels = cell >= 34.0;
        let label_h = if !show_labels {
            0.0
        } else if cell >= 90.0 {
            20.0
        } else {
            14.0
        };
        let incompatible = self
            .project
            .as_ref()
            .and_then(|p| p.compat.get(entry.name.as_ref()))
            .is_some_and(|ok| !ok);

        let label_h = if show_labels && cell >= 90.0 {
            32.0
        } else {
            label_h
        };
        let mark = entry.mark.as_deref().and_then(t::mark_color);
        div()
            .id(index)
            .w(px(cell))
            .h(px(cell_h))
            .flex()
            .flex_col()
            .bg(if selected { t::cell_selected_bg() } else { t::cell_bg() })
            .border_1()
            .border_color(if selected {
                t::cell_selected_ring()
            } else {
                mark.unwrap_or_else(t::cell_border)
            })
            .rounded_md()
            .cursor_pointer()
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                // Notes are transient: picking a glyph clears them so
                // the bottom bar's count shows again.
                this.status_note = None;
                if jump_on_click {
                    this.open_editor(index);
                } else {
                    let modifiers = event.modifiers();
                    if modifiers.platform {
                        // Cmd-click toggles membership.
                        this.grid_toggle_multi(index);
                    } else if modifiers.shift {
                        this.grid_extend_multi(index);
                    } else {
                        this.selected = Some(index);
                        this.multi_selected.clear();
                    }
                    if event.click_count() >= 2 {
                        this.open_editor(index);
                    }
                }
                cx.notify();
            }))
            .child(
                div().flex_1().child(
                    canvas(
                        move |bounds, _, _| bounds,
                        move |_, bounds: Bounds<gpui::Pixels>, window, _| {
                            use kurbo::Shape as _;
                            let h = f32::from(bounds.size.height) as f64;
                            let w = f32::from(bounds.size.width) as f64;
                            // The web's grid thumbnail box
                            // (glyph_svg.rs): one vertical scale for
                            // every glyph in the grid, so a period
                            // stays a dot and an M stays tall, with the
                            // baseline the same distance down each
                            // cell. The em window is a minimum, not a
                            // crop: it grows to hold ink that runs past
                            // it rather than clipping a descender.
                            const EM_FILL: f64 = 0.65;
                            const BASELINE_FROM_TOP: f64 = 0.8;
                            let ink = outline.bounding_box();
                            let (ink_x0, ink_w) = if outline.elements().is_empty()
                                || ink.width() <= 0.0
                            {
                                // Blank glyph: centre its advance.
                                (0.0, advance.max(1.0))
                            } else {
                                (ink.x0, ink.width())
                            };
                            let em_height = upm.max(1.0) / EM_FILL;
                            let em_top = -BASELINE_FROM_TOP * em_height;
                            let (top, bottom) = if outline.elements().is_empty() {
                                (em_top, em_top + em_height)
                            } else {
                                (
                                    em_top.min(-ink.y1),
                                    (em_top + em_height).max(-ink.y0),
                                )
                            };
                            let box_h = (bottom - top).max(1.0);
                            // "meet": the box fits inside the cell,
                            // centred on both axes.
                            let scale = (w / ink_w).min(h / box_h);
                            let x_offset = (w - ink_w * scale) / 2.0 - ink_x0 * scale;
                            let baseline =
                                (h - box_h * scale) / 2.0 - top * scale;
                            let transform = Affine::translate((x_offset, baseline))
                                * Affine::scale_non_uniform(scale, -scale);
                            if let Some(path) = build_fill_path(&outline, transform, bounds.origin)
                            {
                                window.paint_path(
                                    path,
                                    if selected {
                                        t::cell_selected_ring()
                                    } else {
                                        mark.unwrap_or_else(t::glyph_fill)
                                    },
                                );
                            }
                        },
                    )
                    // A canvas has no intrinsic size; without this it
                    // lays out at 0x0 and paints nothing.
                    .size_full(),
                ),
            )
            .when(show_labels, |el| el.child(
                div()
                    .h(px(label_h))
                    .px_1()
                    .flex()
                    .flex_col()
                    .text_size(px(if cell >= 90.0 { 10.0 } else { 8.0 }))
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_color(if selected {
                                t::cell_selected_ring()
                            } else {
                                mark.unwrap_or_else(t::text)
                            })
                            .when(incompatible, |el| {
                                el.child(
                                    div()
                                        .w(px(6.0))
                                        .h(px(6.0))
                                        .rounded_full()
                                        .bg(t::anchor()),
                                )
                            })
                            .child(name),
                    )
                    .when(cell >= 90.0, |el| {
                        el.child(
                            div()
                                .text_color(if selected {
                                    t::cell_selected_ring()
                                } else {
                                    mark.unwrap_or_else(t::text_muted)
                                })
                                .child(unicode_label.unwrap_or_else(|| "".into())),
                        )
                    }),
            ))
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
            SidebarFilter::Category(c) => category == *c,
            SidebarFilter::Subfilter(c, sub) => {
                category == *c
                    && sb::glyph_matches_subfilter(
                        name,
                        &Self::glyph_codepoints(font, name),
                        sub,
                    )
            }
            SidebarFilter::LanguageGroup(gi) => sb::language_groups()
                .get(*gi)
                .is_some_and(|group| {
                    sb::glyph_matches_language_group(
                        name,
                        &Self::glyph_codepoints(font, name),
                        group,
                    )
                }),
            SidebarFilter::Language(gi, fi) => sb::language_groups()
                .get(*gi)
                .and_then(|group| group.filters.get(*fi))
                .is_some_and(|f| {
                    sb::glyph_matches_character_filter(
                        name,
                        &Self::glyph_codepoints(font, name),
                        f,
                    )
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
                            cp.map(GC::from_codepoint).unwrap_or(GC::Other)
                                == *category
                        })
                        .count()
                }
            })
            .collect();
        let mut subfilters = std::collections::HashMap::new();
        for (ci, (category, label)) in SIDEBAR_CATEGORIES.iter().enumerate() {
            for (si, (sub, _)) in
                sb::category_subfilters(label).iter().enumerate()
            {
                let count = glyphs
                    .iter()
                    .filter(|(name, cp, cps)| {
                        cp.map(GC::from_codepoint).unwrap_or(GC::Other)
                            == *category
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
                    .filter(|(name, _, cps)| {
                        sb::glyph_matches_language_group(name, cps, group)
                    })
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
                                sb::glyph_matches_character_filter(
                                    name, cps, filter,
                                )
                            })
                            .count()
                    })
                    .collect(),
            );
            missing.push(
                group
                    .filters
                    .iter()
                    .map(|filter| {
                        sb::missing_targets(&name_cps, filter).len()
                    })
                    .collect(),
            );
        }
        let builtins = sb::builtin_filters()
            .iter()
            .map(|builtin| match &builtin.glyphset {
                Some(set) => glyphs
                    .iter()
                    .filter(|(name, _, cps)| {
                        sb::glyph_matches_character_filter(name, cps, set)
                    })
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
        self.sidebar_counts = Some(SidebarCounts {
            total: glyphs.len(),
            categories,
            subfilters,
            groups,
            languages,
            missing,
            builtins,
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
                self.glyph_passes_filter(
                    font,
                    entry.name.as_ref(),
                    entry.codepoint,
                    &filter,
                )
            })
            .map(|entry| entry.name.to_string())
            .collect();
        self.sidebar_matches = Some(matches);
    }

    /// File → New Font: an Untitled GF-template UFO, in memory until
    /// Save As picks a destination.
    fn command_new_font(&mut self) {
        // No std::env::temp_dir here: it panics on wasm. The path is
        // provisional either way — Save As replaces it.
        #[cfg(target_family = "wasm")]
        let path = PathBuf::from("Untitled.ufo");
        #[cfg(not(target_family = "wasm"))]
        let path = std::env::temp_dir().join("Untitled.ufo");
        self.axis_sliders.clear();
        self.sessions.clear();
        self.active_session = 0;
        self.project = Some(Project::new_font(path));
        self.mode = Mode::Grid;
        self.selected = None;
        self.multi_selected.clear();
        self.last_editor = None;
        self.sidebar_counts = None;
        self.sidebar_matches = None;
        self.sidebar_filter = SidebarFilter::All;
        self.search_query.clear();
        self.rebuild_text_models();
        self.status_note = Some(
            "New font · Save As… picks where it lives on disk".into(),
        );
    }

    /// Save As: pick a directory; the active master saves there under
    /// its family-style name and keeps saving there from now on.
    fn command_save_as(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Save In".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(dir) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |workspace, cx| {
                if let Some(project) = workspace.project.as_mut() {
                    for master in project.masters.iter_mut() {
                        let family = master
                            .font
                            .font_info
                            .family_name
                            .clone()
                            .unwrap_or_else(|| "Untitled".into())
                            .replace(' ', "");
                        let style = master
                            .font
                            .font_info
                            .style_name
                            .clone()
                            .unwrap_or_else(|| "Regular".into())
                            .replace(' ', "");
                        master.source_path =
                            dir.join(format!("{family}-{style}.ufo"));
                        master.dirty = true;
                    }
                }
                workspace.command_save(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The grid's visible order (same filter + sort the grid draws).
    fn visible_grid_indices(&self) -> Vec<usize> {
        let Some(font) = self.font() else { return Vec::new() };
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
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string())
        else {
            return;
        };
        if let Some(primary) = self.selected {
            if let Some(primary_name) =
                self.font().map(|f| f.glyphs[primary].name.to_string())
            {
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
        let Some(font) = self.font() else { return Vec::new() };
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

    /// Copy the selection as text (the glyphs' characters), the web
    /// sidebar footer's action.
    fn command_copy_selection_text(&mut self, cx: &mut Context<Self>) {
        let Some(font) = self.font() else { return };
        let text: String = self
            .selection_names()
            .iter()
            .filter_map(|name| {
                font.name_map
                    .get(name)
                    .and_then(|&i| font.glyphs[i].codepoint)
            })
            .collect();
        if text.is_empty() {
            self.status_note = Some("Nothing encoded to copy".into());
            return;
        }
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
        self.status_note = Some(
            format!("Copied {} character{}", text.chars().count(), {
                if text.chars().count() == 1 { "" } else { "s" }
            })
            .into(),
        );
    }

    /// Does a glyph match the sidebar search, honoring scope, regex,
    /// and case options (web glyphMatchesSidebarSearch)?
    fn search_matches(&self, name: &str, codepoint: Option<char>) -> bool {
        let query = self.search_query.trim();
        if query.is_empty() {
            return true;
        }
        let unicode_hex = codepoint
            .map(|c| format!("{:04X}", c as u32))
            .unwrap_or_default();
        let chars = codepoint.map(String::from).unwrap_or_default();
        let haystacks: Vec<&str> = match self.search_mode {
            1 => vec![name],
            2 => vec![unicode_hex.as_str(), chars.as_str()],
            _ => vec![name, unicode_hex.as_str(), chars.as_str()],
        };
        if self.search_regex {
            let pattern = if self.search_case {
                query.to_string()
            } else {
                format!("(?i){query}")
            };
            return match regex::Regex::new(&pattern) {
                Ok(re) => haystacks.iter().any(|h| re.is_match(h)),
                // A half-typed pattern matches everything, like the web.
                Err(_) => true,
            };
        }
        if self.search_case {
            haystacks.iter().any(|h| h.contains(query))
        } else {
            let needle = query.to_lowercase();
            haystacks
                .iter()
                .any(|h| h.to_lowercase().contains(&needle))
        }
    }

    /// Add every glyph a target-bearing language filter still misses
    /// (web generateMissing): empty glyphs named and encoded from the
    /// filter's targets, in every master.
    fn command_generate_missing(&mut self, group: usize, filter_index: usize) {
        use runebender_core::sidebar as sb;
        let Some(filter) = sb::language_groups()
            .get(group)
            .and_then(|g| g.filters.get(filter_index))
        else {
            return;
        };
        let existing: Vec<(String, Vec<u32>)> = self
            .font()
            .map(|f| {
                f.glyphs
                    .iter()
                    .map(|entry| {
                        (
                            entry.name.to_string(),
                            Self::glyph_codepoints(f, entry.name.as_ref()),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let targets: Vec<(String, u32)> = sb::missing_targets(&existing, filter)
            .into_iter()
            .map(|t| (t.name.clone(), t.unicode))
            .collect();
        if targets.is_empty() {
            return;
        }
        let Some(project) = self.project.as_mut() else { return };
        let upm = project.active_font().units_per_em;
        let mut added = 0usize;
        for master in project.masters.iter_mut() {
            for (name, unicode) in &targets {
                if master.name_map.contains_key(name) {
                    continue;
                }
                let mut glyph = norad::Glyph::new(name.as_str());
                glyph.width = (upm * 0.5).round();
                if let Some(c) = char::from_u32(*unicode) {
                    glyph.codepoints = norad::Codepoints::new([c]);
                }
                master.font.default_layer_mut().insert_glyph(glyph);
                master.dirty = true;
                master.modified_glyphs.insert(name.clone());
            }
            master.refresh_from_font();
        }
        added += targets.len();
        self.sidebar_counts = None;
        self.status_note = Some(
            format!(
                "Added {added} missing glyph{}",
                if added == 1 { "" } else { "s" }
            )
            .into(),
        );
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
                let pt = |dx: f32, dy: f32| {
                    gpui::point(o.x + px(cx_ + dx), o.y + px(cy + dy))
                };
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
            .rounded_sm()
            .text_sm()
            .cursor_pointer()
            .flex()
            .items_center()
            .gap_1()
            .when(active, |el| {
                el.border_1().border_color(t::accent()).text_color(t::accent())
            })
            .when(!active, |el| el.text_color(t::text()))
            .when_some(chevron, |el, expanded| {
                el.child(Self::row_chevron(expanded))
            })
            .when_some(icon, |el, icon| {
                el.child(
                    div()
                        .w(px(16.0))
                        .text_color(if active {
                            t::accent()
                        } else {
                            t::text_muted()
                        })
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
            .rounded_sm()
            .border_1()
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .cursor_pointer()
            .when(active, |el| {
                el.border_color(t::accent()).text_color(t::accent())
            })
            .when(!active, |el| {
                el.border_color(t::cell_border()).text_color(t::text_muted())
            })
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                on(this);
                cx.notify();
            }))
    }

    fn category_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        use runebender_core::sidebar as sb;
        let counts = self.sidebar_counts.as_ref();

        // Categories: expandable rows with the web's subfilters.
        let mut categories = div().flex().flex_col();
        for (ci, (category, label)) in SIDEBAR_CATEGORIES.iter().enumerate() {
            let subs = sb::category_subfilters(label);
            let count = counts.map(|c| c.categories[ci]).unwrap_or(0);
            let expanded = self.expanded_categories.contains(&ci);
            let mut row = self
                .sidebar_row(
                    ("category", ci),
                    false,
                    (!subs.is_empty()).then_some(expanded),
                    None,
                    SharedString::from(*label),
                    format!("{count}").into(),
                    if ci == 0 {
                        SidebarFilter::All
                    } else {
                        SidebarFilter::Category(*category)
                    },
                    cx,
                )
                .into_any_element();
            if !subs.is_empty() {
                // A separate click target for the chevron would fight
                // the row click; double-purpose: clicking an already
                // selected row toggles expansion instead.
                let category = *category;
                let selected = self.sidebar_filter
                    == SidebarFilter::Category(category)
                    || subs.iter().any(|(sub, _)| {
                        self.sidebar_filter
                            == SidebarFilter::Subfilter(category, sub)
                    });
                row = self
                    .sidebar_row(
                        ("category", ci),
                        false,
                        Some(expanded),
                        None,
                        SharedString::from(*label),
                        format!("{count}").into(),
                        SidebarFilter::Category(category),
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if selected {
                            if !this.expanded_categories.remove(&ci) {
                                this.expanded_categories.insert(ci);
                            }
                        }
                        this.set_sidebar_filter(SidebarFilter::Category(
                            category,
                        ));
                        cx.notify();
                    }))
                    .into_any_element();
            }
            categories = categories.child(row);
            if expanded {
                for (si, (sub, sub_label)) in subs.iter().enumerate() {
                    let count = counts
                        .and_then(|c| c.subfilters.get(&(ci, si)).copied())
                        .unwrap_or(0);
                    categories = categories.child(self.sidebar_row(
                        ("subfilter", ci * 100 + si),
                        true,
                        None,
                        None,
                        SharedString::from(*sub_label),
                        format!("{count}").into(),
                        SidebarFilter::Subfilter(*category, sub),
                        cx,
                    ));
                }
            }
        }

        // Languages: script groups with per-set coverage, like the
        // web sidebar and Glyphs.
        let mut languages = div().flex().flex_col();
        for (gi, group) in sb::language_groups().iter().enumerate() {
            let count = counts.map(|c| c.groups[gi]).unwrap_or(0);
            let expanded = self.expanded_scripts.contains(&gi);
            let selected = self.sidebar_filter
                == SidebarFilter::LanguageGroup(gi)
                || (0..group.filters.len()).any(|fi| {
                    self.sidebar_filter == SidebarFilter::Language(gi, fi)
                });
            languages = languages.child(
                self.sidebar_row(
                    ("script", gi),
                    false,
                    Some(expanded),
                    Some(group.icon.clone().into()),
                    group.label.clone().into(),
                    format!("{count}").into(),
                    SidebarFilter::LanguageGroup(gi),
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if selected {
                        if !this.expanded_scripts.remove(&gi) {
                            this.expanded_scripts.insert(gi);
                        }
                    } else {
                        this.expanded_scripts.insert(gi);
                    }
                    this.set_sidebar_filter(SidebarFilter::LanguageGroup(gi));
                    cx.notify();
                })),
            );
            if expanded {
                for (fi, filter) in group.filters.iter().enumerate() {
                    let count = counts
                        .map(|c| c.languages[gi][fi])
                        .unwrap_or(0);
                    let missing = counts
                        .map(|c| c.missing[gi][fi])
                        .unwrap_or(0);
                    let count_text = match filter.expected_count {
                        Some(expected) => format!("{count}/{expected}"),
                        None => format!("{count}"),
                    };
                    let row = self.sidebar_row(
                        ("language", gi * 100 + fi),
                        true,
                        None,
                        None,
                        filter.label.clone().into(),
                        count_text.into(),
                        SidebarFilter::Language(gi, fi),
                        cx,
                    );
                    if missing > 0 {
                        // "+" generates the filter's missing glyphs.
                        languages = languages.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(div().flex_1().child(row))
                                .child(
                                    div()
                                        .id(("gen-missing", gi * 100 + fi))
                                        .w(px(18.0))
                                        .h(px(18.0))
                                        .rounded_sm()
                                        .border_1()
                                        .border_color(t::cell_border())
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_sm()
                                        .text_color(t::text_muted())
                                        .cursor_pointer()
                                        .child("+")
                                        .on_click(cx.listener(
                                            move |this, _, _, cx| {
                                                this.command_generate_missing(
                                                    gi, fi,
                                                );
                                                cx.notify();
                                            },
                                        )),
                                ),
                        );
                    } else {
                        languages = languages.child(row);
                    }
                }
            }
        }

        // Filters: the Runebender builtins plus headline GF sets.
        let mut filters = div().flex().flex_col();
        for (bi, builtin) in sb::builtin_filters().iter().enumerate() {
            let count = counts.map(|c| c.builtins[bi]).unwrap_or(0);
            let count_text = match builtin
                .glyphset
                .as_ref()
                .and_then(|set| set.expected_count)
            {
                Some(expected) => format!("{count}/{expected}"),
                None => format!("{count}"),
            };
            filters = filters.child(self.sidebar_row(
                ("builtin", bi),
                false,
                None,
                None,
                builtin.label.clone().into(),
                count_text.into(),
                SidebarFilter::Builtin(bi),
                cx,
            ));
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .p_2()
                    .flex()
                    .items_stretch()
                    .gap_1()
                    .border_b_1()
                    .border_color(t::panel_outline())
                    .child(div().flex_1().child(
                        gpui_component::input::Input::new(&self.search),
                    ))
                    .child(self.search_toggle(
                        "search-mode",
                        match self.search_mode {
                            1 => "N",
                            2 => "U",
                            _ => "A",
                        },
                        self.search_mode != 0,
                        |this| this.search_mode = (this.search_mode + 1) % 3,
                        cx,
                    ))
                    .child(self.search_toggle(
                        "search-regex",
                        ".*",
                        self.search_regex,
                        |this| this.search_regex = !this.search_regex,
                        cx,
                    ))
                    .child(self.search_toggle(
                        "search-case",
                        "Aa",
                        self.search_case,
                        |this| this.search_case = !this.search_case,
                        cx,
                    )),
            )
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .child(self.section(cx, "Categories", categories))
                    .child(self.section(cx, "Languages", languages))
                    .child(self.section(cx, "Filters", filters)),
            )
            // Mark colours sit at the foot of the sidebar, beside the
            // glyphs they apply to, the way the web places them.
            .child(self.mark_colors_panel(cx))
    }

    /// Right tile: details of the selected glyph, like
    /// runebender-web's GlyphInfoSidebar.
    fn glyph_info_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        // Read-only facts read as one line each, label left and value
        // right. A stack of big accent-green headings for one-word
        // values was most of what made this panel shout.
        let row = |header: &'static str, value: SharedString| {
            div()
                .h(px(18.0))
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .text_xs()
                .child(div().text_color(t::text_muted()).child(header))
                .child(div().text_sm().text_color(t::text()).child(value))
        };
        let mut panel = div().flex().flex_col().gap_2();
        let (Some(project), Some(index)) = (self.project.as_ref(), self.selected) else {
            return self.section(
                cx,
                "Glyph",
                div()
                    .text_sm()
                    .text_color(t::text_muted())
                    .child("Select a glyph"),
            );
        };
        let font = project.active_font();
        let Some(entry) = font.glyphs.get(index) else {
            return self.section(cx, "Glyph", div());
        };
        let name = entry.name.to_string();
        let master = project.master_names[project.active].clone();
        let contours = font
            .font
            .get_glyph(name.as_str())
            .map(|g| g.contours.len())
            .unwrap_or(0);
        let _ = name;
        // Editable fields commit on Enter (rename, unicode, kerning
        // groups); the rest stay read-only rows.
        let input_row = |header: &'static str,
                         input: &gpui::Entity<gpui_component::input::InputState>| {
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(div().text_xs().text_color(t::text_muted()).child(header))
                .child(gpui_component::input::Input::new(input))
        };
        let pair_row = |header: &'static str,
                        a: &gpui::Entity<gpui_component::input::InputState>,
                        b: &gpui::Entity<gpui_component::input::InputState>| {
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(div().text_xs().text_color(t::text_muted()).child(header))
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child(div().flex_1().child(
                            gpui_component::input::Input::new(a),
                        ))
                        .child(div().flex_1().child(
                            gpui_component::input::Input::new(b),
                        )),
                )
        };
        // Width and the sidebearings are edited here, beside the name
        // and the kerning groups, the way the web keeps a glyph's
        // metrics in one panel. Enter commits each field.
        let metric_field = |label_text: &'static str,
                            input: &gpui::Entity<gpui_component::input::InputState>| {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(
                    div()
                        .text_xs()
                        .text_color(t::text_muted())
                        .child(label_text),
                )
                .child(gpui_component::input::Input::new(input))
        };
        // In the editor the metric fields live in the floating panel
        // over the canvas (Glyphs-style), so they appear here only in
        // the grid: one input entity, one place on screen.
        let in_editor = matches!(self.mode, Mode::Editor(_));
        panel = panel
            .child(row("Master", master))
            .child(input_row("Glyph Name", &self.glyph_inputs.name))
            .when(in_editor, |el| {
                el.child(row("Width", format!("{:.0}", entry.advance).into()))
            })
            .when(!in_editor, |el| {
                el.child(
                    div()
                        .flex()
                        .gap_1()
                        .child(metric_field("Width", &self.metric_inputs.width))
                        .child(metric_field("LSB", &self.metric_inputs.lsb))
                        .child(metric_field("RSB", &self.metric_inputs.rsb)),
                )
            })
            .child(pair_row(
                "Kerning Groups (L · R)",
                &self.glyph_inputs.group_l,
                &self.glyph_inputs.group_r,
            ))
            .child(input_row("Unicode", &self.glyph_inputs.unicode))
            .child(row("Contours", format!("{contours}").into()));
        self.section(cx, "Glyph", panel)
    }

    /// Colors panel: mark-color swatches for the selected glyph, like
    /// the web grid's bottom-right panel.
    /// Right-panel live preview of the selected glyph: outline plus
    /// control points, the way runebender-web fills the space between
    /// the info sections and the colors.
    fn glyph_preview_panel(&self) -> gpui::Div {
        let data = self.selected.and_then(|index| {
            let font = self.font()?;
            let entry = &font.glyphs[index];
            Some((
                entry.contour_path.clone(),
                entry.component_path.clone(),
                entry.points.clone(),
                entry.advance,
                font.ascender,
                font.descender,
            ))
        });
        let body: gpui::AnyElement = match data {
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(t::text_muted())
                .into_any_element(),
            Some((outline, components, points, advance, ascender, descender)) => {
                canvas(
                    move |bounds, _, _| bounds,
                    move |_, bounds: Bounds<gpui::Pixels>, window, _| {
                        let w: f32 = bounds.size.width.into();
                        let h: f32 = bounds.size.height.into();
                        if w < 40.0 || h < 40.0 {
                            return;
                        }
                        // Fit the em box (with the glyph's actual
                        // advance) into the panel with padding.
                        use kurbo::Shape as _;
                        let ink = {
                            let mut b = outline.bounding_box();
                            if !components.elements().is_empty() {
                                b = b.union(components.bounding_box());
                            }
                            b
                        };
                        let (ink_w, ink_h, ink_cx, ink_cy) =
                            if ink.width() > 0.0 && ink.height() > 0.0 {
                                (ink.width(), ink.height(), ink.center().x, ink.center().y)
                            } else {
                                // A blank glyph still needs a box: use
                                // its advance and the em.
                                let em_h = (ascender - descender).max(1.0);
                                let em_w = advance.max(em_h * 0.3);
                                (em_w, em_h, em_w / 2.0, (ascender + descender) / 2.0)
                            };
                        let scale = ((w as f64 * 0.88) / ink_w)
                            .min((h as f64 * 0.88) / ink_h);
                        let origin_x = w as f64 / 2.0 - ink_cx * scale;
                        let baseline = h as f64 / 2.0 + ink_cy * scale;
                        let view = Affine::translate((origin_x, baseline))
                            * Affine::scale_non_uniform(scale, -scale);
                        let to_screen = |x: f64, y: f64| {
                            let p = view * kurbo::Point::new(x, y);
                            gpui::point(
                                bounds.origin.x + px(p.x as f32),
                                bounds.origin.y + px(p.y as f32),
                            )
                        };
                        // No metric frame here: this tile is a shape
                        // preview, and the box only ate the space the
                        // outline could use.
                        if !components.elements().is_empty()
                            && let Some(p) = build_fill_path(
                                &components,
                                view,
                                bounds.origin,
                            )
                        {
                            window.paint_path(p, t::component_fill());
                        }
                        if let Some(p) = build_path(
                            &outline,
                            view,
                            bounds.origin,
                            PathBuilder::stroke(px(1.0)),
                        ) {
                            window.paint_path(p, t::path_stroke());
                        }
                        // Handle lines then points, editor-style but
                        // small.
                        let mut handles = PathBuilder::stroke(px(1.0));
                        let mut any_handles = false;
                        for p in points.iter() {
                            if p.on_curve {
                                continue;
                            }
                            let contour_pts: Vec<&GlyphPoint> = points
                                .iter()
                                .filter(|q| q.contour == p.contour)
                                .collect();
                            let n = contour_pts.len();
                            let Some(pos) = contour_pts
                                .iter()
                                .position(|q| q.index == p.index)
                            else {
                                continue;
                            };
                            let prev = contour_pts[(pos + n - 1) % n];
                            let next = contour_pts[(pos + 1) % n];
                            let anchor = if prev.on_curve {
                                prev
                            } else if next.on_curve {
                                next
                            } else {
                                continue;
                            };
                            handles.move_to(to_screen(p.x, p.y));
                            handles.line_to(to_screen(anchor.x, anchor.y));
                            any_handles = true;
                        }
                        if any_handles && let Ok(p) = handles.build() {
                            window.paint_path(p, t::handle_line());
                        }
                        let ring =
                            |center: Point<gpui::Pixels>,
                             r: f32,
                             color: gpui::Rgba,
                             window: &mut Window| {
                                let cx_: f32 = center.x.into();
                                let cy_: f32 = center.y.into();
                                let shape = kurbo::Circle::new(
                                    (cx_ as f64, cy_ as f64),
                                    r as f64,
                                )
                                .to_path(0.25);
                                if let Some(p) = build_fill_path(
                                    &shape,
                                    Affine::IDENTITY,
                                    gpui::point(px(0.0), px(0.0)),
                                ) {
                                    window.paint_path(p, t::point_inner());
                                }
                                if let Some(p) = build_path(
                                    &shape,
                                    Affine::IDENTITY,
                                    gpui::point(px(0.0), px(0.0)),
                                    PathBuilder::stroke(px(1.0)),
                                ) {
                                    window.paint_path(p, color);
                                }
                            };
                        for p in points.iter() {
                            let center = to_screen(p.x, p.y);
                            if !p.on_curve {
                                ring(
                                    center,
                                    2.0,
                                    t::point_offcurve_outer(),
                                    window,
                                );
                            } else if p.smooth {
                                ring(
                                    center,
                                    3.0,
                                    t::point_smooth_outer(),
                                    window,
                                );
                            } else if p.hyper {
                                ring(center, 3.0, t::point_hyper_outer(), window);
                            } else {
                                window.paint_quad(gpui::fill(
                                    Bounds::from_corners(
                                        gpui::point(
                                            center.x - px(2.5),
                                            center.y - px(2.5),
                                        ),
                                        gpui::point(
                                            center.x + px(2.5),
                                            center.y + px(2.5),
                                        ),
                                    ),
                                    t::point_corner_outer(),
                                ));
                            }
                        }
                    },
                )
                .size_full()
                .into_any_element()
            }
        };
        div().flex_1().min_h(px(200.0)).p_1().child(body)
    }

    fn mark_colors_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        let current = self
            .selected
            .and_then(|i| self.font().and_then(|f| f.glyphs.get(i)))
            .and_then(|e| e.mark.clone());
        // One row, always: each swatch sits in an equal-width column,
        // so the spacing between them and the margin at both ends stay
        // the same at any panel width.
        const SWATCH: f32 = 14.0;
        // (bar height - swatch) / 2, so the ring of space around the
        // row is the same on every side.
        const INSET: f32 = (BOTTOM_BAR_H - SWATCH) / 2.0;
        let slot = |child: gpui::Stateful<gpui::Div>| child;
        let mut swatches =
            div().flex().items_center().justify_between().w_full();
        for (index, (label, color)) in t::mark_palette().into_iter().enumerate() {
            let is_current = current.as_deref() == Some(label.as_str());
            swatches = swatches.child(slot(
                div()
                    .id(("mark-swatch", index))
                    .w(px(SWATCH))
                    .h(px(SWATCH))
                    .flex_shrink_0()
                    .rounded_full()
                    .bg(color)
                    .border_2()
                    .border_color(if is_current {
                        t::cell_selected_ring()
                    } else {
                        gpui::Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }
                    })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_selected_mark(Some(label.clone()));
                        cx.notify();
                    })),
            ));
        }
        swatches = swatches.child(slot(
            div()
                .id("mark-clear")
                .w(px(SWATCH))
                .h(px(SWATCH))
                .flex_shrink_0()
                .rounded_full()
                .border_1()
                .border_color(if current.is_none() {
                    t::cell_selected_ring()
                } else {
                    t::cell_border()
                })
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(t::text_muted())
                .cursor_pointer()
                .child("×")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.set_selected_mark(None);
                    cx.notify();
                })),
        ));
        // No header, no collapse: it is one row of swatches that is
        // always up. It carries its own top rule since it is the last
        // thing in the sidebar.
        div()
            .h(px(BOTTOM_BAR_H))
            .flex()
            .items_center()
            .border_t_1()
            .border_color(t::panel_outline())
            .px(px(INSET))
            .child(swatches)
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
                    runebender_core::theme_oklch::set_glyph_mark(
                        glyph,
                        label.as_deref(),
                    );
                });
            }
        }
    }

    /// Editor sidebar: search + scrollable mini glyph grid, so glyph
    /// switching doesn't require leaving the editor.
    fn editor_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let _query = self.search_query.clone();
        let fit = self.sidebar_cell_metrics();
        let mut rows_total = 0usize;
        let mut shown = 0usize;
        let cells: Vec<_> = match self.font() {
            Some(font) => {
                let matched: Vec<usize> = (0..font.glyphs.len())
                    .filter(|&i| {
                        self.search_matches(
                            font.glyphs[i].name.as_ref(),
                            font.glyphs[i].codepoint,
                        )
                    })
                    .collect();
                shown = matched.len();
                rows_total = matched.len().div_ceil(fit.cols);
                let start = self
                    .sidebar_scroll_row
                    .min(rows_total.saturating_sub(1))
                    * fit.cols;
                matched
                    .into_iter()
                    .skip(start)
                    .take(fit.cols * fit.rows)
                    .map(|i| {
                        self.glyph_cell_sized(
                            i, fit.cell_w, fit.cell_h, true, cx,
                        )
                        .into_any_element()
                    })
                    .collect()
            }
            None => Vec::new(),
        };
        // The sidebar's own tabs, like the web's editor sidebar: the
        // glyph list, and the designspace axes.
        let has_axes = !self.axis_sliders.is_empty();
        let tab = |id: &'static str, label: &'static str, which: u8, cx: &mut Context<Self>| {
            div()
                .id(id)
                .h(px(20.0))
                .px_2()
                .flex()
                .items_center()
                .rounded_sm()
                .text_xs()
                .cursor_pointer()
                .when(self.sidebar_tab == which, |el| {
                    el.border_1().border_color(t::accent()).text_color(t::accent())
                })
                .when(self.sidebar_tab != which, |el| {
                    el.border_1()
                        .border_color(t::cell_border())
                        .text_color(t::text_muted())
                })
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.sidebar_tab = which;
                    cx.notify();
                }))
        };
        // An axis-less font has no Axes tab, so a stale selection
        // falls back to the glyph list.
        let tab_now = if !has_axes && self.sidebar_tab == 2 {
            0
        } else {
            self.sidebar_tab
        };
        let on_glyphs = tab_now == 0;
        div()
            .size_full()
            .flex()
            .flex_col()
            .min_h(px(0.0))
            .child(
                div()
                    .px_2()
                    .pt_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(tab("sidebar-tab-glyphs", "Glyphs", 0, cx))
                    .child(tab("sidebar-tab-shapes", "Shapes", 1, cx))
                    .when(has_axes, |el| {
                        el.child(tab("sidebar-tab-axes", "Axes", 2, cx))
                    })
                    .child(tab("sidebar-tab-chat", "Chat", 3, cx)),
            )
            .when(tab_now == 1, |el| {
                el.child(
                    div()
                        .id("sidebar-shapes")
                        .flex_1()
                        .min_h(px(0.0))
                        .overflow_y_scroll()
                        .child(self.sidebar_shapes(cx)),
                )
            })
            .when(tab_now == 2, |el| {
                el.child(
                    div()
                        .flex_1()
                        .min_h(px(0.0))
                        .p_2()
                        .children(self.axes_section(cx)),
                )
            })
            .when(tab_now == 3, |el| {
                el.child(
                    div()
                        .flex_1()
                        .min_h(px(0.0))
                        .p_2()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .text_xs()
                        .text_color(t::text_muted())
                        .child("Chat")
                        .child(
                            "A place for an assistant that can see the                              glyph. Not wired up yet.",
                        ),
                )
            })
            .when(on_glyphs, |el| el
            .child(
                div()
                    .p_2()
                    .flex()
                    .items_stretch()
                    .gap_1()
                    .border_b_1()
                    .border_color(t::panel_outline())
                    .child(div().flex_1().child(
                        gpui_component::input::Input::new(&self.search),
                    ))
                    .child(self.search_toggle(
                        "search-mode",
                        match self.search_mode {
                            1 => "N",
                            2 => "U",
                            _ => "A",
                        },
                        self.search_mode != 0,
                        |this| this.search_mode = (this.search_mode + 1) % 3,
                        cx,
                    ))
                    .child(self.search_toggle(
                        "search-regex",
                        ".*",
                        self.search_regex,
                        |this| this.search_regex = !this.search_regex,
                        cx,
                    ))
                    .child(self.search_toggle(
                        "search-case",
                        "Aa",
                        self.search_case,
                        |this| this.search_case = !this.search_case,
                        cx,
                    )),
            )
            .child(
                // Measured the same way the main grid is, so the mini
                // cells stretch to fill the pane and a row is never
                // left half-showing at the bottom.
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .relative()
                    .child({
                        let this = cx.entity().downgrade();
                        canvas(
                            move |bounds: Bounds<gpui::Pixels>,
                                  _,
                                  app: &mut gpui::App| {
                                this.update(app, |this, cx| {
                                    if this.sidebar_viewport != bounds.size {
                                        this.sidebar_viewport = bounds.size;
                                        cx.notify();
                                    }
                                })
                                .ok();
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full()
                    })
                    .child(
                        div()
                            .id("editor-sidebar-grid")
                            .size_full()
                            .min_h(px(0.0))
                            .overflow_hidden()
                            .child(
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
                                            .children(cells),
                                    ),
                            )
                            .on_scroll_wheel(cx.listener(
                                move |this, ev: &gpui::ScrollWheelEvent, _, cx| {
                                    let dy = match ev.delta {
                                        gpui::ScrollDelta::Pixels(p) => f32::from(p.y),
                                        gpui::ScrollDelta::Lines(p) => p.y * 24.0,
                                    };
                                    if Self::scroll_grid_rows(
                                        &mut this.sidebar_scroll_row,
                                        dy,
                                        fit.cell_h + GRID_GAP,
                                        fit.rows,
                                        rows_total,
                                    ) {
                                        cx.notify();
                                    }
                                },
                            )),
                    ),
            )
            .child(
                // Same bar the main grid has, and the same height, so
                // the two line up across the divider.
                div()
                    .h(px(BOTTOM_BAR_H))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .border_t_1()
                    .border_color(t::panel_outline())
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(t::text_muted())
                            .child(SharedString::from(format!(
                                "{} glyphs",
                                shown
                            ))),
                    )
                    .children(self.sidebar_slider.as_ref().map(|slider| {
                        div().w(px(96.0)).child(flat_slider(slider, cx))
                    })),
            ))
            // Colours stay put whichever tab is up.
            .child(self.mark_colors_panel(cx))
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
                .rounded_sm()
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

        let counts: Vec<usize> =
            glyph.contours.iter().map(|c| c.points.len()).collect();
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
                    let Mode::Editor(index) = this.mode else { return };
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
                                let mut path =
                                    gpui::PathBuilder::fill();
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
            .rounded_md()
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
            .child(Self::icon_tile("tool-pen", "pen", tool == Tool::Pen).on_click(
                cx.listener(|this, _, _, cx| {
                    this.editor.tool = Tool::Pen;
                    cx.notify();
                }),
            ))
            .child(
                Self::icon_tile(
                    "tool-shapes",
                    if self.editor.shape_ellipse { "shape-ellipse" } else { "shape-rectangle" },
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
                Self::icon_tile("tool-text", "text", tool == Tool::Text).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.pen_finish();
                        this.editor.tool = Tool::Text;
                        cx.notify();
                    }),
                ),
            )
            .child(
                Self::icon_tile("tool-knife", "knife", tool == Tool::Knife).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.pen_finish();
                        this.editor.tool = Tool::Knife;
                        cx.notify();
                    }),
                ),
            )
            .child(
                Self::icon_tile("tool-hyperpen", "hyperpen", tool == Tool::HyperPen)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.pen_finish();
                        this.editor.tool = Tool::HyperPen;
                        cx.notify();
                    })),
            )
            .child(
                Self::icon_tile("tool-preview", "preview", tool == Tool::Preview)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.pen_finish();
                        if this.editor.tool == Tool::Preview {
                            this.editor.tool = this.editor.previous_tool;
                        } else {
                            this.editor.previous_tool = this.editor.tool;
                            this.editor.tool = Tool::Preview;
                        }
                        cx.notify();
                    })),
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
                .rounded_sm()
                .border_1()
                .border_color(if active { t::accent() } else { t::cell_border() })
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
                button("dir-ltr", "LTR", !auto && dir == TextDirection::LeftToRight)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.edit_buffer
                            .set_direction(runebender_core::text::TextDirection::LeftToRight);
                        this.edit_buffer.shape_arabic_if_rtl();
                        this.sync_sort_offset();
                        cx.notify();
                    })),
            )
            .child(
                button("dir-rtl", "RTL", !auto && dir == TextDirection::RightToLeft)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.edit_buffer
                            .set_direction(runebender_core::text::TextDirection::RightToLeft);
                        this.edit_buffer.shape_arabic_if_rtl();
                        this.sync_sort_offset();
                        cx.notify();
                    })),
            )
            .child(button("dir-auto", "Auto", auto).on_click(cx.listener(
                |this, _, _, cx| {
                    this.edit_buffer.set_auto_direction();
                    this.edit_buffer.shape_arabic_if_rtl();
                    this.sync_sort_offset();
                    cx.notify();
                },
            )))
    }

    /// Transformations section for the right sidebar (editor mode).
    fn transform_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let text_op = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_2()
                .py_0p5()
                .rounded_sm()
                .text_sm()
                .text_color(t::text())
                .cursor_pointer()
                .border_1()
                .border_color(t::cell_border())
                .child(label)
        };
        self.section(
            cx,
            "Transformations",
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child(Self::icon_tile("op-flip-h", "flip-h", false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.apply_transform(Affine::scale_non_uniform(-1.0, 1.0));
                                cx.notify();
                            }),
                        ))
                        .child(Self::icon_tile("op-flip-v", "flip-v", false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.apply_transform(Affine::scale_non_uniform(1.0, -1.0));
                                cx.notify();
                            }),
                        ))
                        .child(Self::icon_tile("op-rot-ccw", "rot-ccw", false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.apply_transform(Affine::rotate(
                                    std::f64::consts::FRAC_PI_2,
                                ));
                                cx.notify();
                            }),
                        ))
                        .child(Self::icon_tile("op-rot-cw", "rot-cw", false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.apply_transform(Affine::rotate(
                                    -std::f64::consts::FRAC_PI_2,
                                ));
                                cx.notify();
                            }),
                        ))
                        .child(
                            Self::icon_tile("op-duplicate", "duplicate", false)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.command_duplicate();
                                    cx.notify();
                                })),
                        )
                        .child(
                            Self::icon_tile(
                                "op-duplicate-repeat",
                                "duplicate-repeat",
                                false,
                            )
                            .on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.command_duplicate_repeat();
                                    cx.notify();
                                },
                            )),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child(Self::icon_tile("op-union", "union", false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.command_boolean(linesweeper::BinaryOp::Union);
                                cx.notify();
                            }),
                        ))
                        .child(Self::icon_tile("op-subtract", "subtract", false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.command_boolean(linesweeper::BinaryOp::Difference);
                                cx.notify();
                            }),
                        ))
                        .child(Self::icon_tile("op-intersect", "intersect", false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.command_boolean(
                                    linesweeper::BinaryOp::Intersection,
                                );
                                cx.notify();
                            }),
                        ))
                        .child(Self::icon_tile("op-exclude", "exclude", false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.command_boolean(linesweeper::BinaryOp::Xor);
                                cx.notify();
                            }),
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .child(text_op("op-harmonize", "Harmonize").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.apply_curve_op(CurveOp::Harmonize);
                                cx.notify();
                            },
                        )))
                        .child(text_op("op-balance", "Balance").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.apply_curve_op(CurveOp::Balance);
                                cx.notify();
                            },
                        )))
                        .child(text_op("op-optimize", "Optimize").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.apply_curve_op(CurveOp::Optimize(0.12));
                                cx.notify();
                            },
                        )))
                        .child(text_op("op-round", "Round").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.command_round_corners();
                                cx.notify();
                            },
                        )))
                        .child(text_op("op-reverse", "Reverse").on_click(cx.listener(
                            |this, _, _, cx| {
                                if let Mode::Editor(index) = this.mode {
                                    this.push_undo_snapshot(index);
                                    let selected = this.editor.selected.clone();
                                    let changed = this
                                        .font_mut()
                                        .and_then(|f| {
                                            f.edit_glyph(index, |g| {
                                                runebender_core::glyph_ops::reverse_contours(
                                                    g, &selected,
                                                )
                                            })
                                        })
                                        .unwrap_or(false);
                                    if !changed {
                                        this.editor.undo.pop();
                                    } else {
                                        this.editor.selected.clear();
                                    }
                                }
                                cx.notify();
                            },
                        ))),
                ),
        )
    }

    /// Copy the open glyph's outline into the UFO background layer
    /// (public.background), creating the layer on first use.
    fn command_send_to_background(&mut self) {
        let Mode::Editor(index) = self.mode else { return };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string())
        else {
            return;
        };
        if let Some(font) = self.font_mut() {
            let source = font.font.get_glyph(name.as_str()).cloned();
            if let (Some(source), Ok(layer)) = (
                source,
                font.font.layers.get_or_create_layer("public.background"),
            ) {
                let mut background = norad::Glyph::new(name.as_str());
                background.width = source.width;
                background.contours = source.contours.clone();
                layer.insert_glyph(background);
                font.dirty = true;
            }
        }
        self.status_note = Some("Sent to background".into());
    }

    /// Exchange the outline with the background layer's copy.
    fn command_swap_background(&mut self) {
        let Mode::Editor(index) = self.mode else { return };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string())
        else {
            return;
        };
        self.push_undo_snapshot(index);
        let mut swapped = false;
        if let Some(font) = self.font_mut() {
            let background = Self::background_layer_name(&font.font);
            if let Some(background) = background {
                let fg = font.font.get_glyph(name.as_str()).map(|g| g.contours.clone());
                let bg = font
                    .font
                    .layers
                    .get(&background)
                    .and_then(|l| l.get_glyph(name.as_str()))
                    .map(|g| g.contours.clone());
                if let (Some(fg), Some(bg)) = (fg, bg) {
                    if let Some(layer) = font.font.layers.get_mut(&background) {
                        if let Some(g) = layer.get_glyph_mut(name.as_str()) {
                            g.contours = fg;
                        }
                    }
                    font.edit_glyph(index, |g| {
                        g.contours = bg;
                    });
                    swapped = true;
                }
            }
        }
        if !swapped {
            self.editor.undo.pop();
            self.status_note = Some("No background to swap".into());
        }
    }

    /// Drop the background layer's copy of the open glyph.
    fn command_clear_background(&mut self) {
        let Mode::Editor(index) = self.mode else { return };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string())
        else {
            return;
        };
        if let Some(font) = self.font_mut() {
            let background = Self::background_layer_name(&font.font);
            if let Some(background) = background
                && let Some(layer) = font.font.layers.get_mut(&background)
            {
                layer.remove_glyph(name.as_str());
                font.dirty = true;
            }
        }
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

    /// Curves section: comb + continuity toggles (web CurvePanel).
    fn curves_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let toggle = |id: &'static str,
                      label: &'static str,
                      active: bool,
                      cx: &mut Context<Self>,
                      on: fn(&mut Self)| {
            div()
                .id(id)
                .px_2()
                .py_0p5()
                .rounded_sm()
                .text_sm()
                .cursor_pointer()
                .border_1()
                .when(active, |el| {
                    el.border_color(t::accent()).text_color(t::accent())
                })
                .when(!active, |el| {
                    el.border_color(t::cell_border()).text_color(t::text())
                })
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    on(this);
                    cx.notify();
                }))
        };
        let body = div()
            .flex()
            .gap_1()
            .child(toggle(
                "curve-comb",
                "Curvature comb",
                self.curve_comb,
                cx,
                |this| this.curve_comb = !this.curve_comb,
            ))
            .child(toggle(
                "curve-continuity",
                "Continuity G0–G3",
                self.curve_continuity,
                cx,
                |this| this.curve_continuity = !this.curve_continuity,
            ));
        self.section(cx, "Curves", body)
    }

    /// Measure-tool HUD layer toggles (web SelectPanel): only shown
    /// while the Measure tool is active.
    /// Measure overlays. These are view options, not a tool mode: the
    /// web keeps them in the sidebar and honours them whatever tool is
    /// up, so a length stays on screen while you edit.
    fn measure_section(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let toggle = |id: &'static str,
                      label: &'static str,
                      active: bool,
                      cx: &mut Context<Self>,
                      on: fn(&mut MeasureOpts)| {
            div()
                .id(id)
                .px_2()
                .py_0p5()
                .rounded_sm()
                .text_sm()
                .cursor_pointer()
                .border_1()
                .when(active, |el| {
                    el.border_color(t::accent()).text_color(t::accent())
                })
                .when(!active, |el| {
                    el.border_color(t::cell_border()).text_color(t::text())
                })
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    on(&mut this.measure_opts);
                    cx.notify();
                }))
        };
        let o = self.measure_opts;
        let body = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .child(toggle("ms-colorize", "colorize outline", o.colorize, cx, |o| {
                o.colorize = !o.colorize
            }))
            .child(toggle("ms-handles", "handle lengths", o.handles, cx, |o| {
                o.handles = !o.handles
            }))
            .child(toggle("ms-segments", "segment lengths", o.segments, cx, |o| {
                o.segments = !o.segments
            }))
            .child(toggle("ms-spans", "stems & counters", o.spans, cx, |o| {
                o.spans = !o.spans
            }))
            .child(toggle(
                "ms-sidebearings",
                "side bearings",
                o.sidebearings,
                cx,
                |o| o.sidebearings = !o.sidebearings,
            ))
            .child(toggle("ms-sizes", "segment sizes", o.sizes, cx, |o| {
                o.sizes = !o.sizes
            }))
            .child(toggle("ms-popcount", "popcount sums", o.popcount, cx, |o| {
                o.popcount = !o.popcount
            }))
            // popcount is left out of all-on/all-off on purpose: it is
            // not a layer, it is how the labels that are on get written.
            .child(toggle("ms-all", "all on", false, cx, |o| {
                o.colorize = true;
                o.handles = true;
                o.segments = true;
                o.spans = true;
                o.sidebearings = true;
                o.sizes = true;
            }))
            .child(toggle("ms-none", "all off", false, cx, |o| {
                o.colorize = false;
                o.handles = false;
                o.segments = false;
                o.spans = false;
                o.sidebearings = false;
                o.sizes = false;
            }));
        Some(self.section(cx, "Measure", body))
    }

    /// Background section: show/send/swap/clear plus the reference
    /// glyph (web's Background block).
    fn background_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let button = |id: &'static str,
                      label: &'static str,
                      active: bool,
                      cx: &mut Context<Self>,
                      on: fn(&mut Self)| {
            div()
                .id(id)
                .px_2()
                .py_0p5()
                .rounded_sm()
                .text_sm()
                .cursor_pointer()
                .border_1()
                .when(active, |el| {
                    el.border_color(t::accent()).text_color(t::accent())
                })
                .when(!active, |el| {
                    el.border_color(t::cell_border()).text_color(t::text())
                })
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    on(this);
                    cx.notify();
                }))
        };
        let body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .child(button(
                        "bg-show",
                        "Show background",
                        self.show_background,
                        cx,
                        |this| this.show_background = !this.show_background,
                    ))
                    .child(button(
                        "bg-send",
                        "Send to background",
                        false,
                        cx,
                        |this| this.command_send_to_background(),
                    ))
                    .child(button("bg-swap", "Swap", false, cx, |this| {
                        this.command_swap_background()
                    }))
                    .child(button("bg-clear", "Clear", false, cx, |this| {
                        this.command_clear_background()
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(t::text_muted())
                            .child("Reference"),
                    )
                    .child(div().flex_1().child(
                        gpui_component::input::Input::new(
                            &self.reference_glyph_input,
                        ),
                    )),
            );
        self.section(cx, "Background", body)
    }

    /// Layers section: one row per master, the active one highlighted.
    fn layers_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let (names, active): (Vec<SharedString>, usize) = match &self.project {
            Some(p) => (p.master_names.clone(), p.active),
            None => (Vec::new(), 0),
        };
        let reference = self.reference_layers.clone();
        // A thumbnail of the current glyph in each master, the web
        // MasterToolbar's glyph buttons relocated into this section.
        let glyph_name: Option<String> = self
            .selected
            .and_then(|i| self.font().map(|f| f.glyphs[i].name.to_string()));
        let thumbs: Vec<Option<(Arc<BezPath>, f64, f64, f64)>> = match (
            &self.project,
            &glyph_name,
        ) {
            (Some(p), Some(name)) => p
                .masters
                .iter()
                .map(|m| {
                    m.name_map.get(name).map(|&g| {
                        (
                            m.glyphs[g].path.clone(),
                            m.glyphs[g].advance,
                            m.ascender,
                            m.descender,
                        )
                    })
                })
                .collect(),
            _ => Vec::new(),
        };
        let rows: Vec<_> = names
            .into_iter()
            .enumerate()
            .map(|(i, name)| {
                let is_active = i == active;
                let eye_on = reference.contains(&i);
                let thumb = thumbs.get(i).cloned().flatten();
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .children(thumb.map(|(path, advance, asc, desc)| {
                        div().w(px(22.0)).h(px(22.0)).child(
                            canvas(
                                move |bounds, _, _| bounds,
                                move |_,
                                      bounds: Bounds<gpui::Pixels>,
                                      window,
                                      _| {
                                    let h: f32 = bounds.size.height.into();
                                    let w: f32 = bounds.size.width.into();
                                    let em = (asc - desc).max(1.0);
                                    let scale = (h as f64 / em)
                                        .min(w as f64 / advance.max(1.0));
                                    let ox = (w as f64
                                        - advance * scale)
                                        / 2.0;
                                    let baseline =
                                        h as f64 + desc * scale;
                                    let view = Affine::translate((
                                        ox, baseline,
                                    ))
                                        * Affine::scale_non_uniform(
                                            scale, -scale,
                                        );
                                    if let Some(p) = build_fill_path(
                                        &path,
                                        view,
                                        bounds.origin,
                                    ) {
                                        window.paint_path(
                                            p,
                                            t::glyph_fill(),
                                        );
                                    }
                                },
                            )
                            .size_full(),
                        )
                    }))
                    .child(
                        // Eye: draw this master as a dim reference
                        // underlay in the editor (Glyphs-style layer
                        // visibility). The active master is always
                        // drawn, so its eye is implicit.
                        div()
                            .id(("layer-eye", i))
                            .w(px(20.0))
                            .text_sm()
                            .cursor_pointer()
                            .text_color(if eye_on { t::text() } else { t::text_muted() })
                            .child(if is_active {
                                "●"
                            } else if eye_on {
                                "●"
                            } else {
                                "○"
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.reference_layers.remove(&i) {
                                    this.reference_layers.insert(i);
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id(("layer", i))
                            .flex_1()
                            .px_2()
                            .py_0p5()
                            .rounded_sm()
                            .text_sm()
                            .cursor_pointer()
                            .when(is_active, |el| {
                                el.bg(t::cell_selected_bg()).text_color(t::text())
                            })
                            .when(!is_active, |el| el.text_color(t::text_muted()))
                            .child(name)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.switch_master(i);
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        let body = div().flex().flex_col().children(rows);
        self.section(cx, "Masters", body)
    }

    /// Navigate section: the active master with previous/next
    /// steppers, like the Glyphs Navigate panel.
    fn navigate_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let (name, count): (SharedString, usize) = match &self.project {
            Some(p) => (p.master_names[p.active].clone(), p.masters.len()),
            None => ("—".into(), 0),
        };
        let stepper = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_2()
                .py_0p5()
                .rounded_sm()
                .border_1()
                .border_color(t::cell_border())
                .text_sm()
                .text_color(t::text())
                .cursor_pointer()
                .child(label)
        };
        let body = div()
            .flex()
            .items_center()
            .gap_2()
            .child(div().flex_1().text_sm().text_color(t::text()).child(name))
            .when(count > 1, |el| {
                el.child(stepper("nav-prev", "←").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.command_step_master(-1);
                        cx.notify();
                    },
                )))
                .child(stepper("nav-next", "→").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.command_step_master(1);
                        cx.notify();
                    },
                )))
            });
        self.section(cx, "Navigate", body)
    }

    /// The context-menu overlay, absolutely positioned inside the
    /// editor container.
    fn context_menu_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<gpui::Div>> {
        let menu = self.context_menu.as_ref()?;
        let item = |id: (&'static str, usize),
                    label: SharedString,
                    action: &'static str,
                    cx: &mut Context<Self>| {
            div()
                .id(id)
                .px_3()
                .py_1()
                .text_sm()
                .text_color(t::text())
                .cursor_pointer()
                .hover(|el| el.bg(t::cell_selected_bg()))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.context_menu_action(action);
                    cx.notify();
                }))
        };
        let mut list = div().flex().flex_col().py_1();
        // Component items first: when you right-click a component,
        // that is what you meant, and the lock is the thing you reach
        // for most while placing marks.
        match menu.component {
            Some((_, true)) => {
                list = list.child(item(
                    ("cm", 0),
                    "Unlock from Anchor".into(),
                    "unlock-component",
                    cx,
                ));
            }
            Some((_, false)) => {
                list = list.child(item(
                    ("cm", 0),
                    "Lock to Anchor".into(),
                    "lock-component",
                    cx,
                ));
            }
            None => {}
        }
        if menu.component.is_some() {
            list = list.child(item(
                ("cm", 1),
                "Decompose Component".into(),
                "decompose-component",
                cx,
            ));
        } else if menu.has_components {
            list = list.child(item(
                ("cm", 1),
                "Decompose Components".into(),
                "decompose-all",
                cx,
            ));
        }
        if menu.adding_component {
            list = list.child(
                div()
                    .px_3()
                    .py_1()
                    .w(px(180.0))
                    .child(gpui_component::input::Input::new(
                        &self.component_name_input,
                    )),
            );
        } else {
            list = list.child(item(
                ("cm", 2),
                "Add Component…".into(),
                "add-component",
                cx,
            ));
        }
        if menu.start_point.is_some() {
            list = list.child(item(
                ("cm", 3),
                "Set Start Point".into(),
                "set-start",
                cx,
            ));
        }
        if menu.contour.is_some() {
            list = list.child(item(
                ("cm", 4),
                "Reverse Contour".into(),
                "reverse",
                cx,
            ));
        }
        if !self.editor.selected.is_empty() {
            list = list.child(item(
                ("cm", 5),
                "Round Corners".into(),
                "round-corners",
                cx,
            ));
        }
        if let Some(ci) = menu.contour {
            if ci > 0 {
                list = list.child(item(
                    ("cm", 6),
                    format!("Move Contour Up ({ci} → {})", ci - 1).into(),
                    "move-up",
                    cx,
                ));
            }
            if ci + 1 < menu.contour_count {
                list = list.child(item(
                    ("cm", 7),
                    format!("Move Contour Down ({ci} → {})", ci + 1).into(),
                    "move-down",
                    cx,
                ));
            }
        }
        list = list.child(item(
            ("cm", 8),
            "Add Anchor Here".into(),
            "add-anchor",
            cx,
        ));
        if menu.anchor.is_some() {
            list = list.child(item(
                ("cm", 9),
                "Delete Anchor".into(),
                "delete-anchor",
                cx,
            ));
        }
        Some(
            div()
                .id("context-menu")
                .absolute()
                .left(menu.at.x)
                .top(menu.at.y)
                // Clicks inside the menu must not reach the canvas:
                // its mouse-down would dismiss the menu before the
                // item's click fires (and start a marquee besides).
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _, _, cx| {
                        cx.stop_propagation();
                    }),
                )
                .bg(t::panel_bg())
                .border_1()
                .border_color(t::panel_outline())
                .rounded_md()
                .shadow_md()
                .min_w(px(180.0))
                .child(list),
        )
    }

    fn editor_view(&self, index: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let font = self.font().unwrap();
        let entry = &font.glyphs[index];
        let outline = entry.contour_path.clone();
        let component_path = entry.component_path.clone();
        let component_names = entry.component_names.clone();
        // The text buffer, web-style: every sort's fill (the active
        // one too while the text tool is up), its quiet metric box,
        // corner marks (kern-colored during a kern drag), and the
        // caret. Coordinates are relative to the active sort.
        struct SortPaint {
            path: Option<Arc<BezPath>>,
            x: f64,
            y: f64,
            advance: f64,
            active: bool,
            /// 0 = normal, 1 = kern-active, 2 = kern-previous.
            kern: u8,
        }
        let text_mode = self.editor.tool == Tool::Text;
        let (sort_paints, text_caret): (Vec<SortPaint>, Option<(f64, f64)>) = {
            let line_height = self.text_line_height();
            let layout = self.edit_buffer.layout(line_height);
            let active = self.edit_buffer.active_sort();
            let kern_sort = self.edit_buffer.manual_kerning_sort();
            let off = self.editor.sort_offset;
            let paints = layout
                .items
                .iter()
                .filter_map(|item| {
                    let sort = self.edit_buffer.sort(item.index)?;
                    if sort.is_absorbed() {
                        return None;
                    }
                    let is_active = Some(item.index) == active;
                    let path = sort
                        .glyph_name()
                        .and_then(|n| font.name_map.get(n))
                        .map(|&g| font.glyphs[g].path.clone());
                    Some(SortPaint {
                        path,
                        x: item.x - off.0,
                        y: item.y - off.1,
                        advance: item.advance_width,
                        active: is_active,
                        kern: match kern_sort {
                            Some(k) if k == item.index => 1,
                            Some(k) if k == item.index + 1 => 2,
                            _ => 0,
                        },
                    })
                })
                .collect();
            let caret = text_mode
                .then(|| (layout.cursor_x - off.0, layout.cursor_y - off.1));
            (paints, caret)
        };
        let (sort_top, sort_bottom) = self.text_sort_bounds();

        // Masters toggled visible in the Layers section, drawn as dim
        // reference underlays.
        let reference_paths: Vec<Arc<BezPath>> = self
            .project
            .as_ref()
            .map(|p| {
                self.reference_layers
                    .iter()
                    .filter(|&&i| i != p.active && i < p.masters.len())
                    .filter_map(|&i| {
                        p.masters[i]
                            .glyphs
                            .iter()
                            .find(|g| g.name == entry.name)
                            .map(|g| g.path.clone())
                    })
                    .collect()
            })
            .unwrap_or_default();
        let ghost: Option<Arc<BezPath>> = self
            .project
            .as_ref()
            .and_then(|p| p.interpolated_glyph(entry.name.as_ref()))
            .map(|(path, _)| Arc::new(path));
        let points = entry.points.clone();
        // Where each closed contour starts and which way it runs, for
        // the start arrow. Open contours (pen paths in progress) get
        // none, like the web.
        let start_markers: Vec<((f64, f64), (f64, f64), bool)> = font
            .font
            .get_glyph(entry.name.as_ref())
            .map(|g| {
                g.contours
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| {
                        c.points
                            .first()
                            .is_none_or(|p| p.typ != norad::PointType::Move)
                    })
                    .filter_map(|(ci, _)| {
                        let mut here = entry
                            .points
                            .iter()
                            .filter(|p| p.contour == ci)
                            .peekable();
                        let all: Vec<&GlyphPoint> = here.by_ref().collect();
                        let first = all.iter().position(|p| p.on_curve)?;
                        let start = all[first];
                        let next = all[(first + 1) % all.len()];
                        Some((
                            (start.x, start.y),
                            (next.x, next.y),
                            self.editor
                                .selected
                                .contains(&(start.contour, start.index)),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let anchors = entry.anchors.clone();
        let selected_anchors = self.editor.selected_anchors.clone();
        let advance = entry.advance;
        let ascender = font.ascender;
        let descender = font.descender;
        let upm = font.units_per_em;
        let x_height = font.x_height;
        let cap_height = font.cap_height;
        // The metric box runs to the upm when that is higher than the
        // ascender, so an icon font's full em still reads as its space
        // (web `glyph_metric_bounds`).
        let box_top = upm.max(ascender);
        let box_bottom = descender;

        let transform = self.editor.transform();
        let zoom = self.editor.zoom();
        let selected_points = self.editor.selected.clone();
        let marquee = match &self.editor.drag {
            Some(Drag::Marquee { start, current, .. }) => Some((*start, *current)),
            _ => None,
        };
        let shape_preview = match &self.editor.drag {
            Some(Drag::Shape { start, current }) => {
                Some((*start, *current, self.editor.shape_ellipse))
            }
            _ => None,
        };
        let measure_line = match &self.editor.drag {
            Some(Drag::Measure { start, current }) => Some((*start, *current)),
            _ => None,
        };
        // Curve overlays: comb strips and continuity rings, computed
        // in design space from the shared analyses in core.
        let comb_strips: Vec<Vec<runebender_core::curve::CombSample>> =
            if self.curve_comb && self.editor.tool != Tool::Preview {
                font.font
                    .get_glyph(entry.name.as_ref())
                    .map(|g| {
                        let cubics =
                            runebender_core::curve::cubics_from_norad(g);
                        let maxk =
                            runebender_core::curve::max_curvature(&cubics);
                        if maxk <= 1e-12 {
                            (Vec::new(), 0.0)
                        } else {
                            (
                                runebender_core::curve::curvature_comb(
                                    &cubics,
                                    1.0,
                                    74.0 / maxk,
                                    false,
                                    16,
                                ),
                                maxk,
                            )
                        }
                    })
                    .map(|(strips, _)| strips)
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
        let comb_maxk: f64 = comb_strips
            .iter()
            .flat_map(|s| s.iter())
            .map(|s| s.kappa.abs())
            .fold(0.0, f64::max);
        let continuity_rings: Vec<(kurbo::Point, gpui::Rgba)> =
            if self.curve_continuity && self.editor.tool != Tool::Preview {
                font.font
                    .get_glyph(entry.name.as_ref())
                    .map(|g| {
                        let cubics =
                            runebender_core::curve::cubics_from_norad(g);
                        runebender_core::curve::node_continuity(&cubics)
                            .into_iter()
                            .filter_map(|nc| {
                                use runebender_core::curve::GLevel;
                                let color = match nc.level {
                                    GLevel::Corner => return None,
                                    GLevel::G2 | GLevel::G3 => {
                                        t::continuity_g2()
                                    }
                                    GLevel::G1 => t::continuity_g1(),
                                    GLevel::G1Line => t::continuity_line(),
                                    GLevel::Kink => t::continuity_kink(),
                                };
                                Some((nc.at, color))
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
        // Measure-tool HUD: colorized strokes, measurements, and side
        // bearings from core's measure module, in design space. The
        // paint closure maps them to the screen and draws dimension
        // lines + labels.
        let measure_opts = self.measure_opts;
        // Every segment's own bounding box, for the size labels.
        let segment_boxes: Vec<kurbo::Rect> = if self.measure_opts.sizes {
            use kurbo::Shape as _;
            font.font
                .get_glyph(entry.name.as_ref())
                .map(|g| {
                    runebender_core::segment_ops::segments(g)
                        .into_iter()
                        .map(|hit| hit.seg.bounding_box())
                        .filter(|b| b.width() >= 1.0 || b.height() >= 1.0)
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let measure_hud: Option<(
            Vec<runebender_core::measure::ColoredStroke>,
            Vec<runebender_core::measure::Measurement>,
            Option<runebender_core::measure::SideBearings>,
        )> = if measure_opts.any() && self.editor.tool != Tool::Preview {
            font.font.get_glyph(entry.name.as_ref()).map(|g| {
                use runebender_core::measure;
                use runebender_core::model::workspace::Contour as WContour;
                let paths: Vec<runebender_core::path::Path> = g
                    .contours
                    .iter()
                    .map(|c| {
                        runebender_core::path::Path::from_contour(
                            &WContour::from_norad(c),
                        )
                    })
                    .collect();
                let strokes = if measure_opts.colorize {
                    measure::colored_strokes(&paths)
                } else {
                    Vec::new()
                };
                let measurements = if measure_opts.handles
                    || measure_opts.segments
                    || measure_opts.spans
                {
                    measure::glyph_measurements(&paths)
                } else {
                    Vec::new()
                };
                let sb = (measure_opts.sidebearings && g.width > 0.0)
                    .then(|| measure::side_bearings(&paths, g.width))
                    .flatten();
                (strokes, measurements, sb)
            })
        } else {
            None
        };
        // Background layer outline + reference glyph ghost.
        let background_path: Option<Arc<BezPath>> = self
            .show_background
            .then(|| {
                Self::background_layer_name(&font.font).and_then(|layer| {
                    font.font
                        .layers
                        .get(&layer)
                        .and_then(|l| l.get_glyph(entry.name.as_ref()))
                        .map(|g| {
                            Arc::new(
                                runebender_core::glyph_paths::contours_to_bezpath(g),
                            )
                        })
                })
            })
            .flatten();
        let reference_path: Option<Arc<BezPath>> = self
            .reference_glyph
            .as_ref()
            .and_then(|name| font.name_map.get(name))
            .map(|&g| font.glyphs[g].path.clone());
        // Alt-hover segment highlight (select tool).
        let hover_seg = self.editor.segment_hover;
        // Sidebearing edge under the pointer (or mid-drag).
        let sidebearing_hover = self.editor.sidebearing_hover.or(match &self.editor.drag {
            Some(Drag::Sidebearing { right, .. }) => Some(*right),
            _ => None,
        });
        let component_selected = self.editor.selected_component.is_some();
        // Pen rubber band: last on-curve of the open contour to the
        // pointer, with a ring on the start point when close would
        // land (web PenPreview).
        let pen_preview: Option<((f64, f64), (f64, f64), Option<(f64, f64)>)> = (|| {
            let contour = self
                .editor
                .pen
                .as_ref()
                .map(|p| p.contour)
                .or(self.editor.hyper_contour)?;
            let pointer = self.editor.pointer?;
            let (px_, py_) = self.editor.window_to_design(pointer);
            let glyph = font.font.get_glyph(entry.name.as_ref())?;
            let points = &glyph.contours.get(contour)?.points;
            let last = points.iter().rev().find(|p| {
                p.typ != norad::PointType::OffCurve
            })?;
            let start = points.first()?;
            let close_radius = HIT_RADIUS_PX / self.editor.zoom();
            let close = (points.len() >= 3
                && ((start.x - px_).powi(2) + (start.y - py_).powi(2)).sqrt()
                    <= close_radius)
                .then_some((start.x, start.y));
            Some(((last.x, last.y), (px_, py_), close))
        })();

        // Knife drag: the cut line plus its contour intersections.
        let knife_line: Option<((f64, f64), (f64, f64), Vec<kurbo::Point>)> =
            match &self.editor.drag {
                Some(Drag::Knife { start, current }) => {
                    let hits = font
                        .font
                        .get_glyph(entry.name.as_ref())
                        .map(|g| {
                            runebender_core::knife::knife_hit_points(
                                g,
                                kurbo::Point::new(start.0, start.1),
                                kurbo::Point::new(current.0, current.1),
                            )
                        })
                        .unwrap_or_default();
                    Some((*start, *current, hits))
                }
                _ => None,
            };
        let preview_mode = self.editor.tool == Tool::Preview;
        let bounds_slot = self.editor.bounds.clone();
        let needs_fit = !self.editor.initialized;

        div()
            .flex_1()
            .relative()
            .children(self.context_menu_overlay(cx))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    this.editor_mouse_down(
                        event.position,
                        event.modifiers.shift,
                        event.modifiers.alt,
                        event.click_count,
                    );
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |this, event: &gpui::MouseMoveEvent, _, cx| {
                if event.pressed_button == Some(MouseButton::Left) {
                    if this.editor_mouse_drag(
                        event.position,
                        event.modifiers.shift,
                        event.modifiers.alt,
                    ) {
                        cx.notify();
                    }
                } else if this.editor_hover(event.position, event.modifiers.alt) {
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _: &gpui::MouseUpEvent, _, cx| {
                    this.editor_mouse_up();
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    this.editor_context_menu(event.position);
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
                        // Everything the editor draws is clipped to
                        // the canvas: without a mask the outline and
                        // the neighbouring sorts paint straight over
                        // the header and the panels beside it.
                        window.with_content_mask(
                            Some(gpui::ContentMask { bounds }),
                            move |window| {
                            let mut transform = transform;
                            let mut zoom = zoom;
                            if needs_fit {
                                // First paint after opening: fit the glyph.
                                // Recompute locally; the entity state is
                                // fitted on the next mouse interaction via
                                // the same bounds slot.
                                let h: f32 = bounds.size.height.into();
                                let w: f32 = bounds.size.width.into();
                                let mut vp = ViewPort::new();
                                vp.fit_to_canvas(
                                    w as f64,
                                    h as f64,
                                    advance,
                                    ascender,
                                    descender,
                                    0.62,
                                );
                                transform = vp.affine();
                                zoom = vp.zoom;
                            }
                            let _ = cx;
                            let origin = bounds.origin;
                            let to_screen = |x: f64, y: f64| {
                                let p = transform * kurbo::Point::new(x, y);
                                gpui::point(origin.x + px(p.x as f32), origin.y + px(p.y as f32))
                            };

                            // Zoom-dependent design grid behind everything
                            // (web draw_design_grid): the 8-unit lattice
                            // fades in past 0.8x, and past 8x a 2-unit fine
                            // grid joins underneath — the 8s stay one grid
                            // at every zoom. Anchored at the active sort's
                            // origin (our design space is sort-relative),
                            // so the baseline lands on a gridline.
                            let smoothstep = |t: f64| t * t * (3.0 - 2.0 * t);
                            let grid_mid_alpha =
                                smoothstep(((zoom - 0.8) / 0.8).clamp(0.0, 1.0));
                            let grid_close_alpha =
                                smoothstep(((zoom - 8.0) / 8.0).clamp(0.0, 1.0));
                            if !preview_mode && grid_mid_alpha > 0.0 {
                                let inv = transform.inverse();
                                let bw: f32 = bounds.size.width.into();
                                let bh: f32 = bounds.size.height.into();
                                let c0 = inv * kurbo::Point::new(0.0, 0.0);
                                let c1 = inv * kurbo::Point::new(bw as f64, bh as f64);
                                let (min_x, max_x) = (c0.x.min(c1.x), c0.x.max(c1.x));
                                let (min_y, max_y) = (c0.y.min(c1.y), c0.y.max(c1.y));
                                let level = |spacing: f64,
                                                 skip_every: u64,
                                                 width_px: f32,
                                                 color: gpui::Rgba,
                                                 window: &mut Window| {
                                    let mut pb = PathBuilder::stroke(px(width_px));
                                    for ix in (min_x / spacing).floor() as i64
                                        ..=(max_x / spacing).ceil() as i64
                                    {
                                        if skip_every > 0
                                            && ix.unsigned_abs() % skip_every == 0
                                        {
                                            continue;
                                        }
                                        let x = ix as f64 * spacing;
                                        pb.move_to(to_screen(x, min_y));
                                        pb.line_to(to_screen(x, max_y));
                                    }
                                    for iy in (min_y / spacing).floor() as i64
                                        ..=(max_y / spacing).ceil() as i64
                                    {
                                        if skip_every > 0
                                            && iy.unsigned_abs() % skip_every == 0
                                        {
                                            continue;
                                        }
                                        let y = iy as f64 * spacing;
                                        pb.move_to(to_screen(min_x, y));
                                        pb.line_to(to_screen(max_x, y));
                                    }
                                    if let Ok(p) = pb.build() {
                                        window.paint_path(p, color);
                                    }
                                };
                                level(
                                    8.0,
                                    0,
                                    1.0,
                                    t::design_grid_coarse(grid_mid_alpha as f32),
                                    window,
                                );
                                let close_alpha =
                                    smoothstep(((zoom - 8.0) / 8.0).clamp(0.0, 1.0));
                                if close_alpha > 0.0 {
                                    // The 2s only; every 4th line is an 8
                                    // the mid pass already drew.
                                    level(
                                        2.0,
                                        4,
                                        0.5,
                                        t::design_grid_fine(close_alpha as f32),
                                        window,
                                    );
                                }
                            }

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
                            if !text_mode {
                                // Every guide the font defines, the way
                                // the web draws them: the baseline
                                // always, then the box edges, the upm,
                                // ascender, descender, x-height and
                                // cap-height, deduplicated.
                                let mut ys = vec![0.0, box_top, box_bottom, upm, ascender, descender];
                                ys.extend(x_height);
                                ys.extend(cap_height);
                                ys.retain(|y: &f64| y.is_finite());
                                ys.sort_by(|a, b| a.total_cmp(b));
                                ys.dedup_by(|a, b| (*a - *b).abs() < 0.001);
                                for y in ys {
                                    hline(y, window);
                                }
                                for (right, x) in [(false, 0.0), (true, advance)] {
                                    let hovered = sidebearing_hover == Some(right);
                                    let a = to_screen(x, box_top);
                                    let b = to_screen(x, box_bottom);
                                    let (grow_l, grow_r) =
                                        if hovered { (1.0, 2.0) } else { (0.0, 1.0) };
                                    window.paint_quad(gpui::fill(
                                        Bounds::from_corners(
                                            gpui::point(a.x - px(grow_l), a.y),
                                            gpui::point(a.x + px(grow_r), b.y),
                                        ),
                                        if hovered {
                                            t::text_cursor()
                                        } else {
                                            t::metrics_line()
                                        },
                                    ));
                                }
                            }

                            // Space-hold preview: the filled glyph and
                            // nothing else on top of it.
                            if preview_mode {
                                let mut combined = outline.as_ref().clone();
                                combined.extend(component_path.elements().iter().cloned());
                                if let Some(p) =
                                    build_fill_path(&combined, transform, origin)
                                {
                                    window.paint_path(p, t::text());
                                }
                            }

                            // The text buffer, web-style. Quiet metric
                            // boxes first so marks and fills sit on top.
                            let zoom_now = zoom;
                            let sort_h_px =
                                ((sort_top - sort_bottom).max(1.0) * zoom_now).max(1.0);
                            let mark = (sort_h_px * 0.05).clamp(1.5, 24.0);
                            let marks_visible = mark >= 3.0;
                            let line = |a: Point<gpui::Pixels>,
                                            b: Point<gpui::Pixels>,
                                            color: gpui::Rgba,
                                            window: &mut Window| {
                                let mut pb = PathBuilder::stroke(px(1.0));
                                pb.move_to(a);
                                pb.line_to(b);
                                if let Ok(p) = pb.build() {
                                    window.paint_path(p, color);
                                }
                            };
                            if !preview_mode && marks_visible {
                                for sp in sort_paints.iter() {
                                    // Quiet full box for the sorts nobody is
                                    // editing (the active one draws its own
                                    // metrics outside text mode).
                                    if !sp.active {
                                        let color = t::metric_quiet();
                                        for ex in [sp.x, sp.x + sp.advance] {
                                            line(
                                                to_screen(ex, sp.y + sort_bottom),
                                                to_screen(ex, sp.y + sort_top),
                                                color,
                                                window,
                                            );
                                        }
                                        for my in [sort_bottom, 0.0, ascender, sort_top] {
                                            line(
                                                to_screen(sp.x, sp.y + my),
                                                to_screen(sp.x + sp.advance, sp.y + my),
                                                color,
                                                window,
                                            );
                                        }
                                    }
                                    // Corner marks: inward ticks at each
                                    // metric height on both edges, clipped
                                    // to the box. Skipped for the active
                                    // sort outside text mode (it has the
                                    // full green box instead).
                                    if sp.active && !text_mode {
                                        continue;
                                    }
                                    let color = match sp.kern {
                                        1 => t::kern_active(),
                                        2 => t::kern_previous(),
                                        _ => t::metrics_line(),
                                    };
                                    let ca = to_screen(sp.x, sp.y + sort_bottom);
                                    let cb =
                                        to_screen(sp.x + sp.advance, sp.y + sort_top);
                                    let (left, right) =
                                        (ca.x.min(cb.x), ca.x.max(cb.x));
                                    let (top_px, bottom_px) =
                                        (ca.y.min(cb.y), ca.y.max(cb.y));
                                    let mark_px = px(mark as f32);
                                    for ex in [sp.x, sp.x + sp.advance] {
                                        for my in [sort_bottom, 0.0, ascender, sort_top]
                                        {
                                            let c = to_screen(ex, sp.y + my);
                                            let x0 = (c.x - mark_px).max(left);
                                            let x1 = (c.x + mark_px).min(right);
                                            if x1 > x0 {
                                                line(
                                                    gpui::point(x0, c.y),
                                                    gpui::point(x1, c.y),
                                                    color,
                                                    window,
                                                );
                                            }
                                            let y0 = (c.y - mark_px).max(top_px);
                                            let y1 = (c.y + mark_px).min(bottom_px);
                                            if y1 > y0 {
                                                line(
                                                    gpui::point(c.x, y0),
                                                    gpui::point(c.x, y1),
                                                    color,
                                                    window,
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                            // Sort fills: everyone but the active sort —
                            // and the active one too while the text tool
                            // is up (points return with select). Once the
                            // design grid is up (you are drawing, not
                            // reading) the neighbours thin to a 0.34 fill
                            // plus an outline with read-only grey points,
                            // the web's zoomed-in treatment.
                            let zoomed_in = !preview_mode && zoom > 0.8;
                            // The web's point_scale curve, simplified to
                            // its zoom ramps (device scale is 1 here).
                            let point_scale = if zoom <= 0.8 {
                                0.72 + (1.0 - 0.72) * smoothstep((zoom / 0.8).clamp(0.0, 1.0))
                            } else if zoom <= 8.0 {
                                1.0 + 0.6 * smoothstep(((zoom - 0.8) / 7.2).clamp(0.0, 1.0))
                            } else {
                                1.6 + 0.8 * smoothstep(((zoom - 8.0) / 20.0).clamp(0.0, 1.0))
                            };
                            for sp in sort_paints.iter() {
                                // The active sort renders as editable
                                // chrome except in text mode, where it is
                                // a plain fill like its neighbors. The
                                // preview fill already drew it.
                                if sp.active && (!text_mode || preview_mode) {
                                    continue;
                                }
                                let Some(path) = sp.path.as_ref() else {
                                    continue;
                                };
                                let dim = zoomed_in && !sp.active;
                                let sort_transform =
                                    transform * Affine::translate((sp.x, sp.y));
                                if let Some(p) =
                                    build_fill_path(path, sort_transform, origin)
                                {
                                    let mut fill = t::glyph_fill();
                                    if dim {
                                        fill.a *= 0.34;
                                    }
                                    window.paint_path(p, fill);
                                }
                                if !dim {
                                    continue;
                                }
                                // Outline + read-only points so the
                                // neighbour reads as structure.
                                if let Some(p) = build_path(
                                    path,
                                    sort_transform,
                                    origin,
                                    PathBuilder::stroke(px(1.0)),
                                ) {
                                    window.paint_path(p, t::glyph_fill());
                                }
                                use kurbo::Shape as _;
                                let on_r = 4.5 * point_scale * 0.85;
                                let off_r = 4.5 * point_scale * 0.6;
                                let screen = |pt: kurbo::Point| {
                                    let sp2 = sort_transform * pt;
                                    kurbo::Point::new(
                                        sp2.x + f64::from(f32::from(origin.x)),
                                        sp2.y + f64::from(f32::from(origin.y)),
                                    )
                                };
                                let mut dots = BezPath::new();
                                let mut handles = PathBuilder::stroke(px(1.0));
                                let mut any_handles = false;
                                let mut current = kurbo::Point::ZERO;
                                let mut start = kurbo::Point::ZERO;
                                let hline2 = |a: kurbo::Point,
                                                  b: kurbo::Point,
                                                  pb: &mut PathBuilder,
                                                  any: &mut bool| {
                                    pb.move_to(gpui::point(
                                        px(a.x as f32),
                                        px(a.y as f32),
                                    ));
                                    pb.line_to(gpui::point(
                                        px(b.x as f32),
                                        px(b.y as f32),
                                    ));
                                    *any = true;
                                };
                                for el in path.elements() {
                                    match *el {
                                        kurbo::PathEl::MoveTo(p) => {
                                            let p = screen(p);
                                            dots.extend(
                                                kurbo::Circle::new(p, on_r)
                                                    .to_path(0.25),
                                            );
                                            current = p;
                                            start = p;
                                        }
                                        kurbo::PathEl::LineTo(p) => {
                                            let p = screen(p);
                                            dots.extend(
                                                kurbo::Circle::new(p, on_r)
                                                    .to_path(0.25),
                                            );
                                            current = p;
                                        }
                                        kurbo::PathEl::QuadTo(c, p) => {
                                            let (c, p) = (screen(c), screen(p));
                                            dots.extend(
                                                kurbo::Circle::new(c, off_r)
                                                    .to_path(0.25),
                                            );
                                            dots.extend(
                                                kurbo::Circle::new(p, on_r)
                                                    .to_path(0.25),
                                            );
                                            hline2(current, c, &mut handles, &mut any_handles);
                                            hline2(c, p, &mut handles, &mut any_handles);
                                            current = p;
                                        }
                                        kurbo::PathEl::CurveTo(c1, c2, p) => {
                                            let (c1, c2, p) =
                                                (screen(c1), screen(c2), screen(p));
                                            dots.extend(
                                                kurbo::Circle::new(c1, off_r)
                                                    .to_path(0.25),
                                            );
                                            dots.extend(
                                                kurbo::Circle::new(c2, off_r)
                                                    .to_path(0.25),
                                            );
                                            dots.extend(
                                                kurbo::Circle::new(p, on_r)
                                                    .to_path(0.25),
                                            );
                                            hline2(current, c1, &mut handles, &mut any_handles);
                                            hline2(c2, p, &mut handles, &mut any_handles);
                                            current = p;
                                        }
                                        kurbo::PathEl::ClosePath => {
                                            current = start;
                                        }
                                    }
                                }
                                if any_handles && let Ok(p) = handles.build() {
                                    window.paint_path(p, t::point_readonly());
                                }
                                if let Some(p) = build_fill_path(
                                    &dots,
                                    Affine::IDENTITY,
                                    gpui::point(px(0.0), px(0.0)),
                                ) {
                                    window.paint_path(p, t::point_inner());
                                }
                                if let Some(p) = build_path(
                                    &dots,
                                    Affine::IDENTITY,
                                    gpui::point(px(0.0), px(0.0)),
                                    PathBuilder::stroke(px(1.0)),
                                ) {
                                    window.paint_path(p, t::point_readonly());
                                }
                            }
                            // Caret: line plus inward triangles, sized off
                            // the sort's on-screen height like the web.
                            if let Some((cx_, cy)) = text_caret {
                                let top = to_screen(cx_, cy + sort_top);
                                let bottom = to_screen(cx_, cy + sort_bottom);
                                let caret_color = t::text_cursor();
                                window.paint_quad(gpui::fill(
                                    Bounds::from_corners(
                                        gpui::point(top.x - px(0.75), top.y),
                                        gpui::point(top.x + px(0.75), bottom.y),
                                    ),
                                    caret_color,
                                ));
                                let tri_scale =
                                    ((sort_h_px * 0.09).clamp(4.0, 34.0)) / 24.0;
                                let tw = px((24.0 * tri_scale) as f32);
                                let th = px((16.0 * tri_scale) as f32);
                                let mut tri = PathBuilder::fill();
                                tri.move_to(gpui::point(top.x - tw / 2.0, top.y));
                                tri.line_to(gpui::point(top.x + tw / 2.0, top.y));
                                tri.line_to(gpui::point(top.x, top.y + th));
                                if let Ok(p) = tri.build() {
                                    window.paint_path(p, caret_color);
                                }
                                let mut tri = PathBuilder::fill();
                                tri.move_to(gpui::point(bottom.x - tw / 2.0, bottom.y));
                                tri.line_to(gpui::point(bottom.x + tw / 2.0, bottom.y));
                                tri.line_to(gpui::point(bottom.x, bottom.y - th));
                                if let Ok(p) = tri.build() {
                                    window.paint_path(p, caret_color);
                                }
                            }

                            // Reference layers: other masters as dim strokes.
                            for path in &reference_paths {
                                if let Some(p) = build_path(
                                    path,
                                    transform,
                                    origin,
                                    PathBuilder::stroke(px(1.0)),
                                ) {
                                    window.paint_path(p, t::reference_layer());
                                }
                            }

                            // Components: dim distinct fill, not editable
                            // directly (Cmd+Shift+D decomposes).
                            if !component_path.elements().is_empty()
                                && let Some(p) =
                                    build_fill_path(&component_path, transform, origin)
                            {
                                let color = if component_selected {
                                    t::component_selected_fill()
                                } else {
                                    t::component_fill()
                                };
                                window.paint_path(p, color);
                            }
                            // Interpolated instance at the axes-bar
                            // location, as a ghost outline.
                            if let Some(ghost) = &ghost
                                && let Some(p) = build_path(
                                    ghost,
                                    transform,
                                    origin,
                                    PathBuilder::stroke(px(1.0)),
                                )
                            {
                                window.paint_path(p, t::ghost());
                            }
                            // Reference glyph: a ghost fill so it never
                            // reads as the background layer's outline.
                            if let Some(path) = &reference_path
                                && let Some(p) =
                                    build_fill_path(path, transform, origin)
                            {
                                let mut fill = t::glyph_fill();
                                fill.a *= 0.22;
                                window.paint_path(p, fill);
                            }
                            // Background layer: a quiet outline behind the
                            // drawing, the way Glyphs shows a background.
                            if let Some(path) = &background_path
                                && let Some(p) = build_path(
                                    path,
                                    transform,
                                    origin,
                                    PathBuilder::stroke(px(1.0)),
                                )
                            {
                                window.paint_path(p, t::metric_quiet());
                            }
                            // Curvature comb, behind the outline so points
                            // stay selectable over it.
                            for strip in &comb_strips {
                                for w in strip.windows(2) {
                                    let (s0, s1) = (&w[0], &w[1]);
                                    let mut quad = BezPath::new();
                                    quad.move_to(transform * s0.on);
                                    quad.line_to(transform * s1.on);
                                    quad.line_to(transform * s1.outer);
                                    quad.line_to(transform * s0.outer);
                                    quad.close_path();
                                    let k = if comb_maxk > 1e-12 {
                                        (s0.kappa.abs() + s1.kappa.abs()) * 0.5
                                            / comb_maxk
                                    } else {
                                        0.0
                                    };
                                    if let Some(p) = build_fill_path(
                                        &quad,
                                        Affine::IDENTITY,
                                        origin,
                                    ) {
                                        window
                                            .paint_path(p, t::comb_gradient(k));
                                    }
                                }
                            }

                            // Ghost fill under the glyph being edited: the
                            // same grey the inactive sorts use at a tenth
                            // strength, so counters read as counters
                            // without competing with the outline (web
                            // ACTIVE_GLYPH_FILL_ALPHA).
                            if !preview_mode && !text_mode {
                                let mut combined = outline.as_ref().clone();
                                combined
                                    .extend(component_path.elements().iter().cloned());
                                if let Some(p) =
                                    build_fill_path(&combined, transform, origin)
                                {
                                    let mut fill = t::glyph_fill();
                                    fill.a *= 0.16;
                                    window.paint_path(p, fill);
                                }
                            }
                            // Edit mode is a stroked outline (no fill),
                            // like the other editors.
                            if !preview_mode
                                && !text_mode
                                && let Some(path) =
                                build_path(&outline, transform, origin, PathBuilder::stroke(px(1.0)))
                            {
                                window.paint_path(path, t::path_stroke());
                            }

                            // Handle lines: each off-curve connects to its
                            // anchoring on-curve neighbor.
                            if !preview_mode && !text_mode {
                                let mut lines = PathBuilder::stroke(px(1.0));
                                let mut any_line = false;
                                for (i, p) in points.iter().enumerate() {
                                    if p.on_curve {
                                        continue;
                                    }
                                    // Neighbors within the same contour, cyclic.
                                    let contour_pts: Vec<&GlyphPoint> = points
                                        .iter()
                                        .filter(|q| q.contour == p.contour)
                                        .collect();
                                    let n = contour_pts.len();
                                    let pos = contour_pts
                                        .iter()
                                        .position(|q| q.index == p.index)
                                        .unwrap_or(0);
                                    let prev = contour_pts[(pos + n - 1) % n];
                                    let next = contour_pts[(pos + 1) % n];
                                    let anchor = if prev.on_curve {
                                        prev
                                    } else if next.on_curve {
                                        next
                                    } else {
                                        continue;
                                    };
                                    lines.move_to(to_screen(p.x, p.y));
                                    lines.line_to(to_screen(anchor.x, anchor.y));
                                    any_line = true;
                                    let _ = i;
                                }
                                if any_line && let Ok(path) = lines.build() {
                                    window.paint_path(path, t::handle_line());
                                }
                            }

                            // Points: smooth = blue circle, corner = green
                            // square, off-curve = purple circle, selection
                            // in yellow/orange — the shared palette.
                            let circle = |window: &mut Window,
                                          center: Point<gpui::Pixels>,
                                          r: f32,
                                          color: gpui::Rgba| {
                                use kurbo::Shape;
                                let cx_: f32 = center.x.into();
                                let cy_: f32 = center.y.into();
                                let shape = kurbo::Circle::new(
                                    (cx_ as f64, cy_ as f64),
                                    r as f64,
                                )
                                .to_path(0.25);
                                if let Some(p) = build_fill_path(
                                    &shape,
                                    Affine::IDENTITY,
                                    gpui::point(px(0.0), px(0.0)),
                                ) {
                                    window.paint_path(p, color);
                                }
                            };
                            // A point is a dark window with a coloured
                            // ring, the web's recipe: a halo casing so
                            // it keeps an edge over the outline and the
                            // comb, an interior fill that masks what
                            // runs underneath, then a constant-width
                            // ring on top. Selected points fill yellow
                            // and ring in the selection colour.
                            let ps = point_scale as f32;
                            let ring_w = (1.5 * ps).max(1.0);
                            let halo_w = ring_w + 2.0;
                            let shape = |center: Point<gpui::Pixels>,
                                         r: f32,
                                         square: bool|
                             -> kurbo::BezPath {
                                use kurbo::Shape as _;
                                let (cx_, cy_) =
                                    (f32::from(center.x) as f64, f32::from(center.y) as f64);
                                if square {
                                    kurbo::Rect::new(
                                        cx_ - r as f64,
                                        cy_ - r as f64,
                                        cx_ + r as f64,
                                        cy_ + r as f64,
                                    )
                                    .to_path(0.1)
                                } else {
                                    kurbo::Circle::new((cx_, cy_), r as f64).to_path(0.15)
                                }
                            };
                            let zero = gpui::point(px(0.0), px(0.0));
                            for p in points.iter() {
                                if preview_mode || text_mode {
                                    break;
                                }
                                let center = to_screen(p.x, p.y);
                                let is_selected =
                                    selected_points.contains(&(p.contour, p.index));
                                let (ring, inner) = if is_selected {
                                    (t::point_selected_ring(), t::point_selected())
                                } else if p.hyper {
                                    (t::point_hyper_outer(), t::point_inner())
                                } else if !p.on_curve {
                                    (t::point_offcurve_outer(), t::point_inner())
                                } else if p.smooth {
                                    (t::point_smooth_outer(), t::point_inner())
                                } else {
                                    (t::point_corner_outer(), t::point_inner())
                                };
                                let is_square = p.on_curve && !p.smooth && !p.hyper;
                                let r = if p.hyper && p.on_curve {
                                    if is_selected { 5.0 } else { 4.0 }
                                } else if is_square {
                                    if is_selected { 4.5 } else { 3.5 }
                                } else if is_selected {
                                    5.5
                                } else {
                                    4.5
                                } * ps;
                                let path = shape(center, r, is_square);
                                if let Some(p) = build_path(
                                    &path,
                                    Affine::IDENTITY,
                                    zero,
                                    PathBuilder::stroke(px(halo_w)),
                                ) {
                                    window.paint_path(p, t::halo());
                                }
                                if let Some(p) =
                                    build_fill_path(&path, Affine::IDENTITY, zero)
                                {
                                    window.paint_path(p, inner);
                                }
                                // The point is a window onto the design
                                // grid: the gridlines that cross it are
                                // redrawn inside, tinted with the
                                // point's own colour, so you can read
                                // where it sits (web draws this by
                                // clipping the grid to the point; gpui
                                // masks rectangles only, so the chords
                                // are solved instead — exact, and it
                                // costs a few lines per point).
                                if grid_mid_alpha > 0.0 && !preview_mode && !text_mode {
                                    let (cx_, cy_) = (
                                        f32::from(center.x) as f64,
                                        f32::from(center.y) as f64,
                                    );
                                    let r = r as f64;
                                    let inv = transform.inverse();
                                    for (spacing, alpha, wide) in [
                                        (8.0_f64, grid_mid_alpha, 1.0_f32),
                                        (2.0, grid_close_alpha, 0.7),
                                    ] {
                                        if alpha <= 0.0 {
                                            continue;
                                        }
                                        let mut tint = ring;
                                        tint.a = alpha as f32;
                                        let mut lines = PathBuilder::stroke(px(wide));
                                        let mut any = false;
                                        // Vertical gridlines: the chord
                                        // is the circle's half-height at
                                        // that offset (the full radius
                                        // for a square point).
                                        let a = (inv * kurbo::Point::new(cx_ - r, cy_)).x;
                                        let b = (inv * kurbo::Point::new(cx_ + r, cy_)).x;
                                        let (lo, hi) = (a.min(b), a.max(b));
                                        for k in (lo / spacing).ceil() as i64
                                            ..=(hi / spacing).floor() as i64
                                        {
                                            let sx = (transform
                                                * kurbo::Point::new(
                                                    k as f64 * spacing,
                                                    0.0,
                                                ))
                                            .x;
                                            let d = sx - cx_;
                                            let half = if is_square {
                                                r
                                            } else {
                                                (r * r - d * d).max(0.0).sqrt()
                                            };
                                            if half <= 0.2 {
                                                continue;
                                            }
                                            lines.move_to(gpui::point(
                                                px(sx as f32),
                                                px((cy_ - half) as f32),
                                            ));
                                            lines.line_to(gpui::point(
                                                px(sx as f32),
                                                px((cy_ + half) as f32),
                                            ));
                                            any = true;
                                        }
                                        let a = (inv * kurbo::Point::new(cx_, cy_ - r)).y;
                                        let b = (inv * kurbo::Point::new(cx_, cy_ + r)).y;
                                        let (lo, hi) = (a.min(b), a.max(b));
                                        for k in (lo / spacing).ceil() as i64
                                            ..=(hi / spacing).floor() as i64
                                        {
                                            let sy = (transform
                                                * kurbo::Point::new(
                                                    0.0,
                                                    k as f64 * spacing,
                                                ))
                                            .y;
                                            let d = sy - cy_;
                                            let half = if is_square {
                                                r
                                            } else {
                                                (r * r - d * d).max(0.0).sqrt()
                                            };
                                            if half <= 0.2 {
                                                continue;
                                            }
                                            lines.move_to(gpui::point(
                                                px((cx_ - half) as f32),
                                                px(sy as f32),
                                            ));
                                            lines.line_to(gpui::point(
                                                px((cx_ + half) as f32),
                                                px(sy as f32),
                                            ));
                                            any = true;
                                        }
                                        if any && let Ok(p) = lines.build() {
                                            window.paint_path(p, tint);
                                        }
                                    }
                                }
                                if let Some(p) = build_path(
                                    &path,
                                    Affine::IDENTITY,
                                    zero,
                                    PathBuilder::stroke(px(ring_w)),
                                ) {
                                    window.paint_path(p, ring);
                                }
                            }
                            // Start-of-contour arrow: which point a closed
                            // contour begins at, and which way it runs
                            // (web draw_start_arrow).
                            if !preview_mode && !text_mode {
                                for start in start_markers.iter() {
                                    let (from, to, selected) = *start;
                                    let a = to_screen(from.0, from.1);
                                    let b = to_screen(to.0, to.1);
                                    let size =
                                        (if selected { 6.5 } else { 5.5 }) * ps;
                                    let dir = (
                                        f32::from(b.x - a.x),
                                        f32::from(b.y - a.y),
                                    );
                                    let len = (dir.0 * dir.0 + dir.1 * dir.1).sqrt();
                                    if len < 0.001 {
                                        continue;
                                    }
                                    let f = (dir.0 / len, dir.1 / len);
                                    let perp = (-f.1, f.0);
                                    let cx_ = f32::from(a.x) + perp.0 * 8.0 * ps;
                                    let cy_ = f32::from(a.y) + perp.1 * 8.0 * ps;
                                    let tip = (cx_ + f.0 * size, cy_ + f.1 * size);
                                    let base = (
                                        cx_ - f.0 * size * 0.5,
                                        cy_ - f.1 * size * 0.5,
                                    );
                                    let left = (
                                        base.0 + perp.0 * size * 0.5,
                                        base.1 + perp.1 * size * 0.5,
                                    );
                                    let right = (
                                        base.0 - perp.0 * size * 0.5,
                                        base.1 - perp.1 * size * 0.5,
                                    );
                                    let mut pb = PathBuilder::fill();
                                    pb.move_to(gpui::point(px(tip.0), px(tip.1)));
                                    pb.line_to(gpui::point(px(left.0), px(left.1)));
                                    pb.line_to(gpui::point(px(right.0), px(right.1)));
                                    pb.close();
                                    if let Ok(path) = pb.build() {
                                        window.paint_path(
                                            path,
                                            if selected {
                                                t::point_selected()
                                            } else {
                                                t::point_smooth_outer()
                                            },
                                        );
                                    }
                                }
                            }
                            // Anchors: diamonds (rotated squares drawn as
                            // two overlapping quads approximate; use a
                            // filled path).
                            // Anchors are diamonds built like points: a
                            // dark window with a coloured ring, sized
                            // off the smooth-point radius and widened a
                            // little so a rotated square reads as the
                            // same size (web ANCHOR_DIAMOND_SCALE).
                            for (ai, (_, ax, ay)) in anchors.iter().enumerate() {
                                if preview_mode || text_mode {
                                    break;
                                }
                                let center = to_screen(*ax, *ay);
                                let is_selected = selected_anchors.contains(&ai);
                                let r = (if is_selected { 5.5 } else { 4.5 }) * ps * 1.35;
                                let (cx_, cy_) = (
                                    f32::from(center.x) as f64,
                                    f32::from(center.y) as f64,
                                );
                                let r = r as f64;
                                let mut diamond = BezPath::new();
                                diamond.move_to((cx_, cy_ - r));
                                diamond.line_to((cx_ + r, cy_));
                                diamond.line_to((cx_, cy_ + r));
                                diamond.line_to((cx_ - r, cy_));
                                diamond.close_path();
                                let (ring, inner) = if is_selected {
                                    (t::point_selected_ring(), t::point_selected())
                                } else {
                                    (t::anchor(), t::point_inner())
                                };
                                if let Some(p) = build_path(
                                    &diamond,
                                    Affine::IDENTITY,
                                    zero,
                                    PathBuilder::stroke(px(halo_w)),
                                ) {
                                    window.paint_path(p, t::halo());
                                }
                                if let Some(p) =
                                    build_fill_path(&diamond, Affine::IDENTITY, zero)
                                {
                                    window.paint_path(p, inner);
                                }
                                if let Some(p) = build_path(
                                    &diamond,
                                    Affine::IDENTITY,
                                    zero,
                                    PathBuilder::stroke(px(ring_w)),
                                ) {
                                    window.paint_path(p, ring);
                                }
                            }

                            // Shapes-tool live preview.
                            if let Some((a, b, ellipse)) = shape_preview {
                                use kurbo::Shape as _;
                                let rect = kurbo::Rect::from_points(
                                    kurbo::Point::new(a.0, a.1),
                                    kurbo::Point::new(b.0, b.1),
                                );
                                let shape: BezPath = if ellipse {
                                    kurbo::Ellipse::from_rect(rect).to_path(0.1)
                                } else {
                                    rect.to_path(0.1)
                                };
                                if let Some(p) = build_path(
                                    &shape,
                                    transform,
                                    origin,
                                    PathBuilder::stroke(px(1.0)),
                                ) {
                                    window.paint_path(p, t::accent());
                                }
                            }
                            // Measure-tool line.
                            if let Some(seg) = hover_seg {
                                let mut pb = PathBuilder::stroke(px(3.0));
                                match seg {
                                    kurbo::PathSeg::Line(l) => {
                                        pb.move_to(to_screen(l.p0.x, l.p0.y));
                                        pb.line_to(to_screen(l.p1.x, l.p1.y));
                                    }
                                    kurbo::PathSeg::Quad(q) => {
                                        pb.move_to(to_screen(q.p0.x, q.p0.y));
                                        pb.curve_to(
                                            to_screen(q.p2.x, q.p2.y),
                                            to_screen(q.p1.x, q.p1.y),
                                        );
                                    }
                                    kurbo::PathSeg::Cubic(c) => {
                                        pb.move_to(to_screen(c.p0.x, c.p0.y));
                                        pb.cubic_bezier_to(
                                            to_screen(c.p3.x, c.p3.y),
                                            to_screen(c.p1.x, c.p1.y),
                                            to_screen(c.p2.x, c.p2.y),
                                        );
                                    }
                                }
                                if let Ok(p) = pb.build() {
                                    window.paint_path(p, t::accent());
                                }
                            }
                            if let Some(((lx, ly), (cx3, cy3), close)) = pen_preview {
                                let mut pb = PathBuilder::stroke(px(1.0));
                                pb.move_to(to_screen(lx, ly));
                                pb.line_to(to_screen(cx3, cy3));
                                if let Ok(p) = pb.build() {
                                    window.paint_path(p, t::accent());
                                }
                                if let Some((sx2, sy2)) = close {
                                    circle(
                                        window,
                                        to_screen(sx2, sy2),
                                        6.0,
                                        t::accent(),
                                    );
                                }
                            }
                            if let Some(((sx, sy), (cx2, cy2), hits)) = &knife_line {
                                let a = to_screen(*sx, *sy);
                                let b = to_screen(*cx2, *cy2);
                                let mut line = PathBuilder::stroke(px(1.0));
                                line.move_to(a);
                                line.line_to(b);
                                if let Ok(p) = line.build() {
                                    window.paint_path(p, t::anchor());
                                }
                                for hit in hits {
                                    let c = to_screen(hit.x, hit.y);
                                    circle(window, c, 3.5, t::anchor());
                                }
                            }
                            if let Some((a, b)) = measure_line {
                                let mut pb = PathBuilder::stroke(px(1.0));
                                let pa = to_screen(a.0, a.1);
                                let pbp = to_screen(b.0, b.1);
                                pb.move_to(pa);
                                pb.line_to(pbp);
                                if let Ok(p) = pb.build() {
                                    window.paint_path(p, t::accent());
                                }
                            }
                            // Measure-tool HUD (web draw_measurements):
                            // popcount-colored outline, dimension lines
                            // with outward arrowheads, and labels that
                            // dodge each other. Fades in with zoom.
                            if let Some((strokes, measurements, sb)) = &measure_hud {
                                use runebender_core::measure::{self, MeasureKind};
                                let t32 = (((zoom - 0.30) / 0.40).clamp(0.0, 1.0)) as f32;
                                if t32 > 0.0 {
                                    let fade = |mut c: gpui::Rgba, mul: f32| {
                                        c.a *= t32 * mul;
                                        c
                                    };
                                    for cs in strokes {
                                        let width = if cs.wide { 1.5 } else { 1.0 };
                                        if let Some(p) = build_path(
                                            &cs.path,
                                            transform,
                                            origin,
                                            PathBuilder::stroke(px(width)),
                                        ) {
                                            window.paint_path(
                                                p,
                                                fade(t::popcount_tier(cs.popcount), 1.0),
                                            );
                                        }
                                    }
                                    let gp = |p: kurbo::Point| {
                                        gpui::point(
                                            origin.x + px(p.x as f32),
                                            origin.y + px(p.y as f32),
                                        )
                                    };
                                    // A span's dimension line: a shaft that
                                    // stops short of both endpoints with an
                                    // outward arrowhead at each end.
                                    let dim_line = |window: &mut gpui::Window,
                                                    a: kurbo::Point,
                                                    b: kurbo::Point,
                                                    color: gpui::Rgba| {
                                        let (dx, dy) = (b.x - a.x, b.y - a.y);
                                        let len = dx.hypot(dy);
                                        if len < 1e-3 {
                                            return;
                                        }
                                        let (ux, uy) = (dx / len, dy / len);
                                        let (nx, ny) = (-uy, ux);
                                        let (end_gap, head, wing) = (3.0, 7.0, 4.0);
                                        let a2 = kurbo::Point::new(
                                            a.x + ux * end_gap,
                                            a.y + uy * end_gap,
                                        );
                                        let b2 = kurbo::Point::new(
                                            b.x - ux * end_gap,
                                            b.y - uy * end_gap,
                                        );
                                        let mut pb = PathBuilder::stroke(px(1.25));
                                        pb.move_to(gp(a2));
                                        pb.line_to(gp(b2));
                                        for (p0, sx) in [(a2, 1.0), (b2, -1.0)] {
                                            for side in [1.0, -1.0] {
                                                pb.move_to(gp(p0));
                                                pb.line_to(gp(kurbo::Point::new(
                                                    p0.x + sx * ux * head + side * nx * wing,
                                                    p0.y + sx * uy * head + side * ny * wing,
                                                )));
                                            }
                                        }
                                        if let Ok(p) = pb.build() {
                                            window.paint_path(p, color);
                                        }
                                    };
                                    // Place a label just off its line, then
                                    // step it outward (and to the other
                                    // side) until it clears every label
                                    // already placed this frame.
                                    let label_px = px(13.0);
                                    let line_h = px(15.0);
                                    let label_font = window.text_style().font();
                                    let mut placed: Vec<kurbo::Rect> = Vec::new();
                                    let draw_label =
                                        |window: &mut gpui::Window,
                                         cx: &mut gpui::App,
                                         a: kurbo::Point,
                                         b: kurbo::Point,
                                         text: String,
                                         color: gpui::Rgba,
                                         placed: &mut Vec<kurbo::Rect>| {
                                            let label_text =
                                                gpui::SharedString::from(text);
                                            let run = gpui::TextRun {
                                                len: label_text.len(),
                                                font: label_font.clone(),
                                                color: color.into(),
                                                background_color: None,
                                                underline: None,
                                                strikethrough: None,
                                            };
                                            let line = window.text_system().shape_line(
                                                label_text.clone(),
                                                label_px,
                                                std::slice::from_ref(&run),
                                                None,
                                            );
                                            let w = f32::from(line.width) as f64;
                                            let h = f32::from(line_h) as f64;
                                            let (dx, dy) = (b.x - a.x, b.y - a.y);
                                            let len = dx.hypot(dy).max(1e-6);
                                            let (mut nx, mut ny) = (-dy / len, dx / len);
                                            let horizontalish = dx.abs() >= dy.abs();
                                            if (horizontalish && ny > 0.0)
                                                || (!horizontalish && nx < 0.0)
                                            {
                                                nx = -nx;
                                                ny = -ny;
                                            }
                                            let mid = a.midpoint(b);
                                            let (base, step, pad) = (6.0, h + 4.0, 2.0);
                                            let top_left = |dirx: f64, diry: f64, dist: f64| {
                                                let cx0 = mid.x + dirx * dist;
                                                let cy0 = mid.y + diry * dist;
                                                let x = if dirx > 0.3 {
                                                    cx0
                                                } else if dirx < -0.3 {
                                                    cx0 - w
                                                } else {
                                                    cx0 - w / 2.0
                                                };
                                                let y = if diry > 0.3 {
                                                    cy0
                                                } else if diry < -0.3 {
                                                    cy0 - h
                                                } else {
                                                    cy0 - h / 2.0
                                                };
                                                kurbo::Point::new(x, y)
                                            };
                                            let mut chosen = top_left(nx, ny, base);
                                            'search: for &sign in &[1.0_f64, -1.0] {
                                                let (dirx, diry) = (nx * sign, ny * sign);
                                                for k in 0..6 {
                                                    let cand = top_left(
                                                        dirx,
                                                        diry,
                                                        base + k as f64 * step,
                                                    );
                                                    let rect = kurbo::Rect::new(
                                                        cand.x - pad,
                                                        cand.y - pad,
                                                        cand.x + w + pad,
                                                        cand.y + h + pad,
                                                    );
                                                    let clear = !placed.iter().any(|r| {
                                                        r.x0 < rect.x1
                                                            && rect.x0 < r.x1
                                                            && r.y0 < rect.y1
                                                            && rect.y0 < r.y1
                                                    });
                                                    if clear {
                                                        chosen = cand;
                                                        break 'search;
                                                    }
                                                }
                                            }
                                            placed.push(kurbo::Rect::new(
                                                chosen.x,
                                                chosen.y,
                                                chosen.x + w,
                                                chosen.y + h,
                                            ));
                                            // A casing around the
                                            // numerals, not a filled
                                            // box: the web strokes each
                                            // glyph in the halo colour
                                            // before filling it. gpui
                                            // has no stroked text, so
                                            // the line is painted eight
                                            // times around the centre
                                            // instead, which reads the
                                            // same and keeps the canvas
                                            // visible behind the label.
                                            let mut halo_color = t::halo();
                                            halo_color.a *= t32;
                                            let halo_run = gpui::TextRun {
                                                len: run.len,
                                                font: label_font.clone(),
                                                color: halo_color.into(),
                                                background_color: None,
                                                underline: None,
                                                strikethrough: None,
                                            };
                                            let halo_line =
                                                window.text_system().shape_line(
                                                    label_text.clone(),
                                                    label_px,
                                                    std::slice::from_ref(&halo_run),
                                                    None,
                                                );
                                            for (ox, oy) in [
                                                (-1.0, 0.0),
                                                (1.0, 0.0),
                                                (0.0, -1.0),
                                                (0.0, 1.0),
                                                (-1.0, -1.0),
                                                (1.0, -1.0),
                                                (-1.0, 1.0),
                                                (1.0, 1.0),
                                            ] {
                                                let _ = halo_line.paint(
                                                    gp(kurbo::Point::new(
                                                        chosen.x + ox,
                                                        chosen.y + oy,
                                                    )),
                                                    line_h,
                                                    gpui::TextAlign::Left,
                                                    None,
                                                    window,
                                                    cx,
                                                );
                                            }
                                            let _ = line.paint(
                                                gp(chosen),
                                                line_h,
                                                gpui::TextAlign::Left,
                                                None,
                                                window,
                                                cx,
                                            );
                                        };
                                    if let Some(sb) = sb {
                                        for (is_left, x, y, val) in [
                                            (true, sb.min_x, sb.y_left, sb.lsb),
                                            (false, sb.max_x, sb.y_right, sb.rsb),
                                        ] {
                                            let color = fade(
                                                t::popcount_tier(measure::popcount(val)),
                                                0.9,
                                            );
                                            let margin_x =
                                                if is_left { 0.0 } else { sb.advance };
                                            let a = transform
                                                * kurbo::Point::new(margin_x, y);
                                            let b = transform * kurbo::Point::new(x, y);
                                            dim_line(window, a, b, color);
                                            draw_label(
                                                window,
                                                cx,
                                                a,
                                                b,
                                                measure_opts.label(val),
                                                color,
                                                &mut placed,
                                            );
                                        }
                                    }
                                    for m in measurements {
                                        let show = match m.kind {
                                            MeasureKind::Handle => measure_opts.handles,
                                            MeasureKind::Segment => measure_opts.segments,
                                            MeasureKind::Horizontal
                                            | MeasureKind::Vertical => measure_opts.spans,
                                        };
                                        if !show {
                                            continue;
                                        }
                                        let a = transform * m.a;
                                        let b = transform * m.b;
                                        let color = fade(
                                            t::popcount_tier(measure::popcount(m.length)),
                                            1.0,
                                        );
                                        if matches!(
                                            m.kind,
                                            MeasureKind::Horizontal | MeasureKind::Vertical
                                        ) {
                                            dim_line(window, a, b, color);
                                        }
                                        draw_label(
                                            window,
                                            cx,
                                            a,
                                            b,
                                            measure_opts.label(m.length),
                                            color,
                                            &mut placed,
                                        );
                                    }
                                    // Segment sizes: each curve's own
                                    // box, labelled at its centre, so
                                    // the whole glyph can be read at
                                    // once instead of one selection at
                                    // a time.
                                    for b in segment_boxes.iter() {
                                        let c0 = transform
                                            * kurbo::Point::new(b.x0, b.y0);
                                        let c1 = transform
                                            * kurbo::Point::new(b.x1, b.y1);
                                        let rect = kurbo::Rect::from_points(c0, c1);
                                        let mut frame = PathBuilder::stroke(px(1.0));
                                        let corners = [
                                            (rect.x0, rect.y0),
                                            (rect.x1, rect.y0),
                                            (rect.x1, rect.y1),
                                            (rect.x0, rect.y1),
                                        ];
                                        frame.move_to(gp(kurbo::Point::new(
                                            corners[0].0,
                                            corners[0].1,
                                        )));
                                        for (x, y) in corners.iter().skip(1) {
                                            frame.line_to(gp(kurbo::Point::new(*x, *y)));
                                        }
                                        frame.line_to(gp(kurbo::Point::new(
                                            corners[0].0,
                                            corners[0].1,
                                        )));
                                        let color = fade(t::metric_quiet(), 1.0);
                                        if let Ok(p) = frame.build() {
                                            window.paint_path(p, color);
                                        }
                                        let text = format!(
                                            "{:.0}×{:.0}",
                                            b.width(),
                                            b.height()
                                        );
                                        let mid_left = kurbo::Point::new(
                                            rect.x0,
                                            rect.center().y,
                                        );
                                        let mid_right = kurbo::Point::new(
                                            rect.x1,
                                            rect.center().y,
                                        );
                                        draw_label(
                                            window,
                                            cx,
                                            mid_left,
                                            mid_right,
                                            text,
                                            fade(t::text(), 1.0),
                                            &mut placed,
                                        );
                                    }
                                }
                            }
                            // Continuity rings around on-curve nodes.
                            if !continuity_rings.is_empty() {
                                use kurbo::Shape as _;
                                let r = (4.5 * 1.9) as f64;
                                for (at, color) in &continuity_rings {
                                    let c = transform * *at;
                                    let circle = kurbo::Circle::new(c, r)
                                        .to_path(0.25);
                                    if let Some(p) = build_path(
                                        &circle,
                                        Affine::IDENTITY,
                                        origin,
                                        PathBuilder::stroke(px(1.5)),
                                    ) {
                                        window.paint_path(p, *color);
                                    }
                                }
                            }

                            // Marquee rectangle.
                            if let Some((a, b)) = marquee {
                                let pa = to_screen(a.0, a.1);
                                let pb = to_screen(b.0, b.1);
                                let rect = Bounds::from_corners(
                                    gpui::point(pa.x.min(pb.x), pa.y.min(pb.y)),
                                    gpui::point(pa.x.max(pb.x), pa.y.max(pb.y)),
                                );
                                window.paint_quad(gpui::fill(rect, t::marquee_fill()));
                                window.paint_quad(gpui::outline(
                                    rect,
                                    t::marquee_stroke(),
                                    gpui::BorderStyle::Solid,
                                ));
                            }
                            let _ = (zoom, &component_names);
                            },
                        );
                    },
                )
                .size_full(),
            )
            .child(self.editor_info_panel(index, cx))
    }

    /// The floating info panel Glyphs puts at the bottom of the edit
    /// view: the glyph's name and codepoint, its sidebearings and
    /// width, its kerning groups, and — while something is selected —
    /// the selection's position and size.
    fn editor_info_panel(
        &self,
        index: usize,
        _cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        if self.editor.tool == Tool::Preview {
            return div().into_any_element();
        }
        let Some(font) = self.font() else {
            return div().into_any_element();
        };
        let entry = &font.glyphs[index];
        let name: SharedString = entry.name.to_string().into();
        let unicode: SharedString = entry
            .codepoint
            .map(|c| format!("{:04X}", c as u32))
            .unwrap_or_default()
            .into();
        let group_l = runebender_core::glyph_ops::kern_group(
            &font.font,
            entry.name.as_ref(),
            true,
        )
        .map(|g| g.as_str().replace("public.kern1.", ""))
        .unwrap_or_default();
        let group_r = runebender_core::glyph_ops::kern_group(
            &font.font,
            entry.name.as_ref(),
            false,
        )
        .map(|g| g.as_str().replace("public.kern2.", ""))
        .unwrap_or_default();

        // One card, built on a 6px rhythm: an 8px inset on every side,
        // 6px between rows, and a header band the same height as the
        // fields under it.
        const CARD_PAD: f32 = 8.0;
        const CARD_GAP: f32 = 6.0;
        const CARD_RADIUS: f32 = 6.0;
        const HEADER_H: f32 = 22.0;
        let card = || {
            div()
                .rounded(px(CARD_RADIUS))
                .border_1()
                .border_color(t::panel_outline())
                .bg(t::panel_bg())
                .flex()
                .flex_col()
        };
        let label = |text: SharedString| {
            div()
                .text_xs()
                .text_color(t::text_muted())
                .child(text)
        };
        let metric = |input: &gpui::Entity<gpui_component::input::InputState>| {
            use gpui_component::Sizable as _;
            div()
                .w(px(64.0))
                .child(gpui_component::input::Input::new(input).small())
        };

        let metrics = card()
            .child(
                // Header: the glyph on the left, its codepoint on the
                // right. A quiet band, not a colour statement — the
                // corners follow the card's radius so nothing pokes
                // out past the border.
                div()
                    .h(px(HEADER_H))
                    .px(px(CARD_PAD))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .rounded_t(px(CARD_RADIUS - 1.0))
                    .bg(t::cell_selected_bg())
                    .border_b_1()
                    .border_color(t::panel_outline())
                    .text_sm()
                    .text_color(t::text())
                    .child(name)
                    .child(
                        div()
                            .text_color(t::text_muted())
                            .child(unicode),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(CARD_GAP))
                    .p(px(CARD_PAD))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(CARD_GAP))
                            .child(label("LSB".into()))
                            .child(metric(&self.metric_inputs.lsb))
                            .child(metric(&self.metric_inputs.width))
                            .child(metric(&self.metric_inputs.rsb))
                            .child(label("RSB".into())),
                    )
                    .child(
                        // Kerning groups sit under the sidebearing they
                        // apply to, the way Glyphs stacks them.
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(label(SharedString::from(group_l)))
                            .child(label(SharedString::from(group_r))),
                    ),
            );

        let selection = self.selection_bounds().map(|r| {
            let readout = |name: &'static str, value: f64| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(CARD_GAP))
                    .child(div().w(px(10.0)).text_xs().text_color(t::text_muted()).child(name))
                    .child(
                        div()
                            .text_sm()
                            .text_color(t::text())
                            .child(SharedString::from(format!("{value:.0}"))),
                    )
            };
            card()
                .child(
                    div()
                        .h(px(HEADER_H))
                        .px(px(CARD_PAD))
                        .flex()
                        .items_center()
                        .rounded_t(px(CARD_RADIUS - 1.0))
                        .bg(t::cell_selected_bg())
                        .border_b_1()
                        .border_color(t::panel_outline())
                        .text_sm()
                        .text_color(t::text_muted())
                        .child("Selection"),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(CARD_PAD * 2.0))
                        .p(px(CARD_PAD))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(CARD_GAP))
                                .child(readout("X", r.x0))
                                .child(readout("Y", r.y0)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(CARD_GAP))
                                .child(readout("W", r.width()))
                                .child(readout("H", r.height())),
                        ),
                )
        });

        div()
            .absolute()
            .bottom(px(12.0))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .items_end()
            .gap_2()
            .child(metrics)
            .children(selection)
            .into_any_element()
    }

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
        let Mode::Editor(index) = self.mode else { return };
        let Some(font) = self.font() else { return };
        let (dx, dy) = self.editor.window_to_design(pos);
        let tolerance = 16.0 / self.editor.zoom().max(1e-6);
        let entry = &font.glyphs[index];
        let anchor = entry
            .anchors
            .iter()
            .enumerate()
            .map(|(i, (_, x, y))| {
                (((x - dx).powi(2) + (y - dy).powi(2)).sqrt(), i)
            })
            .filter(|(dist, _)| *dist <= tolerance)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, i)| i);
        let norad_glyph = font.font.get_glyph(entry.name.as_ref());
        let component = if anchor.is_none() {
            norad_glyph
                .and_then(|g| {
                    runebender_core::glyph_ops::component_at(
                        &font.font,
                        g,
                        kurbo::Point::new(dx, dy),
                    )
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
        });
    }

    /// Run one context-menu action and close the menu.
    fn context_menu_action(&mut self, action: &'static str) {
        let Some(menu) = self.context_menu.take() else { return };
        let Mode::Editor(index) = self.mode else { return };
        match action {
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
                                    &font_clone, g, ci,
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
                let ok = self
                    .font_mut()
                    .is_some_and(|f| f.decompose(index));
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
            "set-start" => {
                if let Some((ci, pi)) = menu.start_point {
                    self.push_undo_snapshot(index);
                    let ok = self
                        .font_mut()
                        .and_then(|f| {
                            f.edit_glyph(index, |g| {
                                runebender_core::glyph_ops::set_contour_start(
                                    g, ci, pi,
                                )
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
                    let target: std::collections::HashSet<(usize, usize)> =
                        [(ci, 0)].into();
                    let ok = self
                        .font_mut()
                        .and_then(|f| {
                            f.edit_glyph(index, |g| {
                                runebender_core::glyph_ops::reverse_contours(
                                    g, &target,
                                )
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
                                runebender_core::glyph_ops::move_contour(
                                    g, ci, up,
                                )
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
                    font.add_anchor(
                        index,
                        menu.design.0.round(),
                        menu.design.1.round(),
                    );
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
        let Mode::Editor(index) = self.mode else { return };
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
                    runebender_core::glyph_ops::add_component(
                        &font_clone,
                        g,
                        &base,
                    )
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

    fn editor_mouse_down(&mut self, pos: Point<gpui::Pixels>, shift: bool, alt: bool, click_count: usize) {
        self.context_menu = None;
        self.nudging = false;

        self.ensure_editor_fit();
        let Mode::Editor(index) = self.mode else {
            return;
        };
        if click_count >= 2 {
            if self.double_click_edit(pos) {
                return;
            }
            if self.activate_sort_at_pos(pos) {
                return;
            }
        }
        if self.editor.tool == Tool::Text {
            self.text_tool_click(pos, shift);
            return;
        }
        if self.editor.tool == Tool::Knife {
            let (dx, dy) = self.editor.window_to_design(pos);
            self.editor.drag = Some(Drag::Knife {
                start: (dx, dy),
                current: (dx, dy),
            });
            return;
        }
        if self.editor.tool == Tool::HyperPen {
            self.hyper_pen_mouse_down(index, pos, shift);
            return;
        }
        if self.editor.tool == Tool::Pen {
            self.pen_mouse_down(index, pos, alt);
            return;
        }
        if self.editor.tool == Tool::Preview {
            // Preview is the pan tool: dragging moves the viewport,
            // the way the web's PreviewTool does. Hold space to reach
            // it from any other tool.
            let local = self.editor.window_to_local(pos);
            self.editor.drag = Some(Drag::Pan {
                last: (local.x, local.y),
            });
            return;
        }
        if matches!(self.editor.tool, Tool::Shapes | Tool::Measure) {
            let (dx, dy) = self.editor.window_to_design(pos);
            self.editor.drag = Some(if self.editor.tool == Tool::Shapes {
                Drag::Shape {
                    start: (dx, dy),
                    current: (dx, dy),
                }
            } else {
                Drag::Measure {
                    start: (dx, dy),
                    current: (dx, dy),
                }
            });
            return;
        }
        if alt && self.editor.tool == Tool::Select {
            // Alt-click on a line segment converts it to a curve
            // (thirds handles); otherwise alt-drag pans.
            let (adx, ady) = self.editor.window_to_design(pos);
            let radius = HIT_RADIUS_PX / self.editor.zoom();
            let converted = self
                .font()
                .and_then(|f| f.font.get_glyph(f.glyphs[index].name.as_ref()))
                .and_then(|g| {
                    runebender_core::segment_ops::nearest_segment_with_t(
                        g,
                        kurbo::Point::new(adx, ady),
                        radius,
                    )
                })
                .filter(|(hit, _)| matches!(hit.seg, kurbo::PathSeg::Line(_)));
            if let Some((seg_hit, _)) = converted {
                self.push_undo_snapshot(index);
                let new_controls = self
                    .font_mut()
                    .and_then(|f| {
                        f.edit_glyph(index, |g| {
                            runebender_core::segment_ops::convert_line_to_curve(
                                g, &seg_hit,
                            )
                        })
                    })
                    .flatten();
                match new_controls {
                    Some(ids) => {
                        self.editor.selected = ids.into_iter().collect();
                    }
                    None => {
                        self.editor.undo.pop();
                    }
                }
                self.editor.segment_hover = None;
                return;
            }
            let local = self.editor.window_to_local(pos);
            self.editor.drag = Some(Drag::Pan {
                last: (local.x, local.y),
            });
            return;
        }
        let Some(font) = self.font() else {
            return;
        };
        let (dx, dy) = self.editor.window_to_design(pos);
        let tolerance = HIT_RADIUS_PX / self.editor.zoom();
        let point_tolerance = POINT_HIT_RADIUS_PX / self.editor.zoom();
        // Copy the point data out so selection can mutate afterwards.
        let all_points: Vec<((usize, usize), (f64, f64))> = font.glyphs[index]
            .points
            .iter()
            .map(|p| ((p.contour, p.index), (p.x, p.y)))
            .collect();
        // Anchors take priority over points.
        let anchor_hit = font.glyphs[index]
            .anchors
            .iter()
            .enumerate()
            .map(|(i, (_, x, y))| {
                let dist = ((x - dx).powi(2) + (y - dy).powi(2)).sqrt();
                (dist, i, (*x, *y))
            })
            .filter(|(dist, _, _)| *dist <= point_tolerance)
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let hit = all_points
            .iter()
            .map(|(id, (x, y))| {
                let dist = ((x - dx).powi(2) + (y - dy).powi(2)).sqrt();
                (dist, *id)
            })
            .filter(|(dist, _)| *dist <= point_tolerance)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id);

        match hit {
            Some(id) => {
                self.editor.selected_component = None;
                if shift {
                    if !self.editor.selected.remove(&id) {
                        self.editor.selected.insert(id);
                    }
                } else if !self.editor.selected.contains(&id) {
                    self.editor.selected.clear();
                    self.editor.selected.insert(id);
                }
                if self.editor.selected.contains(&id) {
                    let originals: std::collections::HashMap<
                        (usize, usize),
                        (f64, f64),
                    > = all_points.iter().copied().collect();
                    let anchor = self.selected_anchor_origin(index);
                    self.push_undo_snapshot(index);
                    self.editor.drag = Some(Drag::Points {
                        start: (dx, dy),
                        originals,
                        anchor,
                    });
                }
            }
            None => {
                // Points outrank anchors, and an anchor may be dragged
                // together with a point selection (web keeps points and
                // anchors in one selection). Shift adds to what is
                // there; a plain click on a fresh anchor starts over.
                if let Some((_, ai, _)) = anchor_hit {
                    self.editor.selected_component = None;
                    let already = self.editor.selected_anchors.contains(&ai);
                    if shift {
                        if already {
                            self.editor.selected_anchors.retain(|a| *a != ai);
                            return;
                        }
                        self.editor.selected_anchors.push(ai);
                    } else {
                        if !already {
                            // A plain click on an unselected anchor
                            // starts over; clicking one of a group
                            // drags the whole group.
                            self.editor.selected.clear();
                            self.editor.selected_anchors = vec![ai];
                        }
                    }
                    let originals: std::collections::HashMap<
                        (usize, usize),
                        (f64, f64),
                    > = all_points.iter().copied().collect();
                    let anchor = self.selected_anchor_origin(index);
                    self.push_undo_snapshot(index);
                    self.editor.drag = Some(Drag::Points {
                        start: (dx, dy),
                        originals,
                        anchor,
                    });
                    return;
                }
                self.editor.selected_anchors.clear();
                // Sidebearing edge before segments: with a small or
                // negative sidebearing the outline runs along the
                // metric line, and a click on the line must not drag
                // the stem that shares it (web ordering).
                let (top_b, bottom_b) = self.text_sort_bounds();
                let advance = self
                    .font()
                    .map(|f| f.glyphs[index].advance)
                    .unwrap_or(0.0);
                if dy >= bottom_b - tolerance && dy <= top_b + tolerance {
                    let edge = if (dx - advance).abs() <= tolerance {
                        Some(true)
                    } else if dx.abs() <= tolerance {
                        Some(false)
                    } else {
                        None
                    };
                    if let Some(right) = edge {
                        self.push_undo_snapshot(index);
                        self.editor.sidebearing_hover = Some(right);
                        self.editor.drag = Some(Drag::Sidebearing {
                            right,
                            start_x: dx,
                            applied: 0.0,
                            start_width: advance,
                        });
                        return;
                    }
                }
                // A click on a segment selects its points and drags
                // them together, like the web select tool.
                let seg = self
                    .font()
                    .and_then(|f| f.font.get_glyph(f.glyphs[index].name.as_ref()))
                    .and_then(|g| {
                        runebender_core::segment_ops::nearest_segment_with_t(
                            g,
                            kurbo::Point::new(dx, dy),
                            tolerance,
                        )
                    });
                if let Some((seg_hit, _)) = seg {
                    let ids = seg_hit.point_ids();
                    if shift {
                        if ids.iter().all(|id| self.editor.selected.contains(id)) {
                            // Shift-clicking a segment that is already
                            // selected takes it back out, and starts no
                            // drag (web returns Some(false) here).
                            for id in &ids {
                                self.editor.selected.remove(id);
                            }
                            return;
                        }
                        self.editor.selected.extend(ids.iter().copied());
                    } else {
                        self.editor.selected = ids.iter().copied().collect();
                    }
                    let originals: std::collections::HashMap<
                        (usize, usize),
                        (f64, f64),
                    > = all_points.iter().copied().collect();
                    let anchor = self.selected_anchor_origin(index);
                    self.push_undo_snapshot(index);
                    self.editor.drag = Some(Drag::Points {
                        start: (dx, dy),
                        originals,
                        anchor,
                    });
                    return;
                }
                let component_hit = self
                    .font()
                    .and_then(|f| {
                        let g = f.font.get_glyph(f.glyphs[index].name.as_ref())?;
                        runebender_core::glyph_ops::component_at(
                            &f.font,
                            g,
                            kurbo::Point::new(dx, dy),
                        )
                        .map(|ci| {
                            let t = &g.components[ci].transform;
                            (ci, (t.x_offset, t.y_offset))
                        })
                    });
                if let Some((ci, orig)) = component_hit {
                    self.editor.selected_component = Some(ci);
                    self.editor.selected.clear();
                    // An aligned component belongs to its anchor, so
                    // dragging is refused rather than quietly breaking
                    // the link — the Glyphs contract: unlock first,
                    // then move (web translate_selected_component).
                    let aligned = self
                        .font()
                        .and_then(|f| f.font.get_glyph(f.glyphs[index].name.as_ref()))
                        .and_then(|g| g.components.get(ci))
                        .is_some_and(|c| {
                            !runebender_core::composites::component_alignment_disabled(c)
                        });
                    if aligned {
                        self.status_note = Some(
                            "Component is anchor-locked · unlock it in the Selection panel to move it"
                                .into(),
                        );
                        return;
                    }
                    self.push_undo_snapshot(index);
                    self.editor.drag = Some(Drag::Component {
                        index: ci,
                        start: (dx, dy),
                        orig,
                    });
                    return;
                }
                self.editor.selected_component = None;
                if !shift {
                    self.editor.selected.clear();
                    self.editor.selected_anchors.clear();
                }
                // The selection the marquee started from: every drag
                // step recomputes selection = base ∪ enclosed, so
                // shrinking the box gives points back (web
                // select_in_screen_rect).
                let base = self.editor.selected.clone();
                let base_anchors = self.editor.selected_anchors.clone();
                self.editor.drag = Some(Drag::Marquee {
                    start: (dx, dy),
                    current: (dx, dy),
                    base,
                    base_anchors,
                });
            }
        }
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

    fn editor_mouse_drag(
        &mut self,
        pos: Point<gpui::Pixels>,
        shift: bool,
        alt: bool,
    ) -> bool {
        let Mode::Editor(index) = self.mode else {
            return false;
        };
        if self.editor.tool == Tool::Pen {
            return self.pen_mouse_drag(index, pos);
        }
        let (dx, dy) = self.editor.window_to_design(pos);
        self.editor.cursor = (dx, dy);
        match &mut self.editor.drag {
            Some(Drag::Points { start, originals, anchor }) => {
                // The whole gesture is measured from where it began, so
                // grid snapping cannot accumulate drift, and core owns
                // the rules: handles ride along with their on-curve
                // point, smooth tangents stay aimed, points land on the
                // design grid. Alt moves the selection alone.
                let delta = (dx - start.0, dy - start.1);
                let originals = originals.clone();
                let anchor = anchor.clone();
                let selected = self.editor.selected.clone();
                let Some(font) = self.font_mut() else {
                    return false;
                };
                let mut changed = font
                    .edit_glyph(index, |g| {
                        runebender_core::point_ops::translate_points(
                            g, &selected, &originals, delta, alt,
                        )
                    })
                    .unwrap_or(false);
                for (ai, (ox, oy)) in anchor {
                    use runebender_core::point_ops::snap_coord;
                    font.set_anchor(
                        index,
                        ai,
                        snap_coord(ox + delta.0),
                        snap_coord(oy + delta.1),
                    );
                    changed = true;
                }
                changed
            }
            Some(Drag::TextKern) => {
                let bx = dx + self.editor.sort_offset.0;
                let changed = self.edit_buffer.drag_manual_kerning(bx).is_some();
                if changed {
                    self.sync_sort_offset();
                }
                changed
            }
            Some(Drag::Sidebearing {
                right,
                start_x,
                applied,
                start_width,
            }) => {
                let (right, start_x, prev_applied, start_width) =
                    (*right, *start_x, *applied, *start_width);
                // Snap to the zoom-matched grid step like the web:
                // 2 units zoomed close, 8 mid, whole units otherwise.
                let zoom = self.editor.zoom();
                let snap = if zoom > 8.0 {
                    2.0
                } else if zoom > 0.8 {
                    8.0
                } else {
                    1.0
                };
                let total = dx - start_x;
                let target = if right {
                    ((start_width + total) / snap).round() * snap - start_width
                } else {
                    (total / snap).round() * snap
                };
                let step = target - prev_applied;
                if step == 0.0 {
                    return false;
                }
                if let Some(Drag::Sidebearing { applied, .. }) =
                    &mut self.editor.drag
                {
                    *applied = target;
                }
                let changed = if right {
                    self.font_mut().is_some_and(|f| {
                        f.edit_glyph(index, |g| {
                            g.width += step;
                            g.width >= 0.0
                        })
                        .unwrap_or(false)
                    })
                } else {
                    // The left edge moves: ink stays put on screen by
                    // shifting glyph space and the viewport together.
                    let ok = self.font_mut().is_some_and(|f| {
                        f.edit_glyph(index, |g| {
                            if g.width - step < 0.0 {
                                return false;
                            }
                            runebender_core::glyph_ops::shift_ink(g, -step);
                            g.width -= step;
                            true
                        })
                        .unwrap_or(false)
                    });
                    if ok {
                        self.editor.viewport.offset.x += step * self.editor.zoom();
                    }
                    ok
                };
                if changed {
                    self.rebuild_text_models();
                    self.sync_sort_offset();
                }
                changed
            }
            Some(Drag::Component { index: ci, start, orig }) => {
                let (ci, start, orig) = (*ci, *start, *orig);
                let target = (
                    (orig.0 + dx - start.0).round(),
                    (orig.1 + dy - start.1).round(),
                );
                self.font_mut().is_some_and(|f| {
                    f.edit_glyph(index, |g| {
                        if let Some(c) = g.components.get_mut(ci) {
                            c.transform.x_offset = target.0;
                            c.transform.y_offset = target.1;
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false)
                })
            }
            Some(Drag::Pan { last }) => {
                let (lx, ly) = *last;
                *last = (0.0, 0.0); // placeholder; recomputed below
                let local = {
                    // Reborrow immutably for the conversion.
                    let ed = &self.editor;
                    ed.window_to_local(pos)
                };
                self.editor.viewport.offset.x += local.x - lx;
                self.editor.viewport.offset.y += local.y - ly;
                if let Some(Drag::Pan { last }) = &mut self.editor.drag {
                    *last = (local.x, local.y);
                }
                true
            }
            Some(Drag::Knife { start, current })
            | Some(Drag::Measure { start, current }) => {
                // Shift locks the line to an axis (web
                // constrain_measure_end).
                *current = if shift {
                    let (sx, sy) = *start;
                    if (dx - sx).abs() >= (dy - sy).abs() {
                        (dx, sy)
                    } else {
                        (sx, dy)
                    }
                } else {
                    (dx, dy)
                };
                true
            }
            Some(Drag::Shape { start, current }) => {
                // Shift locks the shape square (web constrain_point).
                *current = if shift {
                    let (sx, sy) = *start;
                    let size = (dx - sx).abs().max((dy - sy).abs());
                    (
                        sx + size * (dx - sx).signum(),
                        sy + size * (dy - sy).signum(),
                    )
                } else {
                    (dx, dy)
                };
                true
            }
            Some(Drag::Marquee { start, current, base, base_anchors }) => {
                *current = (dx, dy);
                let (sx, sy) = *start;
                let (base, base_anchors) = (base.clone(), base_anchors.clone());
                self.select_in_rect(index, (sx, sy), (dx, dy), &base, &base_anchors);
                true
            }
            None => false,
        }
    }

    /// Idle mouse move over the canvas: track the pointer for pen
    /// previews, and alt-hover highlights the nearest segment
    /// (select tool), like the web editor.
    fn editor_hover(&mut self, pos: Point<gpui::Pixels>, alt: bool) -> bool {
        let Mode::Editor(index) = self.mode else {
            return false;
        };
        let mut changed = false;
        let track_pointer = matches!(
            self.editor.tool,
            Tool::Pen | Tool::HyperPen | Tool::Select
        );
        if track_pointer {
            let moved = self
                .editor
                .pointer
                .is_none_or(|p| p != pos);
            self.editor.pointer = Some(pos);
            // Re-render for the pen rubber band only while drawing.
            if moved
                && (self.editor.pen.is_some() || self.editor.hyper_contour.is_some())
            {
                changed = true;
            }
        }
        if self.editor.tool == Tool::Select && self.editor.drag.is_none() {
            let (dx, dy) = self.editor.window_to_design(pos);
            let tolerance = HIT_RADIUS_PX / self.editor.zoom();
            let (top_b, bottom_b) = self.text_sort_bounds();
            let advance = self
                .font()
                .map(|f| f.glyphs[index].advance)
                .unwrap_or(0.0);
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
                .map(|p| (p.contour, p.index)),
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

    fn editor_mouse_up(&mut self) {
        if self.editor.tool == Tool::Pen {
            if let Some(pen) = self.editor.pen.as_mut() {
                pen.placing = None;
            }
            return;
        }
        let Mode::Editor(index) = self.mode else {
            self.editor.drag = None;
            return;
        };
        if let Some(Drag::Shape { start, current }) = self.editor.drag.as_ref() {
            let rect = kurbo::Rect::from_points(
                kurbo::Point::new(start.0, start.1),
                kurbo::Point::new(current.0, current.1),
            );
            self.editor.drag = None;
            if rect.width() >= 2.0 && rect.height() >= 2.0 {
                self.push_undo_snapshot(index);
                let ellipse = self.editor.shape_ellipse;
                if let Some(font) = self.font_mut() {
                    font.add_shape_contour(index, rect, ellipse);
                }
            }
            return;
        }
        if matches!(self.editor.drag, Some(Drag::TextKern)) {
            self.editor.drag = None;
            if self.edit_buffer.end_manual_kerning() {
                self.sync_kerning_from_buffer();
            }
            return;
        }
        if matches!(self.editor.drag, Some(Drag::Measure { .. })) {
            self.editor.drag = None;
            return;
        }
        if let Some(Drag::Knife { start, current }) = self.editor.drag.take() {
            let p0 = kurbo::Point::new(start.0, start.1);
            let p1 = kurbo::Point::new(current.0, current.1);
            // Fewer than two crossings can't produce a cut; skip the
            // edit entirely so a missed slice leaves nothing dirty.
            let crossings = self
                .font()
                .and_then(|f| f.font.get_glyph(f.glyphs[index].name.as_ref()))
                .map(|g| runebender_core::knife::knife_hit_points(g, p0, p1).len())
                .unwrap_or(0);
            if p0.distance(p1) >= 2.0 && crossings >= 2 {
                self.push_undo_snapshot(index);
                let changed = self
                    .font_mut()
                    .and_then(|f| {
                        f.edit_glyph(index, |g| {
                            runebender_core::knife::knife_cut_glyph(g, p0, p1)
                        })
                    })
                    .unwrap_or(false);
                if !changed {
                    self.editor.undo.pop();
                } else {
                    self.editor.selected.clear();
                }
            }
            return;
        }
        if matches!(self.editor.drag, Some(Drag::Points { .. })) {
            // A released drag settles its handles on the design grid,
            // re-aiming smooth tangents afterwards (web
            // snap_selected_offcurves_to_grid on left_drag_ended).
            let selected = self.editor.selected.clone();
            if let Some(font) = self.font_mut() {
                font.edit_glyph(index, |g| {
                    runebender_core::point_ops::snap_selected_offcurves(g, &selected)
                });
            }
            self.editor.drag = None;
            return;
        }
        if let Some(Drag::Marquee { start, current, base, base_anchors }) =
            self.editor.drag.take()
        {
            self.select_in_rect(index, start, current, &base, &base_anchors);
        }
        self.editor.drag = None;
    }

    /// Pen click: place a point (line segment from the previous one,
    /// curve if the previous point was dragged into a handle), start
    /// a contour if none is open, or close the contour when clicking
    /// its first point.
    fn pen_mouse_down(&mut self, index: usize, pos: Point<gpui::Pixels>, alt: bool) {
        let (dx, dy) = self.editor.window_to_design(pos);
        let (x, y) = (dx.round(), dy.round());
        let tolerance = HIT_RADIUS_PX / self.editor.zoom();

        // Web pen: with no path in progress, a click on an existing
        // segment inserts a point on it (alt converts a line to a
        // curve instead).
        if self.editor.pen.is_none() {
            let snap_radius = 10.0 / self.editor.zoom().max(1e-6);
            let seg = self
                .font()
                .and_then(|f| f.font.get_glyph(f.glyphs[index].name.as_ref()))
                .and_then(|g| {
                    runebender_core::segment_ops::nearest_segment_with_t(
                        g,
                        kurbo::Point::new(dx, dy),
                        snap_radius,
                    )
                });
            if let Some((seg_hit, t)) = seg {
                self.push_undo_snapshot(index);
                let result = self.font_mut().and_then(|f| {
                    f.edit_glyph(index, |g| {
                        if alt {
                            runebender_core::segment_ops::convert_line_to_curve(
                                g, &seg_hit,
                            )
                            .map(|ids| ids[0])
                        } else {
                            runebender_core::segment_ops::insert_point_on_segment(
                                g, &seg_hit, t,
                            )
                        }
                    })
                });
                match result.flatten() {
                    Some(id) => {
                        self.editor.selected = [id].into();
                    }
                    None => {
                        self.editor.undo.pop();
                    }
                }
                return;
            }
        }
        self.push_undo_snapshot(index);

        match self.editor.pen.take() {
            None => {
                if let Some(contour) =
                    self.font_mut().and_then(|f| f.start_contour(index, x, y))
                {
                    self.editor.pen = Some(PenState {
                        contour,
                        prev_out_handle: None,
                        placing: Some((x, y)),
                    });
                }
            }
            Some(pen) => {
                // Near the contour's start point? Close it.
                let start = self.font().and_then(|f| {
                    f.glyphs[index]
                        .points
                        .iter()
                        .find(|p| p.contour == pen.contour && p.index == 0)
                        .map(|p| (p.x, p.y))
                });
                let closing = start.is_some_and(|(sx, sy)| {
                    ((sx - dx).powi(2) + (sy - dy).powi(2)).sqrt() <= tolerance
                });
                let controls = pen.prev_out_handle.map(|out| {
                    let target = if closing { start.unwrap() } else { (x, y) };
                    // Incoming control defaults onto the target until
                    // the user drags this point into a curve.
                    (out, target)
                });
                if let Some(font) = self.font_mut() {
                    if closing {
                        font.close_contour(index, pen.contour, controls);
                        self.editor.pen = None;
                    } else {
                        font.append_segment(index, pen.contour, controls, x, y, false);
                        self.editor.pen = Some(PenState {
                            contour: pen.contour,
                            prev_out_handle: None,
                            placing: Some((x, y)),
                        });
                    }
                }
            }
        }
    }

    /// Pen drag while placing a point: pull out symmetric handles.
    /// The outgoing handle follows the cursor; the segment into the
    /// point (if curved) gets the mirrored incoming handle.
    fn pen_mouse_drag(&mut self, index: usize, pos: Point<gpui::Pixels>) -> bool {
        let (dx, dy) = self.editor.window_to_design(pos);
        let Some(pen) = self.editor.pen.as_mut() else {
            return false;
        };
        let Some((px, py)) = pen.placing else {
            return false;
        };
        let out = (dx.round(), dy.round());
        let mirror = ((2.0 * px - out.0).round(), (2.0 * py - out.1).round());
        pen.prev_out_handle = Some(out);
        let contour = pen.contour;
        // If the just-placed point ended a curve segment, move its
        // incoming control to the mirror and mark the point smooth.
        #[allow(clippy::type_complexity)]
        let updates: Option<Vec<((usize, usize), (f64, f64))>> = self.font().map(|f| {
            let pts: Vec<_> = f.glyphs[index]
                .points
                .iter()
                .filter(|p| p.contour == contour)
                .collect();
            let n = pts.len();
            // Points layout when last segment was a curve:
            // [... c1 c2 P] — c2 is at n-2.
            if n >= 3 && !pts[n - 2].on_curve && pts[n - 1].x == px && pts[n - 1].y == py {
                vec![((contour, n - 2), mirror)]
            } else {
                Vec::new()
            }
        });
        if let (Some(updates), Some(font)) = (updates, self.font_mut())
            && !updates.is_empty()
        {
            font.set_points(index, &updates);
        }
        true
    }

    /// Finish an open pen contour without closing it.
    /// Hyper pen click: extend the open hyperbezier contour
    /// (shift-click adds a corner point), close it by clicking its
    /// first point, or start a new one.
    fn hyper_pen_mouse_down(
        &mut self,
        index: usize,
        pos: Point<gpui::Pixels>,
        corner: bool,
    ) {
        let (dx, dy) = self.editor.window_to_design(pos);
        let (x, y) = (dx.round(), dy.round());
        let tolerance = HIT_RADIUS_PX / self.editor.zoom();
        self.push_undo_snapshot(index);

        match self.editor.hyper_contour {
            None => {
                if let Some(contour) = self
                    .font_mut()
                    .and_then(|f| f.start_hyper_contour(index, x, y))
                {
                    self.editor.hyper_contour = Some(contour);
                }
            }
            Some(contour) => {
                let start = self.font().and_then(|f| {
                    let c = f.font.get_glyph(f.glyphs[index].name.as_ref())?;
                    let p = c.contours.get(contour)?.points.first()?;
                    Some((p.x, p.y))
                });
                let closes = start.is_some_and(|(sx, sy)| {
                    ((sx - x).powi(2) + (sy - y).powi(2)).sqrt() <= tolerance
                });
                if closes {
                    if let Some(font) = self.font_mut() {
                        font.close_hyper_contour(index, contour);
                    }
                    self.editor.hyper_contour = None;
                } else if let Some(font) = self.font_mut() {
                    font.append_hyper_point(index, contour, x, y, corner);
                }
            }
        }
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
        let Some(index) = (match self.mode {
            Mode::Editor(index) => Some(index),
            Mode::Grid => self.selected,
        }) else {
            return;
        };
        self.push_undo_snapshot(index);
        let Some(font) = self.font_mut() else {
            return;
        };
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

    /// Rename the selected glyph in every master, updating components,
    /// groups, kerning, and the open text session.
    fn apply_glyph_rename(&mut self, new_name: &str) {
        let Some(index) = self.selected else { return };
        let Some(old) = self.font().map(|f| f.glyphs[index].name.to_string())
        else {
            return;
        };
        let new_name = new_name.trim().to_string();
        if new_name.is_empty() || new_name == old {
            return;
        }
        let Some(project) = self.project.as_mut() else { return };
        let mut renamed = false;
        for master in project.masters.iter_mut() {
            if runebender_core::glyph_ops::rename_glyph(
                &mut master.font,
                &old,
                &new_name,
            ) {
                master.dirty = true;
                master.kerning_dirty = true;
                master.modified_glyphs.remove(&old);
                master.modified_glyphs.insert(new_name.clone());
                master.refresh_from_font();
                renamed = true;
            }
        }
        if !renamed {
            self.status_note =
                Some(format!("Cannot rename {old} to {new_name}").into());
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
                self.edit_buffer.update_glyph(
                    i,
                    new_name.clone(),
                    codepoint,
                    advance,
                );
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
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string())
        else {
            return;
        };
        let Some(project) = self.project.as_mut() else { return };
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
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string())
        else {
            return;
        };
        let Some(project) = self.project.as_mut() else { return };
        for master in project.masters.iter_mut() {
            if runebender_core::glyph_ops::set_kern_group(
                &mut master.font,
                &name,
                first_side,
                text,
            ) {
                master.dirty = true;
                master.kerning_dirty = true;
            }
        }
        self.rebuild_text_models();
    }

    /// Fill the Glyph panel's editable fields from the selected glyph
    /// unless one of them is being typed in.
    fn refresh_glyph_inputs(
        &mut self,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !force
            && window
                .focused(cx)
                .is_some_and(|f| f != self.focus_handle)
        {
            return;
        }
        let Some(index) = self.selected else { return };
        let Some(font) = self.font() else { return };
        let Some(entry) = font.glyphs.get(index) else { return };
        let name = entry.name.to_string();
        let unicode = entry
            .codepoint
            .map(|c| format!("{:04X}", c as u32))
            .unwrap_or_default();
        let group_l =
            runebender_core::glyph_ops::kern_group(&font.font, &name, true)
                .map(|g| g.as_str().replace("public.kern1.", ""))
                .unwrap_or_default();
        let group_r =
            runebender_core::glyph_ops::kern_group(&font.font, &name, false)
                .map(|g| g.as_str().replace("public.kern2.", ""))
                .unwrap_or_default();
        let set = |entity: &gpui::Entity<gpui_component::input::InputState>,
                   value: String,
                   window: &mut Window,
                   cx: &mut Context<Self>| {
            entity.update(cx, |st, cx| {
                if st.value() != value.as_str() {
                    st.set_value(value, window, cx);
                }
            });
        };
        let name_input = self.glyph_inputs.name.clone();
        let unicode_input = self.glyph_inputs.unicode.clone();
        let l_input = self.glyph_inputs.group_l.clone();
        let r_input = self.glyph_inputs.group_r.clone();
        set(&name_input, name, window, cx);
        set(&unicode_input, unicode, window, cx);
        set(&l_input, group_l, window, cx);
        set(&r_input, group_r, window, cx);
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
            if window
                .focused(cx)
                .is_some_and(|f| f != self.focus_handle)
            {
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
        let set = |entity: &gpui::Entity<gpui_component::input::InputState>,
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
        let Mode::Editor(index) = self.mode else { return };
        let Some(ai) = self.editor.selected_anchor() else { return };
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
        let Mode::Editor(index) = self.mode else { return None };
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
            let transform = runebender_core::glyph_paths::component_affine(
                &component.transform,
            );
            let path = transform
                * &runebender_core::glyph_paths::glyph_to_bezpath(
                    base, &font.font,
                );
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
        let Mode::Editor(index) = self.mode else { return };
        if !value.is_finite() {
            return;
        }
        let Some(bounds) = self.selection_bounds() else { return };
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
        let Mode::Editor(index) = self.mode else { return };
        if !value.is_finite() || value <= 0.0 {
            return;
        }
        let Some(bounds) = self.selection_bounds() else { return };
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
                        runebender_core::glyph_ops::translate_component(
                            g, ci, delta.x, delta.y,
                        )
                    })
                })
                .unwrap_or(false);
        }
        if let Some(ai) = self.editor.selected_anchor() {
            let target = self
                .font()
                .and_then(|f| {
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
                            runebender_core::glyph_paths::component_affine(
                                &component.transform,
                            );
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
                    runebender_core::glyph_ops::transform_selection(
                        g, &selected, transform,
                    )
                })
            })
            .unwrap_or(false)
    }

    /// Keep the Selection X/Y inputs showing the selected point.
    fn refresh_coord_inputs(&mut self, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !force
            && window
                .focused(cx)
                .is_some_and(|f| f != self.focus_handle)
        {
            return;
        }
        let (x, y, w, h) = match self.selection_bounds() {
            Some(bounds) => {
                let reference =
                    self.coord_quadrant.point_in_dspace_rect(bounds);
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
                let Mode::Editor(index) = self.mode else { return None };
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

    /// Selection section: count plus editable X/Y for a single point.
    fn selection_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let count = self.editor.selected.len();
        let single = self.single_selected_point();
        let mut body = div().flex().flex_col().gap_2().child(
            div()
                .text_sm()
                .text_color(t::text_muted())
                .child(match count {
                    0 => "No points selected".to_string(),
                    1 => "1 point".to_string(),
                    n => format!("{n} points"),
                }),
        );
        let _ = single;
        // A whole segment selected: report the curve's real size, which
        // is what you compare when matching one curve to another.
        if let Some((segments, r)) = self.selected_segment_bounds() {
            let label = if segments == 1 {
                "Segment".to_string()
            } else {
                format!("{segments} segments")
            };
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .text_sm()
                    .child(
                        div().text_color(t::text_muted()).child(label),
                    )
                    .child(
                        div().text_color(t::text()).child(SharedString::from(
                            format!("{:.0} × {:.0}", r.width(), r.height()),
                        )),
                    ),
            );
        }
        let has_selection = !self.editor.selected.is_empty()
            || self.editor.selected_component.is_some()
            || !self.editor.selected_anchors.is_empty();
        if has_selection {
            use runebender_core::path::Quadrant;
            let field = |label: &'static str,
                         input: &gpui::Entity<gpui_component::input::InputState>| {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(14.0)).text_sm().text_color(t::text_muted()).child(label))
                    .child(div().flex_1().child(gpui_component::input::Input::new(input)))
            };
            // The 9-point reference picker (web coordinate quadrant):
            // numeric X/Y and W/H act about the chosen corner.
            const QUADRANTS: [[Quadrant; 3]; 3] = [
                [Quadrant::TopLeft, Quadrant::Top, Quadrant::TopRight],
                [Quadrant::Left, Quadrant::Center, Quadrant::Right],
                [
                    Quadrant::BottomLeft,
                    Quadrant::Bottom,
                    Quadrant::BottomRight,
                ],
            ];
            let mut picker = div().flex().flex_col().gap_0p5();
            for (ri, row_quads) in QUADRANTS.iter().enumerate() {
                let mut row_el = div().flex().gap_0p5();
                for (qi, quadrant) in row_quads.iter().enumerate() {
                    let quadrant = *quadrant;
                    let active = self.coord_quadrant == quadrant;
                    row_el = row_el.child(
                        div()
                            .id(("quadrant", ri * 3 + qi))
                            .w(px(10.0))
                            .h(px(10.0))
                            .rounded_sm()
                            .cursor_pointer()
                            .border_1()
                            .when(active, |el| {
                                el.bg(t::accent()).border_color(t::accent())
                            })
                            .when(!active, |el| {
                                el.border_color(t::cell_border())
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.coord_quadrant = quadrant;
                                cx.notify();
                            })),
                    );
                }
                picker = picker.child(row_el);
            }
            body = body.child(
                div()
                    .flex()
                    .gap_3()
                    .items_center()
                    .child(picker)
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(field("X", &self.metric_inputs.x))
                            .child(field("Y", &self.metric_inputs.y)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(field("W", &self.metric_inputs.w))
                            .child(field("H", &self.metric_inputs.h)),
                    ),
            );
        }
        // Selected anchor: editable name (web AnchorPanel).
        if !self.editor.selected_anchors.is_empty() {
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(t::text_muted())
                            .child("Anchor"),
                    )
                    .child(div().flex_1().child(
                        gpui_component::input::Input::new(
                            &self.anchor_name_input,
                        ),
                    )),
            );
        }
        // Selected component: name plus the anchor lock, the Glyphs
        // contract — locked follows its anchor, free is draggable.
        if let (Mode::Editor(index), Some(ci)) =
            (&self.mode, self.editor.selected_component)
        {
            let index = *index;
            let info = self
                .font()
                .and_then(|f| f.font.get_glyph(f.glyphs[index].name.as_ref()))
                .and_then(|g| g.components.get(ci))
                .map(|c| {
                    (
                        c.base.to_string(),
                        !runebender_core::composites::component_alignment_disabled(c),
                    )
                });
            if let Some((base, aligned)) = info {
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .text_color(t::text_muted())
                                .child(format!("Component /{base}")),
                        )
                        .child(
                            div()
                                .id("component-lock")
                                .px_2()
                                .py_0p5()
                                .rounded_sm()
                                .text_sm()
                                .cursor_pointer()
                                .border_1()
                                .when(aligned, |el| {
                                    el.border_color(t::accent())
                                        .text_color(t::accent())
                                })
                                .when(!aligned, |el| {
                                    el.border_color(t::cell_border())
                                        .text_color(t::text())
                                })
                                .child(if aligned { "Locked" } else { "Free" })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_component_alignment(index, ci);
                                    cx.notify();
                                })),
                        ),
                );
            }
        }
        self.section(cx, "Selection", body)
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
        let Some(aligned) = currently_aligned else { return };
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

    /// Round the selected corners into fillets sized like the
    /// glyph's existing rounding.
    fn command_round_corners(&mut self) {
        let Mode::Editor(index) = self.mode else { return };
        self.push_undo_snapshot(index);
        let selected = self.editor.selected.clone();
        let new_selection = self
            .font_mut()
            .and_then(|f| {
                f.edit_glyph(index, |g| {
                    runebender_core::glyph_ops::round_selected_corners(
                        g, &selected,
                    )
                })
            })
            .flatten();
        match new_selection {
            Some(selection) => self.editor.selected = selection,
            None => {
                self.editor.undo.pop();
            }
        }
    }

    /// Glyph → Trace Image…: pick an image, autotrace it through
    /// img2bez (the web editor's tracer), and replace the current
    /// glyph's contours with the result. Undoable.
    fn command_trace_image(&mut self, cx: &mut Context<Self>) {
        let Mode::Editor(index) = self.mode else { return };
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Trace".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let bytes = std::fs::read(&path);
            this.update(cx, |workspace, cx| {
                match bytes {
                    Ok(bytes) => workspace.apply_image_trace(index, &bytes),
                    Err(e) => {
                        workspace.status_note =
                            Some(format!("Trace: {e}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
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

    /// Duplicate the selection: contours holding selected points, or
    /// the selected component or anchor, offset (20, 20), clones
    /// selected (web duplicateSelection).
    fn command_duplicate(&mut self) {
        let Mode::Editor(index) = self.mode else { return };
        self.push_undo_snapshot(index);
        let changed = if let Some(ci) = self.editor.selected_component {
            let new_index = self
                .font_mut()
                .and_then(|f| {
                    f.edit_glyph(index, |g| {
                        runebender_core::glyph_ops::duplicate_component(g, ci)
                    })
                })
                .flatten();
            if let Some(new_index) = new_index {
                self.editor.selected_component = Some(new_index);
            }
            new_index.is_some()
        } else if let Some(ai) = self.editor.selected_anchor() {
            let new_index = self
                .font_mut()
                .and_then(|f| {
                    f.edit_glyph(index, |g| {
                        runebender_core::glyph_ops::duplicate_anchor(g, ai)
                    })
                })
                .flatten();
            if let Some(new_index) = new_index {
                self.editor.selected_anchors = vec![new_index];
            }
            new_index.is_some()
        } else {
            let selected = self.editor.selected.clone();
            let new_selection = self
                .font_mut()
                .and_then(|f| {
                    f.edit_glyph(index, |g| {
                        runebender_core::glyph_ops::duplicate_selection(
                            g, &selected,
                        )
                    })
                })
                .flatten();
            match new_selection {
                Some(selection) => {
                    self.editor.selected = selection;
                    true
                }
                None => false,
            }
        };
        if !changed {
            self.editor.undo.pop();
        }
    }

    /// Duplicate, then re-apply the last flip/rotate — the web's
    /// duplicate-repeat, for rotated repeats around a center.
    fn command_duplicate_repeat(&mut self) {
        let before = self.editor.undo.len();
        self.command_duplicate();
        if self.editor.undo.len() == before {
            return;
        }
        if let Some(transform) = self.editor.last_transform {
            let Mode::Editor(index) = self.mode else { return };
            let selected = self.editor.selected.clone();
            self.font_mut().and_then(|f| {
                f.edit_glyph(index, |g| {
                    runebender_core::glyph_ops::transform_selection(
                        g, &selected, transform,
                    )
                })
            });
        }
    }

    /// Flip/rotate the selection (whole glyph when nothing selected)
    /// about its bbox center, with an undo snapshot.
    fn apply_transform(&mut self, transform: Affine) {
        let Mode::Editor(index) = self.mode else { return };
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
    /// Switch the palette: the app's own colours, the widget library's
    /// theme, and the menu tick all follow.
    fn command_set_theme(
        &mut self,
        id: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !t::set_theme(id) {
            return;
        }
        t::install_component_theme(cx);
        cx.set_menus(app_menus());
        self.status_note = Some(
            format!(
                "{} theme",
                t::THEMES
                    .iter()
                    .find(|(name, _)| *name == id)
                    .map(|(_, label)| *label)
                    .unwrap_or(id)
            )
            .into(),
        );
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

    /// Save every dirty master (native), or PUT modified files to the
    /// workspace server (web).
    fn command_save(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_family = "wasm")]
        {
            self.save_to_web_host(cx);
        }
        #[cfg(not(target_family = "wasm"))]
        {
            let _ = cx;
            if let Some(project) = self.project.as_mut() {
                let mut saved = Vec::new();
                let mut failed = Vec::new();
                for master in project.masters.iter_mut() {
                    if !master.dirty {
                        continue;
                    }
                    match master.save() {
                        Ok(()) => saved.push(master.source_path.display().to_string()),
                        Err(e) => failed.push(format!("{e}")),
                    }
                }
                *self.last_save.lock().unwrap() = web_time::Instant::now();
                self.last_save_label = Some(
                    chrono::Local::now().format("%-I:%M %p").to_string().into(),
                );
                self.status_note = Some(if !failed.is_empty() {
                    format!("Save failed: {}", failed.join("; ")).into()
                } else if saved.is_empty() {
                    "Nothing to save".into()
                } else {
                    format!("Saved {}", saved.join(", ")).into()
                });
            }
        }
    }

    /// Copy the selected contours (whole glyph when nothing selected).
    fn command_copy(&mut self) {
        let in_editor = matches!(self.mode, Mode::Editor(_));
        let index = match self.mode {
            Mode::Editor(i) => Some(i),
            Mode::Grid => self.selected,
        };
        if let (Some(index), Some(font)) = (index, self.font()) {
            let selected = if in_editor {
                self.editor.selected.clone()
            } else {
                Default::default()
            };
            self.clipboard = font.contours_for_copy(index, &selected);
            self.status_note =
                Some(format!("Copied {} contours", self.clipboard.len()).into());
        }
    }

    /// Paste copied contours into the current glyph, with undo.
    fn command_paste(&mut self) {
        let index = match self.mode {
            Mode::Editor(i) => Some(i),
            Mode::Grid => self.selected,
        };
        let Some(index) = index else { return };
        if self.clipboard.is_empty() {
            return;
        }
        self.push_undo_snapshot(index);
        let contours = self.clipboard.clone();
        if let Some(font) = self.font_mut() {
            font.paste_contours(index, &contours);
        }
        if let Some(project) = self.project.as_mut() {
            let name = project.active_font().glyphs[index].name.to_string();
            project.recheck_compat(&name);
        }
    }

    /// Cmd+V, routed the web way: copied contours paste whenever the
    /// outline clipboard holds something and the Text tool isn't the
    /// one in hand; otherwise the system clipboard's text types into
    /// the editor's buffer.
    fn command_paste_routed(&mut self, cx: &mut Context<Self>) {
        let text_target = matches!(self.mode, Mode::Editor(_));
        if (!self.clipboard.is_empty() && self.editor.tool != Tool::Text)
            || !text_target
        {
            self.command_paste();
            return;
        }
        self.paste_text_into_buffer(cx);
    }

    /// Paste the system clipboard's text into the editor's buffer,
    /// character by character (web pasteTextIntoBuffer): switches to
    /// the Text tool, line breaks for newlines, characters with no
    /// glyph skipped.
    fn paste_text_into_buffer(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text())
        else {
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

    /// Remove overlap on the open glyph, with undo.
    fn command_remove_overlap(&mut self) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        self.push_undo_snapshot(index);
        let changed = self.font_mut().is_some_and(|f| f.remove_overlap(index));
        if !changed {
            self.editor.undo.pop();
        } else {
            self.editor.selected.clear();
        }
    }

    /// Boolean path op over the glyph's contours (web boolean tiles):
    /// union merges everything; the others apply first contour vs the
    /// rest combined.
    fn command_boolean(&mut self, op: linesweeper::BinaryOp) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        self.push_undo_snapshot(index);
        let changed = self
            .font_mut()
            .and_then(|f| {
                f.edit_glyph(index, |g| {
                    match runebender_core::glyph_ops::boolean_contours(g, op) {
                        Some(contours) => {
                            g.contours = contours;
                            true
                        }
                        None => false,
                    }
                })
            })
            .unwrap_or(false);
        if !changed {
            self.editor.undo.pop();
        } else {
            self.editor.selected.clear();
        }
    }

    /// Make the selected on-curve point the contour's start point.
    fn command_set_start_point(&mut self) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        if self.editor.selected.len() != 1 {
            return;
        }
        let (contour, point) = *self.editor.selected.iter().next().unwrap();
        self.push_undo_snapshot(index);
        let changed = self
            .font_mut()
            .and_then(|f| {
                f.edit_glyph(index, |g| {
                    runebender_core::glyph_ops::set_contour_start(g, contour, point)
                })
            })
            .unwrap_or(false);
        if !changed {
            self.editor.undo.pop();
        } else {
            self.editor.selected = [(contour, 0)].into();
        }
    }

    /// Tab / shift-Tab: step the point selection through the glyph's
    /// points in contour order (web cycle_selected_point). Bound as an
    /// action so gpui's default tab-stop traversal never runs.
    fn command_cycle_point(&mut self, back: bool) -> bool {
        let Mode::Editor(index) = self.mode else {
            return false;
        };
        let ids: Vec<(usize, usize)> = self
            .font()
            .map(|f| {
                f.glyphs[index]
                    .points
                    .iter()
                    .map(|p| (p.contour, p.index))
                    .collect()
            })
            .unwrap_or_default();
        if ids.is_empty() {
            return false;
        }
        let positions: Vec<usize> = ids
            .iter()
            .enumerate()
            .filter(|(_, id)| self.editor.selected.contains(id))
            .map(|(i, _)| i)
            .collect();
        let target = if positions.is_empty() {
            if back { ids.len() - 1 } else { 0 }
        } else if back {
            let first = positions[0];
            if first == 0 { ids.len() - 1 } else { first - 1 }
        } else {
            (positions[positions.len() - 1] + 1) % ids.len()
        };
        self.editor.selected_component = None;
        self.editor.selected = [ids[target]].into();
        true
    }

    /// Reverse the selected contours (all when none selected), undo.
    fn command_reverse(&mut self) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        self.push_undo_snapshot(index);
        let selected = self.editor.selected.clone();
        let changed = self
            .font_mut()
            .and_then(|f| {
                f.edit_glyph(index, |g| {
                    runebender_core::glyph_ops::reverse_contours(g, &selected)
                })
            })
            .unwrap_or(false);
        if !changed {
            self.editor.undo.pop();
        } else {
            self.editor.selected.clear();
        }
    }

    /// Step to the next/previous master (menu: View).
    fn command_step_master(&mut self, delta: isize) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let n = project.masters.len() as isize;
        if n < 2 {
            return;
        }
        let next = (project.active as isize + delta).rem_euclid(n) as usize;
        self.switch_master(next);
    }

    /// Decompose the open glyph's components, with undo.
    fn command_decompose(&mut self) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        self.push_undo_snapshot(index);
        let changed = self.font_mut().is_some_and(|f| f.decompose(index));
        if !changed {
            self.editor.undo.pop();
        }
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
                        runebender_core::glyph_ops::translate_component(
                            g, ci, dx, dy,
                        )
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
            && let Some((x, y)) =
                font.glyphs[index].anchors.get(ai).map(|(_, x, y)| (*x, *y))
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

    /// The Glyphs-style tab strip under the header: a Font tab that
    /// returns to the full glyph overview, plus one tab per edit
    /// session, titled with the session's text.
    fn tab_strip(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        if self.project.is_none() {
            return div().into_any_element();
        }
        let in_editor = matches!(self.mode, Mode::Editor(_));
        let tab = |id: gpui::ElementId, label: SharedString, active: bool| {
            div()
                .id(id)
                .h(px(TAB_H))
                .px_2()
                .flex()
                .items_center()
                .rounded_sm()
                .text_sm()
                .cursor_pointer()
                .when(active, |el| {
                    el.border_1()
                        .border_color(t::accent())
                        .text_color(t::accent())
                })
                .when(!active, |el| {
                    el.border_1()
                        .border_color(t::cell_border())
                        .text_color(t::text_muted())
                })
                .child(label)
        };
        // Each session tab reads like Glyphs: the buffer's text, with
        // /name for unencoded glyphs, trimmed to fit.
        let session_label = |buffer: &runebender_core::text::TextBuffer,
                             fallback: &str|
         -> SharedString {
            let mut label = String::new();
            for i in 0..buffer.len() {
                let Some(sort) = buffer.sort(i) else {
                    continue;
                };
                if sort.is_absorbed() {
                    continue;
                }
                match &sort.kind {
                    runebender_core::text::TextSortKind::Glyph {
                        codepoint,
                        name,
                        ..
                    } => match codepoint {
                        Some(c) => label.push(*c),
                        None => {
                            label.push('/');
                            label.push_str(name);
                        }
                    },
                    _ => label.push(' '),
                }
                if label.chars().count() > 24 {
                    label.truncate(
                        label
                            .char_indices()
                            .nth(24)
                            .map(|(i, _)| i)
                            .unwrap_or(label.len()),
                    );
                    label.push('…');
                    break;
                }
            }
            if label.is_empty() {
                label = fallback.to_string();
            }
            label.into()
        };
        let labels: Vec<SharedString> = self
            .sessions
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let fallback: String = if i == self.active_session {
                    match self.mode {
                        Mode::Editor(index) => self
                            .font()
                            .map(|f| f.glyphs[index].name.to_string())
                            .unwrap_or_default(),
                        Mode::Grid => s.glyph_name.clone(),
                    }
                } else {
                    s.glyph_name.clone()
                };
                if i == self.active_session {
                    session_label(&self.edit_buffer, &fallback)
                } else {
                    session_label(&s.buffer, &fallback)
                }
            })
            .collect();
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(tab("tab-font".into(), "Font".into(), !in_editor).on_click(
                cx.listener(|this, _, _, cx| {
                    if let Mode::Editor(index) = this.mode {
                        this.last_editor = Some(index);
                        let name = this
                            .font()
                            .map(|f| f.glyphs[index].name.to_string());
                        if let (Some(name), Some(project)) =
                            (name, this.project.as_mut())
                        {
                            project.recheck_compat(&name);
                        }
                        this.mode = Mode::Grid;
                        this.status_note = None;
                        cx.notify();
                    }
                }),
            ))
            .children(labels.into_iter().enumerate().map(|(i, label)| {
                let active = in_editor && i == self.active_session;
                tab(("tab-session", i).into(), label, active)
                    .flex()
                    .items_center()
                    .gap_1()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // Return to the session as it was left: same
                        // buffer, tool, undo stack.
                        this.activate_session(i);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .id(("tab-close", i))
                            .px_0p5()
                            .rounded_sm()
                            .text_color(t::text_muted())
                            .hover(|el| el.text_color(t::text()))
                            .child("×")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_session(i);
                                cx.notify();
                            })),
                    )
            }))
            .child(
                tab("tab-new".into(), "+".into(), false)
                    .w(px(TAB_H))
                    .justify_center()
                    .on_click(cx.listener(
                    |this, _, _, cx| {
                        this.command_new_session();
                        cx.notify();
                    },
                )),
            )
            .into_any_element()
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let (title, status): (SharedString, SharedString) =
            match (self.font(), &self.load_error) {
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
            .gap_3()
            // Even margins: the strip sits the same distance from the
            // window's sides as from its top.
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
                    .rounded_md()
                    .cursor_pointer()
                    .child(icon_svg("glyph-grid", if self.left_collapsed {
                        t::text_muted()
                    } else {
                        t::text()
                    }))
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
                    .child(
                        div()
                            .text_sm()
                            .text_color(t::status_yellow())
                            .child(status),
                    ),
            )
            .when(
                in_editor && self.editor.tool == Tool::Text,
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
            // NOTE: .max() must precede .min(): SliderState starts at
            // max=100 and each setter clamps the current value, so a
            // min above 100 panics (f32::clamp with min > max).
            let slider = cx.new(|_| {
                gpui_component::slider::SliderState::new()
                    .max(axis.max as f32)
                    .min(axis.min as f32)
                    .step(1.0)
                    .default_value(axis.default as f32)
            });
            let axis_info = axis.clone();
            let sub = cx.subscribe_in(&slider, window, {
                move |this: &mut Workspace,
                      _,
                      event: &gpui_component::slider::SliderEvent,
                      _window,
                      cx| {
                    let gpui_component::slider::SliderEvent::Change(value) = event else {
                        return;
                    };
                    let raw = value.start() as f64;
                    if let Some(project) = this.project.as_mut() {
                        project.location.insert(
                            axis_info.name.clone(),
                            runebender_core::var_model::normalize_value(
                                raw,
                                axis_info.min,
                                axis_info.default,
                                axis_info.max,
                            ),
                        );
                    }
                    cx.notify();
                }
            });
            self._subscriptions.push(sub);
            self.axis_sliders.push((i, slider));
        }
    }

    /// Axis slider row (designspaces only).
    /// Axes section for a sidebar: one labeled slider per designspace
    /// axis (the web/Glyphs place these in a pane, not a full-width
    /// strip).
    fn axes_section(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let project = self.project.as_ref()?;
        if self.axis_sliders.is_empty() {
            return None;
        }
        let mut rows = div().flex().flex_col().gap_2();
        for (axis_index, slider) in &self.axis_sliders {
            let Some(axis) = project.axes.get(*axis_index) else {
                continue;
            };
            rows = rows.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(t::text_muted())
                            .child(axis.tag.clone()),
                    )
                    .child(
                        div().flex_1().child(flat_slider(slider, cx)),
                    ),
            );
        }
        Some(self.section(cx, "Axes", rows))
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
        let inventory =
            runebender_core::text::TextGlyphInventory::from_font(&font.font);
        let kerning = runebender_core::text::TextKerningModel::from_font(&font.font);
        let edit_widths: Vec<(usize, String, Option<char>, f64)> = (0..self
            .edit_buffer
            .len())
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
                runebender_core::glyph_ops::component_at(
                    &font.font,
                    g,
                    kurbo::Point::new(dx, dy),
                )
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
        let Some(activation) =
            self.edit_buffer
                .activate_sort_at(bx, by, line_height, top, bottom)
        else {
            return false;
        };
        let name = self
            .edit_buffer
            .sort(activation.index)
            .and_then(|s| s.glyph_name())
            .map(str::to_string);
        let target = name.and_then(|n| {
            self.font().and_then(|f| f.name_map.get(&n).copied())
        });
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
                .filter_map(|(first, seconds): (norad::Name, std::collections::BTreeMap<norad::Name, f64>)| {
                    Some((first, seconds))
                })
                .collect();
            font.kerning_dirty = true;
            font.dirty = true;
        }
        self.rebuild_text_models();
    }

    /// A key while the editor's text tool is active. Typing composes
    /// text around the open glyph; the open glyph follows the active
    /// sort.
    fn handle_edit_text_key(&mut self, event: &gpui::KeyDownEvent) -> bool {
        let key = event.keystroke.key.as_str();
        if self.font().is_none() {
            return false;
        }
        let line_height = self.text_line_height();
        let handled = match key {
            "backspace" => {
                self.edit_buffer.delete_before_cursor();
                let cursor = self.edit_buffer.cursor();
                self.edit_buffer.shape_arabic_around_if_rtl(cursor);
                true
            }
            "delete" => {
                self.edit_buffer.delete_after_cursor();
                let cursor = self.edit_buffer.cursor();
                self.edit_buffer.shape_arabic_around_if_rtl(cursor);
                true
            }
            "left" => {
                self.edit_buffer.move_cursor_visual_left();
                true
            }
            "right" => {
                self.edit_buffer.move_cursor_visual_right();
                true
            }
            "up" => self.edit_buffer.move_cursor_vertically(-1, line_height),
            "down" => self.edit_buffer.move_cursor_vertically(1, line_height),
            "home" => {
                self.edit_buffer.move_cursor_to_line_edge(false);
                true
            }
            "end" => {
                self.edit_buffer.move_cursor_to_line_edge(true);
                true
            }
            "enter" => {
                self.edit_buffer.insert_line_break();
                true
            }
            "escape" => {
                // The web editor keeps text mode alive on Escape.
                true
            }
            _ => {
                let Some(text) = event.keystroke.key_char.as_deref() else {
                    return true;
                };
                let mut inserted = false;
                for c in text.chars() {
                    if c.is_control() {
                        continue;
                    }
                    // Typing the active sort's own character reuses its
                    // live (possibly just edited) advance width, like
                    // the web editor.
                    let active_advance = self
                        .edit_buffer
                        .active_sort()
                        .and_then(|i| self.edit_buffer.sort(i))
                        .and_then(|sort| match &sort.kind {
                            runebender_core::text::TextSortKind::Glyph {
                                codepoint,
                                ..
                            } => *codepoint,
                            _ => None,
                        })
                        .filter(|active_char| *active_char == c)
                        .and_then(|_| {
                            let Mode::Editor(index) = self.mode else {
                                return None;
                            };
                            self.font().map(|f| f.glyphs[index].advance)
                        });
                    inserted |= self
                        .edit_buffer
                        .insert_character_with_active_advance(c, active_advance);
                }
                inserted
            }
        };
        if handled {
            self.sync_sort_offset();
        }
        true
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

    fn preview_strip(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let Some(font) = self.font() else {
            return div().into_any_element();
        };
        let ascender = font.ascender;
        let descender = font.descender;
        let upm = font.units_per_em;
        let line_height = self.text_line_height();
        let layout = self.edit_buffer.layout(line_height);
        // Each sort's outline, its pen position, and its advance, so
        // the line can be measured and centered.
        let items: Vec<(Arc<BezPath>, f64, f64, f64)> = layout
            .items
            .iter()
            .filter_map(|item| {
                let sort = self.edit_buffer.sort(item.index)?;
                if sort.is_absorbed() {
                    return None;
                }
                let name = sort.glyph_name()?;
                let glyph = *font.name_map.get(name)?;
                Some((
                    font.glyphs[glyph].path.clone(),
                    item.x,
                    item.y,
                    font.glyphs[glyph].advance,
                ))
            })
            .collect();
        let line_width = items
            .iter()
            .map(|(_, x, _, adv)| x + adv)
            .fold(0.0_f64, f64::max);
        // The line's ink, in design units relative to the first
        // baseline: what the preview centres on.
        let ink_extent: Option<(f64, f64)> = {
            use kurbo::Shape as _;
            let mut extent: Option<(f64, f64)> = None;
            for (path, _, y, _) in items.iter() {
                if path.elements().is_empty() {
                    continue;
                }
                let b = path.bounding_box();
                let (top, bottom) = (b.y1 + y, b.y0 + y);
                extent = Some(match extent {
                    Some((t, bo)) => (t.max(top), bo.min(bottom)),
                    None => (top, bottom),
                });
            }
            extent
        };

        let blur = self.preview_blur;
        let blur_cache = self.preview_blur_cache.clone();
        let invert = self.preview_invert;

        let body = div().size_full().min_h(px(0.0)).child(
            canvas(
                move |bounds, _, _| bounds,
                move |_, bounds: Bounds<gpui::Pixels>, window, _| {
                    let w: f64 = f32::from(bounds.size.width) as f64;
                    let h: f64 = f32::from(bounds.size.height) as f64;
                    let (ink, ground) = if invert {
                        (t::window_bg(), t::preview_glyph())
                    } else {
                        (t::preview_glyph(), t::panel_bg())
                    };
                    window.paint_quad(gpui::fill(bounds, ground));
                    // The type fits the pane, the way Glyphs and the
                    // web preview do it: one scale that fits vertically
                    // and the whole line horizontally, whichever is
                    // tighter. Drag the pane taller and the text grows
                    // with it.
                    //
                    // The em box is the wrong thing to centre on: for
                    // "8" the descender depth is empty, so centring the
                    // box leaves the ink riding high. Centre the ink
                    // the line actually has instead, which also keeps a
                    // deep Arabic descender in the middle of the pane
                    // rather than hanging off the bottom. The em box is
                    // the fallback when there is no ink at all.
                    let pad = 16.0;
                    let (ink_top, ink_bottom) = ink_extent
                        .unwrap_or((ascender, descender));
                    let ink_h = (ink_top - ink_bottom).max(1.0);
                    let by_height = (h - pad * 2.0).max(1.0) / ink_h;
                    let by_width = if line_width > 0.0 {
                        (w - pad * 2.0).max(1.0) / line_width
                    } else {
                        by_height
                    };
                    let scale = by_height.min(by_width);
                    // Baseline placed so the ink's own middle lands on
                    // the pane's middle.
                    let baseline = h / 2.0 + (ink_top + ink_bottom) / 2.0 * scale;
                    let text_w = line_width * scale;
                    let origin_x = (w - text_w) / 2.0;
                    let _ = (upm, ascender, descender);
                    // gpui paints paths, not filters, so a blur is a
                    // stack of offset passes: one ring plus the middle,
                    // each at a fraction of the ink's alpha.
                    // One path for the whole line, in the pane's own
                    // pixel space.
                    let mut line = BezPath::new();
                    for (path, x, y, _) in items.iter() {
                        let transform = Affine::translate((
                            origin_x + x * scale,
                            baseline - y * scale,
                        )) * Affine::scale_non_uniform(scale, -scale);
                        line.extend(
                            (transform * path.as_ref().clone()).into_iter(),
                        );
                    }
                    if blur > 0.05 {
                        // Rasterized and blurred for real: gpui has no
                        // blur for paths, and stacking offset copies
                        // reads as ghosting rather than defocus.
                        let key = blur_key(&line, w, h, blur, ink, ground);
                        let cached = {
                            let slot = blur_cache.lock().unwrap();
                            slot.as_ref()
                                .filter(|(k, _)| *k == key)
                                .map(|(_, image)| image.clone())
                        };
                        let image = cached.or_else(|| {
                            let image = blur::blurred_line(
                                &line,
                                w as f32,
                                h as f32,
                                window.scale_factor(),
                                ink,
                                ground,
                                blur,
                            )?;
                            *blur_cache.lock().unwrap() =
                                Some((key, image.clone()));
                            Some(image)
                        });
                        if let Some(image) = image {
                            let _ = window.paint_image(
                                bounds,
                                bounds,
                                gpui::Corners::default(),
                                image,
                                0,
                                false,
                            );
                            return;
                        }
                    }
                    if let Some(p) =
                        build_fill_path(&line, Affine::IDENTITY, bounds.origin)
                    {
                        window.paint_path(p, ink);
                    }
                },
            )
            .size_full(),
        );

        let _ = cx;
        div()
            .size_full()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .bg(t::panel_bg())
            .border_t_1()
            .border_color(t::cell_border())
            .child(body)
            .into_any_element()
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

    /// Add an empty glyph to every master (bottom bar +), like
    /// Glyphs' new-glyph command, and select it.
    fn command_add_glyph(&mut self) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        // First free name: glyph, glyph.001, glyph.002, ...
        let taken: std::collections::HashSet<String> = project
            .masters
            .iter()
            .flat_map(|m| m.name_map.keys().cloned())
            .collect();
        let mut name = "glyph".to_string();
        let mut counter = 0;
        while taken.contains(&name) {
            counter += 1;
            name = format!("glyph.{counter:03}");
        }
        let upm = project.active_font().units_per_em;
        for master in project.masters.iter_mut() {
            master.add_glyph(&name, (upm * 0.5).round());
        }
        let name_owned = name.clone();
        project.recheck_compat(&name_owned);
        self.selected = self
            .font()
            .and_then(|f| f.name_map.get(&name).copied());
        self.sidebar_counts = None;
        self.status_note = Some(format!("Added {name}").into());
    }

    /// Remove the selected glyph from every master (bottom bar −).
    fn command_remove_glyph(&mut self) {
        let Some(index) = self.selected else {
            self.status_note = Some("Select a glyph to remove".into());
            return;
        };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string())
        else {
            return;
        };
        if let Some(project) = self.project.as_mut() {
            for master in project.masters.iter_mut() {
                master.remove_glyph(&name);
            }
        }
        self.selected = None;
        self.sidebar_counts = None;
        self.status_note = Some(format!("Removed {name}").into());
    }

    /// Create the bottom bar's cell-size slider once a window exists.
    fn ensure_preview_slider(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.preview_blur_slider.is_some() {
            return;
        }
        let slider = cx.new(|_| {
            gpui_component::slider::SliderState::new()
                .max(12.0)
                .min(0.0)
                .step(0.5)
                .default_value(0.0)
        });
        let sub = cx.subscribe_in(&slider, window, {
            move |this: &mut Workspace,
                  _,
                  event: &gpui_component::slider::SliderEvent,
                  _window,
                  cx| {
                let gpui_component::slider::SliderEvent::Change(value) = event
                else {
                    return;
                };
                this.preview_blur = value.start();
                cx.notify();
            }
        });
        self._subscriptions.push(sub);
        self.preview_blur_slider = Some(slider);
    }

    fn ensure_sidebar_slider(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sidebar_slider.is_some() {
            return;
        }
        let slider = cx.new(|_| {
            gpui_component::slider::SliderState::new()
                .max(120.0)
                .min(24.0)
                .step(2.0)
                .default_value(MINI_CELL)
        });
        let sub = cx.subscribe_in(&slider, window, {
            move |this: &mut Workspace,
                  _,
                  event: &gpui_component::slider::SliderEvent,
                  _window,
                  cx| {
                let gpui_component::slider::SliderEvent::Change(value) = event
                else {
                    return;
                };
                this.sidebar_cell_size = value.start();
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
            gpui_component::slider::SliderState::new()
                .max(200.0)
                .min(48.0)
                .step(4.0)
                .default_value(CELL)
        });
        let sub = cx.subscribe_in(&slider, window, {
            move |this: &mut Workspace,
                  _,
                  event: &gpui_component::slider::SliderEvent,
                  _window,
                  cx| {
                let gpui_component::slider::SliderEvent::Change(value) = event
                else {
                    return;
                };
                this.grid_cell_size = value.start();
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
            let _query = self.search_query.clone();
            let shown = self
                .font()
                .map(|f| {
                    f.glyphs
                        .iter()
                        .filter(|entry| {
                            self.sidebar_matches
                                .as_ref()
                                .is_none_or(|m| m.contains(entry.name.as_ref()))
                                && self.search_matches(
                                    entry.name.as_ref(),
                                    entry.codepoint,
                                )
                        })
                        .count()
                })
                .unwrap_or(0);
            let center: SharedString = match &self.status_note {
                Some(note) => note.clone(),
                None => format!(
                    "{} selected · {shown}/{total} glyphs",
                    usize::from(self.selected.is_some())
                )
                .into(),
            };
            let bar_button = |id: &'static str, label: &'static str| {
                div()
                    .id(id)
                    .w(px(BAR_BUTTON))
                    .h(px(BAR_BUTTON))
                    .rounded_sm()
                    .border_1()
                    .border_color(t::cell_border())
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(t::text())
                    .cursor_pointer()
                    .child(label)
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
                .child(bar_button("add-glyph", "+").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.command_add_glyph();
                        cx.notify();
                    },
                )))
                .child(bar_button("remove-glyph", "−").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.command_remove_glyph();
                        cx.notify();
                    },
                )))
                .child(
                    div()
                        .flex_1()
                        .text_center()
                        .text_sm()
                        .text_color(t::text_muted())
                        .child(center),
                )
                .children(self.cell_slider.as_ref().map(|slider| {
                    div().w(px(140.0)).child(flat_slider(slider, cx))
                }));
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
                        None => {
                            format!("{} · unencoded · advance {}", g.name, g.advance).into()
                        }
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
            .children(
                matches!(self.mode, Mode::Editor(_))
                    .then(|| self.preview_toggle(cx)),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .text_sm()
                    .text_color(t::text_muted())
                    .child(text),
            )
            .children(
                matches!(self.mode, Mode::Editor(_))
                    .then(|| self.preview_controls(cx)),
            )
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
        let mut watcher = match notify::recommended_watcher(
            move |res: Result<notify::Event, notify::Error>| {
                if res.is_ok() {
                    let _ = tx.unbounded_send(());
                }
            },
        ) {
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
                if last_save.lock().unwrap().elapsed()
                    < std::time::Duration::from_secs(2)
                {
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
                        workspace.status_note =
                            Some(format!("Connected · {n} masters · Cmd+S saves to the server").into());
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
            self.status_note =
                Some("No server connected: open with ?server=http://…".into());
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
                match web_host::put_file(&client, &base, file, etags.get(&file.path).map(|s| s.as_str()))
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

    /// Cmd+O: native open dialog for a .designspace, .ufo, or folder.
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

    fn handle_key(&mut self, event: &gpui::KeyDownEvent, _cx: &mut Context<Self>) -> bool {
        let key = event.keystroke.key.as_str();
        let cmd = event.keystroke.modifiers.platform;
        let shift = event.keystroke.modifiers.shift;
        let in_editor = matches!(self.mode, Mode::Editor(_));
        let ctrl = event.keystroke.modifiers.control;
        let alt = event.keystroke.modifiers.alt;
        // Web nudge steps: 2 design units, 8 with shift, 32 with ctrl
        // (grid-sized moves).
        let step = if ctrl {
            32.0
        } else if shift {
            8.0
        } else {
            2.0
        };
        match (key, cmd) {
            ("escape", _) if in_editor => {
                if self.context_menu.is_some() {
                    self.context_menu = None;
                } else if self.editor.pen.is_some() || self.editor.hyper_contour.is_some() {
                    self.pen_finish();
                } else {
                    let Mode::Editor(index) = self.mode else {
                        return false;
                    };
                    let name = self
                        .font()
                        .map(|f| f.glyphs[index].name.to_string());
                    if let (Some(name), Some(project)) = (name, self.project.as_mut()) {
                        project.recheck_compat(&name);
                    }
                    if let Mode::Editor(index) = self.mode {
                        self.last_editor = Some(index);
                    }
                    self.mode = Mode::Grid;
                    self.status_note = None;
                }
                true
            }
            ("enter", _)
                if in_editor
                    && (self.editor.pen.is_some()
                        || self.editor.hyper_contour.is_some()) =>
            {
                self.pen_finish();
                true
            }
            ("enter", false) if !in_editor => {
                if let Some(index) = self.selected {
                    self.open_editor(index);
                    true
                } else {
                    false
                }
            }
            (_, false) if in_editor && self.editor.tool == Tool::Text => {
                self.handle_edit_text_key(event)
            }
            ("v", false) if in_editor => {
                self.pen_finish();
                self.editor.tool = Tool::Select;
                true
            }
            ("p", false) if in_editor => {
                self.editor.tool = Tool::Pen;
                true
            }
            ("r", false) if in_editor => {
                if self.editor.tool == Tool::Shapes {
                    self.editor.shape_ellipse = !self.editor.shape_ellipse;
                }
                self.pen_finish();
                self.editor.tool = Tool::Shapes;
                true
            }
            ("m", false) if in_editor => {
                self.pen_finish();
                self.editor.tool = Tool::Measure;
                true
            }
            ("t", false) if in_editor => {
                self.pen_finish();
                self.editor.tool = Tool::Text;
                true
            }
            ("k", false) if in_editor => {
                self.pen_finish();
                self.editor.tool = Tool::Knife;
                true
            }
            ("h", false) if in_editor => {
                self.pen_finish();
                self.editor.tool = Tool::HyperPen;
                true
            }
            ("space", false) if in_editor => {
                // Hold space for the filled preview, like the web
                // editor; releasing returns to the previous tool.
                if self.editor.tool != Tool::Preview {
                    self.editor.previous_tool = self.editor.tool;
                    self.editor.tool = Tool::Preview;
                }
                true
            }
            ("a", false) if in_editor => {
                let Mode::Editor(index) = self.mode else {
                    return false;
                };
                self.push_undo_snapshot(index);
                let (cx_, cy_) = self.editor.cursor;
                if let Some(font) = self.font_mut() {
                    font.add_anchor(index, cx_.round(), cy_.round());
                }
                true
            }
            ("backspace", false)
                if in_editor
                    && (self.editor.pen.is_some()
                        || self.editor.hyper_contour.is_some()) =>
            {
                let Mode::Editor(index) = self.mode else {
                    return false;
                };
                let contour = self
                    .editor
                    .pen
                    .as_ref()
                    .map(|p| p.contour)
                    .or(self.editor.hyper_contour)
                    .unwrap();
                let remaining = self
                    .font_mut()
                    .and_then(|f| {
                        f.edit_glyph(index, |g| {
                            runebender_core::segment_ops::delete_last_pen_point(
                                g, contour,
                            )
                        })
                    })
                    .flatten();
                if remaining == Some(0) {
                    if let Some(font) = self.font_mut() {
                        font.remove_contour_if_degenerate(index, contour);
                    }
                    self.editor.pen = None;
                    self.editor.hyper_contour = None;
                }
                remaining.is_some()
            }
            ("backspace" | "delete", false)
                if in_editor && self.editor.selected_component.is_some() =>
            {
                let Mode::Editor(index) = self.mode else {
                    return false;
                };
                let ci = self.editor.selected_component.take().unwrap();
                self.push_undo_snapshot(index);
                let changed = self
                    .font_mut()
                    .and_then(|f| {
                        f.edit_glyph(index, |g| {
                            runebender_core::glyph_ops::delete_component(g, ci)
                        })
                    })
                    .unwrap_or(false);
                if !changed {
                    self.editor.undo.pop();
                }
                changed
            }
            ("backspace" | "delete", false) if in_editor && !self.editor.selected_anchors.is_empty() => {
                let Mode::Editor(index) = self.mode else {
                    return false;
                };
                let mut anchors =
                    std::mem::take(&mut self.editor.selected_anchors);
                anchors.sort_unstable();
                self.push_undo_snapshot(index);
                if let Some(font) = self.font_mut() {
                    // Highest index first: deleting shifts the ones
                    // after it.
                    for ai in anchors.into_iter().rev() {
                        font.delete_anchor(index, ai);
                    }
                }
                true
            }
            ("backspace" | "delete", false) if in_editor => {
                if self.editor.selected.is_empty() {
                    false
                } else {
                    let Mode::Editor(index) = self.mode else {
                        return false;
                    };
                    self.push_undo_snapshot(index);
                    let selected = self.editor.selected.clone();
                    let changed = self
                        .font_mut()
                        .is_some_and(|f| f.delete_points(index, &selected));
                    if changed {
                        self.editor.selected.clear();
                    }
                    changed
                }
            }
            ("s", false) if in_editor => {
                let Mode::Editor(index) = self.mode else {
                    return false;
                };
                if self.editor.selected.is_empty() {
                    false
                } else {
                    self.push_undo_snapshot(index);
                    let selected = self.editor.selected.clone();
                    self.font_mut()
                        .is_some_and(|f| f.toggle_smooth(index, &selected))
                }
            }
            ("left", false) if in_editor => self.nudge_selection(-step, 0.0, alt),
            ("right", false) if in_editor => self.nudge_selection(step, 0.0, alt),
            ("up", false) if in_editor => self.nudge_selection(0.0, step, alt),
            ("down", false) if in_editor => self.nudge_selection(0.0, -step, alt),
            ("+" | "=", false) if in_editor => {
                let zoom = self.editor.viewport.zoom;
                self.editor.viewport.zoom = (zoom * ZOOM_KEY_STEP).min(ZOOM_MAX);
                true
            }
            ("-" | "_", false) if in_editor => {
                let zoom = self.editor.viewport.zoom;
                self.editor.viewport.zoom = (zoom / ZOOM_KEY_STEP).max(ZOOM_MIN);
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
        // Claim focus only when nothing else has it, so text inputs
        // (the search box) keep theirs while focused.
        if window.focused(cx).is_none() {
            window.focus(&self.focus_handle, cx);
        }

        self.ensure_axis_sliders(window, cx);
        self.ensure_cell_slider(window, cx);
        self.ensure_sidebar_slider(window, cx);
        self.ensure_preview_slider(window, cx);
        if self.sidebar_counts.is_none() && self.project.is_some() {
            self.rebuild_sidebar_cache();
        }
        self.refresh_metric_inputs(false, window, cx);
        if matches!(self.mode, Mode::Editor(_)) {
            self.refresh_coord_inputs(false, window, cx);
        }
        self.refresh_glyph_inputs(false, window, cx);
        use gpui_component::resizable::{h_resizable, resizable_panel, v_resizable};

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
                        div()
                            .flex_1()
                            .min_h(px(0.0))
                            .child(
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
                let matches = self.sidebar_matches.clone();
                let sort_unicode = self.sort_unicode;
                let fit = self.grid_cell_metrics();
                let (cell_w, cell_h) = (fit.cell_w, fit.cell_h);
                let mut rows_total = 0usize;
                let grid: Vec<_> = match self.font() {
                    Some(font) => {
                        let mut indices: Vec<usize> = (0..font.glyphs.len())
                            .filter(|&i| {
                                let entry = &font.glyphs[i];
                                matches
                                    .as_ref()
                                    .is_none_or(|m| m.contains(entry.name.as_ref()))
                                    && self.search_matches(
                                        entry.name.as_ref(),
                                        entry.codepoint,
                                    )
                            })
                            .collect();
                        if !sort_unicode {
                            // Font order is already unicode order, so
                            // the Name toggle sorts alphabetically.
                            indices
                                .sort_by_key(|&i| font.glyphs[i].name.clone());
                        }
                        rows_total = indices.len().div_ceil(fit.cols);
                        // Only the rows on screen are built: the view
                        // starts at a row boundary and holds exactly
                        // the rows that fit, so nothing is ever half
                        // drawn at either edge.
                        let start = self
                            .grid_scroll_row
                            .min(rows_total.saturating_sub(1))
                            * fit.cols;
                        indices
                            .into_iter()
                            .skip(start)
                            .take(fit.cols * fit.rows)
                            .map(|i| {
                                self.glyph_cell_sized(
                                    i, cell_w, cell_h, false, cx,
                                )
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
                .size_full();
                (
                    self.category_sidebar(cx).into_any_element(),
                    div()
                        .size_full()
                        .min_h(px(0.0))
                        .flex()
                        .flex_col()
                        .child(
                            div()
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
                                )),
                        )
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
                el.child(self.navigate_section(cx))
                    .child(self.glyph_info_panel(cx))
                    .child(self.selection_section(cx))
                    .children(self.measure_section(cx))
                    .child(self.transform_section(cx))
                    .child(self.curves_section(cx))
                    .child(self.background_section(cx))
                    .child(self.layers_section(cx))
                    .children(self.axes_section(cx))
            })
            .when(!in_editor, |el| {
                el.child(self.glyph_info_panel(cx))
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
                            .child(
                                div().size_full().bg(t::panel_bg()).child(right),
                            ),
                    ),
            )
            .into_any_element();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(t::window_bg())
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
            .on_action(cx.listener(|this, _: &CopyContours, _, cx| {
                this.command_copy();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &PasteContours, _, cx| {
                this.command_paste_routed(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &CopySelectedGlyphs, _, cx| {
                this.command_copy_selection_text(cx);
                cx.notify();
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
                                runebender_core::glyph_ops::convert_hyper_to_cubic(
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
            .on_action(cx.listener(|this, _: &RoundCorners, _, cx| {
                this.command_round_corners();
                cx.notify();
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
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if this.handle_key(event, cx) {
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
                if event.keystroke.key.as_str() == "space"
                    && this.editor.tool == Tool::Preview
                {
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
    let app = gpui_platform::application().with_assets(gpui_component_assets::Assets);
    #[cfg(target_family = "wasm")]
    let app = gpui_platform::single_threaded_web()
        .with_assets(gpui_component_assets::Assets::default());
    let launch = move |cx: &mut App| {
        gpui_component::init(cx);
        t::install_component_theme(cx);

        // The keymap for app commands; menu items show these as their
        // key equivalents.
        cx.bind_keys([
            gpui::KeyBinding::new("cmd-o", OpenFont, None),
            gpui::KeyBinding::new("cmd-n", NewFont, None),
            gpui::KeyBinding::new("cmd-shift-s", SaveFontAs, None),
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
            gpui::KeyBinding::new("cmd-shift-r", ReverseContours, None),
            gpui::KeyBinding::new("cmd-0", ZoomToFit, None),
            gpui::KeyBinding::new("cmd-d", DuplicateSelection, None),
            gpui::KeyBinding::new("cmd-shift-t", DuplicateRepeat, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());

        // One menu definition, three consumers: the macOS native bar
        // (set_menus), the stored menus on Windows/Linux, and the
        // in-window bar drawn where no native bar exists.
        #[cfg(not(target_family = "wasm"))]
        cx.set_menus(app_menus());
        gpui_component::GlobalState::global_mut(cx).set_app_menus(
            app_menus().into_iter().map(|menu| menu.owned()).collect(),
        );
        #[cfg(not(target_os = "macos"))]
        let app_menu_bar = gpui_component::menu::AppMenuBar::new(cx);

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
                        gpui_component::input::InputState::new(window, cx)
                            .placeholder("Search glyphs")
                    });
                    let metric = |cx: &mut Context<Workspace>, window: &mut Window| {
                        cx.new(|cx| gpui_component::input::InputState::new(window, cx))
                    };
                    let width_input = metric(cx, window);
                    let lsb_input = metric(cx, window);
                    let rsb_input = metric(cx, window);
                    let x_input = metric(cx, window);
                    let y_input = metric(cx, window);
                    let w_input = metric(cx, window);
                    let h_input = metric(cx, window);
                    let metric_sub = |cx: &mut Context<Workspace>,
                                      window: &mut Window,
                                      state: &gpui::Entity<gpui_component::input::InputState>,
                                      which: MetricField| {
                        let state = state.clone();
                        cx.subscribe_in(&state, window, {
                            let state = state.clone();
                            move |this: &mut Workspace,
                                  _,
                                  ev: &gpui_component::input::InputEvent,
                                  window,
                                  cx| {
                                if matches!(
                                    ev,
                                    gpui_component::input::InputEvent::PressEnter { .. }
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
                                     state: &gpui::Entity<gpui_component::input::InputState>,
                                     is_x: bool| {
                        let state = state.clone();
                        cx.subscribe_in(&state, window, {
                            let state = state.clone();
                            move |this: &mut Workspace,
                                  _,
                                  ev: &gpui_component::input::InputEvent,
                                  window,
                                  cx| {
                                if matches!(
                                    ev,
                                    gpui_component::input::InputEvent::PressEnter { .. }
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
                        gpui_component::input::InputState,
                    >,
                                    is_width: bool| {
                        let state = state.clone();
                        cx.subscribe_in(&state, window, {
                            let state = state.clone();
                            move |this: &mut Workspace,
                                  _,
                                  ev: &gpui_component::input::InputEvent,
                                  window,
                                  cx| {
                                if matches!(
                                    ev,
                                    gpui_component::input::InputEvent::PressEnter { .. }
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
                        gpui_component::input::InputState,
                    >,
                                     which: u8| {
                        let state = state.clone();
                        cx.subscribe_in(&state, window, {
                            let state = state.clone();
                            move |this: &mut Workspace,
                                  _,
                                  ev: &gpui_component::input::InputEvent,
                                  window,
                                  cx| {
                                if matches!(
                                    ev,
                                    gpui_component::input::InputEvent::PressEnter { .. }
                                ) {
                                    let text =
                                        state.read(cx).value().to_string();
                                    match which {
                                        0 => this.apply_glyph_rename(&text),
                                        1 => this.apply_glyph_unicode(&text),
                                        2 => this.apply_kern_group(true, &text),
                                        _ => this.apply_kern_group(false, &text),
                                    }
                                    this.refresh_glyph_inputs(true, window, cx);
                                    cx.notify();
                                }
                            }
                        })
                    };
                    let component_name_input = cx.new(|cx| {
                        gpui_component::input::InputState::new(window, cx)
                            .placeholder("glyph name")
                    });
                    let reference_glyph_input = cx.new(|cx| {
                        gpui_component::input::InputState::new(window, cx)
                            .placeholder("glyph name")
                    });
                    let sub_ref = cx.subscribe_in(&reference_glyph_input, window, {
                        let state = reference_glyph_input.clone();
                        move |this: &mut Workspace,
                              _,
                              ev: &gpui_component::input::InputEvent,
                              _window,
                              cx| {
                            if matches!(
                                ev,
                                gpui_component::input::InputEvent::PressEnter { .. }
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
                        gpui_component::input::InputState::new(window, cx)
                            .placeholder("anchor name")
                    });
                    let sub_anchor = cx.subscribe_in(&anchor_name_input, window, {
                        let state = anchor_name_input.clone();
                        move |this: &mut Workspace,
                              _,
                              ev: &gpui_component::input::InputEvent,
                              _window,
                              cx| {
                            if matches!(
                                ev,
                                gpui_component::input::InputEvent::PressEnter { .. }
                            ) {
                                let text = state.read(cx).value().to_string();
                                this.apply_anchor_name(&text);
                                cx.notify();
                            }
                        }
                    });
                    let sub_comp = cx.subscribe_in(&component_name_input, window, {
                        let state = component_name_input.clone();
                        move |this: &mut Workspace,
                              _,
                              ev: &gpui_component::input::InputEvent,
                              window,
                              cx| {
                            if matches!(
                                ev,
                                gpui_component::input::InputEvent::PressEnter { .. }
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
                    let sub_gn = glyph_sub(cx, window, &name_input, 0);
                    let sub_gu = glyph_sub(cx, window, &unicode_input, 1);
                    let sub_gl = glyph_sub(cx, window, &group_l_input, 2);
                    let sub_gr = glyph_sub(cx, window, &group_r_input, 3);
                    let subscription = cx.subscribe_in(&search, window, {
                        let search = search.clone();
                        move |this: &mut Workspace,
                              _,
                              ev: &gpui_component::input::InputEvent,
                              _window,
                              cx| {
                            if matches!(ev, gpui_component::input::InputEvent::Change) {
                                this.search_query =
                                    search.read(cx).value().to_string().to_lowercase();
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
                        left_collapsed: false,
                        #[cfg(not(target_os = "macos"))]
                        app_menu_bar: app_menu_bar.clone(),
                        focus_handle: cx.focus_handle(),
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
                        reference_glyph: None,
                        reference_glyph_input: reference_glyph_input.clone(),
                        component_name_input: component_name_input.clone(),
                        anchor_name_input: anchor_name_input.clone(),
                        glyph_inputs: GlyphInputs {
                            name: name_input,
                            unicode: unicode_input,
                            group_l: group_l_input,
                            group_r: group_r_input,
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
                        axis_sliders: Vec::new(),
                        clipboard: Vec::new(),
                        #[cfg(target_family = "wasm")]
                        web_host: None,
                        _watcher: None,
                        last_save: Arc::new(Mutex::new(web_time::Instant::now())),
                        _subscriptions: vec![
                            subscription, sub_w, sub_l, sub_r, sub_x, sub_y,
                            sub_gn, sub_gu, sub_gl, sub_gr, sub_comp,
                            sub_sw, sub_sh, sub_anchor, sub_ref,
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
                // Handle shortcuts before any binding runs. Two
                // reasons: Tab must cycle point selection (the web
                // behavior) instead of gpui-component Root's tab-stop
                // traversal, and on wasm ALL action dispatch panics
                // today — gpui-component force-enables gpui's
                // "profiler" feature, whose action timing calls
                // std::time::Instant::now (unsupported on wasm). So
                // the web build routes every bound shortcut through
                // this interceptor instead of actions.
                let shortcut_target = workspace.clone();
                cx.intercept_keystrokes(move |event, _window, cx| {
                    let ks = &event.keystroke;
                    let cmd = ks.modifiers.platform;
                    let shift = ks.modifiers.shift;
                    if ks.modifiers.control || ks.modifiers.alt {
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
                cx.new(|cx| gpui_component::Root::new(workspace, window, cx))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ufo_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../runebender-web/assets/test-fonts/VirtuaGrotesk-Regular.ufo")
    }

    #[test]
    fn designspace_loads_with_masters() {
        let project = Project::load(&default_font_path()).expect("designspace loads");
        assert_eq!(project.masters.len(), 2, "regular + bold");
        assert!(project.master_names.iter().any(|n| n.contains("Bold")));
        // Active master is the default location (Regular).
        assert!(!project.master_names[project.active].contains("Bold"));
    }

    #[test]
    fn move_point_and_save_roundtrip() {
        let src = test_ufo_path();
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
        model.set_points(
            index,
            &[((before.contour, before.index), (before.x + 10.0, before.y + 5.0))],
        );
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

    #[test]
    fn snapshot_restore_roundtrip() {
        let mut model = FontModel::load(&test_ufo_path()).expect("load");
        let index = model
            .glyphs
            .iter()
            .position(|g| g.name.as_ref() == "a")
            .unwrap();
        let before = model.snapshot_contours(index).unwrap();
        let p0 = model.glyphs[index].points[0];
        model.set_points(index, &[((p0.contour, p0.index), (p0.x + 25.0, p0.y))]);
        assert_ne!(model.glyphs[index].points[0].x, p0.x);
        model.restore_contours(index, before);
        assert_eq!(model.glyphs[index].points[0].x, p0.x);
        assert_eq!(model.glyphs[index].points[0].y, p0.y);
    }

    #[test]
    fn pen_primitives_build_a_closed_contour() {
        let mut model = FontModel::load(&test_ufo_path()).expect("load");
        let index = model
            .glyphs
            .iter()
            .position(|g| g.name.as_ref() == "space")
            .unwrap();
        let base_contours = model.snapshot_contours(index).unwrap().contours.len();

        let c = model.start_contour(index, 0.0, 0.0).unwrap();
        model.append_segment(index, c, None, 100.0, 0.0, false); // line
        model.append_segment(
            index,
            c,
            Some(((130.0, 40.0), (130.0, 80.0))),
            100.0,
            120.0,
            true,
        ); // curve
        model.close_contour(index, c, None);

        let contours = model.snapshot_contours(index).unwrap().contours;
        assert_eq!(contours.len(), base_contours + 1);
        let new = &contours[c];
        assert!(new.is_closed(), "contour should be closed");
        // move->line conversion on close + 2 on-curves + 2 off-curves
        assert_eq!(new.points.len(), 5);
        assert_eq!(new.points[0].typ, norad::PointType::Line);
        assert!(new.points[4].smooth);
        // The outline cache rebuilt and is drawable.
        assert!(!model.glyphs[index].path.elements().is_empty());

        // Degenerate contour cleanup: a single stray point goes away.
        let c2 = model.start_contour(index, 5.0, 5.0).unwrap();
        model.remove_contour_if_degenerate(index, c2);
        assert_eq!(
            model.snapshot_contours(index).unwrap().contours.len(),
            base_contours + 1
        );
    }

    #[test]
    fn delete_and_smooth_operations() {
        let mut model = FontModel::load(&test_ufo_path()).expect("load");
        let index = model
            .glyphs
            .iter()
            .position(|g| g.name.as_ref() == "space")
            .unwrap();

        // Build a closed square with one curved corner:
        // (0,0) -line- (100,0) -line- (100,100) -curve- (0,100) -close-
        let c = model.start_contour(index, 0.0, 0.0).unwrap();
        model.append_segment(index, c, None, 100.0, 0.0, false);
        model.append_segment(index, c, None, 100.0, 100.0, false);
        model.append_segment(
            index,
            c,
            Some(((80.0, 130.0), (20.0, 130.0))),
            0.0,
            100.0,
            true,
        );
        model.close_contour(index, c, None);
        let count_points = |m: &FontModel| {
            m.snapshot_contours(index).unwrap().contours[c].points.len()
        };
        assert_eq!(count_points(&model), 6); // 4 on + 2 off

        // Toggle smooth on the curve's endpoint.
        let curve_end_index = model.glyphs[index]
            .points
            .iter()
            .find(|p| p.contour == c && p.x == 0.0 && p.y == 100.0)
            .map(|p| (p.contour, p.index))
            .unwrap();
        let sel: std::collections::HashSet<_> = [curve_end_index].into();
        assert!(model.toggle_smooth(index, &sel));

        // Delete one off-curve: the curve segment becomes a line.
        let off = model.glyphs[index]
            .points
            .iter()
            .find(|p| p.contour == c && !p.on_curve)
            .map(|p| (p.contour, p.index))
            .unwrap();
        let sel: std::collections::HashSet<_> = [off].into();
        assert!(model.delete_points(index, &sel));
        assert_eq!(count_points(&model), 4); // pure quad now
        let snapshot = model.snapshot_contours(index).unwrap();
        let contour_data = &snapshot.contours[c];
        assert!(contour_data.is_closed());
        assert!(contour_data
            .points
            .iter()
            .all(|p| p.typ != norad::PointType::OffCurve));

        // Delete an on-curve point: square becomes a triangle.
        let corner = model.glyphs[index]
            .points
            .iter()
            .find(|p| p.contour == c && p.x == 100.0 && p.y == 0.0)
            .map(|p| (p.contour, p.index))
            .unwrap();
        let sel: std::collections::HashSet<_> = [corner].into();
        assert!(model.delete_points(index, &sel));
        assert_eq!(count_points(&model), 3);

        // Delete everything: the contour disappears.
        let all: std::collections::HashSet<_> = model.glyphs[index]
            .points
            .iter()
            .filter(|p| p.contour == c)
            .map(|p| (p.contour, p.index))
            .collect();
        assert!(model.delete_points(index, &all));
        assert!(model.snapshot_contours(index).unwrap().contours.len() <= c);
    }

    #[test]
    fn curve_ops_run_via_shared_core() {
        let mut model = FontModel::load(&test_ufo_path()).expect("load");
        let index = model
            .glyphs
            .iter()
            .position(|g| g.name.as_ref() == "o")
            .unwrap();
        let none = std::collections::HashSet::new();
        let before: Vec<(f64, f64)> = model.glyphs[index].points.iter().map(|p| (p.x, p.y)).collect();
        // Balance evens handle tension; on a real glyph something moves.
        let changed = model.curve_op(index, &none, CurveOp::Balance);
        let after: Vec<(f64, f64)> = model.glyphs[index].points.iter().map(|p| (p.x, p.y)).collect();
        if changed {
            assert_ne!(before, after);
        }
        // On-curve points never move under balance.
        for (i, p) in model.glyphs[index].points.iter().enumerate() {
            if p.on_curve {
                assert_eq!(before[i], (p.x, p.y), "on-curve moved at {i}");
            }
        }
        // Harmonize and optimize execute without panicking and keep
        // the outline drawable.
        model.curve_op(index, &none, CurveOp::Harmonize);
        model.curve_op(index, &none, CurveOp::Optimize(0.12));
        assert!(!model.glyphs[index].path.elements().is_empty());
    }

    #[test]
    fn metric_edits() {
        let mut model = FontModel::load(&test_ufo_path()).expect("load");
        let index = model
            .glyphs
            .iter()
            .position(|g| g.name.as_ref() == "n")
            .unwrap();
        let ink = model.ink_bounds(index).unwrap();
        let advance = model.glyphs[index].advance;

        // Width edit changes only the advance.
        model.set_advance(index, advance + 20.0);
        assert_eq!(model.glyphs[index].advance, advance + 20.0);
        assert_eq!(model.ink_bounds(index).unwrap().x0, ink.x0);

        // LSB edit shifts the ink, advance untouched.
        model.shift_ink(index, 10.0);
        let ink2 = model.ink_bounds(index).unwrap();
        assert_eq!(ink2.x0, ink.x0 + 10.0);
        assert_eq!(ink2.x1, ink.x1 + 10.0);
        assert_eq!(model.glyphs[index].advance, advance + 20.0);
        assert!(model.dirty);
    }

    #[test]
    fn smooth_handle_constraint_keeps_collinearity() {
        let mut model = FontModel::load(&test_ufo_path()).expect("load");
        let index = model
            .glyphs
            .iter()
            .position(|g| g.name.as_ref() == "space")
            .unwrap();
        // Two curve segments joined at a smooth point (100,100):
        let c = model.start_contour(index, 0.0, 0.0).unwrap();
        model.append_segment(
            index,
            c,
            Some(((40.0, 60.0), (60.0, 100.0))),
            100.0,
            100.0,
            true,
        );
        model.append_segment(
            index,
            c,
            Some(((140.0, 100.0), (180.0, 60.0))),
            200.0,
            0.0,
            false,
        );
        model.close_contour(index, c, None);

        // Points in contour c: find indices of the incoming handle
        // (60,100), the smooth point (100,100), the outgoing (140,100).
        let find = |m: &FontModel, x: f64, y: f64| {
            m.glyphs[index]
                .points
                .iter()
                .find(|p| p.contour == c && p.x == x && p.y == y)
                .map(|p| p.index)
                .unwrap()
        };
        let incoming = find(&model, 60.0, 100.0);
        let outgoing = find(&model, 140.0, 100.0);

        // Drag the incoming handle downward; the outgoing must rotate
        // to stay collinear through (100,100).
        model.set_points(index, &[((c, incoming), (60.0, 80.0))]);
        model.edit_glyph(index, |g| {
            ops::constrain_smooth_neighbor(g, c, incoming)
        });
        let pts = &model.glyphs[index].points;
        let out_pt = pts.iter().find(|p| p.contour == c && p.index == outgoing).unwrap();
        // Collinearity: cross product of (anchor-incoming) and
        // (outgoing-anchor) near zero (integer rounding allowed).
        let cross = (100.0 - 60.0) * (out_pt.y - 100.0) - (100.0 - 80.0) * (out_pt.x - 100.0);
        assert!(cross.abs() <= 60.0, "not collinear enough: {cross} ({}, {})", out_pt.x, out_pt.y);
        // Length preserved (was 40).
        let len = ((out_pt.x - 100.0f64).powi(2) + (out_pt.y - 100.0f64).powi(2)).sqrt();
        assert!((len - 40.0).abs() < 2.0, "length changed: {len}");
    }

    #[test]
    fn anchor_lifecycle_with_undo_snapshot() {
        let mut model = FontModel::load(&test_ufo_path()).expect("load");
        let index = model
            .glyphs
            .iter()
            .position(|g| g.name.as_ref() == "n")
            .unwrap();
        let before = model.snapshot_contours(index).unwrap();
        let base = model.glyphs[index].anchors.len();

        model.add_anchor(index, 200.0, 500.0);
        assert_eq!(model.glyphs[index].anchors.len(), base + 1);
        model.set_anchor(index, base, 210.0, 490.0);
        assert_eq!(model.glyphs[index].anchors[base].1, 210.0);
        model.delete_anchor(index, base);
        assert_eq!(model.glyphs[index].anchors.len(), base);

        // Snapshot restore also brings anchors and width back.
        model.add_anchor(index, 1.0, 2.0);
        model.set_advance(index, 999.0);
        model.restore_contours(index, before);
        assert_eq!(model.glyphs[index].anchors.len(), base);
        assert_ne!(model.glyphs[index].advance, 999.0);
    }

    #[test]
    fn kerning_lookup_and_exception() {
        let mut model = FontModel::load(&test_ufo_path()).expect("load");
        // Group fallback resolves (VirtuaGrotesk has kern groups); the
        // exact value doesn't matter, just that lookup doesn't panic
        // and exceptions override.
        let base = ops::kern_value(&model.font, "A", "V");
        ops::set_kern_pair(&mut model.font, "A", "V", base - 14.0);
        assert_eq!(ops::kern_value(&model.font, "A", "V"), base - 14.0);
        // Unrelated pair unaffected by the exception.
        let _ = ops::kern_value(&model.font, "o", "o");
    }

    #[test]
    fn interpolation_at_midpoint() {
        let mut project = Project::load(&default_font_path()).expect("designspace");
        assert!(project.model.is_some(), "two masters, model expected");
        // Move every axis to its normalized midpoint toward max.
        let axis_names: Vec<String> =
            project.axes.iter().map(|a| a.name.clone()).collect();
        for name in &axis_names {
            project.location.insert(name.clone(), 0.5);
        }
        let (path, advance) = project
            .interpolated_glyph("n")
            .expect("compatible masters interpolate");
        assert!(!path.elements().is_empty());
        // The interpolated advance sits between the two masters'.
        let a0 = project.masters[0].font.get_glyph("n").unwrap().width;
        let a1 = project.masters[1].font.get_glyph("n").unwrap().width;
        let (lo, hi) = (a0.min(a1), a0.max(a1));
        assert!(
            advance >= lo - 1e-6 && advance <= hi + 1e-6,
            "advance {advance} outside [{lo}, {hi}]"
        );
        // Default location yields no ghost.
        for name in &axis_names {
            project.location.insert(name.clone(), 0.0);
        }
        assert!(project.interpolated_glyph("n").is_none());
    }

    #[test]
    fn shape_contours() {
        let mut model = FontModel::load(&test_ufo_path()).expect("load");
        let index = model
            .glyphs
            .iter()
            .position(|g| g.name.as_ref() == "space")
            .unwrap();
        let base = model.snapshot_contours(index).unwrap().contours.len();
        let rect = kurbo::Rect::new(10.0, 20.0, 110.0, 220.0);
        model.add_shape_contour(index, rect, false);
        model.add_shape_contour(index, rect, true);
        let contours = model.snapshot_contours(index).unwrap().contours;
        assert_eq!(contours.len(), base + 2);
        let square = &contours[base];
        assert_eq!(square.points.len(), 4);
        assert!(square.is_closed());
        let circle = &contours[base + 1];
        assert_eq!(circle.points.len(), 12); // 4 on + 8 off
        assert!(circle.is_closed());
        // Ellipse extremes touch the rect edges.
        let xs: Vec<f64> = circle.points.iter().map(|p| p.x).collect();
        assert_eq!(xs.iter().cloned().fold(f64::MAX, f64::min), 10.0);
        assert_eq!(xs.iter().cloned().fold(f64::MIN, f64::max), 110.0);
    }

    #[test]
    fn compat_map_flags_structure_changes() {
        let mut project = Project::load(&default_font_path()).expect("designspace");
        // Demo masters are interpolation-compatible for letters.
        assert_eq!(project.compat.get("n"), Some(&true));
        // Break compatibility in one master and recheck.
        let idx = project.masters[0]
            .glyphs
            .iter()
            .position(|g| g.name.as_ref() == "n")
            .unwrap();
        let rect = kurbo::Rect::new(0.0, 0.0, 50.0, 50.0);
        project.masters[0].add_shape_contour(idx, rect, false);
        project.recheck_compat("n");
        assert_eq!(project.compat.get("n"), Some(&false));
    }

    #[test]
    fn decompose_components() {
        let mut model = FontModel::load(&test_ufo_path()).expect("load");
        let index = model
            .glyphs
            .iter()
            .position(|g| !g.component_names.is_empty())
            .expect("demo font has composite glyphs");
        use kurbo::Shape;
        let area_before = model.glyphs[index].path.area().abs();
        let contours_before = model.snapshot_contours(index).unwrap().contours.len();
        assert!(model.decompose(index));
        let snap = model.snapshot_contours(index).unwrap();
        assert!(snap.components.is_empty());
        assert!(snap.contours.len() > contours_before);
        // The rendered ink is essentially unchanged (integer rounding).
        let area_after = model.glyphs[index].path.area().abs();
        assert!(
            (area_before - area_after).abs() / area_before.max(1.0) < 0.02,
            "area changed too much: {area_before} -> {area_after}"
        );
        assert!(model.glyphs[index].component_names.is_empty());
    }

    #[test]
    fn remove_overlap_unions_contours() {
        use kurbo::Shape;
        let mut model = FontModel::load(&test_ufo_path()).expect("load");
        let index = model
            .glyphs
            .iter()
            .position(|g| g.name.as_ref() == "space")
            .unwrap();
        // Two overlapping squares: union area = 100*100 + 100*100 - 50*50.
        model.add_shape_contour(index, kurbo::Rect::new(0.0, 0.0, 100.0, 100.0), false);
        model.add_shape_contour(index, kurbo::Rect::new(50.0, 50.0, 150.0, 150.0), false);
        assert!(model.remove_overlap(index));
        let snap = model.snapshot_contours(index).unwrap();
        assert_eq!(snap.contours.len(), 1, "union should merge to one contour");
        let area = model.glyphs[index].path.area().abs();
        assert!(
            (area - 17500.0).abs() < 100.0,
            "union area wrong: {area} (expected ~17500)"
        );
        assert!(snap.contours[0].is_closed());
    }

    #[test]
    fn glyphs_import_end_to_end() {
        // Use the minimal fixture from runebender-core's tests via a
        // real conversion + load cycle.
        const MINIMAL: &str = r#"{
.appVersion = "3300";
.formatVersion = 3;
axes = (
{
name = Weight;
tag = wght;
}
);
familyName = TestSans;
fontMaster = (
{
ascender = 800;
axesValues = (400);
capHeight = 700;
descender = -200;
id = m01;
name = Regular;
},
{
ascender = 800;
axesValues = (700);
capHeight = 700;
descender = -200;
id = m02;
name = Bold;
}
);
glyphs = (
{
glyphname = A;
layers = (
{
layerId = m01;
shapes = (
{
closed = 1;
nodes = (
(0,0,l),
(100,0,l),
(50,700,l)
);
}
);
width = 600;
},
{
layerId = m02;
shapes = (
{
closed = 1;
nodes = (
(0,0,l),
(140,0,l),
(70,700,l)
);
}
);
width = 640;
}
);
unicode = 65;
}
);
unitsPerEm = 1000;
}"#;
        let dir = std::env::temp_dir().join("rbg-glyphs-import-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let glyphs_path = dir.join("TestSans.glyphs");
        std::fs::write(&glyphs_path, MINIMAL).unwrap();
        let project = Project::load(&glyphs_path).expect("glyphs project loads");
        assert_eq!(project.masters.len(), 2);
        let a = project
            .active_font()
            .glyphs
            .iter()
            .find(|g| g.name.as_ref() == "A")
            .expect("glyph A");
        assert!(!a.path.elements().is_empty());
        std::fs::remove_dir_all(&dir).ok();
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
