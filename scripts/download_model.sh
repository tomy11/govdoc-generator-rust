#!/usr/bin/env bash
#
# Prepare the Typhoon MLX model weights for the local LLM provider.
#
# There is no prebuilt MLX repo for typhoon2.5-qwen3-4b, so by default this
# converts the public base model to a 4-bit MLX build with mlx_lm.convert. The
# base (~8 GB) is downloaded automatically; the quantized output (~2.5 GB) lands
# in ./models, which is gitignored. Run this once per machine before starting
# the API with LLM_BACKEND=typhoon-local.
#
# Override via env vars:
#   PYTHON           Python interpreter (default: python3)
#   LLM_MODEL_REPO   Base HF repo to convert (default: typhoon-ai/typhoon2.5-qwen3-4b)
#   LLM_MODEL_PATH   Local destination dir  (default: models/typhoon2.5-qwen3-4b-mlx)
#   MLX_QUANTIZE     Set to 0 to convert without 4-bit quantization (default: 1)
set -euo pipefail

PYTHON="${PYTHON:-python3}"
MODEL_REPO="${LLM_MODEL_REPO:-typhoon-ai/typhoon2.5-qwen3-4b}"
MODEL_PATH="${LLM_MODEL_PATH:-models/typhoon2.5-qwen3-4b-mlx}"
MLX_QUANTIZE="${MLX_QUANTIZE:-1}"

if ! command -v "${PYTHON}" >/dev/null 2>&1; then
  echo "'${PYTHON}' not found. Set PYTHON to your interpreter, e.g. PYTHON=python3" >&2
  exit 1
fi

if ! "${PYTHON}" -c "import mlx_lm" >/dev/null 2>&1; then
  echo "mlx_lm not found for ${PYTHON}. Install it with:" >&2
  echo "  ${PYTHON} -m pip install -U mlx-lm" >&2
  exit 1
fi

quant_args=()
if [ "${MLX_QUANTIZE}" != "0" ]; then
  quant_args=(-q)
fi

echo "Converting ${MODEL_REPO} -> ${MODEL_PATH} (quantize=${MLX_QUANTIZE})"
"${PYTHON}" -m mlx_lm convert \
  --hf-path "${MODEL_REPO}" \
  --mlx-path "${MODEL_PATH}" \
  "${quant_args[@]}"

echo
echo "Done. Model is at ${MODEL_PATH}"
echo "Next: start the server with scripts/serve_llm.sh"
