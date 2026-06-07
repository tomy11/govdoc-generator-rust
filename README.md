# govdoc-generator-rust

[![CI](https://github.com/tomy11/govdoc-generator-rust/actions/workflows/ci.yml/badge.svg)](https://github.com/tomy11/govdoc-generator-rust/actions/workflows/ci.yml)

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

Configuration is read from the environment; a `.env` file in the working
directory is loaded automatically (shell env wins over `.env`). Copy
`.env.example` to `.env` and fill in secrets like `LLM_API_KEY` there rather
than passing them on the command line. Set `SQLITE_PATH` to persist templates
and ingested examples across restarts; without it the store is in-memory only.

## Desktop App (Tauri)

A Tauri v2 desktop shell lives in `src-tauri/` with a dependency-free static
frontend in `ui/`. It uses the **sidecar** approach: the app bundles the
`govdoc-api` binary, spawns it on launch (and kills it on exit), and the webview
talks to it over `http://127.0.0.1:8000` (permissive CORS makes this work).

The shell is excluded from the cargo workspace (heavy, platform-specific deps),
so `cargo test --workspace` and CI are unaffected.

```bash
# one-time: cargo install tauri-cli --version "^2.0" --locked
./scripts/desktop_dev.sh        # builds the sidecar, then runs `cargo tauri dev`
```

`scripts/prepare_sidecar.sh` builds `govdoc-api --release` and places it at
`src-tauri/binaries/govdoc-api-<target-triple>` (gitignored) where Tauri expects
it. Configure the runtime (cloud vs local backends) via `.env` as usual — the
sidecar inherits it. For a distributable bundle: `cargo tauri build`.

## API Surface

The local API currently exposes:

- `GET /` or `GET /docs` (JSON endpoint index; no Swagger UI)
- `GET /health`
- `GET /status` (active backends + readiness, for the UI)
- `POST /generate`
- `POST /edit`
- `POST /render`
- `POST /ingest`
- `POST /ingest/ocr`
- `POST /documents` · `GET /documents` · `GET /documents/:id` · `DELETE /documents/:id`
- `GET /templates`
- `POST /templates`
- `GET /templates/default`

CORS is permissive (the API binds to localhost and is meant to run as a Tauri
sidecar / local tool), so a desktop webview can call it cross-origin. `GET
/status` reports which backends are active so the UI can guide setup.

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

Examples are stored in SQLite (`gov_doc_memory`) with their embeddings.
`SqliteMemoryRepository` answers similarity queries from a persistent vector
index (`HNSW_INDEX_PATH`) that is loaded at startup, updated on ingest, and
rebuilt from SQLite when missing. SQLite stays the source of truth; the index is
a derived cache. Search is a brute-force cosine scan (fine at desktop scale; a
true HNSW graph can replace the internals later). The store starts empty —
populate it via the ingestion endpoints below.

## Ingestion

Add examples that `/generate` can retrieve:

- `POST /ingest` — store a structured example directly:

  ```json
  {
    "doc_type": "ภายนอก",
    "fields": { "doc_type": "ภายนอก", "subject": "...", "body": ["..."] },
    "summary": "optional; derived from fields when omitted",
    "agency": "optional",
    "recipient_class": "optional"
  }
  ```

- `POST /ingest/ocr` — OCR a local image/PDF into an example via Typhoon OCR
  (cloud-only, uses `LLM_API_KEY`):

  ```json
  { "file_path": "/path/to/scan.pdf", "doc_type": "ภายนอก" }
  ```

  An LLM pass (the configured `LLM_BACKEND`) parses the OCR text into the
  document schema so the stored example is structured rather than a raw blob;
  on any failure it falls back to storing the raw text. The response reports
  `structured: true|false`. Pass `"structure": false` to skip the LLM pass.

Each ingest embeds the summary (when `EMBEDDING_BACKEND=remote`) and stores it
with the document. With the fake embedding backend the example is still stored
and surfaced through recency-based retrieval (`embedded: false` in the
response).

> Note: Typhoon does not expose a `/v1/embeddings` endpoint, so point `remote`
> at the local sidecar below or another OpenAI-compatible provider (e.g. OpenAI
> `text-embedding-3-small`).

### Local embeddings sidecar

`scripts/serve_embeddings.py` serves an OpenAI-compatible `/v1/embeddings`
endpoint from a local multilingual model (Thai-friendly, no cloud key). Requires
`sentence-transformers`:

```bash
python3 -m pip install -U sentence-transformers
./scripts/serve_embeddings.sh                 # downloads BAAI/bge-m3 on first run
```

Then point the API at it (e.g. in `.env`):

```bash
EMBEDDING_BACKEND=remote
EMBEDDING_BASE_URL=http://127.0.0.1:8090/v1
EMBEDDING_MODEL=BAAI/bge-m3
EMBEDDING_DIM=1024
```

Override the model with `EMBEDDING_LOCAL_MODEL` (e.g.
`intfloat/multilingual-e5-small` with `EMBEDDING_DIM=384` for a lighter
footprint).

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
