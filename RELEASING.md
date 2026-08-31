# Releasing

How a runebender-gpui release will be cut. No release exists yet;
this file exists so the first one is mechanical.

## Checklist

1. Make sure CI is green on `main`, the wasm job included.
2. Run `cargo vet` and clear anything it raises. New dependencies
   land as audits or exemptions in `supply-chain/`, never silently.
3. Pin `runebender-core` to that crate's release tag, not a loose
   revision.
4. Move the `Unreleased` notes in `CHANGELOG.md` under the new
   version heading, with the date.
5. Bump `version` in `Cargo.toml`, tag `vX.Y.Z`, and push the tag.
6. Create a GitHub release from the tag, pasting the changelog
   section.
7. Build and deploy the browser bundle from the tag, so
   runebender.org/gpui matches the release.

## Distribution

The editor depends on GPUI from the Zed git repository, which does
not publish to crates.io, so this crate cannot be published there.
A release is a git tag; users install with
`cargo install --git https://github.com/eliheuer/runebender-gpui --tag vX.Y.Z`.

## Versioning

Semantic Versioning from the first release. Before 1.0, breaking
changes bump the minor version.
