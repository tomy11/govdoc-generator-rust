use govdoc_domain::{
    lookup_closing, lookup_salutation, AnnouncementDoc, CriticReview, DocRequest, DocType,
    ExternalDoc, InternalDoc, OrderDoc,
};
use schemars::{schema_for, JsonSchema};
use serde_json::{json, Value};
use thiserror::Error;

use crate::ports::{EmbeddingProvider, LlmProvider, MemoryRepository};

pub const GENERATOR_SYSTEM_PROMPT: &str = "คุณคือผู้ช่วยสร้างหนังสือราชการไทย\nสร้าง JSON ตาม schema ที่กำหนดให้เท่านั้น\nเนื้อหาต้องถูกต้องตามระเบียบงานสารบรรณ\nใช้ภาษาไทยที่ถูกต้อง เป็นทางการ";

pub const CRITIC_SYSTEM_PROMPT: &str = "คุณคือผู้ตรวจสอบหนังสือราชการไทย\nตรวจสอบว่า JSON ถูกต้องตามระเบียบงานสารบรรณหรือไม่\nตอบเป็น JSON ด้วย {\"passed\": bool, \"issues\": [...], \"suggestions\": [...]}";

#[derive(Clone, Debug, PartialEq)]
pub struct TraceEvent {
    pub step: String,
    pub detail: Value,
}

pub struct GenerationServices<'a> {
    pub generator: &'a dyn LlmProvider,
    pub critic: &'a dyn LlmProvider,
    pub memory_repo: &'a dyn MemoryRepository,
    pub embedding_provider: &'a dyn EmbeddingProvider,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GenerationOptions {
    pub max_rounds: usize,
    pub use_critic: bool,
}

impl Default for GenerationOptions {
    fn default() -> Self {
        Self {
            max_rounds: 3,
            use_critic: true,
        }
    }
}

