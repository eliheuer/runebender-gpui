// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! The world outside the window: files, watching, the browser host.
//!
//! Build scripts and export paths, watching sources for changes made
//! by other tools, reloading, and the web host's fetch and save.

use super::*;

impl Workspace {
    /// The repo's own Google Fonts build script above the source
    /// (build-fontc.sh preferred, then build.sh), with the directory
    /// to run it from. A repo pipeline carries the gftools fixes,
    /// STAT, and statics that a raw compile does not.
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

    /// Watch every master's UFO directory; external changes reload
    /// the affected masters (in-memory edits are never clobbered:
    /// dirty masters skip the reload with a status note). Our own
    /// saves are suppressed via the last_save timestamp.
    #[cfg(target_family = "wasm")]
    pub(crate) fn start_watching(&mut self, _cx: &mut Context<Self>) {
        // No filesystem on the web: live reload will ride the host
        // data layer instead.
    }

    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn start_watching(&mut self, cx: &mut Context<Self>) {
        use futures::StreamExt;
        self._watcher = None;
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
        let mut watcher =
            match notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                if res.is_ok() {
                    let _ = tx.unbounded_send(());
                }
            }) {
                Ok(w) => w,
                Err(_) => return,
            };
        for master in &project.masters {
            let _ = notify::Watcher::watch(
                &mut watcher,
                &master.source_path,
                notify::RecursiveMode::Recursive,
            );
        }
        self._watcher = Some(watcher);
        let last_save = self.last_save.clone();
        cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                // Debounce: drain everything arriving in the next
                // half second into one reload.
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(500))
                    .await;
                while rx.try_recv().is_ok() {}
                if last_save.lock().unwrap().elapsed() < std::time::Duration::from_secs(2) {
                    continue;
                }
                if this
                    .update(cx, |workspace, cx| {
                        workspace.reload_from_disk();
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// Re-read every clean master from disk, keeping the open glyph.
    pub(crate) fn reload_from_disk(&mut self) {
        self.sidebar_counts = None;
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

    /// Connect to the workspace server named by ?server= and load
    /// its fonts (web builds).
    #[cfg(target_family = "wasm")]
    pub(crate) fn connect_web_host(&mut self, base: String, cx: &mut Context<Self>) {
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
                        workspace.sidebar_counts = None;
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
    /// modified glifs and kerning, each PUT with its If-Match ETag.
    #[cfg(target_family = "wasm")]
    pub(crate) fn save_to_web_host(&mut self, cx: &mut Context<Self>) {
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
                match runebender_core::font_memory::glif_bytes(glyph) {
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
                match runebender_core::font_memory::kerning_plist_bytes(&master.font) {
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
        let etags: std::collections::HashMap<String, String> = host.etags.clone();
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
    pub(crate) fn open_dialog(&mut self, cx: &mut Context<Self>) {
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
                        workspace.sidebar_counts = None;
                        workspace.load_error = None;
                        workspace.mode = Mode::Grid;
                        workspace.selected = None;
                        workspace.status_note = None;
                        workspace.search_query.clear();
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
