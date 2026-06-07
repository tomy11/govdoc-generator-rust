#!/usr/bin/env bash
#
# Build the govdoc-api binary and place it where Tauri expects the sidecar:
#   src-tauri/binaries/govdoc-api-<target-triple>
#
# Run this before `cargo tauri dev` / `cargo tauri build`. Safe to run from any
# directory (it resolves the repo root from its own location).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TRIPLE="$(rustc -vV | sed -n 's/host: //p')"

cargo build -p govdoc-api --release --manifest-path "$ROOT/Cargo.toml"
mkdir -p "$ROOT/src-tauri/binaries"
cp "$ROOT/target/release/govdoc-api" "$ROOT/src-tauri/binaries/govdoc-api-${TRIPLE}"

echo "sidecar ready: src-tauri/binaries/govdoc-api-${TRIPLE}"
