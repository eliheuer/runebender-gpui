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

use crate::Mode;
use crate::PathBuf;
use crate::Workspace;
use gpui::SharedString;
use runebender_core::document::nodes::{NodeGraph, Problem, Registry};
use runebender_core::document::nodes_run::{Event, Status};
use std::path::Path;

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
    pub(crate) fn open(path: &Path, registry: Registry) -> Result<Self, String> {
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
            .map(|(k, v)| format!("{k} {}", value_text(v)))
            .collect();
        Some(parts.join(" · ").into())
    }
}

/// `bolden.nodes.json` as `bolden`.
pub(crate) fn file_label(path: &Path) -> SharedString {
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
    pub(crate) fn open_nodes_file(&mut self, path: &Path) {
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

// ---- the canvas: layout, hit-testing, and the drag ----

/// Box width, in canvas units.
pub(crate) const NODE_W: f64 = 168.0;
/// Header band height.
pub(crate) const HEADER_H: f64 = 22.0;
/// One port row.
pub(crate) const ROW_H: f64 = 18.0;
/// Port dot radius.
pub(crate) const PORT_R: f64 = 4.5;
/// Inner padding.
pub(crate) const PAD: f64 = 6.0;

/// One port as laid out: where its dot sits, in canvas units.
#[derive(Debug, Clone)]
pub(crate) struct PortBox {
    /// The port name.
    pub(crate) name: String,
    /// What it carries.
    pub(crate) kind: runebender_core::document::nodes::Kind,
    /// The dot's centre.
    pub(crate) at: kurbo::Point,
    /// Which row of the box it sits on, from the top.
    pub(crate) row: usize,
    /// A wire is on it.
    pub(crate) linked: bool,
    /// What was typed into it, shown beside the name.
    pub(crate) value: Option<String>,
}

/// One node as laid out.
#[derive(Debug, Clone)]
pub(crate) struct NodeBox {
    /// The node.
    pub(crate) id: u32,
    /// The type's title, in the header.
    pub(crate) title: String,
    /// The box, in canvas units.
    pub(crate) rect: kurbo::Rect,
    /// Ports down the left edge.
    pub(crate) inputs: Vec<PortBox>,
    /// Ports down the right edge.
    pub(crate) outputs: Vec<PortBox>,
}

/// The canvas state that is not the file: where the view is, what is
/// selected, what the mouse is doing.
#[derive(Debug, Clone, Default)]
pub(crate) struct NodesView {
    /// Canvas units to pixels. Nodes are Y-down in the file, the
    /// viewport is Y-up, so a node point goes in as `(x, -y)`.
    pub(crate) viewport: runebender_core::ui::editing::viewport::ViewPort,
    /// The viewport has been placed once.
    pub(crate) fitted: bool,
    /// The selected node.
    pub(crate) selected: Option<u32>,
    /// The drag in progress.
    pub(crate) drag: Option<NodeDrag>,
    /// Where the paint closure records the canvas bounds, so a mouse
    /// position can be made local.
    pub(crate) bounds: Arc<Mutex<gpui::Bounds<gpui::Pixels>>>,
}

/// What a drag on the canvas is doing.
#[derive(Debug, Clone)]
pub(crate) enum NodeDrag {
    /// Moving a node: where the gesture began and the node's position
    /// then, in canvas units.
    Move {
        /// The node.
        id: u32,
        /// Where the pointer went down.
        start: kurbo::Point,
        /// The node's position then.
        origin: [f32; 2],
    },
    /// Panning: the last pointer position in window pixels.
    Pan {
        /// The last pointer position.
        last: kurbo::Point,
    },
    /// Pulling a wire from an output to wherever the pointer is, in
    /// window pixels.
    Wire {
        /// The node the wire leaves.
        from: u32,
        /// The output it leaves by.
        output: String,
        /// What it carries, so only a matching input takes it.
        kind: runebender_core::document::nodes::Kind,
        /// Where the pointer is.
        to: kurbo::Point,
    },
}

/// A canvas point to window pixels.
pub(crate) fn to_screen(
    vp: &runebender_core::ui::editing::viewport::ViewPort,
    origin: gpui::Point<gpui::Pixels>,
    p: kurbo::Point,
) -> gpui::Point<gpui::Pixels> {
    let s = vp.to_screen(kurbo::Point::new(p.x, -p.y));
    gpui::point(
        origin.x + gpui::px(crate::view::render::px32(s.x)),
        origin.y + gpui::px(crate::view::render::px32(s.y)),
    )
}

/// A window point to canvas units.
fn to_canvas(
    vp: &runebender_core::ui::editing::viewport::ViewPort,
    origin: gpui::Point<gpui::Pixels>,
    p: gpui::Point<gpui::Pixels>,
) -> kurbo::Point {
    let local = kurbo::Point::new(
        f64::from(f32::from(p.x - origin.x)),
        f64::from(f32::from(p.y - origin.y)),
    );
    let d = vp.screen_to_design(local);
    kurbo::Point::new(d.x, -d.y)
}

/// A typed value as the box shows it: a whole number without its
/// `.0`, a string bare.
pub(crate) fn value_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{f:.0}"),
            _ => n.to_string(),
        },
        other => other.to_string(),
    }
}

