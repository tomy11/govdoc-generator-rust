use serde_json::Value;
use thiserror::Error;

use crate::ports::LlmProvider;

pub const EDITOR_SYSTEM_PROMPT: &str = "คุณคือผู้ช่วยแก้ไขหนังสือราชการไทย\nแก้ไขเฉพาะฟิลด์ที่ระบุตามคำสั่งเท่านั้น\nตอบกลับเฉพาะข้อความที่แก้แล้วเท่านั้น\nคงรูปแบบเดิมไว้ อย่าเปลี่ยนโครงสร้าง";

#[derive(Debug, Error)]
pub enum EditError {
    #[error("Document has no field '{0}'")]
    MissingField(String),
    #[error("Unsupported field type for '{0}'")]
    UnsupportedField(String),
    #[error("LLM error: {0}")]
    Llm(#[from] anyhow::Error),
}

pub async fn edit_document_json(
    mut doc: Value,
    edit_instructions: &str,
    editor: &dyn LlmProvider,
    target_fields: &[String],
) -> Result<Value, EditError> {
    let object = doc
        .as_object_mut()
        .ok_or_else(|| EditError::UnsupportedField("<root>".to_string()))?;

    for field in target_fields {
        let value = object
            .get(field)
            .cloned()
            .ok_or_else(|| EditError::MissingField(field.clone()))?;

        let edited = match value {
            Value::String(text) => {
                Value::String(edit_scalar(field, &text, edit_instructions, editor).await?)
            }
            Value::Array(items) => {
                let paragraphs = items
                    .into_iter()
                    .filter_map(|item| item.as_str().map(ToOwned::to_owned))
                    .collect::<Vec<_>>();
                Value::Array(
                    edit_list(field, &paragraphs, edit_instructions, editor)
                        .await?
                        .into_iter()
                        .map(Value::String)
                        .collect(),
                )
            }
            _ => return Err(EditError::UnsupportedField(field.clone())),
        };
        object.insert(field.clone(), edited);
    }

    Ok(doc)
}

async fn edit_scalar(
    field: &str,
    value: &str,
    instructions: &str,
    editor: &dyn LlmProvider,
) -> Result<String, EditError> {
    let prompt = format!(
        "แก้ไขฟิลด์ '{field}' ต่อไปนี้ตามคำสั่ง:\n{value}\n\nคำสั่งแก้ไข: {instructions}\n\nตอบเฉพาะข้อความที่แก้แล้วเท่านั้น"
    );
    Ok(editor
        .complete(EDITOR_SYSTEM_PROMPT, &prompt, 4096)
        .await?
        .trim()
        .to_string())
}

async fn edit_list(
    field: &str,
    items: &[String],
    instructions: &str,
    editor: &dyn LlmProvider,
) -> Result<Vec<String>, EditError> {
    let prompt = format!(
        "แก้ไขฟิลด์ '{field}' ต่อไปนี้ตามคำสั่ง:\n{}\n\nคำสั่งแก้ไข: {instructions}\n\nตอบเฉพาะข้อความที่แก้แล้วเท่านั้น แต่ละบรรทัดคือ 1 ย่อหน้า",
        items.join("\n")
    );
    let result = editor.complete(EDITOR_SYSTEM_PROMPT, &prompt, 4096).await?;
    Ok(result
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}
