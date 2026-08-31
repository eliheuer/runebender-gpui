// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! Glyph menu: add, remove, duplicate, generate, groups, and glyph export.

use crate::Workspace;
use gpui::Context;
#[cfg(not(target_family = "wasm"))]
use runebender_core::formats::svg::glyph_svg;
#[cfg(not(target_family = "wasm"))]
use std::path::PathBuf;
impl Workspace {
    /// Copy the selection as text (the glyphs' characters), the web
    /// sidebar footer's action.
    pub(crate) fn command_copy_selection_text(&mut self, cx: &mut Context<Self>) {
        let Some(font) = self.font() else { return };
        let text: String = self
            .selection_names()
            .iter()
            .filter_map(|name| {
                font.name_map
                    .get(name)
                    .and_then(|&i| font.glyphs[i].codepoint)
            })
            .collect();
        if text.is_empty() {
            self.status_note = Some("Nothing encoded to copy".into());
            return;
        }
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
        self.status_note = Some(
            format!("Copied {} character{}", text.chars().count(), {
                if text.chars().count() == 1 { "" } else { "s" }
            })
            .into(),
        );
    }

    /// Add every glyph a target-bearing language filter still misses
    /// (web generateMissing): empty glyphs named and encoded from the
    /// filter's targets, in every master.
    pub(crate) fn command_generate_missing(&mut self, group: usize, filter_index: usize) {
        use runebender_core::ui::sidebar as sb;
        let Some(filter) = sb::language_groups()
            .get(group)
            .and_then(|g| g.filters.get(filter_index))
        else {
            return;
        };
        let existing: Vec<(String, Vec<u32>)> = self
            .font()
            .map(|f| {
                f.glyphs
                    .iter()
                    .map(|entry| {
                        (
                            entry.name.to_string(),
                            Self::glyph_codepoints(f, entry.name.as_ref()),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let targets: Vec<(String, u32)> = sb::missing_targets(&existing, filter)
            .into_iter()
            .map(|t| (t.name.clone(), t.unicode))
            .collect();
        if targets.is_empty() {
            return;
        }
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let upm = project.active_font().units_per_em;
        let mut added = 0usize;
        for master in project.masters.iter_mut() {
            for (name, unicode) in &targets {
                if master.name_map.contains_key(name) {
                    continue;
                }
                let mut glyph = norad::Glyph::new(name.as_str());
                glyph.width = (upm * 0.5).round();
                if let Some(c) = char::from_u32(*unicode) {
                    glyph.codepoints = norad::Codepoints::new([c]);
                }
                master.font.default_layer_mut().insert_glyph(glyph);
                master.dirty = true;
                master.modified_glyphs.insert(name.clone());
            }
            master.refresh_from_font();
        }
        added += targets.len();
        self.sidebar.counts = None;
        self.status_note = Some(
            format!(
                "Added {added} missing glyph{}",
                if added == 1 { "" } else { "s" }
            )
            .into(),
        );
    }

    /// Glyph > Duplicate Glyph: copy the current glyph (outline,
    /// components, anchors, width) to the next free name.NNN in
    /// every master, unencoded.
    pub(crate) fn command_duplicate_glyph(&mut self) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(base) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let taken: std::collections::HashSet<String> = project
            .masters
            .iter()
            .flat_map(|m| m.name_map.keys().cloned())
            .collect();
        let stem = base.split('.').next().unwrap_or(&base).to_string();
        let mut counter = 1;
        let mut name = format!("{stem}.{counter:03}");
        while taken.contains(&name) {
            counter += 1;
            name = format!("{stem}.{counter:03}");
        }
        for master in project.masters.iter_mut() {
            let Some(src) = master.font.get_glyph(base.as_str()).cloned() else {
                continue;
            };
            master.add_glyph(&name, src.width);
            if let Some(copy) = master.font.get_glyph_mut(name.as_str()) {
                copy.contours = src.contours.clone();
                copy.components = src.components.clone();
                copy.anchors = src.anchors.clone();
                copy.width = src.width;
                copy.lib = src.lib.clone();
            }
            master.dirty = true;
            master.modified_glyphs.insert(name.clone());
            master.refresh_from_font();
        }
        project.recheck_compat(&name);
        self.selected = self.font().and_then(|f| f.name_map.get(&name).copied());
        self.sidebar.counts = None;
        self.status_note = Some(format!("Duplicated {base} as {name}").into());
    }

    /// Glyph > Export Glyph as SVG: writes `<name>.svg` beside the
    /// project source (or the home directory before Save As).
    pub(crate) fn command_export_glyph_svg(&mut self) {
        #[cfg(target_family = "wasm")]
        {
            self.status_note = Some("SVG export: desktop only".into());
        }
        #[cfg(not(target_family = "wasm"))]
        {
            let Some(index) = self.current_glyph_index() else {
                return;
            };
            let Some(font) = self.font() else { return };
            let name = font.glyphs[index].name.to_string();
            let Some(glyph) = font.font.get_glyph(name.as_str()) else {
                return;
            };
            let path = runebender_core::outline::glyph_paths::glyph_to_bezpath(glyph, &font.font);
            let ascender = font.font.font_info.ascender.unwrap_or(800.0);
            let descender = font.font.font_info.descender.unwrap_or(-200.0);
            let svg = glyph_svg(&path, glyph.width, ascender, descender);
            let dir = self
                .project
                .as_ref()
                .and_then(|p| p.export_source.as_ref())
                .and_then(|p| p.parent().map(PathBuf::from))
                .unwrap_or_else(|| {
                    std::env::var("HOME")
                        .map(PathBuf::from)
                        .unwrap_or_else(|_| PathBuf::from("."))
                });
            let file = dir.join(format!("{name}.svg"));
            self.status_note = Some(match std::fs::write(&file, svg) {
                Ok(()) => format!("Wrote {}", file.display()).into(),
                Err(e) => format!("SVG export failed: {e}").into(),
            });
        }
    }

    /// Add the grid selection to a kerning group (creating it as
    /// needed), on every master. `first_side` = public.kern1, the
    /// first glyph's right edge.
    pub(crate) fn command_add_selection_to_group(&mut self, first_side: bool, group: &str) {
        let names = self.selection_names();
        if names.is_empty() {
            self.status_note = Some("Select glyphs in the grid first".into());
            return;
        }
        let prefix = if first_side {
            "public.kern1."
        } else {
            "public.kern2."
        };
        let full = format!("{prefix}{group}");
        let Ok(group_name) = norad::Name::new(&full) else {
            return;
        };
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let mut added = 0usize;
        for master in project.masters.iter_mut() {
            let members = master.font.groups.entry(group_name.clone()).or_default();
            for name in &names {
                if let Ok(member) = norad::Name::new(name)
                    && !members.contains(&member)
                {
                    members.push(member);
                    added += 1;
                }
            }
            master.dirty = true;
        }
        self.rebuild_text_models();
        self.status_note = Some(format!("@{group}: {added} membership(s) added").into());
    }

    /// Drop one glyph from a kerning group, on every master. An
    /// emptied group is removed.
    pub(crate) fn command_remove_from_group(&mut self, full_group: &str, member: &str) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        for master in project.masters.iter_mut() {
            let mut emptied = false;
            if let Some(members) = master.font.groups.get_mut(full_group) {
                members.retain(|m| m.as_str() != member);
                emptied = members.is_empty();
            }
            if emptied {
                master.font.groups.retain(|k, _| k.as_str() != full_group);
            }
            master.dirty = true;
        }
        self.rebuild_text_models();
    }

    /// Add an empty glyph to every master (bottom bar +), like
    /// Glyphs' new-glyph command, and select it.
    pub(crate) fn command_add_glyph(&mut self) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        // First free name: glyph, glyph.001, glyph.002, ...
        let taken: std::collections::HashSet<String> = project
            .masters
            .iter()
            .flat_map(|m| m.name_map.keys().cloned())
            .collect();
        let mut name = "glyph".to_string();
        let mut counter = 0;
        while taken.contains(&name) {
            counter += 1;
            name = format!("glyph.{counter:03}");
        }
        let upm = project.active_font().units_per_em;
        for master in project.masters.iter_mut() {
            master.add_glyph(&name, (upm * 0.5).round());
        }
        let name_owned = name.clone();
        project.recheck_compat(&name_owned);
        self.selected = self.font().and_then(|f| f.name_map.get(&name).copied());
        self.sidebar.counts = None;
        self.status_note = Some(format!("Added {name}").into());
    }

    /// Remove the selected glyph from every master (bottom bar −).
    pub(crate) fn command_remove_glyph(&mut self) {
        let Some(index) = self.selected else {
            self.status_note = Some("Select a glyph to remove".into());
            return;
        };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        if let Some(project) = self.project.as_mut() {
            for master in project.masters.iter_mut() {
                master.remove_glyph(&name);
            }
        }
        self.selected = None;
        self.sidebar.counts = None;
        self.status_note = Some(format!("Removed {name}").into());
    }
}
