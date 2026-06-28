# Changelog

## Unreleased

- Vendor Git's C reftable implementation and generate checked-in raw bindings directly from its public headers.
- Use a standalone C platform implementation by default, with the gitoxide-backed Rust implementation available through the `rust-platform` feature.
