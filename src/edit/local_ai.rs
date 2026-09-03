// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The local models panel: finding models on disk and running one.
//!
//! The model runtime is `font-ml`, a separate program. This shell
//! never links it: it finds the binary, runs it over the UFO on disk,
//! and reads the proposal layer it leaves behind. That keeps candle
//! and its build out of the editor, and it means the command line,
//! an agent, and this panel all get the same answer from the same
//! tool. What the shell owns is the seam: save first, run, pull the
//! proposal layer into the open font, and hand it to core to install
//! or discard.

use std::sync::{Arc, Mutex};

use crate::PathBuf;
use crate::Workspace;
use gpui::SharedString;
use runebender_core::document::proposal::{self, ProposalSummary};

#[cfg(not(target_family = "wasm"))]
use crate::CONFIG;
#[cfg(not(target_family = "wasm"))]
use gpui::Context;

/// One task as `font-ml tasks --json` describes it. The shell keeps
/// only what a row needs; the full spec and its schema stay with the
/// tool. No task name is written in this crate: a task font-ml adds
/// shows up here without a shell change.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub(crate) struct TaskRow {
    /// The name `font-ml run` takes.
    pub(crate) name: String,
    /// One line for the button.
    pub(crate) title: String,
    /// A few lines for a tooltip.
    #[serde(default)]
    pub(crate) help: String,
    /// Whether the installed font-ml runs it.
    #[serde(default)]
    pub(crate) implemented: bool,
    /// What it takes, by name and kind.
    #[serde(default)]
    pub(crate) inputs: Vec<TaskInput>,
}

/// One input of a task, by name and kind.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub(crate) struct TaskInput {
    /// The flag name.
    pub(crate) name: String,
    /// The kind, as font-ml names it: source, model, glyph, glyphs,
    /// number, flag, text.
    pub(crate) kind: String,
}

impl TaskRow {
    /// The rows in a `font-ml tasks --json` answer, in the tool's
    /// order. A malformed answer is an empty list, not a crash.
    pub(crate) fn parse(json: &str) -> Vec<Self> {
        let value: serde_json::Value = match serde_json::from_str(json) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        value
            .get("tasks")
            .and_then(|t| serde_json::from_value(t.clone()).ok())
            .unwrap_or_default()
    }

    /// Whether the task takes a set of glyphs, so "every drawn glyph"
    /// is a call it understands.
    pub(crate) fn takes_glyphs(&self) -> bool {
        self.inputs.iter().any(|i| i.kind == "glyphs")
    }

    /// Whether the task takes one glyph, so "this glyph" is a call.
    pub(crate) fn takes_glyph(&self) -> bool {
        self.inputs
            .iter()
            .any(|i| i.kind == "glyph" || i.kind == "glyphs")
    }
}

