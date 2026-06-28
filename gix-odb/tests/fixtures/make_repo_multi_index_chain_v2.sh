#!/usr/bin/env bash
set -eu -o pipefail

# Create a repository whose five V2 layers are compacted to three, with the middle layer
# preserving the non-lexicographic pack order C, A, B for object-store lookup tests.
git init -q repo
cd repo
git checkout -q -b main
git config maintenance.auto false
git config midx.version 2

pack_dir=.git/objects/pack
chain_file=$pack_dir/multi-pack-index.d/multi-pack-index-chain

for name in D C A B E; do
  echo "$name" > "file-$name"
  git add .
  git commit -qm "pack $name"
  git pack-objects --all --unpacked "$pack_dir/pack-$name" >/dev/null
  git prune-packed
  git multi-pack-index write --incremental
done

from=$(awk 'NR == 2 { print; exit }' "$chain_file")
to=$(awk 'NR == 4 { print; exit }' "$chain_file")
git multi-pack-index compact --incremental "$from" "$to"

git count-objects -v > expected-count-objects
git cat-file --batch-all-objects --batch-check='%(objectname)' > all-objects
