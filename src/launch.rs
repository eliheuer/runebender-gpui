// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The launch path: the keymap, the keystroke router, the font named
//! on the command line, `--fonts`, and which family the interface
//! text uses. Everything `main()` calls before the window opens.

use crate::CopyContours;
use crate::CorrectPathDirection;
use crate::Decompose;
use crate::DeselectAllPoints;
use crate::DuplicateRepeat;
use crate::DuplicateSelection;
use crate::ExportFont;
use crate::FlipHorizontal;
use crate::FlipVertical;
use crate::InvertPointSelection;
use crate::Mode;
use crate::NewFont;
use crate::NewGlyph;
use crate::OpenFont;
use crate::PasteContours;
use crate::Quit;
use crate::Redo;
use crate::RemoveOverlap;
use crate::ReverseContours;
use crate::SaveFont;
use crate::SaveFontAs;
use crate::SelectAllPoints;
use crate::SyncMetrics;
use crate::TidyPaths;
use crate::Undo;
use crate::Workspace;
use crate::ZoomToFit;
#[cfg(not(target_family = "wasm"))]
use crate::widgets;
use gpui::App;
use gpui::SharedString;
use kurbo::Affine;
#[cfg(not(target_family = "wasm"))]
use runebender_core::document::project::Project;
#[cfg(not(target_family = "wasm"))]
use std::path::PathBuf;
impl Workspace {
    /// Route keystrokes before any binding runs, so Tab cycles the
    /// point selection rather than walking tab stops. On the web
    /// build this also stands in for action dispatch.
    pub(crate) fn install_shortcuts(workspace: &gpui::Entity<Self>, cx: &mut App) {
        let shortcut_target = workspace.clone();
        cx.intercept_keystrokes(move |event, window, cx| {
            let ks = &event.keystroke;
            let cmd = ks.modifiers.platform;
            let shift = ks.modifiers.shift;
            if ks.modifiers.control || ks.modifiers.alt {
                return;
            }
            // A focused text field owns its keystrokes,
            // including Tab and the clipboard.
            if widgets::input::any_field_focused(window, cx) {
                return;
            }
            if ks.key == "tab" && !cmd {
                cx.stop_propagation();
                shortcut_target.update(cx, |this, cx| {
                    if this.command_cycle_point(shift) {
                        cx.notify();
                    }
                });
                return;
            }
            if !cfg!(target_family = "wasm") || !cmd {
                return;
            }
            let handled = shortcut_target.update(cx, |this, cx| {
                match (ks.key.as_str(), shift) {
                    ("s", false) => this.command_save(cx),
                    ("n", false) => this.command_new_font(),
                    ("z", false) => {
                        this.undo();
                        this.rebuild_text_models();
                    }
                    ("z", true) => {
                        this.redo();
                        this.rebuild_text_models();
                    }
                    ("c", false) => this.command_copy(),
                    ("v", false) => this.command_paste_routed(cx),
                    ("o", true) => this.command_remove_overlap(),
                    ("d", true) => this.command_decompose(),
                    ("d", false) => this.command_duplicate(),
                    ("t", true) => this.command_duplicate_repeat(),
                    ("h", true) => {
                        this.apply_transform(Affine::scale_non_uniform(-1.0, 1.0));
                    }
                    ("v", true) => {
                        this.apply_transform(Affine::scale_non_uniform(1.0, -1.0));
                    }
                    ("r", true) => this.command_reverse(),
                    ("0", false) => {
                        if matches!(this.mode, Mode::Editor(_)) {
                            this.editor.initialized = false;
                            this.ensure_editor_fit();
                        }
                    }
                    _ => return false,
                }
                cx.notify();
                true
            });
            if handled {
                cx.stop_propagation();
            }
        })
        .detach();
    }
}

/// The keymap for app commands. Menu items show these as their key
/// equivalents.
pub(crate) fn keymap() -> Vec<gpui::KeyBinding> {
    vec![
        gpui::KeyBinding::new("cmd-o", OpenFont, None),
        gpui::KeyBinding::new("cmd-n", NewFont, None),
        gpui::KeyBinding::new("cmd-shift-s", SaveFontAs, None),
        gpui::KeyBinding::new("cmd-e", ExportFont, None),
        gpui::KeyBinding::new("cmd-s", SaveFont, None),
        gpui::KeyBinding::new("cmd-z", Undo, None),
        gpui::KeyBinding::new("cmd-shift-z", Redo, None),
        gpui::KeyBinding::new("cmd-c", CopyContours, None),
        gpui::KeyBinding::new("cmd-v", PasteContours, None),
        gpui::KeyBinding::new("cmd-shift-o", RemoveOverlap, None),
        gpui::KeyBinding::new("cmd-shift-d", Decompose, None),
        gpui::KeyBinding::new("cmd-q", Quit, None),
        gpui::KeyBinding::new("cmd-shift-h", FlipHorizontal, None),
        gpui::KeyBinding::new("cmd-shift-v", FlipVertical, None),
        // Glyphs' shortcuts where they translate: Cmd-Shift-R
        // corrects direction, Cmd-Shift-T tidies (reverse and
        // duplicate-repeat move to Opt variants).
        gpui::KeyBinding::new("cmd-shift-r", CorrectPathDirection, None),
        gpui::KeyBinding::new("cmd-alt-shift-r", ReverseContours, None),
        gpui::KeyBinding::new("cmd-0", ZoomToFit, None),
        gpui::KeyBinding::new("cmd-d", DuplicateSelection, None),
        gpui::KeyBinding::new("cmd-shift-t", TidyPaths, None),
        gpui::KeyBinding::new("cmd-alt-shift-t", DuplicateRepeat, None),
        gpui::KeyBinding::new("cmd-ctrl-m", SyncMetrics, None),
        gpui::KeyBinding::new("cmd-a", SelectAllPoints, None),
        gpui::KeyBinding::new("cmd-alt-a", DeselectAllPoints, None),
        gpui::KeyBinding::new("cmd-alt-shift-i", InvertPointSelection, None),
        gpui::KeyBinding::new("cmd-alt-shift-n", NewGlyph, None),
    ]
}

