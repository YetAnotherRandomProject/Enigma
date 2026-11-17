#!/usr/bin/env sh
# CI locale : formatage, lints, tests.
set -eu
cd "$(dirname "$0")/.."
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
