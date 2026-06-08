use std::path::Path;

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

pub struct SqliteStore {
    conn: Connection,
}

pub struct NewMemoryRecord<'a> {
    pub doc_type: &'a str,
    pub summary_text: &'a str,
    pub fields: &'a Value,
    pub recipient_class: Option<&'a str>,
    pub agency: Option<&'a str>,
    pub template_id: Option<&'a str>,
    pub raw_text: Option<&'a str>,
    /// Optional dense vector for semantic retrieval, stored as a JSON array.
    pub embedding: Option<&'a [f32]>,
}

pub struct NewTemplateRecord<'a> {
    pub doc_type: &'a str,
    pub name: &'a str,
    pub file_path: &'a str,
    pub agency: Option<&'a str>,
    pub is_default: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TemplateRecord {
    pub id: i64,
    pub doc_type: String,
    pub agency: Option<String>,
    pub name: String,
    pub file_path: String,
    pub is_default: bool,
}

/// A saved generated document (full content).
#[derive(Clone, Debug, PartialEq)]
pub struct DocumentRecord {
    pub id: i64,
    pub doc_type: String,
    pub title: Option<String>,
    pub doc_json: Value,
    pub created_at: String,
}

/// Lightweight row for listing saved documents.
#[derive(Clone, Debug, PartialEq)]
pub struct DocumentSummary {
    pub id: i64,
    pub doc_type: String,
    pub title: Option<String>,
    pub created_at: String,
}

pub struct NewGeneralDocument<'a> {
    pub filename: &'a str,
    pub file_path: &'a str,
    pub page_count: i64,
}

