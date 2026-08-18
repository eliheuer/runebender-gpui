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
    canvas, div, prelude::*, px, size, App, Bounds, Context, MouseButton,
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

    /// Clone a glyph's contours for undo snapshots.
    fn snapshot_contours(&self, glyph_index: usize) -> Option<Vec<norad::Contour>> {
        let name = self.glyphs[glyph_index].name.to_string();
        self.font
            .get_glyph(name.as_str())
            .map(|g| g.contours.clone())
    }

    /// Replace a glyph's contours (undo/redo) and rebuild caches.
    fn restore_contours(&mut self, glyph_index: usize, contours: Vec<norad::Contour>) {
        let name = self.glyphs[glyph_index].name.to_string();
        if let Some(glyph) = self.font.default_layer_mut().get_glyph_mut(name.as_str()) {
            glyph.contours = contours;
            self.dirty = true;
        }
        self.rebuild_entry(glyph_index);
    }

    /// Set several points at once (multi-point drag): each entry is
    /// ((contour, index), new position).
    fn set_points(&mut self, glyph_index: usize, updates: &[((usize, usize), (f64, f64))]) {
        let name = self.glyphs[glyph_index].name.to_string();
        let Some(glyph) = self.font.default_layer_mut().get_glyph_mut(name.as_str()) else {
            return;
        };
        for ((contour, index), (x, y)) in updates {
            if let Some(point) = glyph
                .contours
                .get_mut(*contour)
                .and_then(|c| c.points.get_mut(*index))
            {
                point.x = *x;
                point.y = *y;
            }
        }
        self.dirty = true;
        self.rebuild_entry(glyph_index);
    }

    /// Start a new open contour at (x, y). Returns its index.
    fn start_contour(&mut self, glyph_index: usize, x: f64, y: f64) -> Option<usize> {
        let name = self.glyphs[glyph_index].name.to_string();
        let glyph = self.font.default_layer_mut().get_glyph_mut(name.as_str())?;
        let point = norad::ContourPoint::new(x, y, norad::PointType::Move, false, None, None);
        glyph.contours.push(norad::Contour::new(vec![point], None));
        let contour = glyph.contours.len() - 1;
        self.dirty = true;
        self.rebuild_entry(glyph_index);
        Some(contour)
    }

    /// Append points to an open contour (pen tool). Pass the two
    /// off-curve controls for a curve segment, or none for a line.
    fn append_segment(
        &mut self,
        glyph_index: usize,
        contour: usize,
        controls: Option<((f64, f64), (f64, f64))>,
        x: f64,
        y: f64,
        smooth: bool,
    ) {
        let name = self.glyphs[glyph_index].name.to_string();
        let Some(glyph) = self.font.default_layer_mut().get_glyph_mut(name.as_str()) else {
            return;
        };
        let Some(c) = glyph.contours.get_mut(contour) else {
            return;
        };
        let typ = if controls.is_some() {
            norad::PointType::Curve
        } else {
            norad::PointType::Line
        };
        if let Some(((c1x, c1y), (c2x, c2y))) = controls {
            c.points.push(norad::ContourPoint::new(
                c1x,
                c1y,
                norad::PointType::OffCurve,
                false,
                None,
                None,
            ));
            c.points.push(norad::ContourPoint::new(
                c2x,
                c2y,
                norad::PointType::OffCurve,
                false,
                None,
                None,
            ));
        }
        c.points
            .push(norad::ContourPoint::new(x, y, typ, smooth, None, None));
        self.dirty = true;
        self.rebuild_entry(glyph_index);
    }

    /// Close an open contour: the Move start point becomes the final
    /// segment's target. `controls` curves the closing segment.
    fn close_contour(
        &mut self,
        glyph_index: usize,
        contour: usize,
        controls: Option<((f64, f64), (f64, f64))>,
    ) {
        let name = self.glyphs[glyph_index].name.to_string();
        let Some(glyph) = self.font.default_layer_mut().get_glyph_mut(name.as_str()) else {
            return;
        };
        let Some(c) = glyph.contours.get_mut(contour) else {
            return;
        };
        let Some(first) = c.points.first_mut() else {
            return;
        };
        if first.typ != norad::PointType::Move {
            return;
        }
        // In UFO, a closed contour simply has no Move point: the
        // final segment wraps around to the first point.
        first.typ = if controls.is_some() {
            norad::PointType::Curve
        } else {
            norad::PointType::Line
        };
        if let Some(((c1x, c1y), (c2x, c2y))) = controls {
            c.points.push(norad::ContourPoint::new(
                c1x,
                c1y,
                norad::PointType::OffCurve,
                false,
                None,
                None,
            ));
            c.points.push(norad::ContourPoint::new(
                c2x,
                c2y,
                norad::PointType::OffCurve,
                false,
                None,
                None,
            ));
        }
        self.dirty = true;
        self.rebuild_entry(glyph_index);
    }

    /// Delete an unfinished pen contour (Esc while drawing a single
    /// stray point, for example).
    fn remove_contour_if_degenerate(&mut self, glyph_index: usize, contour: usize) {
        let name = self.glyphs[glyph_index].name.to_string();
        let Some(glyph) = self.font.default_layer_mut().get_glyph_mut(name.as_str()) else {
            return;
        };
        if glyph
            .contours
            .get(contour)
            .is_some_and(|c| c.points.len() < 2)
        {
            glyph.contours.remove(contour);
            self.dirty = true;
            self.rebuild_entry(glyph_index);
        }
    }

    /// Delete the given points. Selected on-curve points vanish with
    /// their incoming controls (neighbors reconnect); selected
    /// off-curve points turn their segment into a line. Contours left
    /// without segments are removed. Returns true if anything changed.
    fn delete_points(
        &mut self,
        glyph_index: usize,
        selected: &std::collections::HashSet<(usize, usize)>,
    ) -> bool {
        if selected.is_empty() {
            return false;
        }
        let name = self.glyphs[glyph_index].name.to_string();
        let Some(glyph) = self.font.default_layer_mut().get_glyph_mut(name.as_str()) else {
            return false;
        };
        let mut changed = false;
        let mut contour_index = 0usize;
        glyph.contours.retain_mut(|contour| {
            let ci = contour_index;
            contour_index += 1;
            let any_here = selected.iter().any(|(c, _)| *c == ci);
            if !any_here {
                return true;
            }
            changed = true;

            // Parse into segments anchored at on-curve points.
            struct Seg {
                x: f64,
                y: f64,
                smooth: bool,
                controls: Option<((f64, f64), (f64, f64))>,
                on_index: usize,
                control_indices: Vec<usize>,
                is_move: bool,
            }
            let closed = contour
                .points
                .first()
                .is_none_or(|p| p.typ != norad::PointType::Move);
            let mut segs: Vec<Seg> = Vec::new();
            let mut pending: Vec<(usize, (f64, f64))> = Vec::new();
            for (i, p) in contour.points.iter().enumerate() {
                match p.typ {
                    norad::PointType::OffCurve => pending.push((i, (p.x, p.y))),
                    _ => {
                        let controls = if pending.len() == 2 {
                            Some((pending[0].1, pending[1].1))
                        } else {
                            None
                        };
                        segs.push(Seg {
                            x: p.x,
                            y: p.y,
                            smooth: p.smooth,
                            controls,
                            on_index: i,
                            control_indices: pending.iter().map(|(i, _)| *i).collect(),
                            is_move: p.typ == norad::PointType::Move,
                        });
                        pending.clear();
                    }
                }
            }
            // Closed contours may carry trailing off-curves that wrap
            // to the first on-curve point.
            if closed && pending.len() == 2 && !segs.is_empty() {
                segs[0].controls = Some((pending[0].1, pending[1].1));
                segs[0].control_indices = pending.iter().map(|(i, _)| *i).collect();
            }

            // Apply the deletions.
            segs.retain(|seg| !selected.contains(&(ci, seg.on_index)));
            for seg in segs.iter_mut() {
                if seg
                    .control_indices
                    .iter()
                    .any(|i| selected.contains(&(ci, *i)))
                {
                    seg.controls = None;
                }
            }
            if segs.is_empty() {
                return false; // drop the contour
            }

            // Reserialize.
            let mut points: Vec<norad::ContourPoint> = Vec::new();
            let n = segs.len();
            for (k, seg) in segs.iter().enumerate() {
                let is_first = k == 0;
                // An open contour starts with a bare Move point; its
                // controls (if the old first point was deleted) drop.
                let controls = if !closed && is_first { None } else { seg.controls };
                let typ = if !closed && is_first {
                    norad::PointType::Move
                } else if controls.is_some() {
                    norad::PointType::Curve
                } else {
                    norad::PointType::Line
                };
                // For closed contours the wrap-around controls of the
                // first on-curve point go at the END of the list.
                if let (Some((c1, c2)), false) = (controls, closed && is_first) {
                    points.push(norad::ContourPoint::new(
                        c1.0,
                        c1.1,
                        norad::PointType::OffCurve,
                        false,
                        None,
                        None,
                    ));
                    points.push(norad::ContourPoint::new(
                        c2.0,
                        c2.1,
                        norad::PointType::OffCurve,
                        false,
                        None,
                        None,
                    ));
                }
                points.push(norad::ContourPoint::new(
                    seg.x, seg.y, typ, seg.smooth, None, None,
                ));
                let _ = seg.is_move;
                let _ = n;
            }
            if closed {
                if let Some((c1, c2)) = segs[0].controls {
                    points.push(norad::ContourPoint::new(
                        c1.0,
                        c1.1,
                        norad::PointType::OffCurve,
                        false,
                        None,
                        None,
                    ));
                    points.push(norad::ContourPoint::new(
                        c2.0,
                        c2.1,
                        norad::PointType::OffCurve,
                        false,
                        None,
                        None,
                    ));
                }
                // First point's type reflects its wrap-around controls.
                if let Some(first) = points.first_mut() {
                    first.typ = if segs[0].controls.is_some() {
                        norad::PointType::Curve
                    } else {
                        norad::PointType::Line
                    };
                }
            }
            contour.points = points;
            true
        });
        if changed {
            self.dirty = true;
            self.rebuild_entry(glyph_index);
        }
        changed
    }

    /// Toggle smooth/corner on the given on-curve points.
    fn toggle_smooth(
        &mut self,
        glyph_index: usize,
        selected: &std::collections::HashSet<(usize, usize)>,
    ) -> bool {
        let name = self.glyphs[glyph_index].name.to_string();
        let Some(glyph) = self.font.default_layer_mut().get_glyph_mut(name.as_str()) else {
            return false;
        };
        let mut changed = false;
        for (ci, contour) in glyph.contours.iter_mut().enumerate() {
            for (pi, p) in contour.points.iter_mut().enumerate() {
                if p.typ != norad::PointType::OffCurve && selected.contains(&(ci, pi)) {
                    p.smooth = !p.smooth;
                    changed = true;
                }
            }
        }
        if changed {
            self.dirty = true;
            self.rebuild_entry(glyph_index);
        }
        changed
    }

    fn save(&mut self) -> Result<(), norad::error::FontWriteError> {
        self.font.save(&self.source_path)?;
        self.dirty = false;
        Ok(())
    }
}

