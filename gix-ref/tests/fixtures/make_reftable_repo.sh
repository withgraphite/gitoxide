#!/usr/bin/env bash
set -eu -o pipefail

git init -q
git config user.name committer
git config user.email committer@example.com
git config commit.gpgSign false

git checkout -b main
touch this
git add this
git commit -q -m c1
echo hello >> this
git commit -q -am c2

git clone --ref-format=reftable . reftable-clone
