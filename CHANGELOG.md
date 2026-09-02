# Changelog

All notable changes to runebender-gpui. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the
project will use [Semantic Versioning](https://semver.org/) once
releases begin.

## [Unreleased]

No releases yet. `AGENTS.md` has the checklist for the first one.
Until then, `main` is the only line and this section stays open.

### Changed

- The inspector's controls come from one place, `view/controls.rs`:
  buttons, toggles, row labels, and fields share one height, one type
  size, and fill their row, so nothing clips at the panel edge.
  Commands read as verbs (Add extremes, Round corners), units live in
  the label (Slant °, Fit curve %), and the Local AI panel uses the
  same controls.
- A bare `runebender-gpui` opens with no font and File → Open, instead
  of a Virtua Grotesk checkout beside the repository. Nothing on one
  machine is a default for everyone else's.
- A selected glyph-grid cell follows the theme's `cellSelectedFill`
  and `cellSelectedInk` tokens: Gray and Light invert to the ink, Dark
  keeps its lift. The HUD card headers no longer borrow the selected
  fill.
- Selection in the chrome is shown by inversion, ink on the panel
  fill, not by the green accent: active tools, toggles, tabs, sidebar
  rows, menu items, and the focused field. The accent stays for
  meaning only. Two theme accessors carry it, `selected_bg` and
  `selected_ink`.
- The glyph grid's side padding equals the gap between cells, so the
  margin is the same on all four sides and the cells take the width.
- Text fields sit in the panel colour with a quiet `fieldOutline`
  rule, instead of a darker box with the panel's outline.
- Glyph-grid cells fill with the mark colour in every theme, not only
  Gray, from the shared token file.
- Words: sentence case in Font info, "Saved" in the muted text colour
  with only "Not saved" in the warning colour, and the LTR/RTL/Auto
  buttons on the tab height so the header is one row of controls.
- `RB_SIDEBAR_TAB=<0..3>` starts the left sidebar on that tab, the way
  `RB_OPEN_GLYPH` starts in the editor, so a capture reaches the Local
  AI panel without clicks.
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
