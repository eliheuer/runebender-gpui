// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Runebender theme, resolved from the shared OKLCH token file in
//! runebender-core (`themes/runebender.theme.json`).
//!
//! runebender-web generates its CSS variables from the same file, so
//! the editors match byte-for-byte.

use std::sync::RwLock;

use crate::view::render::px32;
use crate::view::render::to_count;
use gpui::Rgba;
use runebender_core::ui::color::ColorRgba;
use runebender_core::ui::theme::{self, Theme};

/// The themes in the shared token file, in menu order.
pub(crate) const THEMES: [(&str, &str); 3] =
    [("dark", "Dark"), ("gray", "Gray"), ("light", "Light")];

/// The live theme.
///
/// Resolving one is cheap but not free, and every colour below reads
/// it. So the resolved theme is kept behind a lock and leaked.
/// Switching is rare, and a leaked theme gives every accessor a
/// `'static` reference with no per-call clone.
static CURRENT: RwLock<Option<(&'static str, &'static Theme)>> = RwLock::new(None);

/// The theme a fresh install starts in.
///
/// One name, so the fallback in `theme()` and the answer from
/// `current_theme()` cannot disagree about what "no choice yet" means.
pub(crate) const DEFAULT_THEME: &str = "gray";

/// The resolved live theme, loading `DEFAULT_THEME` on the first call.
fn theme() -> &'static Theme {
    if let Some((_, theme)) = *CURRENT.read().expect("theme lock") {
        return theme;
    }
    set_theme(DEFAULT_THEME);
    CURRENT
        .read()
        .expect("theme lock")
        .expect("the default theme is in the shared token file")
        .1
}

/// Switch the palette.
///
/// A name the token file does not define leaves the current theme
/// alone. Returns `false` for such a name.
pub(crate) fn set_theme(id: &str) -> bool {
    let Some((name, _)) = THEMES.iter().find(|(name, _)| *name == id) else {
        return false;
    };
    let Some(resolved) = theme::load_theme(id) else {
        return false;
    };
    *CURRENT.write().expect("theme lock") = Some((name, Box::leak(Box::new(resolved))));
    true
}

/// The active theme's id.
pub(crate) fn current_theme() -> &'static str {
    theme();
    CURRENT
        .read()
        .expect("theme lock")
        .map(|(id, _)| id)
        .unwrap_or(DEFAULT_THEME)
}

/// Converts a core `ColorRgba` with 0..=255 channels to a gpui `Rgba`
/// with 0.0..=1.0 channels.
fn c(color: ColorRgba) -> Rgba {
    Rgba {
        r: color.r as f32 / 255.0,
        g: color.g as f32 / 255.0,
        b: color.b as f32 / 255.0,
        a: color.a as f32 / 255.0,
    }
}

// ---- marks ----

/// How a mark is drawn on a cell.
///
/// Themes disagree. Tinting a rule works on a near-black or
/// near-white ground and fails on a mid grey. There, a hue saturated
/// enough to read sits at mid lightness too.
pub(crate) struct MarkPaint {
    /// Cell fill, when the theme fills its marks.
    pub bg: Option<Rgba>,
    /// Cell rule.
    pub border: Rgba,
    /// Label colour.
    pub ink: Rgba,
}

/// The paint for a marked cell, or `None` when the glyph has no mark.
/// One place decides, so the grid, the detail view and the list cannot
/// drift apart on it.
pub(crate) fn mark_paint(label: Option<&str>) -> Option<MarkPaint> {
    let color = mark_color(label?)?;
    let theme = theme();
    Some(match theme.mark_style {
        theme::MarkStyle::Fill => MarkPaint {
            bg: Some(color),
            border: theme.mark_outline.map(c).unwrap_or_else(cell_border),
            ink: theme.mark_ink.map(c).unwrap_or_else(text),
        },
        theme::MarkStyle::Border => MarkPaint {
            bg: None,
            border: color,
            ink: color,
        },
    })
}

// ---- geometry ----
//
// Shape is themed the same way colour is. Call these instead of
// `rounded_sm()` / `border_1()`, or a theme cannot change them.

/// The default corner, on small chrome.
pub(crate) fn radius() -> gpui::Pixels {
    gpui::px(theme().geometry.radius)
}

/// Pressable tiles: toolbar tiles, sidebar tabs, toggles.
pub(crate) fn radius_control() -> gpui::Pixels {
    gpui::px(theme().geometry.radius_control)
}

/// The ordinary rule, on panels and chrome.
pub(crate) fn stroke() -> gpui::Pixels {
    gpui::px(theme().geometry.stroke)
}

/// Rings that mark a thing selected or grabbable. Never `stroke()`
/// doubled: that assumes a 1px base and goes too heavy from a 2px one.
pub(crate) fn stroke_emphasis() -> gpui::Pixels {
    gpui::px(theme().geometry.stroke_emphasis)
}