pub struct NewGeneralDocumentBlock<'a> {
    pub document_id: i64,
    pub page_number: i64,
    pub block_index: i64,
    pub block_type: &'a str,
    pub text: Option<&'a str>,
    pub bbox_json: Option<&'a str>,
    pub style_json: Option<&'a str>,
    pub image_path: Option<&'a str>,
    pub embedding: Option<&'a [f32]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GeneralDocumentSummary {
    pub id: i64,
    pub filename: String,
    pub file_path: String,
    pub page_count: i64,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GeneralDocumentPage {
    pub id: i64,
    pub document_id: i64,
    pub page_number: i64,
    pub status: String,
    pub ocr_text: Option<String>,
    pub edited_text: Option<String>,
    pub error: Option<String>,
    pub page_image_path: Option<String>,
    pub ocr_raw_json: Option<String>,
    pub page_width: Option<i64>,
    pub page_height: Option<i64>,
    pub layout_warning: Option<String>,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GeneralDocumentBlock {
    pub id: i64,
    pub document_id: i64,
    pub page_number: i64,
    pub block_index: i64,
    pub block_type: String,
    pub text: Option<String>,
    pub bbox_json: Option<String>,
    pub style_json: Option<String>,
    pub image_path: Option<String>,
    pub embedding: Option<Vec<f32>>,
    pub created_at: String,
    pub updated_at: String,
}

impl SqliteStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
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
            "#,
        )?;
        // Backfill the embedding column for databases created before it existed.
        // Errors with "duplicate column" when already present, which we ignore.
        let _ = self
            .conn
            .execute("ALTER TABLE gov_doc_memory ADD COLUMN embedding TEXT", []);
        let _ = self.conn.execute(
            "ALTER TABLE general_document_page ADD COLUMN page_image_path TEXT",
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE general_document_page ADD COLUMN ocr_raw_json TEXT",
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE general_document_page ADD COLUMN page_width INTEGER",
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE general_document_page ADD COLUMN page_height INTEGER",
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE general_document_page ADD COLUMN layout_warning TEXT",
            [],
        );
        Ok(())
    }

    pub fn store_memory(&self, record: NewMemoryRecord<'_>) -> Result<i64> {
        let embedding_json = record.embedding.map(serde_json::to_string).transpose()?;
        self.conn.execute(
            r#"
            INSERT INTO gov_doc_memory (
                doc_type, summary_text, fields_json, recipient_class, agency, template_id,
                raw_text, embedding
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
            params![
                record.doc_type,
                record.summary_text,
                serde_json::to_string(record.fields)?,
                record.recipient_class,
                record.agency,
                record.template_id,
                record.raw_text,
                embedding_json
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_memory_fields(&self, id: i64) -> Result<Option<Value>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT fields_json FROM gov_doc_memory WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;

        json.map(|text| serde_json::from_str(&text).map_err(Into::into))
            .transpose()
    }

    /// Most recent memory `fields_json` values, optionally filtered by doc type.
    /// Used as the non-vector retrieval fallback.
    pub fn recent_memory_fields(&self, doc_type: Option<&str>, limit: usize) -> Result<Vec<Value>> {
        let limit = limit as i64;
        let rows: Vec<String> = match doc_type {
            Some(doc_type) => {
                let mut stmt = self.conn.prepare(
                    "SELECT fields_json FROM gov_doc_memory WHERE doc_type = ?1 ORDER BY id DESC LIMIT ?2",
                )?;
                let mapped = stmt.query_map(params![doc_type, limit], |row| row.get(0))?;
                mapped.collect::<rusqlite::Result<_>>()?
            }
            None => {
                let mut stmt = self
                    .conn
                    .prepare("SELECT fields_json FROM gov_doc_memory ORDER BY id DESC LIMIT ?1")?;
                let mapped = stmt.query_map(params![limit], |row| row.get(0))?;
                mapped.collect::<rusqlite::Result<_>>()?
            }
        };
        rows.into_iter()
            .map(|text| serde_json::from_str(&text).map_err(Into::into))
            .collect()
    }

    /// All stored `(id, embedding)` pairs for a doc type, skipping rows without
    /// an embedding. Used to build the in-memory vector index.
    pub fn memory_embeddings(&self, doc_type: Option<&str>) -> Result<Vec<(i64, Vec<f32>)>> {
        let raw: Vec<(i64, String)> = match doc_type {
            Some(doc_type) => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, embedding FROM gov_doc_memory WHERE embedding IS NOT NULL AND doc_type = ?1",
                )?;
                let mapped =
                    stmt.query_map(params![doc_type], |row| Ok((row.get(0)?, row.get(1)?)))?;
                mapped.collect::<rusqlite::Result<_>>()?
            }
            None => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, embedding FROM gov_doc_memory WHERE embedding IS NOT NULL",
                )?;
                let mapped = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
                mapped.collect::<rusqlite::Result<_>>()?
            }
        };
        raw.into_iter()
            .map(|(id, json)| Ok((id, serde_json::from_str::<Vec<f32>>(&json)?)))
            .collect()
    }

    /// All stored `(id, doc_type, embedding)` triples, for rebuilding the
    /// persistent vector index from the source-of-truth database.
    pub fn memory_vectors(&self) -> Result<Vec<(i64, String, Vec<f32>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, doc_type, embedding FROM gov_doc_memory WHERE embedding IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, doc_type, json) = row?;
            out.push((id, doc_type, serde_json::from_str::<Vec<f32>>(&json)?));
        }
        Ok(out)
    }

    /// Fetch `fields_json` for the given ids, preserving the input order.
    pub fn memory_fields_by_ids(&self, ids: &[i64]) -> Result<Vec<Value>> {
        let mut fields = Vec::with_capacity(ids.len());
        for &id in ids {
            if let Some(value) = self.get_memory_fields(id)? {
                fields.push(value);
            }
        }
        Ok(fields)
    }

    /// Persist a generated document and return its id.
    pub fn save_document(
        &self,
        doc_type: &str,
        title: Option<&str>,
        doc_json: &Value,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO document (doc_type, title, doc_json) VALUES (?1, ?2, ?3)",
            params![doc_type, title, serde_json::to_string(doc_json)?],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// List saved documents (newest first), optionally filtered by doc type.
    pub fn list_documents(&self, doc_type: Option<&str>) -> Result<Vec<DocumentSummary>> {
        let summary = |row: &rusqlite::Row<'_>| {
            Ok(DocumentSummary {
                id: row.get(0)?,
                doc_type: row.get(1)?,
                title: row.get(2)?,
                created_at: row.get(3)?,
            })
        };
        let mut documents = Vec::new();
        match doc_type {
            Some(doc_type) => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, doc_type, title, created_at FROM document WHERE doc_type = ?1 ORDER BY id DESC",
                )?;
                for row in stmt.query_map(params![doc_type], summary)? {
                    documents.push(row?);
                }
            }
            None => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, doc_type, title, created_at FROM document ORDER BY id DESC",
                )?;
                for row in stmt.query_map([], summary)? {
                    documents.push(row?);
                }
            }
        }
        Ok(documents)
    }

    /// Fetch one saved document with its full content.
    pub fn get_document(&self, id: i64) -> Result<Option<DocumentRecord>> {
        let row: Option<(i64, String, Option<String>, String, String)> = self
            .conn
            .query_row(
                "SELECT id, doc_type, title, doc_json, created_at FROM document WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?;

        row.map(|(id, doc_type, title, json, created_at)| {
            Ok(DocumentRecord {
                id,
                doc_type,
                title,
                doc_json: serde_json::from_str(&json)?,
                created_at,
            })
        })
        .transpose()
    }

    /// Replace a saved document. Returns whether a row was updated.
    pub fn update_document(
        &self,
        id: i64,
        doc_type: &str,
        title: Option<&str>,
        doc_json: &Value,
    ) -> Result<bool> {
        let affected = self.conn.execute(
            "UPDATE document SET doc_type = ?1, title = ?2, doc_json = ?3 WHERE id = ?4",
            params![doc_type, title, serde_json::to_string(doc_json)?, id],
        )?;
        Ok(affected > 0)
    }

    /// Delete a saved document. Returns whether a row was removed.
    pub fn delete_document(&self, id: i64) -> Result<bool> {
        let affected = self
            .conn
            .execute("DELETE FROM document WHERE id = ?1", params![id])?;
        Ok(affected > 0)
    }

    pub fn create_template(&self, record: NewTemplateRecord<'_>) -> Result<i64> {
        if record.is_default {
            self.unset_default(record.doc_type, record.agency)?;
        }
        self.conn.execute(
            r#"
            INSERT INTO doc_template (doc_type, agency, name, file_path, is_default)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![
                record.doc_type,
                record.agency,
                record.name,
                record.file_path,
                record.is_default as i64
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_template(&self, id: i64) -> Result<Option<TemplateRecord>> {
        self.conn
            .query_row(
                "SELECT id, doc_type, agency, name, file_path, is_default FROM doc_template WHERE id = ?1",
                params![id],
                template_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_templates(
        &self,
        doc_type: Option<&str>,
        agency: Option<&str>,
    ) -> Result<Vec<TemplateRecord>> {
        let mut templates = Vec::new();
        match (doc_type, agency) {
            (Some(doc_type), Some(agency)) => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, doc_type, agency, name, file_path, is_default FROM doc_template WHERE doc_type = ?1 AND agency = ?2 ORDER BY id DESC",
                )?;
                let rows = stmt.query_map(params![doc_type, agency], template_from_row)?;
                for row in rows {
                    templates.push(row?);
                }
            }
            (Some(doc_type), None) => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, doc_type, agency, name, file_path, is_default FROM doc_template WHERE doc_type = ?1 ORDER BY id DESC",
                )?;
                let rows = stmt.query_map(params![doc_type], template_from_row)?;
                for row in rows {
                    templates.push(row?);
                }
            }
            (None, Some(agency)) => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, doc_type, agency, name, file_path, is_default FROM doc_template WHERE agency = ?1 ORDER BY id DESC",
                )?;
                let rows = stmt.query_map(params![agency], template_from_row)?;
                for row in rows {
                    templates.push(row?);
                }
            }
            (None, None) => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, doc_type, agency, name, file_path, is_default FROM doc_template ORDER BY id DESC",
                )?;
                let rows = stmt.query_map([], template_from_row)?;
                for row in rows {
                    templates.push(row?);
                }
            }
        }
        Ok(templates)
    }

    pub fn resolve_default(
        &self,
        doc_type: &str,
        agency: Option<&str>,
    ) -> Result<Option<TemplateRecord>> {
        if let Some(agency) = agency {
            let agency_template = self.find_default(doc_type, Some(agency))?;
            if agency_template.is_some() {
                return Ok(agency_template);
            }
        }
        self.find_default(doc_type, None)
    }

    fn find_default(&self, doc_type: &str, agency: Option<&str>) -> Result<Option<TemplateRecord>> {
        if let Some(agency) = agency {
            return self
                .conn
                .query_row(
                    "SELECT id, doc_type, agency, name, file_path, is_default FROM doc_template WHERE doc_type = ?1 AND agency = ?2 AND is_default = 1",
                    params![doc_type, agency],
                    template_from_row,
                )
                .optional()
                .map_err(Into::into);
        }

        self.conn
            .query_row(
                "SELECT id, doc_type, agency, name, file_path, is_default FROM doc_template WHERE doc_type = ?1 AND agency IS NULL AND is_default = 1",
                params![doc_type],
                template_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    fn unset_default(&self, doc_type: &str, agency: Option<&str>) -> Result<()> {
        match agency {
            Some(agency) => self.conn.execute(
                "UPDATE doc_template SET is_default = 0 WHERE doc_type = ?1 AND agency = ?2",
                params![doc_type, agency],
            )?,
            None => self.conn.execute(
                "UPDATE doc_template SET is_default = 0 WHERE doc_type = ?1 AND agency IS NULL",
                params![doc_type],
            )?,
        };
        Ok(())
    }

    pub fn create_general_document(&self, record: NewGeneralDocument<'_>) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO general_document (filename, file_path, page_count) VALUES (?1, ?2, ?3)",
            params![record.filename, record.file_path, record.page_count],
        )?;
        let id = self.conn.last_insert_rowid();
        for page in 1..=record.page_count {
            self.conn.execute(
                "INSERT INTO general_document_page (document_id, page_number) VALUES (?1, ?2)",
                params![id, page],
            )?;
        }
        Ok(id)
    }

    pub fn list_general_documents(&self) -> Result<Vec<GeneralDocumentSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, filename, file_path, page_count, status, created_at, updated_at FROM general_document ORDER BY id DESC",
        )?;
        let rows = stmt.query_map([], general_document_from_row)?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    pub fn get_general_document(&self, id: i64) -> Result<Option<GeneralDocumentSummary>> {
        self.conn
            .query_row(
                "SELECT id, filename, file_path, page_count, status, created_at, updated_at FROM general_document WHERE id = ?1",
                params![id],
                general_document_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn update_general_document_file_path(&self, id: i64, file_path: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE general_document SET file_path = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
            params![file_path, id],
        )?;
        Ok(())
    }

    pub fn list_general_document_pages(
        &self,
        document_id: i64,
    ) -> Result<Vec<GeneralDocumentPage>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, document_id, page_number, status, ocr_text, edited_text, error, page_image_path, ocr_raw_json, page_width, page_height, layout_warning, updated_at FROM general_document_page WHERE document_id = ?1 ORDER BY page_number",
        )?;
        let rows = stmt.query_map(params![document_id], general_page_from_row)?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    pub fn get_general_document_page(
        &self,
        document_id: i64,
        page_number: i64,
    ) -> Result<Option<GeneralDocumentPage>> {
        self.conn
            .query_row(
                "SELECT id, document_id, page_number, status, ocr_text, edited_text, error, page_image_path, ocr_raw_json, page_width, page_height, layout_warning, updated_at FROM general_document_page WHERE document_id = ?1 AND page_number = ?2",
                params![document_id, page_number],
                general_page_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn update_general_page_asset(
        &self,
        document_id: i64,
        page_number: i64,
        page_image_path: Option<&str>,
        page_width: Option<i64>,
        page_height: Option<i64>,
        layout_warning: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE general_document_page SET page_image_path = ?1, page_width = ?2, page_height = ?3, layout_warning = ?4, updated_at = CURRENT_TIMESTAMP WHERE document_id = ?5 AND page_number = ?6",
            params![
                page_image_path,
                page_width,
                page_height,
                layout_warning,
                document_id,
                page_number
            ],
        )?;
        Ok(())
    }

    pub fn update_general_page_ocr(
        &self,
        document_id: i64,
        page_number: i64,
        status: &str,
        text: Option<&str>,
        raw_json: Option<&Value>,
        error: Option<&str>,
    ) -> Result<()> {
        let raw_json = raw_json.map(serde_json::to_string).transpose()?;
        self.conn.execute(
            "UPDATE general_document_page SET status = ?1, ocr_text = ?2, ocr_raw_json = ?3, error = ?4, updated_at = CURRENT_TIMESTAMP WHERE document_id = ?5 AND page_number = ?6",
            params![status, text, raw_json, error, document_id, page_number],
        )?;
        self.update_general_document_status(document_id)?;
        Ok(())
    }

    pub fn replace_general_page_blocks(
        &mut self,
        document_id: i64,
        page_number: i64,
        blocks: &[NewGeneralDocumentBlock<'_>],
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM general_document_block WHERE document_id = ?1 AND page_number = ?2",
            params![document_id, page_number],
        )?;
        for block in blocks {
            let embedding_json = block.embedding.map(serde_json::to_string).transpose()?;
            tx.execute(
                r#"
                INSERT INTO general_document_block (
                    document_id, page_number, block_index, block_type, text,
                    bbox_json, style_json, image_path, embedding
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                "#,
                params![
                    block.document_id,
                    block.page_number,
                    block.block_index,
                    block.block_type,
                    block.text,
                    block.bbox_json,
                    block.style_json,
                    block.image_path,
                    embedding_json,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_general_document_blocks(
        &self,
        document_id: i64,
        page_number: Option<i64>,
    ) -> Result<Vec<GeneralDocumentBlock>> {
        let mut blocks = Vec::new();
        match page_number {
            Some(page_number) => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, document_id, page_number, block_index, block_type, text, bbox_json, style_json, image_path, embedding, created_at, updated_at FROM general_document_block WHERE document_id = ?1 AND page_number = ?2 ORDER BY page_number, block_index",
                )?;
                let rows =
                    stmt.query_map(params![document_id, page_number], general_block_from_row)?;
                for row in rows {
                    blocks.push(row?);
                }
            }
            None => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, document_id, page_number, block_index, block_type, text, bbox_json, style_json, image_path, embedding, created_at, updated_at FROM general_document_block WHERE document_id = ?1 ORDER BY page_number, block_index",
                )?;
                let rows = stmt.query_map(params![document_id], general_block_from_row)?;
                for row in rows {
                    blocks.push(row?);
                }
            }
        }
        Ok(blocks)
    }

    pub fn save_general_revision(
        &self,
        document_id: i64,
        page_number: i64,
        instruction: &str,
        text: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO general_document_revision (document_id, page_number, instruction, text) VALUES (?1, ?2, ?3, ?4)",
            params![document_id, page_number, instruction, text],
        )?;
        self.conn.execute(
            "UPDATE general_document_page SET edited_text = ?1, updated_at = CURRENT_TIMESTAMP WHERE document_id = ?2 AND page_number = ?3",
            params![text, document_id, page_number],
        )?;
        self.update_general_document_status(document_id)?;
        Ok(())
    }

    fn update_general_document_status(&self, document_id: i64) -> Result<()> {
        let total: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM general_document_page WHERE document_id = ?1",
            params![document_id],
            |row| row.get(0),
        )?;
        let succeeded: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM general_document_page WHERE document_id = ?1 AND status = 'succeeded'",
            params![document_id],
            |row| row.get(0),
        )?;
        let failed: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM general_document_page WHERE document_id = ?1 AND status = 'failed'",
            params![document_id],
            |row| row.get(0),
        )?;
        let status = if total > 0 && succeeded == total {
            "succeeded"
        } else if failed > 0 {
            "partial"
        } else if succeeded > 0 {
            "running"
        } else {
            "pending"
        };
        self.conn.execute(
            "UPDATE general_document SET status = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
            params![status, document_id],
        )?;
        Ok(())
    }
}

