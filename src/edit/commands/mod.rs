// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the menus and shortcuts call.
//!
//! Every method here is the whole of one user-facing command: the menu
//! item, the keyboard shortcut and the context menu all land on the
//! same function. They are the layer between an intent ("remove the
//! overlap") and the operation in runebender-core that performs it.

mod annotations;
mod color;
mod features;
mod file;
mod glyph;
mod images;
mod layers;
mod masters;
mod models;
mod paths;

/// A design-space value as the `f32` a designspace document stores.
///
/// Axis positions and instance locations are `f32` in the file
/// format, so a value on its way there narrows here rather than at
/// each field.
pub(crate) fn ds_f32(value: f64) -> f32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the designspace format stores axis values as f32"
    )]
    {
        value as f32
    }
}

/// The index `step` places from `index`, wrapping in a list of
/// `count` items.
///
/// Stepping through samples, masters, or themes all want this, and
/// all of them wrap in both directions.
pub(crate) fn rotate(index: usize, count: usize, step: isize) -> usize {
    if count == 0 {
        return 0;
    }
    let offset = step.unsigned_abs() % count;
    if step >= 0 {
        (index + offset) % count
    } else {
        (index + count - offset) % count
    }
}
