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
comment that says what the file is for. Read those comments first.

| File | Holds |
|---|---|
| `main.rs` | the `Workspace` state, the actions, the menus, the render tree, `main()` |
| `startup.rs` | `Workspace::new`: every input widget and its subscription, the keymap |
| `commands.rs` | one method per user-facing command |
| `canvas.rs`, `input.rs` | the grid and the editing view; pointer and keyboard on them |
| `panels/` | one file per panel region |
| `chrome.rs` | header, status bar, sliders |
| `session.rs`, `grid.rs`, `sidebar.rs` | master switching and undo; grid geometry; filters and search |
| `editing.rs`, `inspector.rs`, `text_tool.rs` | selection and its operations; the right panel's fields; the text tool |
| `local_ai.rs`, `host.rs` | models on disk; files, watching, the browser host |
| `theme.rs`, `config.rs`, `journal.rs` | theme accessors; the config file; the operation log |
| `widgets/` | text input, slider, resizable split, in-window menu bar |
| `web_host.rs`, `tests.rs` | what the browser build needs; tests of the shell |

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
`../runebender-web/assets/test-fonts` or `$RUNEBENDER_TEST_FONTS`.

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
