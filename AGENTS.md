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
| `workspace.rs` | the `Workspace` struct and the types it is made of |
| `actions.rs` | the action list and the menus |
| `startup.rs` | `Workspace::new`: every input widget and its subscription, the keymap, argument handling |
| `view/` | what the window shows: `canvas`, `grid`, `chrome`, `panels/` (one file per region), `render`, `paint`, `blur`, `theme` |
| `edit/` | what the user does: `commands`, `editing`, `input`, `inspector`, `session`, `sidebar`, `text_tool`, `local_ai` |
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

## The gate

CI runs on every push, on Linux and macOS: `cargo fmt --check`,
`cargo clippy --all-targets`, `cargo doc --no-deps`, `cargo test`,
and a release build, with warnings denied. The Linux job installs
the libraries gpui's Wayland and X11 backends link against. `unsafe`
is denied; a test that must set an environment variable allows it
explicitly.

CI's stable can be newer than yours. If clippy passes locally and
fails there, run it under the toolchain CI reports.

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

## Git

- Commit locally as you work. Push when a phase is coherent.
- Commit messages say why. The diff shows what.
- No `Co-Authored-By` trailers for agents.
- Stage explicit paths. Never `git add -A`.
