# Parity with Counterpunch

Counterpunch (counterpunch.space) is Yanone's browser editor,
built complex-scripts-first: Arabic, Devanagari, and friends are
the core case, not an extension. Alpha now, 1.0 aimed at
October 2026, GPLv3. It matters here because it is the clearest
statement of what an Arabic-first editor should do — the same
ground this editor wants to win on.

Researched 2026-08-26 from counterpunch.space, the LGM 2026 talk
abstract, the GitHub repository status table, and the TypeDrawers
announcement thread.

Legend: `[x]` we have it, `[~]` partial, `[ ]` missing.

## Counterpunch's pitch, item by item

- [~] Live shaping while editing: HarfBuzz-shaped, "100%
      consistent with the end product". Ours shapes through the
      shared text engine (fea-rs + shaper); the claim to match
      is production-accuracy — worth an audit against harfbuzz
      output for the Arabic test fonts.
- [ ] Composition-first (ccmp) workflow: edit base glyphs and
      combining marks once, derived shapes update immediately;
      precomposed forms exist only at export. We have component
      auto-alignment from anchors (the display half); we do not
      generate ccmp or treat composites as build products.
- [ ] In-place component editing: edit a component's base inside
      the composite that uses it, in context. We jump to the
      base glyph instead (double-click).
- [ ] Code-driven dynamic glyph filters: user-written predicates
      as sidebar filters (their "language packs" ride on this).
      Our filters are fixed categories + GF coverage + search.
- [ ] Matrix Mode: the positional-forms review grid (praised in
      the announcement thread). For Arabic: letters as rows,
      isol/init/medi/fina as columns, whole font reviewable at
      a glance. Nothing like it here yet — and it would suit
      this editor extremely well.
- [ ] Python scripting + assistant. Out of scope here (the
      extension story is Rust + the workspace server), but the
      capability gap is real for script-driven workflows.
- [~] File formats: they open .glyphs/.glyphspackage/.ufo/
      .designspace/.vfj/.sfd, save .babelfont/.glyphs, export
      .ttf. We open .ufo/.designspace/.glyphs/.ttf/.otf, save
      UFO + designspace, export through fontc/GF pipelines —
      even, except .vfj/.sfd import.
- [x] Bidirectional text support — both have it.
- [x] Variable font editing, kerning, sidebearings, font info,
      undo — both have it; our kerning/groups UI is ahead of
      their alpha.
- [ ] Their missing features today (per their own status table):
      OpenType feature code generator (we HAVE generation for
      init/medi/fina/liga — ahead), live collaboration (neither).