/// Run one font-ml task to completion on the calling thread, feeding
/// progress lines into `progress` and parking the child in `job` so
/// it can be killed. Returns the JSON object font-ml printed last.
#[cfg(not(target_family = "wasm"))]
#[allow(
    clippy::too_many_arguments,
    reason = "one call, one process; a struct would only rename the arguments"
)]
fn run_font_ml(
    font_ml: &std::path::Path,
    task: &str,
    model: &std::path::Path,
    source: &std::path::Path,
    glyph: Option<&str>,
    strength: f64,
    reference: Option<&std::path::Path>,
    progress: &Mutex<Option<(usize, usize, String)>>,
    job: &Mutex<Option<std::process::Child>>,
) -> Result<serde_json::Value, String> {
    use std::io::BufRead as _;
    let mut cmd = std::process::Command::new(font_ml);
    cmd.arg("run")
        .arg(task)
        .arg("--model")
        .arg(model)
        .arg("--source")
        .arg(source)
        .arg("--strength")
        .arg(format!("{strength}"))
        .arg("--write")
        .arg("--json")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    match glyph {
        Some(name) => {
            cmd.arg("--glyph").arg(name);
        }
        None => {
            cmd.arg("--all");
        }
    }
    if let Some(reference) = reference {
        cmd.arg("--reference").arg(reference);
    }
    let mut child = cmd.spawn().map_err(|e| format!("{e}"))?;
    let stderr = child.stderr.take().ok_or("no stderr")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    *job.lock().unwrap_or_else(|e| e.into_inner()) = Some(child);
    // stdout is one JSON object at the end and stays small; read it on
    // a thread so a full pipe can never stall the run.
    let stdout_reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = std::io::Read::read_to_string(&mut std::io::BufReader::new(stdout), &mut text);
        text
    });
    let mut last_error = String::new();
    for line in std::io::BufReader::new(stderr)
        .lines()
        .map_while(Result::ok)
    {
        match parse_progress(&line) {
            Some((done, total, glyph)) => {
                *progress.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some((done, total, glyph.to_string()));
            }
            None if !line.trim().is_empty() => last_error = line,
            None => {}
        }
    }
    let status = {
        let mut slot = job.lock().unwrap_or_else(|e| e.into_inner());
        match slot.as_mut() {
            Some(child) => child.wait().map_err(|e| format!("{e}"))?,
            None => return Err("cancelled".into()),
        }
    };
    let stdout = stdout_reader.join().unwrap_or_default();
    let report: serde_json::Value = stdout
        .lines()
        .rev()
        .find_map(|l| serde_json::from_str(l).ok())
        .unwrap_or(serde_json::Value::Null);
    if status.success() {
        Ok(report)
    } else if status.code().is_none() {
        Err("cancelled".into())
    } else {
        Err(report
            .get("error")
            .and_then(|e| e.as_str())
            .map(str::to_string)
            .unwrap_or(last_error))
    }
}

/// A progress line as font-ml prints it: `progress <done>/<total> <glyph>`.
pub(crate) fn parse_progress(line: &str) -> Option<(usize, usize, &str)> {
    let rest = line.strip_prefix("progress ")?;
    let (count, glyph) = rest.split_once(' ')?;
    let (done, total) = count.split_once('/')?;
    Some((done.parse().ok()?, total.parse().ok()?, glyph.trim()))
}

