# Designing this editor

The shared rules are
[DESIGN.md in runebender-core](https://github.com/eliheuer/runebender-core/blob/main/DESIGN.md):
name a token rather than a value, keep the canvas quiet, and the
mistakes worth knowing by name. Read that first. This file is the
part that is specific to GPUI.

## Where the tokens are

| What | Where |
|---|---|
| Colour, radius, stroke | `view/theme.rs`, resolved from core's `themes/runebender.theme.json` |
| Space and size | GPUI's own scale: `px_1()`, `px_2()`, `text_xs()`, `rounded_md()` |
| Fixed measurements | the constants in `workspace.rs`: `CELL`, `GRID_GAP`, `GRID_PAD`, `TAB_H`, `BOTTOM_BAR_H`, `BAR_BUTTON` |
| Drawing on the canvas | `view/canvas/editor.rs`, one `paint_*` function per layer |

GPUI ships the spacing and type scale, so this editor has no design
system file of its own. Use GPUI's scale and reach for a constant
only when a measurement is structural, such as the height of the tab
bar or the size of a grid cell. A new constant goes in
`workspace.rs`, next to the others, with a comment saying what it
measures.

## Conventions

- Call a `theme::` accessor. Never construct an `Rgba` in a view.
- Panels read the workspace and do not hold state, so what you see is
  always what the document says.
- A new panel goes in `view/panels/`, one file per region, and gets
  its measurements from the same places the others use.
- The canvas paints in layers, back to front. A new layer is a new
  `paint_*` function called from `paint_scene`, not a branch inside
  an existing one.

## Looking at it

`RB_OPEN_GLYPH=<name> cargo run <font>` starts in the editor on that
glyph, so a capture needs no clicks. Check a change in Gray and in
Light.

Do not launch the GUI while the user is at the machine.
