# runebender-gpui

A font editor built on [GPUI](https://gpui.rs/), the UI framework from
[Zed](https://zed.dev/). This is a sibling of
[runebender-xilem](https://github.com/eliheuer/runebender-xilem). The
two ports exist to compare Xilem and GPUI on the same application and
to measure the trade-offs between them.

[runebender-web](https://github.com/eliheuer/runebender-web) is the
most complete version and is the reference for features and behavior.

## Run it

```sh
cargo run
```

The repo pins the stable Rust toolchain in `rust-toolchain.toml`. A
GPUI dependency (`pathfinder_simd`) does not compile on current
nightly toolchains.

GPUI and `gpui_platform` come from the Zed git repository (not
crates.io) because the editor shell uses
[gpui-component](https://github.com/longbridge/gpui-component),
which tracks Zed main.

## Status

Early but usable for simple edits: glyph grid with search,
double-click to edit, drag points or marquee-select, arrows nudge,
Cmd+Z/Cmd+Shift+Z undo/redo, wheel pan, Cmd+wheel zoom, Cmd+S
saves the UFO.

## License

Apache-2.0.
