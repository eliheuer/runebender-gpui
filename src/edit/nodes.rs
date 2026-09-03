// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Nodes in the panel: a `.nodes.json` file as rows, run through core.
//!
//! Core owns the file, the registry, and the runner
//! (`runebender_core::document::nodes` and `nodes_run`). This shell
//! finds the files beside the font, shows a chosen one in run order,
//! runs it on a background thread, and redraws each row as core
//! reports it. Nothing here knows a node type by name; the rows come
//! from the registry, which comes from core and from `font-ml tasks
//! --json`.
//!
//! The canvas that draws the same file as boxes and wires comes
//! later, on the Nodes tab; this list is what runs until then, and
//! what stays when a window is too narrow for a canvas.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::PathBuf;
use crate::Workspace;
use gpui::SharedString;
use runebender_core::document::nodes::{NodeGraph, Problem, Registry};
use runebender_core::document::nodes_run::{Event, Status};

#[cfg(not(target_family = "wasm"))]
use gpui::Context;

/// How one row looks between runs.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RowState {
    /// Not run yet this session.
    Waiting,
    /// Running; the text is the tool's progress, when it has any.
    Running(Option<SharedString>),
    /// Ended as core said.
    Done(Status, Option<SharedString>),
}

/// A file open in the panel.
#[derive(Debug, Clone)]
pub(crate) struct GraphState {
    /// The file.
    pub(crate) path: PathBuf,
    /// Its contents.
    pub(crate) graph: NodeGraph,
    /// Every node type the file may use.
    pub(crate) registry: Registry,
    /// Node ids in run order, when the file has one.
    pub(crate) order: Vec<u32>,
    /// What stops it running. Empty means it runs.
    pub(crate) problems: Vec<Problem>,
    /// Per node, by id.
    pub(crate) rows: BTreeMap<u32, RowState>,
    /// Whether a run is going.
    pub(crate) running: bool,
}

impl GraphState {
    /// Reads a file against a registry.
    pub(crate) fn open(path: &std::path::Path, registry: Registry) -> Result<Self, String> {
        let graph = NodeGraph::load(path)?;
        let problems = graph.validate(&registry);
        let order = graph.order().unwrap_or_default();
        let rows = graph
            .nodes
            .iter()
            .map(|n| (n.id, RowState::Waiting))
            .collect();
        Ok(Self {
            path: path.to_path_buf(),
            graph,
            registry,
            order,
            problems,
            rows,
            running: false,
        })
    }

    /// The title a row shows: the type's title, or its name when the
    /// registry does not know it.
    pub(crate) fn title(&self, id: u32) -> SharedString {
        let Some(node) = self.graph.node(id) else {
            return SharedString::default();
        };
        match self.registry.get(&node.type_name) {
            Some(t) => t.title.clone().into(),
            None => node.type_name.clone().into(),
        }
    }

    /// What a row was given by hand, one line: `name=value ...`.
    pub(crate) fn values_line(&self, id: u32) -> Option<SharedString> {
        let node = self.graph.node(id)?;
        if node.values.is_empty() {
            return None;
        }
        let parts: Vec<String> = node
            .values
            .iter()
            .map(|(k, v)| match v {
                serde_json::Value::String(s) => format!("{k} {s}"),
                other => format!("{k} {other}"),
            })
            .collect();
        Some(parts.join(" · ").into())
    }
}

/// `bolden.nodes.json` as `bolden`.
pub(crate) fn file_label(path: &std::path::Path) -> SharedString {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.trim_end_matches(".nodes.json").to_string())
        .unwrap_or_default()
        .into()
}

impl Workspace {
    /// The registry: core's types plus what font-ml declared, when it
    /// has answered.
    pub(crate) fn node_registry(&self) -> Registry {
        let mut registry = Registry::core();
        if let Some(json) = &self.models.tasks_json {
            registry.add_tool("font-ml", json);
        }
        registry
    }

