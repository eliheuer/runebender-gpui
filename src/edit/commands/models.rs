// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Local models: choosing, scoring, and running the bolden model.
//!
//! Every model call goes through the `font-ml` binary; see
//! `edit/local_ai.rs` for why.

use crate::Mode;
use crate::Workspace;
use gpui::Context;
impl Workspace {
    /// Choose a model directory and remember it.
    pub(crate) fn command_choose_model(&mut self, cx: &mut Context<'_, Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose model".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(dir) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |workspace, cx| {
                workspace.load_model(&dir);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Score the model on the open glyph against the master furthest
    /// from the active one, which is the one it is trying to predict.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn command_score_model(&mut self, cx: &mut Context<'_, Self>) {
        let Mode::Editor(index) = self.mode else {
            self.status_note = Some("Open a glyph first".into());
            return;
        };
        let Some(model) = self.models.dir.clone() else {
            return;
        };
        let Some(font_ml) = Self::font_ml_binary() else {
            self.status_note = Some("font-ml not found".into());
            return;
        };
        let Some(project) = self.project.as_ref() else {
            return;
        };
        if project.masters.len() < 2 {
            self.status_note = Some("Nothing to score against: one master".into());
            return;
        }
        let target = if project.active == 0 {
            project.masters.len() - 1
        } else {
            0
        };
        let Some(entry) = project.active_font().glyphs.get(index) else {
            return;
        };
        let name = entry.name.to_string();
        let regular = project.active_font().source_path.clone();
        let bold = project.masters[target].source_path.clone();
        let strength = self.models.strength;
        self.status_note = Some(format!("Scoring {name}…").into());
        cx.spawn(async move |this, cx| {
            let result: Result<serde_json::Value, String> = cx
                .background_executor()
                .spawn({
                    let name = name.clone();
                    async move {
                        let output = std::process::Command::new(&font_ml)
                            .arg("eval")
                            .arg("--model")
                            .arg(&model)
                            .arg("--regular")
                            .arg(&regular)
                            .arg("--bold")
                            .arg(&bold)
                            .arg("--glyphs")
                            .arg(&name)
                            .arg("--strength")
                            .arg(format!("{strength}"))
                            .arg("--json")
                            .output()
                            .map_err(|e| format!("{e}"))?;
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        let report: serde_json::Value = stdout
                            .lines()
                            .rev()
                            .find_map(|l| serde_json::from_str(l).ok())
                            .unwrap_or(serde_json::Value::Null);
                        if output.status.success() {
                            Ok(report)
                        } else {
                            Err(report
                                .get("error")
                                .and_then(|e| e.as_str())
                                .unwrap_or("eval failed")
                                .to_string())
                        }
                    }
                })
                .await;
            this.update(cx, |workspace, cx| {
                match result {
                    Ok(report) => {
                        let row = report
                            .get("per_glyph")
                            .and_then(|g| g.as_array())
                            .and_then(|g| g.first())
                            .cloned();
                        let model = row
                            .as_ref()
                            .and_then(|r| r.get("model"))
                            .and_then(|v| v.as_f64());
                        let baseline = row
                            .as_ref()
                            .and_then(|r| r.get("baseline"))
                            .and_then(|v| v.as_f64());
                        match (model, baseline) {
                            (Some(model), Some(baseline)) => {
                                workspace.status_note = Some(
                                    format!("{name}: model {model:.1}, mean-shift {baseline:.1}")
                                        .into(),
                                );
                                workspace.models.score =
                                    Some((name.clone().into(), model, baseline));
                            }
                            _ => {
                                workspace.status_note =
                                    Some("Masters are not point-compatible here".into());
                            }
                        }
                    }
                    Err(e) => workspace.status_note = Some(format!("font-ml: {e}").into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// In the browser there is no process to run.
    #[cfg(target_family = "wasm")]
    pub(crate) fn command_score_model(&mut self, _cx: &mut Context<'_, Self>) {
        self.status_note = Some("Local models run in the desktop app".into());
    }

    /// Glyph > Bolden With Model…: pick a model directory, predict a
    /// heavier version of the open glyph, and install it.
    ///
    /// The prediction is structure-forced: the model may only move the
    /// points that are already there, so the result stays
    /// point-compatible with what it came from. It lands in the
    /// current glyph and is undoable, so the way to reject it is
    /// Cmd+Z.
    pub(crate) fn command_bolden_with_model(&mut self, cx: &mut Context<'_, Self>) {
        let Mode::Editor(index) = self.mode else {
            self.status_note = Some("Open a glyph first".into());
            return;
        };
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose model".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(dir) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |workspace, cx| {
                workspace.load_model(&dir);
                workspace.run_task(Some(index), cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
