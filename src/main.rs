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
    anchors: Arc<Vec<(SharedString, f64, f64)>>,
    advance: f64,
}

struct FontModel {
    font: norad::Font,
    /// codepoint → index into `glyphs`, for the text preview.
    codepoint_map: std::collections::HashMap<char, usize>,
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
                anchors: Arc::new(extract_anchors(glyph)),
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

        let codepoint_map = glyphs
            .iter()
            .enumerate()
            .filter_map(|(i, g)| g.codepoint.map(|c| (c, i)))
            .collect();

        Ok(Self {
            font,
            codepoint_map,
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
        let glyph_advance = glyph.width;
        let path = Arc::new(glyph_path::glyph_to_bezpath(glyph, &self.font));
        let points = Arc::new(extract_points(glyph));
        let anchors = Arc::new(extract_anchors(glyph));
        let entry = &mut self.glyphs[glyph_index];
        entry.path = path;
        entry.points = points;
        entry.anchors = anchors;
        entry.advance = glyph_advance;
    }

    /// Clone a glyph's editable state for undo snapshots.
    fn snapshot_contours(&self, glyph_index: usize) -> Option<GlyphSnapshot> {
        let name = self.glyphs[glyph_index].name.to_string();
        self.font.get_glyph(name.as_str()).map(|g| GlyphSnapshot {
            contours: g.contours.clone(),
            anchors: g.anchors.clone(),
            width: g.width,
        })
    }

    /// Replace a glyph's editable state (undo/redo) and rebuild caches.
    fn restore_contours(&mut self, glyph_index: usize, snapshot: GlyphSnapshot) {
        let name = self.glyphs[glyph_index].name.to_string();
        if let Some(glyph) = self.font.default_layer_mut().get_glyph_mut(name.as_str()) {
            glyph.contours = snapshot.contours;
            glyph.anchors = snapshot.anchors;
            glyph.width = snapshot.width;
            self.dirty = true;
        }
        self.rebuild_entry(glyph_index);
    }

    fn set_anchor(&mut self, glyph_index: usize, anchor: usize, x: f64, y: f64) {
        let name = self.glyphs[glyph_index].name.to_string();
        if let Some(glyph) = self.font.default_layer_mut().get_glyph_mut(name.as_str()) {
            if let Some(a) = glyph.anchors.get_mut(anchor) {
                a.x = x;
                a.y = y;
                self.dirty = true;
            }
        }
        self.rebuild_entry(glyph_index);
    }

    fn add_anchor(&mut self, glyph_index: usize, x: f64, y: f64) {
        let name = self.glyphs[glyph_index].name.to_string();
        if let Some(glyph) = self.font.default_layer_mut().get_glyph_mut(name.as_str()) {
            let n = glyph.anchors.len();
            let anchor_name = norad::Name::new(&format!("anchor.{n}")).ok();
            glyph
                .anchors
                .push(norad::Anchor::new(x, y, anchor_name, None, None));
            self.dirty = true;
        }
        self.rebuild_entry(glyph_index);
    }

    fn delete_anchor(&mut self, glyph_index: usize, anchor: usize) {
        let name = self.glyphs[glyph_index].name.to_string();
        if let Some(glyph) = self.font.default_layer_mut().get_glyph_mut(name.as_str()) {
            if anchor < glyph.anchors.len() {
                glyph.anchors.remove(anchor);
                self.dirty = true;
            }
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

    /// Apply a curve-quality operation (shared geometry from
    /// runebender-core) to the selected points, or to the whole glyph
    /// when the selection is empty. Only closed all-cubic contours
    /// participate.
    fn curve_op(
        &mut self,
        glyph_index: usize,
        selected: &std::collections::HashSet<(usize, usize)>,
        op: CurveOp,
    ) -> bool {
        use runebender_core::curve::{OptPoint, balance, harmonize, optimize_contour};
        let all = selected.is_empty();
        let name = self.glyphs[glyph_index].name.to_string();
        let Some(glyph) = self.font.default_layer_mut().get_glyph_mut(name.as_str()) else {
            return false;
        };
        let mut changed = false;
        for (ci, contour) in glyph.contours.iter_mut().enumerate() {
            if !contour.is_closed() {
                continue;
            }
            let pts = &mut contour.points;
            let n = pts.len();
            if n < 4 {
                continue;
            }
            let on = |p: &norad::ContourPoint| p.typ != norad::PointType::OffCurve;
            let in_scope = |i: usize| all || selected.contains(&(ci, i));
            match op {
                CurveOp::Harmonize => {
                    let mut updates: Vec<(usize, kurbo::Point)> = Vec::new();
                    for i in 0..n {
                        if !on(&pts[i]) || !pts[i].smooth || !in_scope(i) {
                            continue;
                        }
                        let (a1, a2, b1, b2) =
                            ((i + n - 2) % n, (i + n - 1) % n, (i + 1) % n, (i + 2) % n);
                        if on(&pts[a1]) || on(&pts[a2]) || on(&pts[b1]) || on(&pts[b2]) {
                            continue;
                        }
                        let point = |k: usize| kurbo::Point::new(pts[k].x, pts[k].y);
                        if let Some((na2, nb1)) =
                            harmonize(point(a1), point(a2), point(i), point(b1), point(b2))
                        {
                            updates.push((a2, na2.round()));
                            updates.push((b1, nb1.round()));
                        }
                    }
                    for (k, p) in updates {
                        pts[k].x = p.x;
                        pts[k].y = p.y;
                        changed = true;
                    }
                }
                CurveOp::Balance => {
                    let mut updates: Vec<(usize, kurbo::Point)> = Vec::new();
                    for i in 0..n {
                        let (b, c, d) = ((i + 1) % n, (i + 2) % n, (i + 3) % n);
                        if !on(&pts[i]) || on(&pts[b]) || on(&pts[c]) || !on(&pts[d]) {
                            continue;
                        }
                        if !(in_scope(i) || in_scope(b) || in_scope(c) || in_scope(d)) {
                            continue;
                        }
                        let point = |k: usize| kurbo::Point::new(pts[k].x, pts[k].y);
                        if let Some((np1, np2)) = balance(point(i), point(b), point(c), point(d))
                        {
                            updates.push((b, np1.round()));
                            updates.push((c, np2.round()));
                        }
                    }
                    for (k, p) in updates {
                        pts[k].x = p.x;
                        pts[k].y = p.y;
                        changed = true;
                    }
                }
                CurveOp::Optimize(tol) => {
                    if !all && !(0..n).any(in_scope) {
                        continue;
                    }
                    let opts: Vec<OptPoint> = pts
                        .iter()
                        .map(|p| OptPoint {
                            p: kurbo::Point::new(p.x, p.y),
                            on: on(p),
                            smooth: p.smooth,
                        })
                        .collect();
                    let newpos = optimize_contour(&opts, tol);
                    for (i, p) in pts.iter_mut().enumerate() {
                        if p.typ == norad::PointType::OffCurve
                            && (kurbo::Point::new(p.x, p.y) - newpos[i]).hypot() > 1e-6
                        {
                            p.x = newpos[i].x;
                            p.y = newpos[i].y;
                            changed = true;
                        }
                    }
                }
            }
        }
        if changed {
            self.dirty = true;
            self.rebuild_entry(glyph_index);
        }
        changed
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

    /// Shift all of a glyph's ink horizontally (LSB edits). Component
    /// references shift via their transform offset.
    fn shift_ink(&mut self, glyph_index: usize, dx: f64) {
        let name = self.glyphs[glyph_index].name.to_string();
        if let Some(glyph) = self.font.default_layer_mut().get_glyph_mut(name.as_str()) {
            for contour in glyph.contours.iter_mut() {
                for p in contour.points.iter_mut() {
                    p.x += dx;
                }
            }
            for component in glyph.components.iter_mut() {
                component.transform.x_offset += dx;
            }
            self.dirty = true;
        }
        self.rebuild_entry(glyph_index);
    }

    fn rebuild_metrics(&mut self, glyph_index: usize) {
        let name = self.glyphs[glyph_index].name.to_string();
        if let Some(glyph) = self.font.get_glyph(name.as_str()) {
            self.glyphs[glyph_index].advance = glyph.width;
        }
    }

    /// After moving one off-curve handle, keep its sibling handle
    /// collinear through the shared smooth on-curve point (length
    /// preserved). No-op when the shared point is a corner.
    fn constrain_smooth_neighbor(&mut self, glyph_index: usize, contour: usize, index: usize) {
        let name = self.glyphs[glyph_index].name.to_string();
        let Some(glyph) = self.font.default_layer_mut().get_glyph_mut(name.as_str()) else {
            return;
        };
        let Some(c) = glyph.contours.get_mut(contour) else {
            return;
        };
        let n = c.points.len();
        if n < 4 {
            return;
        }
        let closed = c.points.first().is_none_or(|p| p.typ != norad::PointType::Move);
        let step = |i: usize, d: isize| -> Option<usize> {
            let j = i as isize + d;
            if closed {
                Some(((j % n as isize + n as isize) % n as isize) as usize)
            } else if (0..n as isize).contains(&j) {
                Some(j as usize)
            } else {
                None
            }
        };
        let is_off = |p: &norad::ContourPoint| p.typ == norad::PointType::OffCurve;
        if !is_off(&c.points[index]) {
            return;
        }
        // Find the on-curve anchor this handle attaches to and the
        // sibling handle on the anchor's other side.
        let mut fix = None;
        if let (Some(a), Some(sib)) = (step(index, 1), step(index, 2)) {
            if !is_off(&c.points[a]) && c.points[a].smooth && is_off(&c.points[sib]) {
                fix = Some((a, sib));
            }
        }
        if fix.is_none() {
            if let (Some(a), Some(sib)) = (step(index, -1), step(index, -2)) {
                if !is_off(&c.points[a]) && c.points[a].smooth && is_off(&c.points[sib]) {
                    fix = Some((a, sib));
                }
            }
        }
        let Some((a, sib)) = fix else {
            return;
        };
        let anchor = kurbo::Point::new(c.points[a].x, c.points[a].y);
        let dragged = kurbo::Point::new(c.points[index].x, c.points[index].y);
        let sibling_pt = kurbo::Point::new(c.points[sib].x, c.points[sib].y);
        let dir = anchor - dragged;
        let len = dir.hypot();
        if len < 1e-6 {
            return;
        }
        let unit = dir / len;
        let sib_len = (sibling_pt - anchor).hypot();
        let new_sib = anchor + unit * sib_len;
        c.points[sib].x = new_sib.x.round();
        c.points[sib].y = new_sib.y.round();
        self.dirty = true;
        self.rebuild_entry(glyph_index);
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

/// One undo step: a glyph's full editable state.
#[derive(Clone)]
struct GlyphSnapshot {
    contours: Vec<norad::Contour>,
    anchors: Vec<norad::Anchor>,
    width: f64,
}

/// Which metric field is being edited.
#[derive(Clone, Copy)]
enum MetricField {
    Width,
    Lsb,
    Rsb,
}

/// A curve-quality operation from `runebender_core::curve`.
#[derive(Clone, Copy)]
enum CurveOp {
    Harmonize,
    Balance,
    Optimize(f64),
}

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
    /// Dragging an anchor.
    Anchor {
        index: usize,
        start: (f64, f64),
        orig: (f64, f64),
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
            zoom: 1.0,
            pan: (0.0, 0.0),
            initialized: false,
            tool: Tool::Select,
            pen: None,
            selected: std::collections::HashSet::new(),
            selected_anchor: None,
            cursor: (0.0, 0.0),
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
    metric_inputs: MetricInputs,
    preview_input: gpui::Entity<gpui_component::input::InputState>,
    preview_text: SharedString,
    _subscriptions: Vec<gpui::Subscription>,
}

/// The editor's Width / LSB / RSB / X / Y fields.
struct MetricInputs {
    width: gpui::Entity<gpui_component::input::InputState>,
    lsb: gpui::Entity<gpui_component::input::InputState>,
    rsb: gpui::Entity<gpui_component::input::InputState>,
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
        self.editor.selected_anchor = None;
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
        let anchors = entry.anchors.clone();
        let selected_anchor = self.editor.selected_anchor;
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

        let op_button = |id: &'static str, label_text: &'static str| {
            div()
                .id(id)
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(t::cell_border())
                .text_color(t::text_muted())
                .text_sm()
                .cursor_pointer()
                .child(label_text)
        };
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
                    )))
                    .child(div().w_2())
                    .child(op_button("op-harmonize", "Harmonize").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.apply_curve_op(CurveOp::Harmonize);
                            cx.notify();
                        },
                    )))
                    .child(op_button("op-balance", "Balance").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.apply_curve_op(CurveOp::Balance);
                            cx.notify();
                        },
                    )))
                    .child(op_button("op-optimize", "Optimize").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.apply_curve_op(CurveOp::Optimize(0.12));
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
                        // Anchors: diamonds (rotated squares drawn as
                        // two overlapping quads approximate; use a
                        // filled path).
                        for (ai, (_, ax, ay)) in anchors.iter().enumerate() {
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

    /// Text preview strip: the preview string set in the active
    /// master, glyphs positioned by their advances.
    fn preview_strip(&self) -> impl IntoElement + use<> {
        let Some(font) = self.font() else {
            return div().into_any_element();
        };
        let upm = font.units_per_em;
        let descent = font.descender;
        let line: Vec<(Arc<BezPath>, f64)> = self
            .preview_text
            .chars()
            .filter_map(|c| font.codepoint_map.get(&c))
            .map(|&i| (font.glyphs[i].path.clone(), font.glyphs[i].advance))
            .collect();
        div()
            .h(px(104.0))
            .flex()
            .items_center()
            .gap_3()
            .px_4()
            .bg(t::panel_bg())
            .border_t_1()
            .border_color(t::cell_border())
            .child(div().w(px(180.0)).child(gpui_component::input::Input::new(
                &self.preview_input,
            )))
            .child(
                div().flex_1().h_full().child(
                    canvas(
                        move |bounds, _, _| bounds,
                        move |_, bounds: Bounds<gpui::Pixels>, window, _| {
                            let h: f32 = bounds.size.height.into();
                            let scale = (h as f64 * 0.72) / upm;
                            let baseline = h as f64 * 0.82 + descent * scale;
                            let mut x_cursor = 0.0f64;
                            for (path, advance) in line.iter() {
                                let transform =
                                    Affine::translate((x_cursor * scale, baseline))
                                        * Affine::scale_non_uniform(scale, -scale);
                                if let Some(p) =
                                    build_fill_path(path, transform, bounds.origin)
                                {
                                    window.paint_path(p, t::glyph_fill());
                                }
                                x_cursor += advance;
                            }
                        },
                    )
                    .size_full(),
                ),
            )
            .into_any_element()
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
                    }
                    Err(e) => workspace.load_error = Some(e.into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn handle_key(&mut self, event: &gpui::KeyDownEvent, cx: &mut Context<Self>) -> bool {
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
            ("o", true) => {
                self.open_dialog(cx);
                true
            }
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

        if matches!(self.mode, Mode::Editor(_)) {
            self.refresh_metric_inputs(false, window, cx);
        }
        let content = match self.mode {
            Mode::Editor(index) if self.project.is_some() => div()
                .flex()
                .flex_col()
                .flex_1()
                .child(self.editor_view(index, cx).into_any_element())
                .child(self.metrics_bar())
                .into_any_element(),
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
                if this.handle_key(event, cx) {
                    cx.notify();
                }
            }))
            .child(self.header(cx))
            .child(content)
            .child(self.preview_strip())
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
                    let metric = |cx: &mut Context<Workspace>, window: &mut Window| {
                        cx.new(|cx| gpui_component::input::InputState::new(window, cx))
                    };
                    let width_input = metric(cx, window);
                    let lsb_input = metric(cx, window);
                    let rsb_input = metric(cx, window);
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
                    let preview_input = cx.new(|cx| {
                        gpui_component::input::InputState::new(window, cx)
                            .placeholder("Preview text")
                            .default_value("hamburgevons")
                    });
                    let sub_p = cx.subscribe_in(&preview_input, window, {
                        let preview_input = preview_input.clone();
                        move |this: &mut Workspace,
                              _,
                              ev: &gpui_component::input::InputEvent,
                              _window,
                              cx| {
                            if matches!(ev, gpui_component::input::InputEvent::Change) {
                                this.preview_text =
                                    preview_input.read(cx).value().to_string().into();
                                cx.notify();
                            }
                        }
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
                        metric_inputs: MetricInputs {
                            width: width_input,
                            lsb: lsb_input,
                            rsb: rsb_input,
                        },
                        preview_input,
                        preview_text: "hamburgevons".into(),
                        _subscriptions: vec![subscription, sub_w, sub_l, sub_r, sub_p],
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
