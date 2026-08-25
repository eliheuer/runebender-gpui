# Parity with Fontra

Fontra (fontra.xyz) is the open-source, browser-based,
variable-first editor by Black Foundry and Just van Rossum, with
Google backing. It matters here for two reasons: it is the other
serious open-source editor, and its HarfBuzz-driven edit view is
the current reference for editing inside shaped Arabic text.

Researched 2026-08-26 from docs.fontra.xyz, the Fontra blog's
text-shaping post, and the GitHub repository. `GLYPHS4.md` is the
big checklist; this file tracks only where Fontra leads or
differs.

Legend: `[x]` we have it, `[~]` partial, `[ ]` missing.

## Where Fontra leads

- [~] Shaping in the edit view. Fontra shapes the editing text
      with HarfBuzz and lets you edit glyphs inside the shaped
      run. Our shared text engine also shapes (fea-rs compiled
      features, bidi), and the editor edits inside the buffer —
      but Fontra adds the panel work below.
- [x] Shaping inspector (2026-08-26): the editor's Shaping
      section — characters logical, glyphs visual, advances,
      cluster cross-highlighting, double-click to edit.
- [x] Feature toggles (2026-08-26): every features.fea tag as
      a chip, default → off → on, reshaping live. Script and
      language overrides remain.
- [~] Positioning in the preview (2026-08-26): Generate now
      writes mark/mkmk from anchors and curs from entry/exit, so
      one Generate + Apply positions marks in the shaped preview.
      Fontra skips the Apply step (true live emulation); ours is
      one click behind.
- [ ] Insertion markers in feature code ("# Automatic Code")
      controlling where generated code lands among hand-written
      blocks. Our Generate replaces whole blocks by tag.
- [~] Axis mappings: avar pairs editable in the Axes section;
      avar2 cross-axis mapping remains.
- [ ] Status definitions: per-source glyph status colors with
      font-wide definitions.
- [x] Related Glyphs (2026-08-26): components, suffix
      siblings, and used-by chips in the editor panel.
- [ ] Multi-window / project server (fontra-pak, collaboration).

## Where we lead or match

- [x] Native app performance (gpui); Fontra is browser-only.
- [x] Compiled-binary opening (TTF/OTF import).
- [x] Kerning groups, group kerning, kerning panel.
- [x] HOI (trajectories, intermediate points, velocity) — Fontra
      has nothing comparable.
- [x] Guides (global + local), zones, stems, annotations, masks,
      corner components, filters (extrude, roughen, offset,
      stroke), COLR authoring.
- [x] Background images; Fontra documents them too — parity.
- [~] Variable-first: both edit designspaces; Fontra's sources
      panel and cross-axis mapping are ahead; our brace layers,
      rules, and instance editing are ahead.
