# Rust Port Plan

## MVP Scope

The Rust MVP should support local desktop workflows without requiring
PostgreSQL:

- Define and validate the four document types.
- Apply deterministic salutation and closing rules.
- Generate and edit document JSON through provider traits.
- Store examples, templates, and document metadata in SQLite.
- Retrieve similar examples through an HNSW vector index.
- Expose local HTTP endpoints compatible with the current Python API.

## Out Of Scope For The First Milestone

- Native `.docx` rendering parity with `docxtpl`.
- Full Tauri UI.
- Multi-user server deployment.
- PostgreSQL/pgvector compatibility.

## Recommended Milestones

1. `govdoc-domain` compiles and passes schema/rule tests.
2. `govdoc-usecases` supports fake LLM generation and editing tests.
3. `govdoc-storage` creates SQLite tables and stores/retrieves memory/template rows.
4. HNSW retrieval is wired behind an interface.
5. `govdoc-api` exposes `/health`, `/generate`, `/edit`, `/render`.
6. Tauri shell calls the local Rust commands/API.

## Current Milestone Status

This project is currently tracking the implementation plan below:

- M1: Set up Rust project, configuration, error handling, and logging.
- M2: Port data models plus parser/serializer behavior.
- M3: Port core logic with unit tests.
- M4: Port interfaces such as API or CLI.
- M5: Add integration tests and behavior parity coverage.
- M6: Cleanup, documentation, and release build validation.

Status as of 2026-06-06:

- M1: Complete for MVP. Workspace, crate layout, configuration examples, and
  error handling are present. Runtime tracing/log setup can still be improved.
- M2: Complete for MVP. Domain data models and Serde JSON behavior are ported.
- M3: Complete for MVP. Generation, edit, storage, deterministic rules, and HNSW
  logic have unit coverage.
- M4: Complete for MVP via the local Axum API. A separate CLI has not been added.
- M5: Complete for MVP. API behavior parity tests cover the main local
  contracts.
- M6: Complete for MVP. Docs/config cleanup is done, and local validation passes
  including release build.

## Remaining Work

- Replace fake LLM and embedding providers with real provider implementations.
- Add parity fixtures from the original Python project for request/response
  behavior comparisons.
- Exercise `.docx` rendering with real templates and sidecar output checks.
- Persist API state through `SQLITE_PATH` instead of the default in-memory store.
- Wire runtime tracing/logging instead of relying on startup `println!` output.
- Add CI for `fmt`, `clippy`, tests, and release builds.
- Build the Tauri desktop shell after core/API behavior is stable.
