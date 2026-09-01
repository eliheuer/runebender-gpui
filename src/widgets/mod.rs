// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Widgets this editor needs, written here rather than pulled in.
//!
//! See `slider.rs` for why: the dependency they replace forces
//! `cargo install --locked` and breaks the browser build.

pub(crate) mod input;
#[cfg(not(target_os = "macos"))]
pub(crate) mod menu_bar;
pub(crate) mod resizable;
pub(crate) mod slider;
