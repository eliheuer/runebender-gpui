// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! The Runebender theme, resolved from the shared OKLCH token file in
//! runebender-core (`themes/runebender.theme.json`) — the same source
//! runebender-web generates its CSS variables from, so the editors
//! match byte-for-byte.

use std::sync::OnceLock;

use gpui::Rgba;
use runebender_core::theme::ColorRgba;
use runebender_core::theme_oklch::{self, Theme};

fn theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        theme_oklch::load_theme("dark").expect("dark theme in shared token file")
    })
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
    c(theme().surface("buttonHover"))
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
pub fn info_header() -> Rgba {
    accent()
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
    let mut config = ThemeConfig {
        name: "runebender-dark".into(),
        mode: ThemeMode::Dark,
        ..Default::default()
    };
    config.colors.background = Some(hex(t.surface("app")));
    config.colors.foreground = Some(hex(t.text("primary")));
    config.colors.border = Some(hex(t.surface("outline")));
    config.colors.input = Some(hex(t.surface("outline")));
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

    let theme = Theme::global_mut(cx);
    theme.dark_theme = std::rc::Rc::new(config);
    Theme::change(ThemeMode::Dark, None, cx);
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

// ---- anchors ----

pub fn anchor() -> Rgba {
    c(theme().role("danger"))
}
