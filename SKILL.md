---
name: govdoc-generator-rust
description: Use when working on the Rust/Tauri version of govdoc-generator, including domain parity, SQLite + HNSW local storage, document generation/edit orchestration, API compatibility, and desktop packaging for Windows or macOS.
---

# govdoc-generator-rust

## Purpose

Build a desktop-first Rust version of `govdoc-generator` while preserving the
Python project's clean architecture and API behavior.

## Source Parity

Use the Python project at `/Users/prachya.w/Documents/GitHub/govdoc-generator`
as the behavior source of truth.

Key source files:

- `src/domain/schemas.py` -> `crates/govdoc-domain`
- `src/domain/rules.py` -> deterministic salutation/closing lookup
- `src/usecases/generation.py` -> generator/critic orchestration
- `src/usecases/edit.py` -> targeted field editing
- `src/usecases/ingestion.py` -> PDF ingest summary + embedding
- `src/infrastructure/repository.py` -> SQLite metadata + vector retrieval plan
- `src/api/routes.py` -> HTTP route compatibility

## Architecture

Keep the Rust workspace layered:

- `govdoc-domain`: pure enums, structs, validation, deterministic rules
- `govdoc-usecases`: generation, editing, ingestion orchestration
- `govdoc-storage`: SQLite metadata and HNSW semantic search boundary
- `govdoc-api`: Axum route layer for local server or Tauri sidecar use

Do not let API, database, LLM, or desktop UI concerns leak into
`govdoc-domain`.

## Storage Plan

For desktop builds, prefer local-first storage:

- SQLite stores metadata, document fields, raw text, template records, and file paths.
- HNSW stores vector embeddings for similarity retrieval.
- Tantivy is optional later for full-text search, not required for the first MVP.

The HNSW index is a replacement for the Python version's `pgvector` similarity
search, not a replacement for all database responsibilities.

## Migration Order

1. Port domain schemas and deterministic rules.
2. Add tests that mirror Python `tests/test_schemas.py` and `tests/test_rules.py`.
3. Port generation prompt builders and generator/critic loop using traits.
4. Add fake LLM tests before real provider integration.
5. Add SQLite repositories.
6. Add HNSW vector index behind a trait.
7. Add Axum endpoints with Python-compatible JSON shapes.
8. Keep `.docx` rendering behind a boundary; use a Python sidecar first if native
   Rust rendering is not ready.
9. Add Tauri desktop shell after core/API behavior is stable.

## Validation

For every changed crate, run:

```bash
cargo fmt
cargo test
cargo clippy --all-targets -- -D warnings
```

If dependencies are not available locally, document that validation was blocked
by dependency download/network access.

