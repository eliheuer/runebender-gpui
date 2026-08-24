# Parity with Glyphs 4

Glyphs 4 (glyphsapp.com, released 2025) is the reference commercial
font editor. This file maps its full feature surface against
runebender-gpui and ranks the gaps. `PARITY.md` tracks parity with
runebender-web; this file tracks the larger target.

Researched 2026-08-24 from the Glyphs 4 announcement, the 4.0
changelog (updates.glyphsapp.com/Glyphs4.0-4000.html), the feature
pages, and the Learn tutorial index.

Legend: `[x]` we have it, `[~]` partial, `[ ]` missing.
Deliberate non-goals are in the last section.

## Drawing and editing

- [x] Bezier pen tool (plus HyperPen, which Glyphs does not have)
- [x] Select tool: nodes, handles, marquee, multi-select
- [x] Knife tool
- [x] Shapes tool (rectangle, ellipse, shift-lock)
- [x] Measure tool with option toggles (stems, side bearings,
      handle lengths, colorized outline)
- [x] Remove overlap, union, subtract, intersect, exclude
- [x] Flip, rotate 90/180, duplicate, duplicate-repeat
- [x] Round corners
- [x] Harmonize, balance, optimize (Glyphs calls harmonize
      "green harmony" / star nodes)
- [x] Reverse contours, set start point, tidy equivalents
- [x] Nudge with grid steps, numeric move/scale with a
      reference point
- [x] Curvature comb and continuity display
- [~] On-canvas transform: we have flips and fixed rotations, but
      not free rotate or scale from the bounding box
      (Glyphs 4 added Illustrator-style corner rotation)
- [ ] Star nodes as a stored node attribute (we harmonize on
      demand instead; no G2-locked node type)
- [ ] Pencil / freehand drawing (web has the sketch tool; parked)
- [ ] Pixel tool for pixel fonts
- [ ] Fit Curve panel (set both handles to a percentage)
- [ ] Node corner radius stored on the node

## Shape reuse

- [x] Components: place, drag, decompose one or all, open base on
      double click, lock
- [x] Component auto-alignment from anchors, live
- [ ] Smart components (per-component axes, "glyph axes" in 4)
- [ ] Corner components
- [ ] Segment components
- [ ] Head components
- [ ] Strokes: live offset on open paths, caps, miters
- [ ] Pen points: variable stroke width along a path (new in 4)
- [ ] Brushes

## Glyph and layer model

- [x] Anchors: add, drag, rename, delete
- [x] Mark attachment preview through composites
- [x] Background layer per glyph (UFO public.background)
- [x] Reference glyph underlay
- [x] Mark colors, tags on glyphs
- [x] Rename glyph with reference fixup
- [x] Unicode assignment editing
- [~] Free per-glyph layers: backup to a named layer, draw any
      layer as an underlay, swap with the drawing, delete (Masters
      section). Editing directly on a non-default layer comes later
- [ ] Intermediate ("brace") layers per glyph
- [ ] Alternate ("bracket") layers for shape switching
- [~] Guides: global guides land (draw, drag with snap, add and
      delete from the context menu; fontinfo guidelines). Local
      per-glyph guides and locking come later
- [ ] Images placed in a layer as a tracing template
- [ ] Glyph notes

## Spacing and kerning

- [x] Sidebearing drag at the glyph edges
- [x] Metrics fields (native; browser blocked by the focus bug)
- [x] Kerning by drag in text mode, per master
- [x] Kerning group editing on the glyph
- [x] Kerning panel: the grid's Kerning section lists the active
      master's pairs with filter, edit, and delete
- [ ] Visual kerning groups: drag glyphs onto group shelves
      (new in 4)
- [ ] Batch sidebearing editing across many glyphs
- [ ] Metrics keys (side bearings linked by formula, "=n+10")
- [ ] Contextual kerning
- [x] Auto "kern" feature generation at export (fontc writes
      kern, mark, and mkmk from UFO kerning, groups, and anchors)

