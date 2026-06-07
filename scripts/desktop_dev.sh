#!/usr/bin/env bash
#
# Run the Tauri desktop app in dev mode. Prepares the API sidecar binary, then
# launches the app (which spawns the sidecar and serves the ui/ frontend).
#
# Requires the Tauri CLI:
#   cargo install tauri-cli --version "^2.0" --locked
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
"$ROOT/scripts/prepare_sidecar.sh"

if ! cargo tauri --version >/dev/null 2>&1; then
  echo "Tauri CLI not found. Install it with:" >&2
  echo "  cargo install tauri-cli --version \"^2.0\" --locked" >&2
  exit 1
fi

cd "$ROOT"
exec cargo tauri dev
