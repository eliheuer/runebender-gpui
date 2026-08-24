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
- [ ] Free per-glyph layers (any number, named, with a layers
      panel; UFO supports this, we only touch two layers)
- [ ] Intermediate ("brace") layers per glyph
- [ ] Alternate ("bracket") layers for shape switching
- [ ] Guides: local per glyph and global per master, editable
      and lockable (we only draw metric lines from fontinfo)
- [ ] Images placed in a layer as a tracing template
- [ ] Glyph notes

## Spacing and kerning

- [x] Sidebearing drag at the glyph edges
- [x] Metrics fields (native; browser blocked by the focus bug)
- [x] Kerning by drag in text mode, per master
- [x] Kerning group editing on the glyph
- [ ] Kerning panel: list all pairs, search, edit, delete,
      per-master view
- [ ] Visual kerning groups: drag glyphs onto group shelves
      (new in 4)
- [ ] Batch sidebearing editing across many glyphs
- [ ] Metrics keys (side bearings linked by formula, "=n+10")
- [ ] Contextual kerning
- [ ] Auto "kern" feature generation at export

## Masters, interpolation, variable fonts

- [x] Designspace masters: switch, per-master editing
- [x] Axis sliders with live interpolation preview
- [x] Interpolation ghost in the edit view
- [x] Master thumbnails in the sidebar
- [ ] Master compatibility report (mismatched points, order,
      start points) with visual diff
- [ ] Add or delete a master from inside the editor
- [ ] Instance list: define, name, preview, reorder
- [ ] Axis mappings (avar) editing
- [ ] Reinterpolate a layer from the other masters
- [ ] Axis particles: generate master and instance grids from
      named stops (new in 4)

## Font-level data

- [~] Metric lines from fontinfo are drawn; nothing is editable
- [ ] Font Info editing: family name, style names, versioning,
      copyright, designer, license
- [ ] Vertical metrics editing (ascender, descender, x-height,
      cap height, per master)
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
- [ ] Feature code editing (.fea) with syntax check
- [ ] Automatic feature generation (kern, mark, mkmk, liga, ccmp)
- [ ] Feature preview toggles in the editor
- [ ] Classes and prefixes management
- [ ] Tokens / feature variations

## Export

- [ ] Static OTF and TTF export
- [ ] Variable font export (TTF and CFF2)
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
- [ ] Text Preview window: long texts, tracking, line height,
      alignment, feature toggles, instance switching
- [ ] Interpolation preview strip across all instances

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
- [ ] Slanter: oblique with contrast and weight correction
      (new in 4)
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

1. **Export through fontc.** One menu item closes the largest
   gap: File > Export runs fontc on the open source and reports
   the result. Static and variable TTF land at once because
   fontc does both. This makes the editor produce fonts, which
   is the whole point of Glyphs. Later: bundle fontc as a
   library instead of a binary on PATH.
2. **Font Info editing.** A font info panel that writes
   fontinfo.plist: names, metrics, per-master values. This is
   also flagged in PARITY.md as a daily-driver need.
3. **Guides.** Editable global guides per master (UFO
   fontinfo guidelines) and local guides per glyph. Drawing
   support half exists.
4. **Kerning panel.** List pairs per master, search, edit,
   delete. The data model already round-trips kerning.plist.
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
