# Progress Log

## 2026-06-07 (later) — Tauri-readiness (CORS + /status)

### Context

There is no UI in this repo yet — only the HTTP API. Decided to target a Tauri
desktop app via the **sidecar HTTP** approach (bundle `govdoc-api`, webview
calls localhost) with a **hybrid** model runtime (cloud or local per backend
env). These changes prep the API for that.

### Work Completed

- Added a permissive `CorsLayer` so the desktop webview can call the localhost
  API cross-origin (verified: preflight returns `access-control-allow-origin: *`).
- Added `GET /status` reporting the active backends (llm/embedding/ocr) and
  readiness (e.g. cloud key present, persistence on), so the UI can show config
  and flag setup gaps. Added to the `/docs` index; unit tested.

### Validation

`cargo clippy -p govdoc-api --all-targets -- -D warnings` and the api test suite
(16 tests) pass.

### Next (Tauri build, not yet started)

- Scaffold `src-tauri` + a frontend; bundle `govdoc-api` as a sidecar and manage
  its lifecycle.
- Document persistence: a `documents` table + save/list/get endpoints so the UI
  can show generated-document history (generate currently does not persist its
  output).
- Optional generate progress streaming (SSE) for slow local models.
- Replace the Python sidecars for a distributable build (hybrid): keep cloud as
  the light default; for local, move toward a bundled llama.cpp binary, a
  Rust-native embedding, and native docx rendering to avoid bundling Python.

## 2026-06-07 (later) — Persistent vector index + CI

### Work Completed

- Added `PersistentVectorIndex` (`crates/govdoc-storage/src/vector_index.rs`): a
  per-doc-type cosine index serialized to `HNSW_INDEX_PATH`. It is loaded at
  startup, updated on ingest, and rebuilt from SQLite (`memory_vectors`) when the
  file is missing/corrupt. SQLite stays the source of truth; the index is a
  derived cache, so retrieval no longer reloads and rebuilds per query. Search is
  still brute-force cosine (the `HnswIndex` name is aspirational; a real HNSW can
  slot in behind it later). Unit tested for doc-type-scoped search and
  disk reload.
- Wired it through `AppState` (`open_index`), `SqliteMemoryRepository`, and the
  ingest path (writes SQLite then the index, no nested locks).
- Verified live: ingest with real bge-m3 embeddings writes the index file, and a
  restarted process loads it and serves retrieval (`examples: 1`).
- Added `.github/workflows/ci.yml`: fmt check, clippy (`-D warnings`), test, and
  release build on push/PR (ubuntu, stable, cargo cache). Added a CI badge.

### Validation

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D
warnings`, and `cargo test --workspace` (35 tests, 0 failures) all pass.

## 2026-06-07 (later) — Structure OCR text via LLM

### Work Completed

- Added `structure_document_from_text` (usecases) that runs an LLM pass to parse
  free OCR text into the per-doc-type schema and validates the result, reusing
  the same `complete_json` + schema-validation path as generation. Unit tested
  with a stub LLM for both the success and invalid-schema (fallback) cases.
- `POST /ingest/ocr` now structures the OCR text by default: on success it stores
  schema-shaped fields and derives the embedding summary from them; on any
  failure it falls back to the raw-text example. Response gained `structured`,
  and the request accepts `structure: false` to opt out.
- Docs: README ingestion note on the LLM structuring pass and opt-out.

### Validation

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D
warnings`, and `cargo test --workspace` (33 tests, 0 failures) all pass. The
structuring path uses the same mechanism already proven live in the local-MLX
`/generate` run, so no new live model run was required.

## 2026-06-07 (later) — Real local embeddings

### Work Completed

- Confirmed Typhoon has no `/v1/embeddings` endpoint (404; chat is 401), so the
  already-wired `TyphoonEmbeddingProvider` needs a different OpenAI-compatible
  source. No Rust changes were required — only an endpoint to point at.
- Added a local embeddings sidecar: `scripts/serve_embeddings.py` serves
  OpenAI-compatible `POST /v1/embeddings` + `GET /v1/models` from a
  sentence-transformers model (default `BAAI/bge-m3`, 1024-dim, Thai-friendly,
  no prefixes). `scripts/serve_embeddings.sh` is the launcher.
