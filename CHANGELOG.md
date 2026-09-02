# Changelog

All notable changes to runebender-gpui. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the
project will use [Semantic Versioning](https://semver.org/) once
releases begin.

## [Unreleased]

No releases yet. `AGENTS.md` has the checklist for the first one.
Until then, `main` is the only line and this section stays open.

### Changed

- The canvas reads in every theme. The glyph gets a mid-tone fill from
  the new `outlineFill` token instead of a 16% wash of the ink, and
  metric lines draw in the quiet `metricsLine` neutral instead of the
  green accent, so the outline is the loudest thing on the canvas.
- Themes: Midnight removed. Dark, Gray (default), Light.
- Local models run through the `font-ml` binary as a separate process.
  The crate no longer links `font-ml` or candle, which takes them out
  of every build, the browser build included. The panel finds the
  binary (`$RUNEBENDER_FONT_ML`, PATH, `~/.cargo/bin`), saves, runs
  `font-ml run bolden --write`, and adopts the proposal layer it
  leaves. "Bolden this glyph" still installs at once, undo to reject.
  New: "Propose Bold master" runs every drawn glyph and leaves the
  result waiting in the panel with Install and Discard. Scoring runs
  `font-ml eval` the same way.
- A long run shows its count in the panel (`bolden: 120/397 (H)`),
  read from font-ml's progress lines, and a Cancel button kills the
  process. font-ml writes its proposal only at the end, so a cancelled
  run leaves nothing behind.
- The panel's task rows come from `font-ml tasks --json`. Each task
  the tool reports as built gets "this glyph" and, when it takes a
  glyph set, "every glyph". No task name lives in the panel; the one
  left in the crate is the Glyph > Bolden With Model… menu item.
- Undo lives in core. `EditorState` no longer holds snapshot stacks;
  `push_undo_snapshot`, `undo`, and `redo` call `Master::record_undo`,
  `undo`, and `redo`, and an edit that changed nothing calls
  `discard_last_undo`. History is per glyph name and survives opening
  another glyph, so it is no longer cleared on open.
