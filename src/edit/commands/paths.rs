// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! Paths menu: cleanup, booleans, transforms, curve operations, filters, and the clipboard.

use crate::Mode;
use crate::Workspace;
use crate::workspace::Tool;
use gpui::Context;
use runebender_core::outline::cleanup::add_extreme_points;
use runebender_core::outline::cleanup::correct_path_directions;
use runebender_core::outline::cleanup::fit_curve_handles;
use runebender_core::outline::cleanup::round_glyph_coordinates;
use runebender_core::outline::cleanup::tidy_contours;
use runebender_core::outline::convert::cubics_to_quads;
use runebender_core::outline::convert::quads_to_cubics;
use runebender_core::outline::effects::apply_corner_at;
use runebender_core::outline::effects::expand_stroke_contours;
use runebender_core::outline::effects::extrude_glyph_contours;
use runebender_core::outline::effects::offset_glyph_contours;
use runebender_core::outline::effects::roughen_glyph_contours;
use std::collections::HashSet;
impl Workspace {
    /// Convert the open glyph's curves between cubic and quadratic,
    /// in every master; structure must stay shared. Quads to cubics
    /// is exact. The other way approximates within upm/1000 units,
    /// the tolerance the TrueType compilers use.
    pub(crate) fn command_convert_curves(&mut self, to_cubic: bool) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        if let Mode::Editor(i) = self.mode {
            self.push_undo_snapshot(i);
        }
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let name = project.active_font().glyphs[index].name.to_string();
        let tolerance = (project.active_font().units_per_em / 1000.0).max(0.5);
        let mut converted = 0usize;
        for master in project.masters.iter_mut() {
            let Some(gi) = master.name_map.get(name.as_str()).copied() else {
                continue;
            };
            let ok = master
                .edit_glyph(gi, |g| {
                    if to_cubic {
                        quads_to_cubics(g)
                    } else {
                        cubics_to_quads(g, tolerance)
                    }
                })
                .unwrap_or(false);
            if ok {
                converted += 1;
            }
        }
        project.compute_compat();
        self.editor.selected.clear();
        self.status_note = Some(
            if converted == 0 {
                format!(
                    "Nothing to convert to {}",
                    if to_cubic { "cubic" } else { "quadratic" }
                )
            } else {
                format!(
                    "Converted to {} in {converted} master(s)",
                    if to_cubic { "cubic" } else { "quadratic" }
                )
            }
            .into(),
        );
    }

    /// Apply a corner glyph at the context-menu node, in every
    /// master (all masters must keep the same structure). The name
    /// accepts "chamfer" or "_corner.chamfer".
    pub(crate) fn command_apply_corner(&mut self, node: (usize, usize), name: &str) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let glyph_name = project.active_font().glyphs[index].name.to_string();
        let corner_name = if name.starts_with("_corner.") {
            name.to_string()
        } else {
            format!("_corner.{name}")
        };
        let mut applied = 0usize;
        for master in project.masters.iter_mut() {
            let Some(corner) = master.font.get_glyph(corner_name.as_str()).cloned() else {
                continue;
            };
            let Some(gi) = master.name_map.get(glyph_name.as_str()).copied() else {
                continue;
            };
            let ok = master
                .edit_glyph(gi, |g| apply_corner_at(g, &corner, node.0, node.1))
                .unwrap_or(false);
            if ok {
                applied += 1;
            }
        }
        if applied == 0 {
            self.status_note = Some(
                format!("No corner applied · needs a {corner_name} glyph and a line corner").into(),
            );
            return;
        }
        project.compute_compat();
        self.editor.selected.clear();
        self.status_note = Some(format!("{corner_name} applied in {applied} master(s)").into());
    }

    /// Path > Tidy up Paths on the current glyph (active master).
    pub(crate) fn command_tidy_paths(&mut self) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        self.push_undo_snapshot(index);
        let removed = self
            .font_mut()
            .and_then(|f| f.edit_glyph(index, tidy_contours))
            .unwrap_or(0);
        if removed == 0 {
            self.editor.undo.pop();
        }
        self.status_note = Some(format!("Tidy up Paths: {removed} point(s) removed").into());
    }

    /// Path > Correct Path Direction on the current glyph.
    pub(crate) fn command_correct_path_direction(&mut self) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        self.push_undo_snapshot(index);
        let flipped = self
            .font_mut()
            .and_then(|f| f.edit_glyph(index, correct_path_directions))
            .unwrap_or(0);
        if flipped == 0 {
            self.editor.undo.pop();
        }
        self.status_note =
            Some(format!("Correct Path Direction: {flipped} contour(s) reversed").into());
    }

    /// Path > Round Coordinates on the current glyph.
    pub(crate) fn command_round_coordinates(&mut self) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        self.push_undo_snapshot(index);
        let moved = self
            .font_mut()
            .and_then(|f| f.edit_glyph(index, round_glyph_coordinates))
            .unwrap_or(0);
        if moved == 0 {
            self.editor.undo.pop();
        }
        self.status_note = Some(format!("Round Coordinates: {moved} point(s) moved").into());
    }

    /// Edit > Select All / Deselect All / Invert Selection on the
    /// open glyph's points.
    pub(crate) fn command_select_points(&mut self, mode: u8) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        let all: Vec<(usize, usize)> = self
            .font()
            .and_then(|f| f.font.get_glyph(f.glyphs[index].name.as_ref()))
            .map(|g| {
                g.contours
                    .iter()
                    .enumerate()
                    .flat_map(|(ci, c)| (0..c.points.len()).map(move |pi| (ci, pi)))
                    .collect()
            })
            .unwrap_or_default();
        match mode {
            0 => {
                self.editor.selected = all
                    .into_iter()
                    .filter(|id| !self.editor.locked_points.contains(id))
                    .collect();
            }
            1 => self.editor.selected.clear(),
            _ => {
                let current = std::mem::take(&mut self.editor.selected);
                self.editor.selected = all
                    .into_iter()
                    .filter(|id| !current.contains(id) && !self.editor.locked_points.contains(id))
                    .collect();
            }
        }
    }

    /// Round the selected corners into fillets sized like the
    /// glyph's existing rounding.
    pub(crate) fn command_round_corners(&mut self) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        self.push_undo_snapshot(index);
        let selected = self.editor.selected.clone();
        let new_selection = self
            .font_mut()
            .and_then(|f| {
                f.edit_glyph(index, |g| {
                    runebender_core::outline::glyph_ops::round_selected_corners(g, &selected)
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

    /// Duplicate the selection: contours holding selected points,
    /// or the selected component or anchor, offset (20, 20), clones
    /// selected. This is `duplicateSelection` in the web editor.
    pub(crate) fn command_duplicate(&mut self) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        self.push_undo_snapshot(index);
        let changed = if let Some(ci) = self.editor.selected_component {
            let new_index = self
                .font_mut()
                .and_then(|f| {
                    f.edit_glyph(index, |g| {
                        runebender_core::outline::component_ops::duplicate_component(g, ci)
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
                        runebender_core::outline::glyph_ops::duplicate_anchor(g, ai)
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
                        runebender_core::outline::glyph_ops::duplicate_selection(g, &selected)
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

    /// Duplicate, then re-apply the last flip/rotate, for rotated
    /// repeats around a center. This is duplicate-repeat in the web
    /// editor.
    pub(crate) fn command_duplicate_repeat(&mut self) {
        let before = self.editor.undo.len();
        self.command_duplicate();
        if self.editor.undo.len() == before {
            return;
        }
        if let Some(transform) = self.editor.last_transform {
            let Mode::Editor(index) = self.mode else {
                return;
            };
            let selected = self.editor.selected.clone();
            self.font_mut().and_then(|f| {
                f.edit_glyph(index, |g| {
                    runebender_core::outline::glyph_ops::transform_selection(
                        g, &selected, transform,
                    )
                })
            });
        }
    }

    /// Copy the selected contours (whole glyph when nothing selected).
    pub(crate) fn command_copy(&mut self) {
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
            self.status_note = Some(format!("Copied {} contours", self.clipboard.len()).into());
        }
    }

    /// Paste copied contours into the current glyph, with undo.
    pub(crate) fn command_paste(&mut self) {
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

    /// Route Cmd+V the way the web editor does. If the outline
    /// clipboard holds contours and the Text tool is not in hand,
    /// they paste. Otherwise the system clipboard's text types into
    /// the editor's buffer.
    pub(crate) fn command_paste_routed(&mut self, cx: &mut Context<Self>) {
        let text_target = matches!(self.mode, Mode::Editor(_));
        if (!self.clipboard.is_empty() && self.editor.tool != Tool::Text) || !text_target {
            self.command_paste();
            return;
        }
        self.paste_text_into_buffer(cx);
    }

    /// Remove overlap on the open glyph, with undo.
    pub(crate) fn command_remove_overlap(&mut self) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        self.push_undo_snapshot(index);
        let changed = self.font_mut().is_some_and(|f| f.remove_overlap(index));
        if !changed {
            self.editor.undo.pop();
        } else {
            self.journal("remove overlap", Some(index), None);
            self.editor.selected.clear();
        }
    }

    /// Expand contours into stroked outlines. Each selected contour
    /// becomes the outline of a stroke of the typed width, with
    /// round joins and caps; when nothing is selected, every contour
    /// does. This is the Make Stroke half of Offset Curve in Glyphs.
    ///
    /// This is the monoline workflow: draw open skeleton paths, type
    /// a weight, get letterforms.
    pub(crate) fn command_expand_stroke(&mut self, width: f64) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        if width.is_nan() || width <= 0.0 {
            return;
        }
        self.push_undo_snapshot(index);
        let selected_contours: HashSet<usize> =
            self.editor.selected.iter().map(|(c, _)| *c).collect();
        let changed = self
            .font_mut()
            .and_then(|f| {
                f.edit_glyph(index, |g| {
                    expand_stroke_contours(g, &selected_contours, width)
                })
            })
            .unwrap_or(false);
        if !changed {
            self.editor.undo.pop();
        } else {
            self.editor.selected.clear();
        }
    }

    /// Offset the whole glyph bolder (positive) or lighter
    /// (negative) by the typed number of units.
    pub(crate) fn command_offset(&mut self, delta: f64) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        self.push_undo_snapshot(index);
        let changed = self
            .font_mut()
            .and_then(|f| f.edit_glyph(index, |g| offset_glyph_contours(g, delta)))
            .unwrap_or(false);
        if !changed {
            self.editor.undo.pop();
        } else {
            self.editor.selected.clear();
        }
    }

    /// Fit Curve: set selected segments' handles to a percentage of
    /// their tangent-intersection maximum.
    pub(crate) fn command_fit_curve(&mut self, fraction: f64) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        self.push_undo_snapshot(index);
        let selected = self.editor.selected.clone();
        let changed = self
            .font_mut()
            .and_then(|f| f.edit_glyph(index, |g| fit_curve_handles(g, &selected, fraction)))
            .unwrap_or(false);
        if !changed {
            self.editor.undo.pop();
        }
    }

    /// Extrude field: "offset" or "offset,angle" (angle default 30,
    /// the Glyphs default). Prefix with k to keep the front face
    /// ("k15,30" = Don't Subtract).
    pub(crate) fn command_extrude(&mut self, text: &str) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        let trimmed = text.trim();
        let keep_front = trimmed.starts_with(['k', 'K']);
        let trimmed = trimmed.trim_start_matches(['k', 'K']).trim();
        let mut parts = trimmed.split(',').map(str::trim);
        let Some(Ok(offset)) = parts.next().map(str::parse::<f64>) else {
            return;
        };
        let angle = parts
            .next()
            .and_then(|p| p.parse::<f64>().ok())
            .unwrap_or(30.0);
        self.push_undo_snapshot(index);
        let changed = self
            .font_mut()
            .and_then(|f| {
                f.edit_glyph(index, |g| {
                    extrude_glyph_contours(g, offset, angle, keep_front)
                })
            })
            .unwrap_or(false);
        if !changed {
            self.editor.undo.pop();
        } else {
            self.editor.selected.clear();
        }
    }

    /// Roughen field: "segment" or "segment,h,v" (h and v default to
    /// the segment length and half of it). New random rough each
    /// apply.
    pub(crate) fn command_roughen(&mut self, text: &str) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        let mut parts = text.trim().split(',').map(str::trim);
        let Some(Ok(seg)) = parts.next().map(str::parse::<f64>) else {
            return;
        };
        let h = parts
            .next()
            .and_then(|p| p.parse::<f64>().ok())
            .unwrap_or(seg);
        let v = parts
            .next()
            .and_then(|p| p.parse::<f64>().ok())
            .unwrap_or(seg / 2.0);
        self.push_undo_snapshot(index);
        self.roughen_seed = self.roughen_seed.wrapping_add(1);
        let seed = self.roughen_seed;
        let selected_contours: HashSet<usize> =
            self.editor.selected.iter().map(|(c, _)| *c).collect();
        let changed = self
            .font_mut()
            .and_then(|f| {
                f.edit_glyph(index, |g| {
                    roughen_glyph_contours(g, &selected_contours, seg, h, v, seed)
                })
            })
            .unwrap_or(false);
        if !changed {
            self.editor.undo.pop();
        } else {
            self.editor.selected.clear();
        }
    }

    /// Path > Add Extremes.
    pub(crate) fn command_add_extremes(&mut self) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        self.push_undo_snapshot(index);
        let selected = self.editor.selected.clone();
        let changed = self
            .font_mut()
            .and_then(|f| f.edit_glyph(index, |g| add_extreme_points(g, &selected)))
            .unwrap_or(false);
        if !changed {
            self.editor.undo.pop();
            self.status_note = Some("No missing extremes".into());
        } else {
            self.editor.selected.clear();
        }
    }

    /// Combine the glyph's contours with a boolean operation, under
    /// one undo step. Union merges everything; the other operations
    /// apply the first contour against the rest combined. A no-op
    /// pops the snapshot. The operations are the web editor's
    /// boolean tiles.
    pub(crate) fn command_boolean(&mut self, op: linesweeper::BinaryOp) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        self.push_undo_snapshot(index);
        let changed =
            self.font_mut()
                .and_then(|f| {
                    f.edit_glyph(index, |g| {
                        match runebender_core::outline::glyph_ops::boolean_contours(g, op) {
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
    pub(crate) fn command_set_start_point(&mut self) {
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
                    runebender_core::outline::glyph_ops::set_contour_start(g, contour, point)
                })
            })
            .unwrap_or(false);
        if !changed {
            self.editor.undo.pop();
        } else {
            self.editor.selected = [(contour, 0)].into();
        }
    }

    /// Tab / shift-Tab: step the point selection through the
    /// glyph's points in contour order. Bound as an action so gpui's
    /// default tab-stop traversal never runs. This is
    /// `cycle_selected_point` in the web editor.
    pub(crate) fn command_cycle_point(&mut self, back: bool) -> bool {
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
    pub(crate) fn command_reverse(&mut self) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        self.push_undo_snapshot(index);
        let selected = self.editor.selected.clone();
        let changed = self
            .font_mut()
            .and_then(|f| {
                f.edit_glyph(index, |g| {
                    runebender_core::outline::glyph_ops::reverse_contours(g, &selected)
                })
            })
            .unwrap_or(false);
        if !changed {
            self.editor.undo.pop();
        } else {
            self.editor.selected.clear();
        }
    }

    /// Decompose the open glyph's components, with undo.
    pub(crate) fn command_decompose(&mut self) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        self.push_undo_snapshot(index);
        let changed = self.font_mut().is_some_and(|f| f.decompose(index));
        if !changed {
            self.editor.undo.pop();
        } else {
            self.journal("decompose", Some(index), None);
        }
    }
}
