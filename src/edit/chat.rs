// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Chat pane: a local language model over the open font.
//!
//! The model runs in `font-ml chat`, a separate process, over the
//! harness core defines (`runebender-core agent tools`). This shell
//! keeps the conversation, sends it for each turn on stdin, reads the
//! JSON events back, and shows them. When the model proposes, the
//! layer lands on disk like any other proposal and is adopted into
//! the open font for the person to install or discard. The shell
//! never lets the model near the foreground: the tool list has no
//! way to, and the pane adds none.

use std::sync::{Arc, Mutex};

use crate::PathBuf;
use crate::Workspace;
use gpui::SharedString;
use serde_json::Value;

#[cfg(not(target_family = "wasm"))]
use gpui::Context;

/// One row in the transcript.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ChatEntry {
    /// What the person typed.
    User(SharedString),
    /// What the model said, tool blocks removed; grows while streaming.
    Assistant(SharedString),
    /// A tool the model called, with a one-line result.
    Tool {
        /// The tool name.
        name: SharedString,
        /// Whether it ran.
        ok: bool,
        /// One line about the result.
        note: SharedString,
    },
    /// Something went wrong.
    Error(SharedString),
}

/// The pane's state.
#[derive(Default)]
pub(crate) struct ChatState {
    /// The chosen model directory.
    pub(crate) model: Option<PathBuf>,
    /// The GGUF models found under the model roots.
    pub(crate) installed: Vec<(String, PathBuf)>,
    /// Whether the roots were scanned.
    pub(crate) scanned: bool,
    /// The transcript.
    pub(crate) entries: Vec<ChatEntry>,
    /// The conversation as font-ml wants it back for the next turn.
    pub(crate) messages: Vec<Value>,
    /// What is happening, while a turn runs.
    pub(crate) busy: Option<SharedString>,
    /// The running process, so Cancel can kill it.
    pub(crate) job: Option<Arc<Mutex<Option<std::process::Child>>>>,
    /// Speed of the last turn.
    pub(crate) last_speed: Option<SharedString>,
}

/// Where `runebender-core` is: `$RUNEBENDER_CORE`, PATH, then
/// `~/.cargo/bin`.
fn core_binary() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("RUNEBENDER_CORE").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    if let Some(found) = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("runebender-core"))
            .find(|c| c.is_file())
    }) {
        return Some(found);
    }
    let home = std::env::var_os("HOME")?;
    let cargo_bin = PathBuf::from(home).join(".cargo/bin/runebender-core");
    cargo_bin.is_file().then_some(cargo_bin)
}

impl Workspace {
    /// The GGUF models on disk, scanned once and on demand.
    pub(crate) fn scan_chat_models(&mut self) {
        self.chat.installed = runebender_core::document::nodes_run::installed_chat_models(
            Self::models_dir().as_deref(),
        );
        self.chat.scanned = true;
        if self.chat.model.is_none() {
            // Prefer the 4B when it is there; it is the smallest that
            // reads a tool result and decides well.
            let pick = self
                .chat
                .installed
                .iter()
                .find(|(n, _)| n.contains("4b"))
                .or_else(|| self.chat.installed.first())
                .map(|(_, p)| p.clone());
            self.chat.model = pick;
        }
    }

    /// Forgets the transcript. The model keeps nothing between turns.
    pub(crate) fn chat_clear(&mut self) {
        self.chat.entries.clear();
        self.chat.messages.clear();
        self.chat.last_speed = None;
    }

    /// Sends one message and runs the turn on a background thread.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn chat_send(&mut self, text: String, cx: &mut Context<'_, Self>) {
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        if self.chat.busy.is_some() {
            self.status_note = Some("The model is still answering".into());
            return;
        }
        let Some(model) = self.chat.model.clone() else {
            self.status_note = Some(
                "No chat model: put a folder with a .gguf and tokenizer.json under \
                 ~/.runebender/models"
                    .into(),
            );
            return;
        };
        let Some(font_ml) = self.models.binary.clone() else {
            self.status_note = Some("font-ml not found".into());
            return;
        };
        let Some(core) = core_binary() else {
            self.status_note = Some(
                "runebender-core not found: cargo install --git \
                 https://github.com/eliheuer/runebender-core, or set RUNEBENDER_CORE"
                    .into(),
            );
            return;
        };
        // The tools read the font on disk.
        if self
            .project
            .as_ref()
            .is_some_and(|p| p.masters.iter().any(|m| m.dirty))
        {
            self.command_save(cx);
        }
        let Some(project) = self.project.as_ref() else {
            self.status_note = Some("Open a font first".into());
            return;
        };
        let font = project
            .export_source
            .clone()
            .unwrap_or_else(|| project.active_font().source_path.clone());
        let source = project.active_font().source_path.clone();

