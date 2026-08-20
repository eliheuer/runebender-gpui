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

The repo pins the stable Rust toolchain in `rust-toolchain.toml`. A
GPUI dependency (`pathfinder_simd`) does not compile on current
nightly toolchains.

GPUI and `gpui_platform` come from the Zed git repository (not
crates.io) because the editor shell uses
[gpui-component](https://github.com/longbridge/gpui-component),
which tracks Zed main.

## Status

Close to feature parity with the previous web editor.
[PARITY.md](PARITY.md) tracks what remains.

## License

Apache-2.0.
