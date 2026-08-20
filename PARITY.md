# Parity with runebender-web

The goal: runebender-gpui replaces runebender-web as the daily
driver, and long-term as the editor on runebender.org. This file
tracks what remains. Check items off as they land; add items as
testing finds them. The web editor's code is the spec
(`../runebender-web/core/src/` and `src/Runebender.vue`).

Audited 2026-08-20 against web's full wasm API surface and
component list.

## Done (the short version)

All eight edit modes (Select, Pen, HyperPen, Knife, Measure,
Shapes, Preview, Text) with faithful web behaviors; the shared text
engine with shaping, bidi, kerning drags, and caret editing;
sidebearing edge drags; components (select, drag, delete,
double-click opens base, decompose); booleans; set start point;
undo/redo; copy/paste contours + text paste routing; nudge with web
grid steps; flips/rotations; harmonize/balance/optimize; anchors
(add, select, drag, delete); designspace masters, axes sliders,
interpolation ghost; design grid + zoomed-in neighbor treatment;
marks/colors; the full sidebar (categories, languages with GF
coverage, filters, search, sort); Glyphs-style bottom bar, tab
strip, right-panel glyph preview; add/remove glyph; live
file-watch reload; .glyphs import; native menus + shortcuts;
browser build over WebGPU with workspace-server load/save.

## Editing gaps

- [x] Round selected corners (menu, context menu, Round button;
      fillet size/handle ratio inferred from existing corners)
- [x] Duplicate selection / duplicate-repeat (Cmd+D / Cmd+Shift+T,
      Transformations tiles, Glyph menu; contours, components, and
      anchors; repeat re-applies the last flip/rotate)
- [x] Rotate selection 180° (Glyph menu)
- [x] Shift-lock constraints: shapes square/circular, knife and
      measure lines axis-locked (web keeps point drags free — shift
      is the selection modifier there)
- [x] Convert hyperbezier contour to cubic (Glyph menu; selected
      contours, or all hyper contours when nothing is selected)
- [x] Numeric move/scale with reference point: the Selection panel
      grew the 9-point quadrant picker with X/Y (move so the
      reference lands there) and W/H (scale about the reference),
      working on points, the selected component, or anchor.
      Committing is native-only until the browser input bug is
      fixed.
- [x] Right-click context menus: lock/unlock component, decompose
      one/all, add component, set start point, reverse contour,
      round corners, move contour up/down, add/delete anchor
- [x] Component auto-alignment for composites — accents follow
      their base anchors live; anchor-locked components refuse
      drags with a Lock/Free toggle in the Selection panel
      (core `composites` module)
- [x] Add component to a glyph (context menu, name typed inline;
      lands anchor-locked so marks snap to their anchor)
- [x] Anchor editing: name field in the Selection panel, coords via
      the X/Y reference fields (anchor selected → bounds is its
      point)
- [ ] Measure-tool option toggles (web SelectPanel/measure options:
      colorize outline, handle lengths, segment lengths, stems &
      counters, side bearings, popcount sums)
- [x] Curvature comb + continuity display (Curves section toggles;
      shared analyses from core's curve module, web's color ramp)
- [ ] Sketch tool (parked deliberately; SketchPanel.vue)

## Glyph and font data gaps

- [x] Edit kerning groups (Glyph panel inputs → groups.plist, both
      sides, empty clears; every master)
- [x] Edit unicode assignment (Glyph panel input, 0041/U+0041/0x41)
- [x] Rename glyph (Glyph panel name field; updates components in
      other glyphs, group memberships, kerning keys, the open text
      session, and re-points selection). Native-only until the
      browser input-focus bug is fixed.
- [ ] Font info editing — NOTE: web cannot edit font info either
      (setFontInfo only feeds renderer metrics), so this is
      beyond-parity; keep for the daily-driver goal
- [ ] New font from template (web newProject.ts /
      newFontTemplate.generated.ts)
- [x] Background layer: show/send/swap/clear against the UFO
      public.background layer (norad layers, saved with the font),
      drawn as a quiet outline behind the drawing
- [x] Reference glyph underlay (name field in the Background
      section; ghost fill behind the drawing)
- [ ] Image trace to glyph (web `traceImageToGlif`)
- [ ] Glyph anatomy panel (web GlyphAnatomyPanel.vue)

## Browser build blockers (the runebender.org end goal)

All four are gpui_web/upstream issues; native is unaffected.

- [ ] Text inputs cannot take focus in the browser except the
      left-panel search (metrics fields, Glyph panel fields — grid
      and editor mode alike) — needs a minimal repro against zed
- [ ] In-window menu items never activate — same class of focus
      bug; needs upstream repro
- [ ] All gpui action dispatch panics on wasm: gpui-component
      force-enables gpui's `profiler` feature and the profiler calls
      `std::time::Instant::now()` per action. Worked around with a
      keystroke interceptor; the real fix is `web_time` in gpui's
      profiler (zed PR) or gpui-component dropping the feature
- [ ] Clipboard read is a stub in gpui_web (no text paste in the
      browser); needs a JS paste-event bridge or upstream support
- [ ] Script icons are tofu in the browser (bundled UI font has no
      Arabic/Hebrew; no system fallback) — bundle a small fallback
      or subset
- [ ] Cross-origin isolation on GitHub Pages: the build uses wasm
      atomics, so deployment needs coi-serviceworker (Pages cannot
      set COOP/COEP headers)
- [ ] Workspace-server flow: SSE live reload in the browser, 409
      conflict UI, and DELETE for removed glyphs (native full save
      already handles removals)

## Smaller UX tail (found in testing, keep adding here)

- [x] Search modes: scope (all/name/unicode), regex, and match-case
      toggles beside the search box
- [x] Copy Selection footer + grid multi-select (cmd-click toggles,
      shift-click ranges in visible order; marks apply to the whole
      selection; the footer copies the selection as text)
- [x] Header save state with timestamp (Saved 1:16 PM after a
      save; the on-disk path was already shown)
- [ ] Master switcher as glyph thumbnails (web MasterToolbar)
- [ ] Multiple edit-session tabs (the strip supports one session;
      web's "+" spawns tabs)
- [x] Missing-glyph indicators + generate: target-bearing language
      rows show a "+" that adds the missing glyphs (named and
      encoded) to every master

## Out of scope for parity

- AI chat / Comfy panel (AiChatPanel.vue) — DesignBot work, parked
- SVG export paths used internally by the web renderer
