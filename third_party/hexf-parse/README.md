# hexf-parse (0BSD)

A copy of the `parse` crate from <https://github.com/lifthrasiir/hexf> at
commit `8a14eb63c3`, which relicensed it from CC0-1.0 to 0BSD. The 0.2.1
release on crates.io still says CC0-1.0, which Fedora does not allow for
code, and upstream has not published the new license. wineo reaches it
through naga and wgpu.

The code is unmodified: `Cargo.toml` and `src/lib.rs` are upstream's, and
`LICENSE` is upstream's text from a later commit (`f43eda3`), since
`8a14eb63c3` has none. The root `Cargo.toml` swaps it in with
`[patch.crates-io]`.

Delete this directory and the patch once upstream publishes a release with
the 0BSD license.