/// Lays out one node: header, one row per port on either side, the
/// dots on the edges.
pub(crate) fn node_box(
    state: &GraphState,
    node: &runebender_core::document::nodes::Node,
) -> NodeBox {
    let ty = state.registry.get(&node.type_name);
    let title = ty
        .map(|t| t.title.clone())
        .unwrap_or_else(|| node.type_name.clone());
    let inputs: Vec<_> = ty.map(|t| t.inputs.clone()).unwrap_or_default();
    let outputs: Vec<_> = ty.map(|t| t.outputs.clone()).unwrap_or_default();
    // Outputs take the top rows, inputs the rows under them, so a
    // long typed value never runs into an output's name.
    let rows = (inputs.len() + outputs.len()).max(1);
    let x = f64::from(node.pos[0]);
    let y = f64::from(node.pos[1]);
    let h = HEADER_H + PAD + ROW_H * rows as f64;
    let rect = kurbo::Rect::new(x, y, x + NODE_W, y + h);
    let row_y = |i: usize| y + HEADER_H + PAD / 2.0 + ROW_H * (i as f64 + 0.5);
    let first_input = outputs.len();
    let inputs = inputs
        .iter()
        .enumerate()
        .map(|(i, p)| PortBox {
            name: p.name.clone(),
            kind: p.kind,
            at: kurbo::Point::new(x, row_y(first_input + i)),
            row: first_input + i,
            linked: state.graph.link_into(node.id, &p.name).is_some(),
            value: node.values.get(&p.name).map(value_text),
        })
        .collect();
    let outputs = outputs
        .iter()
        .enumerate()
        .map(|(i, p)| PortBox {
            name: p.name.clone(),
            kind: p.kind,
            at: kurbo::Point::new(x + NODE_W, row_y(i)),
            row: i,
            linked: state
                .graph
                .links
                .iter()
                .any(|l| l.from() == node.id && l.output() == p.name),
            value: None,
        })
        .collect();
    NodeBox {
        id: node.id,
        title,
        rect,
        inputs,
        outputs,
    }
}

/// What is under a canvas point.
enum Hit {
    /// A node's box.
    Node(u32),
    /// An input dot: node, port, kind.
    Input(u32, String, runebender_core::document::nodes::Kind),
    /// An output dot: node, port, kind.
    Output(u32, String, runebender_core::document::nodes::Kind),
    /// Nothing.
    Empty,
}

impl Workspace {
    /// The canvas origin in window pixels, from the last paint.
    fn nodes_origin(&self) -> gpui::Point<gpui::Pixels> {
        self.models
            .graph_view
            .bounds
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .origin
    }

    /// What sits under a canvas point, top box first.
    fn nodes_hit(&self, at: kurbo::Point) -> Hit {
        let Some(state) = self.models.graph.as_ref() else {
            return Hit::Empty;
        };
        let reach = PORT_R * 2.0;
        // Later nodes draw on top, so they are hit first.
        for node in state.graph.nodes.iter().rev() {
            let nb = node_box(state, node);
            for p in &nb.outputs {
                if (p.at - at).hypot() <= reach {
                    return Hit::Output(node.id, p.name.clone(), p.kind);
                }
            }
            for p in &nb.inputs {
                if (p.at - at).hypot() <= reach {
                    return Hit::Input(node.id, p.name.clone(), p.kind);
                }
            }
            if nb.rect.contains(at) {
                return Hit::Node(node.id);
            }
        }
        Hit::Empty
    }