// ============================================================================
// PROJECT (designspace or single UFO)
// ============================================================================

/// An open project: one or more master UFOs, optionally tied together
/// by a designspace document.
struct Project {
    masters: Vec<FontModel>,
    active: usize,
    /// Style names for the master switcher, one per master.
    master_names: Vec<SharedString>,
}

impl Project {
    fn load(path: &std::path::Path) -> Result<Self, String> {
        if path.extension().is_some_and(|e| e == "designspace") {
            let doc = norad::designspace::DesignSpaceDocument::load(path)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            let dir = path.parent().unwrap_or(std::path::Path::new("."));
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
            for source in &doc.sources {
                if !seen.insert(source.filename.clone()) {
                    continue; // per-layer duplicate source entries
                }
                let ufo_path = dir.join(&source.filename);
                let model = FontModel::load(&ufo_path)
                    .map_err(|e| format!("{}: {e}", ufo_path.display()))?;
                let is_default = source.location.iter().all(|d| {
                    let value = d.xvalue.or(d.uservalue).unwrap_or(0.0);
                    defaults
                        .get(d.name.as_str())
                        .is_some_and(|v| (*v - value).abs() < f32::EPSILON)
                });
                if is_default {
                    default_index = masters.len();
                }
                let name = source
                    .stylename
                    .clone()
                    .unwrap_or_else(|| source.filename.clone());
                masters.push(model);
                master_names.push(name.into());
            }
            if masters.is_empty() {
                return Err(format!("{}: no sources", path.display()));
            }
            Ok(Self {
                active: default_index,
                masters,
                master_names,
            })
        } else {
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
            })
        }
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