impl Workspace {
    /// Where a model is looked for when nobody points at one.
    ///
    /// `$RUNEBENDER_MODELS` wins, then the config file, then
    /// `~/.runebender/models`. The variable wins because a setting
    /// for one run has to beat a setting meant for every run.
    ///
    /// A model is a directory holding `config.json`, so dropping one
    /// in is the whole installation step: no rebuild, no account, no
    /// file picker.
    pub(crate) fn models_dir() -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os("RUNEBENDER_MODELS") {
            return Some(PathBuf::from(dir));
        }
        #[cfg(not(target_family = "wasm"))]
        if let Some(dir) = CONFIG.get().and_then(|c| c.models.clone()) {
            return Some(dir);
        }
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".runebender/models"))
    }

    /// Every model directory under `models_dir`, by name.
    ///
    /// Sorted, so the list does not reshuffle between launches on
    /// whatever order the filesystem hands back.
    pub(crate) fn installed_models() -> Vec<(String, PathBuf)> {
        let Some(root) = Self::models_dir() else {
            return Vec::new();
        };
        let Ok(entries) = std::fs::read_dir(&root) else {
            return Vec::new();
        };
        let mut found: Vec<(String, PathBuf)> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join("config.json").is_file())
            .filter_map(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| (n.to_string(), p.clone()))
            })
            .collect();
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found
    }

    /// Where the font-ml binary is: `$RUNEBENDER_FONT_ML`, then PATH,
    /// then `~/.cargo/bin`. None means it is not installed.
    pub(crate) fn font_ml_binary() -> Option<PathBuf> {
        if let Some(t) = std::env::var_os("RUNEBENDER_FONT_ML").filter(|t| !t.is_empty()) {
            return Some(PathBuf::from(t));
        }
        if let Some(found) = std::env::var_os("PATH").and_then(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join("font-ml"))
                .find(|c| c.is_file())
        }) {
            return Some(found);
        }
        let home = std::env::var_os("HOME")?;
        let cargo_bin = PathBuf::from(home).join(".cargo/bin/font-ml");
        cargo_bin.is_file().then_some(cargo_bin)
    }

    /// Remember a model directory and describe it from its
    /// `config.json`, without loading the weights. Loading is
    /// font-ml's job, at run time.
    pub(crate) fn load_model(&mut self, dir: &std::path::Path) {
        let config = match std::fs::read_to_string(dir.join("config.json")) {
            Ok(text) => text,
            Err(e) => {
                self.status_note = Some(format!("Model: {e}").into());
                return;
            }
        };
        let parsed: serde_json::Value = match serde_json::from_str(&config) {
            Ok(v) => v,
            Err(e) => {
                self.status_note = Some(format!("Model: config.json: {e}").into());
                return;
            }
        };
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "model".into());
        let kind = parsed
            .get("kind")
            .and_then(|k| k.as_str())
            .unwrap_or("outline");
        let shape = match (parsed.get("layers"), parsed.get("dims")) {
            (Some(l), Some(d)) => format!(", {l} layers × {d}"),
            _ => String::new(),
        };
        self.models.summary = Some(format!("{name}: {kind}{shape}").into());
        self.models.dir = Some(dir.to_path_buf());
        self.rescan_models();
        self.models.score = None;
        self.status_note = Some("Model chosen".into());
    }

    /// Ask font-ml what it can do, once. The rows arrive later and the
    /// panel redraws with them.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn load_tasks(&mut self, cx: &mut Context<'_, Self>) {
        if self.models.tasks_asked {
            return;
        }
        self.models.tasks_asked = true;
        // The directory scan and the PATH walk happen here, once, and
        // again when a model is chosen; never in a render.
        self.rescan_models();
        let Some(font_ml) = self.models.binary.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let text: String = cx
                .background_executor()
                .spawn(async move {
                    std::process::Command::new(&font_ml)
                        .arg("tasks")
                        .arg("--json")
                        .output()
                        .ok()
                        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                        .unwrap_or_default()
                })
                .await;
            this.update(cx, |workspace, cx| {
                workspace.models.tasks = Some(TaskRow::parse(&text));
                // The raw answer feeds core's node registry, so a node
                // file can name any task the tool declares.
                workspace.models.tasks_json = serde_json::from_str(&text).ok();
                workspace.scan_nodes_files();
                // QA hook, like RB_OPEN_GLYPH: RB_NODES=<file> opens a
                // nodes file in the panel at launch.
                if let Some(file) = std::env::var_os("RB_NODES").filter(|f| !f.is_empty()) {
                    workspace.open_nodes_file(std::path::Path::new(&file));
                    // RB_MODE=nodes starts on the canvas.
                    if std::env::var("RB_MODE").as_deref() == Ok("nodes") {
                        workspace.enter_nodes_mode();
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// In the browser there is no process to ask.
    #[cfg(target_family = "wasm")]
    pub(crate) fn load_tasks(&mut self, _cx: &mut gpui::Context<'_, Self>) {
        self.models.tasks_asked = true;
        self.models.tasks = Some(Vec::new());
        self.rescan_models();
    }

    /// Look at the disk again: the models directory and the binary.
    pub(crate) fn rescan_models(&mut self) {
        self.models.installed = Self::installed_models();
        self.models.binary = Self::font_ml_binary();
    }

    /// What the active master has waiting, from any task.
    pub(crate) fn refresh_proposal(&mut self) {
        self.models.proposals = self
            .font()
            .map(|f| proposal::list(&f.font))
            .unwrap_or_default()
            .into_iter()
            .filter(|p| !p.glyphs.is_empty())
            .collect();
    }

    /// Pull a proposal layer from the UFO on disk into the open font,
    /// replacing any earlier proposal for the task. font-ml wrote it;
    /// the in-memory font has not seen it yet.
    fn adopt_proposal_from_disk(
        &mut self,
        task: &str,
        source: &std::path::Path,
    ) -> Result<ProposalSummary, String> {
        let on_disk = norad::Font::load(source).map_err(|e| e.to_string())?;
        let layer_name = proposal::layer_name(task);
        let glyphs: Vec<norad::Glyph> = on_disk
            .layers
            .get(&layer_name)
            .map(|l| l.iter().cloned().collect())
            .unwrap_or_default();
        if glyphs.is_empty() {
            return Err(format!("font-ml left no {layer_name} layer"));
        }
        let font = self.font_mut().ok_or("no font open")?;
        font.font.layers.remove(&layer_name);
        let summary = proposal::write(&mut font.font, task, glyphs).map_err(|e| e.to_string())?;
        font.dirty = true;
        Ok(summary)
    }

    /// Install a waiting proposal: one undo step per glyph.
    pub(crate) fn install_proposal(&mut self, task: &str, only: Option<Vec<String>>) {
        let result = self
            .font_mut()
            .map(|f| f.install_proposal(task, only.as_deref(), true));
        match result {
            Some(Ok(done)) => {
                self.journal(
                    "install proposal",
                    None,
                    Some(format!(
                        "{}: {} installed, {} skipped",
                        done.task,
                        done.installed.len(),
                        done.skipped.len()
                    )),
                );
                self.status_note = Some(
                    format!(
                        "Installed {} glyphs from {}{}. Undo takes them back one at a time.",
                        done.installed.len(),
                        done.task,
                        if done.skipped.is_empty() {
                            String::new()
                        } else {
                            format!(", {} skipped", done.skipped.len())
                        }
                    )
                    .into(),
                );
                if let (Some(index), Some(font)) = (self.selected, self.font_mut()) {
                    font.rebuild_entry(index);
                }
            }
            Some(Err(e)) => self.status_note = Some(format!("{e}").into()),
            None => {}
        }
        self.refresh_proposal();
    }

    /// Drop a waiting proposal without installing it.
    pub(crate) fn discard_proposal(&mut self, task: &str) {
        let result = self.font_mut().map(|f| f.discard_proposal(task));
        match result {
            Some(Ok(n)) => self.status_note = Some(format!("Discarded {n} proposed glyphs").into()),
            Some(Err(e)) => self.status_note = Some(format!("{e}").into()),
            None => {}
        }
        self.refresh_proposal();
    }

    /// Run the task with font-ml over the open master. `glyph` names
    /// one glyph, which is installed as soon as it arrives (undo to
    /// reject); None runs every drawn glyph and leaves the result
    /// waiting in the panel to install or discard.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn run_task(
        &mut self,
        task: &str,
        glyph: Option<usize>,
        cx: &mut Context<'_, Self>,
    ) {
        let task = task.to_string();
        let Some(model) = self.models.dir.clone() else {
            self.status_note = Some("Choose a model first".into());
            return;
        };
        let Some(font_ml) = self.models.binary.clone() else {
            self.status_note = Some(
                "font-ml not found: cargo install --git https://github.com/eliheuer/font-ml, \
                 or set RUNEBENDER_FONT_ML"
                    .into(),
            );
            return;
        };
        if self.models.busy.is_some() {
            self.status_note = Some("A model is already running".into());
            return;
        }
        // font-ml reads the UFO on disk, so what is on disk has to be
        // what is on screen.
        if self.font().is_some_and(|f| f.dirty) {
            self.command_save(cx);
        }
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let active = project.active_font();
        let source = active.source_path.clone();
        if !source.is_dir() {
            self.status_note = Some("Save the font before running a model".into());
            return;
        }
        let glyph_name = glyph.and_then(|i| active.glyphs.get(i).map(|g| g.name.to_string()));
        if glyph.is_some() && glyph_name.is_none() {
            return;
        }
        // The other master, where it says what weight it carries.
        let reference = (project.masters.len() > 1).then(|| {
            let other = if project.active == 0 {
                project.masters.len() - 1
            } else {
                0
            };
            project.masters[other].source_path.clone()
        });
        let strength = self.models.strength;
        let label: SharedString = match &glyph_name {
            Some(name) => format!("Running {task} on {name}…").into(),
            None => format!("Running {task} on every glyph…").into(),
        };
        self.models.busy = Some(label.clone());
        self.status_note = Some(label);

        // The process runs on a background thread. Its progress lines
        // land in `progress`, its handle in `job` so Cancel can kill
        // it, and its result in `finished`. The foreground polls a few
        // times a second and redraws the count.
        let progress: Arc<Mutex<Option<(usize, usize, String)>>> = Arc::new(Mutex::new(None));
        let job: Arc<Mutex<Option<std::process::Child>>> = Arc::new(Mutex::new(None));
        let finished: Arc<Mutex<Option<Result<serde_json::Value, String>>>> =
            Arc::new(Mutex::new(None));
        self.models.job = Some(job.clone());
        cx.background_executor()
            .spawn({
                let source = source.clone();
                let task = task.clone();
                let progress = progress.clone();
                let job = job.clone();
                let finished = finished.clone();
                async move {
                    let result = run_font_ml(
                        &font_ml,
                        &task,
                        &model,
                        &source,
                        glyph_name.as_deref(),
                        strength,
                        reference.as_deref(),
                        &progress,
                        &job,
                    );
                    *finished.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
                }
            })
            .detach();
        cx.spawn(async move |this, cx| {
            let result = loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(200))
                    .await;
                if let Some(result) = finished.lock().unwrap_or_else(|e| e.into_inner()).take() {
                    break result;
                }
                let seen = progress.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if let Some((done, total, glyph)) = seen {
                    let task = task.clone();
                    this.update(cx, |workspace, cx| {
                        workspace.models.busy =
                            Some(format!("{task}: {done}/{total} ({glyph})").into());
                        cx.notify();
                    })
                    .ok();
                }
            };
            this.update(cx, |workspace, cx| {
                workspace.models.busy = None;
                workspace.models.job = None;
                match result {
                    Ok(report) => workspace.task_finished(&task, &source, glyph, &report),
                    Err(e) => workspace.status_note = Some(format!("font-ml: {e}").into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Stop the running task. font-ml writes its proposal only at the
    /// end, so a killed run leaves nothing behind.
    pub(crate) fn cancel_task(&mut self) {
        let Some(job) = self.models.job.take() else {
            return;
        };
        if let Some(child) = job.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let _ = child.kill();
        }
        self.status_note = Some("Cancelled".into());
    }

    /// What happens when font-ml comes back: the proposal layer is
    /// adopted from disk, and a single glyph is installed at once.
    #[cfg(not(target_family = "wasm"))]
    fn task_finished(
        &mut self,
        task: &str,
        source: &std::path::Path,
        glyph: Option<usize>,
        report: &serde_json::Value,
    ) {
        let summary = match self.adopt_proposal_from_disk(task, source) {
            Ok(s) => s,
            Err(e) => {
                self.status_note = Some(format!("font-ml: {e}").into());
                return;
            }
        };
        match glyph {
            Some(index) => {
                let name = self
                    .font()
                    .and_then(|f| f.glyphs.get(index).map(|g| g.name.to_string()));
                let moved = report.get("moved").and_then(|v| v.as_u64()).unwrap_or(0);
                let points = report.get("points").and_then(|v| v.as_u64()).unwrap_or(0);
                let advance = report
                    .get("advance_delta")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                self.install_proposal(task, name.clone().map(|n| vec![n]));
                self.editor.selected.clear();
                // A model's output is the edit most worth having a
                // record of: it is the one nobody watched being made.
                self.journal(
                    "run model task",
                    Some(index),
                    Some(format!(
                        "{moved}/{points} points moved, advance {advance:+}"
                    )),
                );
                self.status_note = Some(
                    format!(
                        "{task} on {}: {moved}/{points} points moved, advance {advance:+}. \
                         Undo to reject.",
                        name.unwrap_or_default()
                    )
                    .into(),
                );
            }
            None => {
                self.status_note = Some(
                    format!(
                        "{} glyphs proposed ({} keep structure). Install or discard in the panel.",
                        summary.glyphs.len(),
                        summary.compatible.len()
                    )
                    .into(),
                );
                self.refresh_proposal();
            }
        }
    }

    /// In the browser there is no process to run.
    #[cfg(target_family = "wasm")]
    pub(crate) fn run_task(
        &mut self,
        _task: &str,
        _glyph: Option<usize>,
        _cx: &mut gpui::Context<'_, Self>,
    ) {
        self.status_note = Some("Local models run in the desktop app".into());
    }
}
