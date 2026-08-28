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
pub const THEMES: [(&str, &str); 5] = [
    ("dark", "Dark"),
    ("midnight", "Midnight"),
    ("gray", "Gray"),
    ("paper", "Paper"),
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

// ---- geometry ----
//
// Shape is themed the same way colour is. Call these instead of
// `rounded_sm()` / `border_1()`, or a theme cannot change them.

/// Corner radius for small chrome: tiles, tabs, swatches.
pub fn radius_sm() -> gpui::Pixels {
    gpui::px(theme().geometry.radius_small)
}

/// Corner radius for larger surfaces: panels, popovers, buttons.
pub fn radius_md() -> gpui::Pixels {
    gpui::px(theme().geometry.radius_medium)
}

/// Border width for every themed rule.
pub fn stroke() -> gpui::Pixels {
    gpui::px(theme().geometry.stroke)
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
/// The accent behind selected text: the same hue, quiet enough that
/// the glyphs on top stay readable.
pub fn accent_soft() -> Rgba {
    let a = accent();
    Rgba { a: 0.28, ..a }
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
/// Local (per-glyph) guides: the accent hue, thinned the same way,
/// so the two guide scopes read apart at a glance.
pub fn guide_local() -> Rgba {
    let mut color = accent();
    color.a = 0.75;
    color
}
/// Alignment-zone bands: the accent at a whisper, Glyphs' beige
/// zones in this palette's terms.
pub fn zone_band() -> Rgba {
    let mut color = accent();
    color.a = 0.10;
    color
}
/// Annotation marks: arrows, circles, and notes in the kern-drag
/// red, full strength — working marks should shout a little.
pub fn annotation() -> Rgba {
    c(theme().role("kernActive"))
}
/// The HOI velocity ribbon's speed ramp: slow steps in a deep
/// ember, fast ones in gold — Glyphs' Show velocity, in this
/// palette's warm terms. `t` is the normalized speed.
pub fn velocity_ramp(t: f64) -> Rgba {
    let t = t.clamp(0.0, 1.0) as f32;
    let slow = (0.52, 0.16, 0.10);
    let fast = (0.87, 0.62, 0.16);
    Rgba {
        r: slow.0 + (fast.0 - slow.0) * t,
        g: slow.1 + (fast.1 - slow.1) * t,
        b: slow.2 + (fast.2 - slow.2) * t,
        a: 0.55,
    }
}
/// HOI node trajectories: the across-the-axis connector line…
pub fn trajectory_line() -> Rgba {
    with_alpha(theme().role("kernActive"), 0.55)
}
/// …and its velocity dots.
pub fn trajectory_dot() -> Rgba {
    c(theme().role("kernActive"))
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

#[cfg(test)]
mod colour_tests {
    #[test]
    fn text_colours_are_opaque() {
        for (name, c) in [
            ("text", super::text()),
            ("text_muted", super::text_muted()),
            ("accent", super::accent()),
        ] {
            println!("{name}: r={} g={} b={} a={}", c.r, c.g, c.b, c.a);
            assert!(c.a > 0.0, "{name} is fully transparent");
        }
    }
}

#[cfg(test)]
mod perf {
    /// Theme accessors are called hundreds of times per frame, so the
    /// cost of one has to be negligible. This is a floor check, not a
    /// benchmark: it fails only if a lookup becomes expensive enough
    /// to matter at that rate.
    #[test]
    fn a_theme_lookup_is_cheap() {
        let n = 100_000;
        let start = std::time::Instant::now();
        let mut sink = 0f32;
        for _ in 0..n {
            sink += super::text().r + super::accent().g;
        }
        let each = start.elapsed().as_nanos() as f64 / n as f64;
        println!("{each:.0} ns per lookup (sink {sink})");
        assert!(
            each < 2000.0,
            "a theme lookup costs {each:.0}ns; at ~1000 per frame that is \
             visible"
        );
    }
}
