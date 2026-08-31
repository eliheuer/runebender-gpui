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
