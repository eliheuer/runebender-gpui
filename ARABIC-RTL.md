# Arabic & RTL: the focus document

A core selling point of this editor is being better for Arabic
and RTL scripts. This file ranks that work. It synthesizes the
Glyphs Arabic workflow (glyphsapp.com/learn/arabic), Fontra's
shaped edit view (see FONTRA.md), and Counterpunch's
complex-scripts-first design (see COUNTERPUNCH.md), against what
this editor has today. Virtua Grotesk's Arabic set (862 glyphs,
init/medi/fina, mark anchors) is the working test font.

## What we already have

- The shared text engine shapes with compiled features (fea-rs)
  and handles bidi; the editor edits inside the shaped buffer.
- Automatic init/medi/fina and liga generation from glyph names
  (the Features section's Generate button).
- Mark anchors with live composite alignment (top/_top pairs),
  anchor editing, mark color workflow.
- Arabic glyph categories, the Arabic sidebar section, GF Arabic
  Core coverage counts, missing-glyph generation.
- Kerning (groups, panel, text-mode drags) — direction handling
  unaudited for RTL pairs.
- Metrics keys, components, guides, zones: script-neutral tools
  that Arabic work leans on.

## The ranked gaps

1. Direction control in the edit view: an RTL/LTR/auto toggle
   (Glyphs' bottom-right button). The engine already stores
   direction; there is no UI.
2. Shaping inspector: characters in logical order against output
   glyphs in visual order — names, advances, cluster links
   (Fontra's panel). The single most useful Arabic debugging
   surface.
3. Feature toggles in preview: turn liga/calt/ss01 on and off
   while looking at shaped text; script/language override.
4. Cursive attachment: entry/exit anchors driving a generated
   curs feature, with the connection drawn in the editor
   (Glyphs' cascade workflow).
5. Positional-forms matrix view: letters as rows, isol/init/
   medi/fina as columns (Counterpunch's Matrix Mode). The
   Arabic review surface.
6. ccmp generation: compose marks once, generate ccmp (and
   optionally precomposed forms at export) — Counterpunch's
   composition-first core.
7. Mark cloud preview: every attachable mark ghosted on the
   base while editing anchors (Glyphs Cmd-U view).
8. Joining-line QA: check that connecting strokes meet at the
   same height across init/medi/fina (the Glyphs tutorial's
   segment-component overlap concern; Virtua's DESIGN.md has
   the same rule).
9. RTL kerning audit: confirm pair direction, group sides, and
   drag gestures are correct in RTL runs; fix what is not.
10. In-place component editing (Counterpunch): edit the base
    inside the composite, in context.

Items 1–3 are Fontra's lead; 4, 7, 8 are the Glyphs Arabic
workflow; 5, 6, 10 are Counterpunch's identity. 1–5 are the
current build order.
