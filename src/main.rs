// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! Runebender GPUI: a font editor built on [GPUI](https://gpui.rs/),
//! started as a point of comparison against
//! [runebender-xilem](https://github.com/eliheuer/runebender-xilem).

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
        SaveFont,
        Undo,
        Redo,
        CopyContours,
        PasteContours,
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
        Harmonize,
        Balance,
        Optimize,
        ZoomToFit,
        NextMaster,
        PreviousMaster,
        Quit
    ]
);

/// The application menu, used three ways: the native macOS menu bar,
/// the stored menu Windows/Linux expose to `get_menus`, and the
/// in-window menu bar (gpui-component AppMenuBar) drawn on every
/// platform that has no native bar, the browser included.
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
                MenuItem::action("Open…", OpenFont),
                MenuItem::separator(),
                MenuItem::action("Save", SaveFont),
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
                MenuItem::action("Reverse Contours", ReverseContours),
                MenuItem::action("Set Start Point", SetStartPoint),
                MenuItem::separator(),
                MenuItem::action("Union", BooleanUnion),
                MenuItem::action("Subtract", BooleanSubtract),
                MenuItem::action("Intersect", BooleanIntersect),
                MenuItem::action("Exclude", BooleanExclude),
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
                MenuItem::action("Next Master", NextMaster),
                MenuItem::action("Previous Master", PreviousMaster),
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
    codepoint_map: std::collections::HashMap<char, usize>,
    /// glyph name → index into `glyphs` (text buffer sorts carry
    /// names, including unencoded ligature glyphs from shaping).
    name_map: std::collections::HashMap<String, usize>,
    family_name: SharedString,
    source_path: PathBuf,
    units_per_em: f64,
    ascender: f64,
    descender: f64,
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
        self.modified_glyphs.insert(name);
        self.rebuild_entry(glyph_index);
        Some(result)
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
        let family_name = info.family_name.clone().unwrap_or_else(|| "Untitled".into());

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

        let codepoint_map = glyphs
            .iter()
            .enumerate()
            .filter_map(|(i, g)| g.codepoint.map(|c| (c, i)))
            .collect();
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
            codepoint_map,
            name_map,
            family_name: family_name.into(),
            source_path,
            units_per_em,
            ascender,
            descender,
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
    fn constrain_smooth_neighbor(&mut self, glyph_index: usize, contour: usize, index: usize) {
        self.edit_glyph(glyph_index, |g| {
            ops::constrain_smooth_neighbor(g, contour, index)
        });
    }

    /// Kerning between two glyphs with UFO group fallback.
    fn kern_value(&self, left: &str, right: &str) -> f64 {
        ops::kern_value(&self.font, left, right)
    }

    /// Set an exception-level (glyph-to-glyph) kern pair.
    fn set_kern_pair(&mut self, left: &str, right: &str, value: f64) {
        ops::set_kern_pair(&mut self.font, left, right, value);
        self.dirty = true;
        self.kerning_dirty = true;
    }

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
            let pad = 5.0_f64;
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
    /// each selected point's original position.
    Points {
        start: (f64, f64),
        originals: Vec<((usize, usize), (f64, f64))>,
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
    /// Rubber-band selection rectangle, in design space.
    Marquee {
        start: (f64, f64),
        current: (f64, f64),
    },
    /// Dragging an anchor.
    Anchor {
        index: usize,
        start: (f64, f64),
        orig: (f64, f64),
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
    selected_anchor: Option<usize>,
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
    fn new() -> Self {
        Self {
            sort_offset: (0.0, 0.0),
            previous_tool: Tool::Select,
            hyper_contour: None,
            segment_hover: None,
            selected_component: None,
            sidebearing_hover: None,
            pointer: None,
            viewport: ViewPort::new(),
            initialized: false,
            tool: Tool::Select,
            pen: None,
            shape_ellipse: false,
            selected: std::collections::HashSet::new(),
            selected_anchor: None,
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
            .fit_to_canvas(w as f64, h as f64, advance, ascender, descender, 0.62);
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
    project: Option<Project>,
    load_error: Option<SharedString>,
    selected: Option<usize>,
    category: runebender_core::category::GlyphCategory,
    mode: Mode,
    editor: EditorState,
    /// The preview strip's text, as a real text-engine buffer:
    /// typing, caret, bidi, kerning, and shaping all come from
    /// runebender-core; this struct only draws it.
    preview_buffer: runebender_core::text::TextBuffer,
    /// The editor's text buffer (the text tool): the open glyph is
    /// the active sort; other sorts render as filled context around
    /// it, exactly the web and xilem model.
    edit_buffer: runebender_core::text::TextBuffer,
    /// Keys route to the preview buffer (click the strip to focus,
    /// Escape to leave).
    text_focus: bool,
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
    metric_inputs: MetricInputs,
    preview_bounds: Arc<Mutex<Bounds<gpui::Pixels>>>,
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
struct MetricInputs {
    width: gpui::Entity<gpui_component::input::InputState>,
    lsb: gpui::Entity<gpui_component::input::InputState>,
    rsb: gpui::Entity<gpui_component::input::InputState>,
    /// Selected point coordinates (Selection section).
    x: gpui::Entity<gpui_component::input::InputState>,
    y: gpui::Entity<gpui_component::input::InputState>,
}

const CELL: f32 = 96.0;
const HIT_RADIUS_PX: f64 = 8.0;

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

    fn open_editor(&mut self, index: usize) {
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
        self.editor.selected_anchor = None;
    }

    fn glyph_cell(&self, index: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        self.glyph_cell_sized(index, CELL, false, cx)
    }

    fn glyph_cell_sized(
        &self,
        index: usize,
        cell: f32,
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
        };
        let outline = entry.path.clone();
        let advance = entry.advance;
        let ascender = font.ascender;
        let descender = font.descender;
        let label_h = if cell >= 90.0 { 20.0 } else { 14.0 };
        let incompatible = self
            .project
            .as_ref()
            .and_then(|p| p.compat.get(entry.name.as_ref()))
            .is_some_and(|ok| !ok);

        let label_h = if cell >= 90.0 { 32.0 } else { label_h };
        let mark = entry.mark.as_deref().and_then(t::mark_color);
        div()
            .id(index)
            .w(px(cell))
            .h(px(cell + label_h))
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
                this.text_focus = false;
                if jump_on_click {
                    this.open_editor(index);
                } else {
                    this.selected = Some(index);
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
                                window.paint_path(path, mark.unwrap_or_else(t::glyph_fill));
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
                                    t::accent()
                                } else {
                                    mark.unwrap_or_else(t::text_muted)
                                })
                                .child(unicode_label.unwrap_or_else(|| "".into())),
                        )
                    }),
            )
    }

    /// Left sidebar tile: search plus the category filter list,
    /// like runebender-web's CategorySidebar.
    fn category_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        use runebender_core::category::GlyphCategory as GC;
        const CATEGORIES: [(GC, &str); 8] = [
            (GC::All, "All"),
            (GC::Letter, "Letter"),
            (GC::Number, "Number"),
            (GC::Punctuation, "Punctuation"),
            (GC::Symbol, "Symbol"),
            (GC::Mark, "Mark"),
            (GC::Separator, "Separator"),
            (GC::Other, "Other"),
        ];
        // Glyph counts per category, like the web sidebar.
        let mut counts = [0usize; 8];
        if let Some(font) = self.font() {
            for entry in &font.glyphs {
                counts[0] += 1;
                let category = entry
                    .codepoint
                    .map(GC::from_codepoint)
                    .unwrap_or(GC::Other);
                if let Some(slot) =
                    CATEGORIES.iter().position(|(c, _)| *c == category)
                {
                    counts[slot] += 1;
                }
            }
        }
        let mut list = div().flex().flex_col().gap_1();
        for (i, (category, label)) in CATEGORIES.into_iter().enumerate() {
            let active = self.category == category;
            list = list.child(
                div()
                    .id(("category", i))
                    .px_2()
                    .py_0p5()
                    .rounded_sm()
                    .text_sm()
                    .cursor_pointer()
                    .flex()
                    .justify_between()
                    .when(active, |el| {
                        el.border_1().border_color(t::accent()).text_color(t::accent())
                    })
                    .when(!active, |el| el.text_color(t::text()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.category = category;
                        cx.notify();
                    }))
                    .child(label)
                    .child(
                        div()
                            .text_color(if active { t::accent() } else { t::text_muted() })
                            .child(format!("{}", counts[i])),
                    ),
            );
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .p_2()
                    .border_b_1()
                    .border_color(t::panel_outline())
                    .child(gpui_component::input::Input::new(&self.search)),
            )
            .child(self.section(cx, "Categories", list))
    }

    /// Right tile: details of the selected glyph, like
    /// runebender-web's GlyphInfoSidebar.
    fn glyph_info_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        let row = |header: &'static str, value: SharedString| {
            div()
                .flex()
                .flex_col()
                .child(div().text_sm().text_color(t::info_header()).child(header))
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
        let left_group = runebender_core::glyph_ops::kern_group(&font.font, &name, true)
            .map(|g| g.as_str().replace("public.kern1.", ""))
            .unwrap_or_else(|| "(empty)".into());
        let right_group = runebender_core::glyph_ops::kern_group(&font.font, &name, false)
            .map(|g| g.as_str().replace("public.kern2.", ""))
            .unwrap_or_else(|| "(empty)".into());
        panel = panel
            .child(row("Master", master))
            .child(row("Glyph Name", entry.name.clone()))
            .child(row("Width", format!("{:.0}", entry.advance).into()))
            .child(row(
                "Kerning Groups",
                format!("L {left_group} · R {right_group}").into(),
            ))
            .child(row(
                "Unicode",
                entry
                    .codepoint
                    .map(|c| format!("{:04X}", c as u32))
                    .unwrap_or_else(|| "—".into())
                    .into(),
            ))
            .child(row("Contours", format!("{contours}").into()));
        self.section(cx, "Glyph", panel)
    }

    /// Colors panel: mark-color swatches for the selected glyph, like
    /// the web grid's bottom-right panel.
    fn mark_colors_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        let current = self
            .selected
            .and_then(|i| self.font().and_then(|f| f.glyphs.get(i)))
            .and_then(|e| e.mark.clone());
        let mut swatches = div().flex().flex_wrap().gap_2();
        for (index, (label, color)) in t::mark_palette().into_iter().enumerate() {
            let is_current = current.as_deref() == Some(label.as_str());
            swatches = swatches.child(
                div()
                    .id(("mark-swatch", index))
                    .w(px(22.0))
                    .h(px(22.0))
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
            );
        }
        swatches = swatches.child(
            div()
                .id("mark-clear")
                .w(px(22.0))
                .h(px(22.0))
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
        );
        self.section(cx, "Colors", swatches)
    }

    /// Set or clear the selected glyph's mark color.
    fn set_selected_mark(&mut self, label: Option<String>) {
        let Some(index) = self.selected else { return };
        let Some(font) = self.font_mut() else { return };
        font.edit_glyph(index, |glyph| {
            runebender_core::theme_oklch::set_glyph_mark(glyph, label.as_deref());
        });
    }

    /// Editor sidebar: search + scrollable mini glyph grid, so glyph
    /// switching doesn't require leaving the editor.
    fn editor_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let query = self.search_query.clone();
        let cells: Vec<_> = match self.font() {
            Some(font) => (0..font.glyphs.len())
                .filter(|&i| {
                    query.is_empty() || font.glyphs[i].name.to_lowercase().contains(&query)
                })
                .map(|i| self.glyph_cell_sized(i, 44.0, true, cx).into_any_element())
                .collect(),
            None => Vec::new(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .min_h(px(0.0))
            .child(
                div()
                    .p_2()
                    .border_b_1()
                    .border_color(t::panel_outline())
                    .child(gpui_component::input::Input::new(&self.search)),
            )
            .child(
                div()
                    .id("editor-sidebar-grid")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .child(div().flex().flex_wrap().gap_1().p_2().children(cells)),
            )
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
            .gap_2()
            .px_3()
            .py_2()
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
                        )),
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

    /// Layers section: one row per master, the active one highlighted.
    fn layers_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let (names, active): (Vec<SharedString>, usize) = match &self.project {
            Some(p) => (p.master_names.clone(), p.active),
            None => (Vec::new(), 0),
        };
        let reference = self.reference_layers.clone();
        let rows: Vec<_> = names
            .into_iter()
            .enumerate()
            .map(|(i, name)| {
                let is_active = i == active;
                let eye_on = reference.contains(&i);
                div()
                    .flex()
                    .items_center()
                    .gap_1()
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
        self.section(cx, "Layers", body)
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
        let anchors = entry.anchors.clone();
        let selected_anchor = self.editor.selected_anchor;
        let advance = entry.advance;
        let ascender = font.ascender;
        let descender = font.descender;

        let transform = self.editor.transform();
        let zoom = self.editor.zoom();
        let selected_points = self.editor.selected.clone();
        let marquee = match &self.editor.drag {
            Some(Drag::Marquee { start, current }) => Some((*start, *current)),
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
                    if this.editor_mouse_drag(event.position) {
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
                            hline(0.0, window);
                            hline(ascender, window);
                            hline(descender, window);
                            for (right, x) in [(false, 0.0), (true, advance)] {
                                let hovered = sidebearing_hover == Some(right);
                                let a = to_screen(x, ascender);
                                let b = to_screen(x, descender);
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
                        let mut line = |a: Point<gpui::Pixels>,
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
                        // is up (points return with select).
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
                            let sort_transform =
                                transform * Affine::translate((sp.x, sp.y));
                            if let Some(p) =
                                build_fill_path(path, sort_transform, origin)
                            {
                                window.paint_path(p, t::glyph_fill());
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
                        let square = |window: &mut Window,
                                      center: Point<gpui::Pixels>,
                                      r: f32,
                                      color: gpui::Rgba| {
                            window.paint_quad(gpui::fill(
                                Bounds::from_corners(
                                    gpui::point(center.x - px(r), center.y - px(r)),
                                    gpui::point(center.x + px(r), center.y + px(r)),
                                ),
                                color,
                            ));
                        };
                        for p in points.iter() {
                            if preview_mode || text_mode {
                                break;
                            }
                            let center = to_screen(p.x, p.y);
                            let is_selected =
                                selected_points.contains(&(p.contour, p.index));
                            // Web style: colored ring, dark inner;
                            // selected points fill solid yellow.
                            let (ring, inner) = if is_selected {
                                (t::point_selected(), t::point_selected())
                            } else if p.hyper {
                                (t::point_hyper_outer(), t::point_inner())
                            } else if !p.on_curve {
                                (t::point_offcurve_outer(), t::point_inner())
                            } else if p.smooth {
                                (t::point_smooth_outer(), t::point_inner())
                            } else {
                                (t::point_corner_outer(), t::point_inner())
                            };
                            if p.on_curve && !p.smooth {
                                square(window, center, 4.5, ring);
                                square(window, center, 2.5, inner);
                            } else if p.on_curve {
                                circle(window, center, 4.5, ring);
                                circle(window, center, 2.5, inner);
                            } else {
                                circle(window, center, 3.5, ring);
                                circle(window, center, 1.8, inner);
                            }
                        }
                        // Anchors: diamonds (rotated squares drawn as
                        // two overlapping quads approximate; use a
                        // filled path).
                        for (ai, (_, ax, ay)) in anchors.iter().enumerate() {
                            if preview_mode || text_mode {
                                break;
                            }
                            let c = to_screen(*ax, *ay);
                            let r = px(5.0);
                            let mut pb = PathBuilder::fill();
                            pb.move_to(gpui::point(c.x, c.y - r));
                            pb.line_to(gpui::point(c.x + r, c.y));
                            pb.line_to(gpui::point(c.x, c.y + r));
                            pb.line_to(gpui::point(c.x - r, c.y));
                            pb.close();
                            if let Ok(pth) = pb.build() {
                                let color = if selected_anchor == Some(ai) {
                                    t::accent()
                                } else {
                                    t::anchor()
                                };
                                window.paint_path(pth, color);
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
        let Some(font) = self.font() else {
            return;
        };
        let entry = &font.glyphs[index];
        let (advance, asc, desc) = (entry.advance, font.ascender, font.descender);
        self.editor.fit(advance, asc, desc);
    }

    fn editor_mouse_down(&mut self, pos: Point<gpui::Pixels>, shift: bool, alt: bool, click_count: usize) {
        self.text_focus = false;

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
            .filter(|(dist, _, _)| *dist <= tolerance)
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((_, ai, orig)) = anchor_hit {
            self.editor.selected_anchor = Some(ai);
            self.editor.selected.clear();
            self.push_undo_snapshot(index);
            self.editor.drag = Some(Drag::Anchor {
                index: ai,
                start: (dx, dy),
                orig,
            });
            return;
        }
        self.editor.selected_anchor = None;

        let hit = all_points
            .iter()
            .map(|(id, (x, y))| {
                let dist = ((x - dx).powi(2) + (y - dy).powi(2)).sqrt();
                (dist, *id)
            })
            .filter(|(dist, _)| *dist <= tolerance)
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
                    let originals: Vec<((usize, usize), (f64, f64))> = all_points
                        .into_iter()
                        .filter(|(id, _)| self.editor.selected.contains(id))
                        .collect();
                    self.push_undo_snapshot(index);
                    self.editor.drag = Some(Drag::Points {
                        start: (dx, dy),
                        originals,
                    });
                }
            }
            None => {
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
                        self.editor.selected.extend(ids.iter().copied());
                    } else {
                        self.editor.selected = ids.iter().copied().collect();
                    }
                    let originals: Vec<((usize, usize), (f64, f64))> = all_points
                        .into_iter()
                        .filter(|(id, _)| self.editor.selected.contains(id))
                        .collect();
                    self.push_undo_snapshot(index);
                    self.editor.drag = Some(Drag::Points {
                        start: (dx, dy),
                        originals,
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
                }
                self.editor.drag = Some(Drag::Marquee {
                    start: (dx, dy),
                    current: (dx, dy),
                });
            }
        }
    }

    fn editor_mouse_drag(&mut self, pos: Point<gpui::Pixels>) -> bool {
        let Mode::Editor(index) = self.mode else {
            return false;
        };
        if self.editor.tool == Tool::Pen {
            return self.pen_mouse_drag(index, pos);
        }
        let (dx, dy) = self.editor.window_to_design(pos);
        self.editor.cursor = (dx, dy);
        match &mut self.editor.drag {
            Some(Drag::Anchor { index: ai, start, orig }) => {
                let (ai, start, orig) = (*ai, *start, *orig);
                let target = (
                    (orig.0 + dx - start.0).round(),
                    (orig.1 + dy - start.1).round(),
                );
                if let Some(font) = self.font_mut() {
                    font.set_anchor(index, ai, target.0, target.1);
                    return true;
                }
                false
            }
            Some(Drag::Points { start, originals }) => {
                let (sx, sy) = *start;
                let updates: Vec<((usize, usize), (f64, f64))> = originals
                    .iter()
                    .map(|(id, (ox, oy))| {
                        (*id, ((ox + dx - sx).round(), (oy + dy - sy).round()))
                    })
                    .collect();
                let single = if updates.len() == 1 {
                    Some(updates[0].0)
                } else {
                    None
                };
                if let Some(font) = self.font_mut() {
                    font.set_points(index, &updates);
                    if let Some((contour, point_index)) = single {
                        font.constrain_smooth_neighbor(index, contour, point_index);
                    }
                    return true;
                }
                false
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
            Some(Drag::Knife { current, .. }) => {
                *current = (dx, dy);
                true
            }
            Some(Drag::Marquee { current, .. })
            | Some(Drag::Shape { current, .. })
            | Some(Drag::Measure { current, .. }) => {
                *current = (dx, dy);
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
        if let Some(Drag::Marquee { start, current }) = self.editor.drag.take() {
            let (x0, x1) = (start.0.min(current.0), start.0.max(current.0));
            let (y0, y1) = (start.1.min(current.1), start.1.max(current.1));
            let inside: Vec<(usize, usize)> = match self.font() {
                Some(font) => font.glyphs[index]
                    .points
                    .iter()
                    .filter(|p| p.x >= x0 && p.x <= x1 && p.y >= y0 && p.y <= y1)
                    .map(|p| (p.contour, p.index))
                    .collect(),
                None => Vec::new(),
            };
            self.editor.selected.extend(inside);
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
        let Mode::Editor(index) = self.mode else {
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
    fn refresh_metric_inputs(&mut self, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Mode::Editor(index) = self.mode else {
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
    fn apply_coord(&mut self, is_x: bool, value: f64) {
        self.status_note = Some(format!("apply_coord {is_x} {value}").into());
        let Mode::Editor(index) = self.mode else {
            return;
        };
        let Some(point) = self.single_selected_point() else {
            self.status_note = Some("apply_coord: no single point".into());
            return;
        };
        let (x, y) = if is_x { (value, point.y) } else { (point.x, value) };
        self.push_undo_snapshot(index);
        let id = (point.contour, point.index);
        if let Some(font) = self.font_mut() {
            font.edit_glyph(index, |g| {
                runebender_core::glyph_ops::set_points(g, &[(id, (x, y))]);
                runebender_core::glyph_ops::constrain_smooth_neighbor(g, id.0, id.1);
            });
        }
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
        let (x, y) = match self.single_selected_point() {
            Some(p) => (format!("{:.0}", p.x), format!("{:.0}", p.y)),
            None => (String::new(), String::new()),
        };
        for (entity, value) in [
            (self.metric_inputs.x.clone(), x),
            (self.metric_inputs.y.clone(), y),
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
        if single.is_some() {
            let field = |label: &'static str,
                         input: &gpui::Entity<gpui_component::input::InputState>| {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(14.0)).text_sm().text_color(t::text_muted()).child(label))
                    .child(div().flex_1().child(gpui_component::input::Input::new(input)))
            };
            body = body
                .child(field("X", &self.metric_inputs.x))
                .child(field("Y", &self.metric_inputs.y));
        }
        self.section(cx, "Selection", body)
    }

    /// Flip/rotate the selection (whole glyph when nothing selected)
    /// about its bbox center, with an undo snapshot.
    fn apply_transform(&mut self, transform: Affine) {
        let Mode::Editor(index) = self.mode else { return };
        self.push_undo_snapshot(index);
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
    fn push_undo_snapshot(&mut self, index: usize) {
        if let Some(snapshot) = self.font().and_then(|f| f.snapshot_contours(index)) {
            self.editor.undo.push(snapshot);
            self.editor.redo.clear();
        }
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
    fn nudge_selection(&mut self, dx: f64, dy: f64) -> bool {
        let Mode::Editor(index) = self.mode else {
            return false;
        };
        if self.editor.selected.is_empty() {
            return false;
        }
        self.push_undo_snapshot(index);
        let selected = self.editor.selected.clone();
        let Some(font) = self.font_mut() else {
            return false;
        };
        let updates: Vec<((usize, usize), (f64, f64))> = font.glyphs[index]
            .points
            .iter()
            .filter(|p| selected.contains(&(p.contour, p.index)))
            .map(|p| ((p.contour, p.index), (p.x + dx, p.y + dy)))
            .collect();
        font.set_points(index, &updates);
        true
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
            let local = self.editor.window_to_local(event.position);
            let factor = (delta.1 * 0.01).exp();
            self.editor.viewport.zoom_about(local, factor, 0.01, 100.0);
        } else {
            self.editor.viewport.pan(delta.0, delta.1);
        }
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let (title, status): (SharedString, SharedString) =
            match (self.font(), &self.load_error) {
                (Some(font), _) => (
                    format!(
                        "{} · {} glyphs · {} upm",
                        font.source_path.display(),
                        font.glyphs.len(),
                        font.units_per_em
                    )
                    .into(),
                    if font.dirty {
                        "Not saved".into()
                    } else {
                        "Saved".into()
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
            .px_3()
            .py_1p5()
            .bg(t::panel_bg())
            .border_b_1()
            .border_color(t::panel_outline())
            .child(
                div()
                    .id("toggle-left")
                    .w(px(26.0))
                    .h(px(26.0))
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
            .child(self.master_switcher(cx))
    }

    /// One button per master; the active one gets the accent border.
    fn master_switcher(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let (names, active): (Vec<SharedString>, usize) = match &self.project {
            Some(p) if p.masters.len() > 1 => (p.master_names.clone(), p.active),
            _ => (Vec::new(), 0),
        };
        div().flex().gap_1().children(names.into_iter().enumerate().map(
            move |(i, name)| {
                let is_active = i == active;
                let dirty = self
                    .project
                    .as_ref()
                    .is_some_and(|p| p.masters[i].dirty);
                let label: SharedString = if dirty {
                    format!("{name} •").into()
                } else {
                    name
                };
                div()
                    .id(("master", i))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(if is_active { t::accent() } else { t::cell_border() })
                    .text_color(if is_active { t::text() } else { t::text_muted() })
                    .text_sm()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.switch_master(i);
                        cx.notify();
                    }))
                    .child(label)
            },
        ))
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
    fn axes_bar(&self) -> impl IntoElement + use<> {
        let Some(project) = self.project.as_ref() else {
            return div().into_any_element();
        };
        if self.axis_sliders.is_empty() {
            return div().into_any_element();
        }
        let mut row = div()
            .flex()
            .items_center()
            .gap_4()
            .px_4()
            .py_1()
            .bg(t::panel_bg())
            .border_t_1()
            .border_color(t::cell_border());
        for (axis_index, slider) in &self.axis_sliders {
            let axis = &project.axes[*axis_index];
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().text_sm().text_color(t::text_muted()).child(axis.tag.clone()))
                    .child(div().w(px(160.0)).child(gpui_component::slider::Slider::new(slider))),
            );
        }
        row.into_any_element()
    }

    /// Bottom bar in editor mode: Width / LSB / RSB fields.
    fn metrics_bar(&self) -> impl IntoElement + use<> {
        let field = |label_text: &'static str,
                     state: &gpui::Entity<gpui_component::input::InputState>| {
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(div().text_sm().text_color(t::text_muted()).child(label_text))
                .child(div().w(px(80.0)).child(gpui_component::input::Input::new(state)))
        };
        div()
            .flex()
            .items_center()
            .gap_4()
            .px_4()
            .py_2()
            .bg(t::panel_bg())
            .border_t_1()
            .border_color(t::cell_border())
            .child(field("Width", &self.metric_inputs.width))
            .child(field("LSB", &self.metric_inputs.lsb))
            .child(field("RSB", &self.metric_inputs.rsb))
    }

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
        let widths: Vec<(usize, String, Option<char>, f64)> = (0..self
            .preview_buffer
            .len())
            .filter_map(|i| {
                let sort = self.preview_buffer.sort(i)?;
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
        self.preview_buffer.set_glyph_inventory(inventory.clone());
        self.preview_buffer.set_kerning_model(kerning.clone());
        for (i, name, codepoint, advance) in widths {
            self.preview_buffer.update_glyph(i, name, codepoint, advance);
        }
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
            self.editor.selected_anchor = None;
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
                self.editor.selected_anchor = None;
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

    /// The kern pair at the preview caret: the glyph sorts just
    /// before and after the cursor.
    fn preview_kern_pair(&self) -> Option<(String, String)> {
        let cursor = self.preview_buffer.cursor();
        if cursor == 0 {
            return None;
        }
        let left = self.preview_buffer.sort(cursor - 1)?.glyph_name()?.to_string();
        let right = self.preview_buffer.sort(cursor)?.glyph_name()?.to_string();
        Some((left, right))
    }

    /// Text preview strip: a live text-engine buffer. Click to place
    /// the caret and focus; type to edit; comma/period kern the pair
    /// at the caret.
    fn preview_strip(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let Some(font) = self.font() else {
            return div().into_any_element();
        };
        let upm = font.units_per_em;
        let ascender = font.ascender;
        let descender = font.descender;
        let line_height = self.text_line_height();
        let layout = self.preview_buffer.layout(line_height);
        let items: Vec<(Arc<BezPath>, f64, f64)> = layout
            .items
            .iter()
            .filter_map(|item| {
                let sort = self.preview_buffer.sort(item.index)?;
                if sort.is_absorbed() {
                    return None;
                }
                let name = sort.glyph_name()?;
                let glyph = *font.name_map.get(name)?;
                Some((font.glyphs[glyph].path.clone(), item.x, item.y))
            })
            .collect();
        let caret = self
            .text_focus
            .then_some((layout.cursor_x, layout.cursor_y));
        let bounds_slot = self.preview_bounds.clone();
        div()
            .h(px(104.0))
            .flex()
            .items_center()
            .bg(t::panel_bg())
            .border_t_1()
            .border_color(if self.text_focus {
                t::accent()
            } else {
                t::cell_border()
            })
            .child(
                div()
                    .id("preview-strip")
                    .flex_1()
                    .h_full()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                            this.preview_click(event.position);
                            cx.notify();
                        }),
                    )
                    .child(
                        canvas(
                            move |bounds, _, _| bounds,
                            move |_, bounds: Bounds<gpui::Pixels>, window, _| {
                                *bounds_slot.lock().unwrap() = bounds;
                                let h: f32 = bounds.size.height.into();
                                let scale = (h as f64 * 0.72) / upm;
                                let baseline = h as f64 * 0.82 + descender * scale;
                                let origin_x: f64 = 24.0;
                                for (path, x, y) in items.iter() {
                                    let transform = Affine::translate((
                                        origin_x + x * scale,
                                        baseline - y * scale,
                                    )) * Affine::scale_non_uniform(scale, -scale);
                                    if let Some(p) =
                                        build_fill_path(path, transform, bounds.origin)
                                    {
                                        window.paint_path(p, t::preview_glyph());
                                    }
                                }
                                if let Some((cx_, cy)) = caret {
                                    let x = bounds.origin.x
                                        + px((origin_x + cx_ * scale) as f32);
                                    let top = bounds.origin.y
                                        + px((baseline - cy * scale
                                            - ascender * scale)
                                            as f32);
                                    let bottom = bounds.origin.y
                                        + px((baseline - cy * scale
                                            - descender * scale)
                                            as f32);
                                    window.paint_quad(gpui::fill(
                                        Bounds::from_corners(
                                            gpui::point(x - px(1.0), top),
                                            gpui::point(x + px(1.0), bottom),
                                        ),
                                        t::accent(),
                                    ));
                                }
                            },
                        )
                        .size_full(),
                    ),
            )
            .into_any_element()
    }

    /// Click in the preview strip: place the caret there and take
    /// text focus.
    fn preview_click(&mut self, pos: Point<gpui::Pixels>) {
        let Some(font) = self.font() else { return };
        let upm = font.units_per_em;
        let descender = font.descender;
        let bounds = *self.preview_bounds.lock().unwrap();
        let h: f32 = bounds.size.height.into();
        let scale = (h as f64 * 0.72) / upm;
        let baseline = h as f64 * 0.82 + descender * scale;
        let origin_x: f64 = 24.0;
        let px_x: f32 = (pos.x - bounds.origin.x).into();
        let px_y: f32 = (pos.y - bounds.origin.y).into();
        let x = (px_x as f64 - origin_x) / scale;
        let y = (baseline - px_y as f64) / scale;
        let line_height = self.text_line_height();
        let (top, bottom) = self.text_sort_bounds();
        self.preview_buffer
            .place_cursor_at(x, y, line_height, top, bottom);
        self.text_focus = true;
    }

    /// A key while the preview strip has text focus. Returns true if
    /// consumed.
    fn handle_text_key(&mut self, event: &gpui::KeyDownEvent) -> bool {
        let key = event.keystroke.key.as_str();
        if event.keystroke.modifiers.platform {
            // Cmd shortcuts (save, copy, …) keep working while typing.
            return false;
        }
        if self.font().is_none() {
            return false;
        }
        let line_height = self.text_line_height();
        match key {
            "escape" => {
                self.text_focus = false;
                true
            }
            "backspace" => {
                self.preview_buffer.delete_before_cursor();
                true
            }
            "delete" => {
                self.preview_buffer.delete_after_cursor();
                true
            }
            "left" => {
                self.preview_buffer.move_cursor_visual_left();
                true
            }
            "right" => {
                self.preview_buffer.move_cursor_visual_right();
                true
            }
            "up" => self.preview_buffer.move_cursor_vertically(-1, line_height),
            "down" => self.preview_buffer.move_cursor_vertically(1, line_height),
            "enter" => {
                self.preview_buffer.insert_line_break();
                true
            }
            "comma" | "period" | "," | "." | "<" | ">" => {
                // Kern the pair at the caret, like the old preview.
                // Shifted comma/period arrive as "<" and ">" on some
                // platforms; both mean the 10-unit step.
                let negative = matches!(key, "comma" | "," | "<");
                let big =
                    event.keystroke.modifiers.shift || matches!(key, "<" | ">");
                let delta =
                    if negative { -1.0 } else { 1.0 } * if big { 10.0 } else { 2.0 };
                if let Some((left, right)) = self.preview_kern_pair() {
                    if let Some(font) = self.font_mut() {
                        let value = font.kern_value(&left, &right) + delta;
                        font.set_kern_pair(&left, &right, value);
                    }
                    self.rebuild_text_models();
                }
                true
            }
            _ => {
                let Some(text) = event.keystroke.key_char.as_deref() else {
                    return true; // swallow unhandled keys while typing
                };
                for c in text.chars() {
                    if !c.is_control() {
                        self.preview_buffer.insert_character(c);
                    }
                }
                true
            }
        }
    }

    fn status_bar(&self) -> impl IntoElement + use<> {
        let pair_status: Option<SharedString> = self
            .text_focus
            .then(|| self.preview_kern_pair())
            .flatten()
            .and_then(|(ln, rn)| {
                self.font().map(|font| {
                    let v = font.kern_value(&ln, &rn);
                    format!(
                        "kern {ln}/{rn} = {v:.0} · comma/period adjust (shift = 10)"
                    )
                    .into()
                })
            });
        let text: SharedString = if let Some(note) = &self.status_note {
            note.clone()
        } else if let Some(pair) = pair_status {
            pair
        } else {
            match (&self.mode, self.selected, self.font()) {
                (Mode::Editor(i), _, Some(font)) => {
                    let g = &font.glyphs[*i];
                    let sel = match self.editor.selected.len() {
                        0 => String::new(),
                        n => format!(" · {n} selected"),
                    };
                    let comps = if g.component_names.is_empty() {
                        String::new()
                    } else {
                        format!(
                            " · components: {} (Cmd+Shift+D decomposes)",
                            g.component_names
                                .iter()
                                .map(|n| n.as_ref())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    };
                    let tool = match self.editor.tool {
                        Tool::Select => "V select",
                        Tool::Pen => "P pen: click adds, drag curves, click start closes, Enter ends",
                        Tool::Shapes => "R shapes: drag draws (press again for ellipse)",
                        Tool::Measure => "M measure: drag to read distances",
                        Tool::Text => "T text: type composes · click places caret · double-click a sort edits it · shift-drag kerns",
                        Tool::Knife => "K knife: drag a line to cut contours",
                        Tool::HyperPen => "H hyper pen: click adds smooth, shift-click corner, click start closes, Enter ends",
                        Tool::Preview => "space preview: filled outline, no points",
                    };
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
                    format!("{}{}{} · {tool} · Cmd+Z undo · Cmd+S saves · Esc", g.name, sel, comps).into()
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
            .px_4()
            .py_1()
            .bg(t::panel_bg())
            .border_t_1()
            .border_color(t::cell_border())
            .text_sm()
            .text_color(t::text_muted())
            .child(text)
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
                    self.editor.selected_anchor = None;
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
                        workspace.project = Some(project);
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
                        workspace.project = Some(project);
                        workspace.load_error = None;
                        workspace.mode = Mode::Grid;
                        workspace.selected = None;
                        workspace.status_note = None;
                        workspace.search_query.clear();
                        workspace.rebuild_text_models();
                    for c in "hamburgevons".chars() {
                        workspace.preview_buffer.insert_character(c);
                    }
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
        if self.text_focus && self.handle_text_key(event) {
            return true;
        }

        let key = event.keystroke.key.as_str();
        let cmd = event.keystroke.modifiers.platform;
        let shift = event.keystroke.modifiers.shift;
        let in_editor = matches!(self.mode, Mode::Editor(_));
        let ctrl = event.keystroke.modifiers.control;
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
                if self.editor.pen.is_some() || self.editor.hyper_contour.is_some() {
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
            ("backspace" | "delete", false) if in_editor && self.editor.selected_anchor.is_some() => {
                let Mode::Editor(index) = self.mode else {
                    return false;
                };
                let ai = self.editor.selected_anchor.take().unwrap();
                self.push_undo_snapshot(index);
                if let Some(font) = self.font_mut() {
                    font.delete_anchor(index, ai);
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
            ("left", false) if in_editor => self.nudge_selection(-step, 0.0),
            ("right", false) if in_editor => self.nudge_selection(step, 0.0),
            ("up", false) if in_editor => self.nudge_selection(0.0, step),
            ("down", false) if in_editor => self.nudge_selection(0.0, -step),
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
        if matches!(self.mode, Mode::Editor(_)) {
            self.refresh_metric_inputs(false, window, cx);
            self.refresh_coord_inputs(false, window, cx);
        }
        use gpui_component::resizable::{h_resizable, resizable_panel};

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
                    .child(self.editor_view(index, cx).into_any_element())
                    .child(self.metrics_bar())
                    .into_any_element(),
            ),
            _ => {
                let query = self.search_query.clone();
                let category = self.category;
                let grid: Vec<_> = match self.font() {
                    Some(font) => (0..font.glyphs.len())
                        .filter(|&i| {
                            let entry = &font.glyphs[i];
                            let category_ok = category
                                == runebender_core::category::GlyphCategory::All
                                || entry.codepoint.map_or(
                                    category
                                        == runebender_core::category::GlyphCategory::Other,
                                    |c| {
                                        runebender_core::category::GlyphCategory::from_codepoint(c)
                                            == category
                                    },
                                );
                            category_ok
                                && (query.is_empty()
                                    || entry.name.to_lowercase().contains(&query))
                        })
                        .map(|i| self.glyph_cell(i, cx).into_any_element())
                        .collect(),
                    None => Vec::new(),
                };
                (
                    self.category_sidebar(cx).into_any_element(),
                    div()
                        .id("glyph-grid")
                        .size_full()
                        .min_h(px(0.0))
                        .overflow_y_scroll()
                        .child(div().flex().flex_wrap().gap_2().p_3().children(grid))
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
                    .child(self.transform_section(cx))
                    .child(self.layers_section(cx))
                    .child(self.mark_colors_panel(cx))
            })
            .when(!in_editor, |el| {
                el.child(self.glyph_info_panel(cx))
                    .child(self.layers_section(cx))
                    .child(self.mark_colors_panel(cx))
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
                                div()
                                    .size_full()
                                    .bg(t::panel_bg())
                                    .border_r_1()
                                    .border_color(t::panel_outline())
                                    .child(left),
                            ),
                    )
                    .child(resizable_panel().child(center))
                    .child(
                        resizable_panel()
                            .size(px(230.0))
                            .size_range(px(170.0)..px(440.0))
                            .child(
                                div()
                                    .size_full()
                                    .bg(t::panel_bg())
                                    .border_l_1()
                                    .border_color(t::panel_outline())
                                    .child(right),
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
                this.command_paste();
                cx.notify();
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
                if event.keystroke.key.as_str() == "space"
                    && this.editor.tool == Tool::Preview
                {
                    this.editor.tool = this.editor.previous_tool;
                    cx.notify();
                }
            }))
            .child(self.header(cx))
            .child(content)
            .child(self.axes_bar())
            .child(self.preview_strip(cx))
            .child(self.status_bar())
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
                                cx.notify();
                            }
                        }
                    });
                    let mut workspace = Workspace {
                        project,
                        load_error,
                        selected: None,
                        category: runebender_core::category::GlyphCategory::All,
                        mode: start_mode,
                        editor: EditorState::new(),
                        preview_buffer: runebender_core::text::TextBuffer::new(),
                        edit_buffer: runebender_core::text::TextBuffer::new(),
                        text_focus: false,
                        collapsed_sections: std::collections::HashSet::new(),
                        reference_layers: std::collections::HashSet::new(),
                        left_collapsed: false,
                        #[cfg(not(target_os = "macos"))]
                        app_menu_bar: app_menu_bar.clone(),
                        focus_handle: cx.focus_handle(),
                        status_note: None,
                        search,
                        search_query: String::new(),
                        metric_inputs: MetricInputs {
                            width: width_input,
                            lsb: lsb_input,
                            rsb: rsb_input,
                            x: x_input,
                            y: y_input,
                        },
                        preview_bounds: Arc::new(Mutex::new(Bounds::default())),
                        axis_sliders: Vec::new(),
                        clipboard: Vec::new(),
                        #[cfg(target_family = "wasm")]
                        web_host: None,
                        _watcher: None,
                        last_save: Arc::new(Mutex::new(web_time::Instant::now())),
                        _subscriptions: vec![
                            subscription, sub_w, sub_l, sub_r, sub_x, sub_y,
                        ],
                    };
                    workspace.rebuild_text_models();
                    for c in "hamburgevons".chars() {
                        workspace.preview_buffer.insert_character(c);
                    }
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
                            ("z", false) => {
                                this.undo();
                                this.rebuild_text_models();
                            }
                            ("z", true) => {
                                this.redo();
                                this.rebuild_text_models();
                            }
                            ("c", false) => this.command_copy(),
                            ("v", false) => this.command_paste(),
                            ("o", true) => this.command_remove_overlap(),
                            ("d", true) => this.command_decompose(),
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
        model.constrain_smooth_neighbor(index, c, incoming);
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
        let base = model.kern_value("A", "V");
        model.set_kern_pair("A", "V", base - 14.0);
        assert_eq!(model.kern_value("A", "V"), base - 14.0);
        assert!(model.dirty);
        // Unrelated pair unaffected by the exception.
        let _ = model.kern_value("o", "o");
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
