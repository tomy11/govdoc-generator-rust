#!/usr/bin/env bash
#
# Start the local OpenAI-compatible embeddings server (see serve_embeddings.py).
# Point the API at it with:
#   EMBEDDING_BACKEND=remote
#   EMBEDDING_BASE_URL=http://127.0.0.1:8090/v1
#   EMBEDDING_MODEL=BAAI/bge-m3
#   EMBEDDING_DIM=1024
#
# Requires sentence-transformers:
#   python3 -m pip install -U sentence-transformers
#
# Env vars:
#   PYTHON                 Python interpreter (default: python3)
#   EMBEDDING_LOCAL_MODEL  Model to load (default: BAAI/bge-m3)
#   EMBEDDING_LOCAL_HOST   Bind host (default: 127.0.0.1)
#   EMBEDDING_LOCAL_PORT   Bind port (default: 8090)
set -euo pipefail

PYTHON="${PYTHON:-python3}"
HERE="$(cd "$(dirname "$0")" && pwd)"

if ! command -v "${PYTHON}" >/dev/null 2>&1; then
  echo "'${PYTHON}' not found. Set PYTHON to your interpreter, e.g. PYTHON=python3" >&2
  exit 1
fi

if ! "${PYTHON}" -c "import sentence_transformers" >/dev/null 2>&1; then
  echo "sentence-transformers not found for ${PYTHON}. Install it with:" >&2
  echo "  ${PYTHON} -m pip install -U sentence-transformers" >&2
  exit 1
fi

exec "${PYTHON}" "${HERE}/serve_embeddings.py"
