// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! Local models: choosing, scoring, and running the bolden model.

use crate::*;

impl Workspace {
    /// Choose a model directory and remember it.
    pub(crate) fn command_choose_model(&mut self, cx: &mut Context<Self>) {
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
    pub(crate) fn command_score_model(&mut self) {
        let Mode::Editor(index) = self.mode else {
            self.status_note = Some("Open a glyph first".into());
            return;
        };
        let Some(dir) = self.models.dir.clone() else {
            return;
        };
        let Ok(checkpoint) = font_ml::Checkpoint::open(&dir) else {
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
        let advance = entry.advance;
        let unicode = entry.codepoint.map(|c| c as u32);
        let (Some(from), Some(actual)) = (
            project.active_font().font.get_glyph(name.as_str()),
            project.masters[target].font.get_glyph(name.as_str()),
        ) else {
            return;
        };
        let (Some(from_ops), Some(actual_ops)) = (
            font_ml::ufo::glyph_ops(from),
            font_ml::ufo::glyph_ops(actual),
        ) else {
            self.status_note = Some("No outline to score".into());
            return;
        };
        let Some(model) = self.models.loaded.clone() else {
            return;
        };
        let center = checkpoint
            .config
            .delta_center
            .map(|c| (c[0], c[1]))
            .unwrap_or((0, 0));
        let Ok(result) = font_ml::bolden::bolden(
            &model,
            &name,
            unicode,
            advance,
            &from_ops,
            center,
            checkpoint.config.trim_close,
            self.models.strength,
        ) else {
            return;
        };
        let score = font_ml::eval::score(
            &name,
            &result.to,
            &actual_ops,
            &result.from,
            (center.0 as f64, center.1 as f64),
        );
        if score.model.is_nan() {
            self.status_note = Some("Masters are not point-compatible here".into());
            return;
        }
        self.status_note = Some(
            format!(
                "{name}: model {:.1}, mean-shift {:.1}",
                score.model, score.baseline
            )
            .into(),
        );
        self.models.score = Some((name.into(), score.model, score.baseline));
    }

    /// Glyph > Bolden With Model…: pick a model directory, predict a
    /// heavier version of the open glyph, and install it as a proposal.
    ///
    /// The prediction is structure-forced: the model may only move the
    /// points that are already there, so the result stays
    /// point-compatible with what it came from. It lands in the
    /// current glyph and is undoable, so the way to reject it is
    /// Cmd+Z.
    pub(crate) fn command_bolden_with_model(&mut self, cx: &mut Context<Self>) {
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
                workspace.apply_bolden(index, &dir);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
