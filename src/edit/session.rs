// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Which font is active, and the editor sessions over it.
//!
//! Switching masters, parking and resuming a glyph's edit session, undo
//! and redo snapshots, and the operation journal.

use crate::Mode;
use crate::Workspace;
#[cfg(not(target_family = "wasm"))]
use crate::platform::journal;
use crate::workspace::EditSession;
use crate::workspace::EditorState;
use crate::workspace::Tool;
use runebender_core::document::project::Master;
impl Workspace {
    /// The active master, if a project is open.
    pub(crate) fn font(&self) -> Option<&Master> {
        self.project.as_ref().map(|p| p.active_font())
    }

    /// The active master, mutably, if a project is open.
    pub(crate) fn font_mut(&mut self) -> Option<&mut Master> {
        self.project.as_mut().map(|p| p.active_font_mut())
    }

    /// Switch the active master, keeping the open glyph (by name)
    /// when it exists in the target master.
    pub(crate) fn switch_master(&mut self, master: usize) {
        self.sidebar.counts = None;
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
        project.snap_location_to_master(master);
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

    /// Write the live editor state back into the active session's
    /// slot before switching to another tab.
    pub(crate) fn park_active_session(&mut self) {
        let glyph = match self.mode {
            Mode::Editor(i) => Some(i),
            Mode::Grid => self.last_editor,
        };
        let name = glyph
            .and_then(|i| self.font().and_then(|f| f.glyphs.get(i)))
            .map(|g| g.name.to_string());
        let Some(slot) = self.sessions.get_mut(self.active_session) else {
            return;
        };
        if let Some(name) = name {
            slot.glyph_name = name;
        }
        slot.editor = std::mem::replace(&mut self.editor, EditorState::new());
        slot.buffer = std::mem::replace(
            &mut self.edit_buffer,
            runebender_core::text::buffer::TextBuffer::new(),
        );
    }

    /// Switch to another edit tab, restoring its buffer, tool,
    /// selection, viewport, and undo stack as they were left.
    pub(crate) fn activate_session(&mut self, target: usize) {
        if target >= self.sessions.len() {
            return;
        }
        let switching = target != self.active_session;
        if switching {
            self.park_active_session();
            let slot = &mut self.sessions[target];
            self.editor = std::mem::replace(&mut slot.editor, EditorState::new());
            self.edit_buffer = std::mem::replace(
                &mut slot.buffer,
                runebender_core::text::buffer::TextBuffer::new(),
            );
            self.active_session = target;
        }
        let name = self.sessions[target].glyph_name.clone();
        let Some(&index) = self.font().and_then(|f| f.name_map.get(name.as_str())) else {
            // The glyph is gone (removed, or absent from this
            // master): drop the dead tab.
            self.close_session(target);
            return;
        };
        self.mode = Mode::Editor(index);
        self.selected = Some(index);
        self.last_editor = Some(index);
        self.status_note = None;
    }

    /// Close an edit tab. Closing the active one activates its
    /// neighbor; closing the last returns to the overview.
    pub(crate) fn close_session(&mut self, target: usize) {
        if target >= self.sessions.len() {
            return;
        }
        self.sessions.remove(target);
        if self.sessions.is_empty() {
            self.active_session = 0;
            self.editor = EditorState::new();
            self.edit_buffer = runebender_core::text::buffer::TextBuffer::new();
            self.last_editor = None;
            self.mode = Mode::Grid;
            return;
        }
        match target.cmp(&self.active_session) {
            std::cmp::Ordering::Less => self.active_session -= 1,
            std::cmp::Ordering::Equal => {
                // The live state belonged to the removed tab: load the
                // neighbor without parking.
                let next = target.min(self.sessions.len() - 1);
                let slot = &mut self.sessions[next];
                self.editor = std::mem::replace(&mut slot.editor, EditorState::new());
                self.edit_buffer = std::mem::replace(
                    &mut slot.buffer,
                    runebender_core::text::buffer::TextBuffer::new(),
                );
                self.active_session = next;
                let name = self.sessions[next].glyph_name.clone();
                match self
                    .font()
                    .and_then(|f| f.name_map.get(name.as_str()))
                    .copied()
                {
                    Some(index) => {
                        if matches!(self.mode, Mode::Editor(_)) {
                            self.mode = Mode::Editor(index);
                        }
                        self.selected = Some(index);
                        self.last_editor = Some(index);
                    }
                    None => self.close_session(next),
                }
            }
            std::cmp::Ordering::Greater => {}
        }
    }

    /// Open the glyph at `index` in the editor, resetting tool, selection, and undo history.
    pub(crate) fn open_editor(&mut self, index: usize) {
        // Opening from the grid lands in the active tab; the first
        // open creates it.
        if self.sessions.is_empty() {
            self.sessions.push(EditSession {
                glyph_name: String::new(),
                editor: EditorState::new(),
                buffer: runebender_core::text::buffer::TextBuffer::new(),
            });
            self.active_session = 0;
        }
        if let Some(name) = self
            .font()
            .and_then(|f| f.glyphs.get(index))
            .map(|g| g.name.to_string())
            && let Some(slot) = self.sessions.get_mut(self.active_session)
        {
            slot.glyph_name = name;
        }
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
        self.editor.selected_anchors.clear();
    }

    /// The open glyph in the editor, or the grid selection.
    pub(crate) fn current_glyph_index(&self) -> Option<usize> {
        match self.mode {
            Mode::Editor(index) => Some(index),
            Mode::Grid => self.selected,
        }
    }

    /// Fit the editor view to the open glyph's metrics, once per open.
    pub(crate) fn ensure_editor_fit(&mut self) {
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

    /// Re-point selection, the open editor, and the parked session
    /// at the glyph named `name`. Called after a rename or unicode
    /// change reorders the glyph list.
    pub(crate) fn remap_glyph_indices(&mut self, name: &str) {
        let Some(&index) = self.font().and_then(|f| f.name_map.get(name)) else {
            return;
        };
        if self.selected.is_some() {
            self.selected = Some(index);
        }
        if matches!(self.mode, Mode::Editor(_)) {
            self.mode = Mode::Editor(index);
        }
        if self.last_editor.is_some() {
            self.last_editor = Some(index);
        }
    }

    /// Note an edit in the session journal, if one is configured.
    ///
    /// Resolving the glyph name here rather than at each call site
    /// keeps the callers to one line, which is the only way a log like
    /// this stays in step with the code.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn journal(&self, op: &str, index: Option<usize>, detail: Option<String>) {
        let name = index.and_then(|i| self.font().map(|f| f.glyphs[i].name.to_string()));
        journal::record(journal::Entry {
            op,
            glyph: name.as_deref(),
            detail,
        });
    }

    /// There is no file to append to in the browser.
    #[cfg(target_family = "wasm")]
    pub(crate) fn journal(&self, _op: &str, _index: Option<usize>, _detail: Option<String>) {}

    /// Snapshot the glyph's contours onto the undo stack and clear redo.
    pub(crate) fn push_undo_snapshot(&mut self, index: usize) {
        // Any other edit ends a nudge burst, so the next arrow press
        // opens a fresh undo group.
        self.nudging = false;
        if let Some(snapshot) = self.font().and_then(|f| f.snapshot_contours(index)) {
            self.editor.undo.push(snapshot);
            self.editor.redo.clear();
        }
    }

    /// Snapshot for a nudge: a run of arrow presses is one undo step,
    /// the way the web commits one group per burst
    /// (`finishNudgeSelection` on key-up).
    pub(crate) fn push_nudge_snapshot(&mut self, index: usize) {
        if self.nudging {
            return;
        }
        self.push_undo_snapshot(index);
        self.nudging = true;
    }

    /// Restore the last undo snapshot, pushing the current state onto redo.
    pub(crate) fn undo(&mut self) {
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

    /// Restore the last redo snapshot, pushing the current state onto undo.
    pub(crate) fn redo(&mut self) {
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
}
