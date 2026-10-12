#!/bin/bash
set -euo pipefail

# get staged files (filter only added, copied, modified)
staged=$(git diff --cached --name-only --diff-filter=ACM)

if [ -z "$staged" ]; then
  exit 0
fi

# check formatting
dprint check --staged --allow-no-files

# get staged rust files
staged_rs=$(echo "$staged" | grep '\.rs$' || true)

# lint and test if there are staged rust files
if [ -n "$staged_rs" ]; then
  cargo clippy --all-targets --all-features -- -D warnings
  cargo test --all-features
fi
