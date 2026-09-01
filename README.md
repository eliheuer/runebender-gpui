# runebender-gpui

[![CI](https://github.com/eliheuer/runebender-gpui/actions/workflows/ci.yml/badge.svg)](https://github.com/eliheuer/runebender-gpui/actions/workflows/ci.yml)

The main [Runebender](https://runebender.org) font editor, built on
[GPUI](https://gpui.rs/) and [Linebender](https://linebender.org/)
crates. The window, the input, and the drawing live here; every
operation that changes a font lives in
[runebender-core](https://github.com/eliheuer/runebender-core).

A sibling front-end,
[runebender-xilem](https://github.com/eliheuer/runebender-xilem),
builds the same editor on Xilem. The two exist to compare the
frameworks on one real application, and their layouts mirror each
other so a change in one is easy to carry to the other.

An experimental in-browser build runs at
<https://runebender.org/gpui/>.

## Run it

```sh
cargo run
```

Or install it and open a font from anywhere:

```sh
cargo install --git https://github.com/eliheuer/runebender-gpui
runebender-gpui path/to/Font.designspace
```

Every dependency comes from crates.io or a public git repository, and
every git one is pinned to a revision, so a fresh resolve lands on the
same graph and `--locked` is not needed. The repo pins the stable Rust
toolchain in `rust-toolchain.toml`. A GPUI dependency
(`pathfinder_simd`) does not compile on current nightly toolchains.

The shared editing crate,
[runebender-core](https://github.com/eliheuer/runebender-core), is a
git dependency. To work on both at once, clone it and add a cargo
`paths` override so the local copy replaces the published one:

```sh
git clone https://github.com/eliheuer/runebender-core
```

```toml
# .cargo/config.toml in the directory that holds both checkouts
paths = ["runebender-core"]
```

Put that file above the two repositories, not inside them: the
override is a local development setting, not part of either repo.

GPUI and `gpui_platform` come from the Zed git repository, which does
not publish to crates.io. Both are pinned to a revision; bump them
together.

The widgets this editor needs are in `src/widgets`: text fields on
[parley](https://github.com/linebender/parley)'s `PlainEditor`, a
slider, resizable panel groups, and an in-window menu bar. They
replace gpui-component, which pulled gpui by bare URL (so revisions
could not be pinned) and force-enabled gpui's `profiler` feature,
whose `Instant::now()` call panics on wasm.

`gpui_platform` needs its `font-kit` feature. Without it the platform
falls back to a small embedded font list, and text shapes and paints
without ever reaching the screen: the whole interface comes up
wordless. `runebender-gpui --fonts` prints what gpui can see, and the
app warns on startup if the count collapses.

## The browser build

An experimental wasm build runs at <https://runebender.org/gpui/>.
It needs a different toolchain from the native one:

```sh
RUSTUP_TOOLCHAIN=nightly-2026-08-01 \
CARGO_UNSTABLE_BUILD_STD="std,panic_abort" \
trunk build --release --public-url /gpui/
```

Two constraints decide that line. `gpui_web` needs wasm atomics and
shared memory (see `.cargo/config.toml`), and the prebuilt
`wasm32-unknown-unknown` std has neither, so std must be rebuilt:
that is `-Z build-std`, which is nightly only. The nightly must also
be newer than the pinned stable, because gpui uses library features
stable already has. Install `rust-src` and the wasm target for
whichever nightly you use.

To deploy, copy `dist/` into runebender-dot-org's `public/gpui/`,
keeping `coi-serviceworker.min.js` and adding its script tag to the
top of `<head>`. GitHub Pages cannot send the COOP/COEP headers that
shared memory requires, so the worker sets them client-side.

## Unix-shaped

Font editors are usually one program that owns everything: the
drawing, the scripting, the automation, and lately the assistant.
Runebender is built the other way round, as separable parts that talk
through files.

```
runebender-core   every operation that changes a font, no interface
runebender        the same operations from a shell, --json, exit codes
runebender-gpui   a window that draws them
font-ml           local models over the same sources
```

The interface between them is the UFO on disk. The editor reloads what
changes. A script, a build, or an agent can work on a font while you
have it open, and you watch the edits land.

Three consequences follow, and they are the whole argument:

- **Everything you already have keeps working.** git, fontTools,
  fontc, your CI, your own scripts. Nothing has to be ported inside.
- **The shell reaches what the window reaches**, because both call
  the same crate. A check in a build and a check in a window cannot
  disagree.
- **Exit codes mean something.** 0 ok, 2 usage, 4 failed, so a
  pipeline can branch without parsing prose.

Being exact about the claim: `runebender-core` takes paths rather than
stdin. You cannot pipe one invocation into the next. A font source is
a directory, not a stream.
And a window is not a filter. What holds is that the parts are
separable, the interface is a file, and nothing is trapped inside the
application.

## Working with AI

This is also the AI story, and it is the same story.

Runebender stores no credentials, calls no model service, and has no
assistant in the window. That is deliberate, and it is what makes
agents easy to use with it: the seam is the filesystem. An agent is
just another program that edits files, which is what agents are good
at.

An agent edits the UFO and designspace files on disk. The editor
reloads what changed. Nothing has to be integrated, and you keep
whatever agent and subscription you already use.

```sh
runebender-gpui sources/Font.designspace    # leave it open
# then let an agent work on sources/ in another window
```

Three ways to drive it, in the order most people want them:

- **A coding agent on the repository.** Claude Code, Codex, or
  another. Point it at the font repo and keep the editor open beside
  it to watch the edits land. Give it the font's rules in `AGENTS.md`
  or `CLAUDE.md`, and the manual as one file from
  [runebender.org/llms-full.txt](https://runebender.org/llms-full.txt).
- **Local models**, through
  [font-ml](https://github.com/eliheuer/font-ml). Runs on your
  machine, no account, no network. Every command takes `--json` and
  the exit codes distinguish a usage mistake from an unbuilt job from
  a real failure. Run `font-ml tasks` before assuming a job exists.
- **`runebender-core` as a library**, when you want a tool rather
  than a session. It is the editing model with no interface attached.

Type design has constraints an agent will not infer. Masters must stay
point-compatible. Contour direction and start points matter across
masters. Preserving an advance width is not preserving spacing. Write
these down, and ask for a check after every batch rather than at the
end.

The full guide is at
[runebender.org/docs/agents.html](https://runebender.org/docs/agents.html).

## Layout

The source is organized so a newcomer can navigate it: a `Workspace`
struct in `workspace.rs`, its methods split across `view/`, `edit/`,
and `platform/` by concern, one file per panel and one per menu.
`AGENTS.md` has the table, and
[runebender.org/docs/code-layout.html](https://runebender.org/docs/code-layout.html)
the long version. Every item carries a doc comment, and CI enforces
that.

## Status

At feature parity with the retired web editor, and a little past it.
[PARITY.md](PARITY.md) is the record of what that meant.

## License

Apache-2.0 OR MIT, the Linebender convention.
