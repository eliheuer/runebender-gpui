// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! App-level commands and the menus that dispatch them.
//!
//! GPUI does not populate the macOS menu bar on its own; `app_menus`
//! is the one menu definition, consumed by the native bar, the stored
//! menus on Windows and Linux, and the in-window bar drawn where no
//! native bar exists.

use crate::AddExtremes;
use crate::BakeMasks;
use crate::Balance;
use crate::BoldenWithModel;
use crate::BooleanExclude;
use crate::BooleanIntersect;
use crate::BooleanSubtract;
use crate::BooleanUnion;
use crate::CheckJoining;
use crate::CopyContours;
use crate::CopySelectedGlyphs;
use crate::CorrectPathDirection;
use crate::CubicsToQuads;
use crate::Decompose;
use crate::DeselectAllPoints;
use crate::DuplicateGlyph;
use crate::DuplicateRepeat;
use crate::DuplicateSelection;
use crate::ExportFont;
use crate::ExportGlyphSvg;
use crate::FilterExtrude;
use crate::FilterOffsetCurve;
use crate::FilterRoughen;
use crate::FilterSlant;
use crate::FlipHorizontal;
use crate::FlipVertical;
use crate::Harmonize;
use crate::HyperToCubic;
use crate::ImportSvg;
use crate::InvertPointSelection;
use crate::MeasureAllOff;
use crate::MeasureAllOn;
use crate::MeasureColorize;
use crate::MeasureHandles;
use crate::MeasurePopcount;
use crate::MeasureSegments;
use crate::MeasureSideBearings;
use crate::MeasureSizes;
use crate::MeasureSpans;
use crate::NewFont;
use crate::NewGlyph;
use crate::NextMaster;
use crate::NextSampleString;
use crate::OpenFont;
use crate::Optimize;
use crate::PasteContours;
use crate::PlaceImage;
use crate::PreviousMaster;
use crate::PreviousSampleString;
use crate::QuadsToCubics;
use crate::Quit;
use crate::Redo;
use crate::Reinterpolate;
use crate::RemoveGlyphCmd;
use crate::RemoveImage;
use crate::RemoveOverlap;
use crate::ReverseContours;
use crate::Rotate180;
use crate::RotateLeft;
use crate::RotateRight;
use crate::RoundCoordinates;
use crate::RoundCorners;
use crate::SaveFont;
use crate::SaveFontAs;
use crate::SelectAllPoints;
use crate::SetStartPoint;
use crate::SetThemeDark;
use crate::SetThemeGray;
use crate::SetThemeLight;
use crate::SetThemeMidnight;
use crate::ShowAllMasters;
use crate::SortByName;
use crate::SortByUnicode;
use crate::SyncMetrics;
use crate::TidyPaths;
use crate::TraceImage;
use crate::Undo;
use crate::ZoomToFit;
use crate::view::theme as t;
use crate::workspace::MEASURE_MENU;

/// The action that switches to the theme named `id`.
///
/// Returns `None` when the token file gained a theme that no arm
/// here handles. A fallback arm would silently hand a new theme
/// Dark's action, so callers are made to notice instead.
pub(crate) fn theme_action(id: &str) -> Option<Box<dyn gpui::Action>> {
    Some(match id {
        "dark" => Box::new(SetThemeDark),
        "midnight" => Box::new(SetThemeMidnight),
        "gray" => Box::new(SetThemeGray),
        "light" => Box::new(SetThemeLight),
        _ => return None,
    })
}

/// One item per theme, with the active one checked. The menus are
/// rebuilt on a switch so the tick follows.
pub(crate) fn theme_menu_items() -> Vec<gpui::MenuItem> {
    use gpui::MenuItem;
    let current = t::current_theme();
    t::THEMES
        .iter()
        .map(|(id, label)| {
            let action = theme_action(id).expect("every theme has an action");
            MenuItem::Action {
                name: (*label).into(),
                action,
                os_action: None,
                checked: *id == current,
                disabled: false,
            }
        })
        .collect()
}

/// The Measure overlays, as a menu of toggles. They are view options,
/// so they live beside the other view settings rather than taking a
/// panel's worth of sidebar.
pub(crate) fn measure_menu_items() -> Vec<gpui::MenuItem> {
    use gpui::MenuItem;
    let o = *MEASURE_MENU.lock().expect("measure menu");
    let item =
        |name: &'static str, action: Box<dyn gpui::Action>, checked: bool| MenuItem::Action {
            name: name.into(),
            action,
            os_action: None,
            checked,
            disabled: false,
        };
    vec![
        item("Colorize Outline", Box::new(MeasureColorize), o.colorize),
        item("Handle Lengths", Box::new(MeasureHandles), o.handles),
        item("Segment Lengths", Box::new(MeasureSegments), o.segments),
        item("Segment Sizes", Box::new(MeasureSizes), o.sizes),
        item("Stems & Counters", Box::new(MeasureSpans), o.spans),
        item(
            "Side Bearings",
            Box::new(MeasureSideBearings),
            o.sidebearings,
        ),
        MenuItem::separator(),
        // Not a layer: how the labels that are on get written.
        item("Popcount Sums", Box::new(MeasurePopcount), o.popcount),
        MenuItem::separator(),
        MenuItem::action("All On", MeasureAllOn),
        MenuItem::action("All Off", MeasureAllOff),
    ]
}