#[derive(Debug, Error)]
pub enum GenerationError {
    #[error("LLM error: {0}")]
    Llm(#[from] anyhow::Error),
    #[error("validation error: {0}")]
    Validation(String),
    #[error("ไม่ผ่านการตรวจหลัง {0} รอบ ส่งต่อให้มนุษย์ตรวจสอบ")]
    CriticLoopExceeded(usize),
}

pub async fn generate_document_json(
    req: &DocRequest,
    services: GenerationServices<'_>,
    options: GenerationOptions,
    trace: &mut Vec<TraceEvent>,
) -> Result<Value, GenerationError> {
    let examples = retrieve_examples(req, services.memory_repo, services.embedding_provider).await;
    trace.push(TraceEvent {
        step: "retrieval".to_string(),
        detail: json!({ "examples": examples.len() }),
    });

    let schema = schema_for_doc_type(&req.doc_type);
    let prompt = build_generator_prompt(req, &examples, &schema);
    let mut draft = services
        .generator
        .complete_json(GENERATOR_SYSTEM_PROMPT, &prompt, schema.clone(), 8192)
        .await?;
    trace.push(TraceEvent {
        step: "generate".to_string(),
        detail: json!({ "round": 0, "detail": "ร่างแรก" }),
    });

    if options.use_critic {
        let mut passed = false;
        for round in 0..options.max_rounds {
            let critic_prompt = build_critic_prompt(&draft, req);
            let review_raw = services
                .critic
                .complete_json(
                    CRITIC_SYSTEM_PROMPT,
                    &critic_prompt,
                    serde_json::to_value(schema_for!(CriticReview)).unwrap_or_else(|_| json!({})),
                    2048,
                )
                .await?;
            let review: CriticReview = serde_json::from_value(review_raw)
                .map_err(|err| GenerationError::Validation(err.to_string()))?;

            trace.push(TraceEvent {
                step: "critic".to_string(),
                detail: json!({
                    "round": round + 1,
                    "passed": review.passed,
                    "issues": review.issues,
                    "suggestions": review.suggestions
                }),
            });

            if review.passed {
                passed = true;
                break;
            }

            let feedback_prompt = format!(
                "{}\n\nข้อแก้ไขจากผู้ตรวจ:\n{}",
                prompt,
                review
                    .suggestions
                    .iter()
                    .map(|s| format!("- {s}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            draft = services
                .generator
                .complete_json(
                    GENERATOR_SYSTEM_PROMPT,
                    &feedback_prompt,
                    schema.clone(),
                    8192,
                )
                .await?;
            trace.push(TraceEvent {
                step: "generate".to_string(),
                detail: json!({ "round": round + 1, "detail": "แก้ตามข้อติงของ critic" }),
            });
        }
        if !passed {
            return Err(GenerationError::CriticLoopExceeded(options.max_rounds));
        }
    } else {
        trace.push(TraceEvent {
            step: "critic".to_string(),
            detail: json!({ "skipped": true, "detail": "โหมด single-agent — ข้ามการตรวจ" }),
        });
    }

    apply_deterministic_rules(&mut draft, req);
    validate_doc_json(&req.doc_type, draft)
}

pub fn build_query_summary(req: &DocRequest) -> String {
    let mut parts = vec![format!("ประเภท: {}", req.doc_type.as_thai())];
    if !req.subject.is_empty() {
        parts.push(format!("เรื่อง: {}", req.subject));
    }
    if !req.purpose.is_empty() {
        parts.push(format!("จุดประสงค์: {}", req.purpose));
    }
    let recipient = if req.recipient_agency.is_empty() {
        &req.recipient_name
    } else {
        &req.recipient_agency
    };
    if !recipient.is_empty() {
        parts.push(format!("ผู้รับ: {recipient}"));
    }
    if !req.additional_context.is_empty() {
        parts.push(format!("บริบท: {}", req.additional_context));
    }
    parts.join(" | ")
}

async fn retrieve_examples(
    req: &DocRequest,
    memory_repo: &dyn MemoryRepository,
    embedding_provider: &dyn EmbeddingProvider,
) -> Vec<Value> {
    match embedding_provider.embed(&build_query_summary(req)).await {
        Ok(embedding) => memory_repo
            .retrieve_by_similarity(req, &embedding, 3)
            .await
            .unwrap_or_default(),
        Err(_) => memory_repo.retrieve(req, 3).await.unwrap_or_default(),
    }
}

fn build_generator_prompt(req: &DocRequest, examples: &[Value], schema: &Value) -> String {
    let mut parts = vec![format!(
        "จงสร้าง{}ราชการประเภท {}",
        if matches!(req.doc_type, DocType::External | DocType::Internal) {
            "หนังสือ"
        } else {
            ""
        },
        req.doc_type.as_thai()
    )];

    append_if_present(&mut parts, "เรื่อง", &req.subject);
    append_if_present(&mut parts, "จุดประสงค์", &req.purpose);
    append_if_present(&mut parts, "ผู้รับ", &req.recipient_name);
    append_if_present(&mut parts, "หน่วยงานผู้รับ", &req.recipient_agency);
    append_if_present(&mut parts, "ผู้ลงนาม", &req.sender_name);
    append_if_present(&mut parts, "ตำแหน่ง", &req.sender_position);
    append_if_present(&mut parts, "บริบทเพิ่มเติม", &req.additional_context);

    parts.push(String::new());
    parts.push("Schema JSON ที่ต้องเอาต์พุต:".to_string());
    parts.push(serde_json::to_string_pretty(schema).unwrap_or_default());

    if !examples.is_empty() {
        parts.push(String::new());
        parts.push("ตัวอย่างเอกสารที่คล้ายกัน:".to_string());
        for (idx, example) in examples.iter().enumerate() {
            parts.push(format!("--- ตัวอย่าง {} ---", idx + 1));
            parts.push(serde_json::to_string_pretty(example).unwrap_or_default());
        }
    }

    parts.join("\n")
}

fn build_critic_prompt(doc_json: &Value, req: &DocRequest) -> String {
    let checklist = if matches!(req.doc_type, DocType::External | DocType::Internal) {
        vec![
            "มีคำขึ้นต้นที่ถูกต้อง (เรียน/กราบเรียน/กราบนมัสการ/กราบบังคมทูล)",
            "มีคำลงท้ายที่ถูกต้อง",
            "เนื้อความอ่านเข้าใจได้ เหมาะสมกับระดับผู้รับ",
            "ครบถ้วน: เลขที่, วันที่, เรื่อง, เนื้อความ, ลงชื่อ, ตำแหน่ง",
        ]
    } else {
        vec!["มีเลขที่ รายการข้อ วันที่ ผู้ลงนาม ตำแหน่ง", "เนื้อความเป็นข้อ ๆ ชัดเจน"]
    };

    let mut parts = vec![
        "ตรวจสอบ JSON หนังสือราชการต่อไปนี้:".to_string(),
        serde_json::to_string_pretty(doc_json).unwrap_or_default(),
        String::new(),
        "รายการตรวจสอบ:".to_string(),
    ];
    for (idx, item) in checklist.iter().enumerate() {
        parts.push(format!("{}. {}", idx + 1, item));
    }
    parts.push(String::new());
    parts.push(
        "ตอบเป็น JSON: {\"passed\": true/false, \"issues\": [\"...\"], \"suggestions\": [\"...\"]}"
            .to_string(),
    );
    parts.join("\n")
}

fn append_if_present(parts: &mut Vec<String>, label: &str, value: &str) {
    if !value.is_empty() {
        parts.push(format!("{label}: {value}"));
    }
}

fn schema_for_doc_type(doc_type: &DocType) -> Value {
    match doc_type {
        DocType::External => schema_value::<ExternalDoc>(),
        DocType::Internal => schema_value::<InternalDoc>(),
        DocType::Order => schema_value::<OrderDoc>(),
        DocType::Announcement => schema_value::<AnnouncementDoc>(),
    }
}

fn schema_value<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).unwrap_or_else(|_| json!({}))
}

fn apply_deterministic_rules(draft: &mut Value, req: &DocRequest) {
    if let Value::Object(map) = draft {
        map.insert(
            "salutation".to_string(),
            Value::String(lookup_salutation(&req.recipient_class).to_string()),
        );
        map.insert(
            "closing".to_string(),
            Value::String(lookup_closing(&req.recipient_class).to_string()),
        );
    }
}

fn validate_doc_json(doc_type: &DocType, draft: Value) -> Result<Value, GenerationError> {
    match doc_type {
        DocType::External => serde_json::from_value::<ExternalDoc>(draft.clone())
            .map(|_| ())
            .map_err(|err| GenerationError::Validation(err.to_string()))?,
        DocType::Internal => serde_json::from_value::<InternalDoc>(draft.clone())
            .map(|_| ())
            .map_err(|err| GenerationError::Validation(err.to_string()))?,
        DocType::Order => serde_json::from_value::<OrderDoc>(draft.clone())
            .map(|_| ())
            .map_err(|err| GenerationError::Validation(err.to_string()))?,
        DocType::Announcement => serde_json::from_value::<AnnouncementDoc>(draft.clone())
            .map(|_| ())
            .map_err(|err| GenerationError::Validation(err.to_string()))?,
    };

    Ok(draft)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_summary_matches_python_shape() {
        let req = DocRequest {
            doc_type: DocType::External,
            subject: "ขอเชิญร่วมงาน".into(),
            purpose: "แจ้งกำหนดการ".into(),
            recipient_name: "ผู้ปกครอง".into(),
            recipient_class: Default::default(),
            recipient_agency: String::new(),
            sender_name: String::new(),
            sender_position: String::new(),
            additional_context: "ช่วงเดือนมกราคม".into(),
            use_critic: None,
        };

        assert_eq!(
            build_query_summary(&req),
            "ประเภท: ภายนอก | เรื่อง: ขอเชิญร่วมงาน | จุดประสงค์: แจ้งกำหนดการ | ผู้รับ: ผู้ปกครอง | บริบท: ช่วงเดือนมกราคม"
        );
    }
}
