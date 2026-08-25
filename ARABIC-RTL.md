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
3. [done 2026-08-26] Feature toggles: the Shaping section lists
   every features.fea tag as a chip cycling default → off → on,
   reshaping immediately (core grew shape_with_features).
   [done 2026-08-25] Script/language override: locale chips
   (Urdu, Sindhi, Farsi, Kashmiri + Latin locales) drive core's
   set_shaping_locale, making languagesystem-specific rules
   (arab/URD locl) fire in the preview — unit-tested in core.
4. [done 2026-08-26] Cursive attachment: Generate writes a curs
   block from entry/exit anchors (RightToLeft IgnoreMarks,
   NULL for missing sides). Drawing the connection line in the
   editor remains.
5. [done 2026-08-26] Positional-forms matrix: the Forms view
   mode — base letters as rows, isol/init/medi/fina thumbnails
   as columns, dash for missing forms.
6. [done 2026-08-26] ccmp generation: Generate writes ccmp from
   mark compositions (composite-only glyphs, sub base mark by
   composed, longest first). Precomposed-at-export remains a
   choice for later.
7. [done 2026-08-26] Mark cloud: the Background section's
   toggle ghosts every attachable mark on the glyph's anchors,
   live while the anchor drags.
8. [done 2026-08-26] Joining-line QA: Glyph > Check Joining
   measures every form's connecting band (components resolved,
   overlap tongues respected), selects the ones off the common
   band.
9. [done 2026-08-26] RTL kerning audit: storage and application
   were correct (logical-order pairs, swapped lookup per bidi
   run); the manual kern drag was sign-inverted in RTL lines —
   fixed in core (8245365) with a two-direction test.
10. In-place component editing (Counterpunch): edit the base
    inside the composite, in context. Deliberately left for a
    supervised session — it rewires selection, coordinate
    mapping, and undo, too deep to land without QA.

## Status 2026-08-26

Nine of ten landed in one pass; the RTL kerning audit also found
and fixed a real sign-inversion bug in the manual kern drag
(runebender-core 8245365). QA checklist for the next session:
type Arabic in the editor and work the Shaping section (chips,
cluster highlighting, double-click into a glyph, feature
toggles), run Glyph > Check Joining on Virtua, open the Forms
view, toggle the mark cloud on a base letter, and kern an Arabic
pair by dragging in both directions.

Items 1–3 are Fontra's lead; 4, 7, 8 are the Glyphs Arabic
workflow; 5, 6, 10 are Counterpunch's identity. 1–5 are the
current build order.