/// Converts `color` like `c`, then replaces the alpha with `a`.
fn with_alpha(color: ColorRgba, a: f32) -> Rgba {
    let mut rgba = c(color);
    rgba.a = a;
    rgba
}

// ---- surfaces ----

/// The window ground, behind every panel.
pub(crate) fn window_bg() -> Rgba {
    c(theme().surface("app"))
}
/// The fill of panels and bars.
pub(crate) fn panel_bg() -> Rgba {
    c(theme().surface("panel"))
}
/// The header row that stands in for the title bar: a step darker
/// than the panels, so the window controls sit on contrast.
pub(crate) fn titlebar_bg() -> Rgba {
    c(theme().surface("titlebar"))
}
/// The rule around a panel.
pub(crate) fn panel_outline() -> Rgba {
    c(theme().surface("outline"))
}
/// The fill of a text field: one step darker than the panel it sits
/// on, so it reads as a place to type without becoming a box.
pub(crate) fn field_bg() -> Rgba {
    c(theme().surface("field"))
}
/// The rule around a text field: quieter than a panel's, so a column
/// of fields reads as values, not as boxes.
pub(crate) fn field_outline() -> Rgba {
    c(theme().surface("fieldOutline"))
}
/// The fill of an unselected grid cell; the panel fill.
pub(crate) fn cell_bg() -> Rgba {
    panel_bg()
}
/// The rule around a grid cell.
pub(crate) fn cell_border() -> Rgba {
    c(theme().surface("outline"))
}
/// The fill of a selected grid cell. A theme decides: Gray and Light
/// invert to the ink, Dark lifts the cell instead, because a light
/// slab on a dark grid shouts.
pub(crate) fn cell_selected_bg() -> Rgba {
    c(theme().role("cellSelectedFill"))
}
/// The glyph and label on a selected grid cell.
pub(crate) fn cell_selected_ink() -> Rgba {
    c(theme().role("cellSelectedInk"))
}
/// Selected grid cell ring.
pub(crate) fn cell_selected_ring() -> Rgba {
    c(theme().role("gridSelected"))
}

// ---- selection ----

/// The fill of a selected control: the ink itself. A selected state
/// is shown by inverting, not by a hue, so it reads in every theme and
/// for every eye.
pub(crate) fn selected_bg() -> Rgba {
    text()
}
/// The text and icon colour on a selected control: the panel's fill.
pub(crate) fn selected_ink() -> Rgba {
    panel_bg()
}

// ---- accents and text ----

/// The primary text ink.
pub(crate) fn text() -> Rgba {
    c(theme().text("primary"))
}
/// The secondary, quieter text ink.
pub(crate) fn text_muted() -> Rgba {
    c(theme().text("secondary"))
}
/// The wash behind selected text: the ink at a whisper, so the
/// glyphs on top stay readable and no hue is spent on it.
pub(crate) fn accent_soft() -> Rgba {
    let a = text();
    Rgba { a: 0.22, ..a }
}
/// The warning hue, used for status text and the preview glyph.
pub(crate) fn status_yellow() -> Rgba {
    c(theme().role("warning"))
}

// ---- glyph rendering ----

/// The fill of glyph outlines in cells and previews.
pub(crate) fn glyph_fill() -> Rgba {
    c(theme().text("glyph"))
}
/// The stroke of a glyph path in the editing view.
pub(crate) fn path_stroke() -> Rgba {
    c(theme().role("pathStroke"))
}
/// The glyph's fill in the editing view: a mid tone under the
/// outline, so the shape reads at a glance without covering the
/// points. Its own token, because the cell fill (`glyph_fill`) is
/// ink and this is not.
pub(crate) fn outline_fill() -> Rgba {
    c(theme().role("outlineFill"))
}
/// Metric lines such as baseline and x-height: a quiet neutral rule.
/// The outline is what the canvas is for; the metrics sit under it.
pub(crate) fn metrics_line() -> Rgba {
    c(theme().role("metricsLine"))
}
/// The preview-mode glyph fill; the status yellow.
pub(crate) fn preview_glyph() -> Rgba {
    status_yellow()
}
/// The translucent fill of a component.
pub(crate) fn component_fill() -> Rgba {
    with_alpha(theme().role("component"), 0.35)
}
/// The translucent fill of a selected component.
pub(crate) fn component_selected_fill() -> Rgba {
    with_alpha(theme().role("componentSelected"), 0.45)
}
/// The interpolated-instance ghost outline; the component hue at full
/// alpha.
pub(crate) fn ghost() -> Rgba {
    c(theme().role("component"))
}
/// The zoom-dependent design grid line, faded by the level's ramp-in
/// alpha. The colour is a theme role, so the grid sits under the
/// outline on a light theme as well as a dark one.
pub(crate) fn design_grid_fine(alpha: f32) -> Rgba {
    let mut rgba = c(theme().role("designGridFine"));
    rgba.a *= alpha;
    rgba
}
/// The coarse design grid line, faded by `alpha` like the fine one.
pub(crate) fn design_grid_coarse(alpha: f32) -> Rgba {
    let mut rgba = c(theme().role("designGridCoarse"));
    rgba.a *= alpha;
    rgba
}
/// Greyed-out ring on read-only points (inactive sorts, zoomed in).
pub(crate) fn point_readonly() -> Rgba {
    Rgba {
        r: 0x8a as f32 / 255.0,
        g: 0x8a as f32 / 255.0,
        b: 0x8a as f32 / 255.0,
        a: 1.0,
    }
}