- Verified end to end: bge-m3 cosine ranks a near Thai sentence (0.708) above a
  far one (0.554); via the Rust API, `/ingest` returns `embedded: true` and
  `/generate` retrieval reports `examples: 1` over the real vectors.
- Docs: README "Local embeddings sidecar" section; `.env.example` local config
  and the Typhoon-has-no-embeddings note.

### Validation

`cargo fmt --all --check` and `cargo test --workspace` (32 tests) still pass; no
Rust source changed this round.

## 2026-06-07 (later) — Persistence, dotenv, docs index

### Work Completed

- Wired `SQLITE_PATH`: `AppState` now opens a file-backed store (creating parent
  dirs) when the env var is set, falling back to in-memory otherwise. Ingested
  examples and templates survive a restart. Verified live: ingest -> kill ->
  restart -> `/generate` retrieval still reports `examples: 1`. Added a storage
  reopen-persistence unit test and a defensive `ALTER TABLE ... ADD COLUMN
  embedding` for pre-existing databases.
- Load `.env` at startup via `dotenvy` so secrets (e.g. `LLM_API_KEY`) live in
  the gitignored `.env` instead of the command line. Shell env still overrides.
- Added a lightweight endpoint index at `GET /` and `GET /docs` (the Axum port
  has no Swagger UI like the FastAPI original).

### Validation

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D
warnings`, and `cargo test --workspace` (32 tests, 0 failures) all pass.

## 2026-06-07 (later) — Ingestion pipeline

### Work Completed

- Added `TyphoonOcrProvider` (`crates/govdoc-api/src/providers.rs`) calling the
  Typhoon `/v1/ocr` multipart endpoint, with a pure `parse_ocr_response` that
  joins page `natural_text` (or plain content) and surfaces page errors. Unit
  tested. Enabled the `multipart` feature on reqwest.
- Added ingestion endpoints to the API:
  - `POST /ingest` — store a structured example document directly.
  - `POST /ingest/ocr` — OCR a local file into an example via Typhoon OCR.
  Both embed the summary (when `EMBEDDING_BACKEND=remote`) and write to
  `gov_doc_memory`. `AppState::embed_for_storage` returns `None` on the fake
  backend so retrieval falls back to recency instead of indexing zero vectors.
- Closed the loop: integration test ingests an example and confirms `/generate`
  retrieval reports `examples: 1`.
- Made `DocType` `Copy` (fieldless enum) to pass it by value cleanly.
- Docs: README ingestion section + endpoints; `.env.example` `OCR_*` keys.

### Validation

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D
warnings`, and `cargo test --workspace` (30 tests, 0 failures) all pass.

### Follow-ups

- OCR'd text is stored as a raw example (`fields.content`). An optional LLM pass
  to structure it into the per-doc-type schema would make examples stronger.
- `/ingest/ocr` reads a local path (desktop-first); a multipart upload variant
  can be added if needed for remote callers.

## 2026-06-07

### Work Completed

- Added a real LLM provider, `TyphoonProvider`
  (`crates/govdoc-api/src/providers.rs`), that calls any OpenAI-compatible chat
  endpoint. Two backends share it: `typhoon-local` (`mlx_lm.server` + Typhoon
  MLX) and `typhoon-cloud` (hosted `https://api.opentyphoon.ai/v1`, key-based).
- `TyphoonConfig::local()` / `cloud()` provide per-backend defaults; all
  `LLM_*` env vars still override. Added optional `LLM_TOP_P`. `typhoon-cloud`
  fails fast if `LLM_API_KEY` is missing.
- Added a real embedding provider, `TyphoonEmbeddingProvider`, calling an
  OpenAI-compatible `/v1/embeddings` endpoint. Selected by `EMBEDDING_BACKEND`
  (`fake` default, `remote`); config reuses `LLM_BASE_URL`/`LLM_API_KEY`.
- Made retrieval actually use vectors: added `SqliteMemoryRepository`
  (`crates/govdoc-api/src/memory.rs`) that builds an in-memory cosine index from
  stored embeddings per doc type, returns nearest examples, and falls back to
  recency when no embeddings exist. Replaced `EmptyMemoryRepository` in
  `/generate` with it.
- Storage: added an `embedding` column to `gov_doc_memory`; `store_memory` now
  takes an optional vector; added `recent_memory_fields`, `memory_embeddings`,
  and `memory_fields_by_ids`. Covered by unit tests.
