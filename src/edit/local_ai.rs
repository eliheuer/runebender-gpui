// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! The local models panel: finding models on disk and running one.
//!
//! Model discovery under the models directory, loading, learning a
//! weight delta from the font's own reference pairs, and applying the
//! bolden model to a glyph.

use crate::CONFIG;
use crate::PathBuf;
use crate::Workspace;
use runebender_core::outline::effects::bolden_contours;
impl Workspace {
    /// Read a model directory and cache the weights.
    /// Where a model is looked for when nobody points at one.
    ///
    /// `$RUNEBENDER_MODELS`, then the config file, then
    /// `~/.runebender/models`. The variable wins because a setting for
    /// one run has to beat a setting meant for every run.
    ///
    /// `$RUNEBENDER_MODELS`, else `~/.runebender/models`. A model is a
    /// directory holding `config.json`, so dropping one in is the whole
    /// installation step: no rebuild, no account, no file picker.
    pub(crate) fn models_dir() -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os("RUNEBENDER_MODELS") {
            return Some(PathBuf::from(dir));
        }
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

    pub(crate) fn load_model(&mut self, dir: &std::path::Path) {
        let checkpoint = match font_ml::Checkpoint::open(dir) {
            Ok(c) => c,
            Err(e) => {
                self.status_note = Some(format!("Model: {e}").into());
                return;
            }
        };
        match font_ml::outline::OutlineModel::load(&checkpoint) {
            Ok(model) => {
                self.models.summary = Some(checkpoint.summary().into());
                self.models.loaded = Some(std::rc::Rc::new(model));
                self.models.dir = Some(dir.to_path_buf());
                self.models.score = None;
                self.status_note = Some("Model loaded".into());
            }
            Err(e) => self.status_note = Some(format!("Model: {e}").into()),
        }
    }

    /// Run the model over the open glyph and install what it predicts.
    /// How much weight the other master adds, learned from glyphs
    /// drawn in both, and the height it was measured at.
    ///
    /// This is the "draw the key glyphs and let them carry the rest"
    /// workflow: draw n, o, H and O in the heavier master, and every
    /// other glyph is asked to add what those added, from wherever it
    /// already sits. A delta rather than one shared target, because
    /// caps and lowercase are drawn to different weights and a single
    /// target would flatten the difference.
    ///
    /// `None` with one master, or when none of the reference glyphs is
    /// drawn in both yet.
    pub(crate) fn model_weight_delta(&self) -> Option<(f64, f64)> {
        let project = self.project.as_ref()?;
        if project.masters.len() < 2 {
            return None;
        }
        let other = if project.active == 0 {
            project.masters.len() - 1
        } else {
            0
        };
        let here = &project.active_font().font;
        let there = &project.masters[other].font;
        let height = there
            .font_info
            .x_height
            .map(|v| v / 2.0)
            .or_else(|| there.font_info.units_per_em.map(|v| *v * 0.25))
            .unwrap_or(256.0);
        let pairs: Vec<_> = ["n", "o", "H", "O", "i", "l", "h", "m", "u", "I", "E"]
            .iter()
            .filter_map(|name| {
                let a = font_ml::ufo::glyph_ops(here.get_glyph(name)?)?;
                let b = font_ml::ufo::glyph_ops(there.get_glyph(name)?)?;
                Some((
                    font_ml::stems::ops_to_path(&a),
                    font_ml::stems::ops_to_path(&b),
                ))
            })
            .collect();
        font_ml::stems::reference_delta(&pairs, height).map(|d| (d, height))
    }

