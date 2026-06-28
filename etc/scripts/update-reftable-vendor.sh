#!/usr/bin/env bash

set -eu -o pipefail

git_dir="${1:?first argument must be a Git source checkout}"
commit="${2:?second argument must be the Git commit to vendor}"
root="$(git rev-parse --show-toplevel)"
destination="$root/gix-reftable-sys/vendor/reftable"
manifest="$root/gix-reftable-sys/UPSTREAM-PROVENANCE.tsv"

git -C "$git_dir" cat-file -e "${commit}^{commit}"
rm -rf "$destination"
mkdir -p "$destination"

while IFS= read -r path; do
    base="${path##*/}"
    case "$base" in
        system.c|reftable-system.h) continue ;;
    esac
    git -C "$git_dir" show "${commit}:${path}" > "$destination/$base"
done < <(git -C "$git_dir" ls-tree -r --name-only "$commit" reftable)

{
    printf '# upstream-repo: https://github.com/git/git\n'
    printf '# upstream-commit: %s\n' "$commit"
    printf '# columns: path<TAB>upstream-blob<TAB>cat-file\n'
    for file in "$destination"/*; do
        base="${file##*/}"
        blob="$(git -C "$git_dir" rev-parse "${commit}:reftable/$base")"
        printf 'vendor/reftable/%s\t%s\tgit cat-file -p %s:reftable/%s\n' \
            "$base" "$blob" "$commit" "$base"
    done
} > "$manifest"

printf 'Run (cd gix-reftable-sys && git apply LOCAL-PATCHES.patch), then etc/scripts/update-reftable-bindings.sh.\n' >&2
