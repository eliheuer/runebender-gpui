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
        let body = body.child(
            div()
                .id("ai-run")
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
                .child(if in_editor {
                    "Bolden this glyph"
                } else {
                    "Open a glyph to run"
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Mode::Editor(index) = this.mode {
                        this.run_task(Some(index), cx);
                        cx.notify();
                    }
                })),
        );

        // The whole master at once. The result waits as a proposal
        // layer until it is installed or discarded, because a font's
        // worth of edits is not something to land unasked.
        let has_font = self.project.is_some();
        let body = body.child(
            div()
                .id("ai-run-all")
                .px_1()
                .py_0p5()
                .border(t::stroke())
                .border_color(t::panel_outline())
                .cursor_pointer()
                .text_xs()
                .text_color(if has_font { t::text() } else { t::text_muted() })
                .child("Propose Bold master")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.run_task(None, cx);
                    cx.notify();
                })),
        );

        let body = match &self.models.busy {
            Some(note) => body.child(div().text_xs().text_color(t::accent()).child(note.clone())),
            None => body,
        };

        // A proposal waiting: what it holds, and the two answers.
        let body = match &self.models.proposal {
            Some(p) => body
                .child(div().text_xs().text_color(t::text()).child(format!(
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
                                .id("ai-install")
                                .flex_1()
                                .px_1()
                                .py_0p5()
                                .border(t::stroke())
                                .border_color(t::accent())
                                .cursor_pointer()
                                .text_xs()
                                .text_color(t::accent())
                                .child("Install")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.install_proposal(None);
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .id("ai-discard")
                                .flex_1()
                                .px_1()
                                .py_0p5()
                                .border(t::stroke())
                                .border_color(t::panel_outline())
                                .cursor_pointer()
                                .text_xs()
                                .text_color(t::text_muted())
                                .child("Discard")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.discard_proposal();
                                    cx.notify();
                                })),
                        ),
                ),
            None => body,
        };

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
