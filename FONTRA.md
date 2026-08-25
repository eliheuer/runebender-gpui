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
- [ ] Shaping inspector: an "input characters and output glyphs"
      panel — characters in logical order, glyphs in visual
      order with names, advances, offsets, and cluster indices,
      cross-highlighting between the two lists.
- [ ] Feature toggles in the preview: per-feature on/off (GSUB
      listed from the font, kern/mark/mkmk/curs emulated), plus
      script, language, and direction overrides.
- [ ] Live emulated positioning: kerning, mark, mkmk, and curs
      shaped straight from source data (anchors, kerning.plist)
      without compiling, so positioning edits show instantly.
      We recompile the shaper font from features.fea instead;
      anchors do not feed GPOS emulation yet.
- [ ] Insertion markers in feature code ("# Automatic Code")
      controlling where generated code lands among hand-written
      blocks. Our Generate replaces whole blocks by tag.
- [ ] Cross-axis mapping editor (avar2).
- [ ] Status definitions: per-source glyph status colors with
      font-wide definitions.
- [ ] Related Glyphs & Characters panel.
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
