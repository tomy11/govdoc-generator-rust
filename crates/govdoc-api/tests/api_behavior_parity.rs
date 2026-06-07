use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use govdoc_api::{router, AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn post_json(uri: &str, payload: Value) -> (StatusCode, Value) {
    let app = router(AppState::default());
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&body).unwrap_or(Value::Null);
    (status, json)
}

async fn get_json(app: axum::Router, uri: &str) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&body).unwrap_or(Value::Null);
    (status, json)
}

#[tokio::test]
async fn health_endpoint_keeps_contract() {
    let app = router(AppState::default());
    let (status, body) = get_json(app, "/health").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["app"], "govdoc-generator-rust");
}

#[tokio::test]
async fn generate_contract_matches_supported_document_types() {
    let cases = [
        ("ภายนอก", "subject", "ขอเชิญประชุม"),
        ("ภายใน", "subject", "รายงานความคืบหน้า"),
        ("คำสั่ง", "title", "แต่งตั้งคณะทำงาน"),
        ("ประกาศ", "title", "แจ้งวันหยุดราชการ"),
    ];

    for (doc_type, title_field, subject) in cases {
        let (status, body) = post_json(
            "/generate",
            json!({
                "doc_type": doc_type,
                "subject": subject,
                "purpose": "แจ้งเพื่อทราบ",
                "recipient_name": "หัวหน้าส่วนราชการ",
                "sender_name": "นายสมชาย",
                "sender_position": "ผู้อำนวยการ",
                "use_critic": false
            }),
        )
        .await;

        assert_eq!(status, StatusCode::OK, "{doc_type} should generate");
        assert_eq!(body["doc"]["doc_type"], doc_type);
        assert_eq!(body["doc"][title_field], subject);
        assert_eq!(body["trace"][0]["step"], "retrieval");
        assert_eq!(body["trace"][1]["step"], "generate");
        assert_eq!(body["trace"][2]["step"], "critic");
        assert_eq!(body["trace"][2]["detail"]["skipped"], true);
    }
}

#[tokio::test]
async fn generate_applies_deterministic_recipient_rules_after_llm_output() {
    let cases = [
        ("executive", "กราบเรียน", "ขอแสดงความนับถืออย่างยิ่ง"),
        ("monk", "กราบนมัสการ", "ขอนมัสการด้วยความเคารพ"),
        ("senior_official", "เรียน", "ขอแสดงความนับถืออย่างยิ่ง"),
    ];

    for (recipient_class, salutation, closing) in cases {
        let (status, body) = post_json(
            "/generate",
            json!({
                "doc_type": "ภายนอก",
                "subject": "ขอความอนุเคราะห์",
                "purpose": "ขอรับการสนับสนุน",
                "recipient_name": "ผู้รับ",
                "recipient_class": recipient_class,
                "sender_name": "นายสมชาย",
                "sender_position": "ผู้อำนวยการ",
                "use_critic": false
            }),
        )
        .await;

        assert_eq!(status, StatusCode::OK, "{recipient_class} should generate");
        assert_eq!(body["doc"]["salutation"], salutation);
        assert_eq!(body["doc"]["closing"], closing);
    }
}

#[tokio::test]
async fn edit_defaults_to_body_field_and_preserves_other_fields() {
    let (status, body) = post_json(
        "/edit",
        json!({
            "doc_type": "ภายนอก",
            "doc_data": {
                "doc_type": "ภายนอก",
                "subject": "เรื่องเดิม",
                "body": ["ย่อหน้าแรก", "ย่อหน้าที่สอง"],
                "closing": "ขอแสดงความนับถือ"
            },
            "edit_instructions": "ปรับให้เป็นทางการ"
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["subject"], "เรื่องเดิม");
    assert_eq!(body["closing"], "ขอแสดงความนับถือ");
    assert_eq!(
        body["body"],
        json!([
            "ย่อหน้าแรก (แก้ไข: ปรับให้เป็นทางการ)",
            "ย่อหน้าที่สอง (แก้ไข: ปรับให้เป็นทางการ)"
        ])
    );
}

#[tokio::test]
async fn template_default_resolution_prefers_agency_then_central() {
    let app = router(AppState::default());

    for payload in [
        json!({
            "doc_type": "ภายนอก",
            "name": "กลาง",
            "file_path": "templates/central.docx",
            "is_default": true
        }),
        json!({
            "doc_type": "ภายนอก",
            "agency": "กรมตัวอย่าง",
            "name": "กรมตัวอย่าง",
            "file_path": "templates/agency.docx",
            "is_default": true
        }),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/templates")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    let encoded_doc_type = "%E0%B8%A0%E0%B8%B2%E0%B8%A2%E0%B8%99%E0%B8%AD%E0%B8%81";
    let encoded_agency =
        "%E0%B8%81%E0%B8%A3%E0%B8%A1%E0%B8%95%E0%B8%B1%E0%B8%A7%E0%B8%AD%E0%B8%A2%E0%B9%88%E0%B8%B2%E0%B8%87";

    let (agency_status, agency_body) = get_json(
        app.clone(),
        &format!("/templates/default?doc_type={encoded_doc_type}&agency={encoded_agency}"),
    )
    .await;
    assert_eq!(agency_status, StatusCode::OK);
    assert_eq!(agency_body["file_path"], "templates/agency.docx");

    let (central_status, central_body) = get_json(
        app,
        &format!("/templates/default?doc_type={encoded_doc_type}&agency=%E0%B8%81%E0%B8%A3%E0%B8%A1%E0%B8%AD%E0%B8%B7%E0%B9%88%E0%B8%99"),
    )
    .await;
    assert_eq!(central_status, StatusCode::OK);
    assert_eq!(central_body["file_path"], "templates/central.docx");
}

#[tokio::test]
async fn render_validates_document_shape_before_sidecar_configuration() {
    let (status, body) = post_json(
        "/render",
        json!({
            "doc_type": "ภายนอก",
            "doc_data": {
                "doc_type": "ภายนอก",
                "subject": "ข้อมูลไม่ครบ"
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["detail"]
        .as_str()
        .is_some_and(|detail| detail.starts_with("Invalid document data:")));
}
