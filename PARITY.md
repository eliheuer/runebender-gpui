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

- [ ] Round selected corners (web `round_selected_corners`)
- [ ] Duplicate selection / duplicate-repeat (icons already in the
      shared icon set; web `duplicateSelection`,
      `duplicateRepeatSelection`)
- [ ] Rotate selection 180°
- [ ] Shift-lock constraints: shapes to squares/circles, pen and
      point drags to axes (web `setShapeShiftLocked`,
      `setKnifeShiftLocked`, pointer-move shift handling)
- [ ] Convert hyperbezier contour to cubic (web
      `convertHyperToCubic`)
- [ ] Numeric move/scale with reference point (web Transform panel:
      `moveSelectionReference`, `resizeSelectionReference`, and the
      9-point coordinate quadrant selector)
- [ ] Right-click context menus: contour (set start point, reverse,
      move contour order), anchor, component (web `contourContextAt`,
      `anchorContextAt`)
- [ ] Component auto-alignment for composites (web
      `componentAlignmentState`, `setComponentAlignment`,
      `realignComposites`) — accents follow their base anchors
- [ ] Add component to a glyph (web `addComponent`)
- [ ] Anchor panel: name + coords editing for the selected anchor
      (web `updateSelectedAnchor`, AnchorPanel.vue)
- [ ] Measure-tool option toggles (web SelectPanel/measure options:
      colorize outline, handle lengths, segment lengths, stems &
      counters, side bearings, popcount sums)
- [ ] Curvature comb + continuity display (web CurvePanel)
- [ ] Sketch tool (parked deliberately; SketchPanel.vue)

## Glyph and font data gaps

- [ ] Edit kerning groups (web `glifWithKerningGroup`; shown
      read-only today)
- [ ] Edit unicode assignment (web `glifWithUnicode`)
- [ ] Rename glyph (web `setGlyphNameWithCachedComponents`)
- [ ] Font info editing (web `setFontInfo`: upm, metrics, names)
- [ ] New font from template (web newProject.ts /
      newFontTemplate.generated.ts)
- [ ] Background layer outline behind the drawing (web
      `setBackgroundOutline`, "show background")
- [ ] Reference glyph underlay (web `setReferenceOutline`)
- [ ] Image trace to glyph (web `traceImageToGlif`)
- [ ] Glyph anatomy panel (web GlyphAnatomyPanel.vue)

## Browser build blockers (the runebender.org end goal)

All four are gpui_web/upstream issues; native is unaffected.

- [ ] Text inputs cannot take focus in editor mode (metrics fields
      dead in the browser) — needs a minimal repro against zed
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

- [ ] Search modes: name/unicode scope, regex, match-case (web
      sidebar search dropdown)
- [ ] Copy Selection footer in the sidebar (web CategorySidebar)
- [ ] Header save state with timestamp + on-disk path (web TopBar)
- [ ] Master switcher as glyph thumbnails (web MasterToolbar)
- [ ] Multiple edit-session tabs (the strip supports one session;
      web's "+" spawns tabs)
- [ ] Missing-glyph indicators in language filters (web shows what
      a glyphset still needs; core already computes targets)

## Out of scope for parity

- AI chat / Comfy panel (AiChatPanel.vue) — DesignBot work, parked
- SVG export paths used internally by the web renderer
