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

## Supply chain and releases

Dependencies are vetted with cargo-vet; `supply-chain/` holds the
audits and exemptions, and CI runs `cargo vet --locked`. When you
add or bump a dependency, run `cargo vet` and record the result on
purpose. Releases do not exist yet; `RELEASING.md` is the checklist
for the first one, and user-visible changes go under `Unreleased`
in `CHANGELOG.md`.

## Git

- Commit locally as you work. Push when a phase is coherent.
- Commit messages say why. The diff shows what.
- No `Co-Authored-By` trailers for agents.
- Stage explicit paths. Never `git add -A`.
