# govdoc-generator-rust

Rust workspace for a desktop-first Thai government document generator.

This project is a Rust port of `govdoc-generator`, with a local-first storage
plan:

- SQLite for metadata, templates, raw text, and document fields
- HNSW vector index for semantic retrieval
- Optional Tantivy full-text index later if keyword search becomes important

The current milestone is API/domain behavior parity before building a Tauri
desktop shell.

## Workspace

- `crates/govdoc-domain`: document schemas and deterministic Thai government
  salutation/closing rules.
- `crates/govdoc-usecases`: generation, critic loop, editing, and provider
  traits.
- `crates/govdoc-storage`: SQLite template/memory storage and HNSW similarity
  index boundary.
- `crates/govdoc-api`: local Axum API for desktop/Tauri sidecar use.
- `scripts/render_docx_sidecar.py`: Python sidecar bridge for `.docx`
  rendering while native Rust rendering is out of scope.
- `scripts/download_model.sh` / `scripts/serve_llm.sh`: fetch and serve the
  local Typhoon MLX model for the real LLM provider.
- `migrations/0001_init.sql`: initial database schema.

## Quick Start

```bash
cargo test --workspace
cargo run -p govdoc-api
```

The API listens on `127.0.0.1:8000` by default. Override it with:

```bash
GOVDOC_API_ADDR=127.0.0.1:9000 cargo run -p govdoc-api
```

## API Surface

The local API currently exposes:

- `GET /health`
- `POST /generate`
- `POST /edit`
- `POST /render`
- `GET /templates`
- `POST /templates`
- `GET /templates/default`

Generation and editing are wired through provider traits. The backend is
selected at startup by `LLM_BACKEND`:

- `fake` (default): deterministic in-process stub used by tests and offline runs.
- `typhoon-local`: a local OpenAI-compatible server (`mlx_lm.server` running a
  Typhoon MLX model on Apple Silicon).
- `typhoon-cloud`: the hosted Typhoon API at `https://api.opentyphoon.ai/v1`,
  authenticated with `LLM_API_KEY`.

```bash
# Hosted Typhoon cloud (no local model needed)
LLM_BACKEND=typhoon-cloud LLM_API_KEY=sk-... cargo run -p govdoc-api
```

Both Typhoon backends share the same `TyphoonProvider` and `LLM_*` settings;
they differ only in default base URL/model and whether a key is required. Get a
cloud key at <https://opentyphoon.ai>.

## Semantic Retrieval (Embeddings)

`/generate` retrieves similar past examples to ground the draft. The embedding
backend is chosen by `EMBEDDING_BACKEND`:

- `fake` (default): zero vectors. Retrieval falls back to recency-based lookup.
- `remote`: any OpenAI-compatible `/v1/embeddings` endpoint via
  `TyphoonEmbeddingProvider` (`EMBEDDING_BASE_URL`/`EMBEDDING_MODEL`, with the
  API key reused from `LLM_API_KEY`).

Examples are stored in SQLite (`gov_doc_memory`) with their embeddings;
`SqliteMemoryRepository` builds an in-memory cosine index per request and
returns the nearest examples. The store starts empty, so retrieval returns
nothing until examples are ingested — that ingestion path (e.g. via OCR) is the
next piece of work.

> Note: a hosted Typhoon embeddings endpoint was not confirmed available at the
> time of writing; the provider targets the OpenAI-compatible contract so it
> works against Typhoon if/when it ships, or any other compatible service today.

## Local Typhoon (MLX) LLM

MLX cannot be driven from Rust directly, so the model runs behind a local
OpenAI-compatible HTTP server and `TyphoonLocalProvider` talks to it. The same
provider works against any OpenAI-compatible endpoint (vLLM, the Typhoon cloud
API) by changing `LLM_BASE_URL`.

One-time setup (requires `huggingface-cli` and `mlx-lm`):

```bash
# 1. Download weights into ./models (gitignored, ~2.5 GB)
./scripts/download_model.sh

# 2. Start the model server on http://127.0.0.1:8080/v1
./scripts/serve_llm.sh

# 3. Run the API against it
LLM_BACKEND=typhoon-local cargo run -p govdoc-api
```

Alternatively, let the API start and wait for the server itself:

```bash
LLM_BACKEND=typhoon-local LLM_AUTO_SERVE=1 cargo run -p govdoc-api
```

See `.env.example` for all `LLM_*` knobs (base URL, model id, temperature, port,
serve timeout). Embeddings still use the fake provider; the retrieval path falls
back to non-vector retrieval when no real embedding provider is configured.

## Render Sidecar

`POST /render` requires `GOVDOC_RENDERER_CMD` when rendering valid document
JSON:

```bash
GOVDOC_RENDERER_CMD="python3 scripts/render_docx_sidecar.py" cargo run -p govdoc-api
```

`GOVDOC_PYTHON_SOURCE` may point to the original Python project when parity with
existing `.docx` templates is needed.

## Validation

Run the full local validation suite before merging release candidates:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
```

## Milestone Status

- M1: Rust workspace, configuration, and error handling are in place.
- M2: Domain data models and JSON serialization are ported.
- M3: Core generation/edit/storage logic has unit coverage.
- M4: Local API interface is available.
- M5: API behavior parity tests cover the main local contracts.
- M6: Cleanup, docs, and release build validation are the current focus.