    pub(crate) fn apply_bolden(&mut self, index: usize, dir: &std::path::Path) {
        let checkpoint = match font_ml::Checkpoint::open(dir) {
            Ok(c) => c,
            Err(e) => {
                self.status_note = Some(format!("Model: {e}").into());
                return;
            }
        };
        if self.models.loaded.is_none() {
            self.load_model(dir);
        }
        let Some(model) = self.models.loaded.clone() else {
            return;
        };
        let Some(font) = self.font() else { return };
        let Some(entry) = font.glyphs.get(index) else {
            return;
        };
        let name = entry.name.to_string();
        let advance = entry.advance;
        let unicode = entry.codepoint.map(|c| c as u32);
        let Some(glyph) = font.font.get_glyph(name.as_str()) else {
            return;
        };
        let Some(ops) = font_ml::ufo::glyph_ops(glyph) else {
            self.status_note =
                Some("Nothing to bolden: this glyph is built from components".into());
            return;
        };

        let center = checkpoint
            .config
            .delta_center
            .map(|c| (c[0], c[1]))
            .unwrap_or((0, 0));
        let mut result_override = None;
        let predict = |strength: f64| {
            font_ml::bolden::bolden(
                model.as_ref(),
                &name,
                unicode,
                advance,
                &ops,
                center,
                checkpoint.config.trim_close,
                strength,
            )
        };
        let result = match predict(self.models.strength) {
            Ok(r) => r,
            Err(e) => {
                self.status_note = Some(format!("Bolden: {e}").into());
                return;
            }
        };
        // The model is better at shape than at weight, so where the
        // other master is drawn far enough to say what weight it
        // carries, land on that instead of on the slider. Measured on
        // Virtua this took stems from 46 units out to 40, and glyphs
        // at the right weight from 1 in 11 to 5.
        let mut fitted_to: Option<f64> = None;
        if let Some((delta, height)) = self.model_weight_delta() {
            let from_path = font_ml::stems::ops_to_path(&result.from);
            let want =
                font_ml::stems::target_from_delta(&from_path, delta, height).and_then(|target| {
                    font_ml::stems::fit_strength(
                        &from_path,
                        &font_ml::stems::ops_to_path(&result.to),
                        target,
                        height,
                    )
                });
            if let Some(want) = want.filter(|s| s.is_finite() && *s > 0.25 && *s < 4.0)
                && let Ok(refit) = predict(want)
                && refit.is_compatible()
            {
                fitted_to = Some(want);
                result_override = Some(refit);
            }
        }
        let result = result_override.unwrap_or(result);
        // The encoding guarantees this; assert it before writing to a
        // font rather than take it on trust.
        if !result.is_compatible() {
            self.status_note = Some("Refused: the prediction changed the point structure".into());
            return;
        }

        let expected = glyph
            .contours
            .iter()
            .map(|c| c.points.len() + 1)
            .sum::<usize>();
        if result.deltas.len() != expected {
            self.status_note = Some(
                format!(
                    "Refused: model returned {} offsets for {expected} points",
                    result.deltas.len()
                )
                .into(),
            );
            return;
        }
        let contours = bolden_contours(glyph, &result.deltas, center);
        let moved = result
            .deltas
            .iter()
            .filter(|(x, y)| *x != 0 || *y != 0)
            .count();
        self.push_undo_snapshot(index);
        self.font_mut().and_then(|f| {
            f.edit_glyph(index, |g| {
                g.contours = contours.clone();
            })
        });
        self.editor.selected.clear();
        // A model's output is the edit most worth having a record of:
        // it is the one nobody watched being made.
        self.journal(
            "bolden with model",
            Some(index),
            Some(format!(
                "{moved}/{} points moved, advance {:+}{}",
                result.deltas.len(),
                result.advance_delta,
                match fitted_to {
                    Some(s) => format!(", strength fitted to {s:.2}"),
                    None => String::new(),
                }
            )),
        );
        self.status_note = Some(
            format!(
                "Boldened {name}: {moved}/{} points moved, advance {:+}{}. \
                 Undo to reject.",
                result.deltas.len(),
                result.advance_delta,
                match fitted_to {
                    Some(s) => format!(", fitted to {s:.2}x"),
                    None => String::new(),
                }
            )
            .into(),
        );
    }
}
