# Changelog

All notable changes to runebender-gpui. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the
project will use [Semantic Versioning](https://semver.org/) once
releases begin.

## [Unreleased]

No releases yet. `AGENTS.md` has the checklist for the first one.
Until then, `main` is the only line and this section stays open.

### Changed

- Undo lives in core. `EditorState` no longer holds snapshot stacks;
  `push_undo_snapshot`, `undo`, and `redo` call `Master::record_undo`,
  `undo`, and `redo`, and an edit that changed nothing calls
  `discard_last_undo`. History is per glyph name and survives opening
  another glyph, so it is no longer cleared on open.