        self.chat.entries.push(ChatEntry::User(text.clone().into()));
        self.chat
            .messages
            .push(serde_json::json!({ "role": "user", "content": text }));
        self.chat
            .entries
            .push(ChatEntry::Assistant(SharedString::default()));
        self.chat.busy = Some("Loading the model…".into());
        let conversation = serde_json::to_string(&self.chat.messages).unwrap_or_default();

        let events: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let job: Arc<Mutex<Option<std::process::Child>>> = Arc::new(Mutex::new(None));
        let finished: Arc<Mutex<Option<Result<(), String>>>> = Arc::new(Mutex::new(None));
        self.chat.job = Some(job.clone());
        cx.background_executor()
            .spawn({
                let events = events.clone();
                let job = job.clone();
                let finished = finished.clone();
                async move {
                    let result =
                        run_chat(&font_ml, &model, &font, &core, &conversation, &events, &job);
                    *finished.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
                }
            })
            .detach();
        cx.spawn(async move |this, cx| {
            let result = loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(120))
                    .await;
                let batch: Vec<Value> =
                    std::mem::take(&mut *events.lock().unwrap_or_else(|e| e.into_inner()));
                if !batch.is_empty() {
                    this.update(cx, |workspace, cx| {
                        for event in batch {
                            workspace.chat_event(event);
                        }
                        cx.notify();
                    })
                    .ok();
                }
                if let Some(result) = finished.lock().unwrap_or_else(|e| e.into_inner()).take() {
                    break result;
                }
            };
            this.update(cx, |workspace, cx| {
                workspace.chat.busy = None;
                workspace.chat.job = None;
                if let Err(e) = result {
                    workspace.chat.entries.push(ChatEntry::Error(e.into()));
                }
                workspace.chat_finished(&source);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Kills the running turn.
    pub(crate) fn chat_cancel(&mut self) {
        let Some(job) = self.chat.job.take() else {
            return;
        };
        if let Some(child) = job.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let _ = child.kill();
        }
        self.chat.busy = None;
        self.status_note = Some("Cancelled".into());
    }

    /// One event from font-ml, into the transcript.
    fn chat_event(&mut self, event: Value) {
        let kind = event.get("event").and_then(Value::as_str).unwrap_or("");
        match kind {
            "loaded" => {
                let device = event.get("device").and_then(Value::as_str).unwrap_or("cpu");
                self.chat.busy = Some(format!("Thinking on {device}…").into());
            }
            "token" => {
                let text = event.get("text").and_then(Value::as_str).unwrap_or("");
                if let Some(ChatEntry::Assistant(s)) = self.chat.entries.last_mut() {
                    let mut full = s.to_string();
                    full.push_str(text);
                    // Tool blocks are shown as rows, not as text.
                    *s = visible_text(&full).into();
                    // Keep the raw text for the next delta.
                    self.chat_raw = full;
                }
            }
            "tool_call" => {
                let name = event.get("name").and_then(Value::as_str).unwrap_or("?");
                self.chat.busy = Some(format!("Running {name}…").into());
                self.chat.entries.push(ChatEntry::Tool {
                    name: name.to_string().into(),
                    ok: true,
                    note: "…".into(),
                });
            }
            "tool_result" => {
                let name = event.get("name").and_then(Value::as_str).unwrap_or("?");
                let ok = event.get("ok").and_then(Value::as_bool).unwrap_or(false);
                let note = result_note(name, event.get("result").unwrap_or(&Value::Null));
                if let Some(ChatEntry::Tool { note: n, ok: o, .. }) = self
                    .chat
                    .entries
                    .iter_mut()
                    .rev()
                    .find(|e| matches!(e, ChatEntry::Tool { .. }))
                {
                    *n = note.into();
                    *o = ok;
                }
                // The model's next words start a new bubble.
                self.chat
                    .entries
                    .push(ChatEntry::Assistant(SharedString::default()));
                self.chat_raw.clear();
                self.chat.busy = Some("Thinking…".into());
            }
            "done" => {
                let tps = event
                    .get("tokens_per_second")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                let tokens = event.get("tokens").and_then(Value::as_u64).unwrap_or(0);
                self.chat.last_speed = Some(format!("{tokens} tokens, {tps:.1} tok/s").into());
                if let Some(text) = event.get("text").and_then(Value::as_str)
                    && let Some(ChatEntry::Assistant(s)) = self.chat.entries.last_mut()
                {
                    *s = text.to_string().into();
                }
            }
            "messages" => {
                if let Some(m) = event.get("messages").and_then(Value::as_array) {
                    self.chat.messages = m.clone();
                }
            }
            _ => {}
        }
    }

    /// After a turn: proposals the model made are adopted from disk so
    /// they show in the Local AI panel with Install and Discard.
    fn chat_finished(&mut self, source: &std::path::Path) {
        self.chat_raw.clear();
        self.chat
            .entries
            .retain(|e| !matches!(e, ChatEntry::Assistant(s) if s.is_empty()));
        let proposed: Vec<String> = self
            .chat
            .messages
            .iter()
            .filter(|m| m.get("role").and_then(Value::as_str) == Some("tool"))
            .filter_map(|m| m.get("content").and_then(Value::as_str))
            .filter_map(|c| serde_json::from_str::<Value>(c).ok())
            .filter(|r| r.get("name").and_then(Value::as_str) == Some("propose"))
            .filter_map(|r| {
                r.get("result")?
                    .get("proposal")?
                    .get("task")?
                    .as_str()
                    .map(String::from)
            })
            .collect();
        for task in proposed {
            if let Err(e) = self.adopt_proposal_from_disk(&task, source) {
                self.status_note = Some(e.into());
            }
        }
        self.refresh_proposal();
    }

    /// In the browser there is no process to run.
    #[cfg(target_family = "wasm")]
    pub(crate) fn chat_send(&mut self, _text: String, _cx: &mut gpui::Context<'_, Self>) {
        self.status_note = Some("Chat runs in the desktop app".into());
    }
}

/// The reply without its tool blocks, for the bubble.
fn visible_text(raw: &str) -> String {
    let mut out = String::new();
    let mut rest = raw;
    while let Some(start) = rest.find("<tool_call>") {
        out.push_str(&rest[..start]);
        match rest[start..].find("</tool_call>") {
            Some(end) => rest = &rest[start + end + "</tool_call>".len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out.trim_start().to_string()
}

/// One line about what a tool returned.
fn result_note(name: &str, result: &Value) -> String {
    if let Some(e) = result.get("error").and_then(Value::as_str) {
        return e.to_string();
    }
    match name {
        "font_info" => format!(
            "{} {}, {} glyphs",
            result.get("family").and_then(Value::as_str).unwrap_or(""),
            result.get("style").and_then(Value::as_str).unwrap_or(""),
            result.get("glyphs").and_then(Value::as_u64).unwrap_or(0)
        ),
        "read_glyph" => format!(
            "{}: advance {}, {} contours",
            result.get("glyph").and_then(Value::as_str).unwrap_or(""),
            result.get("advance").and_then(Value::as_f64).unwrap_or(0.0),
            result
                .get("contours")
                .and_then(Value::as_array)
                .map_or(0, Vec::len)
        ),
        "propose" => match result.get("proposal") {
            Some(p) if !p.is_null() => format!(
                "proposed {} glyphs, install or discard below",
                p.get("glyphs")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len)
            ),
            _ => "ran, no proposal written".into(),
        },
        "proof" => format!(
            "{} glyphs proofed",
            result
                .get("glyphs")
                .and_then(Value::as_array)
                .map_or(0, Vec::len)
        ),
        "docs" => format!(
            "{} passages",
            result
                .get("passages")
                .and_then(Value::as_array)
                .map_or(0, Vec::len)
        ),
        _ => "done".into(),
    }
}

/// Runs one turn of `font-ml chat` to completion on the calling
/// thread, pushing events as they come.
#[cfg(not(target_family = "wasm"))]
fn run_chat(
    font_ml: &std::path::Path,
    model: &std::path::Path,
    font: &std::path::Path,
    core: &std::path::Path,
    conversation: &str,
    events: &Mutex<Vec<Value>>,
    job: &Mutex<Option<std::process::Child>>,
) -> Result<(), String> {
    use std::io::{BufRead as _, Write as _};
    let mut child = std::process::Command::new(font_ml)
        .arg("chat")
        .arg("--model")
        .arg(model)
        .arg("--font")
        .arg(font)
        .arg("--core")
        .arg(core)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("{e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(conversation.as_bytes());
    }
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let stderr = child.stderr.take();
    *job.lock().unwrap_or_else(|e| e.into_inner()) = Some(child);
    let err_reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut e) = stderr {
            let _ = std::io::Read::read_to_string(&mut e, &mut text);
        }
        text
    });
    for line in std::io::BufReader::new(stdout)
        .lines()
        .map_while(Result::ok)
    {
        if let Ok(v) = serde_json::from_str::<Value>(&line) {
            events.lock().unwrap_or_else(|e| e.into_inner()).push(v);
        }
    }
    let status = {
        let mut slot = job.lock().unwrap_or_else(|e| e.into_inner());
        match slot.as_mut() {
            Some(child) => child.wait().map_err(|e| format!("{e}"))?,
            None => return Err("cancelled".into()),
        }
    };
    let stderr_text = err_reader.join().unwrap_or_default();
    if status.success() {
        Ok(())
    } else if status.code().is_none() {
        Err("cancelled".into())
    } else {
        let last = stderr_text.lines().last().unwrap_or("font-ml chat failed");
        Err(last.to_string())
    }
}
