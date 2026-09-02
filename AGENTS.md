# AGENTS.md

Context for anyone, human or agent, working on `runebender-gpui`.
The reference for how the code is organized is
[runebender.org/docs/code-layout.html](https://runebender.org/docs/code-layout.html).
This file is the short version plus what you need to build and
submit a change.

## What this is

The Runebender font editor's front-end on GPUI. It owns the window,
the input, and the drawing, and calls
[runebender-core](https://github.com/eliheuer/runebender-core) for
everything that changes a font or reads one. If a change you are
making does font work with no GPUI in it, it belongs in core.

## Which shell is which

This is the current shell. runebender-xilem is the long-term target.
That can change, and the rule that makes the change cheap is:

- No local-AI or task logic lives in a shell. Models, proposals, undo,
  and the task list live in runebender-core and in font-ml.
- font-ml is a separate binary. This shell runs it as a subprocess and
  reads JSON back, the way export runs fontc. It never links font-ml
  or candle.
- A proposal from a model is a UFO layer named
  `com.runebender.proposal.<task>`. The shell shows it and asks core to
  install or discard it. It does not interpret it.
- The shell renders state from core. If a feature needs new state,
  add the state to core first.

## Layout

The editor is one `Workspace` struct. Its methods are split across
files by concern, each an `impl Workspace` block with a module
comment that says what the file is for. Each directory has a
`mod.rs` that says what belongs in it. Read those first.

| Path | Holds |
|---|---|
| `main.rs` | `main()` and the module list |
| `workspace.rs` | the `Workspace` struct: five state groups (`grid`, `sidebar`, `preview`, `models`, `inputs`) plus the tools, drags, and sessions |
| `actions.rs` | the action list and the menus |
| `wiring.rs` | `Workspace::new`: every input widget and the subscription that writes it to the font |
| `launch.rs` | the keymap, the keystroke router, argument handling, the interface font |
| `view/` | what the window shows: `canvas/` (grid views; the editing view as a scene plus one paint function per layer), `grid`, `chrome`, `panels/` (one file per region), `render`, `paint`, `blur`, `theme` |
| `edit/` | what the user does: `commands/` (one file per menu), `editing`, `input`, `inspector`, `session`, `sidebar`, `text_tool`, `local_ai` |
| `platform/` | the world outside the window: `config`, `journal`, `host`, `web_host` |
| `widgets/` | text input, slider, resizable split, in-window menu bar |
| `tests.rs` | tests of the shell itself |

The `use runebender_core::...` list at the top of `main.rs` is the
seam between the two crates.

## Porting between the two editors

The same editor is built twice, on GPUI and on Xilem. A feature that
lands in one should be cheap to carry to the other, so the two share
a file layout: the same concern lives at the same path in both, and
a change is a diff you can read side by side.

Mirror where the concern is shared. Diverge where the framework
forces it, and say so in the file's own module comment. Do not force
a match that costs either editor clarity.

| Concern | Both |
|---|---|
| `main()` and the module list | `main.rs` |
| The `Workspace` struct | `workspace.rs` |
| Actions, the menu bar, the keymap | `actions.rs` |
| The event loop and the first frame | `launch.rs` |
| What the window shows | `view/` |
| What the user does | `edit/` |
| The world outside the window | `platform/` |
| Toolkit pieces the framework lacks | `widgets/` |
| The glyph canvas and the grid | `view/canvas/` |
| One file per panel region | `view/panels/` |
| Files, and reloading one master | `platform/host.rs` |
| Watching for other writers | `platform/watch.rs` |

Where they differ on purpose:

| GPUI | Xilem | Why |
|---|---|---|
| `wiring.rs` | `view/recipes.rs` | GPUI builds input widgets once and subscribes. Xilem rebuilds views from state every frame, so there is nothing to wire; what repeats becomes a recipe. |
| GPUI's own scale | `view/design.rs` | GPUI ships `px_1`, `text_xs`, `rounded_md`. Xilem takes a number wherever a measurement goes, so the scale is application code. |
| `view/blur.rs` | none | GPUI blurs box shadows and nothing else, so the preview's blur is rasterized on the CPU. Vello blurs what it is asked to. |
| `RB_OPEN_GLYPH`, `RB_SIDEBAR_TAB` | `--bin screenshot` | Two ways to see a frame without clicking. Xilem has a headless render path; GPUI opens on a named glyph and a named sidebar tab instead. |
| `widgets/` | `widgets/` | Same directory, different contents: each toolkit is missing different things. |

When one editor gets ahead, the port is: read the file at the same
path in the repository that has the feature, and write the same
decomposition here. If it needs a new file, give it the name the
other one uses, so the next port in the other direction is a diff.

## What is not built yet

`docs/ROADMAP.md` lists the gaps: the browser build's, the one
remaining Arabic item, and the long list against Glyphs 4. It is a
list, not a schedule.

## Build and test

```sh
cargo run path/to/Font.designspace
cargo test
cargo fmt
cargo clippy --all-targets
```

`rust-toolchain.toml` pins stable. To work on core at the same time,
clone it beside this repository and put a `paths` override in a
`.cargo/config.toml` above both checkouts, never inside either.

Two tests compile feature code against Virtua Grotesk from
`../virtua-grotesk/sources` or `$RUNEBENDER_TEST_FONTS`.

Do not launch the GUI to check your work while the user is at the
machine. Verify through tests.

The browser build needs its own toolchain:

```sh
RUSTUP_TOOLCHAIN=nightly-2026-08-01 \
CARGO_UNSTABLE_BUILD_STD="std,panic_abort" \
trunk build --release --public-url /gpui/
```

`gpui_web` needs wasm atomics and shared memory (see
`.cargo/config.toml`), and the prebuilt `wasm32-unknown-unknown` std
has neither, so std is rebuilt: that is `-Z build-std`, nightly only.
The nightly must be newer than the pinned stable. Install `rust-src`
and the wasm target for it. To deploy, copy `dist/` into
runebender-dot-org's `public/gpui/`, keeping
`coi-serviceworker.min.js` and its script tag at the top of `<head>`:
GitHub Pages cannot send the COOP/COEP headers shared memory needs.

`gpui_platform` needs its `font-kit` feature. Without it text shapes
and paints without reaching the screen and the interface comes up
wordless. `runebender-gpui --fonts` prints what gpui can see.

## The gate

CI runs on every push, on Linux and macOS: `cargo fmt --check`,
`cargo clippy --all-targets`, `cargo doc --no-deps`, `cargo test`,
and a release build, with warnings denied.
Clippy's `missing_docs_in_private_items` is on in the manifest, so
every item, `pub(crate)` and private included, needs a doc comment
or the build fails. Imports are named; no glob imports outside test
modules. The Linux job installs
the libraries gpui's Wayland and X11 backends link against. `unsafe`
is denied; a test that must set an environment variable allows it
explicitly.

CI's stable can be newer than yours. If clippy passes locally and
fails there, run it under the toolchain CI reports.

## The interface

`DESIGN.md` says how to change what a person looks at: the token
rule, the canvas and the chrome, how interface text is worded, and
the mistakes worth knowing by name. Read it before touching a view.
The tokens themselves are `view/theme.rs`, plus the structural constants in
`workspace.rs`.

## Conventions

- Call `theme::` accessors instead of naming a colour, radius, or
  stroke width.
- A command is the whole of one intent. The menu item, the shortcut,
  and the context menu all land on the same method in `commands.rs`.
- Panels read the workspace; they do not hold state.
- No path to a sibling checkout in a committed file.
- Core is pinned by git revision in `Cargo.toml`, under
  `[dependencies.runebender-core]`. The last `rev` in the file is
  the Zed pin, not core's.
- Strip `[[patch.unused]]` from `Cargo.lock` before committing; a
  local `paths` override adds it.

## Supply chain

Dependencies are vetted with cargo-vet; `supply-chain/` holds the
audits and exemptions, and CI runs `cargo vet --locked`. CI also runs
`cargo deny check advisories`, which is the other half: vet says
where a crate came from, deny says whether anyone has published a
vulnerability against it. `deny.toml` holds the ignore list, one
entry per advisory with the reason and what would let it go. When you
add or bump a dependency, run `cargo vet` and record the result on
purpose.

## Releases

User-visible changes go under `Unreleased` in `CHANGELOG.md` as they
land. No release exists yet. When the first one is cut:

1. CI green on `main`, the wasm job included.
2. `cargo vet` and `cargo deny check advisories` clean.
3. Pin `runebender-core` to that crate's release tag, not a loose
   revision.
4. Move the `Unreleased` notes in `CHANGELOG.md` under the new
   version heading, with the date.
5. Bump `version` in `Cargo.toml`, tag `vX.Y.Z`, and push the tag.
6. Make a GitHub release from the tag with the changelog section as
   the body, and deploy the browser bundle from the same tag so
   runebender.org/gpui matches it.

Semantic Versioning from the first release; before 1.0, a breaking
change bumps the minor version.

GPUI comes from the Zed git repository, which does not publish to
crates.io, so this crate cannot either. A release is a git tag, and
users install with `cargo install --git ... --tag vX.Y.Z`.

## Git

- Commit locally as you work. Push when a phase is coherent.
- Commit messages say why. The diff shows what.
- No `Co-Authored-By` trailers for agents.
- Stage explicit paths. Never `git add -A`.