- NOTE: a hosted Typhoon embeddings endpoint could not be confirmed available;
  the provider targets the OpenAI-compatible contract so it works with Typhoon
  if/when shipped, or any compatible service now.
- Verified the local MLX path end to end on Apple Silicon: converted
  `typhoon-ai/typhoon2.5-qwen3-4b` to 4-bit MLX (~2.1 GB) via `mlx_lm convert`,
  served it with `mlx_lm.server`, and `POST /generate` (LLM_BACKEND=typhoon-local)
  returned schema-valid Thai `ExternalDoc` JSON with deterministic
  salutation/closing applied.
- Fixes found during that run: scripts called `python` (this machine only has
  `python3`) -> now use a `PYTHON` var defaulting to `python3`; no prebuilt
  `-mlx` repo exists -> `download_model.sh` now converts the base model; the
  served model id is the base repo name, so `TyphoonConfig::local()` default
  `LLM_MODEL` is now `typhoon-ai/typhoon2.5-qwen3-4b` (must match
  `/v1/models`).
- `complete_json` instructs the model to emit JSON and defensively extracts it
  (handles raw objects, ```json fences, and surrounding prose). Covered by unit
  tests.
- Added `LlmBackend` selection in `AppState`, chosen from `LLM_BACKEND`
  (`fake` default, `typhoon-local`). `/generate` and `/edit` now build the
  provider from app state instead of hardcoding the fake.
- Added `maybe_start_local_llm` + `wait_until_ready` so the API can optionally
  spawn the model sidecar (`LLM_AUTO_SERVE=1`) and block until it is ready.
- Added `scripts/download_model.sh` and `scripts/serve_llm.sh`; ignored
  `/models` in `.gitignore`; expanded `.env.example` with `LLM_*` settings.
- Documented the local Typhoon MLX flow in `README.md`.

### Validation

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D
warnings`, and `cargo test --workspace` (24 tests, 0 failures) all pass. The
default `fake` backend keeps the API behavior parity tests unchanged.

### Notes / Follow-ups

- The memory store starts empty, so vector retrieval returns nothing until
  examples are ingested. An ingestion path (e.g. Typhoon OCR -> markdown ->
  `store_memory` with embedding) is the next piece of work.
- The `typhoon2.5-qwen3-4b-mlx` HF repo may need to be created via
  `mlx_lm.convert` from the base `typhoon-ai/typhoon2.5-qwen3-4b` if no
  prebuilt MLX repo exists (noted in `download_model.sh`).
- Auto-spawned sidecar is a dev convenience and is not killed on API exit.

## 2026-06-06

### Milestone Status

- M1: Complete for MVP.
- M2: Complete for MVP.
- M3: Complete for MVP.
- M4: Complete for MVP.
- M5: Complete for MVP.
- M6: Complete for MVP.

### Work Completed

- Confirmed the repository is a Rust workspace with four crates:
  `govdoc-domain`, `govdoc-usecases`, `govdoc-storage`, and `govdoc-api`.
- Mapped existing implementation to the M1-M6 plan.
- Added API behavior parity integration tests in
  `crates/govdoc-api/tests/api_behavior_parity.rs`.
- Covered these API behavior contracts:
  - `GET /health`
  - `POST /generate` for all four document types
  - deterministic salutation and closing rules after generation
  - `POST /edit` default target field behavior
  - template default resolution with agency fallback
  - `POST /render` document validation before sidecar configuration
- Renamed the integration test file from milestone-specific naming to the
  standard behavior-oriented name `api_behavior_parity.rs`.
- Cleaned `.env.example` by removing an absolute local path and grouping config
  sections.
- Expanded `README.md` with workspace overview, quick start, API surface,
  render sidecar notes, validation commands, and milestone status.
- Updated this plan and progress log so future work does not depend only on git
  history.

### Validation

The following commands passed:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
```

`cargo test --workspace` passed with 20 tests and 0 failures.

### Remaining Work

- Implement real LLM and embedding providers.
- Add Python parity fixtures for request/response comparisons.
- Validate `.docx` rendering with real templates and sidecar output.
- Persist API state via `SQLITE_PATH` instead of using in-memory defaults.
- Wire runtime tracing/logging.
- Add CI validation workflow.
- Start Tauri shell/desktop packaging when API behavior is stable.
