// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! The config file, read once at startup.
//!
//! `$XDG_CONFIG_HOME/runebender/config.toml`, or
//! `~/.config/runebender/config.toml`. Everything in it is optional,
//! and a file that is missing, unreadable or malformed is the same as
//! no file: a broken config should not stop you opening a font.
//!
//! Precedence is environment, then file, then the built-in default.
//! A variable set for one run has to win over a setting meant for
//! every run, or `RUNEBENDER_MODELS=... runebender-gpui` does nothing.
//!
//! ## Following a system theme
//!
//! Omarchy keeps the active theme at
//! `~/.config/omarchy/current/theme/`, a symlink its theme switcher
//! repoints, and lets you template unsupported applications from
//! `~/.config/omarchy/themed/`. That is the seam: a template there
//! writes this file, and Runebender needs to know nothing about
//! Omarchy. The same trick works for any system that can write a
//! four-line TOML file.

use std::path::PathBuf;

#[cfg(not(target_family = "wasm"))]
use crate::view::theme;
use serde::Deserialize;

/// What the file can say. Every field is optional.
#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Theme id, as View → Theme names them: dark, midnight, gray, light.
    pub theme: Option<String>,
    /// Where to look for local models.
    pub models: Option<PathBuf>,
    /// Where to append the session journal. Unset means no journal.
    pub journal: Option<PathBuf>,
}

/// Where the file lives, whether or not it exists.
pub fn path() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        let xdg = PathBuf::from(xdg);
        if !xdg.as_os_str().is_empty() {
            return Some(xdg.join("runebender/config.toml"));
        }
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/runebender/config.toml"))
}

/// Parse a config from text, keeping what is valid.
///
/// Separate from reading the file so the rules can be tested without
/// a filesystem, and so a caller can report on a string it already has.
pub fn parse(text: &str) -> Result<Config, toml::de::Error> {
    toml::from_str(text)
}

/// The config for this run. Never fails: a bad file yields defaults.
pub fn load() -> Config {
    let Some(path) = path() else {
        return Config::default();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Config::default();
    };
    parse(&text).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_theme_can_be_set() {
        let c = parse("theme = \"gray\"\n").expect("valid");
        assert_eq!(c.theme.as_deref(), Some("gray"));
        assert_eq!(c.models, None);
    }

    #[test]
    fn an_empty_file_is_valid() {
        assert_eq!(parse("").expect("valid"), Config::default());
    }

    /// A typo should be reported rather than silently ignored, or a
    /// setting that never took effect looks like a bug in the editor.
    #[test]
    fn an_unknown_key_is_an_error() {
        assert!(parse("thmee = \"gray\"\n").is_err());
    }

    #[test]
    fn a_wrong_type_is_an_error() {
        assert!(parse("theme = 3\n").is_err());
    }

    /// The whole file is optional, so a broken one must not be fatal:
    /// nobody should be locked out of their font by a stray comma.
    #[test]
    fn a_broken_file_falls_back_to_defaults() {
        let c: Config = parse("theme = [").unwrap_or_default();
        assert_eq!(c, Config::default());
    }

    #[test]
    fn paths_are_paths() {
        let c = parse("models = \"/srv/models\"\njournal = \"/tmp/j.jsonl\"\n").expect("valid");
        assert_eq!(c.models, Some(PathBuf::from("/srv/models")));
        assert_eq!(c.journal, Some(PathBuf::from("/tmp/j.jsonl")));
    }

    /// Every theme name the config may set has to be one the editor
    /// can switch to, or a config file silently does nothing.
    #[test]
    fn the_documented_theme_names_all_resolve() {
        for (id, _) in crate::view::theme::THEMES {
            let c = parse(&format!("theme = \"{id}\"\n")).expect("valid");
            assert_eq!(c.theme.as_deref(), Some(id));
            assert!(
                crate::view::theme::set_theme(id),
                "config could name {id} and it would fail"
            );
        }
        crate::view::theme::set_theme("dark");
    }

    /// What an Omarchy template would produce, checked as a whole so
    /// the documented example cannot drift from what parses.
    #[test]
    fn the_documented_example_parses() {
        let c = parse(
            "# written by ~/.config/omarchy/themed/runebender.toml.tpl\n\
             theme = \"gray\"\n\
             models = \"/home/eli/.runebender/models\"\n",
        )
        .expect("valid");
        assert_eq!(c.theme.as_deref(), Some("gray"));
        assert_eq!(
            c.models,
            Some(PathBuf::from("/home/eli/.runebender/models"))
        );
    }
}
