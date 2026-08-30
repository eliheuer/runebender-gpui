// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! The right panel's fields, and what typing in them does to the font.
//!
//! One `apply_*` per field writes the value through, and one
//! `refresh_*` per group reads the font back into the fields, so a
//! field never holds state the font does not.

use super::*;

impl Workspace {
    /// Non-default, non-background layers of the active master that
    /// hold a copy of `name`.
    pub(crate) fn glyph_layer_names(font: &norad::Font, name: &str) -> Vec<String> {
        font.layers
            .iter()
            .filter(|l| !l.is_default())
            .filter(|l| {
                let ln = l.name().as_str();
                ln != "public.background" && ln != "background"
            })
            .filter(|l| l.contains_glyph(name))
            .map(|l| l.name().to_string())
            .collect()
    }

    /// Set a metrics key on the selected glyph (every master keeps
    /// the same key; the values differ per master when synced).
    pub(crate) fn apply_metrics_key(&mut self, left: bool, text: &str) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let name = project.active_font().glyphs[index].name.to_string();
        let text = text.trim().to_string();
        if !text.is_empty() && parse_metrics_key(&text).is_none() {
            self.status_note = Some("Metrics key: =glyph, =|glyph, =glyph+10, or =50".into());
            return;
        }
        for master in project.masters.iter_mut() {
            if let Some(glyph) = master.font.get_glyph_mut(name.as_str()) {
                write_metrics_key(glyph, left, &text);
                master.dirty = true;
                master.modified_glyphs.insert(name.clone());
            }
        }
        self.command_sync_metrics();
    }

    /// The background layer we read: public.background first, then
    /// RoboFont's conventional plain "background".
    pub(crate) fn background_layer_name(font: &norad::Font) -> Option<String> {
        for candidate in ["public.background", "background"] {
            if font.layers.get(candidate).is_some() {
                return Some(candidate.to_string());
            }
        }
        None
    }

    pub(crate) fn apply_metric(&mut self, which: MetricField, value: f64) {
        // A grid multi-selection batch-edits: the typed value lands
        // on every selected glyph, the Glyphs list-edit behavior.
        // No undo for the batch yet — undo is single-glyph.
        let batch: Vec<usize> = if matches!(self.mode, Mode::Grid) && self.multi_selected.len() > 1
        {
            let Some(font) = self.font() else { return };
            self.multi_selected
                .iter()
                .filter_map(|name| font.name_map.get(name).copied())
                .collect()
        } else {
            let Some(index) = (match self.mode {
                Mode::Editor(index) => Some(index),
                Mode::Grid => self.selected,
            }) else {
                return;
            };
            self.push_undo_snapshot(index);
            vec![index]
        };
        let count = batch.len();
        let Some(font) = self.font_mut() else {
            return;
        };
        for index in batch {
            let ink = font.ink_bounds(index);
            let advance = font.glyphs[index].advance;
            match which {
                MetricField::Width => font.set_advance(index, value.round()),
                MetricField::Lsb => {
                    if let Some(ink) = ink {
                        // Move the ink; the right sidebearing absorbs it.
                        font.shift_ink(index, (value - ink.x0).round());
                    }
                }
                MetricField::Rsb => {
                    if let Some(ink) = ink {
                        font.set_advance(index, (ink.x1 + value).round());
                    } else {
                        font.set_advance(index, (advance + value).round());
                    }
                }
            }
        }
        if count > 1 {
            self.status_note = Some(
                format!(
                    "{} set on {count} glyphs",
                    match which {
                        MetricField::Width => "Width",
                        MetricField::Lsb => "LSB",
                        MetricField::Rsb => "RSB",
                    }
                )
                .into(),
            );
        }
    }

    /// Set (or clear) the selected glyph's note in the active master.
    pub(crate) fn apply_glyph_note(&mut self, text: &str) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        let text = text.trim();
        if let Some(font) = self.font_mut()
            && let Some(glyph) = font.font.get_glyph_mut(name.as_str())
        {
            let new = (!text.is_empty()).then(|| text.to_string());
            if glyph.note != new {
                glyph.note = new;
                font.dirty = true;
                font.modified_glyphs.insert(name);
            }
        }
    }

    /// Set or clear the glyph's production (export) name in every
    /// master's public.postscriptNames mapping.
    pub(crate) fn apply_glyph_production(&mut self, text: &str) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let name = project.active_font().glyphs[index].name.to_string();
        for master in project.masters.iter_mut() {
            if write_production_name(&mut master.font, &name, text.trim()) {
                master.dirty = true;
            }
        }
    }

    /// Rename the selected glyph in every master, updating components,
    /// groups, kerning, and the open text session.
    pub(crate) fn apply_glyph_rename(&mut self, new_name: &str) {
        let Some(index) = self.selected else { return };
        let Some(old) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        let new_name = new_name.trim().to_string();
        if new_name.is_empty() || new_name == old {
            return;
        }
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let mut renamed = false;
        for master in project.masters.iter_mut() {
            if runebender_core::outline::glyph_ops::rename_glyph(&mut master.font, &old, &new_name)
            {
                master.dirty = true;
                master.kerning_dirty = true;
                master.modified_glyphs.remove(&old);
                master.modified_glyphs.insert(new_name.clone());
                master.refresh_from_font();
                renamed = true;
            }
        }
        if !renamed {
            self.status_note = Some(format!("Cannot rename {old} to {new_name}").into());
            return;
        }
        project.compat.remove(&old);
        let recheck = new_name.clone();
        project.recheck_compat(&recheck);
        // Parked tabs on the renamed glyph follow it.
        for slot in &mut self.sessions {
            if slot.glyph_name == old {
                slot.glyph_name = new_name.clone();
            }
        }
        // The open text session keeps working under the new name.
        for i in 0..self.edit_buffer.len() {
            let matches_old = self
                .edit_buffer
                .sort(i)
                .and_then(|s| s.glyph_name())
                .is_some_and(|n| n == old);
            if matches_old {
                let (codepoint, advance) = self
                    .font()
                    .and_then(|f| f.name_map.get(&new_name).copied())
                    .and_then(|g| {
                        self.font()
                            .map(|f| (f.glyphs[g].codepoint, f.glyphs[g].advance))
                    })
                    .unwrap_or((None, 0.0));
                self.edit_buffer
                    .update_glyph(i, new_name.clone(), codepoint, advance);
            }
        }
        self.sidebar_counts = None;
        self.remap_glyph_indices(&new_name);
        self.status_note = Some(format!("Renamed {old} → {new_name}").into());
    }

    /// Set the selected glyph's unicode in every master ("0041",
    /// "U+0041", "0x41"; empty clears).
    pub(crate) fn apply_glyph_unicode(&mut self, text: &str) {
        let Some(index) = self.selected else { return };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let mut ok = false;
        for master in project.masters.iter_mut() {
            if let Some(glyph_index) = master.name_map.get(&name).copied() {
                let changed = master
                    .edit_glyph(glyph_index, |g| {
                        runebender_core::outline::glyph_ops::set_glyph_unicode(g, text)
                    })
                    .unwrap_or(false);
                if changed {
                    master.refresh_from_font();
                    ok = true;
                }
            }
        }
        if !ok {
            self.status_note = Some(format!("Bad unicode: {text}").into());
            return;
        }
        self.sidebar_counts = None;
        self.rebuild_text_models();
        self.remap_glyph_indices(&name);
    }

    /// Set the selected glyph's kerning group on one side, in every
    /// master (groups.plist; empty clears).
    pub(crate) fn apply_kern_group(&mut self, first_side: bool, text: &str) {
        let Some(index) = self.selected else { return };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        let Some(project) = self.project.as_mut() else {
            return;
        };
        for master in project.masters.iter_mut() {
            if runebender_core::outline::glyph_ops::set_kern_group(
                &mut master.font,
                &name,
                first_side,
                text,
            ) {
                master.dirty = true;
                master.kerning_dirty = true;
            }
        }
        self.rebuild_text_models();
    }

    /// Fill the Glyph panel's editable fields from the selected glyph
    /// unless one of them is being typed in.
    pub(crate) fn refresh_glyph_inputs(
        &mut self,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !force && window.focused(cx).is_some_and(|f| f != self.focus_handle) {
            return;
        }
        let Some(index) = self.selected else { return };
        let Some(font) = self.font() else { return };
        let Some(entry) = font.glyphs.get(index) else {
            return;
        };
        let name = entry.name.to_string();
        let unicode = entry
            .codepoint
            .map(|c| format!("{:04X}", c as u32))
            .unwrap_or_default();
        let group_l = runebender_core::outline::glyph_ops::kern_group(&font.font, &name, true)
            .map(|g| g.as_str().replace("public.kern1.", ""))
            .unwrap_or_default();
        let group_r = runebender_core::outline::glyph_ops::kern_group(&font.font, &name, false)
            .map(|g| g.as_str().replace("public.kern2.", ""))
            .unwrap_or_default();
        let set = |entity: &gpui::Entity<widgets::input::InputState>,
                   value: String,
                   window: &mut Window,
                   cx: &mut Context<Self>| {
            entity.update(cx, |st, cx| {
                if st.value() != value.as_str() {
                    st.set_value(value, window, cx);
                }
            });
        };
        let note = font
            .font
            .get_glyph(name.as_str())
            .and_then(|g| g.note.clone())
            .unwrap_or_default();
        let (lkey, rkey) = font
            .font
            .get_glyph(name.as_str())
            .map(|g| {
                (
                    read_metrics_key(g, true).unwrap_or_default(),
                    read_metrics_key(g, false).unwrap_or_default(),
                )
            })
            .unwrap_or_default();
        let name_input = self.glyph_inputs.name.clone();
        let unicode_input = self.glyph_inputs.unicode.clone();
        let l_input = self.glyph_inputs.group_l.clone();
        let r_input = self.glyph_inputs.group_r.clone();
        let note_input = self.glyph_inputs.note.clone();
        let lkey_input = self.glyph_inputs.lsb_key.clone();
        let rkey_input = self.glyph_inputs.rsb_key.clone();
        let production = read_production_name(&font.font, name.as_str()).unwrap_or_default();
        let production_input = self.glyph_inputs.production.clone();
        set(&name_input, name, window, cx);
        set(&unicode_input, unicode, window, cx);
        set(&l_input, group_l, window, cx);
        set(&r_input, group_r, window, cx);
        set(&note_input, note, window, cx);
        set(&lkey_input, lkey, window, cx);
        set(&rkey_input, rkey, window, cx);
        set(&production_input, production, window, cx);
    }

    /// Auto-generated feature blocks from glyph names, the Glyphs
    /// conventions: `.init`/`.medi`/`.fina` suffixes feed the
    /// positional features, and underscore names (f_i, beh-ar_lam-ar)
    /// whose parts all exist feed liga. Returns (tag, body) pairs;
    /// tags with nothing to say are omitted.
    pub(crate) fn generated_feature_blocks(font: &norad::Font) -> Vec<(String, String)> {
        let names: std::collections::BTreeSet<&str> = font
            .default_layer()
            .iter()
            .map(|g| g.name().as_str())
            .collect();
        let mut blocks: Vec<(String, String)> = Vec::new();
        for tag in ["init", "medi", "fina"] {
            let suffix = format!(".{tag}");
            let mut rules = String::new();
            for name in &names {
                let Some(base) = name.strip_suffix(suffix.as_str()) else {
                    continue;
                };
                if names.contains(base) {
                    rules.push_str(&format!("    sub {base} by {name};\n"));
                }
            }
            if !rules.is_empty() {
                blocks.push((tag.to_string(), rules));
            }
        }
        // Cursive attachment: glyphs carrying entry/exit anchors
        // (the Glyphs cascade workflow) feed a curs feature —
        // position cursive <glyph> <entry> <exit>, NULL where a
        // side is missing.
        {
            let mut rules = String::new();
            for glyph in font.default_layer().iter() {
                let mut entry: Option<(f64, f64)> = None;
                let mut exit: Option<(f64, f64)> = None;
                for anchor in &glyph.anchors {
                    match anchor.name.as_ref().map(|n| n.as_str()) {
                        Some("entry") => entry = Some((anchor.x, anchor.y)),
                        Some("exit") => exit = Some((anchor.x, anchor.y)),
                        _ => {}
                    }
                }
                if entry.is_none() && exit.is_none() {
                    continue;
                }
                let fmt = |a: Option<(f64, f64)>| match a {
                    Some((x, y)) => format!("<anchor {x:.0} {y:.0}>"),
                    None => "<anchor NULL>".to_string(),
                };
                rules.push_str(&format!(
                    "    position cursive {} {} {};\n",
                    glyph.name(),
                    fmt(entry),
                    fmt(exit),
                ));
            }
            if !rules.is_empty() {
                let body = format!("    lookupflag RightToLeft IgnoreMarks;\n{rules}");
                blocks.push(("curs".to_string(), body));
            }
        }
        // Mark positioning (mark + mkmk) from anchors, the way
        // Fontra emulates it live: every anchor family X with marks
        // carrying _X gets a markClass; bases with X position them,
        // marks that also carry X stack them. The shaped preview
        // then places vowel marks exactly as the compiled font will.
        {
            use std::collections::BTreeMap;
            // anchor name -> (marks: name, _X pos), (bases: name, X pos),
            // (mark carriers: name, X pos).
            let mut families: BTreeMap<String, AnchorFamily> = BTreeMap::new();
            for glyph in font.default_layer().iter() {
                let is_mark_glyph = glyph
                    .anchors
                    .iter()
                    .any(|a| a.name.as_ref().is_some_and(|n| n.as_str().starts_with('_')));
                for anchor in &glyph.anchors {
                    let Some(name) = anchor.name.as_ref().map(|n| n.as_str()) else {
                        continue;
                    };
                    if name == "entry" || name == "exit" {
                        continue;
                    }
                    if let Some(base_name) = name.strip_prefix('_') {
                        families.entry(base_name.to_string()).or_default().0.push((
                            glyph.name().to_string(),
                            anchor.x,
                            anchor.y,
                        ));
                    } else {
                        let entry = families.entry(name.to_string()).or_default();
                        let record = (glyph.name().to_string(), anchor.x, anchor.y);
                        if is_mark_glyph {
                            entry.2.push(record);
                        } else {
                            entry.1.push(record);
                        }
                    }
                }
            }
            let mut mark_rules = String::new();
            let mut mkmk_rules = String::new();
            let mut classes = String::new();
            for (family, (marks, bases, carriers)) in &families {
                if marks.is_empty() || (bases.is_empty() && carriers.is_empty()) {
                    continue;
                }
                for (mark, x, y) in marks {
                    classes.push_str(&format!(
                        "    markClass {mark} <anchor {x:.0} {y:.0}> @MC_{family};\n"
                    ));
                }
                for (base, x, y) in bases {
                    mark_rules.push_str(&format!(
                        "    pos base {base} <anchor {x:.0} {y:.0}> mark @MC_{family};\n"
                    ));
                }
                for (carrier, x, y) in carriers {
                    mkmk_rules.push_str(&format!(
                        "    pos mark {carrier} <anchor {x:.0} {y:.0}> mark @MC_{family};\n"
                    ));
                }
            }
            if !mark_rules.is_empty() {
                blocks.push(("mark".to_string(), format!("{classes}{mark_rules}")));
            }
            if !mkmk_rules.is_empty() {
                let body = if mark_rules.is_empty() {
                    format!("{classes}{mkmk_rules}")
                } else {
                    // Classes already defined in the mark block.
                    mkmk_rules.clone()
                };
                blocks.push(("mkmk".to_string(), body));
            }
        }
        // Composition (ccmp): a composite-only glyph whose
        // components all exist, with at least one combining mark
        // after the base, substitutes from its parts — edit the base
        // and the mark once, the composed form follows (the
        // composition-first workflow). Longest sequences first.
        {
            let is_mark = |name: &str| {
                font.default_layer()
                    .get_glyph(name)
                    .and_then(|g| g.codepoints.iter().next())
                    .is_some_and(|c| {
                        matches!(
                            runebender_core::analysis::category::GlyphCategory::from_codepoint(c),
                            runebender_core::analysis::category::GlyphCategory::Mark
                        )
                    })
            };
            let mut compositions: Vec<(String, Vec<String>)> = font
                .default_layer()
                .iter()
                .filter(|g| {
                    g.contours.is_empty() && g.components.len() >= 2 && !g.name().contains('.')
                })
                .filter_map(|g| {
                    let parts: Vec<String> =
                        g.components.iter().map(|c| c.base.to_string()).collect();
                    (parts.iter().all(|p| names.contains(p.as_str()))
                        && parts[1..].iter().any(|p| is_mark(p)))
                    .then(|| (g.name().to_string(), parts))
                })
                .collect();
            compositions.sort_by_key(|(_, parts)| std::cmp::Reverse(parts.len()));
            if !compositions.is_empty() {
                let mut rules = String::new();
                for (name, parts) in compositions {
                    rules.push_str(&format!("    sub {} by {name};\n", parts.join(" ")));
                }
                blocks.push(("ccmp".to_string(), rules));
            }
        }
        // Ligatures: longest first, so f_f_i wins over f_f.
        let mut ligatures: Vec<(&str, Vec<&str>)> = names
            .iter()
            .filter(|name| name.contains('_') && !name.contains('.'))
            .filter_map(|name| {
                let parts: Vec<&str> = name.split('_').collect();
                (parts.len() >= 2 && parts.iter().all(|part| names.contains(part)))
                    .then_some((*name, parts))
            })
            .collect();
        ligatures.sort_by_key(|(_, parts)| std::cmp::Reverse(parts.len()));
        if !ligatures.is_empty() {
            let mut rules = String::new();
            for (name, parts) in ligatures {
                rules.push_str(&format!("    sub {} by {name};\n", parts.join(" ")));
            }
            blocks.push(("liga".to_string(), rules));
        }
        // Ligature caret positions: caret_1, caret_2... anchors on a
        // ligature give editing carets between its parts (GDEF
        // LigatureCaretByPos), the Glyphs anchor convention.
        let mut caret_rules = String::new();
        for glyph in font.default_layer().iter() {
            let mut carets: Vec<(u32, f64)> = glyph
                .anchors
                .iter()
                .filter_map(|a| {
                    let n = a
                        .name
                        .as_ref()?
                        .as_str()
                        .strip_prefix("caret_")?
                        .parse::<u32>()
                        .ok()?;
                    Some((n, a.x))
                })
                .collect();
            if carets.is_empty() {
                continue;
            }
            carets.sort_by_key(|(n, _)| *n);
            let positions: Vec<String> = carets.iter().map(|(_, x)| format!("{x:.0}")).collect();
            caret_rules.push_str(&format!(
                "    LigatureCaretByPos {} {};
",
                glyph.name(),
                positions.join(" ")
            ));
        }
        if !caret_rules.is_empty() {
            blocks.push(("table GDEF".to_string(), caret_rules));
        }
        blocks
    }

    /// Replace (or append) one `feature X { … } X;` block in a fea
    /// source. The terminator `} X;` is required syntax, so the block
    /// span is found textually.
    pub(crate) fn replace_feature_block(fea: &str, tag: &str, body: &str) -> String {
        // A tag of "table GDEF" replaces a table block instead;
        // both share the `} NAME;` terminator grammar.
        let (open, close, block) = match tag.strip_prefix("table ") {
            Some(name) => (
                format!("table {name} "),
                format!("}} {name};"),
                format!("table {name} {{\n{body}}} {name};\n"),
            ),
            None => (
                format!("feature {tag} "),
                format!("}} {tag};"),
                format!("feature {tag} {{\n{body}}} {tag};\n"),
            ),
        };
        if let (Some(start), Some(end)) = (fea.find(&open), fea.find(&close))
            && end > start
        {
            let mut out = String::with_capacity(fea.len());
            out.push_str(&fea[..start]);
            out.push_str(block.trim_end());
            out.push_str(&fea[end + close.len()..]);
            return out;
        }
        // New block. An insertion marker (Fontra's convention, one
        // line reading "# Automatic Code") controls where generated
        // code lands among hand-written blocks: each new block goes
        // in just above the marker, so call order is kept and the
        // marker stays for the next Generate.
        for (offset, line) in fea.lines().map({
            let mut pos = 0usize;
            move |line| {
                let at = pos;
                pos += line.len() + 1;
                (at, line)
            }
        }) {
            if line.trim() == "# Automatic Code" {
                let mut out = String::with_capacity(fea.len() + block.len());
                out.push_str(&fea[..offset]);
                out.push_str(&block);
                out.push('\n');
                out.push_str(&fea[offset..]);
                return out;
            }
        }
        let mut out = fea.trim_end().to_string();
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&block);
        out
    }

    /// Compile-check a features.fea against the active master's
    /// glyph set, the same build the text engine shapes with.
    pub(crate) fn check_features_compile(font: &Master, fea: &str) -> Result<(), String> {
        use runebender_core::text::shape::{ShapingFont, ShapingGlyph, ShapingSource};
        let glyphs: Vec<ShapingGlyph> = std::iter::once(ShapingGlyph {
            name: ".notdef".into(),
            advance: 0.0,
            unicodes: Vec::new(),
        })
        .chain(
            font.glyphs
                .iter()
                .filter(|g| g.name.as_ref() != ".notdef")
                .map(|g| ShapingGlyph {
                    name: g.name.to_string(),
                    advance: g.advance,
                    unicodes: g.codepoint.map(|c| c as u32).into_iter().collect(),
                }),
        )
        .collect();
        ShapingFont::build(&ShapingSource {
            units_per_em: font.units_per_em,
            glyphs,
            features: fea.to_string(),
        })
        .map(|_| ())
    }

    /// Push the active master's features.fea into the editor. Hands
    /// off while it holds unapplied edits or focus, unless forced.
    pub(crate) fn refresh_features_input(
        &mut self,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !force
            && (self.features_edited || window.focused(cx).is_some_and(|f| f != self.focus_handle))
        {
            return;
        }
        let Some(font) = self.font() else { return };
        let value = font.font.features.clone();
        self.features_input.update(cx, |st, cx| {
            if st.value() != value.as_str() {
                st.set_value(value, window, cx);
            }
        });
    }

    /// Commit the Kerning section's editor row: set (or update) the
    /// pair on the active master. First and second may be glyph names
    /// or group names (public.kern1./public.kern2.).
    pub(crate) fn apply_kern_pair(&mut self, first: &str, second: &str, value: f64) {
        let (Ok(first), Ok(second)) = (norad::Name::new(first), norad::Name::new(second)) else {
            self.status_note = Some("Kerning: invalid name".into());
            return;
        };
        if let Some(font) = self.font_mut() {
            font.font
                .kerning
                .entry(first)
                .or_default()
                .insert(second, value);
            font.kerning_dirty = true;
            font.dirty = true;
        }
        self.rebuild_text_models();
    }

    /// Remove one kerning pair from the active master.
    pub(crate) fn delete_kern_pair(&mut self, first: &str, second: &str) {
        if let Some(font) = self.font_mut() {
            let mut emptied = false;
            if let Some(seconds) = font.font.kerning.get_mut(first) {
                seconds.retain(|name, _| name.as_str() != second);
                emptied = seconds.is_empty();
            }
            if emptied {
                font.font.kerning.retain(|name, _| name.as_str() != first);
            }
            font.kerning_dirty = true;
            font.dirty = true;
        }
        self.rebuild_text_models();
    }

    /// Commit one Font Info field (Enter in the Font Info section).
    /// The family name is font-wide and lands on every master; style
    /// and the metrics belong to the active master.
    pub(crate) fn apply_font_info(&mut self, field: FontInfoField, text: &str) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let text = text.trim();
        match field {
            FontInfoField::Family => {
                if text.is_empty() {
                    return;
                }
                for master in project.masters.iter_mut() {
                    master.font.font_info.family_name = Some(text.to_string());
                    master.dirty = true;
                }
            }
            FontInfoField::Style => {
                if text.is_empty() {
                    return;
                }
                let active = project.active;
                let master = &mut project.masters[active];
                master.font.font_info.style_name = Some(text.to_string());
                master.dirty = true;
                project.master_names[active] = text.to_string().into();
            }
            FontInfoField::BlueValues
            | FontInfoField::OtherBlues
            | FontInfoField::StemsH
            | FontInfoField::StemsV => {
                // Comma or space separated numbers; empty clears.
                let values: Vec<f64> = text
                    .split([',', ' '])
                    .filter(|part| !part.trim().is_empty())
                    .filter_map(|part| part.trim().parse::<f64>().ok())
                    .collect();
                let stored = (!values.is_empty()).then_some(values);
                let master = &mut project.masters[project.active];
                let info = &mut master.font.font_info;
                match field {
                    FontInfoField::BlueValues => info.postscript_blue_values = stored,
                    FontInfoField::OtherBlues => info.postscript_other_blues = stored,
                    FontInfoField::StemsH => info.postscript_stem_snap_h = stored,
                    _ => info.postscript_stem_snap_v = stored,
                }
                master.dirty = true;
            }
            _ => {
                let Ok(v) = text.parse::<f64>() else { return };
                let master = &mut project.masters[project.active];
                let info = &mut master.font.font_info;
                match field {
                    FontInfoField::Upm => {
                        let Ok(upm) = norad::fontinfo::NonNegativeIntegerOrFloat::try_from(v)
                        else {
                            return;
                        };
                        info.units_per_em = Some(upm);
                        master.units_per_em = v;
                    }
                    FontInfoField::ItalicAngle => info.italic_angle = Some(v),
                    FontInfoField::Ascender => {
                        info.ascender = Some(v);
                        master.ascender = v;
                    }
                    FontInfoField::Descender => {
                        info.descender = Some(v);
                        master.descender = v;
                    }
                    FontInfoField::XHeight => {
                        info.x_height = Some(v);
                        master.x_height = Some(v);
                    }
                    FontInfoField::CapHeight => {
                        info.cap_height = Some(v);
                        master.cap_height = Some(v);
                    }
                    FontInfoField::TypoAscender => {
                        info.open_type_os2_typo_ascender = Some(v as i32)
                    }
                    FontInfoField::TypoDescender => {
                        info.open_type_os2_typo_descender = Some(v as i32)
                    }
                    FontInfoField::TypoLineGap => info.open_type_os2_typo_line_gap = Some(v as i32),
                    FontInfoField::HheaAscender => info.open_type_hhea_ascender = Some(v as i32),
                    FontInfoField::HheaDescender => info.open_type_hhea_descender = Some(v as i32),
                    FontInfoField::HheaLineGap => info.open_type_hhea_line_gap = Some(v as i32),
                    FontInfoField::WinAscent => {
                        if v >= 0.0 {
                            info.open_type_os2_win_ascent = Some(v as u32)
                        }
                    }
                    FontInfoField::WinDescent => {
                        // winDescent is stored positive.
                        if v >= 0.0 {
                            info.open_type_os2_win_descent = Some(v as u32)
                        }
                    }
                    FontInfoField::Family
                    | FontInfoField::Style
                    | FontInfoField::BlueValues
                    | FontInfoField::OtherBlues
                    | FontInfoField::StemsH
                    | FontInfoField::StemsV => unreachable!(),
                }
                master.dirty = true;
            }
        }
    }

    /// Push the active master's font info into the section's inputs.
    /// Skipped while any input is focused, unless `force`, the same
    /// contract as `refresh_metric_inputs`.
    pub(crate) fn refresh_font_info_inputs(
        &mut self,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !force && window.focused(cx).is_some_and(|f| f != self.focus_handle) {
            return;
        }
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let master = &project.masters[project.active];
        let info = &master.font.font_info;
        let opt = |v: Option<f64>| v.map(|v| format!("{v:.0}")).unwrap_or_default();
        let list = |v: &Option<Vec<f64>>| {
            v.as_ref()
                .map(|values| {
                    values
                        .iter()
                        .map(|n| format!("{n:.0}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default()
        };
        let values = [
            (
                &self.font_info_inputs.family,
                info.family_name.clone().unwrap_or_default(),
            ),
            (
                &self.font_info_inputs.style,
                info.style_name.clone().unwrap_or_default(),
            ),
            (
                &self.font_info_inputs.upm,
                format!("{:.0}", master.units_per_em),
            ),
            (
                &self.font_info_inputs.italic_angle,
                info.italic_angle
                    .map(|v| format!("{v}"))
                    .unwrap_or_default(),
            ),
            (
                &self.font_info_inputs.ascender,
                format!("{:.0}", master.ascender),
            ),
            (
                &self.font_info_inputs.descender,
                format!("{:.0}", master.descender),
            ),
            (&self.font_info_inputs.x_height, opt(master.x_height)),
            (&self.font_info_inputs.cap_height, opt(master.cap_height)),
            (
                &self.font_info_inputs.typo_asc,
                info.open_type_os2_typo_ascender
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            ),
            (
                &self.font_info_inputs.typo_desc,
                info.open_type_os2_typo_descender
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            ),
            (
                &self.font_info_inputs.typo_gap,
                info.open_type_os2_typo_line_gap
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            ),
            (
                &self.font_info_inputs.hhea_asc,
                info.open_type_hhea_ascender
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            ),
            (
                &self.font_info_inputs.hhea_desc,
                info.open_type_hhea_descender
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            ),
            (
                &self.font_info_inputs.hhea_gap,
                info.open_type_hhea_line_gap
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            ),
            (
                &self.font_info_inputs.win_asc,
                info.open_type_os2_win_ascent
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            ),
            (
                &self.font_info_inputs.win_desc,
                info.open_type_os2_win_descent
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            ),
            (
                &self.font_info_inputs.blue_values,
                list(&info.postscript_blue_values),
            ),
            (
                &self.font_info_inputs.other_blues,
                list(&info.postscript_other_blues),
            ),
            (
                &self.font_info_inputs.stems_h,
                list(&info.postscript_stem_snap_h),
            ),
            (
                &self.font_info_inputs.stems_v,
                list(&info.postscript_stem_snap_v),
            ),
        ];
        for (entity, value) in values {
            entity.update(cx, |st, cx| {
                if st.value() != value.as_str() {
                    st.set_value(value, window, cx);
                }
            });
        }
    }

    /// Measured stem and bar of a glyph: the narrowest horizontal
    /// and vertical black spans between facing straight edges.
    /// (Counters are white spans; the midpoint containment test
    /// keeps only ink.)
    pub(crate) fn measured_dimensions(&self, name: &str) -> (Option<i64>, Option<i64>) {
        use kurbo::Shape as _;
        use runebender_core::analysis::measure::{self, MeasureKind};
        use runebender_core::outline::path::hyper_model::Contour as WContour;
        let Some(font) = self.font() else {
            return (None, None);
        };
        let Some(g) = font.font.get_glyph(name) else {
            return (None, None);
        };
        if g.contours.is_empty() {
            return (None, None);
        }
        let paths: Vec<runebender_core::outline::path::Path> = g
            .contours
            .iter()
            .map(|c| runebender_core::outline::path::Path::from_contour(&WContour::from_norad(c)))
            .collect();
        let filled = runebender_core::outline::glyph_paths::glyph_to_bezpath(g, &font.font);
        let black = |m: &measure::Measurement| {
            let mid = kurbo::Point::new((m.a.x + m.b.x) / 2.0, (m.a.y + m.b.y) / 2.0);
            filled.contains(mid)
        };
        let measurements = measure::glyph_measurements(&paths);
        let narrowest = |kind: MeasureKind| {
            measurements
                .iter()
                .filter(|m| m.kind == kind)
                .filter(|m| black(m))
                .map(|m| m.length)
                .min()
        };
        (
            narrowest(MeasureKind::Horizontal),
            narrowest(MeasureKind::Vertical),
        )
    }

    pub(crate) fn refresh_metric_inputs(
        &mut self,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The metric fields live in the Glyph panel, which is up in
        // both modes: in the grid they follow the selected cell.
        let Some(index) = (match self.mode {
            Mode::Editor(index) => Some(index),
            Mode::Grid => self.selected,
        }) else {
            return;
        };
        if !force {
            // Any focused element other than the workspace canvas
            // means an input might be active: leave the text alone.
            if window.focused(cx).is_some_and(|f| f != self.focus_handle) {
                return;
            }
        }
        let Some(font) = self.font() else {
            return;
        };
        let advance = font.glyphs[index].advance;
        let ink = font.ink_bounds(index);
        let (lsb, rsb) = match ink {
            Some(r) => (format!("{:.0}", r.x0), format!("{:.0}", advance - r.x1)),
            None => (String::new(), String::new()),
        };
        let width = format!("{advance:.0}");
        let set = |entity: &gpui::Entity<widgets::input::InputState>,
                   value: String,
                   window: &mut Window,
                   cx: &mut Context<Self>| {
            entity.update(cx, |st, cx| {
                if st.value() != value.as_str() {
                    st.set_value(value, window, cx);
                }
            });
        };
        set(&self.metric_inputs.width, width, window, cx);
        set(&self.metric_inputs.lsb, lsb, window, cx);
        set(&self.metric_inputs.rsb, rsb, window, cx);
    }

    /// Set one coordinate of the single selected point (Selection
    /// section X/Y inputs), with an undo snapshot.
    /// Rename the selected anchor (Enter in the Selection panel).
    pub(crate) fn apply_anchor_name(&mut self, text: &str) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        let Some(ai) = self.editor.selected_anchor() else {
            return;
        };
        let name = text.trim();
        if name.is_empty() {
            return;
        }
        let Ok(name) = norad::Name::new(name) else {
            self.status_note = Some(format!("Bad anchor name: {text}").into());
            return;
        };
        self.push_undo_snapshot(index);
        self.font_mut().and_then(|f| {
            f.edit_glyph(index, |g| {
                if let Some(anchor) = g.anchors.get_mut(ai) {
                    anchor.name = Some(name);
                }
            })
        });
    }

    /// Move whatever is selected so the quadrant reference lands on
    /// `value` along one axis (web move_selection_reference).
    pub(crate) fn apply_coord(&mut self, is_x: bool, value: f64) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        if !value.is_finite() {
            return;
        }
        let Some(bounds) = self.selection_bounds() else {
            return;
        };
        let reference = self.coord_quadrant.point_in_dspace_rect(bounds);
        let delta = if is_x {
            kurbo::Vec2::new(value - reference.x, 0.0)
        } else {
            kurbo::Vec2::new(0.0, value - reference.y)
        };
        if delta.hypot() < 1e-9 {
            return;
        }
        self.push_undo_snapshot(index);
        let changed = self.translate_selected(index, delta);
        if !changed {
            self.editor.undo.pop();
        }
    }

    /// Scale whatever is selected about the quadrant reference so its
    /// bounds reach `value` along one axis (web
    /// resize_selection_reference).
    pub(crate) fn apply_size(&mut self, is_width: bool, value: f64) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        if !value.is_finite() || value <= 0.0 {
            return;
        }
        let Some(bounds) = self.selection_bounds() else {
            return;
        };
        let current = if is_width {
            bounds.width()
        } else {
            bounds.height()
        };
        if current.abs() < 1e-9 {
            return;
        }
        let reference = self.coord_quadrant.point_in_dspace_rect(bounds);
        let scale = value / current;
        if (scale - 1.0).abs() < 1e-9 {
            return;
        }
        let (sx, sy) = if is_width { (scale, 1.0) } else { (1.0, scale) };
        let transform = Affine::translate(-reference.to_vec2())
            .then_scale_non_uniform(sx, sy)
            .then_translate(reference.to_vec2());
        self.editor.last_transform = Some(transform);
        self.push_undo_snapshot(index);
        let changed = self.transform_selected(index, transform);
        if !changed {
            self.editor.undo.pop();
        }
    }

    /// Keep the Selection X/Y inputs showing the selected point.
    pub(crate) fn refresh_coord_inputs(
        &mut self,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !force && window.focused(cx).is_some_and(|f| f != self.focus_handle) {
            return;
        }
        let (x, y, w, h) = match self.selection_bounds() {
            Some(bounds) => {
                let reference = self.coord_quadrant.point_in_dspace_rect(bounds);
                (
                    format!("{:.0}", reference.x),
                    format!("{:.0}", reference.y),
                    format!("{:.0}", bounds.width()),
                    format!("{:.0}", bounds.height()),
                )
            }
            None => Default::default(),
        };
        let anchor_name = self
            .editor
            .selected_anchor()
            .and_then(|ai| {
                let Mode::Editor(index) = self.mode else {
                    return None;
                };
                self.font()
                    .and_then(|f| f.glyphs[index].anchors.get(ai).cloned())
            })
            .map(|(name, _, _)| name.to_string())
            .unwrap_or_default();
        for (entity, value) in [
            (self.metric_inputs.x.clone(), x),
            (self.metric_inputs.y.clone(), y),
            (self.metric_inputs.w.clone(), w),
            (self.metric_inputs.h.clone(), h),
            (self.anchor_name_input.clone(), anchor_name),
        ] {
            entity.update(cx, |st, cx| {
                if st.value() != value.as_str() {
                    st.set_value(value, window, cx);
                }
            });
        }
    }

    /// Google Fonts style linking for an instance name: RIBBI styles
    /// link inside the family; anything else becomes its own
    /// stylemap family with regular/italic, the shape gftools
    /// expects (Medium → "Family Medium" + regular).
    pub(crate) fn style_linking(family: &str, style: &str) -> (String, String) {
        match style.to_lowercase().as_str() {
            "regular" | "bold" | "italic" | "bold italic" => {
                (family.to_string(), style.to_lowercase())
            }
            lower => {
                if let Some(base) = lower.strip_suffix(" italic").map(|b| b.len()) {
                    (
                        format!("{family} {}", style[..base].trim()),
                        "italic".to_string(),
                    )
                } else {
                    (format!("{family} {style}"), "regular".to_string())
                }
            }
        }
    }
}
