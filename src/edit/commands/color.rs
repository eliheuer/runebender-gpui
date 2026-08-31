// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! Color: palettes, color layers, COLRv1, gradients.

use crate::Workspace;
use runebender_core::formats::color_font::COLOR_LAYERS_EXPLICIT_KEY;
use runebender_core::formats::color_font::has_v1_entry;
use runebender_core::formats::color_font::linear_gradient_paint;
use runebender_core::formats::color_font::paint_glyph_layer;
use runebender_core::formats::color_font::paint_solid;
use runebender_core::formats::color_font::parse_hex_color;
use runebender_core::formats::color_font::read_color_mapping;
use runebender_core::formats::color_font::read_color_palette;
use runebender_core::formats::color_font::write_color_mapping;
use runebender_core::formats::color_font::write_color_palette;
impl Workspace {
    /// Append a hex color to the palette, on every master (CPAL
    /// palettes must agree across sources). Returns true on success.
    pub(crate) fn command_add_palette_color(&mut self, hex: &str) -> bool {
        let Some(color) = parse_hex_color(hex) else {
            self.status_note = Some("Color: use #RRGGBB or #RRGGBBAA".into());
            return false;
        };
        let Some(project) = self.project.as_mut() else {
            return false;
        };
        for master in project.masters.iter_mut() {
            let mut palette = read_color_palette(&master.font);
            palette.push(color);
            write_color_palette(&mut master.font, &palette);
            master.dirty = true;
        }
        true
    }