fn template_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TemplateRecord> {
    Ok(TemplateRecord {
        id: row.get(0)?,
        doc_type: row.get(1)?,
        agency: row.get(2)?,
        name: row.get(3)?,
        file_path: row.get(4)?,
        is_default: row.get::<_, i64>(5)? == 1,
    })
}

fn general_document_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GeneralDocumentSummary> {
    Ok(GeneralDocumentSummary {
        id: row.get(0)?,
        filename: row.get(1)?,
        file_path: row.get(2)?,
        page_count: row.get(3)?,
        status: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn general_page_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GeneralDocumentPage> {
    Ok(GeneralDocumentPage {
        id: row.get(0)?,
        document_id: row.get(1)?,
        page_number: row.get(2)?,
        status: row.get(3)?,
        ocr_text: row.get(4)?,
        edited_text: row.get(5)?,
        error: row.get(6)?,
        page_image_path: row.get(7)?,
        ocr_raw_json: row.get(8)?,
        page_width: row.get(9)?,
        page_height: row.get(10)?,
        layout_warning: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

fn general_block_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GeneralDocumentBlock> {
    let embedding_json: Option<String> = row.get(9)?;
    let embedding = embedding_json
        .as_deref()
        .and_then(|json| serde_json::from_str::<Vec<f32>>(json).ok());
    Ok(GeneralDocumentBlock {
        id: row.get(0)?,
        document_id: row.get(1)?,
        page_number: row.get(2)?,
        block_index: row.get(3)?,
        block_type: row.get(4)?,
        text: row.get(5)?,
        bbox_json: row.get(6)?,
        style_json: row.get(7)?,
        image_path: row.get(8)?,
        embedding,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_memory_fields_as_json() {
        let store = SqliteStore::open_memory().unwrap();
        let id = store
            .store_memory(NewMemoryRecord {
                doc_type: "ภายนอก",
                summary_text: "summary",
                fields: &serde_json::json!({ "subject": "ทดสอบ" }),
                recipient_class: None,
                agency: None,
                template_id: None,
                raw_text: None,
                embedding: None,
            })
            .unwrap();

        let fields = store.get_memory_fields(id).unwrap().unwrap();

        assert_eq!(fields["subject"], "ทดสอบ");
    }

    #[test]
    fn stores_and_reads_back_embeddings_per_doc_type() {
        let store = SqliteStore::open_memory().unwrap();
        store
            .store_memory(NewMemoryRecord {
                doc_type: "ภายนอก",
                summary_text: "a",
                fields: &serde_json::json!({ "subject": "ก" }),
                recipient_class: None,
                agency: None,
                template_id: None,
                raw_text: None,
                embedding: Some(&[1.0, 0.0, 0.0]),
            })
            .unwrap();
        // Different doc type, and a row without an embedding: both excluded.
        store
            .store_memory(NewMemoryRecord {
                doc_type: "คำสั่ง",
                summary_text: "b",
                fields: &serde_json::json!({ "title": "ข" }),
                recipient_class: None,
                agency: None,
                template_id: None,
                raw_text: None,
                embedding: Some(&[0.0, 1.0, 0.0]),
            })
            .unwrap();
        store
            .store_memory(NewMemoryRecord {
                doc_type: "ภายนอก",
                summary_text: "c",
                fields: &serde_json::json!({ "subject": "ค" }),
                recipient_class: None,
                agency: None,
                template_id: None,
                raw_text: None,
                embedding: None,
            })
            .unwrap();

        let embeddings = store.memory_embeddings(Some("ภายนอก")).unwrap();
        assert_eq!(embeddings.len(), 1);
        assert_eq!(embeddings[0].1, vec![1.0, 0.0, 0.0]);

        let recent = store.recent_memory_fields(Some("ภายนอก"), 5).unwrap();
        assert_eq!(recent.len(), 2);
    }

    #[test]
    fn persists_memory_across_reopen() {
        let path = std::env::temp_dir().join("govdoc-persist-test-9f3a2.sqlite3");
        let _ = std::fs::remove_file(&path);

        {
            let store = SqliteStore::open(&path).unwrap();
            store
                .store_memory(NewMemoryRecord {
                    doc_type: "ภายนอก",
                    summary_text: "persisted",
                    fields: &serde_json::json!({ "subject": "ถาวร" }),
                    recipient_class: None,
                    agency: None,
                    template_id: None,
                    raw_text: None,
                    embedding: Some(&[0.5, 0.5]),
                })
                .unwrap();
        }

        let store = SqliteStore::open(&path).unwrap();
        let recent = store.recent_memory_fields(Some("ภายนอก"), 5).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0]["subject"], "ถาวร");
        assert_eq!(store.memory_embeddings(Some("ภายนอก")).unwrap().len(), 1);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn resolves_agency_default_before_central_default() {
        let store = SqliteStore::open_memory().unwrap();
        store
            .create_template(NewTemplateRecord {
                doc_type: "ภายนอก",
                name: "กลาง",
                file_path: "templates/central.docx",
                agency: None,
                is_default: true,
            })
            .unwrap();
        store
            .create_template(NewTemplateRecord {
                doc_type: "ภายนอก",
                name: "หน่วยงาน",
                file_path: "templates/agency.docx",
                agency: Some("โรงเรียน"),
                is_default: true,
            })
            .unwrap();

        let template = store
            .resolve_default("ภายนอก", Some("โรงเรียน"))
            .unwrap()
            .unwrap();

        assert_eq!(template.file_path, "templates/agency.docx");
    }

    #[test]
    fn saves_lists_gets_and_deletes_documents() {
        let store = SqliteStore::open_memory().unwrap();
        let id = store
            .save_document(
                "ภายนอก",
                Some("ขอเชิญประชุม"),
                &serde_json::json!({ "subject": "ขอเชิญประชุม", "body": ["..."] }),
            )
            .unwrap();
        store
            .save_document("คำสั่ง", None, &serde_json::json!({ "title": "คำสั่งที่ 1" }))
            .unwrap();

        let all = store.list_documents(None).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].doc_type, "คำสั่ง"); // newest first

        let external = store.list_documents(Some("ภายนอก")).unwrap();
        assert_eq!(external.len(), 1);
        assert_eq!(external[0].title.as_deref(), Some("ขอเชิญประชุม"));

        let doc = store.get_document(id).unwrap().unwrap();
        assert_eq!(doc.doc_json["subject"], "ขอเชิญประชุม");

        assert!(store
            .update_document(
                id,
                "ภายนอก",
                Some("ขอเชิญประชุมฉบับแก้ไข"),
                &serde_json::json!({ "subject": "ขอเชิญประชุมฉบับแก้ไข" }),
            )
            .unwrap());
        let updated = store.get_document(id).unwrap().unwrap();
        assert_eq!(updated.title.as_deref(), Some("ขอเชิญประชุมฉบับแก้ไข"));
        assert_eq!(updated.doc_json["subject"], "ขอเชิญประชุมฉบับแก้ไข");
        assert!(!store
            .update_document(
                999,
                "ภายนอก",
                Some("ไม่มี"),
                &serde_json::json!({ "subject": "ไม่มี" }),
            )
            .unwrap());

        assert!(store.delete_document(id).unwrap());
        assert!(!store.delete_document(id).unwrap()); // already gone
        assert_eq!(store.list_documents(None).unwrap().len(), 1);
    }

    #[test]
    fn stores_general_documents_pages_and_revisions() {
        let mut store = SqliteStore::open_memory().unwrap();
        let id = store
            .create_general_document(NewGeneralDocument {
                filename: "manual.pdf",
                file_path: "app-data/general-documents/manual.pdf",
                page_count: 2,
            })
            .unwrap();

        let docs = store.list_general_documents().unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].page_count, 2);

        let pages = store.list_general_document_pages(id).unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].status, "pending");

        store
            .update_general_page_ocr(id, 1, "succeeded", Some("ข้อความหน้า 1"), None, None)
            .unwrap();
        store
            .update_general_page_asset(
                id,
                1,
                Some("app-data/general-documents/1/pages/page-001.png"),
                Some(1000),
                Some(1400),
                None,
            )
            .unwrap();
        let block = NewGeneralDocumentBlock {
            document_id: id,
            page_number: 1,
            block_index: 0,
            block_type: "paragraph",
            text: Some("ข้อความหน้า 1"),
            bbox_json: Some("[0,0,100,20]"),
            style_json: None,
            image_path: None,
            embedding: Some(&[0.1, 0.2, 0.3]),
        };
        store.replace_general_page_blocks(id, 1, &[block]).unwrap();
        store
            .save_general_revision(id, 1, "ตรวจคำผิด", "ข้อความหน้า 1 แก้แล้ว")
            .unwrap();

        let page = store.get_general_document_page(id, 1).unwrap().unwrap();
        assert_eq!(page.ocr_text.as_deref(), Some("ข้อความหน้า 1"));
        assert_eq!(page.edited_text.as_deref(), Some("ข้อความหน้า 1 แก้แล้ว"));
        assert_eq!(page.page_width, Some(1000));
        let blocks = store.list_general_document_blocks(id, Some(1)).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].block_type, "paragraph");
        assert_eq!(blocks[0].embedding.as_deref(), Some(&[0.1, 0.2, 0.3][..]));
        assert_eq!(
            store.get_general_document(id).unwrap().unwrap().status,
            "running"
        );
    }

    #[test]
    fn lists_templates_by_doc_type() {
        let store = SqliteStore::open_memory().unwrap();
        let id = store
            .create_template(NewTemplateRecord {
                doc_type: "ภายนอก",
                name: "กลาง",
                file_path: "templates/central.docx",
                agency: None,
                is_default: true,
            })
            .unwrap();

        let templates = store.list_templates(Some("ภายนอก"), None).unwrap();

        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].id, id);
    }
}
