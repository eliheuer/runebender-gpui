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
pub fn ghost() -> Rgba {
    c(theme().role("component"))
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

// ---- anchors ----

pub fn anchor() -> Rgba {
    c(theme().role("danger"))
}
