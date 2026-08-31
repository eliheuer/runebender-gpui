// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! Annotations on the glyph.

use crate::Workspace;
use runebender_core::formats::lib_keys::Annotation;
use runebender_core::formats::lib_keys::read_annotations;
use runebender_core::formats::lib_keys::write_annotations;
impl Workspace {
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
            && let Some(glyph) = font.font.get_glyph_mut(name.as_str())
        {
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
            && let Some(glyph) = font.font.get_glyph_mut(name.as_str())
        {
            let mut notes = read_annotations(glyph);
            if i < notes.len() {
                notes.remove(i);
                write_annotations(glyph, &notes);
                font.dirty = true;
                font.modified_glyphs.insert(name);
            }
        }
    }
}
