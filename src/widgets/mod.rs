//! Widgets this editor needs, written here rather than pulled in.
//!
//! See `slider.rs` for why: the dependency they replace forces
//! `cargo install --locked` and breaks the browser build.

pub mod input;
pub mod menu_bar;
pub mod resizable;
pub mod slider;