/// The active editor tool.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Select,
    Pen,
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
    /// Rubber-band selection rectangle, in design space.
    Marquee {
        start: (f64, f64),
        current: (f64, f64),
    },
}

/// Editor viewport and interaction state. `zoom` is pixels per font
/// unit; `pan` is the local-pixel position of the design origin
/// (glyph left sidebearing at baseline).
struct EditorState {
    zoom: f64,
    pan: (f64, f64),
    initialized: bool,
    tool: Tool,
    pen: Option<PenState>,
    selected: std::collections::HashSet<(usize, usize)>,
    drag: Option<Drag>,
    /// Undo/redo stacks of contour snapshots for the open glyph.
    undo: Vec<Vec<norad::Contour>>,
    redo: Vec<Vec<norad::Contour>>,
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
            tool: Tool::Select,
            pen: None,
            selected: std::collections::HashSet::new(),
            drag: None,
            undo: Vec::new(),
            redo: Vec::new(),
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
    project: Option<Project>,
    load_error: Option<SharedString>,
    selected: Option<usize>,
    mode: Mode,
    editor: EditorState,
    focus_handle: gpui::FocusHandle,
    status_note: Option<SharedString>,
    search: gpui::Entity<gpui_component::input::InputState>,
    search_query: String,
    _subscriptions: Vec<gpui::Subscription>,
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
    }

    fn open_editor(&mut self, index: usize) {
        self.mode = Mode::Editor(index);
        self.editor.initialized = false;
        self.editor.selected.clear();
        self.editor.drag = None;
        self.editor.undo.clear();
        self.editor.redo.clear();
        self.editor.tool = Tool::Select;
        self.editor.pen = None;
    }

    fn glyph_cell(&self, index: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let font = self.font().unwrap();
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
        let font = self.font().unwrap();
        let entry = &font.glyphs[index];
        let outline = entry.path.clone();
        let points = entry.points.clone();
        let advance = entry.advance;
        let ascender = font.ascender;
        let descender = font.descender;

        let transform = self.editor.transform();
        let zoom = self.editor.zoom;
        let selected_points = self.editor.selected.clone();
        let marquee = match &self.editor.drag {
            Some(Drag::Marquee { start, current }) => Some((*start, *current)),
            _ => None,
        };
        let bounds_slot = self.editor.bounds.clone();
        let needs_fit = !self.editor.initialized;

        let tool = self.editor.tool;
        let tool_button = |id: &'static str, label_text: &'static str, this_tool: Tool| {
            div()
                .id(id)
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(if tool == this_tool { t::accent() } else { t::cell_border() })
                .text_color(if tool == this_tool { t::text() } else { t::text_muted() })
                .text_sm()
                .cursor_pointer()
                .child(label_text)
        };

        div()
            .flex_1()
            .relative()
            .child(
                div()
                    .absolute()
                    .top_2()
                    .left_2()
                    .flex()
                    .gap_1()
                    .child(tool_button("tool-select", "Select", Tool::Select).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.pen_finish();
                            this.editor.tool = Tool::Select;
                            cx.notify();
                        }),
                    ))
                    .child(tool_button("tool-pen", "Pen", Tool::Pen).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.editor.tool = Tool::Pen;
                            cx.notify();
                        },
                    ))),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    this.editor_mouse_down(event.position, event.modifiers.shift);
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
                                selected_points.contains(&(p.contour, p.index));
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
                        // Marquee rectangle.
                        if let Some((a, b)) = marquee {
                            let pa = to_screen(a.0, a.1);
                            let pb = to_screen(b.0, b.1);
                            let rect = Bounds::from_corners(
                                gpui::point(pa.x.min(pb.x), pa.y.min(pb.y)),
                                gpui::point(pa.x.max(pb.x), pa.y.max(pb.y)),
                            );
                            window.paint_quad(gpui::fill(rect, t::marquee_fill()));
                            window.paint_quad(
                                gpui::outline(rect, t::accent(), gpui::BorderStyle::Solid),
                            );
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
        let Some(font) = self.font() else {
            return;
        };
        let entry = &font.glyphs[index];
        let (advance, asc, desc) = (entry.advance, font.ascender, font.descender);
        self.editor.fit(advance, asc, desc);
    }

    fn editor_mouse_down(&mut self, pos: Point<gpui::Pixels>, shift: bool) {
        self.ensure_editor_fit();
        let Mode::Editor(index) = self.mode else {
            return;
        };
        if self.editor.tool == Tool::Pen {
            self.pen_mouse_down(index, pos);
            return;
        }
        let Some(font) = self.font() else {
            return;
        };
        let (dx, dy) = self.editor.window_to_design(pos);
        let tolerance = HIT_RADIUS_PX / self.editor.zoom;
        // Copy the point data out so selection can mutate afterwards.
        let all_points: Vec<((usize, usize), (f64, f64))> = font.glyphs[index]
            .points
            .iter()
            .map(|p| ((p.contour, p.index), (p.x, p.y)))
            .collect();
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
        match &mut self.editor.drag {
            Some(Drag::Points { start, originals }) => {
                let (sx, sy) = *start;
                let updates: Vec<((usize, usize), (f64, f64))> = originals
                    .iter()
                    .map(|(id, (ox, oy))| {
                        (*id, ((ox + dx - sx).round(), (oy + dy - sy).round()))
                    })
                    .collect();
                if let Some(font) = self.font_mut() {
                    font.set_points(index, &updates);
                    return true;
                }
                false
            }
            Some(Drag::Marquee { current, .. }) => {
                *current = (dx, dy);
                true
            }
            None => false,
        }
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
    fn pen_mouse_down(&mut self, index: usize, pos: Point<gpui::Pixels>) {
        let (dx, dy) = self.editor.window_to_design(pos);
        let (x, y) = (dx.round(), dy.round());
        let tolerance = HIT_RADIUS_PX / self.editor.zoom;
        self.push_undo_snapshot(index);

        match self.editor.pen.take() {
            None => {
                if let Some(font) = self.font_mut() {
                    if let Some(contour) = font.start_contour(index, x, y) {
                        self.editor.pen = Some(PenState {
                            contour,
                            prev_out_handle: None,
                            placing: Some((x, y)),
                        });
                    }
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
        if let (Some(updates), Some(font)) = (updates, self.font_mut()) {
            if !updates.is_empty() {
                font.set_points(index, &updates);
            }
        }
        true
    }

    /// Finish an open pen contour without closing it.
    fn pen_finish(&mut self) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        if let Some(pen) = self.editor.pen.take() {
            if let Some(font) = self.font_mut() {
                font.remove_contour_if_degenerate(index, pen.contour);
            }
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

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let (title, subtitle) = match (self.font(), &self.load_error) {
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
            .child(div().flex_1())
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

    fn status_bar(&self) -> impl IntoElement + use<> {
        let text: SharedString = if let Some(note) = &self.status_note {
            note.clone()
        } else {
            match (&self.mode, self.selected, self.font()) {
                (Mode::Editor(i), _, Some(font)) => {
                    let g = &font.glyphs[*i];
                    let sel = match self.editor.selected.len() {
                        0 => String::new(),
                        n => format!(" · {n} selected"),
                    };
                    let tool = match self.editor.tool {
                        Tool::Select => "V select",
                        Tool::Pen => "P pen: click adds, drag curves, click start closes, Enter ends",
                    };
                    format!("{}{} · {tool} · Cmd+Z undo · Cmd+S saves · Esc", g.name, sel).into()
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
        let shift = event.keystroke.modifiers.shift;
        let in_editor = matches!(self.mode, Mode::Editor(_));
        let step = if shift { 10.0 } else { 1.0 };
        match (key, cmd) {
            ("escape", _) if in_editor => {
                if self.editor.pen.is_some() {
                    self.pen_finish();
                } else {
                    self.mode = Mode::Grid;
                    self.status_note = None;
                }
                true
            }
            ("enter", _) if in_editor && self.editor.pen.is_some() => {
                self.pen_finish();
                true
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
            ("z", true) if in_editor => {
                if shift {
                    self.redo();
                } else {
                    self.undo();
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
            ("s", true) => {
                // Save every dirty master.
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
                    self.status_note = Some(if !failed.is_empty() {
                        format!("Save failed: {}", failed.join("; ")).into()
                    } else if saved.is_empty() {
                        "Nothing to save".into()
                    } else {
                        format!("Saved {}", saved.join(", ")).into()
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
        // Claim focus only when nothing else has it, so text inputs
        // (the search box) keep theirs while focused.
        if window.focused(cx).is_none() {
            window.focus(&self.focus_handle, cx);
        }

        let content = match self.mode {
            Mode::Editor(index) if self.project.is_some() => {
                self.editor_view(index, cx).into_any_element()
            }
            _ => {
                let query = self.search_query.clone();
                let grid: Vec<_> = match self.font() {
                    Some(font) => (0..font.glyphs.len())
                        .filter(|&i| {
                            query.is_empty()
                                || font.glyphs[i].name.to_lowercase().contains(&query)
                        })
                        .map(|i| self.glyph_cell(i, cx).into_any_element())
                        .collect(),
                    None => Vec::new(),
                };
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .child(
                        div()
                            .px_4()
                            .py_2()
                            .w(px(320.0))
                            .child(gpui_component::input::Input::new(&self.search)),
                    )
                    .child(
                        div()
                            .id("glyph-grid")
                            .flex_1()
                            .overflow_y_scroll()
                            .child(div().flex().flex_wrap().gap_2().p_4().children(grid)),
                    )
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
            .child(self.header(cx))
            .child(content)
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

fn main() {
    let font_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(default_font_path);

    let (project, load_error) = match Project::load(&font_path) {
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

    gpui_platform::application()
        .with_assets(gpui_component_assets::Assets)
        .run(move |cx: &mut App| {
        gpui_component::init(cx);
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
                    Workspace {
                        project,
                        load_error,
                        selected: None,
                        mode: start_mode,
                        editor: EditorState::new(),
                        focus_handle: cx.focus_handle(),
                        status_note: None,
                        search,
                        search_query: String::new(),
                        _subscriptions: vec![subscription],
                    }
                });
                cx.new(|cx| gpui_component::Root::new(workspace, window, cx))
            },
        )
        .unwrap();
        cx.activate(true);
    });
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
        let base_contours = model.snapshot_contours(index).unwrap().len();

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

        let contours = model.snapshot_contours(index).unwrap();
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
            model.snapshot_contours(index).unwrap().len(),
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
            m.snapshot_contours(index).unwrap()[c].points.len()
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
        let contour_data = &model.snapshot_contours(index).unwrap()[c];
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
        assert!(model.snapshot_contours(index).unwrap().len() <= c);
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
