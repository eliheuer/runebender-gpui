// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! Selection and the edit operations that act on it.
//!
//! Hit testing for guides and anchors, rectangle select, finishing a
//! pen contour, moving and transforming the selection, and the
//! context menu that offers those operations.

use super::*;

impl Workspace {
    /// Commit a dragged intermediate point: store it in the glyph's
    /// HOI lib key (dragging back onto the linear middle clears it),
    /// then rebake the brace layers so every consumer follows.
    pub(crate) fn commit_hoi_intermediate(&mut self, id: (usize, usize), q: (f64, f64)) {
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
    pub(crate) fn bake_hoi(&mut self) {
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
                let mut location = runebender_core::document::var_model::Location::new();
                location.insert(
                    axis.name.clone(),
                    runebender_core::document::var_model::normalize_value(
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

    /// Right-click on the canvas: build the web-style context menu
    /// for whatever is under the cursor.
    pub(crate) fn editor_context_menu(&mut self, pos: Point<gpui::Pixels>) {
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
                runebender_core::outline::glyph_ops::component_at(
                    &font.font,
                    g,
                    kurbo::Point::new(dx, dy),
                )
                .map(|ci| {
                    let aligned =
                        !runebender_core::document::composites::component_alignment_disabled(
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
                    runebender_core::outline::segment_ops::nearest_segment_with_t(
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
    pub(crate) fn context_menu_action(&mut self, action: &'static str) {
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
                        if let (Some(name), Some(f)) = (name, self.font_mut())
                            && let Some(g) = f.font.get_glyph_mut(name.as_str())
                            && gi < g.guidelines.len()
                        {
                            g.guidelines.remove(gi);
                            f.dirty = true;
                            f.modified_glyphs.insert(name);
                        }
                    } else if let Some(f) = self.font_mut()
                        && let Some(gs) = f.font.font_info.guidelines.as_mut()
                        && gi < gs.len()
                    {
                        gs.remove(gi);
                        f.dirty = true;
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
                    if let (Some(name), Some(f)) = (name, self.font_mut())
                        && let Some(g) = f.font.get_glyph_mut(name.as_str())
                    {
                        g.guidelines.push(guide);
                        f.dirty = true;
                        f.modified_glyphs.insert(name);
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
                                runebender_core::outline::glyph_ops::decompose_single_component(
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
                            runebender_core::outline::segment_ops::nearest_segment_with_t(
                                g,
                                kurbo::Point::new(dx, dy),
                                24.0,
                            )
                            .and_then(|(hit, t)| {
                                runebender_core::outline::segment_ops::insert_point_on_segment(
                                    g, &hit, t,
                                )
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
                if let Some(node) = menu.start_point
                    && !self.editor.locked_points.remove(&node)
                {
                    self.editor.locked_points.insert(node);
                    self.editor.selected.remove(&node);
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
                                runebender_core::outline::glyph_ops::set_contour_start(g, ci, pi)
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
                                runebender_core::outline::glyph_ops::reverse_contours(g, &target)
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
                                runebender_core::outline::glyph_ops::move_contour(g, ci, up)
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
    pub(crate) fn commit_add_component(&mut self, base: &str) {
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
                    runebender_core::outline::glyph_ops::add_component(&font_clone, g, &base)
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
    pub(crate) fn selection_bbox(&self, index: usize) -> Option<kurbo::Rect> {
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
    pub(crate) fn guide_distance(line: &norad::Line, dx: f64, dy: f64) -> f64 {
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
    pub(crate) fn guide_hit(&self, dx: f64, dy: f64, tolerance: f64) -> Option<(bool, usize)> {
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
    pub(crate) fn selected_anchor_origin(&self, index: usize) -> Vec<(usize, (f64, f64))> {
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
    pub(crate) fn editor_hover(&mut self, pos: Point<gpui::Pixels>, alt: bool) -> bool {
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
                    runebender_core::outline::segment_ops::nearest_segment_with_t(
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
    pub(crate) fn select_in_rect(
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
    pub(crate) fn hyper_pen_finish(&mut self) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        if let Some(contour) = self.editor.hyper_contour.take()
            && let Some(font) = self.font_mut()
        {
            font.remove_contour_if_degenerate(index, contour);
        }
    }

    pub(crate) fn pen_finish(&mut self) {
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

    /// The single selected point, if exactly one point is selected.
    pub(crate) fn single_selected_point(&self) -> Option<GlyphPoint> {
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

    /// Bounds of whatever is selected: points, else the component,
    /// else the anchor.
    /// The true bounding box of the selected segments: the curve's own
    /// extrema, not the box around its control points. A cubic's
    /// handles usually sit outside the ink, so the point box overstates
    /// how tall or wide a curve actually is — this is the number you
    /// want when matching one curve's size against another.
    ///
    /// `None` unless the selection covers whole segments.
    pub(crate) fn selected_segment_bounds(&self) -> Option<(usize, kurbo::Rect)> {
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
        for hit in runebender_core::outline::segment_ops::segments(glyph) {
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

    pub(crate) fn selection_bounds(&self) -> Option<kurbo::Rect> {
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
            let transform =
                runebender_core::outline::glyph_paths::component_affine(&component.transform);
            let path = transform
                * &runebender_core::outline::glyph_paths::glyph_to_bezpath(base, &font.font);
            return Some(path.bounding_box());
        }
        if let Some(ai) = self.editor.selected_anchor() {
            let (_, x, y) = entry.anchors.get(ai)?;
            return Some(kurbo::Rect::new(*x, *y, *x, *y));
        }
        None
    }

    /// Translate the active selection (points, component, or anchor).
    pub(crate) fn translate_selected(&mut self, index: usize, delta: kurbo::Vec2) -> bool {
        if let Some(ci) = self.editor.selected_component {
            return self
                .font_mut()
                .and_then(|f| {
                    f.edit_glyph(index, |g| {
                        runebender_core::outline::glyph_ops::translate_component(
                            g, ci, delta.x, delta.y,
                        )
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
            if let Some((x, y)) = target
                && let Some(font) = self.font_mut()
            {
                font.set_anchor(index, ai, x.round(), y.round());
                return true;
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
                    runebender_core::outline::glyph_ops::transform_selection(
                        g,
                        &selected,
                        Affine::translate(delta),
                    )
                })
            })
            .unwrap_or(false)
    }

    /// Transform the active selection (points, component, or anchor).
    pub(crate) fn transform_selected(&mut self, index: usize, transform: Affine) -> bool {
        if let Some(ci) = self.editor.selected_component {
            // Bake the scale into the component transform.
            return self
                .font_mut()
                .and_then(|f| {
                    f.edit_glyph(index, |g| {
                        let Some(component) = g.components.get_mut(ci) else {
                            return false;
                        };
                        let current = runebender_core::outline::glyph_paths::component_affine(
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
            if let Some((x, y)) = target
                && let Some(font) = self.font_mut()
            {
                font.set_anchor(index, ai, x.round(), y.round());
                return true;
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
                    runebender_core::outline::glyph_ops::transform_selection(
                        g, &selected, transform,
                    )
                })
            })
            .unwrap_or(false)
    }

    /// Lock the selected component back onto its anchor, or cut it
    /// loose. Unlocking leaves it exactly where it sits; locking
    /// snaps it home (the realign hook runs on the edit).
    pub(crate) fn toggle_component_alignment(&mut self, index: usize, ci: usize) {
        let currently_aligned = self
            .font()
            .and_then(|f| f.font.get_glyph(f.glyphs[index].name.as_ref()))
            .and_then(|g| g.components.get(ci))
            .map(|c| !runebender_core::document::composites::component_alignment_disabled(c));
        let Some(aligned) = currently_aligned else {
            return;
        };
        self.push_undo_snapshot(index);
        self.font_mut().and_then(|f| {
            f.edit_glyph(index, |g| {
                if let Some(component) = g.components.get_mut(ci) {
                    runebender_core::document::composites::set_component_alignment_disabled(
                        component, aligned,
                    );
                }
            })
        });
    }

    pub(crate) fn apply_place_image(
        &mut self,
        index: usize,
        path: &std::path::Path,
        bytes: Vec<u8>,
    ) {
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

    pub(crate) fn glyph_smart_axis_ref(&self) -> gpui::Entity<widgets::input::InputState> {
        self.smart_axis_input.clone()
    }

    /// The decoded background image for a file in the UFO images
    /// store, cached. gpui's RenderImage wants premultiplied BGRA.
    pub(crate) fn glyph_image(&self, file_name: &str) -> Option<Arc<gpui::RenderImage>> {
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
                for px in bytes.as_chunks_mut::<4>().0 {
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

    pub(crate) fn apply_image_trace(&mut self, index: usize, bytes: &[u8]) {
        let Some(font) = self.font() else { return };
        let (ascender, descender) = (font.ascender, font.descender);
        let advance = font
            .glyphs
            .get(index)
            .map(|g| g.advance)
            .unwrap_or(runebender_core::document::new_font::DEFAULT_WIDTH);
        let config = runebender_core::formats::image_trace::TraceConfig {
            target_height: (ascender - descender).max(1.0),
            y_offset: descender,
            advance: advance.max(1.0),
            ..Default::default()
        };
        match runebender_core::formats::image_trace::trace_image(bytes, &config) {
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
    pub(crate) fn apply_transform(&mut self, transform: Affine) {
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
                    runebender_core::outline::glyph_ops::transform_selection(
                        g, &selected, transform,
                    )
                })
            })
            .unwrap_or(false);
        if !changed {
            self.editor.undo.pop();
        }
    }

    pub(crate) fn apply_curve_op(&mut self, op: CurveOp) {
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
    pub(crate) fn toggle_measure(
        &mut self,
        change: impl FnOnce(&mut MeasureOpts),
        cx: &mut Context<Self>,
    ) {
        change(&mut self.measure_opts);
        *MEASURE_MENU.lock().expect("measure menu") = self.measure_opts;
        cx.set_menus(app_menus());
        cx.notify();
    }

    /// Nudge the selected points by (dx, dy) design units.
    /// Arrow-key nudge, with the web's routing: a selected component
    /// moves alone; with no points selected an anchor moves; otherwise
    /// points move, carrying any selected anchors with them. Alt makes
    /// the move independent — selected points travel without their
    /// handles.
    pub(crate) fn nudge_selection(&mut self, dx: f64, dy: f64, independent: bool) -> bool {
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
                        runebender_core::outline::glyph_ops::translate_component(g, ci, dx, dy)
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
                    runebender_core::outline::point_ops::translate_points(
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

    pub(crate) fn editor_scroll(&mut self, event: &gpui::ScrollWheelEvent) {
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
}
