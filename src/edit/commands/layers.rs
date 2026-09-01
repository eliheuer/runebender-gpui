// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Layers: background, backup, brace, and mask layers.

use crate::Mode;
use crate::Workspace;
use crate::edit::commands::ds_f32;
use runebender_core::document::project::BraceSource;
use runebender_core::formats::lib_keys::bake_masks;
use runebender_core::formats::lib_keys::read_masks;
use runebender_core::formats::lib_keys::write_masks;
impl Workspace {
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
                        && let Some(g) = layer.get_glyph_mut(name.as_str())
                    {
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
    /// (backup-1, backup-2, …). This is the copy-layer gesture in
    /// Glyphs.
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
            let mut n = 0_usize;
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

    /// Freeze the current interpolation of the open glyph into a
    /// brace layer at the preview location. This is "+ Intermediate"
    /// in the layers block.
    ///
    /// A brace layer is a named UFO layer plus a sparse designspace
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
                    runebender_core::document::var_model::denormalize_value(
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
            && let Ok(layer) = font.font.layers.get_or_create_layer(&layer_name)
        {
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
                        xvalue: Some(ds_f32(*value)),
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
                    && let Some(g) = layer.get_glyph_mut(name.as_str())
                {
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
            && let Some(glyph) = font.font.get_glyph_mut(name.as_str())
        {
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
        let mut baked = 0_usize;
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
}
