// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! File menu: new, open, save, export, and the session-level toggles.

use crate::Mode;
use crate::PathBuf;
use crate::Workspace;
use crate::app_menus;
use crate::edit::commands::rotate;
#[cfg(not(target_family = "wasm"))]
use crate::platform::host::fontc_binary;
use crate::view::theme as t;
use crate::workspace::EditSession;
use crate::workspace::EditorState;
use crate::workspace::SAMPLE_STRINGS;
use crate::workspace::SidebarFilter;
use gpui::Context;
use gpui::Window;
use runebender_core::document::project::Project;
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
            buffer: runebender_core::text::buffer::TextBuffer::new(),
        });
        self.active_session = self.sessions.len() - 1;
        self.open_editor(glyph);
    }

    /// File > New Font: an Untitled GF-template UFO, in memory until
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
        self.grid.multi_selected.clear();
        self.last_editor = None;
        self.sidebar.counts = None;
        self.sidebar.matches = None;
        self.sidebar.filter = SidebarFilter::All;
        self.sidebar.search_query.clear();
        self.rebuild_text_models();
        self.status_note = Some("New font · Save As… picks where it lives on disk".into());
    }

    /// Save As: pick a directory; the active master saves there under
    /// its family-style name and keeps saving there from now on.
    pub(crate) fn command_save_as(&mut self, cx: &mut Context<'_, Self>) {
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

    /// Switch the palette: the app's own colours, the widget library's
    /// theme, and the menu tick all follow.
    pub(crate) fn command_set_theme(
        &mut self,
        id: &str,
        _window: &mut Window,
        cx: &mut Context<'_, Self>,
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
    pub(crate) fn command_save(&mut self, cx: &mut Context<'_, Self>) {
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

    /// Export the font (File > Export). Dirty masters are saved
    /// first because the build reads from disk. With a Google Fonts
    /// build script above the source, that script is the export: the
    /// repo pipeline is the compatibility authority. Otherwise fontc
    /// compiles the source directly, with a gftools-fix-font pass
    /// when the tool can be found. Runs in the background; reports
    /// through the status note.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn command_export(&mut self, cx: &mut Context<'_, Self>) {
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
                            let path_env = Self::export_path_env(Some(&workdir));
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
                        let path_env = Self::export_path_env(source.parent());
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
    pub(crate) fn command_export(&mut self, _cx: &mut Context<'_, Self>) {
        self.status_note = Some("Export runs in the native app only".into());
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
        let count = SAMPLE_STRINGS.len();
        self.preview.sample_index = rotate(self.preview.sample_index, count, step);
        let sample = SAMPLE_STRINGS[self.preview.sample_index];
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

    /// Step to the next/previous master (menu: View).
    pub(crate) fn command_step_master(&mut self, delta: isize) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let n = project.masters.len();
        if n < 2 {
            return;
        }
        let next = rotate(project.active, n, delta);
        self.switch_master(next);
    }
}
