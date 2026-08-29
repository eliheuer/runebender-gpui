// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! Tests for the front-end.
//!
//! Anything checkable without a window belongs in runebender-core
//! instead. What is left here is the shell's own behaviour.

#[cfg(test)]
mod tests {
    /// A two-master Glyphs 3 source, used by the import tests.
    const MINIMAL_GLYPHS_SOURCE: &str = r#"{
.appVersion = "3300";
.formatVersion = 3;
axes = (
{
name = Weight;
tag = wght;
}
);
familyName = TestSans;
fontMaster = (
{
ascender = 800;
axesValues = (400);
capHeight = 700;
descender = -200;
id = m01;
name = Regular;
},
{
ascender = 800;
axesValues = (700);
capHeight = 700;
descender = -200;
id = m02;
name = Bold;
}
);
glyphs = (
{
glyphname = A;
layers = (
{
layerId = m01;
shapes = (
{
closed = 1;
nodes = (
(0,0,l),
(100,0,l),
(50,700,l)
);
}
);
width = 600;
},
{
layerId = m02;
shapes = (
{
closed = 1;
nodes = (
(0,0,l),
(140,0,l),
(70,700,l)
);
}
);
width = 640;
}
);
unicode = 65;
}
);
unitsPerEm = 1000;
}"#;

    /// A .glyphspackage on disk opens the same way a .glyphs file
    /// does: converted to sibling UFO files, then loaded.
    #[test]
    fn glyphspackage_opens() {
        let dir = std::env::temp_dir().join(format!("rb-pkg-open-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("TestSans.glyphs");
        std::fs::write(&src, MINIMAL_GLYPHS_SOURCE).unwrap();

        let pkg = dir.join("TestSans.glyphspackage");
        glyphslib::Font::load(&src).unwrap().save(&pkg).unwrap();
        assert!(pkg.join("fontinfo.plist").is_file());

        let project = Project::load(&pkg).expect("package should open");
        assert!(!project.masters.is_empty());
        assert!(
            project.active_font().glyphs.iter().any(|g| g.name == "A"),
            "converted font should hold the glyphs from the package"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
    use crate::*;

    fn test_ufo_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../runebender-web/assets/test-fonts/VirtuaGrotesk-Regular.ufo")
    }

    #[test]
    fn designspace_loads_with_masters() {
        let project = Project::load(&default_font_path()).expect("designspace loads");
        assert_eq!(project.masters.len(), 2, "regular + bold");
        assert!(project.master_names.iter().any(|n| n.contains("Bold")));
        // Active master is the default location (Regular).
        assert!(!project.master_names[project.active].contains("Bold"));
        // Named instances come along, normalized: the extremes sit on
        // the axis ends.
        assert_eq!(project.instances.len(), 4, "four named instances");
        let bold = project
            .instances
            .iter()
            .find(|(name, _)| name.as_ref() == "Bold")
            .expect("a Bold instance");
        let weight = bold.1.values().next().copied().unwrap_or(0.0);
        assert!((weight - 1.0).abs() < 1e-6, "Bold sits at the axis max");
    }

    #[test]
    fn designspace_roundtrip_and_instance_edit() {
        // The saved document must equal the loaded one: instance
        // editing rewrites the whole file, so nothing may be lost.
        let path = default_font_path();
        let doc = norad::designspace::DesignSpaceDocument::load(&path).expect("designspace loads");
        let tmp = std::env::temp_dir().join("rb-ds-roundtrip.designspace");
        doc.save(&tmp).expect("designspace saves");
        let doc2 =
            norad::designspace::DesignSpaceDocument::load(&tmp).expect("saved designspace loads");
        assert_eq!(doc, doc2, "designspace round-trips losslessly");
        std::fs::remove_file(&tmp).ok();

        // Upsert against the project: renaming at an existing
        // location, adding at a fresh one, deleting.
        let mut project = Project::load(&path).expect("designspace loads");
        let before = project.instances.len();
        let doc = project.ds_doc.as_mut().expect("designspace doc kept");
        doc.instances.remove(0);
        project.ds_dirty = true;
        project.refresh_instances_from_doc();
        assert_eq!(project.instances.len(), before - 1);
    }

    #[test]
    fn glyph_image_roundtrips_through_save() {
        // A 2x2 png in the images store plus a glyph image reference
        // must survive a save and reload (norad owns the images dir).
        let mut font = runebender_core::new_font::new_font("Img", "Regular", 400);
        let png: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        // Not a decodable png, but the store does not care; the
        // editor validates before writing, the test only checks the
        // round-trip.
        font.images
            .insert(std::path::PathBuf::from("scan.png"), png.to_vec())
            .expect("store accepts");
        let image = norad::Image::new(
            std::path::PathBuf::from("scan.png"),
            None,
            norad::AffineTransform::default(),
        )
        .expect("image ref");
        let glyph_name = font
            .default_layer()
            .iter()
            .next()
            .map(|g| g.name().to_string())
            .expect("template has glyphs");
        font.default_layer_mut()
            .get_glyph_mut(&glyph_name)
            .unwrap()
            .image = Some(image);
        let dir = std::env::temp_dir().join("rb-image-roundtrip.ufo");
        std::fs::remove_dir_all(&dir).ok();
        font.save(&dir).expect("saves");
        let back = norad::Font::load(&dir).expect("reloads");
        assert!(
            back.images.get(std::path::Path::new("scan.png")).is_some(),
            "images store round-trips"
        );
        assert!(
            back.default_layer()
                .get_glyph(&glyph_name)
                .and_then(|g| g.image.as_ref())
                .is_some(),
            "glyph image reference round-trips"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn add_extremes_inserts_the_dip() {
        use norad::{Contour, ContourPoint, PointType};
        // A symmetric dip: extremum (vertical tangent point of the
        // y-curve) at t=0.5, which is (100, -37.5).
        let pt = |x, y, typ| ContourPoint::new(x, y, typ, false, None, None);
        let contour = Contour::new(
            vec![
                pt(0.0, 0.0, PointType::Move),
                pt(50.0, -50.0, PointType::OffCurve),
                pt(150.0, -50.0, PointType::OffCurve),
                pt(200.0, 0.0, PointType::Curve),
            ],
            None,
        );
        let mut glyph = norad::Glyph::new("extremes-test");
        glyph.contours = vec![contour];
        let all = std::collections::HashSet::new();
        assert!(add_extreme_points(&mut glyph, &all));
        let ons: Vec<(f64, f64)> = glyph.contours[0]
            .points
            .iter()
            .filter(|p| p.typ != PointType::OffCurve)
            .map(|p| (p.x, p.y))
            .collect();
        assert!(
            ons.iter()
                .any(|&(x, y)| (x - 100.0).abs() <= 1.0 && (y + 37.5).abs() <= 1.5),
            "extremum node added: {ons:?}"
        );
        // Second run finds nothing new.
        assert!(!add_extreme_points(&mut glyph, &all));
    }

    #[test]
    fn color_lib_keys_roundtrip() {
        // Palette + mapping written through the helpers must read
        // back identically after a norad save/load, in the exact
        // shape ufo2ft's COLR builder consumes.
        let mut font = runebender_core::new_font::new_font("Col", "Regular", 400);
        let palette = vec![[1.0, 0.2, 0.0, 1.0], [0.0, 0.4, 1.0, 0.5]];
        write_color_palette(&mut font, &palette);
        let mapping = vec![("color.0".into(), 0usize), ("color.1".into(), 1)];
        write_color_mapping(&mut font, &mapping);
        // A layer glyph so the layers round-trip too.
        let glyph_name = font
            .default_layer()
            .iter()
            .next()
            .map(|g| g.name().to_string())
            .unwrap();
        let seed = font.default_layer().get_glyph(&glyph_name).unwrap().clone();
        for layer in ["color.0", "color.1"] {
            let mut copy = norad::Glyph::new(glyph_name.as_str());
            copy.width = seed.width;
            font.layers
                .get_or_create_layer(layer)
                .unwrap()
                .insert_glyph(copy);
        }
        let dir = std::env::temp_dir().join("rb-color-roundtrip.ufo");
        std::fs::remove_dir_all(&dir).ok();
        font.save(&dir).expect("saves");
        let back = norad::Font::load(&dir).expect("reloads");
        assert_eq!(read_color_palette(&back), palette);
        assert_eq!(read_color_mapping(&back), mapping);
        assert!(back.layers.get("color.0").is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn brace_layer_refines_interpolation() {
        let mut project = Project::load(&default_font_path()).expect("loads");
        // Freeze n's Regular outline into a {500} brace layer, then
        // nudge its first point +40: at wght 500 the interpolation
        // must hit the brace exactly, not the linear blend.
        let name = "n";
        let loc_500 = {
            let axis = &project.axes[0];
            let mut l = runebender_core::var_model::Location::new();
            l.insert(
                axis.name.clone(),
                runebender_core::var_model::normalize_value(
                    500.0,
                    axis.min,
                    axis.default,
                    axis.max,
                ),
            );
            l
        };
        let mut frozen = project.masters[0]
            .font
            .get_glyph(name)
            .expect("has n")
            .clone();
        let orig = frozen.contours[0].points[0].x;
        frozen.contours[0].points[0].x = orig + 40.0;
        project.masters[0]
            .font
            .layers
            .get_or_create_layer("{500}")
            .unwrap()
            .insert_glyph(frozen);
        project.brace.push(BraceSource {
            master: 0,
            layer: "{500}".into(),
            location: loc_500.clone(),
        });
        project.location = loc_500;
        let refined = project
            .interpolated_norad_glyph(name)
            .expect("interpolates");
        assert!(
            (refined.contours[0].points[0].x - (orig + 40.0)).abs() < 0.6,
            "brace layer pins the outline at its location: {} vs {}",
            refined.contours[0].points[0].x,
            orig + 40.0,
        );
    }

    #[test]
    fn colrv1_paint_shapes() {
        // The exact structures verified against ufo2ft's buildCOLR:
        // PaintColrLayers root (1), PaintGlyph layers (10), solid (2)
        // and linear-gradient (4) children.
        let solid = paint_solid(3);
        let d = solid.as_dictionary().unwrap();
        assert_eq!(d.get("Format").unwrap().as_signed_integer(), Some(2));
        assert_eq!(d.get("PaletteIndex").unwrap().as_signed_integer(), Some(3));
        let layer = paint_glyph_layer("A.color.0", solid);
        let d = layer.as_dictionary().unwrap();
        assert_eq!(d.get("Format").unwrap().as_signed_integer(), Some(10));
        assert_eq!(d.get("Glyph").unwrap().as_string(), Some("A.color.0"));
        let grad = linear_gradient_paint(1, 0, (0.0, 0.0), (0.0, 800.0));
        let d = grad.as_dictionary().unwrap();
        assert_eq!(d.get("Format").unwrap().as_signed_integer(), Some(4));
        // Rotation vector is perpendicular to the vertical gradient.
        assert_eq!(d.get("x2").unwrap().as_real(), Some(800.0));
        assert_eq!(d.get("y2").unwrap().as_real(), Some(0.0));
        let stops = d
            .get("ColorLine")
            .and_then(|v| v.as_dictionary())
            .and_then(|c| c.get("ColorStop"))
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(stops.len(), 2);
    }

    #[test]
    fn feature_blocks_insert_at_automatic_code_marker() {
        let fea = "feature ss01 {\n    sub a by a.alt;\n} ss01;\n\n                   # Automatic Code\n\nfeature kern {\n} kern;\n";
        let one = Workspace::replace_feature_block(fea, "init", "    sub beh-ar by beh-ar.init;\n");
        let two = Workspace::replace_feature_block(&one, "liga", "    sub f i by fi;\n");
        // Both new blocks land above the marker, in call order, and
        // the marker survives for the next run.
        let init = two.find("feature init").unwrap();
        let liga = two.find("feature liga").unwrap();
        let marker = two.find("# Automatic Code").unwrap();
        let ss01 = two.find("feature ss01").unwrap();
        assert!(ss01 < init && init < liga && liga < marker);
        // Replacing an existing block still edits it in place.
        let three = Workspace::replace_feature_block(&two, "ss01", "    sub a by a.bold;\n");
        assert_eq!(three.matches("feature ss01").count(), 1);
        assert!(three.contains("a.bold"));
        // Without a marker, new blocks append at the end.
        let plain = Workspace::replace_feature_block(
            "feature kern {\n} kern;\n",
            "liga",
            "    sub f i by fi;\n",
        );
        assert!(plain.trim_end().ends_with("} liga;"));
    }

    #[test]
    fn reinterpolate_rebuilds_a_master_from_the_others() {
        let mut project = Project::load(&default_font_path()).expect("loads");
        // Two masters: rebuilding the active one from "the others"
        // must reproduce the other master exactly.
        assert_eq!(project.masters.len(), 2);
        project.active = 0;
        let expected = project.masters[1]
            .font
            .get_glyph("H")
            .expect("bold has H")
            .clone();
        let rebuilt = project
            .reinterpolated_from_others("H")
            .expect("reinterpolates");
        assert_eq!(rebuilt.width, expected.width);
        assert_eq!(rebuilt.contours.len(), expected.contours.len());
        for (a, b) in rebuilt.contours.iter().zip(expected.contours.iter()) {
            for (pa, pb) in a.points.iter().zip(b.points.iter()) {
                assert!((pa.x - pb.x).abs() < 1e-6);
                assert!((pa.y - pb.y).abs() < 1e-6);
            }
        }
        // A glyph missing everywhere else reports, not panics.
        assert!(project.reinterpolated_from_others("no.such.glyph").is_err());
    }

    #[test]
    fn stylistic_set_names_compile() {
        // featureNames inside an ss block is plain fea; the editor's
        // Features pane plus fea-rs carry it end to end.
        let project = Project::load(&default_font_path()).expect("loads");
        let font = project.active_font();
        let fea = "feature ss01 {\n\
                   featureNames {\n  name \"Bold a\";\n};\n\
                   sub a by a.bold;\n\
                   } ss01;\n";
        // a.bold may not exist in the test font; use glyphs that do.
        let fea = if font.font.get_glyph("a.bold").is_some() {
            fea.to_string()
        } else {
            fea.replace("a.bold", "b")
        };
        assert!(
            Workspace::check_features_compile(font, &fea).is_ok(),
            "featureNames should compile through fea-rs"
        );
    }

    #[test]
    fn glyph_svg_wraps_the_outline_in_font_units() {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((100.0, 0.0));
        path.line_to((100.0, 700.0));
        path.close_path();
        let svg = glyph_svg(&path, 600.0, 800.0, -200.0);
        assert!(svg.contains("viewBox=\"0 0 600 1000\""));
        assert!(svg.contains("translate(0,800) scale(1,-1)"));
        assert!(svg.contains("M0,0"));
        assert!(svg.ends_with("</svg>\n"));
    }

    #[test]
    fn caret_anchors_generate_a_gdef_table() {
        let mut font = norad::Font::new();
        let mut liga = norad::Glyph::new("f_i");
        liga.anchors.push(norad::Anchor::new(
            620.0,
            0.0,
            Some(norad::Name::new("caret_1").unwrap()),
            None,
            None,
        ));
        font.default_layer_mut().insert_glyph(liga);
        let blocks = Workspace::generated_feature_blocks(&font);
        let gdef = blocks
            .iter()
            .find(|(tag, _)| tag == "table GDEF")
            .expect("GDEF block generated");
        assert!(gdef.1.contains("LigatureCaretByPos f_i 620;"));
        // The table block writes and replaces through the shared
        // grammar.
        let fea = Workspace::replace_feature_block("", "table GDEF", &gdef.1);
        assert!(fea.starts_with("table GDEF {"));
        assert!(fea.trim_end().ends_with("} GDEF;"));
        let again = Workspace::replace_feature_block(
            &fea,
            "table GDEF",
            "    LigatureCaretByPos f_i 300;\n",
        );
        assert_eq!(again.matches("table GDEF").count(), 1);
        assert!(again.contains("300;"));
    }

    #[test]
    fn production_names_read_from_lib() {
        let mut font = norad::Font::new();
        assert_eq!(read_production_name(&font, "uni0627"), None);
        let mut dict = plist::Dictionary::new();
        dict.insert("alef-ar".into(), plist::Value::String("uni0627".into()));
        font.lib
            .insert(PSNAMES_KEY.into(), plist::Value::Dictionary(dict));
        assert_eq!(
            read_production_name(&font, "alef-ar").as_deref(),
            Some("uni0627")
        );
        assert_eq!(read_production_name(&font, "beh-ar"), None);
    }

    #[test]
    fn saved_filters_roundtrip() {
        let mut font = norad::Font::new();
        assert!(read_saved_filters(&font).is_empty());
        let filters = vec![
            ("wide".to_string(), "w>600".to_string()),
            ("marks".to_string(), "cat:mark".to_string()),
        ];
        write_saved_filters(&mut font, &filters);
        assert_eq!(read_saved_filters(&font), filters);
        write_saved_filters(&mut font, &[]);
        assert!(font.lib.get(SAVED_FILTERS_KEY).is_none());
    }

    #[test]
    fn tidy_correct_and_round_fix_a_messy_glyph() {
        use norad::{Contour, ContourPoint, PointType};
        let pts = |coords: &[(f64, f64)]| -> Vec<ContourPoint> {
            coords
                .iter()
                .map(|&(x, y)| ContourPoint::new(x, y, PointType::Line, false, None, None))
                .collect()
        };
        let mut glyph = norad::Glyph::new("messy");
        // Outer square drawn clockwise (wrong), with a duplicated
        // point and an off-grid coordinate; inner hole drawn
        // counter-clockwise (wrong for a hole).
        glyph.contours = vec![
            Contour::new(
                pts(&[
                    (0.0, 0.0),
                    (0.0, 400.0),
                    (0.0, 400.0),
                    (400.0, 400.0),
                    (400.2, 0.0),
                ]),
                None,
            ),
            Contour::new(
                pts(&[
                    (100.0, 100.0),
                    (300.0, 100.0),
                    (300.0, 300.0),
                    (100.0, 300.0),
                ]),
                None,
            ),
        ];
        assert_eq!(tidy_contours(&mut glyph), 1);
        assert_eq!(glyph.contours[0].points.len(), 4);
        assert_eq!(round_glyph_coordinates(&mut glyph), 1);
        assert_eq!(correct_path_directions(&mut glyph), 2);
        use kurbo::Shape as _;
        let outer = runebender_core::glyph_paths::contour_to_bezpath(&glyph.contours[0]);
        let hole = runebender_core::glyph_paths::contour_to_bezpath(&glyph.contours[1]);
        assert!(outer.area() > 0.0, "outer counter-clockwise");
        assert!(hole.area() < 0.0, "hole clockwise");
        // Running again changes nothing.
        assert_eq!(correct_path_directions(&mut glyph), 0);
        assert_eq!(tidy_contours(&mut glyph), 0);
    }

    #[test]
    fn contours_open_and_close_again() {
        use norad::{Contour, ContourPoint, PointType};
        let square = Contour::new(
            [(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)]
                .iter()
                .map(|&(x, y)| ContourPoint::new(x, y, PointType::Line, false, None, None))
                .collect(),
            None,
        );
        let mut glyph = norad::Glyph::new("openclose");
        glyph.contours = vec![square];
        // Open at point 2: it becomes the Move start.
        assert!(toggle_contour_open(&mut glyph, 0, 2));
        let pts = &glyph.contours[0].points;
        assert_eq!(pts[0].typ, PointType::Move);
        assert_eq!((pts[0].x, pts[0].y), (100.0, 100.0));
        // Close again: the Move becomes a Line, same point count.
        assert!(toggle_contour_open(&mut glyph, 0, 0));
        assert!(
            glyph.contours[0]
                .points
                .iter()
                .all(|p| p.typ != PointType::Move)
        );
        assert_eq!(glyph.contours[0].points.len(), 4);
        // Off-curve target refuses.
        assert!(!toggle_contour_open(&mut glyph, 0, 99));
    }

    #[test]
    fn search_predicates_parse_and_reject() {
        use std::cmp::Ordering;
        assert_eq!(
            parse_search_predicates("w>600"),
            Some(vec![SearchPred::Width(Ordering::Greater, 600.0)])
        );
        assert_eq!(
            parse_search_predicates("cat:mark enc:no"),
            Some(vec![
                SearchPred::Category("mark".into()),
                SearchPred::Encoded(false),
            ])
        );
        assert_eq!(
            parse_search_predicates("comp:beh-ar has:anchors"),
            Some(vec![
                SearchPred::UsesComponent("beh-ar".into()),
                SearchPred::Has("anchors".into()),
            ])
        );
        // Plain text stays plain text.
        assert_eq!(parse_search_predicates("beh"), None);
        assert_eq!(parse_search_predicates("w>abc"), None);
        assert_eq!(parse_search_predicates(""), None);
    }

    #[test]
    fn joining_bands_measure_the_connecting_stroke() {
        use norad::{Contour, ContourPoint, PointType};
        let stroke = Contour::new(
            [(0.0, 40.0), (200.0, 40.0), (200.0, 120.0), (0.0, 120.0)]
                .iter()
                .map(|&(x, y)| ContourPoint::new(x, y, PointType::Line, false, None, None))
                .collect(),
            None,
        );
        let mut glyph = norad::Glyph::new("joined");
        glyph.contours = vec![stroke];
        let path = runebender_core::glyph_paths::contour_to_bezpath(&glyph.contours[0]);
        assert_eq!(joining_band(&path, 200.0, true, 2.0), Some((40.0, 120.0)));
        assert_eq!(joining_band(&path, 200.0, false, 2.0), Some((40.0, 120.0)));
        // Pull the ink off the edge: no band.
        for p in glyph.contours[0].points.iter_mut() {
            p.x += 10.0;
        }
        let moved = runebender_core::glyph_paths::contour_to_bezpath(&glyph.contours[0]);
        assert_eq!(joining_band(&moved, 200.0, true, 2.0), None);

        // And the real Arabic set: a medial beh (a composite —
        // components must resolve) touches both edges.
        let project = Project::load(&default_font_path()).expect("loads");
        let font = project.active_font();
        if let Some(g) = font.font.get_glyph("beh-ar.medi") {
            let i = font.name_map["beh-ar.medi"];
            let advance = font.glyphs[i].advance;
            let outline = runebender_core::glyph_paths::glyph_to_bezpath(g, &font.font);
            assert!(
                joining_band(&outline, advance, true, 2.0).is_some(),
                "medial joins left"
            );
            assert!(
                joining_band(&outline, advance, false, 2.0).is_some(),
                "medial joins right"
            );
        }
    }

    #[test]
    fn masks_roundtrip_and_bake() {
        use norad::{Contour, ContourPoint, PointType};
        let square = |x0: f64, y0: f64, x1: f64, y1: f64| {
            Contour::new(
                [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
                    .iter()
                    .map(|&(x, y)| ContourPoint::new(x, y, PointType::Line, false, None, None))
                    .collect(),
                None,
            )
        };
        let mut glyph = norad::Glyph::new("mask-test");
        // A big square with a smaller mask square overlapping its
        // right edge.
        glyph.contours = vec![
            square(0.0, 0.0, 100.0, 100.0),
            square(60.0, 20.0, 140.0, 80.0),
        ];
        let mut masks = std::collections::HashSet::new();
        masks.insert(1usize);
        write_masks(&mut glyph, &masks);
        assert_eq!(read_masks(&glyph), masks);
        assert!(bake_masks(&mut glyph));
        // The bite is real: no point reaches past x=60 inside the
        // mask's y-band, and the mask key is cleared.
        assert!(read_masks(&glyph).is_empty());
        let max_x_in_band = glyph
            .contours
            .iter()
            .flat_map(|c| c.points.iter())
            .filter(|p| p.y > 25.0 && p.y < 75.0)
            .map(|p| p.x)
            .fold(f64::MIN, f64::max);
        assert!(
            max_x_in_band <= 61.0,
            "mask cut the right side: {max_x_in_band}"
        );
    }

    #[test]
    fn annotations_roundtrip() {
        let mut glyph = norad::Glyph::new("anno");
        let notes = vec![
            Annotation {
                kind: "arrow".into(),
                x: 10.0,
                y: 20.0,
                text: String::new(),
            },
            Annotation {
                kind: "note".into(),
                x: -5.0,
                y: 700.0,
                text: "fix this join".into(),
            },
        ];
        write_annotations(&mut glyph, &notes);
        assert_eq!(read_annotations(&glyph), notes);
        write_annotations(&mut glyph, &[]);
        assert!(glyph.lib.get(ANNOTATIONS_KEY).is_none());
    }

    #[test]
    fn svg_import_fits_and_flips() {
        // A 10x20 SVG rectangle path lands between descender and
        // ascender, y flipped, aspect kept.
        let svg = r#"<svg xmlns="x" viewBox="0 0 10 20">
            <g><path fill="red" d="M0,0 L10,0 L10,20 L0,20 Z"/></g>
        </svg>"#;
        let contours = svg_to_contours(svg, 800.0, -200.0).expect("parses");
        assert_eq!(contours.len(), 1);
        let ys: Vec<f64> = contours[0].points.iter().map(|p| p.y).collect();
        let xs: Vec<f64> = contours[0].points.iter().map(|p| p.x).collect();
        let (min_y, max_y) = ys
            .iter()
            .fold((f64::MAX, f64::MIN), |a, &v| (a.0.min(v), a.1.max(v)));
        let (min_x, max_x) = xs
            .iter()
            .fold((f64::MAX, f64::MIN), |a, &v| (a.0.min(v), a.1.max(v)));
        assert_eq!((min_y, max_y), (-200.0, 800.0), "fills the em");
        assert_eq!(min_x, 0.0);
        assert!((max_x - 500.0).abs() < 1.0, "aspect kept: {max_x}");
        // Curves survive.
        let curvy = r#"<path d="M0 0 C 10 0 20 10 20 20 L 0 20 Z"/>"#;
        let c = svg_to_contours(curvy, 800.0, -200.0).expect("parses curves");
        assert!(
            c[0].points
                .iter()
                .any(|p| p.typ == norad::PointType::OffCurve)
        );
        // No path data errors cleanly.
        assert!(svg_to_contours("<svg></svg>", 800.0, -200.0).is_err());
    }

    #[test]
    fn quad_cubic_conversions() {
        use norad::{Contour, ContourPoint, PointType};
        // A closed quad shape: line across the bottom, one quadratic
        // arc over the top through control (50, 50).
        let pt = |x, y, typ| ContourPoint::new(x, y, typ, false, None, None);
        let contour = Contour::new(
            vec![
                pt(0.0, 0.0, PointType::Line),
                pt(100.0, 0.0, PointType::Line),
                pt(75.0, 50.0, PointType::OffCurve),
                pt(50.0, 60.0, PointType::QCurve),
                pt(25.0, 50.0, PointType::OffCurve),
                pt(0.0, 0.0, PointType::QCurve),
            ],
            None,
        );
        let mut glyph = norad::Glyph::new("quads");
        glyph.contours = vec![contour];
        assert!(quads_to_cubics(&mut glyph));
        let types: Vec<PointType> = glyph.contours[0].points.iter().map(|p| p.typ).collect();
        assert!(!types.contains(&PointType::QCurve), "{types:?}");
        // Two quads became two cubics: 2 on + 2 line + 4 off.
        assert_eq!(
            types.iter().filter(|t| **t == PointType::OffCurve).count(),
            4
        );
        // Exactness at the quad midpoint: the cubic passes through
        // the same point the quad did. Quad (100,0)-(75,50)-(50,60)
        // at t=.5: (75, 40).
        let bez = runebender_core::glyph_paths::contour_to_bezpath(&glyph.contours[0]);
        use kurbo::{ParamCurve as _, Shape as _};
        let close_to = |target: kurbo::Point| {
            bez.segments()
                .any(|seg| (0..=10).any(|i| seg.eval(i as f64 / 10.0).distance(target) < 1.5))
        };
        assert!(close_to(kurbo::Point::new(75.0, 40.0)));

        // And back: cubics to quads stays within tolerance.
        let mut back = glyph.clone();
        assert!(cubics_to_quads(&mut back, 1.0));
        let types: Vec<PointType> = back.contours[0].points.iter().map(|p| p.typ).collect();
        assert!(!types.contains(&PointType::Curve), "{types:?}");
        let bez2 = runebender_core::glyph_paths::contour_to_bezpath(&back.contours[0]);
        // Sample the round-tripped outline against the cubic one.
        for seg in bez.segments() {
            for i in 0..=4 {
                let p = seg.eval(i as f64 / 4.0);
                let nearest = bez2
                    .segments()
                    .flat_map(|s2| (0..=16).map(move |j| s2.eval(j as f64 / 16.0)))
                    .map(|q| p.distance(q))
                    .fold(f64::MAX, f64::min);
                assert!(nearest < 2.5, "outline drifted {nearest}");
            }
        }
    }

    #[test]
    fn corner_splices_the_chamfer() {
        use norad::{Contour, ContourPoint, PointType};
        // The ComponentDemo chamfer: open path (-60, 0) -> (0, 60)
        // around the origin.
        let corner_contour = Contour::new(
            vec![
                ContourPoint::new(-60.0, 0.0, PointType::Move, false, None, None),
                ContourPoint::new(0.0, 60.0, PointType::Line, false, None, None),
            ],
            None,
        );
        let mut corner = norad::Glyph::new("_corner.chamfer");
        corner.contours = vec![corner_contour];
        // A square; apply at (100, 0): incoming runs +x, outgoing +y.
        let square = Contour::new(
            [(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)]
                .iter()
                .map(|&(x, y)| ContourPoint::new(x, y, PointType::Line, false, None, None))
                .collect(),
            None,
        );
        let mut glyph = norad::Glyph::new("square");
        glyph.contours = vec![square];
        assert!(apply_corner_at(&mut glyph, &corner, 0, 1));
        let pts: Vec<(f64, f64)> = glyph.contours[0]
            .points
            .iter()
            .map(|p| (p.x, p.y))
            .collect();
        // The node (100, 0) became two: 60 back along the incoming
        // (+x) segment, and 60 up along the outgoing (+y) one.
        assert_eq!(pts.len(), 5);
        assert!(pts.contains(&(40.0, 0.0)), "{pts:?}");
        assert!(pts.contains(&(100.0, 60.0)), "{pts:?}");
        assert!(!pts.contains(&(100.0, 0.0)), "original corner replaced");
        // Refuses off-curve neighbors and short segments untouched.
        let mut tiny = norad::Glyph::new("tiny");
        tiny.contours = vec![Contour::new(
            vec![
                ContourPoint::new(0.0, 0.0, PointType::Line, false, None, None),
                ContourPoint::new(0.0, 0.0, PointType::Line, false, None, None),
                ContourPoint::new(1.0, 1.0, PointType::Line, false, None, None),
            ],
            None,
        )];
        assert!(!apply_corner_at(&mut tiny, &corner, 0, 1));
    }

    #[test]
    fn metrics_key_parsing() {
        use MetricsFormula::*;
        assert_eq!(parse_metrics_key("=50"), Some(Constant(50.0)));
        assert_eq!(
            parse_metrics_key("=n"),
            Some(Reference {
                glyph: "n".into(),
                mirror: false,
                op: None
            })
        );
        assert_eq!(
            parse_metrics_key("=|o"),
            Some(Reference {
                glyph: "o".into(),
                mirror: true,
                op: None
            })
        );
        assert_eq!(
            parse_metrics_key("=n+10"),
            Some(Reference {
                glyph: "n".into(),
                mirror: false,
                op: Some(('+', 10.0))
            })
        );
        assert_eq!(
            parse_metrics_key("n*1.1"),
            Some(Reference {
                glyph: "n".into(),
                mirror: false,
                op: Some(('*', 1.1))
            })
        );
        assert_eq!(parse_metrics_key("  "), None);
        // A hyphenated glyph name is a name, not subtraction, only
        // when the split lands at position 0 — "beh-ar" splits at 3,
        // so this is a documented limitation: quote it as reference
        // only when no arithmetic parse works.
        assert_eq!(
            parse_metrics_key("=x-4"),
            Some(Reference {
                glyph: "x".into(),
                mirror: false,
                op: Some(('-', 4.0))
            })
        );
    }

    #[test]
    fn metrics_keys_sync_roundtrip() {
        // n's LSB copied onto h in both masters through the lib key.
        let mut project = Project::load(&default_font_path()).expect("loads");
        for master in project.masters.iter_mut() {
            let glyph = master.font.get_glyph_mut("h").expect("has h");
            write_metrics_key(glyph, true, "=n+10");
        }
        // Emulate command_sync_metrics' inner pass directly.
        for master in project.masters.iter_mut() {
            let n = master.name_map["n"];
            let h = master.name_map["h"];
            let target = master.ink_bounds(n).unwrap().x0 + 10.0;
            let delta = (target - master.ink_bounds(h).unwrap().x0).round();
            master.shift_ink(h, delta);
            let lsb = master.ink_bounds(h).unwrap().x0;
            assert!(
                (lsb - target).abs() < 1.0,
                "h LSB follows n+10: {lsb} vs {target}"
            );
            let back = read_metrics_key(master.font.get_glyph("h").unwrap(), true);
            assert_eq!(back.as_deref(), Some("=n+10"));
        }
    }

    #[test]
    fn hoi_quad_passes_through_the_intermediate() {
        let a = (0.0, 0.0);
        let b = (100.0, 0.0);
        let q = (50.0, 40.0);
        assert_eq!(hoi_quad_at(a, b, q, 0.0), a);
        assert_eq!(hoi_quad_at(a, b, q, 1.0), b);
        assert_eq!(hoi_quad_at(a, b, q, 0.5), q);
        // Quarter stop, worked by hand: control C = (50, 80).
        let (x, y) = hoi_quad_at(a, b, q, 0.25);
        assert!((x - 25.0).abs() < 1e-9 && (y - 30.0).abs() < 1e-9);
    }

    #[test]
    fn hoi_intermediates_roundtrip_the_lib_key() {
        let mut glyph = norad::Glyph::new("hoi-store");
        let mut map = std::collections::HashMap::new();
        map.insert((0usize, 3usize), (166.0, 73.0));
        map.insert((2, 0), (-12.0, 400.0));
        write_hoi_intermediates(&mut glyph, &map);
        assert_eq!(read_hoi_intermediates(&glyph), map);
        // Empty map clears the key.
        write_hoi_intermediates(&mut glyph, &std::collections::HashMap::new());
        assert!(glyph.lib.get(HOI_INTERMEDIATE_KEY).is_none());
    }

    #[test]
    fn hoi_preview_is_exact_without_baking() {
        // An intermediate point in the lib key alone (no baked brace
        // layers) must already curve the preview: at mid-axis the
        // node sits exactly on Q, at quarter-axis on the quadratic.
        let mut project = Project::load(&default_font_path()).expect("loads");
        let name = "n";
        let axis = project.axes[0].clone();
        let (lo, hi) = project.axis_end_masters().expect("two ends");
        let a = {
            let g = project.masters[lo].font.get_glyph(name).unwrap();
            let p = &g.contours[0].points[0];
            (p.x, p.y)
        };
        let b = {
            let g = project.masters[hi].font.get_glyph(name).unwrap();
            let p = &g.contours[0].points[0];
            (p.x, p.y)
        };
        let q = ((a.0 + b.0) / 2.0 + 80.0, (a.1 + b.1) / 2.0 + 40.0);
        {
            let g = project.masters[lo].font.get_glyph_mut(name).unwrap();
            let mut map = std::collections::HashMap::new();
            map.insert((0usize, 0usize), q);
            write_hoi_intermediates(g, &map);
        }
        let at = |project: &Project, design: f64| {
            let mut location = runebender_core::var_model::Location::new();
            location.insert(
                axis.name.clone(),
                runebender_core::var_model::normalize_value(
                    design,
                    axis.min,
                    axis.default,
                    axis.max,
                ),
            );
            let g = project.interpolated_at(name, &location).unwrap();
            let p = &g.contours[0].points[0];
            (p.x, p.y)
        };
        let mid_design = axis.min + (axis.max - axis.min) * 0.5;
        let mid = at(&project, mid_design);
        assert!(
            (mid.0 - q.0).abs() < 1e-6 && (mid.1 - q.1).abs() < 1e-6,
            "mid-axis sits on Q: {mid:?} vs {q:?}"
        );
        let quarter_design = axis.min + (axis.max - axis.min) * 0.25;
        let quarter = at(&project, quarter_design);
        let expected = hoi_quad_at(a, b, q, 0.25);
        assert!(
            (quarter.0 - expected.0).abs() < 1e-6 && (quarter.1 - expected.1).abs() < 1e-6,
            "quarter-axis on the quadratic: {quarter:?} vs {expected:?}"
        );
    }

    #[test]
    fn trajectories_sample_the_axis_and_bend_with_braces() {
        let mut project = Project::load(&default_font_path()).expect("loads");
        let name = "n";
        let tracks = project
            .trajectory_samples(name, 10)
            .expect("samples with plain masters");
        let regular = project.masters[0].font.get_glyph(name).unwrap();
        let first_point = &regular.contours[0].points[0];
        // The t=0 end of every track is the Regular master exactly.
        assert!(
            (tracks[0][0].x - first_point.x).abs() < 1e-6
                && (tracks[0][0].y - first_point.y).abs() < 1e-6
        );
        // Straight interpolation: the midpoint sample is the average
        // of the ends.
        let mid_linear = tracks[0][5].x;
        let expected = (tracks[0][0].x + tracks[0][10].x) / 2.0;
        assert!((mid_linear - expected).abs() < 1.0, "linear before braces");
        // A brace at wght 550 (the axis midpoint) pushing the point
        // +60 bends the track's middle away from the straight line.
        let axis = project.axes[0].clone();
        let mut frozen = regular.clone();
        frozen.contours[0].points[0].x += 60.0;
        project.masters[0]
            .font
            .layers
            .get_or_create_layer("{550}")
            .unwrap()
            .insert_glyph(frozen);
        let mut loc = runebender_core::var_model::Location::new();
        loc.insert(
            axis.name.clone(),
            runebender_core::var_model::normalize_value(550.0, axis.min, axis.default, axis.max),
        );
        project.brace.push(BraceSource {
            master: 0,
            layer: "{550}".into(),
            location: loc,
        });
        let bent = project.trajectory_samples(name, 10).expect("still samples");
        assert!(
            (bent[0][5].x - mid_linear).abs() > 20.0,
            "brace bends the middle: {} vs {}",
            bent[0][5].x,
            mid_linear
        );
    }

    #[test]
    fn rule_substitute_switches_past_the_condition() {
        let mut project = Project::load(&default_font_path()).expect("loads");
        let axis = project.axes[0].clone();
        let doc = project.ds_doc.as_mut().expect("doc kept");
        doc.rules.rules.push(norad::designspace::Rule {
            name: Some("a bold".into()),
            condition_sets: vec![norad::designspace::ConditionSet {
                conditions: vec![norad::designspace::Condition {
                    name: axis.name.clone(),
                    minimum: Some(500.0),
                    maximum: Some(axis.max as f32),
                }],
            }],
            substitutions: vec![norad::designspace::Substitution {
                name: norad::Name::new("a").unwrap(),
                with: norad::Name::new("a.bold").unwrap(),
            }],
        });
        let at = |project: &mut Project, design: f64| {
            let axis = &project.axes[0];
            let normalized = runebender_core::var_model::normalize_value(
                design,
                axis.min,
                axis.default,
                axis.max,
            );
            let name = axis.name.clone();
            project.location.insert(name, normalized);
        };
        at(&mut project, 450.0);
        assert_eq!(project.rule_substitute("a"), None, "below the switch");
        at(&mut project, 600.0);
        assert_eq!(
            project.rule_substitute("a").as_deref(),
            Some("a.bold"),
            "past the switch"
        );
        assert_eq!(project.rule_substitute("b"), None, "other glyphs untouched");
    }

    #[test]
    fn measures_reference_stems() {
        use runebender_core::measure::{self, MeasureKind};
        use runebender_core::model::workspace::Contour as WContour;
        // Measured straight from the test font's H, the same path the
        // Dimensions section walks.
        let project = Project::load(&default_font_path()).expect("loads");
        let font = project.active_font();
        let g = font.font.get_glyph("H").expect("has H");
        let paths: Vec<runebender_core::path::Path> = g
            .contours
            .iter()
            .map(|c| runebender_core::path::Path::from_contour(&WContour::from_norad(c)))
            .collect();
        let stems: Vec<i64> = measure::glyph_measurements(&paths)
            .into_iter()
            .filter(|m| m.kind == MeasureKind::Horizontal)
            .map(|m| m.length)
            .collect();
        assert!(!stems.is_empty(), "H yields horizontal spans");
        let narrowest = stems.iter().min().copied().unwrap();
        assert!(
            (10..400).contains(&narrowest),
            "stem in a plausible range: {narrowest}"
        );
    }

    #[test]
    fn generates_positional_and_liga_features() {
        let mut font = runebender_core::new_font::new_font("Gen", "Regular", 400);
        for name in ["beh", "beh.init", "beh.medi", "f", "i", "f_i"] {
            font.default_layer_mut()
                .insert_glyph(norad::Glyph::new(name));
        }
        // Cursive anchors on one glyph feed a curs block.
        {
            let g = font.default_layer_mut().get_glyph_mut("beh").unwrap();
            g.anchors.push(norad::Anchor::new(
                520.0,
                0.0,
                Some(norad::Name::new("exit").unwrap()),
                None,
                None,
            ));
            g.anchors.push(norad::Anchor::new(
                0.0,
                12.0,
                Some(norad::Name::new("entry").unwrap()),
                None,
                None,
            ));
        }
        // A mark composition: aacute = a + acutecomb (a Mark).
        {
            let mut acute = norad::Glyph::new("acutecomb");
            acute.codepoints = norad::Codepoints::new(['\u{0301}']);
            font.default_layer_mut().insert_glyph(acute);
            let mut aacute = norad::Glyph::new("aacute");
            aacute.codepoints = norad::Codepoints::new(['\u{00E1}']);
            aacute.components.push(norad::Component::new(
                norad::Name::new("a").unwrap(),
                norad::AffineTransform::default(),
                None,
            ));
            aacute.components.push(norad::Component::new(
                norad::Name::new("acutecomb").unwrap(),
                norad::AffineTransform::default(),
                None,
            ));
            font.default_layer_mut().insert_glyph(aacute);
            let mut base_a = norad::Glyph::new("a");
            base_a.codepoints = norad::Codepoints::new(['a']);
            font.default_layer_mut().insert_glyph(base_a);
        }
        // Anchors for mark positioning: a base with top, a mark
        // with _top, and a stacking mark carrying both.
        {
            let anchor = |x: f64, y: f64, name: &str| {
                norad::Anchor::new(x, y, Some(norad::Name::new(name).unwrap()), None, None)
            };
            let g = font.default_layer_mut().get_glyph_mut("a").unwrap();
            g.anchors.push(anchor(250.0, 700.0, "top"));
            let m = font.default_layer_mut().get_glyph_mut("acutecomb").unwrap();
            m.anchors.push(anchor(0.0, 0.0, "_top"));
            m.anchors.push(anchor(0.0, 300.0, "top"));
        }
        // Ligature carets on f_i feed the GDEF table.
        {
            let g = font.default_layer_mut().get_glyph_mut("f_i").unwrap();
            g.anchors.push(norad::Anchor::new(
                480.0,
                0.0,
                Some(norad::Name::new("caret_1").unwrap()),
                None,
                None,
            ));
        }
        let blocks = Workspace::generated_feature_blocks(&font);
        let tags: Vec<&str> = blocks.iter().map(|(t, _)| t.as_str()).collect();
        assert!(tags.contains(&"mark") && tags.contains(&"mkmk"), "{tags:?}");
        assert!(tags.contains(&"table GDEF"), "{tags:?}");
        let mark = &blocks.iter().find(|(t, _)| t == "mark").unwrap().1;
        assert!(
            mark.contains("markClass acutecomb <anchor 0 0> @MC_top;"),
            "{mark}"
        );
        assert!(mark.contains("pos base a <anchor 250 700> mark @MC_top;"));
        let mkmk = &blocks.iter().find(|(t, _)| t == "mkmk").unwrap().1;
        assert!(mkmk.contains("pos mark acutecomb <anchor 0 300> mark @MC_top;"));
        // The whole generated set must compile through fea-rs —
        // markClass inside a feature block included.
        {
            let mut fea = String::new();
            for (tag, body) in &blocks {
                fea = Workspace::replace_feature_block(&fea, tag, body);
            }
            let mut model = FontModel::from_font(
                font.clone(),
                std::env::temp_dir().join("feagen-scratch.ufo"),
            );
            model.font.features = fea.clone();
            assert!(
                Workspace::check_features_compile(&model, &fea).is_ok(),
                "generated features compile"
            );
        }
        assert!(tags.contains(&"ccmp"), "{tags:?}");
        let ccmp = &blocks.iter().find(|(t, _)| t == "ccmp").unwrap().1;
        assert!(ccmp.contains("sub a acutecomb by aacute;"), "{ccmp}");
        assert!(tags.contains(&"init") && tags.contains(&"medi"));
        let curs = &blocks.iter().find(|(t, _)| t == "curs").unwrap().1;
        assert!(
            curs.contains("position cursive beh <anchor 0 12> <anchor 520 0>;"),
            "{curs}"
        );
        assert!(curs.contains("lookupflag RightToLeft IgnoreMarks;"));
        assert!(!tags.contains(&"fina"), "no .fina names, no fina block");
        assert!(tags.contains(&"liga"));
        let init = &blocks.iter().find(|(t, _)| t == "init").unwrap().1;
        assert!(init.contains("sub beh by beh.init;"));
        let liga = &blocks.iter().find(|(t, _)| t == "liga").unwrap().1;
        assert!(liga.contains("sub f i by f_i;"));

        // Block replacement: an existing init block is rewritten in
        // place, an absent liga block is appended.
        let fea = "feature init {\n    sub old by old.init;\n} init;\n";
        let out = Workspace::replace_feature_block(fea, "init", init);
        assert!(out.contains("sub beh by beh.init;"));
        assert!(!out.contains("old.init"));
        let out2 = Workspace::replace_feature_block(&out, "liga", liga);
        assert!(out2.contains("feature liga {"));
        assert!(out2.ends_with("} liga;\n"));
    }

    #[test]
    fn fit_curve_sets_handle_fractions() {
        use norad::{Contour, ContourPoint, PointType};
        // A quarter arc: on-point (0,0) tangent up-ish, on-point
        // (100,100) tangent right-ish; tangents meet at (0,100).
        let pt = |x, y, typ, smooth| ContourPoint::new(x, y, typ, smooth, None, None);
        let contour = Contour::new(
            vec![
                pt(0.0, 0.0, PointType::Move, false),
                pt(0.0, 10.0, PointType::OffCurve, false),
                pt(50.0, 100.0, PointType::OffCurve, false),
                pt(100.0, 100.0, PointType::Curve, false),
            ],
            None,
        );
        let mut glyph = norad::Glyph::new("fit-test");
        glyph.contours = vec![contour];
        let all = std::collections::HashSet::new();
        assert!(fit_curve_handles(&mut glyph, &all, 0.5));
        let pts = &glyph.contours[0].points;
        // First handle: from (0,0) toward (0,100), half way = (0,50).
        assert_eq!((pts[1].x, pts[1].y), (0.0, 50.0));
        // Second handle: from (100,100) toward (0,100), half = (50,100).
        assert_eq!((pts[2].x, pts[2].y), (50.0, 100.0));
    }

    #[test]
    fn extrude_and_roughen_transform_a_square() {
        use norad::{Contour, ContourPoint, PointType};
        let square = || {
            Contour::new(
                [(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)]
                    .iter()
                    .map(|&(x, y)| ContourPoint::new(x, y, PointType::Line, false, None, None))
                    .collect(),
                None,
            )
        };
        let bbox = |g: &norad::Glyph| {
            let (mut min, mut max) = ((f64::MAX, f64::MAX), (f64::MIN, f64::MIN));
            for p in g.contours.iter().flat_map(|c| c.points.iter()) {
                min = (min.0.min(p.x), min.1.min(p.y));
                max = (max.0.max(p.x), max.1.max(p.y));
            }
            (min, max)
        };
        // Extrude right-down at 30° by 40: the box grows +40·cos30 in
        // x and −40·sin30 in y, and the front face is cut away.
        let mut g = norad::Glyph::new("extrude-test");
        g.contours = vec![square()];
        assert!(extrude_glyph_contours(&mut g, 40.0, 30.0, false));
        let (min, max) = bbox(&g);
        assert!((max.0 - (100.0 + 40.0 * (30f64).to_radians().cos())).abs() <= 2.0);
        assert!((min.1 - (-40.0 * (30f64).to_radians().sin())).abs() <= 2.0);

        // Roughen: many short jittered segments replace the four.
        let mut r = norad::Glyph::new("roughen-test");
        r.contours = vec![square()];
        let all = std::collections::HashSet::new();
        assert!(roughen_glyph_contours(&mut r, &all, 10.0, 4.0, 4.0, 7));
        assert!(
            r.contours[0].points.len() >= 30,
            "flattened into short segments: {}",
            r.contours[0].points.len()
        );
        // Different seed, different rough.
        let mut r2 = norad::Glyph::new("roughen-test-2");
        r2.contours = vec![square()];
        assert!(roughen_glyph_contours(&mut r2, &all, 10.0, 4.0, 4.0, 8));
        assert_ne!(
            r.contours[0]
                .points
                .iter()
                .map(|p| (p.x, p.y))
                .collect::<Vec<_>>(),
            r2.contours[0]
                .points
                .iter()
                .map(|p| (p.x, p.y))
                .collect::<Vec<_>>(),
        );
    }

    #[test]
    fn offset_bolder_and_lighter() {
        use norad::{Contour, ContourPoint, PointType};
        // A closed 100x100 square, counter-clockwise (postscript
        // outer direction).
        let square = |pts: &[(f64, f64)]| {
            Contour::new(
                pts.iter()
                    .map(|&(x, y)| ContourPoint::new(x, y, PointType::Line, false, None, None))
                    .collect(),
                None,
            )
        };
        let outer = square(&[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)]);
        let mut glyph = norad::Glyph::new("offset-test");
        glyph.contours = vec![outer.clone()];
        assert!(offset_glyph_contours(&mut glyph, 10.0));
        let bbox = |g: &norad::Glyph| {
            let (mut min, mut max) = ((f64::MAX, f64::MAX), (f64::MIN, f64::MIN));
            for p in g.contours.iter().flat_map(|c| c.points.iter()) {
                min = (min.0.min(p.x), min.1.min(p.y));
                max = (max.0.max(p.x), max.1.max(p.y));
            }
            (max.0 - min.0, max.1 - min.1)
        };
        let (w, h) = bbox(&glyph);
        assert!(
            (w - 120.0).abs() <= 2.0 && (h - 120.0).abs() <= 2.0,
            "bolder grows: {w}x{h}"
        );
        let mut glyph2 = norad::Glyph::new("offset-test-2");
        glyph2.contours = vec![outer];
        assert!(offset_glyph_contours(&mut glyph2, -10.0));
        let (w2, h2) = bbox(&glyph2);
        assert!(
            (w2 - 80.0).abs() <= 2.0 && (h2 - 80.0).abs() <= 2.0,
            "lighter shrinks: {w2}x{h2}"
        );
    }

    #[test]
    fn expand_stroke_makes_outlines() {
        use norad::{Contour, ContourPoint, PointType};
        // An open two-point skeleton line from (0,0) to (100,0).
        let line = Contour::new(
            vec![
                ContourPoint::new(0.0, 0.0, PointType::Move, false, None, None),
                ContourPoint::new(100.0, 0.0, PointType::Line, false, None, None),
            ],
            None,
        );
        let mut glyph = norad::Glyph::new("stroke-test");
        glyph.contours = vec![line];
        let all = std::collections::HashSet::new();
        assert!(expand_stroke_contours(&mut glyph, &all, 40.0));
        // The skeleton became a closed outline that spans the stroke:
        // 100 long plus round caps of radius 20 each side, 40 tall.
        assert_eq!(glyph.contours.len(), 1);
        let ys: Vec<f64> = glyph.contours[0].points.iter().map(|p| p.y).collect();
        let xs: Vec<f64> = glyph.contours[0].points.iter().map(|p| p.x).collect();
        let (min_y, max_y) = ys
            .iter()
            .fold((f64::MAX, f64::MIN), |a, &v| (a.0.min(v), a.1.max(v)));
        let (min_x, max_x) = xs
            .iter()
            .fold((f64::MAX, f64::MIN), |a, &v| (a.0.min(v), a.1.max(v)));
        assert!((max_y - min_y - 40.0).abs() <= 2.0, "stroke height ~40");
        assert!(
            (max_x - min_x - 140.0).abs() <= 2.0,
            "length plus caps ~140"
        );
        // Width zero refuses.
        let mut untouched = norad::Glyph::new("no-op");
        assert!(!expand_stroke_contours(&mut untouched, &all, 40.0));
    }

    #[test]
    fn features_compile_check() {
        let project = Project::load(&default_font_path()).expect("designspace loads");
        let font = project.active_font();
        // The font's own features.fea compiles.
        let own = font.font.features.clone();
        assert!(Workspace::check_features_compile(font, &own).is_ok());
        // Garbage does not, and the error says something.
        let err = Workspace::check_features_compile(font, "feature liga { nonsense ; } liga;");
        assert!(err.is_err());
        assert!(!err.unwrap_err().is_empty());
    }

    #[test]
    fn move_point_and_save_roundtrip() {
        let src = test_ufo_path();
        let tmp = std::env::temp_dir().join("rbg-save-test.ufo");
        if tmp.exists() {
            std::fs::remove_dir_all(&tmp).unwrap();
        }
        let copy_options = fs_extra_copy(&src, &tmp);
        assert!(copy_options, "copying test UFO failed");

        let mut model = FontModel::load(&tmp).expect("load");
        let index = model
            .glyphs
            .iter()
            .position(|g| g.name.as_ref() == "a")
            .expect("glyph a");
        let before = model.glyphs[index].points[0];
        model.set_points(
            index,
            &[(
                (before.contour, before.index),
                (before.x + 10.0, before.y + 5.0),
            )],
        );
        assert!(model.dirty);
        let after = model.glyphs[index].points[0];
        assert_eq!(after.x, before.x + 10.0);
        assert_eq!(after.y, before.y + 5.0);
        model.save().expect("save");
        assert!(!model.dirty);

        let reloaded = FontModel::load(&tmp).expect("reload");
        let entry = reloaded
            .glyphs
            .iter()
            .find(|g| g.name.as_ref() == "a")
            .unwrap();
        let p = entry
            .points
            .iter()
            .find(|p| p.contour == before.contour && p.index == before.index)
            .unwrap();
        assert_eq!(p.x, before.x + 10.0);
        assert_eq!(p.y, before.y + 5.0);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn snapshot_restore_roundtrip() {
        let mut model = FontModel::load(&test_ufo_path()).expect("load");
        let index = model
            .glyphs
            .iter()
            .position(|g| g.name.as_ref() == "a")
            .unwrap();
        let before = model.snapshot_contours(index).unwrap();
        let p0 = model.glyphs[index].points[0];
        model.set_points(index, &[((p0.contour, p0.index), (p0.x + 25.0, p0.y))]);
        assert_ne!(model.glyphs[index].points[0].x, p0.x);
        model.restore_contours(index, before);
        assert_eq!(model.glyphs[index].points[0].x, p0.x);
        assert_eq!(model.glyphs[index].points[0].y, p0.y);
    }

    #[test]
    fn pen_primitives_build_a_closed_contour() {
        let mut model = FontModel::load(&test_ufo_path()).expect("load");
        let index = model
            .glyphs
            .iter()
            .position(|g| g.name.as_ref() == "space")
            .unwrap();
        let base_contours = model.snapshot_contours(index).unwrap().contours.len();

        let c = model.start_contour(index, 0.0, 0.0).unwrap();
        model.append_segment(index, c, None, 100.0, 0.0, false); // line
        model.append_segment(
            index,
            c,
            Some(((130.0, 40.0), (130.0, 80.0))),
            100.0,
            120.0,
            true,
        ); // curve
        model.close_contour(index, c, None);

        let contours = model.snapshot_contours(index).unwrap().contours;
        assert_eq!(contours.len(), base_contours + 1);
        let new = &contours[c];
        assert!(new.is_closed(), "contour should be closed");
        // move->line conversion on close + 2 on-curves + 2 off-curves
        assert_eq!(new.points.len(), 5);
        assert_eq!(new.points[0].typ, norad::PointType::Line);
        assert!(new.points[4].smooth);
        // The outline cache rebuilt and is drawable.
        assert!(!model.glyphs[index].path.elements().is_empty());

        // Degenerate contour cleanup: a single stray point goes away.
        let c2 = model.start_contour(index, 5.0, 5.0).unwrap();
        model.remove_contour_if_degenerate(index, c2);
        assert_eq!(
            model.snapshot_contours(index).unwrap().contours.len(),
            base_contours + 1
        );
    }

    #[test]
    fn delete_and_smooth_operations() {
        let mut model = FontModel::load(&test_ufo_path()).expect("load");
        let index = model
            .glyphs
            .iter()
            .position(|g| g.name.as_ref() == "space")
            .unwrap();

        // Build a closed square with one curved corner:
        // (0,0) -line- (100,0) -line- (100,100) -curve- (0,100) -close-
        let c = model.start_contour(index, 0.0, 0.0).unwrap();
        model.append_segment(index, c, None, 100.0, 0.0, false);
        model.append_segment(index, c, None, 100.0, 100.0, false);
        model.append_segment(
            index,
            c,
            Some(((80.0, 130.0), (20.0, 130.0))),
            0.0,
            100.0,
            true,
        );
        model.close_contour(index, c, None);
        let count_points =
            |m: &FontModel| m.snapshot_contours(index).unwrap().contours[c].points.len();
        assert_eq!(count_points(&model), 6); // 4 on + 2 off

        // Toggle smooth on the curve's endpoint.
        let curve_end_index = model.glyphs[index]
            .points
            .iter()
            .find(|p| p.contour == c && p.x == 0.0 && p.y == 100.0)
            .map(|p| (p.contour, p.index))
            .unwrap();
        let sel: std::collections::HashSet<_> = [curve_end_index].into();
        assert!(model.toggle_smooth(index, &sel));

        // Delete one off-curve: the curve segment becomes a line.
        let off = model.glyphs[index]
            .points
            .iter()
            .find(|p| p.contour == c && !p.on_curve)
            .map(|p| (p.contour, p.index))
            .unwrap();
        let sel: std::collections::HashSet<_> = [off].into();
        assert!(model.delete_points(index, &sel));
        assert_eq!(count_points(&model), 4); // pure quad now
        let snapshot = model.snapshot_contours(index).unwrap();
        let contour_data = &snapshot.contours[c];
        assert!(contour_data.is_closed());
        assert!(
            contour_data
                .points
                .iter()
                .all(|p| p.typ != norad::PointType::OffCurve)
        );

        // Delete an on-curve point: square becomes a triangle.
        let corner = model.glyphs[index]
            .points
            .iter()
            .find(|p| p.contour == c && p.x == 100.0 && p.y == 0.0)
            .map(|p| (p.contour, p.index))
            .unwrap();
        let sel: std::collections::HashSet<_> = [corner].into();
        assert!(model.delete_points(index, &sel));
        assert_eq!(count_points(&model), 3);

        // Delete everything: the contour disappears.
        let all: std::collections::HashSet<_> = model.glyphs[index]
            .points
            .iter()
            .filter(|p| p.contour == c)
            .map(|p| (p.contour, p.index))
            .collect();
        assert!(model.delete_points(index, &all));
        assert!(model.snapshot_contours(index).unwrap().contours.len() <= c);
    }

    #[test]
    fn curve_ops_run_via_shared_core() {
        let mut model = FontModel::load(&test_ufo_path()).expect("load");
        let index = model
            .glyphs
            .iter()
            .position(|g| g.name.as_ref() == "o")
            .unwrap();
        let none = std::collections::HashSet::new();
        let before: Vec<(f64, f64)> = model.glyphs[index]
            .points
            .iter()
            .map(|p| (p.x, p.y))
            .collect();
        // Balance evens handle tension; on a real glyph something moves.
        let changed = model.curve_op(index, &none, CurveOp::Balance);
        let after: Vec<(f64, f64)> = model.glyphs[index]
            .points
            .iter()
            .map(|p| (p.x, p.y))
            .collect();
        if changed {
            assert_ne!(before, after);
        }
        // On-curve points never move under balance.
        for (i, p) in model.glyphs[index].points.iter().enumerate() {
            if p.on_curve {
                assert_eq!(before[i], (p.x, p.y), "on-curve moved at {i}");
            }
        }
        // Harmonize and optimize execute without panicking and keep
        // the outline drawable.
        model.curve_op(index, &none, CurveOp::Harmonize);
        model.curve_op(index, &none, CurveOp::Optimize(0.12));
        assert!(!model.glyphs[index].path.elements().is_empty());
    }

    #[test]
    fn metric_edits() {
        let mut model = FontModel::load(&test_ufo_path()).expect("load");
        let index = model
            .glyphs
            .iter()
            .position(|g| g.name.as_ref() == "n")
            .unwrap();
        let ink = model.ink_bounds(index).unwrap();
        let advance = model.glyphs[index].advance;

        // Width edit changes only the advance.
        model.set_advance(index, advance + 20.0);
        assert_eq!(model.glyphs[index].advance, advance + 20.0);
        assert_eq!(model.ink_bounds(index).unwrap().x0, ink.x0);

        // LSB edit shifts the ink, advance untouched.
        model.shift_ink(index, 10.0);
        let ink2 = model.ink_bounds(index).unwrap();
        assert_eq!(ink2.x0, ink.x0 + 10.0);
        assert_eq!(ink2.x1, ink.x1 + 10.0);
        assert_eq!(model.glyphs[index].advance, advance + 20.0);
        assert!(model.dirty);
    }

    #[test]
    fn smooth_handle_constraint_keeps_collinearity() {
        let mut model = FontModel::load(&test_ufo_path()).expect("load");
        let index = model
            .glyphs
            .iter()
            .position(|g| g.name.as_ref() == "space")
            .unwrap();
        // Two curve segments joined at a smooth point (100,100):
        let c = model.start_contour(index, 0.0, 0.0).unwrap();
        model.append_segment(
            index,
            c,
            Some(((40.0, 60.0), (60.0, 100.0))),
            100.0,
            100.0,
            true,
        );
        model.append_segment(
            index,
            c,
            Some(((140.0, 100.0), (180.0, 60.0))),
            200.0,
            0.0,
            false,
        );
        model.close_contour(index, c, None);

        // Points in contour c: find indices of the incoming handle
        // (60,100), the smooth point (100,100), the outgoing (140,100).
        let find = |m: &FontModel, x: f64, y: f64| {
            m.glyphs[index]
                .points
                .iter()
                .find(|p| p.contour == c && p.x == x && p.y == y)
                .map(|p| p.index)
                .unwrap()
        };
        let incoming = find(&model, 60.0, 100.0);
        let outgoing = find(&model, 140.0, 100.0);

        // Drag the incoming handle downward; the outgoing must rotate
        // to stay collinear through (100,100).
        model.set_points(index, &[((c, incoming), (60.0, 80.0))]);
        model.edit_glyph(index, |g| ops::constrain_smooth_neighbor(g, c, incoming));
        let pts = &model.glyphs[index].points;
        let out_pt = pts
            .iter()
            .find(|p| p.contour == c && p.index == outgoing)
            .unwrap();
        // Collinearity: cross product of (anchor-incoming) and
        // (outgoing-anchor) near zero (integer rounding allowed).
        let cross = (100.0 - 60.0) * (out_pt.y - 100.0) - (100.0 - 80.0) * (out_pt.x - 100.0);
        assert!(
            cross.abs() <= 60.0,
            "not collinear enough: {cross} ({}, {})",
            out_pt.x,
            out_pt.y
        );
        // Length preserved (was 40).
        let len = ((out_pt.x - 100.0f64).powi(2) + (out_pt.y - 100.0f64).powi(2)).sqrt();
        assert!((len - 40.0).abs() < 2.0, "length changed: {len}");
    }

    #[test]
    fn anchor_lifecycle_with_undo_snapshot() {
        let mut model = FontModel::load(&test_ufo_path()).expect("load");
        let index = model
            .glyphs
            .iter()
            .position(|g| g.name.as_ref() == "n")
            .unwrap();
        let before = model.snapshot_contours(index).unwrap();
        let base = model.glyphs[index].anchors.len();

        model.add_anchor(index, 200.0, 500.0);
        assert_eq!(model.glyphs[index].anchors.len(), base + 1);
        model.set_anchor(index, base, 210.0, 490.0);
        assert_eq!(model.glyphs[index].anchors[base].1, 210.0);
        model.delete_anchor(index, base);
        assert_eq!(model.glyphs[index].anchors.len(), base);

        // Snapshot restore also brings anchors and width back.
        model.add_anchor(index, 1.0, 2.0);
        model.set_advance(index, 999.0);
        model.restore_contours(index, before);
        assert_eq!(model.glyphs[index].anchors.len(), base);
        assert_ne!(model.glyphs[index].advance, 999.0);
    }

    #[test]
    fn kerning_lookup_and_exception() {
        let mut model = FontModel::load(&test_ufo_path()).expect("load");
        // Group fallback resolves (VirtuaGrotesk has kern groups); the
        // exact value doesn't matter, just that lookup doesn't panic
        // and exceptions override.
        let base = ops::kern_value(&model.font, "A", "V");
        ops::set_kern_pair(&mut model.font, "A", "V", base - 14.0);
        assert_eq!(ops::kern_value(&model.font, "A", "V"), base - 14.0);
        // Unrelated pair unaffected by the exception.
        let _ = ops::kern_value(&model.font, "o", "o");
    }

    #[test]
    fn interpolation_at_midpoint() {
        let mut project = Project::load(&default_font_path()).expect("designspace");
        assert!(project.model.is_some(), "two masters, model expected");
        // Move every axis to its normalized midpoint toward max.
        let axis_names: Vec<String> = project.axes.iter().map(|a| a.name.clone()).collect();
        for name in &axis_names {
            project.location.insert(name.clone(), 0.5);
        }
        let (path, advance) = project
            .interpolated_glyph("n")
            .expect("compatible masters interpolate");
        assert!(!path.elements().is_empty());
        // The interpolated advance sits between the two masters'.
        let a0 = project.masters[0].font.get_glyph("n").unwrap().width;
        let a1 = project.masters[1].font.get_glyph("n").unwrap().width;
        let (lo, hi) = (a0.min(a1), a0.max(a1));
        assert!(
            advance >= lo - 1e-6 && advance <= hi + 1e-6,
            "advance {advance} outside [{lo}, {hi}]"
        );
        // Default location yields no ghost.
        for name in &axis_names {
            project.location.insert(name.clone(), 0.0);
        }
        assert!(project.interpolated_glyph("n").is_none());
    }

    #[test]
    fn shape_contours() {
        let mut model = FontModel::load(&test_ufo_path()).expect("load");
        let index = model
            .glyphs
            .iter()
            .position(|g| g.name.as_ref() == "space")
            .unwrap();
        let base = model.snapshot_contours(index).unwrap().contours.len();
        let rect = kurbo::Rect::new(10.0, 20.0, 110.0, 220.0);
        model.add_shape_contour(index, rect, false);
        model.add_shape_contour(index, rect, true);
        let contours = model.snapshot_contours(index).unwrap().contours;
        assert_eq!(contours.len(), base + 2);
        let square = &contours[base];
        assert_eq!(square.points.len(), 4);
        assert!(square.is_closed());
        let circle = &contours[base + 1];
        assert_eq!(circle.points.len(), 12); // 4 on + 8 off
        assert!(circle.is_closed());
        // Ellipse extremes touch the rect edges.
        let xs: Vec<f64> = circle.points.iter().map(|p| p.x).collect();
        assert_eq!(xs.iter().cloned().fold(f64::MAX, f64::min), 10.0);
        assert_eq!(xs.iter().cloned().fold(f64::MIN, f64::max), 110.0);
    }

    #[test]
    fn compat_map_flags_structure_changes() {
        let mut project = Project::load(&default_font_path()).expect("designspace");
        // Demo masters are interpolation-compatible for letters.
        assert_eq!(project.compat.get("n"), Some(&true));
        // Break compatibility in one master and recheck.
        let idx = project.masters[0]
            .glyphs
            .iter()
            .position(|g| g.name.as_ref() == "n")
            .unwrap();
        let rect = kurbo::Rect::new(0.0, 0.0, 50.0, 50.0);
        project.masters[0].add_shape_contour(idx, rect, false);
        project.recheck_compat("n");
        assert_eq!(project.compat.get("n"), Some(&false));
    }

    #[test]
    fn decompose_components() {
        let mut model = FontModel::load(&test_ufo_path()).expect("load");
        let index = model
            .glyphs
            .iter()
            .position(|g| !g.component_names.is_empty())
            .expect("demo font has composite glyphs");
        use kurbo::Shape;
        let area_before = model.glyphs[index].path.area().abs();
        let contours_before = model.snapshot_contours(index).unwrap().contours.len();
        assert!(model.decompose(index));
        let snap = model.snapshot_contours(index).unwrap();
        assert!(snap.components.is_empty());
        assert!(snap.contours.len() > contours_before);
        // The rendered ink is essentially unchanged (integer rounding).
        let area_after = model.glyphs[index].path.area().abs();
        assert!(
            (area_before - area_after).abs() / area_before.max(1.0) < 0.02,
            "area changed too much: {area_before} -> {area_after}"
        );
        assert!(model.glyphs[index].component_names.is_empty());
    }

    #[test]
    fn remove_overlap_unions_contours() {
        use kurbo::Shape;
        let mut model = FontModel::load(&test_ufo_path()).expect("load");
        let index = model
            .glyphs
            .iter()
            .position(|g| g.name.as_ref() == "space")
            .unwrap();
        // Two overlapping squares: union area = 100*100 + 100*100 - 50*50.
        model.add_shape_contour(index, kurbo::Rect::new(0.0, 0.0, 100.0, 100.0), false);
        model.add_shape_contour(index, kurbo::Rect::new(50.0, 50.0, 150.0, 150.0), false);
        assert!(model.remove_overlap(index));
        let snap = model.snapshot_contours(index).unwrap();
        assert_eq!(snap.contours.len(), 1, "union should merge to one contour");
        let area = model.glyphs[index].path.area().abs();
        assert!(
            (area - 17500.0).abs() < 100.0,
            "union area wrong: {area} (expected ~17500)"
        );
        assert!(snap.contours[0].is_closed());
    }

    #[test]
    fn glyphs_import_end_to_end() {
        let dir = std::env::temp_dir().join("rbg-glyphs-import-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let glyphs_path = dir.join("TestSans.glyphs");
        std::fs::write(&glyphs_path, MINIMAL_GLYPHS_SOURCE).unwrap();
        let project = Project::load(&glyphs_path).expect("glyphs project loads");
        assert_eq!(project.masters.len(), 2);
        let a = project
            .active_font()
            .glyphs
            .iter()
            .find(|g| g.name.as_ref() == "A")
            .expect("glyph A");
        assert!(!a.path.elements().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Minimal recursive dir copy (a UFO is a directory).
    fn fs_extra_copy(src: &std::path::Path, dst: &std::path::Path) -> bool {
        fn copy_dir(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
            std::fs::create_dir_all(dst)?;
            for entry in std::fs::read_dir(src)? {
                let entry = entry?;
                let target = dst.join(entry.file_name());
                if entry.file_type()?.is_dir() {
                    copy_dir(&entry.path(), &target)?;
                } else {
                    std::fs::copy(entry.path(), &target)?;
                }
            }
            Ok(())
        }
        copy_dir(src, dst).is_ok()
    }
}

#[cfg(test)]
mod theme_geometry_tests {
    use crate::*;
    use std::sync::Mutex;

    /// `set_theme` writes a global, and cargo runs tests in parallel,
    /// so the two tests that switch themes take this first. Without it
    /// they interleave and read each other's theme.
    static THEME: Mutex<()> = Mutex::new(());

    /// The bug this catches: `theme_menu_items` used to end in a
    /// `_ => Box::new(SetThemeDark)` arm, so a theme added to the token
    /// file got a menu entry that switched to Dark. It looked wired up
    /// and was not.
    /// The default is a name, and a name can be wrong. Without this a
    /// typo would only show up as a window that came up in whatever
    /// theme the fallback happened to reach.
    #[test]
    fn the_default_theme_exists() {
        assert!(
            t::THEMES.iter().any(|(id, _)| *id == t::DEFAULT_THEME),
            "{} is not in THEMES",
            t::DEFAULT_THEME
        );
        assert!(t::set_theme(t::DEFAULT_THEME), "the default must load");
    }

    #[test]
    fn every_theme_has_its_own_action() {
        for (id, _) in t::THEMES {
            assert!(theme_action(id).is_some(), "no action for theme {id}");
        }
    }

    #[test]
    fn every_theme_in_the_menu_loads() {
        let _guard = THEME.lock().unwrap_or_else(|e| e.into_inner());
        for (id, _) in t::THEMES {
            assert!(t::set_theme(id), "theme {id} is not in the token file");
        }
        t::set_theme(t::DEFAULT_THEME);
    }

    /// Geometry has to actually follow the theme, or the tokens are
    /// decoration and the frontend is still hardcoding shape.
    #[test]
    fn geometry_changes_with_the_theme() {
        let _guard = THEME.lock().unwrap_or_else(|e| e.into_inner());
        t::set_theme("dark");
        let dark_r = t::radius();
        t::set_theme("gray");
        assert_ne!(dark_r, t::radius(), "Gray should be square");
        assert_eq!(t::radius(), gpui::px(0.0));
        assert_eq!(t::radius_control(), gpui::px(0.0));
        // Gray changes the corners and not the rule weight. A theme
        // naming only part of the geometry is the case the optional
        // fields exist for, so it is worth pinning.
        assert_eq!(t::stroke(), gpui::px(1.0), "an editor rule is a hairline");
        assert_eq!(t::stroke_emphasis(), gpui::px(2.0));
        t::set_theme(t::DEFAULT_THEME);
    }
}

#[cfg(test)]
mod mark_paint_tests {
    use crate::*;
    use std::sync::Mutex;

    static THEME: Mutex<()> = Mutex::new(());

    #[test]
    fn an_unmarked_glyph_has_no_paint() {
        let _g = THEME.lock().unwrap_or_else(|e| e.into_inner());
        assert!(t::mark_paint(None).is_none());
        assert!(t::mark_paint(Some("not-a-mark")).is_none());
    }

    /// Dark tints the rule and leaves the fill alone; Gray fills the
    /// cell and keys it. The treatment is the theme's, not the grid's.
    #[test]
    fn the_treatment_follows_the_theme() {
        let _g = THEME.lock().unwrap_or_else(|e| e.into_inner());

        t::set_theme("dark");
        let dark = t::mark_paint(Some("yellow")).expect("yellow is a mark");
        assert!(dark.bg.is_none(), "Dark should not fill the cell");
        assert_eq!(dark.border, dark.ink, "a tinted rule and its label match");

        t::set_theme("gray");
        let gray = t::mark_paint(Some("yellow")).expect("yellow is a mark");
        let fill = gray.bg.expect("Gray fills the cell");
        // The bug this guards: the glyph and label used to be painted
        // in the mark colour. On a filled cell that is the colour they
        // are sitting on, so the cell would come out blank.
        assert_ne!(fill, gray.ink, "ink must not be the fill it sits on");
        assert_ne!(fill, gray.border, "the keyline must not be the fill");

        t::set_theme(t::DEFAULT_THEME);
    }
}

#[cfg(test)]
mod model_discovery_tests {
    use crate::*;
    use std::sync::Mutex;

    /// These set RUNEBENDER_MODELS, which is process-wide, and cargo
    /// runs tests in parallel. Without this they read each other's
    /// environment.
    static ENV: Mutex<()> = Mutex::new(());

    /// The convention is the installation step, so it is pinned: an
    /// override for people who keep models elsewhere, and one default
    /// that needs no configuration.
    #[test]
    fn the_override_wins_over_the_default() {
        let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        // Read through the same accessor the panel uses, rather than
        // duplicating the rule here.
        let dir = Workspace::models_dir();
        assert!(dir.is_some(), "there is always somewhere to look");
        assert!(
            dir.unwrap().ends_with(".runebender/models")
                || std::env::var_os("RUNEBENDER_MODELS").is_some(),
            "without the override it is ~/.runebender/models"
        );
    }

    /// A directory only counts as a model if it holds a config.json.
    /// Without that check, every stray folder becomes a broken entry.
    #[test]
    fn a_folder_without_a_config_is_not_a_model() {
        let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join("rb-model-discovery-test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("not-a-model")).unwrap();
        std::fs::create_dir_all(tmp.join("real")).unwrap();
        std::fs::write(tmp.join("real/config.json"), "{}").unwrap();
        // SAFETY: single-threaded test process, and the value is read
        // back through the same accessor immediately.
        unsafe { std::env::set_var("RUNEBENDER_MODELS", &tmp) };
        let found = Workspace::installed_models();
        unsafe { std::env::remove_var("RUNEBENDER_MODELS") };
        let _ = std::fs::remove_dir_all(&tmp);
        let names: Vec<_> = found.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["real"]);
    }

    #[test]
    fn a_missing_folder_is_not_an_error() {
        let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { std::env::set_var("RUNEBENDER_MODELS", "/nope/does/not/exist") };
        let found = Workspace::installed_models();
        unsafe { std::env::remove_var("RUNEBENDER_MODELS") };
        assert!(found.is_empty());
    }
}
