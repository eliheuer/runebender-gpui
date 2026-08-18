// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! Color constants. Warm dark palette, matching the direction of the
//! other Runebender editors. gpui's `rgb()` is not const, so these
//! are functions.

use gpui::{rgb, Rgba};

pub fn window_bg() -> Rgba {
    rgb(0x28211c)
}
pub fn panel_bg() -> Rgba {
    rgb(0x211b17)
}
pub fn cell_bg() -> Rgba {
    rgb(0x2f2822)
}
pub fn cell_selected_bg() -> Rgba {
    rgb(0x3a2f26)
}
pub fn cell_border() -> Rgba {
    rgb(0x3a322b)
}
pub fn accent() -> Rgba {
    rgb(0xd8913c)
}
pub fn glyph_fill() -> Rgba {
    rgb(0xe8ddcf)
}
pub fn text() -> Rgba {
    rgb(0xe8ddcf)
}
pub fn text_muted() -> Rgba {
    rgb(0xa89a86)
}
pub fn metrics_line() -> Rgba {
    rgb(0x4a4038)
}
pub fn editor_fill() -> Rgba {
    Rgba {
        r: 0.91,
        g: 0.87,
        b: 0.81,
        a: 0.08,
    }
}
pub fn marquee_fill() -> Rgba {
    Rgba {
        r: 0.85,
        g: 0.57,
        b: 0.24,
        a: 0.10,
    }
}
pub fn anchor() -> Rgba {
    rgb(0xc75f5f)
}
