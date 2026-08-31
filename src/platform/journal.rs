// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! A record of what the session changed.
//!
//! One JSON object per line, appended as edits happen. It answers the
//! question a proof, a review, or an agent actually asks: what did this
//! session touch, and what did it do to it.
//!
//! It is deliberately not a replay format. Recording a drag as a
//! command you could retype means inventing a command language for
//! every gesture. A log that claims to replay but cannot is worse
//! than one that never claimed to. What this gives you is an honest
//! account: the glyph, the operation, and the shape of the change.
//!
//! The journal is off unless `RUNEBENDER_JOURNAL` or the config file
//! names a path, so no session writes anywhere the user did not ask
//! for.

use std::io::Write;
use std::path::PathBuf;

/// What one edit did.
pub struct Entry<'a> {
    /// The operation's name, as the user would say it: "remove overlap".
    pub op: &'a str,
    /// The glyph it applied to, when it applied to one.
    pub glyph: Option<&'a str>,
    /// Counts worth keeping: points moved, contours removed.
    pub detail: Option<String>,
}

/// Where the log is written, if anywhere.
///
/// `$RUNEBENDER_JOURNAL` wins over the config file. If neither is
/// set, there is no journal: a tool that logs where it was not asked
/// to is a tool people turn off entirely. Returns `None` in that
/// case.
pub fn path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("RUNEBENDER_JOURNAL") {
        return Some(PathBuf::from(p));
    }
    crate::CONFIG.get().and_then(|c| c.journal.clone())
}

/// Escape the few characters that would break a JSON string.
///
/// Glyph names come from a font file and are not guaranteed to be
/// tame, so this cannot assume they are.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Format one entry as its JSON line.
///
/// Separate from the writing so it can be tested without touching a
/// filesystem.
pub fn line(entry: &Entry) -> String {
    let mut s = format!("{{\"op\":\"{}\"", escape(entry.op));
    if let Some(g) = entry.glyph {
        s.push_str(&format!(",\"glyph\":\"{}\"", escape(g)));
    }
    if let Some(d) = &entry.detail {
        s.push_str(&format!(",\"detail\":\"{}\"", escape(d)));
    }
    s.push('}');
    s
}

/// Append one entry, if a journal is configured.
///
/// Failures are silent on purpose: a log that cannot be written is not
/// a reason to interrupt someone's drawing.
pub fn record(entry: Entry) {
    let Some(path) = path() else { return };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{}", line(&entry));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_one_json_object() {
        let e = Entry {
            op: "remove overlap",
            glyph: Some("eight"),
            detail: Some("3 contours to 1".into()),
        };
        assert_eq!(
            line(&e),
            r#"{"op":"remove overlap","glyph":"eight","detail":"3 contours to 1"}"#
        );
    }

    #[test]
    fn the_optional_parts_are_left_out() {
        let e = Entry {
            op: "save",
            glyph: None,
            detail: None,
        };
        assert_eq!(line(&e), r#"{"op":"save"}"#);
    }

    /// A glyph name is font data, not ours, so it cannot be trusted to
    /// be free of the characters that would break the line.
    #[test]
    fn a_hostile_glyph_name_cannot_break_the_line() {
        let e = Entry {
            op: "rename",
            glyph: Some("a\"b\\c\nd"),
            detail: None,
        };
        let out = line(&e);
        assert_eq!(out, r#"{"op":"rename","glyph":"a\"b\\c\nd"}"#);
        assert_eq!(out.lines().count(), 1, "one entry is one line");
    }

    #[test]
    fn nothing_is_written_without_the_variable() {
        // The default has to be off: a tool that logs where it was not
        // asked to is a tool people turn off entirely.
        let before = std::env::var_os("RUNEBENDER_JOURNAL");
        if before.is_none() {
            assert!(path().is_none());
        }
    }
}
