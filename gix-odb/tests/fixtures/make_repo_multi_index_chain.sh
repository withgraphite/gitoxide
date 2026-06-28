#!/usr/bin/env bash
set -eu -o pipefail

# Create a repository with an *incremental* multi-pack index, i.e. a chain of multi-pack index
# layers stored in `objects/pack/multi-pack-index.d/`, as written by
# `git multi-pack-index write --incremental` which requires git 2.47 or newer.
#
# Each round of commits is repacked into a pack of its own before a new layer is appended to the
# chain, so every layer indexes exactly one pack. All file contents are unique, hence each object
# is contained in exactly one layer.
#
# A *standalone* multi-pack index covering the same packs is stored next to the repository as
# `multi-pack-index-flat` so tests can simulate a flat multi-pack index appearing next to (or
# replacing) the chain, like a non-incremental `git multi-pack-index write` would.
#
# `midx.version=1` is set explicitly as gitoxide can only read version 1 multi-pack indexes,
# while git may default to version 2 in the future. Older git versions ignore the setting.

git init -q repo
cd repo
git checkout -q -b main

for round in $(seq 3); do
  for file_id in $(seq -w 30); do
    echo "$round-$file_id" > "file-$round-$file_id"
  done
  git add .
  git commit -qm "round $round"
  git repack -dq
  git -c midx.version=1 multi-pack-index write --incremental
done

git count-objects -v > expected-count-objects
git cat-file --batch-all-objects --batch-check='%(objectname)' > all-objects

# Writing a standalone multi-pack index would remove the chain, so move it out of harm's way.
mv .git/objects/pack/multi-pack-index.d ../multi-pack-index.d.bak
git -c midx.version=1 multi-pack-index write
mv .git/objects/pack/multi-pack-index ../multi-pack-index-flat
mv ../multi-pack-index.d.bak .git/objects/pack/multi-pack-index.d
