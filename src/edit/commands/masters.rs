// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! Masters: interpolation, shape switches, smart components, axis mappings, instances, and cross-master checks.

use crate::*;

impl Workspace {
    /// Glyph > Check Joining: for every positional form, measure
    /// the connecting stroke's band at its joining edges (init and
    /// medi join at x = 0, medi and fina at x = advance), find the
    /// font's common band, and select every form that misses it —
    /// the Arabic joining-line rule, measured instead of eyeballed.
    pub(crate) fn command_check_joining(&mut self) {
        let Some(font) = self.font() else { return };
        let tolerance_edge = 2.0;
        let tolerance_band = 4.0;
        // (glyph index, name, band) per joining edge, plus the forms
        // that should join but never touch their edge.
        let mut bands: Vec<(usize, f64, f64)> = Vec::new();
        let mut broken: Vec<String> = Vec::new();
        for (i, entry) in font.glyphs.iter().enumerate() {
            let name = entry.name.as_ref();
            let (joins_left, joins_right) = if name.ends_with(".init") {
                (true, false)
            } else if name.ends_with(".medi") {
                (true, true)
            } else if name.ends_with(".fina") {
                (false, true)
            } else {
                continue;
            };
            let Some(glyph) = font.font.get_glyph(name) else {
                continue;
            };
            let outline =
                runebender_core::outline::glyph_paths::glyph_to_bezpath(glyph, &font.font);
            for left in [true, false] {
                let should = if left { joins_left } else { joins_right };
                if !should {
                    continue;
                }
                match joining_band(&outline, entry.advance, left, tolerance_edge) {
                    Some((lo, hi)) => bands.push((i, lo, hi)),
                    None => broken.push(name.to_string()),
                }
            }
        }
        if bands.is_empty() && broken.is_empty() {
            self.status_note = Some("Joining: no positional forms to check".into());
            return;
        }
        // The common band: median of the lows and highs.
        let median = |mut values: Vec<f64>| {
            values.sort_by(|a, b| a.total_cmp(b));
            values[values.len() / 2]
        };
        let med_lo = median(bands.iter().map(|(_, lo, _)| *lo).collect());
        let med_hi = median(bands.iter().map(|(_, _, hi)| *hi).collect());
        let mut off: Vec<String> = bands
            .iter()
            .filter(|(_, lo, hi)| {
                (lo - med_lo).abs() > tolerance_band || (hi - med_hi).abs() > tolerance_band
            })
            .map(|(i, _, _)| font.glyphs[*i].name.to_string())
            .collect();
        off.extend(broken.iter().cloned());
        off.sort();
        off.dedup();
        let count = off.len();
        self.multi_selected = off.into_iter().collect();
        self.selected = None;
        self.status_note = Some(
            if count == 0 {
                format!("Joining: all forms share the {med_lo:.0}–{med_hi:.0} band")
            } else {
                format!("Joining: {count} form(s) off the {med_lo:.0}–{med_hi:.0} band (selected)")
            }
            .into(),
        );
    }

