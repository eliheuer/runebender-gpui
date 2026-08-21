# runebender-gpui

A font editor built on [GPUI](https://gpui.rs/) and
[Linebender](https://linebender.org/) crates. This is a sibling of
[runebender-xilem](https://github.com/eliheuer/runebender-xilem). The
two ports exist to compare Xilem and GPUI on the same application and
to measure the trade-offs between them.

An experimental in-browser build runs at
<https://runebender.org/gpui/>.

## Run it

```sh
cargo run
```

Or install it and open a font from anywhere:

```sh
cargo install --locked --git https://github.com/eliheuer/runebender-gpui
runebender-gpui path/to/Font.designspace
```

Every dependency comes from crates.io or a public git repository, so a
plain clone builds. Use `--locked` when installing: several
dependencies track a git branch, and the committed `Cargo.lock` holds
the revisions this editor is known to build against. The repo pins the stable Rust toolchain in
`rust-toolchain.toml`. A GPUI dependency (`pathfinder_simd`) does not
compile on current nightly toolchains.

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

GPUI and `gpui_platform` come from the Zed git repository (not
crates.io) because the editor shell uses
[gpui-component](https://github.com/longbridge/gpui-component),
which tracks Zed main.

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
top of `<head>` — GitHub Pages cannot send the COOP/COEP headers
that shared memory requires, so the worker sets them client-side.

## Status

Close to feature parity with the previous web editor.
[PARITY.md](PARITY.md) tracks what remains.

## License

Apache-2.0.
