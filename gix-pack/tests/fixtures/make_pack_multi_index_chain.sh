#!/usr/bin/env bash
set -eu -o pipefail

# Create repositories with an *incremental* multi-pack index, i.e. a chain of multi-pack index
# layers stored in `objects/pack/multi-pack-index.d/`, as written by
# `git multi-pack-index write --incremental` which requires git 2.47 or newer.
#
# Each round of commits is repacked into a pack of its own before a new layer is appended to the
# chain, so every layer indexes exactly one pack. All file contents are unique, hence each object
# is contained in exactly one layer.
#
# The `three-layer` repository contains ~900 objects so that it statistically always contains
# object ids that are ambiguous on their shortest possible prefix, also across layers,
# which prefix-lookup tests depend on.
#
# `midx.version=1` is set explicitly as gitoxide can only read version 1 multi-pack indexes,
# while git may default to version 2 in the future. Older git versions ignore the setting.
#
# The list of all objects is recorded in `all-objects` for tests to compare against.

function make_chained_repo() {
  local num_layers=${1:?the number of chain layers to create}
  local files_per_layer=${2:?the number of files to add per layer}

  git init -q
  git checkout -q -b main

  local round file_id
  for round in $(seq "$num_layers"); do
    for file_id in $(seq -w "$files_per_layer"); do
      echo "$round-$file_id" > "file-$round-$file_id"
    done
    git add .
    git commit -qm "round $round"
    git repack -dq
    git -c midx.version=1 multi-pack-index write --incremental
  done

  git cat-file --batch-all-objects --batch-check='%(objectname)' > all-objects
}

(mkdir one-layer && cd one-layer && make_chained_repo 1 8)
(mkdir three-layer && cd three-layer && make_chained_repo 3 300)
