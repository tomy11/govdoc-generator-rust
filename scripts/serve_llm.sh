#!/usr/bin/env bash
#
# Start an OpenAI-compatible LLM server for the local Typhoon MLX model.
#
# This is the sidecar that TyphoonLocalProvider talks to over HTTP. Run it in
# its own terminal, or let the API spawn it automatically by setting
# LLM_AUTO_SERVE=1 (see maybe_start_local_llm in govdoc-api).
#
# Requires mlx-lm on Apple Silicon:
#   pip install -U mlx-lm
#
# Env vars:
#   PYTHON            Python interpreter to use (default: python3)
#   LLM_MODEL_PATH    Model dir or HF repo to load (default: models/typhoon2.5-qwen3-4b-mlx)
#   LLM_SERVER_HOST   Bind host  (default: 127.0.0.1)
#   LLM_SERVER_PORT   Bind port  (default: 8080)
set -euo pipefail

PYTHON="${PYTHON:-python3}"
MODEL_PATH="${LLM_MODEL_PATH:-models/typhoon2.5-qwen3-4b-mlx}"
HOST="${LLM_SERVER_HOST:-127.0.0.1}"
PORT="${LLM_SERVER_PORT:-8080}"

if ! command -v "${PYTHON}" >/dev/null 2>&1; then
  echo "'${PYTHON}' not found. Set PYTHON to your interpreter, e.g. PYTHON=python3" >&2
  exit 1
fi

if ! "${PYTHON}" -c "import mlx_lm" >/dev/null 2>&1; then
  echo "mlx_lm not found for ${PYTHON}. Install it with:" >&2
  echo "  ${PYTHON} -m pip install -U mlx-lm" >&2
  exit 1
fi

echo "Serving ${MODEL_PATH} on http://${HOST}:${PORT}/v1"
exec "${PYTHON}" -m mlx_lm.server \
  --model "${MODEL_PATH}" \
  --host "${HOST}" \
  --port "${PORT}"
