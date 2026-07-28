# Provenance

The retained source files and ASCII font data are copied byte-for-byte from
the crates.io package `embedded-graphics 0.8.2`.

- crates.io checksum:
  `4e8da660bb0c829b34a56a965490597f82a55e767b91f9543be80ce8ccb416fe`
- upstream repository:
  <https://github.com/embedded-graphics/embedded-graphics>
- license: MIT OR Apache-2.0

The local copy makes two compatibility changes:

- `Cargo.toml` changes the `az` dependency constraint from `~1.2.0` to `1.3`.
  Retinue's existing firmware graph uses `fixed 1.31.0`, which requires
  `az 1.3`; Cargo cannot resolve the two disjoint, semver-compatible `az 1.x`
  ranges in one workspace lock. `az 1.3` preserves the conversion API used by
  embedded-graphics.
- `src/mono_font/generated/mod.rs` exposes only the ASCII font module used by
  `radio-face`, and only `fonts/raw/ascii` is retained. This keeps the vendored
  compatibility copy bounded instead of importing unused character sets.
- Three lifetime spellings and one unused internal trait method annotation
  silence warnings introduced by newer Rust toolchains without changing
  behavior.

All other retained source files are unchanged.
