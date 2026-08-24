// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! The Runebender theme, resolved from the shared OKLCH token file in
//! runebender-core (`themes/runebender.theme.json`) — the same source
//! runebender-web generates its CSS variables from, so the editors
//! match byte-for-byte.

use std::sync::RwLock;

use gpui::Rgba;
use runebender_core::theme::ColorRgba;
use runebender_core::theme_oklch::{self, Theme};

/// The themes in the shared token file, in menu order.
pub const THEMES: [(&str, &str); 4] = [
    ("dark", "Dark"),
    ("midnight", "Midnight"),
    ("gray", "Gray"),
    ("light", "Light"),
];

/// The live theme. Resolving one is cheap but not free, and every
/// colour below reads it, so the resolved theme is kept behind a lock
/// and leaked: switching is rare, and a leaked theme gives every
/// accessor a `'static` reference with no per-call clone.
static CURRENT: RwLock<Option<(&'static str, &'static Theme)>> = RwLock::new(None);

fn theme() -> &'static Theme {
    if let Some((_, theme)) = *CURRENT.read().expect("theme lock") {
        return theme;
    }
    set_theme("dark");
    CURRENT
        .read()
        .expect("theme lock")
        .expect("dark theme in shared token file")
        .1
}

/// Switch the palette. Returns false for a name the token file does
/// not define, leaving the current theme alone.
pub fn set_theme(id: &str) -> bool {
    let Some((name, _)) = THEMES.iter().find(|(name, _)| *name == id) else {
        return false;
    };
    let Some(resolved) = theme_oklch::load_theme(id) else {
        return false;
    };
    *CURRENT.write().expect("theme lock") =
        Some((name, Box::leak(Box::new(resolved))));
    true
}

/// The active theme's id.
pub fn current_theme() -> &'static str {
    theme();
    CURRENT.read().expect("theme lock").map(|(id, _)| id).unwrap_or("dark")
}

fn c(color: ColorRgba) -> Rgba {
    Rgba {
        r: color.r as f32 / 255.0,
        g: color.g as f32 / 255.0,
        b: color.b as f32 / 255.0,
        a: color.a as f32 / 255.0,
    }
}

fn with_alpha(color: ColorRgba, a: f32) -> Rgba {
    let mut rgba = c(color);
    rgba.a = a;
    rgba
}

// ---- surfaces ----

pub fn window_bg() -> Rgba {
    c(theme().surface("app"))
}
pub fn panel_bg() -> Rgba {
    c(theme().surface("panel"))
}
pub fn panel_outline() -> Rgba {
    c(theme().surface("outline"))
}
pub fn cell_bg() -> Rgba {
    panel_bg()
}
pub fn cell_border() -> Rgba {
    c(theme().surface("outline"))
}
pub fn cell_selected_bg() -> Rgba {
    // Half way between the cell's own ground and the hover surface:
    // enough to read as picked, not so much that the cell jumps out of
    // the grid.
    let base = c(theme().surface("panel"));
    let lift = c(theme().surface("buttonHover"));
    Rgba {
        r: (base.r + lift.r) / 2.0,
        g: (base.g + lift.g) / 2.0,
        b: (base.b + lift.b) / 2.0,
        a: 1.0,
    }
}
/// Selected grid cell ring (neutral, like the web editor).
pub fn cell_selected_ring() -> Rgba {
    c(theme().role("gridSelected"))
}

// ---- accents and text ----

pub fn accent() -> Rgba {
    c(theme().role("accent"))
}
pub fn text() -> Rgba {
    c(theme().text("primary"))
}
pub fn text_muted() -> Rgba {
    c(theme().text("secondary"))
}
pub fn status_yellow() -> Rgba {
    c(theme().role("warning"))
}

// ---- glyph rendering ----

pub fn glyph_fill() -> Rgba {
    c(theme().text("glyph"))
}
pub fn path_stroke() -> Rgba {
    c(theme().role("pathStroke"))
}
pub fn metrics_line() -> Rgba {
    accent()
}
pub fn preview_glyph() -> Rgba {
    status_yellow()
}
pub fn component_fill() -> Rgba {
    with_alpha(theme().role("component"), 0.35)
}
pub fn component_selected_fill() -> Rgba {
    with_alpha(theme().role("componentSelected"), 0.45)
}
pub fn ghost() -> Rgba {
    c(theme().role("component"))
}
/// Zoom-dependent design grid, faded by the level's ramp-in alpha
/// (the web's DESIGN_GRID_FINE/COARSE, shared constants in core).
pub fn design_grid_fine(alpha: f32) -> Rgba {
    let mut rgba = c(runebender_core::theme::design_grid::FINE);
    rgba.a *= alpha;
    rgba
}
pub fn design_grid_coarse(alpha: f32) -> Rgba {
    let mut rgba = c(runebender_core::theme::design_grid::COARSE);
    rgba.a *= alpha;
    rgba
}
/// Greyed-out ring on read-only points (inactive sorts, zoomed in).
pub fn point_readonly() -> Rgba {
    Rgba {
        r: 0x8a as f32 / 255.0,
        g: 0x8a as f32 / 255.0,
        b: 0x8a as f32 / 255.0,
        a: 1.0,
    }
}

