// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The world outside the window: the config file, the operation
//! journal, files and watching, and the browser host.

pub(crate) mod config;
pub(crate) mod host;
pub(crate) mod journal;
#[cfg(target_family = "wasm")]
pub(crate) mod web_host;