    /// Glyph > Reinterpolate: rebuild the current glyph's outline
    /// in the active master from the other masters, evaluated at
    /// this master's location.
    pub(crate) fn command_reinterpolate(&mut self) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        let rebuilt = match self
            .project
            .as_ref()
            .map(|p| p.reinterpolated_from_others(&name))
        {
            Some(Ok(glyph)) => glyph,
            Some(Err(why)) => {
                self.status_note = Some(why.into());
                return;
            }
            None => return,
        };
        self.push_undo_snapshot(index);
        if let Some(font) = self.font_mut() {
            if let Some(glyph) = font.font.get_glyph_mut(name.as_str()) {
                glyph.contours = rebuilt.contours;
                glyph.width = rebuilt.width;
                font.dirty = true;
                font.modified_glyphs.insert(name.clone());
            }
            font.refresh_from_font();
        }
        self.status_note = Some(format!("{name}: reinterpolated from the other masters").into());
    }

    /// Glyph > Sync Metrics: apply every metrics key in every
    /// master. Chained keys (=n where n itself has a key) settle by
    /// repeating passes until nothing moves.
    pub(crate) fn command_sync_metrics(&mut self) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let mut adjusted = 0usize;
        for _pass in 0..5 {
            let mut moved = false;
            for master in project.masters.iter_mut() {
                let keyed: Vec<(usize, Option<String>, Option<String>)> = (0..master.glyphs.len())
                    .filter_map(|i| {
                        let glyph = master.font.get_glyph(master.glyphs[i].name.as_ref())?;
                        let l = read_metrics_key(glyph, true);
                        let r = read_metrics_key(glyph, false);
                        (l.is_some() || r.is_some()).then_some((i, l, r))
                    })
                    .collect();
                for (index, left, right) in keyed {
                    // Targets from the same master's referenced glyphs.
                    let resolve = |master: &Master,
                                   formula: &MetricsFormula,
                                   want_left: bool|
                     -> Option<f64> {
                        match formula {
                            MetricsFormula::Constant(v) => Some(*v),
                            MetricsFormula::Reference { glyph, mirror, op } => {
                                let ref_index = master.name_map.get(glyph.as_str()).copied()?;
                                let ink = master.ink_bounds(ref_index)?;
                                let advance = master.glyphs[ref_index].advance;
                                let read_left = want_left != *mirror;
                                let mut value = if read_left { ink.x0 } else { advance - ink.x1 };
                                if let Some((sign, amount)) = op {
                                    value = match sign {
                                        '+' => value + amount,
                                        '-' => value - amount,
                                        _ => value * amount,
                                    };
                                }
                                Some(value)
                            }
                        }
                    };
                    if let Some(formula) = left.as_deref().and_then(parse_metrics_key)
                        && let (Some(target), Some(ink)) =
                            (resolve(master, &formula, true), master.ink_bounds(index))
                    {
                        let delta = (target - ink.x0).round();
                        if delta != 0.0 {
                            master.shift_ink(index, delta);
                            moved = true;
                            adjusted += 1;
                        }
                    }
                    if let Some(formula) = right.as_deref().and_then(parse_metrics_key)
                        && let (Some(target), Some(ink)) =
                            (resolve(master, &formula, false), master.ink_bounds(index))
                    {
                        let advance = master.glyphs[index].advance;
                        let want = (ink.x1 + target).round();
                        if (advance - want).abs() >= 1.0 {
                            master.set_advance(index, want);
                            moved = true;
                            adjusted += 1;
                        }
                    }
                }
            }
            if !moved {
                break;
            }
        }
        self.rebuild_text_models();
        self.status_note = Some(
            if adjusted == 0 {
                "Metrics keys: everything in sync".to_string()
            } else {
                format!("Metrics keys: {adjusted} sidebearings adjusted")
            }
            .into(),
        );
    }

    /// Interpolation timing: bake an ease into a brace layer at the
    /// preview location. Positive ease means the change comes late
    /// (the light shape holds on), negative means early. Selected
    /// points ease; the rest stay on the straight interpolation, so
    /// the layer stays point-compatible. Standard designspace out —
    /// every compiler understands the result.
    pub(crate) fn command_ease_interpolation(&mut self, ease: f64) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let Some(axis) = project.axes.first().cloned() else {
            return;
        };
        if project.master_at_location().is_some() {
            self.status_note =
                Some("Move the axes off a master first: the ease bakes at that location".into());
            return;
        }
        let name = project.active_font().glyphs[index].name.to_string();
        // Where the preview sits along the axis, 0..1.
        let normalized = project.location.get(&axis.name).copied().unwrap_or(0.0);
        let design = runebender_core::document::var_model::denormalize_value(
            normalized,
            axis.min,
            axis.default,
            axis.max,
        );
        let t01 = ((design - axis.min) / (axis.max - axis.min)).clamp(0.0, 1.0);
        let gamma = (ease / 50.0).exp();
        let eased_t01 = t01.powf(gamma);
        let eased_design = axis.min + (axis.max - axis.min) * eased_t01;
        let mut eased_location = project.location.clone();
        eased_location.insert(
            axis.name.clone(),
            runebender_core::document::var_model::normalize_value(
                eased_design,
                axis.min,
                axis.default,
                axis.max,
            ),
        );
        let here = project.interpolated_norad_glyph(&name);
        let eased = project.interpolated_at(&name, &eased_location);
        let (Some(mut merged), Some(eased)) = (here, eased) else {
            self.status_note = Some("Ease needs compatible masters".into());
            return;
        };
        // Merge: selected points take the eased position.
        let selected = self.editor.selected.clone();
        let all = selected.is_empty();
        for (ci, contour) in merged.contours.iter_mut().enumerate() {
            for (pi, point) in contour.points.iter_mut().enumerate() {
                if !all && !selected.contains(&(ci, pi)) {
                    continue;
                }
                let Some(src) = eased.contours.get(ci).and_then(|c| c.points.get(pi)) else {
                    continue;
                };
                point.x = src.x;
                point.y = src.y;
            }
        }
        self.command_brace_layer_with(Some(merged));
        self.status_note = Some(
            format!(
                "Ease {ease:+.0} baked at {} {design:.0} (t {:.2} → {:.2})",
                axis.tag, t01, eased_t01
            )
            .into(),
        );
    }

    /// Add a shape switch (bracket layer): an unencoded `.bold`
    /// alternate copied into every master, plus a designspace rule
    /// substituting it from `at` up to the end of the first axis.
    /// The repo convention (DESIGN.md): design the alternate in the
    /// Regular master; the copies start red.
    pub(crate) fn command_add_shape_switch(&mut self, at: f64) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(project) = self.project.as_mut() else {
            return;
        };
        if project.ds_doc.is_none() {
            self.status_note = Some("Shape switches need a designspace project".into());
            return;
        }
        let Some(axis) = project.axes.first().cloned() else {
            return;
        };
        if !(axis.min..=axis.max).contains(&at) {
            self.status_note = Some(
                format!(
                    "Switch point outside {} {}–{}",
                    axis.tag, axis.min, axis.max
                )
                .into(),
            );
            return;
        }
        let name = project.active_font().glyphs[index].name.to_string();
        let alt = format!("{name}.bold");
        let (Ok(sub_name), Ok(sub_with)) = (norad::Name::new(&name), norad::Name::new(&alt)) else {
            return;
        };
        for master in project.masters.iter_mut() {
            if master.name_map.contains_key(&alt) {
                continue;
            }
            let Some(source) = master.font.get_glyph(name.as_str()).cloned() else {
                continue;
            };
            let mut copy = norad::Glyph::new(alt.as_str());
            copy.width = source.width;
            copy.contours = source.contours.clone();
            copy.components = source.components.clone();
            copy.anchors = source.anchors.clone();
            // Unencoded, and red: a placeholder awaiting its design
            // (the repo's lane-2 convention).
            runebender_core::ui::theme_oklch::set_glyph_mark(&mut copy, Some("red"));
            master.font.default_layer_mut().insert_glyph(copy);
            master.dirty = true;
            master.modified_glyphs.insert(alt.clone());
            master.refresh_from_font();
        }
        let doc = project.ds_doc.as_mut().expect("checked above");
        doc.rules.processing = norad::designspace::RuleProcessing::Last;
        let exists = doc.rules.rules.iter().any(|rule| {
            rule.substitutions
                .iter()
                .any(|sub| sub.name.as_str() == name)
        });
        if !exists {
            doc.rules.rules.push(norad::designspace::Rule {
                name: Some(format!("{name} bold")),
                condition_sets: vec![norad::designspace::ConditionSet {
                    conditions: vec![norad::designspace::Condition {
                        name: axis.name.clone(),
                        minimum: Some(at as f32),
                        maximum: Some(axis.max as f32),
                    }],
                }],
                substitutions: vec![norad::designspace::Substitution {
                    name: sub_name,
                    with: sub_with,
                }],
            });
        }
        project.ds_dirty = true;
        project.compute_compat();
        self.sidebar_counts = None;
        self.status_note =
            Some(format!("{name} switches to {alt} at {} ≥ {at:.0}", axis.tag).into());
    }

    /// Drop the rule that substitutes this glyph (the alternates
    /// stay; delete them like any glyph).
    pub(crate) fn command_remove_shape_switch(&mut self) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let Some(name) = project
            .active_font()
            .glyphs
            .get(index)
            .map(|g| g.name.to_string())
        else {
            return;
        };
        let Some(doc) = project.ds_doc.as_mut() else {
            return;
        };
        let before = doc.rules.rules.len();
        doc.rules.rules.retain(|rule| {
            !rule
                .substitutions
                .iter()
                .any(|sub| sub.name.as_str() == name)
        });
        if doc.rules.rules.len() != before {
            project.ds_dirty = true;
            self.status_note = Some(format!("Shape switch removed for {name}").into());
        }
    }

    /// "Smart Axis" on a part glyph: "Width,0,100" writes the
    /// glyphsLib smartComponentAxes entry, marks the default glyph
    /// as the bottom pole, and seeds a part.top layer copy marked
    /// as the top — edit it through the swap arrows, place the part
    /// with a value through the Selection panel.
    pub(crate) fn command_make_smart_axis(&mut self, text: &str) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let mut parts = text.split(',').map(str::trim);
        let Some(name) = parts.next().filter(|n| !n.is_empty()) else {
            return;
        };
        let bottom = parts
            .next()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0);
        let top = parts
            .next()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(100.0);
        let Some(glyph_name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        if let Some(font) = self.font_mut() {
            let Some(source) = font.font.get_glyph(glyph_name.as_str()).cloned() else {
                return;
            };
            // The axis entry, glyphsLib-shaped.
            let mut axis = plist::Dictionary::new();
            axis.insert("name".into(), plist::Value::String(name.into()));
            axis.insert("bottomName".into(), plist::Value::String(String::new()));
            axis.insert("bottomValue".into(), plist::Value::Real(bottom));
            axis.insert("topName".into(), plist::Value::String(String::new()));
            axis.insert("topValue".into(), plist::Value::Real(top));
            if let Some(glyph) = font.font.get_glyph_mut(glyph_name.as_str()) {
                glyph.lib.insert(
                    "com.schriftgestaltung.Glyphs.smartComponentAxes".into(),
                    plist::Value::Array(vec![plist::Value::Dictionary(axis)]),
                );
            }
            // Seed the top pole as a layer copy, marked.
            if let Ok(layer) = font.font.layers.get_or_create_layer("part.top") {
                let mut copy = norad::Glyph::new(glyph_name.as_str());
                copy.width = source.width;
                copy.contours = source.contours.clone();
                let mut pole = plist::Dictionary::new();
                pole.insert(name.to_string(), plist::Value::Integer(2u64.into()));
                copy.lib.insert(
                    "com.runebender.partSelection".into(),
                    plist::Value::Dictionary(pole),
                );
                layer.insert_glyph(copy);
            }
            font.dirty = true;
            font.modified_glyphs.insert(glyph_name.clone());
        }
        self.visible_glyph_layers.insert("part.top".into());
        self.status_note = Some(
            format!(
                "{glyph_name} is a smart part: {name} {bottom:.0}–{top:.0} · edit part.top via the swap arrows"
            )
            .into(),
        );
    }

    /// Set the selected smart component's value on its first axis
    /// (the Selection panel's Smart field).
    pub(crate) fn command_set_smart_value(&mut self, text: &str) {
        let Mode::Editor(index) = self.mode else {
            return;
        };
        let Some(ci) = self.editor.selected_component else {
            return;
        };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        // "30" moves the first axis; "Height=30" names one.
        let (axis_named, value) = match text.split_once('=') {
            Some((axis, v)) => (Some(axis.trim()), v.trim().parse::<f64>()),
            None => (None, text.trim().parse::<f64>()),
        };
        let Ok(value) = value else {
            self.status_note = Some("Smart value: 30 or Height=30".into());
            return;
        };
        let axis = self.font().and_then(|f| {
            let glyph = f.font.get_glyph(name.as_str())?;
            let base = f.font.get_glyph(glyph.components.get(ci)?.base.as_str())?;
            let names: Vec<String> = base
                .lib
                .get("com.schriftgestaltung.Glyphs.smartComponentAxes")?
                .as_array()?
                .iter()
                .filter_map(|a| {
                    a.as_dictionary()?
                        .get("name")?
                        .as_string()
                        .map(str::to_string)
                })
                .collect();
            match axis_named {
                Some(want) => names.iter().find(|n| n.eq_ignore_ascii_case(want)).cloned(),
                None => names.first().cloned(),
            }
        });
        let Some(axis) = axis else {
            self.status_note =
                Some("Selected component's base is not smart (or no such axis)".into());
            return;
        };
        if let Some(font) = self.font_mut() {
            let component_count = font
                .font
                .get_glyph(name.as_str())
                .map(|g| g.components.len())
                .unwrap_or(0);
            if let Some(glyph) = font.font.get_glyph_mut(name.as_str()) {
                const KEY: &str = "com.schriftgestaltung.Glyphs.componentsSmartComponentValues";
                let mut rows: Vec<plist::Value> = glyph
                    .lib
                    .get(KEY)
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                rows.resize(
                    component_count,
                    plist::Value::Dictionary(plist::Dictionary::new()),
                );
                if let Some(entry) = rows.get_mut(ci).and_then(|v| v.as_dictionary_mut()) {
                    entry.insert(axis.clone(), plist::Value::Real(value));
                }
                glyph.lib.insert(KEY.into(), plist::Value::Array(rows));
                font.dirty = true;
            }
            font.modified_glyphs.insert(name.clone());
            font.refresh_from_font();
        }
        self.status_note = Some(format!("{axis} = {value:.0} on the component").into());
    }

    /// Add (or replace) one avar mapping pair on the first axis:
    /// user-space input → design-space output, written into the
    /// designspace and saved with it.
    pub(crate) fn command_add_axis_mapping(&mut self, input: f32, output: f32) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let Some(doc) = project.ds_doc.as_mut() else {
            self.status_note = Some("Axis mappings need a designspace project".into());
            return;
        };
        let Some(axis) = doc.axes.first_mut() else {
            return;
        };
        let map = axis.map.get_or_insert_with(Vec::new);
        map.retain(|m| (m.input - input).abs() > 0.01);
        map.push(norad::designspace::AxisMapping { input, output });
        map.sort_by(|a, b| a.input.total_cmp(&b.input));
        project.ds_dirty = true;
        self.status_note = Some(format!("Axis map: {input:.0} → {output:.0}").into());
    }

    pub(crate) fn command_remove_axis_mapping(&mut self, index: usize) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let Some(doc) = project.ds_doc.as_mut() else {
            return;
        };
        let Some(axis) = doc.axes.first_mut() else {
            return;
        };
        if let Some(map) = axis.map.as_mut()
            && index < map.len()
        {
            map.remove(index);
            if map.is_empty() {
                axis.map = None;
            }
            project.ds_dirty = true;
        }
    }

    /// Enter in the Instances field: rename the instance sitting at
    /// the preview location, or add a new one there. The name is the
    /// style name; the full name follows the family.
    pub(crate) fn command_instance_upsert(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let Some(project) = self.project.as_mut() else {
            return;
        };
        if project.ds_doc.is_none() {
            self.status_note = Some("Instances need a designspace project".into());
            return;
        }
        // The preview location in design coordinates, one value per
        // axis, computed before the document is borrowed.
        let wants: Vec<(String, f64)> = project
            .axes
            .iter()
            .map(|axis| {
                let normalized = project.location.get(&axis.name).copied().unwrap_or(0.0);
                let raw = runebender_core::document::var_model::denormalize_value(
                    normalized,
                    axis.min,
                    axis.default,
                    axis.max,
                );
                (axis.name.clone(), raw)
            })
            .collect();
        let defaults: std::collections::HashMap<String, f64> = project
            .axes
            .iter()
            .map(|a| (a.name.clone(), a.default))
            .collect();
        let family = project
            .masters
            .first()
            .and_then(|m| m.font.font_info.family_name.clone());
        let doc = project.ds_doc.as_mut().expect("checked above");
        let at_location = |inst: &norad::designspace::Instance| {
            wants.iter().all(|(axis, want)| {
                let got = inst
                    .location
                    .iter()
                    .find(|d| d.name == *axis)
                    .and_then(|d| d.xvalue.or(d.uservalue))
                    .map(|v| v as f64)
                    .or_else(|| defaults.get(axis).copied())
                    .unwrap_or(0.0);
                (got - want).abs() < 0.5
            })
        };
        let note = match doc.instances.iter().position(at_location) {
            Some(i) => {
                let inst = &mut doc.instances[i];
                inst.stylename = Some(name.to_string());
                if let Some(fam) = &family {
                    inst.name = Some(format!("{fam} {name}"));
                    let (map_family, map_style) = Self::style_linking(fam, name);
                    inst.stylemapfamilyname = Some(map_family);
                    inst.stylemapstylename = Some(map_style);
                }
                format!("Renamed instance to {name}")
            }
            None => {
                let stylemap = family.as_ref().map(|fam| Self::style_linking(fam, name));
                doc.instances.push(norad::designspace::Instance {
                    familyname: family.clone(),
                    stylename: Some(name.to_string()),
                    name: family.as_ref().map(|f| format!("{f} {name}")),
                    stylemapfamilyname: stylemap.as_ref().map(|(f, _)| f.clone()),
                    stylemapstylename: stylemap.as_ref().map(|(_, st)| st.clone()),
                    location: wants
                        .iter()
                        .map(|(axis, value)| norad::designspace::Dimension {
                            name: axis.clone(),
                            xvalue: Some(*value as f32),
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                });
                format!("Added instance {name}")
            }
        };
        project.ds_dirty = true;
        project.refresh_instances_from_doc();
        self.status_note = Some(note.into());
    }

    /// Drop one instance (the × on its row). Saved with the font.
    pub(crate) fn command_instance_delete(&mut self, index: usize) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let Some(doc) = project.ds_doc.as_mut() else {
            return;
        };
        if index < doc.instances.len() {
            doc.instances.remove(index);
            project.ds_dirty = true;
            project.refresh_instances_from_doc();
        }
    }
}