    /// Drop a palette color. Refused while a layer still uses it,
    /// because CPAL indices shift on removal.
    pub(crate) fn command_remove_palette_color(&mut self, index: usize) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let used = project
            .masters
            .first()
            .map(|m| read_color_mapping(&m.font))
            .unwrap_or_default()
            .iter()
            .any(|(_, ci)| *ci == index);
        if used {
            self.status_note = Some("Color is used by a layer · remove the layer first".into());
            return;
        }
        for master in project.masters.iter_mut() {
            let mut palette = read_color_palette(&master.font);
            if index < palette.len() {
                palette.remove(index);
                write_color_palette(&mut master.font, &palette);
                master.dirty = true;
            }
        }
        // Higher indices shifted down: follow them in the mapping.
        for master in project.masters.iter_mut() {
            let mut mapping = read_color_mapping(&master.font);
            for (_, ci) in mapping.iter_mut() {
                if *ci > index {
                    *ci -= 1;
                }
            }
            write_color_mapping(&mut master.font, &mapping);
        }
        if self.color_selected >= index && self.color_selected > 0 {
            self.color_selected -= 1;
        }
    }

    /// Add a color layer: a UFO layer named color.N mapped to the
    /// selected palette color, appended on top. The open glyph's
    /// outline is copied in as a starting point; edit it through the
    /// Glyph Layers swap arrows, drawing per master like any layer.
    pub(crate) fn command_add_color_layer(&mut self) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(project) = self.project.as_ref() else {
            return;
        };
        if read_color_palette(&project.active_font().font).is_empty() {
            self.status_note = Some("Add a palette color first (hex field)".into());
            return;
        }
        let name = project.active_font().glyphs[index].name.to_string();
        let color = self.color_selected;
        let Some(project) = self.project.as_mut() else {
            return;
        };
        // First free color.N name across the mapping.
        let mapping = project
            .masters
            .first()
            .map(|m| read_color_mapping(&m.font))
            .unwrap_or_default();
        let mut n = 0usize;
        let layer_name = loop {
            let candidate = format!("color.{n}");
            if !mapping.iter().any(|(l, _)| *l == candidate) {
                break candidate;
            }
            n += 1;
        };
        for master in project.masters.iter_mut() {
            let mut mapping = read_color_mapping(&master.font);
            mapping.push((layer_name.clone(), color));
            write_color_mapping(&mut master.font, &mapping);
            // Seed the layer with this master's outline of the glyph.
            let seed = master.font.get_glyph(name.as_str()).cloned();
            if let (Some(seed), Ok(layer)) =
                (seed, master.font.layers.get_or_create_layer(&layer_name))
            {
                let mut copy = norad::Glyph::new(name.as_str());
                copy.width = seed.width;
                copy.contours = seed.contours.clone();
                copy.components = seed.components.clone();
                layer.insert_glyph(copy);
            }
            master.dirty = true;
            master.modified_glyphs.insert(name.clone());
        }
        self.show_color_preview = true;
        self.status_note = Some(format!("Color layer {layer_name} added").into());
    }

    /// Remove one mapping row (the UFO layer and its drawings stay).
    pub(crate) fn command_remove_color_layer(&mut self, row: usize) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        for master in project.masters.iter_mut() {
            let mut mapping = read_color_mapping(&master.font);
            if row < mapping.len() {
                mapping.remove(row);
                write_color_mapping(&mut master.font, &mapping);
                master.dirty = true;
            }
        }
    }

    /// Color section's "To v1" button: explode every color glyph's
    /// layers into real suffixed glyphs and write the explicit
    /// colorLayers structures (solid paints), the COLRv1 baseline.
    /// From here ufo2ft's own exploding is off; gradients upgrade
    /// individual paints.
    pub(crate) fn command_convert_to_colrv1(&mut self) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let mut exploded = 0usize;
        for master in project.masters.iter_mut() {
            let mapping = read_color_mapping(&master.font);
            if mapping.is_empty() {
                continue;
            }
            // Which glyphs have color-layer copies at all.
            let color_glyphs: Vec<String> = master
                .font
                .default_layer()
                .iter()
                .map(|g| g.name().to_string())
                .filter(|name| {
                    !name.contains(".color.")
                        && mapping.iter().any(|(layer, _)| {
                            master
                                .font
                                .layers
                                .get(layer)
                                .is_some_and(|l| l.contains_glyph(name))
                        })
                })
                .collect();
            let mut layers_dict = plist::Dictionary::new();
            for name in &color_glyphs {
                let mut rows: Vec<plist::Value> = Vec::new();
                for (layer, color) in &mapping {
                    let Some(copy) = master
                        .font
                        .layers
                        .get(layer)
                        .and_then(|l| l.get_glyph(name.as_str()))
                        .cloned()
                    else {
                        continue;
                    };
                    let suffixed = format!("{name}.{layer}");
                    if master.font.get_glyph(suffixed.as_str()).is_none() {
                        let mut real = norad::Glyph::new(suffixed.as_str());
                        real.width = copy.width;
                        real.contours = copy.contours.clone();
                        real.components = copy.components.clone();
                        master.font.default_layer_mut().insert_glyph(real);
                    }
                    rows.push(paint_glyph_layer(&suffixed, paint_solid(*color)));
                }
                if !rows.is_empty() {
                    let mut root = plist::Dictionary::new();
                    root.insert("Format".into(), plist::Value::Integer(1u64.into()));
                    root.insert("Layers".into(), plist::Value::Array(rows));
                    layers_dict.insert(name.clone(), plist::Value::Dictionary(root));
                    exploded += 1;
                }
            }
            if !layers_dict.is_empty() {
                master.font.lib.insert(
                    COLOR_LAYERS_EXPLICIT_KEY.into(),
                    plist::Value::Dictionary(layers_dict),
                );
                master.dirty = true;
            }
            master.refresh_from_font();
        }
        self.sidebar.counts = None;
        self.status_note = Some(
            if exploded == 0 {
                "No color layers to convert".to_string()
            } else {
                format!("COLRv1: {exploded} glyph entr(ies) written")
            }
            .into(),
        );
    }

    /// Turn one of the selected glyph's color layers into a linear
    /// gradient: from the row's color at the baseline to the
    /// selected swatch at the ascender. Runs the v1 conversion
    /// first when needed.
    pub(crate) fn command_layer_gradient(&mut self, row: usize) {
        let Some(index) = self.current_glyph_index() else {
            return;
        };
        let Some(name) = self.font().map(|f| f.glyphs[index].name.to_string()) else {
            return;
        };
        if !self.font().is_some_and(|f| has_v1_entry(&f.font, &name)) {
            self.command_convert_to_colrv1();
        }
        let stop1 = self.color_selected;
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let mut changed = 0usize;
        for master in project.masters.iter_mut() {
            let (ascender, mapping) = (master.ascender, read_color_mapping(&master.font));
            let Some((_, stop0)) = mapping.get(row) else {
                continue;
            };
            let paint = linear_gradient_paint(*stop0, stop1, (0.0, 0.0), (0.0, ascender));
            let Some(layer) = master
                .font
                .lib
                .get_mut(COLOR_LAYERS_EXPLICIT_KEY)
                .and_then(|v| v.as_dictionary_mut())
                .and_then(|d| d.get_mut(name.as_str()))
                .and_then(|v| v.as_dictionary_mut())
                .and_then(|root| root.get_mut("Layers"))
                .and_then(|v| v.as_array_mut())
                .and_then(|layers| layers.get_mut(row))
                .and_then(|v| v.as_dictionary_mut())
            else {
                continue;
            };
            layer.insert("Paint".into(), paint);
            changed += 1;
            master.dirty = true;
        }
        self.status_note = Some(
            if changed == 0 {
                "Gradient: convert to v1 first (To v1)".to_string()
            } else {
                format!("Layer {row}: linear gradient to color {stop1} in {changed} master(s)")
            }
            .into(),
        );
    }
}