// ---- points (dark inner, colored ring — the web style) ----

/// The dark inner fill every point marker shares.
pub(crate) fn point_inner() -> Rgba {
    c(theme().role("pointInner"))
}
/// The ring on a smooth on-curve point.
pub(crate) fn point_smooth_outer() -> Rgba {
    c(theme().role("pointSmooth"))
}
/// The ring on a corner on-curve point.
pub(crate) fn point_corner_outer() -> Rgba {
    c(theme().role("pointCorner"))
}
/// The ring on a hyperbezier point.
pub(crate) fn point_hyper_outer() -> Rgba {
    c(theme().role("pointHyper"))
}
/// The ring on an off-curve control point.
pub(crate) fn point_offcurve_outer() -> Rgba {
    c(theme().role("pointOffcurve"))
}
/// The fill of a selected point marker.
pub(crate) fn point_selected() -> Rgba {
    c(theme().role("pointSelected"))
}
/// The line from an on-curve point to its off-curve handle.
pub(crate) fn handle_line() -> Rgba {
    c(theme().text("secondary"))
}

// ---- selection marquee ----

/// The translucent interior of the drag-selection marquee.
pub(crate) fn marquee_fill() -> Rgba {
    with_alpha(theme().role("selection"), 0.125)
}
/// The outline of the drag-selection marquee.
pub(crate) fn marquee_stroke() -> Rgba {
    c(theme().role("selection"))
}

// ---- gpui-component theme ----
// ---- glyph mark colors ----

/// The display color for a mark label.
pub(crate) fn mark_color(label: &str) -> Option<Rgba> {
    theme().mark(label).map(c)
}

/// The full mark palette in order, for the Colors panel.
pub(crate) fn mark_palette() -> Vec<(String, Rgba)> {
    theme()
        .marks
        .iter()
        .map(|(name, color)| (name.clone(), c(*color)))
        .collect()
}

/// The text caret.
pub(crate) fn text_cursor() -> Rgba {
    c(theme().role("textCursor"))
}
/// Quiet per-sort metric boxes in the text buffer.
pub(crate) fn metric_quiet() -> Rgba {
    c(theme().role("metricQuiet"))
}
/// Global guides from fontinfo: the status yellow, thinned so the
/// metric lines stay the louder of the two.
pub(crate) fn guide_line() -> Rgba {
    let mut color = status_yellow();
    color.a = 0.75;
    color
}
/// Local (per-glyph) guides: the ink, thinned, so they read apart
/// from the yellow global guides without a second hue.
pub(crate) fn guide_local() -> Rgba {
    let mut color = text();
    color.a = 0.6;
    color
}
/// Alignment-zone bands: the ink at a whisper. These are the beige
/// zones in Glyphs, in this palette's terms.
pub(crate) fn zone_band() -> Rgba {
    let mut color = text();
    color.a = 0.08;
    color
}
/// Live tool feedback on the canvas: the pen's next segment, a shape
/// being dragged out, the measure line, the segment under the
/// pointer. Ink, so it reads on every theme and never competes with
/// the point colours for meaning.
pub(crate) fn tool_feedback() -> Rgba {
    text()
}
/// Annotation marks: arrows, circles, and notes in the kern-drag
/// red, full strength. Working marks should shout a little.
pub(crate) fn annotation() -> Rgba {
    c(theme().role("kernActive"))
}
/// The HOI velocity ribbon's speed ramp: slow steps in a deep
/// ember, fast ones in gold. `t` is the normalized speed. This is
/// Show velocity in Glyphs, in this palette's warm terms.
pub(crate) fn velocity_ramp(t: f64) -> Rgba {
    let t = px32(t.clamp(0.0, 1.0));
    let slow = (0.52, 0.16, 0.10);
    let fast = (0.87, 0.62, 0.16);
    Rgba {
        r: slow.0 + (fast.0 - slow.0) * t,
        g: slow.1 + (fast.1 - slow.1) * t,
        b: slow.2 + (fast.2 - slow.2) * t,
        a: 0.55,
    }
}
/// The HOI node trajectory connector line, across the axis.
pub(crate) fn trajectory_line() -> Rgba {
    with_alpha(theme().role("kernActive"), 0.55)
}
/// The velocity dots on an HOI node trajectory.
pub(crate) fn trajectory_dot() -> Rgba {
    c(theme().role("kernActive"))
}
/// The sort being manually kerned.
pub(crate) fn kern_active() -> Rgba {
    c(theme().role("kernActive"))
}
/// The sort before the one being kerned.
pub(crate) fn kern_previous() -> Rgba {
    c(theme().role("kernPrevious"))
}
/// Reference-layer underlay stroke. A reference layer is another
/// master shown via the Layers eyes.
pub(crate) fn reference_layer() -> Rgba {
    c(theme().role("reference"))
}

