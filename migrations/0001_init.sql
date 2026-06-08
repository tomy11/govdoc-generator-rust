CREATE TABLE IF NOT EXISTS gov_doc_memory (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    doc_type TEXT NOT NULL,
    summary_text TEXT NOT NULL,
    fields_json TEXT NOT NULL,
    recipient_class TEXT,
    agency TEXT,
    template_id TEXT,
    raw_text TEXT,
    embedding TEXT,
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

CREATE TABLE IF NOT EXISTS document (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    doc_type TEXT NOT NULL,
    title TEXT,
    doc_json TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS general_document (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    filename TEXT NOT NULL,
    file_path TEXT NOT NULL,
    page_count INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS general_document_page (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    document_id INTEGER NOT NULL,
    page_number INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    ocr_text TEXT,
    edited_text TEXT,
    error TEXT,
    page_image_path TEXT,
    ocr_raw_json TEXT,
    page_width INTEGER,
    page_height INTEGER,
    layout_warning TEXT,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(document_id, page_number),
    FOREIGN KEY(document_id) REFERENCES general_document(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS general_document_block (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    document_id INTEGER NOT NULL,
    page_number INTEGER NOT NULL,
    block_index INTEGER NOT NULL,
    block_type TEXT NOT NULL,
    text TEXT,
    bbox_json TEXT,
    style_json TEXT,
    image_path TEXT,
    embedding TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(document_id, page_number, block_index),
    FOREIGN KEY(document_id) REFERENCES general_document(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS general_document_revision (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    document_id INTEGER NOT NULL,
    page_number INTEGER NOT NULL,
    instruction TEXT NOT NULL,
    text TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY(document_id) REFERENCES general_document(id) ON DELETE CASCADE
);
