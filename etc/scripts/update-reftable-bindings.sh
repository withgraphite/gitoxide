#!/usr/bin/env bash

set -eu -o pipefail

root="$(git rev-parse --show-toplevel)"
crate="$root/gix-reftable-sys"

test "$(bindgen --version)" = "bindgen 0.72.1"

bindgen "$crate/include/wrapper.h" \
    --output "$crate/src/bindings.rs" \
    --allowlist-function '^reftable_.*' \
    --allowlist-type '^reftable_.*' \
    --allowlist-var '^REFTABLE_.*' \
    --no-layout-tests \
    --no-doc-comments \
    --disable-header-comment \
    --merge-extern-blocks \
    --rust-target 1.85 \
    --rust-edition 2024 \
    -- \
    -I"$crate/include" \
    -I"$crate/vendor/reftable"