// ---- curve overlays (web curve_gradient + continuity palette) ----

/// The comb's cool-to-warm curvature ramp.
pub(crate) fn comb_gradient(t: f64) -> Rgba {
    const STOPS: [[f32; 3]; 5] = [
        [0.16, 0.80, 0.82], // teal
        [0.40, 0.44, 0.95], // indigo
        [0.86, 0.28, 0.72], // magenta
        [1.00, 0.55, 0.24], // orange
        [1.00, 0.84, 0.36], // amber
    ];
    let u = px32(t.clamp(0.0, 1.0)) * (STOPS.len() as f32 - 1.0);
    let i = usize::try_from(to_count(u.floor()))
        .unwrap_or(0)
        .min(STOPS.len() - 2);
    let f = u - i as f32;
    let (a, b) = (STOPS[i], STOPS[i + 1]);
    Rgba {
        r: a[0] + (b[0] - a[0]) * f,
        g: a[1] + (b[1] - a[1]) * f,
        b: a[2] + (b[2] - a[2]) * f,
        a: 1.0,
    }
}

/// Continuity badge for a curvature-continuous (G2 or better) joint:
/// green.
pub(crate) fn continuity_g2() -> Rgba {
    Rgba {
        r: 0.30,
        g: 0.85,
        b: 0.55,
        a: 1.0,
    }
}
/// Continuity badge for a tangent-only (G1) joint: yellow.
pub(crate) fn continuity_g1() -> Rgba {
    Rgba {
        r: 0.95,
        g: 0.80,
        b: 0.30,
        a: 1.0,
    }
}
/// Continuity badge where a curve meets a straight line: neutral grey.
pub(crate) fn continuity_line() -> Rgba {
    Rgba {
        r: 0.55,
        g: 0.62,
        b: 0.70,
        a: 1.0,
    }
}
/// Continuity badge for a kink: red.
pub(crate) fn continuity_kink() -> Rgba {
    Rgba {
        r: 0.95,
        g: 0.35,
        b: 0.30,
        a: 1.0,
    }
}

// ---- measure HUD (web POPCOUNT_1..4 + HALO_COLOR) ----

/// Popcount tier ramp: 1 power is structural (green), 2 an elegant
/// sum (yellow), 3 acceptable (orange), 4+ a flagged correction (red).
pub(crate) fn popcount_tier(pc: u32) -> Rgba {
    match pc {
        0 | 1 => Rgba {
            r: 0.09,
            g: 0.72,
            b: 0.44,
            a: 1.0,
        },
        2 => Rgba {
            r: 1.0,
            g: 0.86,
            b: 0.20,
            a: 1.0,
        },
        3 => Rgba {
            r: 1.0,
            g: 0.60,
            b: 0.06,
            a: 1.0,
        },
        _ => Rgba {
            r: 1.0,
            g: 0.29,
            b: 0.24,
            a: 1.0,
        },
    }
}

/// The dark casing drawn under points and labels, so they keep an
/// edge over an outline or the curvature comb. This is `HALO` in the
/// web editor.
pub(crate) fn halo() -> Rgba {
    c(theme().role("halo"))
}

/// The ring around a selected point. This is `pointSelectedOuter` in
/// the web editor, which feeds it from the selection colour.
pub(crate) fn point_selected_ring() -> Rgba {
    c(theme().role("selection"))
}

// ---- anchors ----

/// The anchor mark's pink. Anchors read as their own kind of thing
/// beside on-curve and off-curve points. This is `ANCHOR_MARK_PINK`
/// in the web editor.
pub(crate) fn anchor() -> Rgba {
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
            ("danger", super::status_yellow()),
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
        let mut sink = 0_f32;
        for _ in 0..n {
            sink += super::text().r + super::text_muted().g;
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