// ---- points (dark inner, colored ring — the web style) ----

pub fn point_inner() -> Rgba {
    c(theme().role("pointInner"))
}
pub fn point_smooth_outer() -> Rgba {
    c(theme().role("pointSmooth"))
}
pub fn point_corner_outer() -> Rgba {
    c(theme().role("pointCorner"))
}
pub fn point_hyper_outer() -> Rgba {
    c(theme().role("pointHyper"))
}
pub fn point_offcurve_outer() -> Rgba {
    c(theme().role("pointOffcurve"))
}
pub fn point_selected() -> Rgba {
    c(theme().role("pointSelected"))
}
pub fn handle_line() -> Rgba {
    c(theme().text("secondary"))
}

// ---- selection marquee ----

pub fn marquee_fill() -> Rgba {
    with_alpha(theme().role("selection"), 0.125)
}
pub fn marquee_stroke() -> Rgba {
    c(theme().role("selection"))
}

// ---- gpui-component theme ----

/// Configure gpui-component's widget theme (inputs, sliders, panels)
/// from the shared OKLCH tokens, so library widgets match the app
/// instead of shipping their default light look.
pub fn install_component_theme(cx: &mut gpui::App) {
    use gpui_component::{Theme, ThemeConfig, ThemeMode};

    let t = theme();
    let hex = |c: ColorRgba| -> gpui::SharedString {
        format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b).into()
    };
    let dark_surface = t.surface("app");
    let is_light =
        (dark_surface.r as u32 + dark_surface.g as u32 + dark_surface.b as u32) / 3 > 127;
    let mut config = ThemeConfig {
        name: format!("runebender-{}", current_theme()).into(),
        mode: if is_light { ThemeMode::Light } else { ThemeMode::Dark },
        ..Default::default()
    };
    config.colors.background = Some(hex(t.surface("app")));
    config.colors.foreground = Some(hex(t.text("primary")));
    config.colors.border = Some(hex(t.surface("outline")));
    // Inputs take both their border and their fill from this one
    // token in dark mode (the fill is it, mixed 30% toward
    // transparent), so the outline surface made a field that sat too
    // bright against the panel. The divider surface is a step darker.
    config.colors.input = Some(hex(t.surface("divider")));
    config.colors.primary = Some(hex(t.role("accent")));
    config.colors.primary_foreground = Some(hex(t.surface("app")));
    config.colors.accent = Some(hex(t.surface("buttonHover")));
    config.colors.accent_foreground = Some(hex(t.text("primary")));
    config.colors.muted = Some(hex(t.surface("panel")));
    config.colors.muted_foreground = Some(hex(t.text("subdued")));
    config.colors.popover = Some(hex(t.surface("panel")));
    config.colors.popover_foreground = Some(hex(t.text("primary")));
    config.colors.list = Some(hex(t.surface("panel")));
    config.colors.list_active = Some(hex(t.surface("buttonHover")));
    config.colors.list_hover = Some(hex(t.surface("button")));
    config.colors.danger = Some(hex(t.role("danger")));
    config.colors.caret = Some(hex(t.text("primary")));

    config.colors.ring = Some(hex(t.role("accent")));
    // The divider being dragged lights up: the resizable handles take
    // their idle colour from `border` and their active one from
    // `drag_border`.
    config.colors.drag_border = Some(hex(t.role("accent")));

    // A light palette needs the library in light mode, or widgets that
    // branch on the mode (not just on the colours) come out wrong.
    let app = t.surface("app");
    let light = (app.r as u32 + app.g as u32 + app.b as u32) / 3 > 127;
    let theme = Theme::global_mut(cx);
    let config = std::rc::Rc::new(config);
    if light {
        theme.light_theme = config;
        Theme::change(ThemeMode::Light, None, cx);
    } else {
        theme.dark_theme = config;
        Theme::change(ThemeMode::Dark, None, cx);
    }
    // Focused inputs keep a single accent border instead of the
    // thick translucent ring painted outside it.
    Theme::global_mut(cx).focus_ring = false;
}