/// The whole application menu, one `Menu` per top-level title, with the current option states checked.
pub(crate) fn app_menus() -> Vec<gpui::Menu> {
    use gpui::{Menu, MenuItem};
    vec![
        Menu {
            name: "Runebender".into(),
            items: vec![MenuItem::action("Quit Runebender", Quit)],
            disabled: false,
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Font", NewFont),
                MenuItem::action("Open…", OpenFont),
                MenuItem::separator(),
                MenuItem::action("Save", SaveFont),
                MenuItem::action("Save As…", SaveFontAs),
                MenuItem::separator(),
                MenuItem::action("Export…", ExportFont),
            ],
            disabled: false,
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::action("Undo", Undo),
                MenuItem::action("Redo", Redo),
                MenuItem::separator(),
                MenuItem::action("Copy", CopyContours),
                MenuItem::action("Paste", PasteContours),
                MenuItem::action("Copy Selected Glyphs as Text", CopySelectedGlyphs),
                MenuItem::separator(),
                MenuItem::action("Select All", SelectAllPoints),
                MenuItem::action("Deselect All", DeselectAllPoints),
                MenuItem::action("Invert Selection", InvertPointSelection),
            ],
            disabled: false,
        },
        // The Glyph / Path / Filter split mirrors Glyphs 4: Glyph
        // manages the glyph set, Path holds outline commands, Filter
        // the parameterized effects.
        Menu {
            name: "Glyph".into(),
            items: vec![
                MenuItem::action("New Glyph", NewGlyph),
                MenuItem::action("Duplicate Glyph", DuplicateGlyph),
                MenuItem::action("Remove Glyph", RemoveGlyphCmd),
                MenuItem::separator(),
                MenuItem::action("Update Metrics", SyncMetrics),
                MenuItem::action("Reinterpolate", Reinterpolate),
                MenuItem::action("Decompose Components", Decompose),
                MenuItem::separator(),
                MenuItem::action("Check Joining", CheckJoining),
                MenuItem::action("Bake Masks", BakeMasks),
                MenuItem::action("Export Glyph as SVG", ExportGlyphSvg),
                MenuItem::separator(),
                MenuItem::action("Trace Image…", TraceImage),
                MenuItem::action("Bolden With Model…", BoldenWithModel),
                MenuItem::action("Place Image…", PlaceImage),
                MenuItem::action("Import SVG…", ImportSvg),
                MenuItem::action("Remove Image", RemoveImage),
            ],
            disabled: false,
        },
        Menu {
            name: "Path".into(),
            items: vec![
                MenuItem::action("Tidy Up Paths", TidyPaths),
                MenuItem::action("Add Extremes", AddExtremes),
                MenuItem::action("Round Coordinates", RoundCoordinates),
                MenuItem::separator(),
                MenuItem::action("Correct Path Direction", CorrectPathDirection),
                MenuItem::action("Reverse Contours", ReverseContours),
                MenuItem::action("Set Start Point", SetStartPoint),
                MenuItem::separator(),
                MenuItem::action("Remove Overlap", RemoveOverlap),
                MenuItem::action("Union", BooleanUnion),
                MenuItem::action("Subtract", BooleanSubtract),
                MenuItem::action("Intersect", BooleanIntersect),
                MenuItem::action("Exclude", BooleanExclude),
                MenuItem::separator(),
                MenuItem::action("Flip Horizontal", FlipHorizontal),
                MenuItem::action("Flip Vertical", FlipVertical),
                MenuItem::action("Rotate 90° Left", RotateLeft),
                MenuItem::action("Rotate 90° Right", RotateRight),
                MenuItem::action("Rotate 180°", Rotate180),
                MenuItem::action("Duplicate Selection", DuplicateSelection),
                MenuItem::action("Duplicate + Repeat", DuplicateRepeat),
                MenuItem::separator(),
                MenuItem::action("Harmonize", Harmonize),
                MenuItem::action("Balance", Balance),
                MenuItem::action("Optimize", Optimize),
                MenuItem::separator(),
                MenuItem::action("Hyperbezier to Cubic", HyperToCubic),
                MenuItem::action("Quadratic to Cubic", QuadsToCubics),
                MenuItem::action("Cubic to Quadratic", CubicsToQuads),
            ],
            disabled: false,
        },
        // Parameterized filters take their values from the matching
        // grid-side section (Offset, Extrude, Roughen, Slanter).
        Menu {
            name: "Filter".into(),
            items: vec![
                MenuItem::action("Offset Curve", FilterOffsetCurve),
                MenuItem::action("Extrude", FilterExtrude),
                MenuItem::action("Roughen", FilterRoughen),
                MenuItem::action("Round Corners", RoundCorners),
                MenuItem::action("Slanter", FilterSlant),
                MenuItem::separator(),
                MenuItem::action("Add Extremes", AddExtremes),
                MenuItem::action("Remove Overlap", RemoveOverlap),
            ],
            disabled: false,
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Zoom to Fit", ZoomToFit),
                MenuItem::separator(),
                MenuItem::action("Show All Masters", ShowAllMasters),
                MenuItem::separator(),
                MenuItem::action("Sort Glyphs by Name", SortByName),
                MenuItem::action("Sort Glyphs by Unicode", SortByUnicode),
                MenuItem::separator(),
                MenuItem::action("Next Master", NextMaster),
                MenuItem::action("Previous Master", PreviousMaster),
                MenuItem::separator(),
                MenuItem::action("Next Sample String", NextSampleString),
                MenuItem::action("Previous Sample String", PreviousSampleString),
                MenuItem::separator(),
                MenuItem::Submenu(Menu {
                    name: "Measure".into(),
                    items: measure_menu_items(),
                    disabled: false,
                }),
                MenuItem::Submenu(Menu {
                    name: "Theme".into(),
                    items: theme_menu_items(),
                    disabled: false,
                }),
            ],
            disabled: false,
        },
    ]
}