## Masters, interpolation, variable fonts

- [x] Designspace masters: switch, per-master editing
- [x] Axis sliders with live interpolation preview
- [x] Interpolation ghost in the edit view
- [x] Master thumbnails in the sidebar
- [~] Master compatibility: grid dots, an Incompatible filter,
      and the Glyph panel names the first disagreeing master pair
      with contour and point counts. A point-level visual diff
      comes later
- [ ] Add or delete a master from inside the editor
- [~] Instance list: designspace instances show under the axis
      sliders; clicking one parks the preview (and the strip) on
      it. Defining and reordering instances comes later
- [ ] Axis mappings (avar) editing
- [ ] Reinterpolate a layer from the other masters
- [ ] Axis particles: generate master and instance grids from
      named stops (new in 4)

## Font-level data

- [~] Font Info editing: the grid's Font Info section edits the
      family name, style name, UPM, italic angle, ascender,
      descender, x-height, and cap height per master. Versioning,
      copyright, designer, and license fields come later
- [ ] Custom parameters / lib editing
- [ ] Grid spacing and subdivision settings
- [x] New font from template
- [x] Add and remove glyphs, missing-glyph generate from
      language coverage
- [x] Glyph info database: categories, scripts, GF language
      coverage (core `category` and `sidebar` modules)
- [ ] Batch rename with find and replace
- [ ] List view: glyph metadata as an editable table

## OpenType features

- [x] Shaping preview in text mode (the shared text engine)
- [~] Feature code editing: the Features section edits
      features.fea with Apply/Revert and a real compile check
      (fea-rs), live into the shaping preview. The prefix/class/
      feature rail and syntax colors come later
- [~] Automatic feature generation: kern, mark, and mkmk come
      from fontc at export; liga and ccmp writers come later
- [ ] Feature preview toggles in the editor
- [ ] Classes and prefixes management
- [ ] Tokens / feature variations

## Export

- [~] TTF export (File > Export, Cmd+E). With a repo build
      script (build-fontc.sh / build.sh) the export runs the
      repo's own Google Fonts pipeline: gftools fixes, STAT, and
      per-instance statics included. Bare sources compile through
      fontc plus a gftools-fix-font pass when the tool is found.
      CFF outlines come later
- [ ] WOFF and WOFF2
- [ ] Instance export from the instance list
- [ ] PostScript autohinting at export
- [ ] TrueType autohinting at export
- [ ] Image export (PNG, SVG, PDF per glyph)
- [x] UFO and designspace save is the native format, so "export
      to UFO" is free

## Testing and preview

- [x] Preview mode (filled, in-editor)
- [x] Text mode: type real words around the glyph, bidi, shaping
- [x] Multiple edit tabs
- [~] Preview strip: the editor's bottom strip previews the text
      line filled, with blur and invert, and follows the
      interpolation location (so instance rows switch it).
      A separate long-text window with tracking, line height, and
      feature toggles comes later
- [ ] Interpolation preview strip across all instances at once

## Hinting

- [ ] PostScript hinting: automatic, plus zones and stems in
      font info
- [ ] TrueType hinting: automatic and manual
- (Manual TT hinting is a non-goal; see below)

## Color fonts

- [ ] COLRv0 / CPAL layered color
- [ ] COLRv1 with gradients
- (sbix and SVG-in-OT are non-goals; see below)

## Application shell

- [x] Native menus and shortcuts
- [x] Dark and light themes, theme menu
- [x] Live file-watch reload
- [x] Open UFO, designspace, .glyphs
- [x] Search with scopes, regex, case toggle
- [x] Grid multi-select with batch mark color
- [~] Font view detail: we show name and unicode; Glyphs 4 has a
      detail mode with category, script, and custom columns
- [ ] Character / glyph info window (Unicode data lookup)
- [ ] Settings window (we have the theme menu only)
- [ ] Autosave and file versioning

