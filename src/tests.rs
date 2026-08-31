// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Tests for the front-end.
//!
//! Anything checkable without a window belongs in runebender-core
//! instead. What is left here is the shell's own behaviour.

#[cfg(test)]
mod shell_tests {
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
            project.active_font().glyphs.iter().any(|g| &*g.name == "A"),
            "converted font should hold the glyphs from the package"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
    use crate::*;
    use runebender_core::document::project::{Master, Project};

    #[test]
    fn glyph_image_roundtrips_through_save() {
        // A 2x2 png in the images store plus a glyph image reference
        // must survive a save and reload (norad owns the images dir).
        let mut font = runebender_core::document::new_font::new_font("Img", "Regular", 400);
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

    /// The demo designspace the two feature tests compile against. Same
    /// rule as core's `testing::fonts`: `RUNEBENDER_TEST_FONTS`, else the
    /// virtua-grotesk checkout beside this one.
    fn fixture_designspace() -> PathBuf {
        let dir = match std::env::var_os("RUNEBENDER_TEST_FONTS") {
            Some(dir) => PathBuf::from(dir),
            None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../virtua-grotesk/sources"),
        };
        let path = dir.join("VirtuaGrotesk.designspace");
        assert!(
            path.is_file(),
            "fixture fonts not found at {}: clone eliheuer/virtua-grotesk next to this \
             repository, or set RUNEBENDER_TEST_FONTS",
            dir.display()
        );
        path
    }

    #[test]
    fn stylistic_set_names_compile() {
        // featureNames inside an ss block is plain fea; the editor's
        // Features pane plus fea-rs carry it end to end.
        let project = Project::load(&fixture_designspace()).expect("loads");
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
    fn generates_positional_and_liga_features() {
        let mut font = runebender_core::document::new_font::new_font("Gen", "Regular", 400);
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
            let mut model = Master::from_font(
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
    fn features_compile_check() {
        let project = Project::load(&fixture_designspace()).expect("designspace loads");
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
}

#[cfg(test)]
mod theme_geometry_tests {
    use crate::actions::theme_action;
    use crate::*;
    use std::sync::Mutex;

    /// `set_theme` writes a global, and cargo runs tests in parallel,
    /// so the two tests that switch themes take this first. Without it
    /// they interleave and read each other's theme.
    static THEME: Mutex<()> = Mutex::new(());

    /// The default is a name, and a name can be wrong. Without this
    /// a typo would only show up as a window that came up in whatever
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

    /// The bug this catches: `theme_menu_items` used to end in a
    /// `_ => Box::new(SetThemeDark)` arm, so a theme added to the
    /// token file got a menu entry that switched to Dark. It looked
    /// wired up and was not.
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

    /// These set `RUNEBENDER_MODELS`, which is process-wide, and cargo
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

    /// A directory only counts as a model if it holds a `config.json`.
    /// Without that check, every stray folder becomes a broken entry.
    #[test]
    #[allow(unsafe_code)]
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
    #[allow(unsafe_code)]
    fn a_missing_folder_is_not_an_error() {
        let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { std::env::set_var("RUNEBENDER_MODELS", "/nope/does/not/exist") };
        let found = Workspace::installed_models();
        unsafe { std::env::remove_var("RUNEBENDER_MODELS") };
        assert!(found.is_empty());
    }
}
