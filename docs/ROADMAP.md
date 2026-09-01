<!-- Copyright 2026 the Runebender Authors -->
<!-- SPDX-License-Identifier: Apache-2.0 OR MIT -->

# Roadmap

What this editor cannot do yet, and what the alternatives can. It
replaces five documents from August: the parity list against the
retired web editor, a feature map against Glyphs 4, a ranked Arabic
and RTL plan, and notes on Fontra and Counterpunch. What those
tracked as done is done; git holds the record.

This is a list of gaps, not a schedule. Nothing here is promised.

## The browser build

The native build does all of this; the wasm one does not.

- Text inputs cannot take focus, except the one the editor forces.
- In-window menu items never activate, the same focus problem.
- Clipboard read is a stub in `gpui_web`, so no text paste.
- Script icons render as tofu: the bundled UI font has no coverage.
- Cross-origin isolation rides a service worker, because GitHub
  Pages cannot send the COOP and COEP headers shared memory needs.
- The workspace-server flow, live reload over SSE and 409 handling
  on save, is not wired up.

## Arabic and RTL

The ranked plan from August is done except one item, which was left
deliberately.

- In-place component editing: edit a base glyph inside the
  composite, in context. It rewires selection, coordinate mapping,
  and undo together, so it needs a supervised session rather than a
  drive-by.

## Against Glyphs 4

The reference commercial editor, and the longest list. Grouped as
its handbook groups them.

**Drawing** Star nodes as a stored attribute; a pencil or freehand
tool; a pixel tool; corner radius stored on the node.

**Shape reuse** Segment and head components; live stroke offset on
open paths with caps and miters; variable stroke width along a path;
brushes.

**Spacing** Contextual kerning.

**Masters and variation** Add or delete a master from inside the
editor; axis particles, meaning generated master and instance grids;
virtual masters carried by a custom parameter.

**Font data** Custom parameter and lib editing; grid spacing and
subdivision settings; batch rename with find and replace.

**Features** Class and prefix management; tokens and feature
variations.

**Export** WOFF and WOFF2; instance export from the instance list;
PostScript and TrueType autohinting.

**Hinting** PostScript hinting with zones and stems; TrueType
hinting, automatic and manual.

**Filters** Simplify, meaning redraw with fewer points; a
transformations panel gathering the ones we have; hatch outline;
interpolate with background; transform metrics.

**Shell** Copy glyphs between open fonts; a settings window, since
we have only the theme menu; autosave and file versioning.

**Preview** An interpolation strip across every instance at once.

## Against the other open editors

Both are browser-based, open source, and further along than this one
on the things they chose to lead with.

[Fontra](https://fontra.xyz), by Black Foundry and Just van Rossum,
is variable-first with a HarfBuzz-driven edit view. What it still
does that we do not: shaping in the edit view itself rather than in
a preview, per-source glyph status colours, and a project server for
multiple windows and collaboration.

[Counterpunch](https://counterpunch.space), by Yanone, is built
complex-scripts-first. What it still does that we do not: live
shaping while editing, in-place component editing, and opening
`.glyphs` and `.glyphspackage` files directly.
