# Runebender GPUI

[![CI](https://github.com/eliheuer/runebender-gpui/actions/workflows/ci.yml/badge.svg)](https://github.com/eliheuer/runebender-gpui/actions/workflows/ci.yml)

The current primary [Runebender](https://runebender.org) frontend
GUI, built on [GPUI](https://gpui.rs/). Everything else for the
application is in
[runebender-core](https://github.com/eliheuer/runebender-core).
[Runebender-Xilem](https://github.com/eliheuer/runebender-xilem)
builds the same editor on Xilem, to compare the two frameworks on one
real application.

An experimental in-browser build runs at
<https://runebender.org/gpui/>.

## Use

```sh
cargo install --git https://github.com/eliheuer/runebender-gpui
runebender-gpui path/to/Font.designspace
```

The manual is at [runebender.org](https://runebender.org/docs/).

## License

Apache-2.0 OR MIT