    /// Opens the canvas. With no file open, the first one beside the
    /// font opens, or a new empty one waits to be saved.
    pub(crate) fn enter_nodes_mode(&mut self) {
        if self.project.is_none() {
            return;
        }
        if let Mode::Editor(index) = self.mode {
            self.last_editor = Some(index);
        }
        if self.models.graph.is_none() {
            self.scan_nodes_files();
            match self.models.graph_files.first().cloned() {
                Some(file) => self.open_nodes_file(&file),
                None => {
                    let dir = self
                        .project
                        .as_ref()
                        .and_then(|p| p.active_font().source_path.parent().map(Path::to_path_buf))
                        .unwrap_or_default();
                    let path = dir.join("nodes").join("untitled.nodes.json");
                    self.models.graph = Some(GraphState {
                        path,
                        graph: NodeGraph::default(),
                        registry: self.node_registry(),
                        order: Vec::new(),
                        problems: Vec::new(),
                        rows: BTreeMap::new(),
                        running: false,
                    });
                }
            }
        }
        self.mode = Mode::Nodes;
        self.status_note = None;
    }

    /// After an edit: problems, order and rows follow the graph.
    fn nodes_revalidate(&mut self) {
        let Some(state) = self.models.graph.as_mut() else {
            return;
        };
        state.problems = state.graph.validate(&state.registry);
        state.order = state.graph.order().unwrap_or_default();
        let ids: Vec<u32> = state.graph.nodes.iter().map(|n| n.id).collect();
        state.rows.retain(|id, _| ids.contains(id));
        for id in ids {
            state.rows.entry(id).or_insert(RowState::Waiting);
        }
    }

    /// Adds a node of `type_name` at the middle of the view.
    pub(crate) fn nodes_add(&mut self, type_name: &str) {
        let origin = self.nodes_origin();
        let bounds = *self
            .models
            .graph_view
            .bounds
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let centre = gpui::point(
            origin.x + bounds.size.width / 2.0,
            origin.y + bounds.size.height / 2.0,
        );
        let at = to_canvas(&self.models.graph_view.viewport, origin, centre);
        let Some(state) = self.models.graph.as_mut() else {
            return;
        };
        let id = state.graph.add(
            type_name,
            [
                crate::view::render::px32(at.x - NODE_W / 2.0),
                crate::view::render::px32(at.y - HEADER_H),
            ],
        );
        self.models.graph_view.selected = Some(id);
        self.nodes_revalidate();
    }

    /// Types a value into a node's input and re-checks the file.
    pub(crate) fn nodes_set_value(&mut self, id: u32, input: &str, value: serde_json::Value) {
        if let Some(node) = self
            .models
            .graph
            .as_mut()
            .and_then(|s| s.graph.node_mut(id))
        {
            node.values.insert(input.to_string(), value);
        }
        self.nodes_revalidate();
    }

    /// Removes the selected node and its wires.
    pub(crate) fn nodes_delete_selected(&mut self) {
        let Some(id) = self.models.graph_view.selected.take() else {
            return;
        };
        if let Some(state) = self.models.graph.as_mut() {
            state.graph.remove(id);
        }
        self.nodes_revalidate();
    }

    /// Writes the open file.
    pub(crate) fn save_nodes_file(&mut self) {
        let Some(state) = self.models.graph.as_ref() else {
            return;
        };
        if let Some(dir) = state.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        self.status_note = Some(match state.graph.save(&state.path) {
            Ok(()) => format!("Saved {}", file_label(&state.path)).into(),
            Err(e) => e.into(),
        });
    }

