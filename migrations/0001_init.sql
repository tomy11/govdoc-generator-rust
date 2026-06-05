CREATE TABLE IF NOT EXISTS gov_doc_memory (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    doc_type TEXT NOT NULL,
    summary_text TEXT NOT NULL,
    fields_json TEXT NOT NULL,
    recipient_class TEXT,
    agency TEXT,
    template_id TEXT,
    raw_text TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS doc_template (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    doc_type TEXT NOT NULL,
    agency TEXT,
    name TEXT NOT NULL,
    file_path TEXT NOT NULL,
    is_default INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(doc_type, agency, name)
);

