# gix-reftable-sys

This crate vendors Git's BSD-licensed C reftable library and exposes its raw, unsafe API.
Use `gix-reftable` for safe, lifetime-aware gitoxide types.
The checked-in Rust FFI is generated directly from all public `reftable-*.h` headers; consumers do not need bindgen or libclang.

The exact upstream commit and blob IDs are recorded in `UPSTREAM-PROVENANCE.tsv`. Refresh the snapshot from a Git checkout with:

```sh
etc/scripts/update-reftable-vendor.sh /path/to/git 014c454799dbf281e634823c6134e8e95978df6f
```

A standalone C platform layer matching Git's `system.c` behavior is used by default, without depending on Git's internal tempfile, lockfile, and compatibility libraries. Enable the `rust-platform` feature to use the alternative implementation backed by gitoxide locking and tempfile primitives.
The small safety patches carried on top of upstream are documented in `LOCAL-PATCHES.md`, stored as `LOCAL-PATCHES.patch`, and enforced by the provenance test.
After refreshing or patching headers, regenerate bindings with `etc/scripts/update-reftable-bindings.sh`.
