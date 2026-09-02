// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Local AI panel: installed models and the bolden control.

use crate::Mode;
use crate::Workspace;
use crate::view::paint::flat_slider;
use crate::view::theme as t;
use gpui::Context;
use gpui::InteractiveElement;
use gpui::ParentElement;
use gpui::SharedString;
use gpui::StatefulInteractiveElement;
use gpui::Styled;
use gpui::div;
use gpui::px;
impl Workspace {
    /// The Local AI section: choose a model, run it, and see how the
    /// result scores against a master already drawn.
    ///
    /// Both halves matter. Running a model is easy to offer and easy
    /// to trust too far; scoring it against work done by hand is what
    /// says whether the proposal was worth having.
    pub(crate) fn local_ai_panel(&self, cx: &mut Context<'_, Self>) -> gpui::Div {
        let body = div().flex().flex_col().gap_1p5();

        // Which model, and a way to change it.
        let label: SharedString = self
            .models
            .summary
            .clone()
            .unwrap_or_else(|| "No model chosen".into());
        let body = body.child(
            div()
                .id("ai-model")
                .px_1()
                .py_0p5()
                .border(t::stroke())
                .border_color(t::panel_outline())
                .cursor_pointer()
                .text_xs()
                .text_color(t::text())
                .child(label)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.command_choose_model(cx);
                })),
        );

        // Anything installed, listed without a file picker. A model is
        // a directory with a config.json in it, so installing one is
        // dropping it in the folder.
        let installed = Self::installed_models();
        let body = if installed.is_empty() {
            body
        } else {
            installed.into_iter().fold(body, |el, (name, path)| {
                let current = self.models.dir.as_deref() == Some(path.as_path());
                el.child(
                    div()
                        .id(SharedString::from(format!("ai-installed-{name}")))
                        .px_1()
                        .py_0p5()
                        .border(t::stroke())
                        .border_color(if current {
                            t::accent()
                        } else {
                            t::panel_outline()
                        })
                        .cursor_pointer()
                        .text_xs()
                        .text_color(if current { t::accent() } else { t::text() })
                        .child(name)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.load_model(&path);
                            cx.notify();
                        })),
                )
            })
        };

        if self.models.dir.is_none() {
            let where_to_put_them = Self::models_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "~/.runebender/models".into());
            return body.child(div().text_xs().text_color(t::text_muted()).child(format!(
                "Drop a model folder in {where_to_put_them} and it \
                 appears here. A model is a folder holding config.json, \
                 weights.safetensors and vocab.txt. Nothing is downloaded."
            )));
        }

        // Strength, because a model can be right about direction and
        // short on distance.
        let body = match &self.models.strength_slider {
            Some(slider) => body.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .w(px(58.0))
                            .text_xs()
                            .text_color(t::text_muted())
                            .child(format!("{:.2}x", self.models.strength)),
                    )
                    .child(div().flex_1().child(flat_slider(slider, cx))),
            ),
            None => body,
        };

        let in_editor = matches!(self.mode, Mode::Editor(_));
        let has_font = self.project.is_some();

        // One row per task font-ml says it runs. The list comes from
        // the tool, so a task it gains appears here with no change to
        // this file. "This glyph" installs at once, undo to reject;
        // "every glyph" leaves a proposal waiting below.
        let tasks: Vec<crate::edit::local_ai::TaskRow> = self
            .models
            .tasks
            .clone()
            .unwrap_or_default()
            .into_iter()
            .filter(|t| t.implemented)
            .collect();
        let body = if self.models.tasks.is_none() {
            body.child(div().text_xs().text_color(t::text_muted()).child(
                if Self::font_ml_binary().is_some() {
                    "Asking font-ml what it can do…"
                } else {
                    "font-ml not found. cargo install --git \
                         https://github.com/eliheuer/font-ml, or set RUNEBENDER_FONT_ML."
                },
            ))
        } else if tasks.is_empty() {
            body.child(
                div()
                    .text_xs()
                    .text_color(t::text_muted())
                    .child("font-ml has no task built yet"),
            )
        } else {
            tasks.into_iter().fold(body, |el, task| {
                let name = task.name.clone();
                let one = task.takes_glyph();
                let all = task.takes_glyphs();
                let row = div().flex().gap_1();
                let row = if one {
                    let name = name.clone();
                    row.child(
                        div()
                            .id(SharedString::from(format!("ai-run-{}", task.name)))
                            .flex_1()
                            .px_1()
                            .py_0p5()
                            .border(t::stroke())
                            .border_color(if in_editor {
                                t::accent()
                            } else {
                                t::panel_outline()
                            })
                            .cursor_pointer()
                            .text_xs()
                            .text_color(if in_editor {
                                t::text()
                            } else {
                                t::text_muted()
                            })
                            .child(format!("{}: this glyph", task.title))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Mode::Editor(index) = this.mode {
                                    this.run_task(&name, Some(index), cx);
                                    cx.notify();
                                }
                            })),
                    )
                } else {
                    row
                };
                let row = if all {
                    let name = name.clone();
                    row.child(
                        div()
                            .id(SharedString::from(format!("ai-run-all-{}", task.name)))
                            .flex_1()
                            .px_1()
                            .py_0p5()
                            .border(t::stroke())
                            .border_color(t::panel_outline())
                            .cursor_pointer()
                            .text_xs()
                            .text_color(if has_font { t::text() } else { t::text_muted() })
                            .child(format!("{}: every glyph", task.title))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.run_task(&name, None, cx);
                                cx.notify();
                            })),
                    )
                } else {
                    row
                };
                el.child(row)
            })
        };

        let body = match &self.models.busy {
            Some(note) => body.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(t::accent())
                            .child(note.clone()),
                    )
                    .child(
                        div()
                            .id("ai-cancel")
                            .px_1()
                            .py_0p5()
                            .border(t::stroke())
                            .border_color(t::panel_outline())
                            .cursor_pointer()
                            .text_xs()
                            .text_color(t::text_muted())
                            .child("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cancel_task();
                                cx.notify();
                            })),
                    ),
            ),
            None => body,
        };

        // Proposals waiting: what each holds, and the two answers.
        let body = self.models.proposals.iter().fold(body, |el, p| {
            let install_task = p.task.clone();
            let discard_task = p.task.clone();
            el.child(div().text_xs().text_color(t::text()).child(format!(
                "{} proposed: {} glyphs, {} keep structure",
                p.task,
                p.glyphs.len(),
                p.compatible.len()
            )))
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(
                        div()
                            .id(SharedString::from(format!("ai-install-{}", p.task)))
                            .flex_1()
                            .px_1()
                            .py_0p5()
                            .border(t::stroke())
                            .border_color(t::accent())
                            .cursor_pointer()
                            .text_xs()
                            .text_color(t::accent())
                            .child("Install")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.install_proposal(&install_task, None);
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("ai-discard-{}", p.task)))
                            .flex_1()
                            .px_1()
                            .py_0p5()
                            .border(t::stroke())
                            .border_color(t::panel_outline())
                            .cursor_pointer()
                            .text_xs()
                            .text_color(t::text_muted())
                            .child("Discard")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.discard_proposal(&discard_task);
                                cx.notify();
                            })),
                    ),
            )
        });

        // The judgement, when there is another master to judge against.
        let body = body.child(
            div()
                .id("ai-score")
                .px_1()
                .py_0p5()
                .border(t::stroke())
                .border_color(t::panel_outline())
                .cursor_pointer()
                .text_xs()
                .text_color(t::text_muted())
                .child("Score against the other master")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.command_score_model(cx);
                    cx.notify();
                })),
        );

        match &self.models.score {
            Some((glyph, model, baseline)) => {
                let better = model < baseline;
                body.child(
                    div()
                        .text_xs()
                        .text_color(if better { t::accent() } else { t::text_muted() })
                        .child(format!(
                            "{glyph}: model {model:.1}, mean-shift {baseline:.1}"
                        )),
                )
            }
            None => body,
        }
    }
}
