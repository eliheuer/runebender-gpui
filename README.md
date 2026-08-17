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

## Status

Early. The editor opens a window and nothing more yet.

## License

Apache-2.0.
