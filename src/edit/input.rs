// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! Pointer and keyboard handling for the canvas.
//!
//! This is where a gesture becomes an intent. The handlers decide what
//! the user meant, from the active tool and what is under the cursor,
//! then call a command or a core operation to carry it out. Geometry
//! belongs in runebender-core, not here.

use crate::Mode;
use crate::Workspace;
/// The nearest master-pair point to the pointer: distance, point id,
/// and its position in each master.
use crate::widgets;
use crate::workspace::Drag;
use crate::workspace::HIT_RADIUS_PX;
use crate::workspace::POINT_HIT_RADIUS_PX;
use crate::workspace::PenState;
use crate::workspace::Tool;
use crate::workspace::ZOOM_KEY_STEP;
use crate::workspace::ZOOM_MAX;
use crate::workspace::ZOOM_MIN;
use gpui::Context;
use gpui::Point;
use gpui::Window;
use kurbo::Affine;
use runebender_core::formats::lib_keys::read_hoi_intermediates;
type NearestPair = (f64, (usize, usize), (f64, f64), (f64, f64));

impl Workspace {
    pub(crate) fn editor_mouse_down(
        &mut self,
        pos: Point<gpui::Pixels>,
        shift: bool,
        alt: bool,
        click_count: usize,
    ) {
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
        if self.project.as_ref().is_some_and(|p| p.showing_instance()) {
            // An instance is a view, never an edit: dragging pans, and
            // the status bar says why nothing else responds.
            let local = self.editor.window_to_local(pos);
            self.editor.drag = Some(Drag::Pan {
                last: (local.x, local.y),
            });
            self.status_note =
                Some("Interpolated instance · move an axis onto a master to edit".into());
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
                    runebender_core::outline::segment_ops::nearest_segment_with_t(
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
                            runebender_core::outline::segment_ops::convert_line_to_curve(
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
        // Free-transform handles outrank everything: a corner grab
        // always transforms the selection (Glyphs 4's on-canvas
        // rotate and scale).
        // A point under the cursor still wins: the box corner usually
        // sits exactly on a selected extreme, and grabbing that node
        // must drag it, not scale the selection. Handles work from
        // the parts of the box no point occupies.
        let point_near = all_points
            .iter()
            .any(|(_, (x, y))| ((x - dx).powi(2) + (y - dy).powi(2)).sqrt() <= point_tolerance);
        if self.editor.tool == Tool::Select
            && !shift
            && !point_near
            && let Some(bbox) = self.selection_bbox(index)
        {
            let zoom = self.editor.zoom().max(1e-6);
            let grab = 7.0 / zoom;
            let ring = 22.0 / zoom;
            let (cx_, cy_) = (bbox.center().x, bbox.center().y);
            let corners = [
                ((bbox.x0, bbox.y0), (bbox.x1, bbox.y1)),
                ((bbox.x1, bbox.y0), (bbox.x0, bbox.y1)),
                ((bbox.x0, bbox.y1), (bbox.x1, bbox.y0)),
                ((bbox.x1, bbox.y1), (bbox.x0, bbox.y0)),
            ];
            let edges = [
                ((cx_, bbox.y0), (cx_, bbox.y1), false, true),
                ((cx_, bbox.y1), (cx_, bbox.y0), false, true),
                ((bbox.x0, cy_), (bbox.x1, cy_), true, false),
                ((bbox.x1, cy_), (bbox.x0, cy_), true, false),
            ];
            let dist = |p: (f64, f64)| ((p.0 - dx).powi(2) + (p.1 - dy).powi(2)).sqrt();
            let mut gesture: Option<((f64, f64), bool, bool, bool)> = None;
            for (corner, opposite) in corners {
                if dist(corner) <= grab {
                    gesture = Some((opposite, false, true, true));
                    break;
                }
            }
            if gesture.is_none() {
                for (mid, opposite, sx, sy) in edges {
                    if dist(mid) <= grab {
                        gesture = Some((opposite, false, sx, sy));
                        break;
                    }
                }
            }
            if gesture.is_none() {
                for (corner, _) in corners {
                    let d = dist(corner);
                    if d > grab && d <= ring {
                        gesture = Some(((cx_, cy_), true, false, false));
                        break;
                    }
                }
            }
            if let Some((anchor, rotate, scale_x, scale_y)) = gesture {
                self.push_undo_snapshot(index);
                self.editor.drag = Some(Drag::FreeTransform {
                    anchor,
                    start: (dx, dy),
                    rotate,
                    scale_x,
                    scale_y,
                    originals: all_points.iter().copied().collect(),
                });
                return;
            }
        }
        // HOI knobs (trajectory intermediate points) come first while
        // the trajectory view is up: each node's knob sits at its
        // intermediate point, or the linear middle.
        if self.editor.tool == Tool::Select
            && self.show_trajectories
            && let Some((lo, hi, curves)) = self.project.as_ref().and_then(|p| {
                let (lo, hi) = p.axis_end_masters()?;
                let name = p.active_font().glyphs[index].name.clone();
                let canon = p.masters[lo].font.get_glyph(name.as_ref())?;
                Some((
                    p.masters[lo].font.get_glyph(name.as_ref())?.clone(),
                    p.masters[hi].font.get_glyph(name.as_ref())?.clone(),
                    read_hoi_intermediates(canon),
                ))
            })
        {
            let grab = 7.0 / self.editor.zoom().max(1e-6);
            let mut best: Option<NearestPair> = None;
            for (ci, (ca, cb)) in lo.contours.iter().zip(hi.contours.iter()).enumerate() {
                for (pi, (pa, pb)) in ca.points.iter().zip(cb.points.iter()).enumerate() {
                    let a = (pa.x, pa.y);
                    let b = (pb.x, pb.y);
                    let q = curves
                        .get(&(ci, pi))
                        .copied()
                        .unwrap_or(((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0));
                    let dist = ((q.0 - dx).powi(2) + (q.1 - dy).powi(2)).sqrt();
                    if dist <= grab && best.is_none_or(|(d, ..)| dist < d) {
                        best = Some((dist, (ci, pi), a, b));
                    }
                }
            }
            if let Some((_, id, a, b)) = best {
                self.hoi_live = Some((id, (dx, dy)));
                self.editor.drag = Some(Drag::HoiKnob { id, a, b });
                return;
            }
        }
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
            .filter(|(dist, id)| {
                *dist <= point_tolerance && !self.editor.locked_points.contains(id)
            })
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
                    let originals: std::collections::HashMap<(usize, usize), (f64, f64)> =
                        all_points.iter().copied().collect();
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
                    let originals: std::collections::HashMap<(usize, usize), (f64, f64)> =
                        all_points.iter().copied().collect();
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
                let advance = self.font().map(|f| f.glyphs[index].advance).unwrap_or(0.0);
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
                        runebender_core::outline::segment_ops::nearest_segment_with_t(
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
                    let originals: std::collections::HashMap<(usize, usize), (f64, f64)> =
                        all_points.iter().copied().collect();
                    let anchor = self.selected_anchor_origin(index);
                    self.push_undo_snapshot(index);
                    self.editor.drag = Some(Drag::Points {
                        start: (dx, dy),
                        originals,
                        anchor,
                    });
                    return;
                }
                let component_hit = self.font().and_then(|f| {
                    let g = f.font.get_glyph(f.glyphs[index].name.as_ref())?;
                    runebender_core::outline::component_ops::component_at(
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
                            !runebender_core::document::composites::component_alignment_disabled(c)
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
                // Show All Masters: a node of another master under
                // the click switches to that master with the node
                // selected — the editable-overlay gesture.
                if self.show_all_masters {
                    let hit: Option<(usize, (usize, usize))> =
                        self.project.as_ref().and_then(|p| {
                            let name = &p.active_font().glyphs[index].name;
                            let mut best: Option<(f64, usize, (usize, usize))> = None;
                            for (m, master) in p.masters.iter().enumerate() {
                                if m == p.active {
                                    continue;
                                }
                                let Some(glyph) = master.glyphs.iter().find(|g| g.name == *name)
                                else {
                                    continue;
                                };
                                for point in glyph.points.iter() {
                                    let dist =
                                        ((point.x - dx).powi(2) + (point.y - dy).powi(2)).sqrt();
                                    if dist <= point_tolerance
                                        && best.is_none_or(|(d, ..)| dist < d)
                                    {
                                        best = Some((dist, m, (point.contour, point.index)));
                                    }
                                }
                            }
                            best.map(|(_, m, id)| (m, id))
                        });
                    if let Some((m, id)) = hit {
                        self.switch_master(m);
                        self.editor.selected.clear();
                        self.editor.selected.insert(id);
                        return;
                    }
                }
                // Guides underlie everything: a guide drag starts
                // only when no point, segment, or component claimed
                // the click.
                if let Some((local, gi)) = self.guide_hit(dx, dy, tolerance) {
                    self.editor.selected_component = None;
                    self.editor.selected.clear();
                    self.editor.selected_anchors.clear();
                    self.editor.drag = Some(Drag::Guide { local, index: gi });
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

    pub(crate) fn editor_mouse_drag(
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
            Some(Drag::Points {
                start,
                originals,
                anchor,
            }) => {
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
                        runebender_core::outline::point_ops::translate_points(
                            g, &selected, &originals, delta, alt,
                        )
                    })
                    .unwrap_or(false);
                for (ai, (ox, oy)) in anchor {
                    use runebender_core::outline::point_ops::snap_coord;
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
            Some(Drag::FreeTransform {
                anchor,
                start,
                rotate,
                scale_x,
                scale_y,
                originals,
            }) => {
                let (ax, ay) = *anchor;
                let affine = if *rotate {
                    let a0 = (start.1 - ay).atan2(start.0 - ax);
                    let a1 = (dy - ay).atan2(dx - ax);
                    let mut angle = a1 - a0;
                    if shift {
                        let step = 15f64.to_radians();
                        angle = (angle / step).round() * step;
                    }
                    Affine::translate((ax, ay))
                        * Affine::rotate(angle)
                        * Affine::translate((-ax, -ay))
                } else {
                    let denx = start.0 - ax;
                    let deny = start.1 - ay;
                    let mut sx = if *scale_x && denx.abs() > 1e-6 {
                        (dx - ax) / denx
                    } else {
                        1.0
                    };
                    let mut sy = if *scale_y && deny.abs() > 1e-6 {
                        (dy - ay) / deny
                    } else {
                        1.0
                    };
                    if shift && *scale_x && *scale_y {
                        // Proportional: the larger factor drives both.
                        let s = sx.abs().max(sy.abs());
                        sx = s * sx.signum();
                        sy = s * sy.signum();
                    }
                    Affine::translate((ax, ay))
                        * Affine::scale_non_uniform(sx, sy)
                        * Affine::translate((-ax, -ay))
                };
                let originals = originals.clone();
                let selected = self.editor.selected.clone();
                self.font_mut().is_some_and(|f| {
                    f.edit_glyph(index, |g| {
                        let mut moved = false;
                        for (c, contour) in g.contours.iter_mut().enumerate() {
                            for (pi, point) in contour.points.iter_mut().enumerate() {
                                if !selected.contains(&(c, pi)) {
                                    continue;
                                }
                                let Some(&(ox, oy)) = originals.get(&(c, pi)) else {
                                    continue;
                                };
                                let p = affine * kurbo::Point::new(ox, oy);
                                let (nx, ny) = (p.x.round(), p.y.round());
                                if point.x != nx || point.y != ny {
                                    point.x = nx;
                                    point.y = ny;
                                    moved = true;
                                }
                            }
                        }
                        moved
                    })
                    .unwrap_or(false)
                })
            }
            Some(Drag::HoiKnob { id, .. }) => {
                // Live only: the knob follows the cursor; commit and
                // bake happen on mouse-up.
                let id = *id;
                self.hoi_live = Some((id, (dx, dy)));
                true
            }
            Some(Drag::Guide { local, index: gi }) => {
                let (local, gi) = (*local, *gi);
                let move_line = |line: &mut norad::Line| match line {
                    norad::Line::Vertical(x) => {
                        let nx = dx.round();
                        let changed = *x != nx;
                        *x = nx;
                        changed
                    }
                    norad::Line::Horizontal(y) => {
                        let ny = dy.round();
                        let changed = *y != ny;
                        *y = ny;
                        changed
                    }
                    norad::Line::Angle { x, y, .. } => {
                        let (nx, ny) = (dx.round(), dy.round());
                        let changed = *x != nx || *y != ny;
                        *x = nx;
                        *y = ny;
                        changed
                    }
                };
                if local {
                    let name = self.font().map(|f| f.glyphs[index].name.to_string());
                    let Some(name) = name else { return false };
                    self.font_mut().is_some_and(|f| {
                        let moved = f
                            .font
                            .get_glyph_mut(name.as_str())
                            .and_then(|g| g.guidelines.get_mut(gi))
                            .map(|guide| move_line(&mut guide.line))
                            .unwrap_or(false);
                        if moved {
                            f.dirty = true;
                            f.modified_glyphs.insert(name);
                        }
                        moved
                    })
                } else {
                    self.font_mut().is_some_and(|f| {
                        let moved = f
                            .font
                            .font_info
                            .guidelines
                            .as_mut()
                            .and_then(|gs| gs.get_mut(gi))
                            .map(|guide| move_line(&mut guide.line))
                            .unwrap_or(false);
                        if moved {
                            f.dirty = true;
                        }
                        moved
                    })
                }
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
                if let Some(Drag::Sidebearing { applied, .. }) = &mut self.editor.drag {
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
                            runebender_core::outline::glyph_ops::shift_ink(g, -step);
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
            Some(Drag::Component {
                index: ci,
                start,
                orig,
            }) => {
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
            Some(Drag::Knife { start, current }) | Some(Drag::Measure { start, current }) => {
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
            Some(Drag::Marquee {
                start,
                current,
                base,
                base_anchors,
            }) => {
                *current = (dx, dy);
                let (sx, sy) = *start;
                let (base, base_anchors) = (base.clone(), base_anchors.clone());
                self.select_in_rect(index, (sx, sy), (dx, dy), &base, &base_anchors);
                true
            }
            None => false,
        }
    }

    pub(crate) fn editor_mouse_up(&mut self) {
        if let Some(Drag::HoiKnob { id, .. }) = self.editor.drag.as_ref() {
            let id = *id;
            self.editor.drag = None;
            if let Some((live_id, q)) = self.hoi_live.take()
                && live_id == id
            {
                self.commit_hoi_intermediate(id, q);
            }
            return;
        }
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
                .map(|g| runebender_core::outline::knife::knife_hit_points(g, p0, p1).len())
                .unwrap_or(0);
            if p0.distance(p1) >= 2.0 && crossings >= 2 {
                self.push_undo_snapshot(index);
                let changed = self
                    .font_mut()
                    .and_then(|f| {
                        f.edit_glyph(index, |g| {
                            runebender_core::outline::knife::knife_cut_glyph(g, p0, p1)
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
                    runebender_core::outline::point_ops::snap_selected_offcurves(g, &selected)
                });
            }
            self.editor.drag = None;
            return;
        }
        if let Some(Drag::Marquee {
            start,
            current,
            base,
            base_anchors,
        }) = self.editor.drag.take()
        {
            self.select_in_rect(index, start, current, &base, &base_anchors);
        }
        self.editor.drag = None;
    }

    /// Pen click: place a point (line segment from the previous one,
    /// curve if the previous point was dragged into a handle), start
    /// a contour if none is open, or close the contour when clicking
    /// its first point.
    pub(crate) fn pen_mouse_down(&mut self, index: usize, pos: Point<gpui::Pixels>, alt: bool) {
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
                    runebender_core::outline::segment_ops::nearest_segment_with_t(
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
                            runebender_core::outline::segment_ops::convert_line_to_curve(
                                g, &seg_hit,
                            )
                            .map(|ids| ids[0])
                        } else {
                            runebender_core::outline::segment_ops::insert_point_on_segment(
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
                if let Some(contour) = self.font_mut().and_then(|f| f.start_contour(index, x, y)) {
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
    pub(crate) fn pen_mouse_drag(&mut self, index: usize, pos: Point<gpui::Pixels>) -> bool {
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
    pub(crate) fn hyper_pen_mouse_down(
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

    /// A key while the editor's text tool is active. Typing composes
    /// text around the open glyph; the open glyph follows the active
    /// sort.
    pub(crate) fn handle_edit_text_key(&mut self, event: &gpui::KeyDownEvent) -> bool {
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
                            runebender_core::text::buffer::TextSortKind::Glyph {
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

    pub(crate) fn handle_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> bool {
        // Typing belongs to whichever field has the keyboard.
        if widgets::input::any_field_focused(window, _cx) {
            return false;
        }
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
                    let name = self.font().map(|f| f.glyphs[index].name.to_string());
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
                    && (self.editor.pen.is_some() || self.editor.hyper_contour.is_some()) =>
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
                    && (self.editor.pen.is_some() || self.editor.hyper_contour.is_some()) =>
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
                            runebender_core::outline::segment_ops::delete_last_pen_point(g, contour)
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
                            runebender_core::outline::component_ops::delete_component(g, ci)
                        })
                    })
                    .unwrap_or(false);
                if !changed {
                    self.editor.undo.pop();
                }
                changed
            }
            ("backspace" | "delete", false)
                if in_editor && !self.editor.selected_anchors.is_empty() =>
            {
                let Mode::Editor(index) = self.mode else {
                    return false;
                };
                let mut anchors = std::mem::take(&mut self.editor.selected_anchors);
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
