// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Features: generating and applying feature code.

use crate::Workspace;
use gpui::Context;
use gpui::Window;
impl Workspace {
    /// Rewrite the automatic feature blocks (init/medi/fina from
    /// name suffixes, liga from underscore names) into the editor
    /// text for review; Apply commits. Hand-written blocks with
    /// other tags are untouched. This is the Features section's
    /// Generate button.
    pub(crate) fn command_generate_features(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(font) = self.font() else { return };
        let blocks = Self::generated_feature_blocks(&font.font);
        if blocks.is_empty() {
            self.features_status = Some("Nothing to generate from glyph names".into());
            return;
        }
        let mut fea = self.inputs.features.read(cx).value().to_string();
        let mut tags: Vec<String> = Vec::new();
        for (tag, body) in blocks {
            fea = Self::replace_feature_block(&fea, &tag, &body);
            tags.push(tag);
        }
        self.inputs.features.update(cx, |st, cx| {
            st.set_value(fea, window, cx);
        });
        self.features_edited = true;
        self.features_status =
            Some(format!("Generated {} · review and Apply", tags.join(", ")).into());
    }

    /// Apply the features editor to the active master: write
    /// features.fea, recompile the shaping models, and report the
    /// compile verdict. A file that does not compile is still saved;
    /// the old joining rules carry on. This is how Glyphs lets you
    /// keep a broken feature file open while you fix it.
    pub(crate) fn command_apply_features(&mut self, cx: &mut Context<Self>) {
        let fea = self.inputs.features.read(cx).value().to_string();
        let verdict = self.font().map(|f| Self::check_features_compile(f, &fea));
        if let Some(font) = self.font_mut() {
            if font.font.features != fea {
                font.font.features = fea;
                font.dirty = true;
            }
        } else {
            return;
        }
        self.features_edited = false;
        self.features_status = Some(match verdict {
            Some(Ok(())) => "Compiled clean · shaping updated".into(),
            Some(Err(e)) => {
                let first = e.lines().find(|l| !l.trim().is_empty()).unwrap_or("error");
                format!("Saved, but does not compile: {first}").into()
            }
            None => "Applied".into(),
        });
        self.rebuild_text_models();
    }
}
