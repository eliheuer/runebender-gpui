// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! What the menus and shortcuts call.
//!
//! Every method here is the whole of one user-facing command: the menu
//! item, the keyboard shortcut and the context menu all land on the
//! same function. They are the layer between an intent ("remove the
//! overlap") and the operation in runebender-core that performs it.

use super::*;

impl Workspace {
    /// The tab strip's "+": a fresh session on the current glyph.
    pub(crate) fn command_new_session(&mut self) {
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

    /// File → New Font: an Untitled GF-template UFO, in memory until
    /// Save As picks a destination.
    pub(crate) fn command_new_font(&mut self) {
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
        self.status_note = Some("New font · Save As… picks where it lives on disk".into());
    }

    /// Save As: pick a directory; the active master saves there under
    /// its family-style name and keeps saving there from now on.
    pub(crate) fn command_save_as(&mut self, cx: &mut Context<Self>) {
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
                        master.source_path = dir.join(format!("{family}-{style}.ufo"));
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

    /// Copy the selection as text (the glyphs' characters), the web
    /// sidebar footer's action.
    pub(crate) fn command_copy_selection_text(&mut self, cx: &mut Context<Self>) {
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

    /// Add every glyph a target-bearing language filter still misses
    /// (web generateMissing): empty glyphs named and encoded from the
    /// filter's targets, in every master.
    pub(crate) fn command_generate_missing(&mut self, group: usize, filter_index: usize) {
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
        let Some(project) = self.project.as_mut() else {
            return;
        };
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

    /// Copy the open glyph's outline into the UFO background layer
    /// (public.background), creating the layer on first use.
    pub(crate) fn command_send_to_background(&mut self) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
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
    pub(crate) fn command_swap_background(&mut self) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        self.push_undo_snapshot(index);
        let mut swapped = false;
        if let Some(font) = self.font_mut() {
            let background = Self::background_layer_name(&font.font);
            if let Some(background) = background {
                let fg = font
                    .font
                    .get_glyph(name.as_str())
                    .map(|g| g.contours.clone());
                let bg = font
                    .font
                    .layers
                    .get(&background)
                    .and_then(|l| l.get_glyph(name.as_str()))
                    .map(|g| g.contours.clone());
                if let (Some(fg), Some(bg)) = (fg, bg) {
                    if let Some(layer) = font.font.layers.get_mut(&background)
                        && let Some(g) = layer.get_glyph_mut(name.as_str()) {
                            g.contours = fg;
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
    pub(crate) fn command_clear_background(&mut self) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
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

    /// Copy the current drawing into a fresh backup layer
    /// (backup-1, backup-2, …): the Glyphs copy-layer gesture.
    pub(crate) fn command_backup_layer(&mut self) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        let mut created = None;
        if let Some(font) = self.font_mut() {
            let Some(source) = font.font.get_glyph(name.as_str()).cloned() else {
                return;
            };
            // Date-named like Glyphs' backup layers; a counter
            // steps in when the same minute already has one.
            let stamp = chrono::Local::now().format("%b %-d, %H.%M").to_string();
            let mut n = 0usize;
            let layer_name = loop {
                let candidate = if n == 0 {
                    stamp.clone()
                } else {
                    format!("{stamp} ({n})")
                };
                let taken = font
                    .font
                    .layers
                    .get(&candidate)
                    .is_some_and(|l| l.contains_glyph(name.as_str()));
                if !taken {
                    break candidate;
                }
                n += 1;
            };
            if let Ok(layer) = font.font.layers.get_or_create_layer(&layer_name) {
                let mut copy = norad::Glyph::new(name.as_str());
                copy.width = source.width;
                copy.contours = source.contours.clone();
                copy.components = source.components.clone();
                copy.anchors = source.anchors.clone();
                layer.insert_glyph(copy);
                font.dirty = true;
                created = Some(layer_name);
            }
        }
        if let Some(layer) = created {
            self.visible_glyph_layers.insert(layer.clone());
            self.status_note = Some(format!("Copied to {layer}").into());
        }
    }

    /// "+ Intermediate" in the layers block: freeze the current
    /// interpolation of the open glyph into a brace layer at the
    /// preview location — a named UFO layer plus a sparse designspace
    /// source, Glyphs' intermediate layer. Edit it via the swap
    /// arrows; the interpolation ghost and strip pick it up live.
    pub(crate) fn command_brace_layer(&mut self) {
        self.command_brace_layer_with(None);
    }

    /// The brace-layer write path, optionally freezing a supplied
    /// glyph instead of the plain interpolation (interpolation
    /// timing bakes eased positions through here).
    pub(crate) fn command_brace_layer_with(&mut self, frozen_override: Option<norad::Glyph>) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(project) = self.project.as_ref() else {
            return;
        };
        if project.ds_doc.is_none() {
            self.status_note = Some("Intermediate layers need a designspace project".into());
            return;
        }
        if project.master_at_location().is_some() {
            self.status_note = Some(
                "Move the axes off a master first: the intermediate freezes that location".into(),
            );
            return;
        }
        let name = project.active_font().glyphs[index].name.to_string();
        // Design coordinates and the "{500}" layer name.
        let coords: Vec<(String, f64)> = project
            .axes
            .iter()
            .map(|axis| {
                let normalized = project.location.get(&axis.name).copied().unwrap_or(0.0);
                (
                    axis.name.clone(),
                    runebender_core::var_model::denormalize_value(
                        normalized,
                        axis.min,
                        axis.default,
                        axis.max,
                    )
                    .round(),
                )
            })
            .collect();
        let layer_name = format!(
            "{{{}}}",
            coords
                .iter()
                .map(|(_, v)| format!("{v:.0}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let normalized_location = project.location.clone();
        let active = project.active;
        let frozen = frozen_override
            .or_else(|| project.interpolated_norad_glyph(&name))
            .or_else(|| project.active_font().font.get_glyph(&name).cloned());
        let Some(frozen) = frozen else { return };
        let filename = project
            .active_font()
            .source_path
            .file_name()
            .map(|f| f.to_string_lossy().to_string());
        let Some(filename) = filename else { return };
        if let Some(font) = self.font_mut()
            && let Ok(layer) = font.font.layers.get_or_create_layer(&layer_name) {
                let mut copy = norad::Glyph::new(name.as_str());
                copy.width = frozen.width;
                copy.contours = frozen.contours.clone();
                copy.components = frozen.components.clone();
                copy.anchors = frozen.anchors.clone();
                layer.insert_glyph(copy);
                font.dirty = true;
                font.modified_glyphs.insert(name.clone());
            }
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let Some(doc) = project.ds_doc.as_mut() else {
            return;
        };
        let already = doc.sources.iter().any(|src| {
            src.layer.as_deref() == Some(layer_name.as_str()) && src.filename == filename
        });
        if !already {
            doc.sources.push(norad::designspace::Source {
                name: Some(format!("brace {layer_name}")),
                filename: filename.clone(),
                layer: Some(layer_name.clone()),
                location: coords
                    .iter()
                    .map(|(axis, value)| norad::designspace::Dimension {
                        name: axis.clone(),
                        xvalue: Some(*value as f32),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            });
            project.brace.push(BraceSource {
                master: active,
                layer: layer_name.clone(),
                location: normalized_location,
            });
            project.ds_dirty = true;
        }
        self.visible_glyph_layers.insert(layer_name.clone());
        self.status_note = Some(format!("Intermediate {layer_name} added for {name}").into());
    }

    /// Glyph > Check Joining: for every positional form, measure
    /// the connecting stroke's band at its joining edges (init and
    /// medi join at x = 0, medi and fina at x = advance), find the
    /// font's common band, and select every form that misses it —
    /// the Arabic joining-line rule, measured instead of eyeballed.
    pub(crate) fn command_check_joining(&mut self) {
        let Some(font) = self.font() else { return };
        let tolerance_edge = 2.0;
        let tolerance_band = 4.0;
        // (glyph index, name, band) per joining edge, plus the forms
        // that should join but never touch their edge.
        let mut bands: Vec<(usize, f64, f64)> = Vec::new();
        let mut broken: Vec<String> = Vec::new();
        for (i, entry) in font.glyphs.iter().enumerate() {
            let name = entry.name.as_ref();
            let (joins_left, joins_right) = if name.ends_with(".init") {
                (true, false)
            } else if name.ends_with(".medi") {
                (true, true)
            } else if name.ends_with(".fina") {
                (false, true)
            } else {
                continue;
            };
            let Some(glyph) = font.font.get_glyph(name) else {
                continue;
            };
            let outline = runebender_core::glyph_paths::glyph_to_bezpath(glyph, &font.font);
            for left in [true, false] {
                let should = if left { joins_left } else { joins_right };
                if !should {
                    continue;
                }
                match joining_band(&outline, entry.advance, left, tolerance_edge) {
                    Some((lo, hi)) => bands.push((i, lo, hi)),
                    None => broken.push(name.to_string()),
                }
            }
        }
        if bands.is_empty() && broken.is_empty() {
            self.status_note = Some("Joining: no positional forms to check".into());
            return;
        }
        // The common band: median of the lows and highs.
        let median = |mut values: Vec<f64>| {
            values.sort_by(|a, b| a.total_cmp(b));
            values[values.len() / 2]
        };
        let med_lo = median(bands.iter().map(|(_, lo, _)| *lo).collect());
        let med_hi = median(bands.iter().map(|(_, _, hi)| *hi).collect());
        let mut off: Vec<String> = bands
            .iter()
            .filter(|(_, lo, hi)| {
                (lo - med_lo).abs() > tolerance_band || (hi - med_hi).abs() > tolerance_band
            })
            .map(|(i, _, _)| font.glyphs[*i].name.to_string())
            .collect();
        off.extend(broken.iter().cloned());
        off.sort();
        off.dedup();
        let count = off.len();
        self.multi_selected = off.into_iter().collect();
        self.selected = None;
        self.status_note = Some(
            if count == 0 {
                format!("Joining: all forms share the {med_lo:.0}–{med_hi:.0} band")
            } else {
                format!("Joining: {count} form(s) off the {med_lo:.0}–{med_hi:.0} band (selected)")
            }
            .into(),
        );
    }

    /// Glyph menu: convert the open glyph's curves between cubic
    /// and quadratic, in every master (structure must stay shared).
    /// Quads to cubics is exact; the other way approximates within
    /// upm/1000 units, the tolerance the TrueType compilers use.
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

    /// Glyph > Reinterpolate: rebuild the current glyph's outline
    /// in the active master from the other masters, evaluated at
    /// this master's location.
    pub(crate) fn command_reinterpolate(&mut self) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        let rebuilt = match self
            .project
            .as_ref()
            .map(|p| p.reinterpolated_from_others(&name))
        {
            Some(Ok(glyph)) => glyph,
            Some(Err(why)) => {
                self.status_note = Some(why.into());
                return;
            }
            None => return,
        };
        self.push_undo_snapshot(index);
        if let Some(font) = self.font_mut() {
            if let Some(glyph) = font.font.get_glyph_mut(name.as_str()) {
                glyph.contours = rebuilt.contours;
                glyph.width = rebuilt.width;
                font.dirty = true;
                font.modified_glyphs.insert(name.clone());
            }
            font.refresh_from_font();
        }
        self.status_note = Some(format!("{name}: reinterpolated from the other masters").into());
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

    /// Glyph > Duplicate Glyph: copy the current glyph (outline,
    /// components, anchors, width) to the next free name.NNN in
    /// every master, unencoded.
    pub(crate) fn command_duplicate_glyph(&mut self) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(base) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let taken: std::collections::HashSet<String> = project
            .masters
            .iter()
            .flat_map(|m| m.name_map.keys().cloned())
            .collect();
        let stem = base.split('.').next().unwrap_or(&base).to_string();
        let mut counter = 1;
        let mut name = format!("{stem}.{counter:03}");
        while taken.contains(&name) {
            counter += 1;
            name = format!("{stem}.{counter:03}");
        }
        for master in project.masters.iter_mut() {
            let Some(src) = master.font.get_glyph(base.as_str()).cloned() else {
                continue;
            };
            master.add_glyph(&name, src.width);
            if let Some(copy) = master.font.get_glyph_mut(name.as_str()) {
                copy.contours = src.contours.clone();
                copy.components = src.components.clone();
                copy.anchors = src.anchors.clone();
                copy.width = src.width;
                copy.lib = src.lib.clone();
            }
            master.dirty = true;
            master.modified_glyphs.insert(name.clone());
            master.refresh_from_font();
        }
        project.recheck_compat(&name);
        self.selected = self.font().and_then(|f| f.name_map.get(&name).copied());
        self.sidebar_counts = None;
        self.status_note = Some(format!("Duplicated {base} as {name}").into());
    }

    /// Glyph > Export Glyph as SVG: writes `<name>.svg` beside the
    /// project source (or the home directory before Save As).
    pub(crate) fn command_export_glyph_svg(&mut self) {
        #[cfg(target_family = "wasm")]
        {
            self.status_note = Some("SVG export: desktop only".into());
        }
        #[cfg(not(target_family = "wasm"))]
        {
            let Some(index) = self.current_glyph_index() else {
                return;
            };
            let Some(font) = self.font() else { return };
            let name = font.glyphs[index].name.to_string();
            let Some(glyph) = font.font.get_glyph(name.as_str()) else {
                return;
            };
            let path = runebender_core::glyph_paths::glyph_to_bezpath(glyph, &font.font);
            let ascender = font.font.font_info.ascender.unwrap_or(800.0);
            let descender = font.font.font_info.descender.unwrap_or(-200.0);
            let svg = glyph_svg(&path, glyph.width, ascender, descender);
            let dir = self
                .project
                .as_ref()
                .and_then(|p| p.export_source.as_ref())
                .and_then(|p| p.parent().map(PathBuf::from))
                .unwrap_or_else(|| {
                    std::env::var("HOME")
                        .map(PathBuf::from)
                        .unwrap_or_else(|_| PathBuf::from("."))
                });
            let file = dir.join(format!("{name}.svg"));
            self.status_note = Some(match std::fs::write(&file, svg) {
                Ok(()) => format!("Wrote {}", file.display()).into(),
                Err(e) => format!("SVG export failed: {e}").into(),
            });
        }
    }

    /// Glyph > Sync Metrics: apply every metrics key in every
    /// master. Chained keys (=n where n itself has a key) settle by
    /// repeating passes until nothing moves.
    pub(crate) fn command_sync_metrics(&mut self) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let mut adjusted = 0usize;
        for _pass in 0..5 {
            let mut moved = false;
            for master in project.masters.iter_mut() {
                let keyed: Vec<(usize, Option<String>, Option<String>)> = (0..master.glyphs.len())
                    .filter_map(|i| {
                        let glyph = master.font.get_glyph(master.glyphs[i].name.as_ref())?;
                        let l = read_metrics_key(glyph, true);
                        let r = read_metrics_key(glyph, false);
                        (l.is_some() || r.is_some()).then_some((i, l, r))
                    })
                    .collect();
                for (index, left, right) in keyed {
                    // Targets from the same master's referenced glyphs.
                    let resolve = |master: &Master,
                                   formula: &MetricsFormula,
                                   want_left: bool|
                     -> Option<f64> {
                        match formula {
                            MetricsFormula::Constant(v) => Some(*v),
                            MetricsFormula::Reference { glyph, mirror, op } => {
                                let ref_index = master.name_map.get(glyph.as_str()).copied()?;
                                let ink = master.ink_bounds(ref_index)?;
                                let advance = master.glyphs[ref_index].advance;
                                let read_left = want_left != *mirror;
                                let mut value = if read_left { ink.x0 } else { advance - ink.x1 };
                                if let Some((sign, amount)) = op {
                                    value = match sign {
                                        '+' => value + amount,
                                        '-' => value - amount,
                                        _ => value * amount,
                                    };
                                }
                                Some(value)
                            }
                        }
                    };
                    if let Some(formula) = left.as_deref().and_then(parse_metrics_key)
                        && let (Some(target), Some(ink)) =
                            (resolve(master, &formula, true), master.ink_bounds(index))
                        {
                            let delta = (target - ink.x0).round();
                            if delta != 0.0 {
                                master.shift_ink(index, delta);
                                moved = true;
                                adjusted += 1;
                            }
                        }
                    if let Some(formula) = right.as_deref().and_then(parse_metrics_key)
                        && let (Some(target), Some(ink)) =
                            (resolve(master, &formula, false), master.ink_bounds(index))
                        {
                            let advance = master.glyphs[index].advance;
                            let want = (ink.x1 + target).round();
                            if (advance - want).abs() >= 1.0 {
                                master.set_advance(index, want);
                                moved = true;
                                adjusted += 1;
                            }
                        }
                }
            }
            if !moved {
                break;
            }
        }
        self.rebuild_text_models();
        self.status_note = Some(
            if adjusted == 0 {
                "Metrics keys: everything in sync".to_string()
            } else {
                format!("Metrics keys: {adjusted} sidebearings adjusted")
            }
            .into(),
        );
    }

    /// Interpolation timing: bake an ease into a brace layer at the
    /// preview location. Positive ease means the change comes late
    /// (the light shape holds on), negative means early. Selected
    /// points ease; the rest stay on the straight interpolation, so
    /// the layer stays point-compatible. Standard designspace out —
    /// every compiler understands the result.
    pub(crate) fn command_ease_interpolation(&mut self, ease: f64) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let Some(axis) = project.axes.first().cloned() else {
            return;
        };
        if project.master_at_location().is_some() {
            self.status_note =
                Some("Move the axes off a master first: the ease bakes at that location".into());
            return;
        }
        let name = project.active_font().glyphs[index].name.to_string();
        // Where the preview sits along the axis, 0..1.
        let normalized = project.location.get(&axis.name).copied().unwrap_or(0.0);
        let design = runebender_core::var_model::denormalize_value(
            normalized,
            axis.min,
            axis.default,
            axis.max,
        );
        let t01 = ((design - axis.min) / (axis.max - axis.min)).clamp(0.0, 1.0);
        let gamma = (ease / 50.0).exp();
        let eased_t01 = t01.powf(gamma);
        let eased_design = axis.min + (axis.max - axis.min) * eased_t01;
        let mut eased_location = project.location.clone();
        eased_location.insert(
            axis.name.clone(),
            runebender_core::var_model::normalize_value(
                eased_design,
                axis.min,
                axis.default,
                axis.max,
            ),
        );
        let here = project.interpolated_norad_glyph(&name);
        let eased = project.interpolated_at(&name, &eased_location);
        let (Some(mut merged), Some(eased)) = (here, eased) else {
            self.status_note = Some("Ease needs compatible masters".into());
            return;
        };
        // Merge: selected points take the eased position.
        let selected = self.editor.selected.clone();
        let all = selected.is_empty();
        for (ci, contour) in merged.contours.iter_mut().enumerate() {
            for (pi, point) in contour.points.iter_mut().enumerate() {
                if !all && !selected.contains(&(ci, pi)) {
                    continue;
                }
                let Some(src) = eased.contours.get(ci).and_then(|c| c.points.get(pi)) else {
                    continue;
                };
                point.x = src.x;
                point.y = src.y;
            }
        }
        self.command_brace_layer_with(Some(merged));
        self.status_note = Some(
            format!(
                "Ease {ease:+.0} baked at {} {design:.0} (t {:.2} → {:.2})",
                axis.tag, t01, eased_t01
            )
            .into(),
        );
    }

    /// Add a shape switch (bracket layer): an unencoded `.bold`
    /// alternate copied into every master, plus a designspace rule
    /// substituting it from `at` up to the end of the first axis.
    /// The repo convention (DESIGN.md): design the alternate in the
    /// Regular master; the copies start red.
    pub(crate) fn command_add_shape_switch(&mut self, at: f64) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(project) = self.project.as_mut() else {
            return;
        };
        if project.ds_doc.is_none() {
            self.status_note = Some("Shape switches need a designspace project".into());
            return;
        }
        let Some(axis) = project.axes.first().cloned() else {
            return;
        };
        if !(axis.min..=axis.max).contains(&at) {
            self.status_note = Some(
                format!(
                    "Switch point outside {} {}–{}",
                    axis.tag, axis.min, axis.max
                )
                .into(),
            );
            return;
        }
        let name = project.active_font().glyphs[index].name.to_string();
        let alt = format!("{name}.bold");
        let (Ok(sub_name), Ok(sub_with)) = (norad::Name::new(&name), norad::Name::new(&alt)) else {
            return;
        };
        for master in project.masters.iter_mut() {
            if master.name_map.contains_key(&alt) {
                continue;
            }
            let Some(source) = master.font.get_glyph(name.as_str()).cloned() else {
                continue;
            };
            let mut copy = norad::Glyph::new(alt.as_str());
            copy.width = source.width;
            copy.contours = source.contours.clone();
            copy.components = source.components.clone();
            copy.anchors = source.anchors.clone();
            // Unencoded, and red: a placeholder awaiting its design
            // (the repo's lane-2 convention).
            runebender_core::theme_oklch::set_glyph_mark(&mut copy, Some("red"));
            master.font.default_layer_mut().insert_glyph(copy);
            master.dirty = true;
            master.modified_glyphs.insert(alt.clone());
            master.refresh_from_font();
        }
        let doc = project.ds_doc.as_mut().expect("checked above");
        doc.rules.processing = norad::designspace::RuleProcessing::Last;
        let exists = doc.rules.rules.iter().any(|rule| {
            rule.substitutions
                .iter()
                .any(|sub| sub.name.as_str() == name)
        });
        if !exists {
            doc.rules.rules.push(norad::designspace::Rule {
                name: Some(format!("{name} bold")),
                condition_sets: vec![norad::designspace::ConditionSet {
                    conditions: vec![norad::designspace::Condition {
                        name: axis.name.clone(),
                        minimum: Some(at as f32),
                        maximum: Some(axis.max as f32),
                    }],
                }],
                substitutions: vec![norad::designspace::Substitution {
                    name: sub_name,
                    with: sub_with,
                }],
            });
        }
        project.ds_dirty = true;
        project.compute_compat();
        self.sidebar_counts = None;
        self.status_note =
            Some(format!("{name} switches to {alt} at {} ≥ {at:.0}", axis.tag).into());
    }

    /// Drop the rule that substitutes this glyph (the alternates
    /// stay; delete them like any glyph).
    pub(crate) fn command_remove_shape_switch(&mut self) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let Some(name) = project
            .active_font()
            .glyphs
            .get(index)
            .map(|g| g.name.to_string())
        else {
            return;
        };
        let Some(doc) = project.ds_doc.as_mut() else {
            return;
        };
        let before = doc.rules.rules.len();
        doc.rules.rules.retain(|rule| {
            !rule
                .substitutions
                .iter()
                .any(|sub| sub.name.as_str() == name)
        });
        if doc.rules.rules.len() != before {
            project.ds_dirty = true;
            self.status_note = Some(format!("Shape switch removed for {name}").into());
        }
    }

    /// Exchange the drawing with a named layer's copy (editor only,
    /// so the swap is undoable like the background swap).
    pub(crate) fn command_swap_layer(&mut self, layer_name: &str) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        self.push_undo_snapshot(index);
        let mut swapped = false;
        if let Some(font) = self.font_mut() {
            let fg = font
                .font
                .get_glyph(name.as_str())
                .map(|g| g.contours.clone());
            let other = font
                .font
                .layers
                .get(layer_name)
                .and_then(|l| l.get_glyph(name.as_str()))
                .map(|g| g.contours.clone());
            if let (Some(fg), Some(other)) = (fg, other) {
                if let Some(layer) = font.font.layers.get_mut(layer_name)
                    && let Some(g) = layer.get_glyph_mut(name.as_str()) {
                        g.contours = fg;
                    }
                font.edit_glyph(index, |g| {
                    g.contours = other;
                });
                swapped = true;
            }
        }
        if !swapped {
            self.editor.undo.pop();
        }
    }

    /// Drop a layer's copy of the current glyph; an emptied layer
    /// goes with it.
    pub(crate) fn command_delete_layer_glyph(&mut self, layer_name: &str) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        if let Some(font) = self.font_mut() {
            let mut emptied = false;
            if let Some(layer) = font.font.layers.get_mut(layer_name) {
                layer.remove_glyph(name.as_str());
                emptied = layer.is_empty();
            }
            if emptied {
                font.font.layers.remove(layer_name);
            }
            font.dirty = true;
        }
        self.visible_glyph_layers.remove(layer_name);
    }

    /// The Features section's Generate button: rewrite the automatic
    /// blocks (init/medi/fina from name suffixes, liga from
    /// underscore names) into the editor text for review; Apply
    /// commits. Hand-written blocks with other tags are untouched.
    pub(crate) fn command_generate_features(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(font) = self.font() else { return };
        let blocks = Self::generated_feature_blocks(&font.font);
        if blocks.is_empty() {
            self.features_status = Some("Nothing to generate from glyph names".into());
            return;
        }
        let mut fea = self.features_input.read(cx).value().to_string();
        let mut tags: Vec<String> = Vec::new();
        for (tag, body) in blocks {
            fea = Self::replace_feature_block(&fea, &tag, &body);
            tags.push(tag);
        }
        self.features_input.update(cx, |st, cx| {
            st.set_value(fea, window, cx);
        });
        self.features_edited = true;
        self.features_status =
            Some(format!("Generated {} · review and Apply", tags.join(", ")).into());
    }

    /// Apply the features editor to the active master: write
    /// features.fea, recompile the shaping models, and report the
    /// compile verdict. A file that does not compile is still saved
    /// (the old joining rules carry on), the way Glyphs lets you keep
    /// a broken feature file open while you fix it.
    pub(crate) fn command_apply_features(&mut self, cx: &mut Context<Self>) {
        let fea = self.features_input.read(cx).value().to_string();
        let verdict = self.font().map(|f| Self::check_features_compile(f, &fea));
        if let Some(font) = self.font_mut() {
            if font.font.features != fea {
                font.font.features = fea;
                font.dirty = true;
            }
        } else {
            return;
        }
        self.features_edited = false;
        self.features_status = Some(match verdict {
            Some(Ok(())) => "Compiled clean · shaping updated".into(),
            Some(Err(e)) => {
                let first = e.lines().find(|l| !l.trim().is_empty()).unwrap_or("error");
                format!("Saved, but does not compile: {first}").into()
            }
            None => "Applied".into(),
        });
        self.rebuild_text_models();
    }

    /// Add the grid selection to a kerning group (creating it as
    /// needed), on every master. `first_side` = public.kern1, the
    /// first glyph's right edge.
    pub(crate) fn command_add_selection_to_group(&mut self, first_side: bool, group: &str) {
        let names = self.selection_names();
        if names.is_empty() {
            self.status_note = Some("Select glyphs in the grid first".into());
            return;
        }
        let prefix = if first_side {
            "public.kern1."
        } else {
            "public.kern2."
        };
        let full = format!("{prefix}{group}");
        let Ok(group_name) = norad::Name::new(&full) else {
            return;
        };
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let mut added = 0usize;
        for master in project.masters.iter_mut() {
            let members = master.font.groups.entry(group_name.clone()).or_default();
            for name in &names {
                if let Ok(member) = norad::Name::new(name)
                    && !members.contains(&member) {
                        members.push(member);
                        added += 1;
                    }
            }
            master.dirty = true;
        }
        self.rebuild_text_models();
        self.status_note = Some(format!("@{group}: {added} membership(s) added").into());
    }

    /// Drop one glyph from a kerning group, on every master. An
    /// emptied group is removed.
    pub(crate) fn command_remove_from_group(&mut self, full_group: &str, member: &str) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        for master in project.masters.iter_mut() {
            let mut emptied = false;
            if let Some(members) = master.font.groups.get_mut(full_group) {
                members.retain(|m| m.as_str() != member);
                emptied = members.is_empty();
            }
            if emptied {
                master.font.groups.retain(|k, _| k.as_str() != full_group);
            }
            master.dirty = true;
        }
        self.rebuild_text_models();
    }

    /// Append a hex color to the palette, on every master (CPAL
    /// palettes must agree across sources). Returns true on success.
    pub(crate) fn command_add_palette_color(&mut self, hex: &str) -> bool {
        let Some(color) = parse_hex_color(hex) else {
            self.status_note = Some("Color: use #RRGGBB or #RRGGBBAA".into());
            return false;
        };
        let Some(project) = self.project.as_mut() else {
            return false;
        };
        for master in project.masters.iter_mut() {
            let mut palette = read_color_palette(&master.font);
            palette.push(color);
            write_color_palette(&mut master.font, &palette);
            master.dirty = true;
        }
        true
    }

    /// Drop a palette color. Refused while a layer still uses it,
    /// because CPAL indices shift on removal.
    pub(crate) fn command_remove_palette_color(&mut self, index: usize) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let used = project
            .masters
            .first()
            .map(|m| read_color_mapping(&m.font))
            .unwrap_or_default()
            .iter()
            .any(|(_, ci)| *ci == index);
        if used {
            self.status_note = Some("Color is used by a layer · remove the layer first".into());
            return;
        }
        for master in project.masters.iter_mut() {
            let mut palette = read_color_palette(&master.font);
            if index < palette.len() {
                palette.remove(index);
                write_color_palette(&mut master.font, &palette);
                master.dirty = true;
            }
        }
        // Higher indices shifted down: follow them in the mapping.
        for master in project.masters.iter_mut() {
            let mut mapping = read_color_mapping(&master.font);
            for (_, ci) in mapping.iter_mut() {
                if *ci > index {
                    *ci -= 1;
                }
            }
            write_color_mapping(&mut master.font, &mapping);
        }
        if self.color_selected >= index && self.color_selected > 0 {
            self.color_selected -= 1;
        }
    }

    /// Add a color layer: a UFO layer named color.N mapped to the
    /// selected palette color, appended on top. The open glyph's
    /// outline is copied in as a starting point; edit it through the
    /// Glyph Layers swap arrows, drawing per master like any layer.
    pub(crate) fn command_add_color_layer(&mut self) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(project) = self.project.as_ref() else {
            return;
        };
        if read_color_palette(&project.active_font().font).is_empty() {
            self.status_note = Some("Add a palette color first (hex field)".into());
            return;
        }
        let name = project.active_font().glyphs[index].name.to_string();
        let color = self.color_selected;
        let Some(project) = self.project.as_mut() else {
            return;
        };
        // First free color.N name across the mapping.
        let mapping = project
            .masters
            .first()
            .map(|m| read_color_mapping(&m.font))
            .unwrap_or_default();
        let mut n = 0usize;
        let layer_name = loop {
            let candidate = format!("color.{n}");
            if !mapping.iter().any(|(l, _)| *l == candidate) {
                break candidate;
            }
            n += 1;
        };
        for master in project.masters.iter_mut() {
            let mut mapping = read_color_mapping(&master.font);
            mapping.push((layer_name.clone(), color));
            write_color_mapping(&mut master.font, &mapping);
            // Seed the layer with this master's outline of the glyph.
            let seed = master.font.get_glyph(name.as_str()).cloned();
            if let (Some(seed), Ok(layer)) =
                (seed, master.font.layers.get_or_create_layer(&layer_name))
            {
                let mut copy = norad::Glyph::new(name.as_str());
                copy.width = seed.width;
                copy.contours = seed.contours.clone();
                copy.components = seed.components.clone();
                layer.insert_glyph(copy);
            }
            master.dirty = true;
            master.modified_glyphs.insert(name.clone());
        }
        self.show_color_preview = true;
        self.status_note = Some(format!("Color layer {layer_name} added").into());
    }

    /// Remove one mapping row (the UFO layer and its drawings stay).
    pub(crate) fn command_remove_color_layer(&mut self, row: usize) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        for master in project.masters.iter_mut() {
            let mut mapping = read_color_mapping(&master.font);
            if row < mapping.len() {
                mapping.remove(row);
                write_color_mapping(&mut master.font, &mapping);
                master.dirty = true;
            }
        }
    }

