# govdoc-generator-rust

Rust workspace for a desktop-first Thai government document generator.

This project is initialized as a Rust port of `govdoc-generator`, with a
local-first storage plan:

- SQLite for metadata, templates, raw text, and document fields
- HNSW vector index for semantic retrieval
- Optional Tantivy full-text index later if keyword search becomes important

The first milestone is API/domain parity with the Python implementation before
building a Tauri desktop shell.

