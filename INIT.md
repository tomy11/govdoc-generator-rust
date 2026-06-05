# Initial Rust Port Plan

## MVP Scope

The first Rust milestone should support local desktop workflows without requiring
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