/// Open the font named on the command line, or the development
/// default when there is none. The error, if any, goes to the status
/// bar rather than stopping the launch.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn open_from_args() -> (Option<Project>, Option<SharedString>) {
    // No path means no font: the window opens empty with File → Open.
    // Nothing on one machine is a default for everyone else's.
    let Some(font_path) = std::env::args().nth(1).map(PathBuf::from) else {
        return (None, None);
    };
    match Project::load(&font_path) {
        Ok(p) => (Some(p), None),
        Err(e) => (None, Some(e.into())),
    }
}

/// List the families gpui can resolve and which of them fall
/// back, for `runebender-gpui --fonts`. A family that shapes to
/// nothing leaves the interface wordless, so this is the first
/// check when it does.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn print_font_families(cx: &mut App) {
    let names = cx.text_system().all_font_names();
    println!("{} families", names.len());
    for name in names {
        // Listed is not the same as loadable: report which
        // families actually resolve, and to what.
        let font = gpui::font(name.as_str());
        let resolved = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cx.text_system().resolve_font(&font)
        }));
        match resolved {
            Ok(id) => {
                let got = cx.text_system().get_font_for_id(id);
                let same = got.as_ref().is_some_and(|f| f.family == name.as_str());
                println!("{name}\t{}", if same { "ok" } else { "FELL BACK" });
            }
            Err(_) => println!("{name}\tPANICKED"),
        }
    }
    // Can the resolved family actually measure a glyph? If
    // this errors the pipeline is broken; if it works the
    // fault is in the style, not the font.
    let font = gpui::font(ui_font_family(cx).as_ref());
    let id = cx.text_system().resolve_font(&font);
    println!(
        "chosen: {} em_width={:?} advance={:?}",
        ui_font_family(cx),
        cx.text_system().em_width(id, gpui::px(13.0)),
        cx.text_system().advance(id, gpui::px(13.0), 'A'),
    );
}

/// The interface font, resolved once against what the platform
/// actually has. A name gpui cannot resolve shapes to nothing, and
/// no text draws at all. So the preferences are tried in order,
/// and the first family the text system reports wins.
pub(crate) fn ui_font_family(cx: &App) -> SharedString {
    // Cached: asking the platform for its font list takes about 140ms,
    // and this is read once per frame. Uncached it capped the whole
    // editor at roughly seven frames a second.
    static RESOLVED: std::sync::OnceLock<SharedString> = std::sync::OnceLock::new();
    if let Some(name) = RESOLVED.get() {
        return name.clone();
    }
    let name = resolve_ui_font_family(cx);
    RESOLVED.set(name.clone()).ok();
    name
}

/// The uncached lookup. Runs once.
pub(crate) fn resolve_ui_font_family(cx: &App) -> SharedString {
    // Each platform's own interface font first, then the families that
    // are actually installed on that platform, then a last-resort
    // shared one. Ordered per platform rather than in one list, or a
    // Linux session walks past four macOS families it will never have
    // before reaching anything it does.
    #[cfg(target_os = "macos")]
    const PREFERRED: &[&str] = &[
        ".SystemUIFont",
        "SF Pro Text",
        "SF Pro Display",
        "Helvetica Neue",
        "Helvetica",
        "Arial",
    ];
    #[cfg(target_os = "windows")]
    const PREFERRED: &[&str] = &["Segoe UI Variable Text", "Segoe UI", "Inter", "Arial"];
    // Cantarell ships with GNOME, Noto Sans with most distributions,
    // DejaVu Sans is the long-standing fallback, and Liberation Sans
    // is the metric-compatible stand-in for Arial.
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    const PREFERRED: &[&str] = &[
        "Inter",
        "Cantarell",
        "Noto Sans",
        "DejaVu Sans",
        "Liberation Sans",
        "Arial",
    ];
    let available = cx.text_system().all_font_names();
    // A handful of families means gpui is on its embedded fallback
    // list rather than the platform's fonts, which is what happens if
    // gpui_platform loses the font-kit feature. Text then shapes and
    // paints without ever reaching the screen, so say so here rather
    // than leave a wordless window to be puzzled over.
    static REPORTED: std::sync::Once = std::sync::Once::new();
    if available.len() < 50 {
        REPORTED.call_once(|| {
            eprintln!(
                "warning: only {} font families visible, so text may not \
                 render. Check that gpui_platform still has the font-kit \
                 feature; --fonts lists what it can see.",
                available.len()
            );
        });
    }
    for name in PREFERRED {
        if available.iter().any(|f| f == name) {
            return (*name).into();
        }
    }
    // Nothing preferred is installed: take whatever there is rather
    // than render an empty window.
    available
        .into_iter()
        .next()
        .map(Into::into)
        .unwrap_or_else(|| "Helvetica".into())
}