    /// Color section's "To v1" button: explode every color glyph's
    /// layers into real suffixed glyphs and write the explicit
    /// colorLayers structures (solid paints), the COLRv1 baseline.
    /// From here ufo2ft's own exploding is off; gradients upgrade
    /// individual paints.
    pub(crate) fn command_convert_to_colrv1(&mut self) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let mut exploded = 0usize;
        for master in project.masters.iter_mut() {
            let mapping = read_color_mapping(&master.font);
            if mapping.is_empty() {
                continue;
            }
            // Which glyphs have color-layer copies at all.
            let color_glyphs: Vec<String> = master
                .font
                .default_layer()
                .iter()
                .map(|g| g.name().to_string())
                .filter(|name| {
                    !name.contains(".color.")
                        && mapping.iter().any(|(layer, _)| {
                            master
                                .font
                                .layers
                                .get(layer)
                                .is_some_and(|l| l.contains_glyph(name))
                        })
                })
                .collect();
            let mut layers_dict = plist::Dictionary::new();
            for name in &color_glyphs {
                let mut rows: Vec<plist::Value> = Vec::new();
                for (layer, color) in &mapping {
                    let Some(copy) = master
                        .font
                        .layers
                        .get(layer)
                        .and_then(|l| l.get_glyph(name.as_str()))
                        .cloned()
                    else {
                        continue;
                    };
                    let suffixed = format!("{name}.{layer}");
                    if master.font.get_glyph(suffixed.as_str()).is_none() {
                        let mut real = norad::Glyph::new(suffixed.as_str());
                        real.width = copy.width;
                        real.contours = copy.contours.clone();
                        real.components = copy.components.clone();
                        master.font.default_layer_mut().insert_glyph(real);
                    }
                    rows.push(paint_glyph_layer(&suffixed, paint_solid(*color)));
                }
                if !rows.is_empty() {
                    let mut root = plist::Dictionary::new();
                    root.insert("Format".into(), plist::Value::Integer(1u64.into()));
                    root.insert("Layers".into(), plist::Value::Array(rows));
                    layers_dict.insert(name.clone(), plist::Value::Dictionary(root));
                    exploded += 1;
                }
            }
            if !layers_dict.is_empty() {
                master.font.lib.insert(
                    COLOR_LAYERS_EXPLICIT_KEY.into(),
                    plist::Value::Dictionary(layers_dict),
                );
                master.dirty = true;
            }
            master.refresh_from_font();
        }
        self.sidebar_counts = None;
        self.status_note = Some(
            if exploded == 0 {
                "No color layers to convert".to_string()
            } else {
                format!("COLRv1: {exploded} glyph entr(ies) written")
            }
            .into(),
        );
    }

    /// Turn one of the selected glyph's color layers into a linear
    /// gradient: from the row's color at the baseline to the
    /// selected swatch at the ascender. Runs the v1 conversion
    /// first when needed.
    pub(crate) fn command_layer_gradient(&mut self, row: usize) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        if !self.font().is_some_and(|f| has_v1_entry(&f.font, &name)) {
            self.command_convert_to_colrv1();
        }
        let stop1 = self.color_selected;
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let mut changed = 0usize;
        for master in project.masters.iter_mut() {
            let (ascender, mapping) = (master.ascender, read_color_mapping(&master.font));
            let Some((_, stop0)) = mapping.get(row) else {
                continue;
            };
            let paint = linear_gradient_paint(*stop0, stop1, (0.0, 0.0), (0.0, ascender));
            let Some(layer) = master
                .font
                .lib
                .get_mut(COLOR_LAYERS_EXPLICIT_KEY)
                .and_then(|v| v.as_dictionary_mut())
                .and_then(|d| d.get_mut(name.as_str()))
                .and_then(|v| v.as_dictionary_mut())
                .and_then(|root| root.get_mut("Layers"))
                .and_then(|v| v.as_array_mut())
                .and_then(|layers| layers.get_mut(row))
                .and_then(|v| v.as_dictionary_mut())
            else {
                continue;
            };
            layer.insert("Paint".into(), paint);
            changed += 1;
            master.dirty = true;
        }
        self.status_note = Some(
            if changed == 0 {
                "Gradient: convert to v1 first (To v1)".to_string()
            } else {
                format!("Layer {row}: linear gradient to color {stop1} in {changed} master(s)")
            }
            .into(),
        );
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
                    runebender_core::glyph_ops::round_selected_corners(g, &selected)
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
    pub(crate) fn command_trace_image(&mut self, cx: &mut Context<Self>) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
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
                        workspace.status_note = Some(format!("Trace: {e}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Choose a model directory and remember it.
    pub(crate) fn command_choose_model(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose model".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(dir) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |workspace, cx| {
                workspace.load_model(&dir);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Score the model on the open glyph against the master furthest
    /// from the active one, which is the one it is trying to predict.
    pub(crate) fn command_score_model(&mut self) {
        let Mode::Editor(index) = self.mode else {
            self.status_note = Some("Open a glyph first".into());
            return;
        };
        let Some(dir) = self.model_dir.clone() else {
            return;
        };
        let Ok(checkpoint) = font_ml::Checkpoint::open(&dir) else {
            return;
        };
        let Some(project) = self.project.as_ref() else {
            return;
        };
        if project.masters.len() < 2 {
            self.status_note = Some("Nothing to score against: one master".into());
            return;
        }
        let target = if project.active == 0 {
            project.masters.len() - 1
        } else {
            0
        };
        let Some(entry) = project.active_font().glyphs.get(index) else {
            return;
        };
        let name = entry.name.to_string();
        let advance = entry.advance;
        let unicode = entry.codepoint.map(|c| c as u32);
        let (Some(from), Some(actual)) = (
            project.active_font().font.get_glyph(name.as_str()),
            project.masters[target].font.get_glyph(name.as_str()),
        ) else {
            return;
        };
        let (Some(from_ops), Some(actual_ops)) = (
            font_ml::ufo::glyph_ops(from),
            font_ml::ufo::glyph_ops(actual),
        ) else {
            self.status_note = Some("No outline to score".into());
            return;
        };
        let Some(model) = self.model_loaded.clone() else {
            return;
        };
        let center = checkpoint
            .config
            .delta_center
            .map(|c| (c[0], c[1]))
            .unwrap_or((0, 0));
        let Ok(result) = font_ml::bolden::bolden(
            &model,
            &name,
            unicode,
            advance,
            &from_ops,
            center,
            checkpoint.config.trim_close,
            self.model_strength,
        ) else {
            return;
        };
        let score = font_ml::eval::score(
            &name,
            &result.to,
            &actual_ops,
            &result.from,
            (center.0 as f64, center.1 as f64),
        );
        if score.model.is_nan() {
            self.status_note = Some("Masters are not point-compatible here".into());
            return;
        }
        self.status_note = Some(
            format!(
                "{name}: model {:.1}, mean-shift {:.1}",
                score.model, score.baseline
            )
            .into(),
        );
        self.model_score = Some((name.into(), score.model, score.baseline));
    }

    /// Glyph > Bolden With Model…: pick a model directory, predict a
    /// heavier version of the open glyph, and install it as a proposal.
    ///
    /// The prediction is structure-forced: the model may only move the
    /// points that are already there, so the result stays
    /// point-compatible with what it came from. It lands in the
    /// current glyph and is undoable, so the way to reject it is
    /// Cmd+Z.
    pub(crate) fn command_bolden_with_model(&mut self, cx: &mut Context<Self>) {
        let Mode::Editor(index) = self.mode else {
            self.status_note = Some("Open a glyph first".into());
            return;
        };
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose model".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(dir) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |workspace, cx| {
                workspace.apply_bolden(index, &dir);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Glyph > Place Image…: copy a picture into the UFO's images
    /// store and set it as this glyph's background image, scaled to
    /// the em and sitting on the descender. The tracing-template
    /// workflow; norad round-trips the images directory.
    pub(crate) fn command_place_image(&mut self, cx: &mut Context<Self>) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Place".into()),
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
                    Ok(bytes) => workspace.apply_place_image(index, &path, bytes),
                    Err(e) => {
                        workspace.status_note = Some(format!("Place image: {e}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// "Smart Axis" on a part glyph: "Width,0,100" writes the
    /// glyphsLib smartComponentAxes entry, marks the default glyph
    /// as the bottom pole, and seeds a part.top layer copy marked
    /// as the top — edit it through the swap arrows, place the part
    /// with a value through the Selection panel.
    pub(crate) fn command_make_smart_axis(&mut self, text: &str) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let mut parts = text.split(',').map(str::trim);
        let Some(name) = parts.next().filter(|n| !n.is_empty()) else {
            return;
        };
        let bottom = parts
            .next()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0);
        let top = parts
            .next()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(100.0);
        let Some(glyph_name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        if let Some(font) = self.font_mut() {
            let Some(source) = font.font.get_glyph(glyph_name.as_str()).cloned() else {
                return;
            };
            // The axis entry, glyphsLib-shaped.
            let mut axis = plist::Dictionary::new();
            axis.insert("name".into(), plist::Value::String(name.into()));
            axis.insert("bottomName".into(), plist::Value::String(String::new()));
            axis.insert("bottomValue".into(), plist::Value::Real(bottom));
            axis.insert("topName".into(), plist::Value::String(String::new()));
            axis.insert("topValue".into(), plist::Value::Real(top));
            if let Some(glyph) = font.font.get_glyph_mut(glyph_name.as_str()) {
                glyph.lib.insert(
                    "com.schriftgestaltung.Glyphs.smartComponentAxes".into(),
                    plist::Value::Array(vec![plist::Value::Dictionary(axis)]),
                );
            }
            // Seed the top pole as a layer copy, marked.
            if let Ok(layer) = font.font.layers.get_or_create_layer("part.top") {
                let mut copy = norad::Glyph::new(glyph_name.as_str());
                copy.width = source.width;
                copy.contours = source.contours.clone();
                let mut pole = plist::Dictionary::new();
                pole.insert(name.to_string(), plist::Value::Integer(2u64.into()));
                copy.lib.insert(
                    "com.runebender.partSelection".into(),
                    plist::Value::Dictionary(pole),
                );
                layer.insert_glyph(copy);
            }
            font.dirty = true;
            font.modified_glyphs.insert(glyph_name.clone());
        }
        self.visible_glyph_layers.insert("part.top".into());
        self.status_note = Some(
            format!(
                "{glyph_name} is a smart part: {name} {bottom:.0}–{top:.0} · edit part.top via the swap arrows"
            )
            .into(),
        );
    }

    /// Set the selected smart component's value on its first axis
    /// (the Selection panel's Smart field).
    pub(crate) fn command_set_smart_value(&mut self, text: &str) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        let Some(ci) = self.editor.selected_component else {
            return;
        };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        // "30" moves the first axis; "Height=30" names one.
        let (axis_named, value) = match text.split_once('=') {
            Some((axis, v)) => (Some(axis.trim()), v.trim().parse::<f64>()),
            None => (None, text.trim().parse::<f64>()),
        };
        let Ok(value) = value else {
            self.status_note = Some("Smart value: 30 or Height=30".into());
            return;
        };
        let axis = self.font().and_then(|f| {
            let glyph = f.font.get_glyph(name.as_str())?;
            let base = f.font.get_glyph(glyph.components.get(ci)?.base.as_str())?;
            let names: Vec<String> = base
                .lib
                .get("com.schriftgestaltung.Glyphs.smartComponentAxes")?
                .as_array()?
                .iter()
                .filter_map(|a| {
                    a.as_dictionary()?
                        .get("name")?
                        .as_string()
                        .map(str::to_string)
                })
                .collect();
            match axis_named {
                Some(want) => names.iter().find(|n| n.eq_ignore_ascii_case(want)).cloned(),
                None => names.first().cloned(),
            }
        });
        let Some(axis) = axis else {
            self.status_note =
                Some("Selected component's base is not smart (or no such axis)".into());
            return;
        };
        if let Some(font) = self.font_mut() {
            let component_count = font
                .font
                .get_glyph(name.as_str())
                .map(|g| g.components.len())
                .unwrap_or(0);
            if let Some(glyph) = font.font.get_glyph_mut(name.as_str()) {
                const KEY: &str = "com.schriftgestaltung.Glyphs.componentsSmartComponentValues";
                let mut rows: Vec<plist::Value> = glyph
                    .lib
                    .get(KEY)
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                rows.resize(
                    component_count,
                    plist::Value::Dictionary(plist::Dictionary::new()),
                );
                if let Some(entry) = rows.get_mut(ci).and_then(|v| v.as_dictionary_mut()) {
                    entry.insert(axis.clone(), plist::Value::Real(value));
                }
                glyph.lib.insert(KEY.into(), plist::Value::Array(rows));
                font.dirty = true;
            }
            font.modified_glyphs.insert(name.clone());
            font.refresh_from_font();
        }
        self.status_note = Some(format!("{axis} = {value:.0} on the component").into());
    }

    /// Flip a contour's mask flag on the open glyph (active master;
    /// mask sets are per master like everything the lib carries).
    pub(crate) fn command_toggle_mask(&mut self, ci: usize) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        if let Some(font) = self.font_mut()
            && let Some(glyph) = font.font.get_glyph_mut(name.as_str()) {
                let mut masks = read_masks(glyph);
                if !masks.remove(&ci) {
                    masks.insert(ci);
                }
                write_masks(glyph, &masks);
                font.dirty = true;
                font.modified_glyphs.insert(name);
            }
    }

    /// Glyph > Bake Masks: make the subtraction real in every
    /// master, so the exported outline matches the preview.
    pub(crate) fn command_bake_masks(&mut self) {
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
        let mut baked = 0usize;
        for master in project.masters.iter_mut() {
            let Some(gi) = master.name_map.get(name.as_str()).copied() else {
                continue;
            };
            if master.edit_glyph(gi, bake_masks).unwrap_or(false) {
                baked += 1;
            }
        }
        project.compute_compat();
        self.editor.selected.clear();
        self.status_note = Some(
            if baked == 0 {
                "No masks to bake".to_string()
            } else {
                format!("Masks baked in {baked} master(s)")
            }
            .into(),
        );
    }

    /// Drop an annotation at a design-space point on the open
    /// glyph (active master; annotations are working notes, never
    /// exported).
    pub(crate) fn command_add_annotation(&mut self, at: (f64, f64), kind: &str, text: &str) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        if let Some(font) = self.font_mut()
            && let Some(glyph) = font.font.get_glyph_mut(name.as_str()) {
                let mut notes = read_annotations(glyph);
                notes.push(Annotation {
                    kind: kind.to_string(),
                    x: at.0.round(),
                    y: at.1.round(),
                    text: text.to_string(),
                });
                write_annotations(glyph, &notes);
                font.dirty = true;
                font.modified_glyphs.insert(name);
            }
    }

    pub(crate) fn command_delete_annotation(&mut self, i: usize) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        if let Some(font) = self.font_mut()
            && let Some(glyph) = font.font.get_glyph_mut(name.as_str()) {
                let mut notes = read_annotations(glyph);
                if i < notes.len() {
                    notes.remove(i);
                    write_annotations(glyph, &notes);
                    font.dirty = true;
                    font.modified_glyphs.insert(name);
                }
            }
    }

    /// Glyph > Import SVG…: parse the file's path outlines and add
    /// them to the open glyph, fitted between descender and
    /// ascender, appended so existing drawing survives (undoable).
    pub(crate) fn command_import_svg(&mut self, cx: &mut Context<Self>) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let text = std::fs::read_to_string(&path);
            this.update(cx, |workspace, cx| {
                let (ascender, descender) = match workspace.font() {
                    Some(f) => (f.ascender, f.descender),
                    None => return,
                };
                match text
                    .map_err(|e| format!("{e}"))
                    .and_then(|t| svg_to_contours(&t, ascender, descender))
                {
                    Ok(contours) => {
                        workspace.push_undo_snapshot(index);
                        let added = contours.len();
                        let ok = workspace
                            .font_mut()
                            .and_then(|f| {
                                f.edit_glyph(index, |g| {
                                    g.contours.extend(contours);
                                    true
                                })
                            })
                            .unwrap_or(false);
                        if ok {
                            workspace.status_note =
                                Some(format!("Imported {added} SVG contour(s)").into());
                        } else {
                            workspace.editor.undo.pop();
                        }
                    }
                    Err(e) => {
                        workspace.status_note = Some(format!("SVG import: {e}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Glyph > Remove Image: unlink this glyph's background image.
    /// The stored file stays; other glyphs may reference it.
    pub(crate) fn command_remove_image(&mut self) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        if let Some(font) = self.font_mut()
            && let Some(glyph) = font.font.get_glyph_mut(name.as_str())
                && glyph.image.take().is_some() {
                    font.dirty = true;
                    font.modified_glyphs.insert(name);
                }
    }

    /// Duplicate the selection: contours holding selected points, or
    /// the selected component or anchor, offset (20, 20), clones
    /// selected (web duplicateSelection).
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
                        runebender_core::glyph_ops::duplicate_selection(g, &selected)
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
                    runebender_core::glyph_ops::transform_selection(g, &selected, transform)
                })
            });
        }
    }

    /// Switch the palette: the app's own colours, the widget library's
    /// theme, and the menu tick all follow.
    pub(crate) fn command_set_theme(
        &mut self,
        id: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !t::set_theme(id) {
            return;
        }
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

    /// Save every dirty master (native), or PUT modified files to the
    /// workspace server (web).
    pub(crate) fn command_save(&mut self, cx: &mut Context<Self>) {
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
                // Instance edits go back into the designspace file.
                if project.ds_dirty
                    && let (Some(doc), Some(path)) =
                        (project.ds_doc.as_ref(), project.export_source.as_ref())
                    && path.extension().is_some_and(|e| e == "designspace")
                {
                    match doc.save(path) {
                        Ok(()) => {
                            project.ds_dirty = false;
                            saved.push(path.display().to_string());
                        }
                        Err(e) => failed.push(format!("{e}")),
                    }
                }
                *self.last_save.lock().unwrap() = web_time::Instant::now();
                self.last_save_label =
                    Some(chrono::Local::now().format("%-I:%M %p").to_string().into());
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

    /// File > Export. Dirty masters are saved first because the
    /// build reads from disk. With a Google Fonts build script above
    /// the source, that script is the export — the repo pipeline is
    /// the compatibility authority. Otherwise fontc compiles the
    /// source directly, with a gftools-fix-font pass when the tool
    /// can be found. Runs in the background; reports through the
    /// status note.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn command_export(&mut self, cx: &mut Context<Self>) {
        if self
            .project
            .as_ref()
            .is_some_and(|p| p.masters.iter().any(|m| m.dirty))
        {
            self.command_save(cx);
        }
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let source = project
            .export_source
            .clone()
            .unwrap_or_else(|| project.masters[project.active].source_path.clone());
        if !source.exists() {
            self.status_note = Some("Save the font before exporting".into());
            return;
        }
        if let Some((script, workdir)) = Self::gf_build_script(&source) {
            let label = script
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "build script".into());
            self.status_note = Some(format!("Exporting through {label}…").into());
            cx.spawn(async move |this, cx| {
                let result: Result<String, String> = cx
                    .background_executor()
                    .spawn({
                        let label = label.clone();
                        async move {
                            let path_env = Workspace::export_path_env(Some(&workdir));
                            let output = std::process::Command::new("/bin/bash")
                                .arg(&script)
                                .current_dir(&workdir)
                                .env("PATH", path_env)
                                .output()
                                .map_err(|e| format!("{e}"))?;
                            if output.status.success() {
                                Ok(format!(
                                    "Exported through {label} → {}",
                                    workdir.join("fonts").display()
                                ))
                            } else {
                                let stderr = String::from_utf8_lossy(&output.stderr);
                                Err(stderr
                                    .lines()
                                    .rev()
                                    .find(|l| !l.trim().is_empty())
                                    .unwrap_or("build script failed")
                                    .to_string())
                            }
                        }
                    })
                    .await;
                this.update(cx, |workspace, cx| {
                    workspace.status_note = Some(match result {
                        Ok(note) => note.into(),
                        Err(e) => format!("Export failed: {e}").into(),
                    });
                    cx.notify();
                })
                .ok();
            })
            .detach();
            return;
        }
        let Some(fontc) = fontc_binary() else {
            self.status_note = Some("fontc not found: cargo install fontc".into());
            return;
        };
        let out_dir = source
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("exports");
        let stem = source
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "font".into());
        let out_file = out_dir.join(format!("{stem}.ttf"));
        self.status_note = Some(format!("Exporting {stem}.ttf…").into());
        cx.spawn(async move |this, cx| {
            let result: Result<(PathBuf, bool), String> = cx
                .background_executor()
                .spawn(async move {
                    std::fs::create_dir_all(&out_dir).map_err(|e| format!("{e}"))?;
                    // fontc's working files go to a temp dir, not the
                    // font's directory, so the file watcher and git
                    // status stay quiet.
                    let build_dir = std::env::temp_dir().join("runebender-fontc");
                    let output = std::process::Command::new(&fontc)
                        .arg(&source)
                        .arg("--output-file")
                        .arg(&out_file)
                        .arg("--build-dir")
                        .arg(&build_dir)
                        .output()
                        .map_err(|e| format!("{e}"))?;
                    if output.status.success() {
                        // Google Fonts spec fixes when gftools is
                        // around (PATH after export_path_env, which
                        // includes any repo venv above the source).
                        let path_env = Workspace::export_path_env(source.parent());
                        let fixed = std::process::Command::new("gftools-fix-font")
                            .arg("-o")
                            .arg(&out_file)
                            .arg(&out_file)
                            .env("PATH", path_env)
                            .output()
                            .is_ok_and(|o| o.status.success());
                        Ok((out_file, fixed))
                    } else {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        Err(stderr
                            .lines()
                            .rev()
                            .find(|l| !l.trim().is_empty())
                            .unwrap_or("fontc failed")
                            .to_string())
                    }
                })
                .await;
            this.update(cx, |workspace, cx| {
                workspace.status_note = Some(match result {
                    Ok((path, fixed)) => if fixed {
                        format!("Exported {} (gftools fixes applied)", path.display())
                    } else {
                        format!(
                            "Exported {} (no gftools on PATH: skipped GF fixes)",
                            path.display()
                        )
                    }
                    .into(),
                    Err(e) => format!("Export failed: {e}").into(),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The browser build has no fontc to run; exporting is native.
    #[cfg(target_family = "wasm")]
    pub(crate) fn command_export(&mut self, _cx: &mut Context<Self>) {
        self.status_note = Some("Export runs in the native app only".into());
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

    /// Cmd+V, routed the web way: copied contours paste whenever the
    /// outline clipboard holds something and the Text tool isn't the
    /// one in hand; otherwise the system clipboard's text types into
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

    /// Boolean path op over the glyph's contours (web boolean tiles):
    /// union merges everything; the others apply first contour vs the
    /// rest combined.
    /// Expand contours into stroked outlines (the Make Stroke half
    /// of Glyphs' Offset Curve): each selected contour — all when
    /// nothing is selected — becomes the outline of a stroke of the
    /// typed width, round joins and caps. The monoline workflow: draw
    /// open skeleton paths, type a weight, get letterforms.
    pub(crate) fn command_expand_stroke(&mut self, width: f64) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        if width.is_nan() || width <= 0.0 {
            return;
        }
        self.push_undo_snapshot(index);
        let selected_contours: std::collections::HashSet<usize> =
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

    /// View > Next/Previous Sample String: rebuild the text buffer
    /// as sample text around the open glyph.
    pub(crate) fn command_sample_string(&mut self, step: isize) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        let Some(font) = self.font() else { return };
        let entry = &font.glyphs[index];
        let (name, codepoint, advance) = (entry.name.to_string(), entry.codepoint, entry.advance);
        let count = SAMPLE_STRINGS.len() as isize;
        self.sample_index = (self.sample_index as isize + step).rem_euclid(count) as usize;
        let sample = SAMPLE_STRINGS[self.sample_index];
        self.edit_buffer.clear();
        // The open glyph leads; the sample text follows it.
        self.edit_buffer.insert_glyph(&name, codepoint, advance);
        self.edit_buffer.activate_sort(0);
        for c in sample.chars() {
            self.edit_buffer.insert_character(c);
        }
        self.edit_buffer.activate_sort(0);
        self.sync_sort_offset();
        self.status_note = Some(format!("Sample: {sample}").into());
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
        let selected_contours: std::collections::HashSet<usize> =
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

    pub(crate) fn command_boolean(&mut self, op: linesweeper::BinaryOp) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        self.push_undo_snapshot(index);
        let changed = self
            .font_mut()
            .and_then(|f| {
                f.edit_glyph(
                    index,
                    |g| match runebender_core::glyph_ops::boolean_contours(g, op) {
                        Some(contours) => {
                            g.contours = contours;
                            true
                        }
                        None => false,
                    },
                )
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
    pub(crate) fn command_step_master(&mut self, delta: isize) {
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

    /// Add (or replace) one avar mapping pair on the first axis:
    /// user-space input → design-space output, written into the
    /// designspace and saved with it.
    pub(crate) fn command_add_axis_mapping(&mut self, input: f32, output: f32) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let Some(doc) = project.ds_doc.as_mut() else {
            self.status_note = Some("Axis mappings need a designspace project".into());
            return;
        };
        let Some(axis) = doc.axes.first_mut() else {
            return;
        };
        let map = axis.map.get_or_insert_with(Vec::new);
        map.retain(|m| (m.input - input).abs() > 0.01);
        map.push(norad::designspace::AxisMapping { input, output });
        map.sort_by(|a, b| a.input.total_cmp(&b.input));
        project.ds_dirty = true;
        self.status_note = Some(format!("Axis map: {input:.0} → {output:.0}").into());
    }

    pub(crate) fn command_remove_axis_mapping(&mut self, index: usize) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let Some(doc) = project.ds_doc.as_mut() else {
            return;
        };
        let Some(axis) = doc.axes.first_mut() else {
            return;
        };
        if let Some(map) = axis.map.as_mut()
            && index < map.len() {
                map.remove(index);
                if map.is_empty() {
                    axis.map = None;
                }
                project.ds_dirty = true;
            }
    }

    /// Enter in the Instances field: rename the instance sitting at
    /// the preview location, or add a new one there. The name is the
    /// style name; the full name follows the family.
    pub(crate) fn command_instance_upsert(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let Some(project) = self.project.as_mut() else {
            return;
        };
        if project.ds_doc.is_none() {
            self.status_note = Some("Instances need a designspace project".into());
            return;
        }
        // The preview location in design coordinates, one value per
        // axis, computed before the document is borrowed.
        let wants: Vec<(String, f64)> = project
            .axes
            .iter()
            .map(|axis| {
                let normalized = project.location.get(&axis.name).copied().unwrap_or(0.0);
                let raw = runebender_core::var_model::denormalize_value(
                    normalized,
                    axis.min,
                    axis.default,
                    axis.max,
                );
                (axis.name.clone(), raw)
            })
            .collect();
        let defaults: std::collections::HashMap<String, f64> = project
            .axes
            .iter()
            .map(|a| (a.name.clone(), a.default))
            .collect();
        let family = project
            .masters
            .first()
            .and_then(|m| m.font.font_info.family_name.clone());
        let doc = project.ds_doc.as_mut().expect("checked above");
        let at_location = |inst: &norad::designspace::Instance| {
            wants.iter().all(|(axis, want)| {
                let got = inst
                    .location
                    .iter()
                    .find(|d| d.name == *axis)
                    .and_then(|d| d.xvalue.or(d.uservalue))
                    .map(|v| v as f64)
                    .or_else(|| defaults.get(axis).copied())
                    .unwrap_or(0.0);
                (got - want).abs() < 0.5
            })
        };
        let note = match doc.instances.iter().position(at_location) {
            Some(i) => {
                let inst = &mut doc.instances[i];
                inst.stylename = Some(name.to_string());
                if let Some(fam) = &family {
                    inst.name = Some(format!("{fam} {name}"));
                    let (map_family, map_style) = Self::style_linking(fam, name);
                    inst.stylemapfamilyname = Some(map_family);
                    inst.stylemapstylename = Some(map_style);
                }
                format!("Renamed instance to {name}")
            }
            None => {
                let stylemap = family.as_ref().map(|fam| Self::style_linking(fam, name));
                doc.instances.push(norad::designspace::Instance {
                    familyname: family.clone(),
                    stylename: Some(name.to_string()),
                    name: family.as_ref().map(|f| format!("{f} {name}")),
                    stylemapfamilyname: stylemap.as_ref().map(|(f, _)| f.clone()),
                    stylemapstylename: stylemap.as_ref().map(|(_, st)| st.clone()),
                    location: wants
                        .iter()
                        .map(|(axis, value)| norad::designspace::Dimension {
                            name: axis.clone(),
                            xvalue: Some(*value as f32),
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                });
                format!("Added instance {name}")
            }
        };
        project.ds_dirty = true;
        project.refresh_instances_from_doc();
        self.status_note = Some(note.into());
    }

    /// Drop one instance (the × on its row). Saved with the font.
    pub(crate) fn command_instance_delete(&mut self, index: usize) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let Some(doc) = project.ds_doc.as_mut() else {
            return;
        };
        if index < doc.instances.len() {
            doc.instances.remove(index);
            project.ds_dirty = true;
            project.refresh_instances_from_doc();
        }
    }

    /// Add an empty glyph to every master (bottom bar +), like
    /// Glyphs' new-glyph command, and select it.
    pub(crate) fn command_add_glyph(&mut self) {
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
        self.selected = self.font().and_then(|f| f.name_map.get(&name).copied());
        self.sidebar_counts = None;
        self.status_note = Some(format!("Added {name}").into());
    }

    /// Remove the selected glyph from every master (bottom bar −).
    pub(crate) fn command_remove_glyph(&mut self) {
        let Some(index) = self.selected else {
            self.status_note = Some("Select a glyph to remove".into());
            return;
        };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
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
}
