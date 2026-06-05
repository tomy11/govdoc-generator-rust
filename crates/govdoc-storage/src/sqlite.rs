use std::path::Path;

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

pub struct SqliteStore {
    conn: Connection,
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

    pub fn store_memory(
        &self,
        doc_type: &str,
        summary_text: &str,
        fields: &Value,
        recipient_class: Option<&str>,
        agency: Option<&str>,
        template_id: Option<&str>,
        raw_text: Option<&str>,
    ) -> Result<i64> {
        self.conn.execute(
            r#"
            INSERT INTO gov_doc_memory (
                doc_type, summary_text, fields_json, recipient_class, agency, template_id, raw_text
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                doc_type,
                summary_text,
                serde_json::to_string(fields)?,
                recipient_class,
                agency,
                template_id,
                raw_text
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

    pub fn create_template(
        &self,
        doc_type: &str,
        name: &str,
        file_path: &str,
        agency: Option<&str>,
        is_default: bool,
    ) -> Result<i64> {
        if is_default {
            self.unset_default(doc_type, agency)?;
        }
        self.conn.execute(
            r#"
            INSERT INTO doc_template (doc_type, agency, name, file_path, is_default)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![doc_type, agency, name, file_path, is_default as i64],
        )?;
        Ok(self.conn.last_insert_rowid())
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

    fn find_default(
        &self,
        doc_type: &str,
        agency: Option<&str>,
    ) -> Result<Option<TemplateRecord>> {
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
            .store_memory(
                "ภายนอก",
                "summary",
                &serde_json::json!({ "subject": "ทดสอบ" }),
                None,
                None,
                None,
                None,
            )
            .unwrap();

        let fields = store.get_memory_fields(id).unwrap().unwrap();

        assert_eq!(fields["subject"], "ทดสอบ");
    }

    #[test]
    fn resolves_agency_default_before_central_default() {
        let store = SqliteStore::open_memory().unwrap();
        store
            .create_template("ภายนอก", "กลาง", "templates/central.docx", None, true)
            .unwrap();
        store
            .create_template(
                "ภายนอก",
                "หน่วยงาน",
                "templates/agency.docx",
                Some("โรงเรียน"),
                true,
            )
            .unwrap();

        let template = store.resolve_default("ภายนอก", Some("โรงเรียน")).unwrap().unwrap();

        assert_eq!(template.file_path, "templates/agency.docx");
    }
}
