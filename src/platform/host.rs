// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Files: build scripts, export paths, reloading a master from disk,
//! and the paths the editor reads and writes.
//!
//! Watching those files for other writers is `platform::watch`.

use crate::Mode;
#[cfg(not(target_family = "wasm"))]
use crate::PathBuf;
use crate::Workspace;
#[cfg(target_family = "wasm")]
use crate::platform::web_host;
use gpui::Context;
#[cfg(not(target_family = "wasm"))]
use runebender_core::document::project::Master;
use runebender_core::document::project::Project;
#[cfg(target_family = "wasm")]
use std::collections::HashMap;
impl Workspace {
    /// The repo's own Google Fonts build script above the source,
    /// with the directory to run it from.
    ///
    /// `build-fontc.sh` is preferred, then `build.sh`. A repo
    /// pipeline carries the gftools fixes, STAT, and statics that a
    /// raw compile does not.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn gf_build_script(source: &std::path::Path) -> Option<(PathBuf, PathBuf)> {
        let mut dir = source.parent()?;
        for _ in 0..4 {
            for name in ["build-fontc.sh", "build.sh"] {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some((candidate, dir.to_path_buf()));
                }
            }
            if dir.join(".git").exists() {
                break;
            }
            dir = dir.parent()?;
        }
        None
    }

    /// PATH for export child processes. The app may have been
    /// launched from the Dock with the minimal system PATH, so the
    /// places build scripts expect (cargo bin, Homebrew, the repo
    /// venv) are put back in front.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn export_path_env(workdir: Option<&std::path::Path>) -> std::ffi::OsString {
        let mut parts: Vec<PathBuf> = Vec::new();
        if let Some(workdir) = workdir {
            parts.push(workdir.join(".venv/bin"));
        }
        if let Some(home) = std::env::var_os("HOME") {
            parts.push(PathBuf::from(&home).join(".cargo/bin"));
        }
        parts.push(PathBuf::from("/opt/homebrew/bin"));
        parts.push(PathBuf::from("/usr/local/bin"));
        if let Some(path) = std::env::var_os("PATH") {
            parts.extend(std::env::split_paths(&path));
        }
        std::env::join_paths(parts.into_iter().filter(|p| p.exists()))
            .unwrap_or_else(|_| std::env::var_os("PATH").unwrap_or_default())
    }

    /// Re-read every clean master from disk, keeping the open glyph.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn reload_from_disk(&mut self) {
        self.sidebar.counts = None;
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let open_glyph_name = match self.mode {
            Mode::Editor(i) => Some(project.active_font().glyphs[i].name.clone()),
            Mode::Grid => None,
        };
        let mut skipped_dirty = false;
        for master in project.masters.iter_mut() {
            if master.dirty {
                skipped_dirty = true;
                continue;
            }
            if let Ok(fresh) = Master::load(&master.source_path) {
                *master = fresh;
            }
        }
        if let Some(name) = open_glyph_name {
            match project
                .active_font()
                .glyphs
                .iter()
                .position(|g| g.name == name)
            {
                Some(index) => {
                    self.mode = Mode::Editor(index);
                    self.editor.selected.clear();
                    self.editor.selected_anchors.clear();
                    self.editor.drag = None;
                }
                None => self.mode = Mode::Grid,
            }
        }
        self.status_note = Some(if skipped_dirty {
            "Changed on disk · dirty masters kept your unsaved edits".into()
        } else {
            "Reloaded from disk".into()
        });
    }

    /// Connect to the workspace server named by `?server=` and load
    /// its fonts (web builds).
    #[cfg(target_family = "wasm")]
    pub(crate) fn connect_web_host(&mut self, base: String, cx: &mut Context<'_, Self>) {
        self.status_note = Some(format!("Connecting to {base}…").into());
        let client = cx.http_client();
        cx.spawn(async move |this, cx| {
            let fetched = web_host::fetch_workspace(client, base.clone()).await;
            this.update(cx, |workspace, cx| {
                match fetched.and_then(|fetched| {
                    web_host::project_from_fetched(&fetched).map(|built| (fetched, built))
                }) {
                    Ok((fetched, (project, ufo_prefixes))) => {
                        let n = project.masters.len();
                        workspace.axis_sliders.clear();
                        workspace.sessions.clear();
                        workspace.active_session = 0;
                        workspace.last_editor = None;
                        workspace.project = Some(project);
                        workspace.refresh_proposal();
                        workspace.sidebar.counts = None;
                        workspace.load_error = None;
                        workspace.mode = Mode::Grid;
                        workspace.selected = None;
                        workspace.web_host = Some(web_host::WebHost {
                            base,
                            etags: fetched.etags,
                            ufo_prefixes,
                        });
                        workspace.status_note = Some(
                            format!("Connected · {n} masters · Cmd+S saves to the server").into(),
                        );
                    }
                    Err(e) => {
                        workspace.load_error = Some(format!("{e}").into());
                        workspace.status_note = None;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Save dirty masters to the workspace server (web builds):
    /// modified glifs and kerning, each PUT with its `If-Match`
    /// ETag.
    #[cfg(target_family = "wasm")]
    pub(crate) fn save_to_web_host(&mut self, cx: &mut Context<'_, Self>) {
        let Some(host) = self.web_host.as_ref() else {
            self.status_note = Some("No server connected: open with ?server=http://…".into());
            return;
        };
        let Some(project) = self.project.as_ref() else {
            return;
        };
        // Collect the files to write while we hold &self.
        let mut to_save: Vec<web_host::SaveFile> = Vec::new();
        let mut saved_masters: Vec<usize> = Vec::new();
        for (mi, master) in project.masters.iter().enumerate() {
            if !master.dirty {
                continue;
            }
            let Some(prefix) = host.ufo_prefixes.get(mi) else {
                continue;
            };
            for name in &master.modified_glyphs {
                let Some(glyph) = master.font.get_glyph(name.as_str()) else {
                    continue;
                };
                let Some(rel) = master.glif_paths.get(name) else {
                    continue;
                };
                match runebender_core::document::font_memory::glif_bytes(glyph) {
                    Ok(bytes) => to_save.push(web_host::SaveFile {
                        path: format!("{prefix}{rel}"),
                        bytes,
                    }),
                    Err(e) => {
                        self.status_note = Some(format!("{e}").into());
                        return;
                    }
                }
            }
            if master.kerning_dirty {
                match runebender_core::document::font_memory::kerning_plist_bytes(&master.font) {
                    Ok(bytes) => to_save.push(web_host::SaveFile {
                        path: format!("{prefix}kerning.plist"),
                        bytes,
                    }),
                    Err(e) => {
                        self.status_note = Some(format!("{e}").into());
                        return;
                    }
                }
            }
            saved_masters.push(mi);
        }
        if to_save.is_empty() {
            self.status_note = Some("Nothing to save".into());
            return;
        }
        let base = host.base.clone();
        let etags: HashMap<String, String> = host.etags.clone();
        let client = cx.http_client();
        let count = to_save.len();
        self.status_note = Some(format!("Saving {count} files…").into());
        cx.spawn(async move |this, cx| {
            let mut new_etags: Vec<(String, String)> = Vec::new();
            let mut failure: Option<String> = None;
            for file in &to_save {
                match web_host::put_file(
                    &client,
                    &base,
                    file,
                    etags.get(&file.path).map(|s| s.as_str()),
                )
                .await
                {
                    Ok(etag) => new_etags.push((file.path.clone(), etag)),
                    Err(e) => {
                        failure = Some(e);
                        break;
                    }
                }
            }
            this.update(cx, |workspace, cx| {
                if let Some(host) = workspace.web_host.as_mut() {
                    for (path, etag) in new_etags {
                        host.etags.insert(path, etag);
                    }
                }
                workspace.status_note = Some(match failure {
                    Some(e) => format!("Save failed: {e}").into(),
                    None => {
                        if let Some(project) = workspace.project.as_mut() {
                            for mi in saved_masters {
                                if let Some(master) = project.masters.get_mut(mi) {
                                    master.dirty = false;
                                    master.modified_glyphs.clear();
                                    master.kerning_dirty = false;
                                }
                            }
                        }
                        format!("Saved {count} files to the server").into()
                    }
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Cmd+O: native open dialog. Directories are selectable, so a
    /// .ufo and a .glyphspackage come through the same way a
    /// .designspace does.
    pub(crate) fn open_dialog(&mut self, cx: &mut Context<'_, Self>) {
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
                        workspace.axis_sliders.clear();
                        workspace.sessions.clear();
                        workspace.active_session = 0;
                        workspace.last_editor = None;
                        workspace.project = Some(project);
                        workspace.refresh_proposal();
                        workspace.sidebar.counts = None;
                        workspace.load_error = None;
                        workspace.mode = Mode::Grid;
                        workspace.selected = None;
                        workspace.status_note = None;
                        workspace.sidebar.search_query.clear();
                        workspace.rebuild_text_models();
                        workspace.start_watching(cx);
                    }
                    Err(e) => workspace.load_error = Some(e.into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// Find the fontc compiler: PATH first, then the default cargo
/// install location, because an app launched from the Dock does not
/// inherit a shell PATH.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn fontc_binary() -> Option<PathBuf> {
    if std::process::Command::new("fontc")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
    {
        return Some(PathBuf::from("fontc"));
    }
    let home = std::env::var_os("HOME")?;
    let cargo_bin = PathBuf::from(home).join(".cargo/bin/fontc");
    cargo_bin.exists().then_some(cargo_bin)
}

/// The designspace a bare `cargo run` opens: Virtua Grotesk from a checkout beside this repository.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn default_font_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../virtua-grotesk/sources/VirtuaGrotesk.designspace")
}
