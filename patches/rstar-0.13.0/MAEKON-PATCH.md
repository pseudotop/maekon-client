# rstar 0.13.0 OCR compatibility patch

Maekon uses the 0.13.0 R-tree for click and typing correlation in OCR results. This patch replaces its stack-backed queue and spill path with Rust's allocation-backed binary heap. It removes the dependency on heapless 0.8. The distance calculation, tree construction and Maekon's spatial-index threshold stay in the 0.13.0 implementation. [Upstream source](https://github.com/georust/rstar/tree/82c969d4677a6f624900056e044d7e7ec8439c18/rstar)

## Provenance

The crates.io archive has SHA-256 `5912b862fa5ffb462607bfd1e35036c458c537921f508c8235a83d5f3987edfe`. Its package metadata identifies upstream commit `82c969d4677a6f624900056e044d7e7ec8439c18`. The README refers to the two license files in the repository root. This directory includes both original files byte for byte. [Crate archive](https://static.crates.io/crates/rstar/rstar-0.13.0.crate) · [Apache license](LICENSE-APACHE) · [MIT license](LICENSE-MIT)

The delta against the published crate includes the normalized manifest, the queue implementation, the versioned API link in the README and the two license files. Follow the packaging exclusions in the parent refresh procedure when comparing against a freshly unpacked archive. [Source delta](UPSTREAM-DELTA.patch) · [Refresh procedure](../README.md#refresh-procedure)

## Removal and verification

A replacement must remove the heapless advisories and preserve the chosen OCR label in direct hits and equal-distance matches across the spatial-index threshold. In 20,736 generated clicks spanning 396 to 4,097 regions, this patch matched rc.11 every time. The published 0.14.0 package changed 1,272 selections on the same inputs. Use the actual detector with the candidate lock for this comparison. Check the crate's unit and documentation tests, the maekon-vision suite, the four supported target graphs, cargo vet, cargo deny and the complete root-lock advisory scan before dropping the override. [Upstream maintenance work](https://github.com/georust/rstar/pull/274)
