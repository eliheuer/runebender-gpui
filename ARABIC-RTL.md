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

1. [done 2026-08-26] Direction control: the LTR/RTL/Auto toggle
   now shows whenever the editor is open, not only on the text
   tool.
2. [done 2026-08-26] Shaping inspector: the editor's Shaping
   section — characters (logical, absorbed parts dimmed) against
   glyph names and advances (visual), cluster cross-highlighting,
   double-click opens the glyph.
3. Feature toggles in preview: turn liga/calt/ss01 on and off
   while looking at shaped text; script/language override.
   NEEDS CORE: the shared engine's shape() takes no feature
   list yet — a runebender-core change, queued for a
   supervised session.
4. [done 2026-08-26] Cursive attachment: Generate writes a curs
   block from entry/exit anchors (RightToLeft IgnoreMarks,
   NULL for missing sides). Drawing the connection line in the
   editor remains.
5. [done 2026-08-26] Positional-forms matrix: the Forms view
   mode — base letters as rows, isol/init/medi/fina thumbnails
   as columns, dash for missing forms.
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