    /// A press: selects, or starts a move, a pan, or a wire.
    pub(crate) fn nodes_mouse_down(&mut self, pos: gpui::Point<gpui::Pixels>, _clicks: usize) {
        let origin = self.nodes_origin();
        if !self.models.graph_view.fitted {
            // First use: canvas units at one pixel each, a margin in.
            self.models.graph_view.viewport.zoom = 1.0;
            self.models.graph_view.viewport.offset = kurbo::Vec2::new(24.0, 24.0);
            self.models.graph_view.fitted = true;
        }
        let at = to_canvas(&self.models.graph_view.viewport, origin, pos);
        let window = kurbo::Point::new(f64::from(f32::from(pos.x)), f64::from(f32::from(pos.y)));
        let drag = match self.nodes_hit(at) {
            Hit::Node(id) => {
                self.models.graph_view.selected = Some(id);
                let origin_pos = self
                    .models
                    .graph
                    .as_ref()
                    .and_then(|s| s.graph.node(id))
                    .map(|n| n.pos)
                    .unwrap_or_default();
                NodeDrag::Move {
                    id,
                    start: at,
                    origin: origin_pos,
                }
            }
            Hit::Output(from, output, kind) => NodeDrag::Wire {
                from,
                output,
                kind,
                to: window,
            },
            Hit::Input(to, input, _) => {
                // Picking up a wired input takes the wire off it, to
                // drop somewhere else or nowhere.
                let existing = self
                    .models
                    .graph
                    .as_ref()
                    .and_then(|s| s.graph.link_into(to, &input).cloned());
                match existing {
                    Some(link) => {
                        let kind = self
                            .models
                            .graph
                            .as_ref()
                            .and_then(|s| {
                                let n = s.graph.node(link.from())?;
                                s.registry
                                    .get(&n.type_name)?
                                    .output(link.output())
                                    .map(|p| p.kind)
                            })
                            .unwrap_or(runebender_core::document::nodes::Kind::Text);
                        if let Some(s) = self.models.graph.as_mut() {
                            s.graph.links.retain(|l| l != &link);
                        }
                        self.nodes_revalidate();
                        NodeDrag::Wire {
                            from: link.from(),
                            output: link.output().to_string(),
                            kind,
                            to: window,
                        }
                    }
                    None => NodeDrag::Pan { last: window },
                }
            }
            Hit::Empty => {
                self.models.graph_view.selected = None;
                NodeDrag::Pan { last: window }
            }
        };
        self.models.graph_view.drag = Some(drag);
    }

    /// The pointer moved with the button down. True when something
    /// changed.
    pub(crate) fn nodes_mouse_drag(&mut self, pos: gpui::Point<gpui::Pixels>) -> bool {
        let origin = self.nodes_origin();
        let window = kurbo::Point::new(f64::from(f32::from(pos.x)), f64::from(f32::from(pos.y)));
        let at = to_canvas(&self.models.graph_view.viewport, origin, pos);
        match self.models.graph_view.drag.clone() {
            Some(NodeDrag::Move {
                id,
                start,
                origin: from,
            }) => {
                if let Some(node) = self
                    .models
                    .graph
                    .as_mut()
                    .and_then(|s| s.graph.node_mut(id))
                {
                    node.pos = [
                        from[0] + crate::view::render::px32(at.x - start.x),
                        from[1] + crate::view::render::px32(at.y - start.y),
                    ];
                }
                true
            }
            Some(NodeDrag::Pan { last }) => {
                self.models
                    .graph_view
                    .viewport
                    .pan(window.x - last.x, window.y - last.y);
                self.models.graph_view.drag = Some(NodeDrag::Pan { last: window });
                true
            }
            Some(NodeDrag::Wire {
                from, output, kind, ..
            }) => {
                self.models.graph_view.drag = Some(NodeDrag::Wire {
                    from,
                    output,
                    kind,
                    to: window,
                });
                true
            }
            None => false,
        }
    }

    /// The release: a wire lands on an input of the same kind, or is
    /// dropped.
    pub(crate) fn nodes_mouse_up(&mut self, pos: gpui::Point<gpui::Pixels>) {
        let origin = self.nodes_origin();
        let at = to_canvas(&self.models.graph_view.viewport, origin, pos);
        if let Some(NodeDrag::Wire {
            from, output, kind, ..
        }) = self.models.graph_view.drag.take()
        {
            if let Hit::Input(to, input, want) = self.nodes_hit(at) {
                if to != from && want == kind {
                    if let Some(s) = self.models.graph.as_mut() {
                        s.graph.connect(from, &output, to, &input);
                    }
                } else if want != kind {
                    self.status_note = Some(format!("{kind} does not go into {want}").into());
                }
            }
            self.nodes_revalidate();
        }
        self.models.graph_view.drag = None;
    }

    /// The wheel zooms about the cursor, as it does on the glyph.
    pub(crate) fn nodes_scroll(&mut self, event: &gpui::ScrollWheelEvent) {
        let origin = self.nodes_origin();
        let delta = match event.delta {
            gpui::ScrollDelta::Pixels(p) => f64::from(f32::from(p.y)),
            gpui::ScrollDelta::Lines(p) => f64::from(p.y * 24.0),
        };
        let local = kurbo::Point::new(
            f64::from(f32::from(event.position.x - origin.x)),
            f64::from(f32::from(event.position.y - origin.y)),
        );
        let factor = (delta * crate::workspace::ZOOM_PER_PIXEL).exp();
        self.models
            .graph_view
            .viewport
            .zoom_about(local, factor, 0.25, 4.0);
        self.models.graph_view.fitted = true;
    }
}
