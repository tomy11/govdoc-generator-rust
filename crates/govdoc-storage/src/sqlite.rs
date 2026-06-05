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
            "#,
        )?;
        Ok(())
    }

    pub fn store_memory(&self, record: NewMemoryRecord<'_>) -> Result<i64> {
        self.conn.execute(
            r#"
            INSERT INTO gov_doc_memory (
                doc_type, summary_text, fields_json, recipient_class, agency, template_id, raw_text
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                record.doc_type,
                record.summary_text,
                serde_json::to_string(record.fields)?,
                record.recipient_class,
                record.agency,
                record.template_id,
                record.raw_text
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
            })
            .unwrap();

        let fields = store.get_memory_fields(id).unwrap().unwrap();

        assert_eq!(fields["subject"], "ทดสอบ");
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
