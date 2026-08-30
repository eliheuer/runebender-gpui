// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! The text tool: a line of sorts you type into and edit in place.
//!
//! Sort geometry, the models behind each sort, kerning edits made by
//! dragging sorts, and pasting into the buffer.

use super::*;

impl Workspace {
    /// Paste the system clipboard's text into the editor's buffer,
    /// character by character (web pasteTextIntoBuffer): switches to
    /// the Text tool, line breaks for newlines, characters with no
    /// glyph skipped.
    pub(crate) fn paste_text_into_buffer(&mut self, cx: &mut Context<Self>) {
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

    /// Bottom bar in editor mode: Width / LSB / RSB fields.
    /// The resolved preview line: glyph index, pen x position (font
    /// units, kerning applied), and advance.
    /// The text sort metric box bounds: top = max(upm, ascender),
    /// bottom = descender — the web editor's text_sort_metric_bounds.
    pub(crate) fn text_sort_bounds(&self) -> (f64, f64) {
        let Some(font) = self.font() else {
            return (1000.0, -200.0);
        };
        (font.units_per_em.max(font.ascender), font.descender)
    }

    /// Line height for the text buffers: the sort box height, so a
    /// second line's box top sits exactly on the first line's bottom.
    pub(crate) fn text_line_height(&self) -> f64 {
        let (top, bottom) = self.text_sort_bounds();
        (top - bottom).max(1.0)
    }

    /// Rebuild the text engine's font models from the active master
    /// (glyph advances, unicode map, kerning with groups, features
    /// for shaping), and refresh the advances of sorts already in
    /// the buffer.
    pub(crate) fn rebuild_text_models(&mut self) {
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
    pub(crate) fn sync_sort_offset(&mut self) {
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
    pub(crate) fn text_tool_click(&mut self, pos: Point<gpui::Pixels>, shift: bool) {
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
            if let Some(index) = hit.active_sort
                && self.edit_buffer.begin_manual_kerning(index, bx)
            {
                self.editor.drag = Some(Drag::TextKern);
                self.sync_sort_offset();
                return;
            }
        }
        self.edit_buffer
            .place_cursor_at(bx, by, line_height, top, bottom);
    }

    /// Double-click editing, in the web's priority order: toggle the
    /// point type under the cursor, else select its whole contour.
    pub(crate) fn double_click_edit(&mut self, pos: Point<gpui::Pixels>) -> bool {
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
    pub(crate) fn activate_sort_at_pos(&mut self, pos: Point<gpui::Pixels>) -> bool {
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
        if let Some(glyph) = target
            && !matches!(self.mode, Mode::Editor(i) if i == glyph)
        {
            self.mode = Mode::Editor(glyph);
            self.selected = Some(glyph);
            self.editor.selected.clear();
            self.editor.selected_anchors.clear();
            self.editor.drag = None;
            self.editor.undo.clear();
            self.editor.redo.clear();
        }
        self.sync_sort_offset();
        true
    }

    /// Write the buffer's kerning (updated by a manual kern drag)
    /// back into the font, wholesale like the web editor does.
    pub(crate) fn sync_kerning_from_buffer(&mut self) {
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
    pub(crate) fn seed_edit_buffer(&mut self, index: usize) {
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
}
