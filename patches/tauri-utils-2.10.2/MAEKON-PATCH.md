# tauri-utils 2.10.2 feature selection

The Tauri configuration utilities use serde_with macros. The default features of serde_with pull the original chrono package into Cargo's resolved graph. This local manifest selects the macros used by tauri-utils and turns off the default features of that dependency. [Upstream source](https://github.com/tauri-apps/tauri/tree/59b63ab500da9859f12d8888c3298bc29bba0b92/crates/tauri-utils)

## Provenance

The crates.io archive has SHA-256 `362ab21cd4cc713f2a02b1a9b5688198b1541cfb6907c4281e9190e0f0ef4aa3`. Its package metadata identifies upstream commit `59b63ab500da9859f12d8888c3298bc29bba0b92`. Both packaged license files match that commit. The source delta changes the serde_with dependency in the normalized Cargo manifest. [Crate archive](https://static.crates.io/crates/tauri-utils/tauri-utils-2.10.2.crate) · [Apache license](LICENSE-APACHE-2.0) · [MIT license](LICENSE-MIT) · [Source delta](UPSTREAM-DELTA.patch)

## Removal and verification

Remove the override when the upstream dependency graph resolves without the original chrono package and the desktop configuration still round-trips. Verify the actual Tauri configuration tests, inspect the four release-target dependency graphs with the application features, and repeat cargo vet, cargo deny and the complete root-lock advisory scan. [Refresh procedure](../README.md#refresh-procedure)
