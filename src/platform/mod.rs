// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The world outside the window: the config file, the operation
//! journal, files, watching for changes, and the browser host.

#[cfg(not(target_family = "wasm"))]
pub(crate) mod config;
pub(crate) mod host;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod journal;
pub(crate) mod watch;
#[cfg(target_family = "wasm")]
pub(crate) mod web_host;

#[cfg(unix)]
pub(crate) mod live;
