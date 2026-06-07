use async_trait::async_trait;
use govdoc_domain::DocType;
use govdoc_usecases::{EmbeddingProvider, LlmProvider};
use serde_json::{json, Value};

pub struct FakeLlmProvider;

#[async_trait]
impl LlmProvider for FakeLlmProvider {
    async fn complete(
        &self,
        system: &str,
        user: &str,
        _max_tokens: usize,
    ) -> anyhow::Result<String> {
        if system.contains("แก้ไขหนังสือราชการไทย") {
            return Ok(fake_edit_from_prompt(user));
        }
        Ok(user.to_string())
    }

    async fn complete_json(
        &self,
        system: &str,
        user: &str,
        _schema: Value,
        _max_tokens: usize,
    ) -> anyhow::Result<Value> {
        if system.contains("ผู้ตรวจสอบ") {
            return Ok(json!({
                "passed": true,
                "issues": [],
                "suggestions": []
            }));
        }

        Ok(fake_document_from_prompt(user))
    }
}

pub struct FakeEmbeddingProvider;

#[async_trait]
impl EmbeddingProvider for FakeEmbeddingProvider {
    async fn embed(&self, _text: &str) -> anyhow::Result<Vec<f32>> {
        Ok(vec![0.0; self.dimensions()])
    }

    fn dimensions(&self) -> usize {
        8
    }
}

fn fake_document_from_prompt(prompt: &str) -> Value {
    let request_line = prompt.lines().next().unwrap_or_default();
    let doc_type = if request_line.contains("ภายใน") {
        DocType::Internal
    } else if request_line.contains("คำสั่ง") {
        DocType::Order
    } else if request_line.contains("ประกาศ") {
        DocType::Announcement
    } else {
        DocType::External
    };

    let subject = extract_prompt_value(prompt, "เรื่อง").unwrap_or_else(|| "เรื่องทดสอบ".into());
    let recipient = extract_prompt_value(prompt, "ผู้รับ").unwrap_or_else(|| "ผู้เกี่ยวข้อง".into());
    let sender_name = extract_prompt_value(prompt, "ผู้ลงนาม").unwrap_or_else(|| "ผู้ลงนาม".into());
    let sender_position = extract_prompt_value(prompt, "ตำแหน่ง").unwrap_or_else(|| "ตำแหน่ง".into());
    let purpose = extract_prompt_value(prompt, "จุดประสงค์").unwrap_or_else(|| "แจ้งเพื่อทราบ".into());

    match doc_type {
        DocType::External => json!({
            "doc_type": "ภายนอก",
            "number": "ศธ 0000/0001",
            "agency": "หน่วยงานตัวอย่าง",
            "date": "1 มกราคม 2569",
            "subject": subject,
            "recipient": recipient,
            "salutation": "",
            "reference": [],
            "enclosure": [],
            "body": [
                format!("ด้วยหน่วยงานตัวอย่างมีความประสงค์{}", purpose),
                "จึงเรียนมาเพื่อโปรดพิจารณา"
            ],
            "closing": "",
            "signer_name": sender_name,
            "signer_position": sender_position
        }),
        DocType::Internal => json!({
            "doc_type": "ภายใน",
            "agency": "หน่วยงานตัวอย่าง",
            "reference_number": "ศธ 0000/0001",
            "date": "1 มกราคม 2569",
            "subject": subject,
            "recipient": recipient,
            "salutation": "",
            "body": [
                format!("ตามที่มีภารกิจเกี่ยวกับ{}", purpose),
                "จึงเรียนมาเพื่อโปรดทราบ"
            ],
            "closing": "",
            "signer_name": sender_name,
            "signer_position": sender_position
        }),
        DocType::Order => json!({
            "doc_type": "คำสั่ง",
            "number": "1/2569",
            "title": subject,
            "body": [
                format!("เพื่อให้การดำเนินงาน{}เป็นไปด้วยความเรียบร้อย", purpose),
                "จึงมีคำสั่งให้ผู้เกี่ยวข้องดำเนินการตามหน้าที่"
            ],
            "date": "1 มกราคม 2569",
            "signer_name": sender_name,
            "signer_position": sender_position
        }),
        DocType::Announcement => json!({
            "doc_type": "ประกาศ",
            "number": "1/2569",
            "title": subject,
            "body": [
                format!("หน่วยงานตัวอย่างขอประกาศให้ทราบเกี่ยวกับ{}", purpose),
                "จึงประกาศมาเพื่อทราบโดยทั่วกัน"
            ],
            "date": "1 มกราคม 2569",
            "signer_name": sender_name,
            "signer_position": sender_position
        }),
    }
}

fn extract_prompt_value(prompt: &str, label: &str) -> Option<String> {
    let prefix = format!("{label}: ");
    prompt.lines().find_map(|line| {
        line.strip_prefix(&prefix)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn fake_edit_from_prompt(prompt: &str) -> String {
    let instructions =
        extract_prompt_value(prompt, "คำสั่งแก้ไข").unwrap_or_else(|| "แก้ไข".to_string());

    if prompt.contains("แต่ละบรรทัดคือ 1 ย่อหน้า") {
        return editable_lines(prompt)
            .into_iter()
            .map(|line| format!("{line} (แก้ไข: {instructions})"))
            .collect::<Vec<_>>()
            .join("\n");
    }

    editable_lines(prompt)
        .into_iter()
        .next()
        .map(|line| format!("{line} (แก้ไข: {instructions})"))
        .unwrap_or_else(|| format!("แก้ไขแล้ว: {instructions}"))
}

fn editable_lines(prompt: &str) -> Vec<String> {
    prompt
        .lines()
        .skip(1)
        .take_while(|line| !line.starts_with("คำสั่งแก้ไข:"))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}