// ---- glyph mark colors ----

/// The mark label a glyph carries (label key, else snapped color).
pub fn mark_label(glyph: &norad::Glyph) -> Option<String> {
    theme_oklch::mark_label_for_glyph(glyph, theme())
}

/// The display color for a mark label.
pub fn mark_color(label: &str) -> Option<Rgba> {
    theme().mark(label).map(c)
}

/// The full mark palette in order, for the Colors panel.
pub fn mark_palette() -> Vec<(String, Rgba)> {
    theme()
        .marks
        .iter()
        .map(|(name, color)| (name.clone(), c(*color)))
        .collect()
}

/// The text caret.
pub fn text_cursor() -> Rgba {
    c(theme().role("textCursor"))
}
/// Quiet per-sort metric boxes in the text buffer.
pub fn metric_quiet() -> Rgba {
    c(theme().role("metricQuiet"))
}
/// Global guides from fontinfo: the status yellow, thinned so the
/// metric lines stay the louder of the two.
pub fn guide_line() -> Rgba {
    let mut color = status_yellow();
    color.a = 0.75;
    color
}
/// The sort being manually kerned.
pub fn kern_active() -> Rgba {
    c(theme().role("kernActive"))
}
/// The sort before the one being kerned.
pub fn kern_previous() -> Rgba {
    c(theme().role("kernPrevious"))
}
/// Reference-layer underlay stroke (other masters shown via the
/// Layers eyes).
pub fn reference_layer() -> Rgba {
    c(theme().role("reference"))
}

// ---- curve overlays (web curve_gradient + continuity palette) ----

/// The comb's cool-to-warm curvature ramp.
pub fn comb_gradient(t: f64) -> Rgba {
    const STOPS: [[f32; 3]; 5] = [
        [0.16, 0.80, 0.82], // teal
        [0.40, 0.44, 0.95], // indigo
        [0.86, 0.28, 0.72], // magenta
        [1.00, 0.55, 0.24], // orange
        [1.00, 0.84, 0.36], // amber
    ];
    let u = (t.clamp(0.0, 1.0) as f32) * (STOPS.len() as f32 - 1.0);
    let i = (u.floor() as usize).min(STOPS.len() - 2);
    let f = u - i as f32;
    let (a, b) = (STOPS[i], STOPS[i + 1]);
    Rgba {
        r: a[0] + (b[0] - a[0]) * f,
        g: a[1] + (b[1] - a[1]) * f,
        b: a[2] + (b[2] - a[2]) * f,
        a: 1.0,
    }
}

pub fn continuity_g2() -> Rgba {
    Rgba { r: 0.30, g: 0.85, b: 0.55, a: 1.0 }
}
pub fn continuity_g1() -> Rgba {
    Rgba { r: 0.95, g: 0.80, b: 0.30, a: 1.0 }
}
pub fn continuity_line() -> Rgba {
    Rgba { r: 0.55, g: 0.62, b: 0.70, a: 1.0 }
}
pub fn continuity_kink() -> Rgba {
    Rgba { r: 0.95, g: 0.35, b: 0.30, a: 1.0 }
}

// ---- measure HUD (web POPCOUNT_1..4 + HALO_COLOR) ----

/// Popcount tier ramp: 1 power is structural (green), 2 an elegant
/// sum (yellow), 3 acceptable (orange), 4+ a flagged correction (red).
pub fn popcount_tier(pc: u32) -> Rgba {
    match pc {
        0 | 1 => Rgba { r: 0.09, g: 0.72, b: 0.44, a: 1.0 },
        2 => Rgba { r: 1.0, g: 0.86, b: 0.20, a: 1.0 },
        3 => Rgba { r: 1.0, g: 0.60, b: 0.06, a: 1.0 },
        _ => Rgba { r: 1.0, g: 0.29, b: 0.24, a: 1.0 },
    }
}

/// The dark casing drawn under points and labels so they keep an edge
/// over an outline or the curvature comb (web HALO).
pub fn halo() -> Rgba {
    c(theme().role("halo"))
}

/// The ring around a selected point (web `pointSelectedOuter`, which
/// the app feeds from the selection colour).
pub fn point_selected_ring() -> Rgba {
    c(theme().role("selection"))
}


// ---- anchors ----

/// Anchors are pink, so they read as their own kind of thing beside
/// on-curve and off-curve points (web ANCHOR_MARK_PINK).
pub fn anchor() -> Rgba {
    theme()
        .mark("pink")
        .map(c)
        .unwrap_or_else(|| c(theme().role("danger")))
}
