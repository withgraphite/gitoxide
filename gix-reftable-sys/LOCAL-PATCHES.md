# Local reftable patches

The vendored tree is based on Git commit `014c454799dbf281e634823c6134e8e95978df6f`. The following focused hardening changes are carried locally:

- validate stack basenames before joining them to the reftable directory;
- replace implementation-defined public bitfields with fixed-width fields for portable Rust FFI;
- reject truncated tables and malformed block/restart bounds;
- make block offsets 64-bit and avoid releasing an uninitialized zlib stream;
- fix object-prefix and reflog update-index comparators;
- reject unsafe block sizes and exhausted or inverted update-index ranges.

The complete, machine-applicable diff is stored in `LOCAL-PATCHES.patch`. After refreshing the upstream snapshot, reapply it from this directory with:

```sh
git apply LOCAL-PATCHES.patch
```

`tests/provenance.rs` requires every other vendored file to remain byte-identical to its recorded upstream blob.
