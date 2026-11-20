#!/usr/bin/env bash
# Local CI: formatting, lints, tests. Same steps the GitHub Actions workflow will run.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> cargo fmt --check"
cargo fmt --all -- --check

echo "==> cargo clippy"
cargo clippy --workspace --all-targets --locked -- -D warnings

# tests/vectors.rs also fails if protocol/vectors/*.json is stale.
echo "==> cargo test"
cargo test --workspace --locked

echo "OK"
