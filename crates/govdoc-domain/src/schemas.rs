use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub enum DocType {
    #[serde(rename = "ภายนอก")]
    External,
    #[serde(rename = "ภายใน")]
    Internal,
    #[serde(rename = "คำสั่ง")]
    Order,
    #[serde(rename = "ประกาศ")]
    Announcement,
}

impl DocType {
    pub fn as_thai(&self) -> &'static str {
        match self {
            Self::External => "ภายนอก",
            Self::Internal => "ภายใน",
            Self::Order => "คำสั่ง",
            Self::Announcement => "ประกาศ",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecipientClass {
    #[default]
    GeneralPublic,
    JuniorOfficial,
    SeniorOfficial,
    Executive,
    Monk,
    Royal,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct ExternalDoc {
    pub doc_type: DocType,
    pub number: String,
    pub agency: String,
    pub date: String,
    pub subject: String,
    pub recipient: String,
    pub salutation: String,
    #[serde(default)]
    pub reference: Vec<String>,
    #[serde(default)]
    pub enclosure: Vec<String>,
    pub body: Vec<String>,
    pub closing: String,
    pub signer_name: String,
    pub signer_position: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct InternalDoc {
    pub doc_type: DocType,
    pub agency: String,
    pub reference_number: String,
    pub date: String,
    pub subject: String,
    pub recipient: String,
    pub salutation: String,
    pub body: Vec<String>,
    pub closing: String,
    pub signer_name: String,
    pub signer_position: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct OrderDoc {
    pub doc_type: DocType,
    pub number: String,
    pub title: String,
    pub body: Vec<String>,
    pub date: String,
    pub signer_name: String,
    pub signer_position: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct AnnouncementDoc {
    pub doc_type: DocType,
    pub number: String,
    pub title: String,
    pub body: Vec<String>,
    pub date: String,
    pub signer_name: String,
    pub signer_position: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "doc_type")]
pub enum GovDoc {
    #[serde(rename = "ภายนอก")]
    External(ExternalDoc),
    #[serde(rename = "ภายใน")]
    Internal(InternalDoc),
    #[serde(rename = "คำสั่ง")]
    Order(OrderDoc),
    #[serde(rename = "ประกาศ")]
    Announcement(AnnouncementDoc),
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct DocRequest {
    pub doc_type: DocType,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub purpose: String,
    #[serde(default)]
    pub recipient_name: String,
    #[serde(default)]
    pub recipient_class: RecipientClass,
    #[serde(default)]
    pub recipient_agency: String,
    #[serde(default)]
    pub sender_name: String,
    #[serde(default)]
    pub sender_position: String,
    #[serde(default)]
    pub additional_context: String,
    #[serde(default)]
    pub use_critic: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct CriticReview {
    pub passed: bool,
    #[serde(default)]
    pub issues: Vec<String>,
    #[serde(default)]
    pub suggestions: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct EditRequest {
    pub doc_type: DocType,
    pub doc_data: serde_json::Value,
    pub edit_instructions: String,
    #[serde(default = "default_target_fields")]
    pub target_fields: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct RenderRequest {
    pub doc_type: DocType,
    pub doc_data: serde_json::Value,
    #[serde(default)]
    pub template_id: Option<i64>,
}

fn default_target_fields() -> Vec<String> {
    vec!["body".to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_type_serializes_to_thai_values() {
        let json = serde_json::to_string(&DocType::External).unwrap();
        assert_eq!(json, "\"ภายนอก\"");
    }

    #[test]
    fn doc_request_defaults_recipient_class() {
        let req: DocRequest = serde_json::from_value(serde_json::json!({
            "doc_type": "ภายนอก"
        }))
        .unwrap();

        assert_eq!(req.recipient_class, RecipientClass::GeneralPublic);
    }
}
