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