## Filters

- [x] Round corners
- [x] Remove overlap
- [ ] Offset curve (bolder or lighter, open-path stroke expand)
- [~] Slanter: the Transformations section shears the selection
      by typed degrees. Contrast and weight corrections come later
- [ ] Simplify: redraw with fewer points (new in 4; our image
      trace covers part of this)
- [ ] Transformations panel as one place for all of these

## Non-goals

These stay out of scope. Do not add them to the gap list.

- Python scripting, plugin manager, macro window. The extension
  story here is Rust and the workspace server, not embedded
  Python.
- Manual TrueType hinting UI. Autohinting at export is enough.
- sbix and SVG-in-OT color tables. COLR covers new work.
- SF Symbols and .glyphsicons export.
- App Store style licensing UI, crash reporter, update checker.
- Glyphs file format *saving*. We read .glyphs; UFO and
  designspace stay the native format.

## The plan

Ordered by leverage. Each step is small enough to land on its own.

1. **Export through fontc.** Done 2026-08-24: File > Export
   (Cmd+E) saves, runs fontc in the background, and reports
   through the status note. Later: bundle fontc as a library
   instead of a binary on PATH.
2. **Font Info editing.** First slice done 2026-08-24: names and
   vertical metrics in the grid's Font Info section. Remaining:
   versioning, copyright, designer, license, custom parameters.
3. **Guides.** Global guides done 2026-08-24 (draw, drag, add,
   delete). Remaining: local per-glyph guides, locking, angled
   guide editing.
4. **Kerning panel.** Done 2026-08-24 in the grid's Kerning
   section. Remaining: visual kerning groups, batch operations.
5. **Free per-glyph layers.** Norad already models UFO layers.
   A layers panel unlocks brace layers later.
6. **Instances.** Read and edit designspace instances, preview
   them with the existing interpolation engine, export them
   through step 1.
7. **Compatibility report.** Point-count and order checks across
   masters, shown in the grid and the editor.
8. **Offset curve and Slanter filters.** Kurbo has the offset
   machinery.
9. **Feature editing.** A .fea editor pane with fea-rs for
   parsing and checks. Auto kern and mark generation can lean
   on fontc's feature writers.
10. **Text Preview window.** The text engine already shapes;
    this is mostly UI.

Corner and smart components, strokes, pen points, color fonts,
and hinting come after these. They are deep model changes and
none of them blocks daily type design work.

## UI reference notes (Glyphs 4 screenshots, 2026-08-24)

Layout facts from Glyphs 4 on the Virtua Grotesk designspace, for
building the matching pieces here.

- Layers palette: masters in bold; backup layers indented under
  their master and named by date ("Aug 24, 26 at 13:09"); + and −
  below the list.
- Kerning window: three columns (Left, Value, Right), group names
  shown as @name in a second color, search on top, pair count at
  the foot.
- Features tab: left rail lists Prefix, Classes, and Features;
  auto-generated entries carry a regenerate badge. Right side is a
  syntax-colored code pane with line numbers. Top: Active and
  Generate automatically checkboxes. Bottom: Update and Compile.
- Font Info is one window with Font / Masters / Exports / Features
  / Document / Notes tabs. Masters tab: Active toggle, name, icon,
  axis coordinates (internal and external), metrics as value +
  zone pairs, stems, and custom parameters carrying the vertical
  metrics (typo/hhea ascender 1024, descender -296, line gap 0;
  winAscent 1112, winDescent 470; UFO Filename).
- Exports tab per instance: Active, style name, weight class,
  width class, style linking, axis coordinates.
- Guides draw blue with a round drag handle; selecting one puts
  name, X, Y, angle, and a lock in the info box.
- Slanter filter: Geometric Slant, Cursivy, stems (Auto Stems),
  Slant and Rotate. Offset Curve filter: Horizontal, Vertical
  (linkable), Make Stroke, Position %, Keep Compatible, cap style.
