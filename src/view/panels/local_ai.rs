// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Local AI panel: installed models, the tasks font-ml runs, and
//! the proposals waiting.

use crate::Mode;
use crate::Workspace;
use crate::view::controls as c;
use crate::view::paint::flat_slider;
use crate::view::theme as t;
use gpui::Context;
use gpui::ParentElement;
use gpui::SharedString;
use gpui::StatefulInteractiveElement;
use gpui::Styled;
use gpui::div;
use gpui::px;
impl Workspace {
    /// The Local AI section: choose a model, run a task, and see how
    /// the result scores against a master already drawn.
    ///
    /// Both halves matter. Running a model is easy to offer and easy
    /// to trust too far; scoring it against work done by hand is what
    /// says whether the proposal was worth having.
    pub(crate) fn local_ai_panel(&self, cx: &mut Context<'_, Self>) -> gpui::Div {
        let body = c::column();

        // Which model, and a way to change it.
        let label: SharedString = self
            .models
            .summary
            .clone()
            .unwrap_or_else(|| "Choose a model…".into());
        let body = body.child(
            c::row().child(
                c::button("ai-model", label).on_click(cx.listener(|this, _, _, cx| {
                    this.command_choose_model(cx);
                })),
            ),
        );

        // Anything installed, listed without a file picker. A model is
        // a directory with a config.json in it, so installing one is
        // dropping it in the folder.
        let installed = self.models.installed.clone();
        let body = if installed.is_empty() {
            body
        } else {
            installed.into_iter().fold(body, |el, (name, path)| {
                let current = self.models.dir.as_deref() == Some(path.as_path());
                el.child(
                    c::row().child(
                        c::toggle(
                            SharedString::from(format!("ai-installed-{name}")),
                            name,
                            current,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.load_model(&path);
                            cx.notify();
                        })),
                    ),
                )
            })
        };

        if self.models.dir.is_none() {
            let where_to_put_them = Self::models_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "~/.runebender/models".into());
            return body.child(div().text_color(t::text_muted()).child(format!(
                "Drop a model folder in {where_to_put_them} and it \
                 appears here. A model is a folder holding config.json, \
                 weights.safetensors and vocab.txt. Nothing is downloaded."
            )));
        }

        // Strength, because a model can be right about direction and
        // short on distance.
        let body = match &self.models.strength_slider {
            Some(slider) => body.child(
                c::row()
                    .child(c::label(format!("Strength {:.2}×", self.models.strength)))
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
            body.child(
                div()
                    .text_color(t::text_muted())
                    .child(if self.models.binary.is_some() {
                        "Asking font-ml what it can do…"
                    } else {
                        "font-ml not found. cargo install --git \
                         https://github.com/eliheuer/font-ml, or set RUNEBENDER_FONT_ML."
                    }),
            )
        } else if tasks.is_empty() {
            body.child(
                div()
                    .text_color(t::text_muted())
                    .child("font-ml has no task built yet"),
            )
        } else {
            tasks.into_iter().fold(body, |el, task| {
                let name = task.name.clone();
                let one = task.takes_glyph();
                let all = task.takes_glyphs();
                let row = c::row();
                let row = if one {
                    let name = name.clone();
                    row.child(
                        c::toggle(
                            SharedString::from(format!("ai-run-{}", task.name)),
                            format!("{}: this glyph", task.title),
                            in_editor,
                        )
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
                        c::toggle(
                            SharedString::from(format!("ai-run-all-{}", task.name)),
                            format!("{}: every glyph", task.title),
                            has_font,
                        )
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
                c::row()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(t::text())
                            .child(note.clone()),
                    )
                    .child(
                        c::button("ai-cancel", "Cancel")
                            .flex_none()
                            .w(px(72.0))
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
            el.child(div().text_color(t::text()).child(format!(
                "{} proposed: {} glyphs, {} keep structure",
                p.task,
                p.glyphs.len(),
                p.compatible.len()
            )))
            .child(
                c::row()
                    .child(
                        c::toggle(
                            SharedString::from(format!("ai-install-{}", p.task)),
                            "Install",
                            true,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.install_proposal(&install_task, None);
                            cx.notify();
                        })),
                    )
                    .child(
                        c::button(
                            SharedString::from(format!("ai-discard-{}", p.task)),
                            "Discard",
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.discard_proposal(&discard_task);
                            cx.notify();
                        })),
                    ),
            )
        });

        // The judgement, when there is another master to judge against.
        let body = body.child(c::row().child(
            c::button("ai-score", "Score against the other master").on_click(cx.listener(
                |this, _, _, cx| {
                    this.command_score_model(cx);
                    cx.notify();
                },
            )),
        ));

        match &self.models.score {
            Some((glyph, model, baseline)) => {
                let better = model < baseline;
                body.child(
                    div()
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