    /// Every `.nodes.json` beside the open font: in the font's
    /// directory, its `nodes` subdirectory, and the same two one level
    /// up, which is where a family's designspace keeps them. Sorted.
    pub(crate) fn scan_nodes_files(&mut self) {
        let mut found: Vec<PathBuf> = Vec::new();
        let Some(project) = self.project.as_ref() else {
            self.models.graph_files = found;
            return;
        };
        let source = project.active_font().source_path.clone();
        let mut roots: Vec<PathBuf> = Vec::new();
        if let Some(dir) = source.parent() {
            roots.push(dir.to_path_buf());
            roots.push(dir.join("nodes"));
            if let Some(up) = dir.parent() {
                roots.push(up.to_path_buf());
                roots.push(up.join("nodes"));
            }
        }
        for root in roots {
            let Ok(entries) = std::fs::read_dir(&root) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.ends_with(".nodes.json") && !found.contains(&path) {
                    found.push(path);
                }
            }
        }
        found.sort();
        self.models.graph_files = found;
    }

    /// Opens a file in the panel. A file that does not validate still
    /// opens, with its problems listed, so it can be fixed.
    pub(crate) fn open_nodes_file(&mut self, path: &std::path::Path) {
        match GraphState::open(path, self.node_registry()) {
            Ok(state) => {
                let n = state.problems.len();
                self.models.graph = Some(state);
                self.status_note = Some(if n == 0 {
                    format!("Opened {}", file_label(path)).into()
                } else {
                    format!("{}: {n} problems", file_label(path)).into()
                });
            }
            Err(e) => self.status_note = Some(e.into()),
        }
    }

    /// Closes the file in the panel.
    pub(crate) fn close_nodes_file(&mut self) {
        self.models.graph = None;
    }

    /// Runs the open file over the active master and the selection,
    /// on a background thread. Rows redraw as core reports; when the
    /// run ends, masters that changed on disk are reloaded and the
    /// proposals list refreshed.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn run_nodes(&mut self, cx: &mut Context<'_, Self>) {
        let (running, problem, graph, registry, path) = {
            let Some(state) = self.models.graph.as_ref() else {
                return;
            };
            (
                state.running,
                state.problems.first().cloned(),
                state.graph.clone(),
                state.registry.clone(),
                state.path.clone(),
            )
        };
        if running {
            self.status_note = Some("Already running".into());
            return;
        }
        if let Some(p) = problem {
            self.status_note = Some(format!("{p}").into());
            return;
        }
        if self.models.busy.is_some() {
            self.status_note = Some("A model is already running".into());
            return;
        }
        // Core reads the UFO on disk, so what is on disk has to be
        // what is on screen.
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
        let font = project
            .export_source
            .clone()
            .unwrap_or_else(|| project.active_font().source_path.clone());
        let master = project
            .master_names
            .get(project.active)
            .map(|m| m.to_string());
        let glyphs = self.selection_names();
        let mut tools = BTreeMap::new();
        if let Some(font_ml) = self.models.binary.clone() {
            tools.insert("font-ml".to_string(), font_ml);
        }
        let models_dir = Self::models_dir();
        if let Some(s) = self.models.graph.as_mut() {
            s.running = true;
            for row in s.rows.values_mut() {
                *row = RowState::Waiting;
            }
        }
        self.models.busy = Some(format!("Running {}…", file_label(&path)).into());

        // Events land in a list the foreground drains a few times a
        // second; the report lands in `finished` at the end.
        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let finished: Arc<Mutex<Option<runebender_core::document::nodes_run::RunReport>>> =
            Arc::new(Mutex::new(None));
        cx.background_executor()
            .spawn({
                let events = events.clone();
                let finished = finished.clone();
                async move {
                    let mut on_event = |e: Event| {
                        events.lock().unwrap_or_else(|e| e.into_inner()).push(e);
                    };
                    let mut ctx = runebender_core::document::nodes_run::RunContext {
                        font: &font,
                        master: master.as_deref(),
                        glyphs,
                        tools,
                        models_dir,
                        force: false,
                        cache: Some(runebender_core::document::nodes_run::cache_path(&path)),
                        on_event: &mut on_event,
                    };
                    let report =
                        runebender_core::document::nodes_run::run(&graph, &registry, &mut ctx);
                    *finished.lock().unwrap_or_else(|e| e.into_inner()) = Some(report);
                }
            })
            .detach();
        cx.spawn(async move |this, cx| {
            let report = loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(150))
                    .await;
                let batch: Vec<Event> =
                    std::mem::take(&mut *events.lock().unwrap_or_else(|e| e.into_inner()));
                if !batch.is_empty() {
                    this.update(cx, |workspace, cx| {
                        for event in batch {
                            workspace.node_event(event);
                        }
                        cx.notify();
                    })
                    .ok();
                }
                if let Some(report) = finished.lock().unwrap_or_else(|e| e.into_inner()).take() {
                    break report;
                }
            };
            this.update(cx, |workspace, cx| {
                workspace.nodes_finished(&report);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// One row changes as core reports.
    fn node_event(&mut self, event: Event) {
        let Some(state) = self.models.graph.as_mut() else {
            return;
        };
        match event {
            Event::Start { id, .. } => {
                state.rows.insert(id, RowState::Running(None));
            }
            Event::Progress {
                id,
                done,
                total,
                label,
            } => {
                state.rows.insert(
                    id,
                    RowState::Running(Some(format!("{done}/{total} {label}").into())),
                );
                self.models.busy =
                    Some(format!("{}: {done}/{total} {label}", state.title(id)).into());
            }
            Event::End {
                id,
                status,
                seconds,
                error,
            } => {
                let note = match (status, error) {
                    (_, Some(e)) => Some(e.into()),
                    (Status::Ran, None) => Some(format!("{seconds:.1}s").into()),
                    _ => None,
                };
                state.rows.insert(id, RowState::Done(status, note));
            }
        }
    }

    /// The run ended. Install changes the font on disk, so masters
    /// that did not change in the editor are re-read.
    #[cfg(not(target_family = "wasm"))]
    fn nodes_finished(&mut self, report: &runebender_core::document::nodes_run::RunReport) {
        self.models.busy = None;
        let installed = report
            .nodes
            .iter()
            .any(|n| n.type_name == "core.install" && n.status == Status::Ran);
        if let Some(state) = self.models.graph.as_mut() {
            state.running = false;
            for n in &report.nodes {
                let note: Option<SharedString> = match n.status {
                    Status::Failed => n
                        .report
                        .get("error")
                        .and_then(|e| e.as_str())
                        .map(|e| e.to_string().into()),
                    Status::Ran => Some(summary_line(n).into()),
                    Status::Skipped => Some("unchanged".into()),
                    Status::Blocked => None,
                };
                state.rows.insert(n.id, RowState::Done(n.status, note));
            }
        }
        if installed {
            self.reload_from_disk();
        }
        self.refresh_proposal();
        let failed = report
            .nodes
            .iter()
            .filter(|n| n.status == Status::Failed)
            .count();
        self.status_note = Some(if report.ok {
            let ran = report
                .nodes
                .iter()
                .filter(|n| n.status == Status::Ran)
                .count();
            let skipped = report.nodes.len() - ran;
            format!("Ran {ran} nodes, {skipped} unchanged").into()
        } else {
            format!("{failed} nodes failed").into()
        });
    }
}

/// One line for a node that ran: what it gave, from its outputs and
/// report.
fn summary_line(n: &runebender_core::document::nodes_run::NodeResult) -> String {
    use runebender_core::document::nodes_run::RunValue;
    let mut parts: Vec<String> = Vec::new();
    for (name, value) in &n.outputs {
        match value {
            RunValue::Layer { name: layer, .. } => {
                parts.push(
                    layer
                        .trim_start_matches("com.runebender.proposal.")
                        .to_string(),
                );
            }
            RunValue::Rows { rows } => parts.push(format!("{} {name}", rows.len())),
            RunValue::Path { path } => {
                parts.push(file_label(path).to_string());
            }
            RunValue::Glyphs { names } if !names.is_empty() => {
                parts.push(format!("{} glyphs", names.len()));
            }
            _ => {}
        }
    }
    if let (Some(model), Some(unchanged)) = (
        n.report.get("model").and_then(|v| v.as_f64()),
        n.report.get("unchanged").and_then(|v| v.as_f64()),
    ) {
        parts.push(format!("model {model:.1} vs unchanged {unchanged:.1}"));
    }
    if parts.is_empty() {
        format!("{:.1}s", n.seconds)
    } else {
        parts.join(" · ")
    }
}

/// In the browser there is no thread to run on.
#[cfg(target_family = "wasm")]
impl Workspace {
    pub(crate) fn run_nodes(&mut self, _cx: &mut gpui::Context<'_, Self>) {
        self.status_note = Some("Nodes run in the desktop app".into());
    }
}
