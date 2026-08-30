// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! The panels either side of the canvas, one file per region.
//!
//! Each function builds one region and reads the workspace rather than
//! holding state of its own, so a panel can be moved or removed
//! without untangling it from the editing model.

use crate::*;

/// A master thumbnail: outline, advance, ascender, descender.
pub(crate) type Thumb = (Arc<BezPath>, f64, f64, f64);

mod editor_info;
mod editor_sidebar;
mod glyph_info;
mod local_ai;
mod preview;
mod tabs;
