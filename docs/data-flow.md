# Data Flow

เอกสารนี้สรุป data flow ของโปรแกรมในสองมุม:

- flow แยกตาม endpoint ของ `govdoc-api`
- flow รวมของทั้งโปรแกรม ตั้งแต่ desktop UI จนถึง storage, model backends, OCR, และ renderer

## ภาพรวมสถาปัตยกรรม

```mermaid
flowchart LR
    User["ผู้ใช้"]
    Tauri["Tauri desktop shell<br/>src-tauri"]
    UI["Static frontend<br/>ui/index.html + app.js"]
    API["govdoc-api sidecar<br/>Axum HTTP API"]
    Domain["govdoc-domain<br/>schemas + rules"]
    Usecases["govdoc-usecases<br/>generate/edit/structure"]
    Storage["govdoc-storage<br/>SQLite + vector index"]
    SQLite[("SQLite<br/>govdoc.sqlite3")]
    Vector[("HNSW index cache<br/>hnsw.index")]
    LLM["LLM backend<br/>fake / typhoon-local / typhoon-cloud"]
    Embed["Embedding backend<br/>fake / remote"]
    OCR["Typhoon OCR<br/>cloud"]
    Renderer["Python docx sidecar<br/>render_docx_sidecar.py"]
    Docx[".docx output"]
    Templates[("Uploaded templates<br/>app-data/templates")]

    User --> Tauri
    Tauri --> UI
    Tauri -->|"spawns on launch"| API
    UI -->|"HTTP localhost<br/>127.0.0.1:8000"| API
    API --> Domain
    API --> Usecases
    API --> Storage
    Storage --> SQLite
    Storage --> Vector
    Usecases --> LLM
    Usecases --> Embed
    API --> OCR
    API --> Renderer
    Renderer --> Docx
    API --> Templates
    Templates --> Renderer
```

## Flow รวมของโปรแกรม

```mermaid
sequenceDiagram
    participant User as ผู้ใช้
    participant Tauri as Tauri shell
    participant UI as Static UI
    participant API as govdoc-api
    participant Store as SQLite store
    participant Index as Vector index
    participant LLM as LLM provider
    participant Embed as Embedding provider
    participant OCR as OCR provider
    participant Render as DOCX renderer

    User->>Tauri: เปิดแอป desktop
    Tauri->>API: spawn sidecar govdoc-api
    API->>Store: open SQLITE_PATH หรือ in-memory
    API->>Index: load HNSW_INDEX_PATH หรือ rebuild จาก SQLite
    Tauri->>UI: load static frontend
    UI->>API: GET /status
    API-->>UI: backend readiness

    alt สร้างเอกสาร
        User->>UI: กรอกฟอร์มแล้วกดสร้าง
        UI->>API: POST /generate
        API->>Store: ค้น examples ล่าสุดหรือใกล้เคียง
        API->>Index: similarity search ถ้ามี embedding
        API->>Embed: embed query ถ้า EMBEDDING_BACKEND=remote
        API->>LLM: generate + critic loop
        LLM-->>API: document JSON
        API-->>UI: doc + trace
    end

    alt บันทึกเอกสาร
        UI->>API: POST /documents
        API->>Store: insert document
        Store-->>API: id
        API-->>UI: saved id
        UI->>API: GET /documents
        API-->>UI: saved document list
    end

    alt export เป็น .docx
        UI->>API: POST /render
        API->>Store: resolve default template
        API->>Render: ส่ง document JSON + template path
        Render-->>API: docx bytes
        API-->>UI: .docx download
    end

    alt เพิ่มตัวอย่างให้ AI เลียนแบบ
        User->>UI: upload รูปหรือ PDF
        UI->>API: POST /ingest/ocr/upload
        API->>OCR: OCR file bytes
        OCR-->>API: raw text
        API->>LLM: structure text เป็น schema
        API->>Embed: embed summary ถ้าเปิด remote embedding
        API->>Store: insert gov_doc_memory
        API->>Index: update vector index ถ้ามี embedding
        API-->>UI: ingest result
    end

    alt เพิ่ม template สำหรับ render
        User->>UI: upload .docx template
        UI->>API: POST /templates/upload
        API->>API: save file under GOVDOC_TEMPLATES_DIR
        API->>Store: register template metadata
        API-->>UI: template record
    end

    User->>Tauri: ปิดแอป
    Tauri->>API: kill sidecar
```

## Flow แยกตาม endpoint

### Health และ status

```mermaid
flowchart TD
    UI["UI / local tool"] --> Health["GET /health"]
    UI --> Status["GET /status"]
    Health --> HealthResp["{ status, app }"]
    Status --> Env["อ่าน backend config จาก AppState/env"]
    Env --> StatusResp["{ llm, embedding, ocr, renderer_configured, persistent }"]
```

### Generate document

```mermaid
flowchart TD
    Req["POST /generate<br/>DocRequest"] --> Validate["deserialize + schema types"]
    Validate --> Build["build LLM + embedding provider"]
    Build --> Memory["SqliteMemoryRepository"]
    Memory --> Retrieve{"embedding backend?"}
    Retrieve -->|"remote"| QueryEmbed["embed query"]
    QueryEmbed --> VectorSearch["vector similarity search"]
    Retrieve -->|"fake"| Recent["recent examples fallback"]
    VectorSearch --> Examples["retrieved examples"]
    Recent --> Examples
    Examples --> Generate["generate_document_json"]
    Generate --> LLM["LLM generation"]
    LLM --> Critic{"use_critic?"}
    Critic -->|"true"| Review["critic loop up to max_rounds"]
    Critic -->|"false"| Done["validated document JSON"]
    Review --> Done
    Done --> Resp["GenerateResponse<br/>{ doc, trace }"]
```

### Edit document

```mermaid
flowchart TD
    Req["POST /edit<br/>EditRequest"] --> Build["build LLM provider"]
    Build --> Edit["edit_document_json"]
    Edit --> Fields["apply target_fields if provided"]
    Fields --> Resp["edited document JSON"]
```

### Render document to DOCX

```mermaid
flowchart TD
    Req["POST /render<br/>RenderRequest"] --> Validate["validate_render_doc"]
    Validate --> Resolve["resolve_template_path"]
    Resolve --> Store["SQLite doc_template"]
    Store --> Template{"default template found?"}
    Template -->|"yes"| Sidecar["render_with_sidecar<br/>GOVDOC_RENDERER_CMD"]
    Template -->|"no"| Sidecar
    Sidecar --> Python["scripts/render_docx_sidecar.py"]
    Python --> Resp["docx bytes<br/>attachment govdoc.docx"]
```

### Ingest structured example

```mermaid
flowchart TD
    Req["POST /ingest<br/>IngestRequest"] --> Summary{"summary provided?"}
    Summary -->|"yes"| UseSummary["use request summary"]
    Summary -->|"no"| Derive["derive_summary from fields"]
    UseSummary --> Embed{"embedding backend?"}
    Derive --> Embed
    Embed -->|"remote"| EmbedCall["embed summary"]
    Embed -->|"fake"| NoVector["no vector"]
    EmbedCall --> Store["insert gov_doc_memory"]
    NoVector --> Store
    Store --> Index{"embedding exists?"}
    Index -->|"yes"| UpdateIndex["update vector index"]
    Index -->|"no"| Resp
    UpdateIndex --> Resp["IngestResponse<br/>{ id, embedded, structured: true }"]
```

### Ingest OCR from local path

```mermaid
flowchart TD
    Req["POST /ingest/ocr<br/>IngestOcrRequest"] --> Read["read local file_path"]
    Read --> Pipeline["run_ocr_ingest"]
    Pipeline --> OCR["Typhoon OCR"]
    OCR --> Text["raw OCR text"]
    Text --> Structure{"structure=true?"}
    Structure -->|"yes"| LLM["LLM structure_document_from_text"]
    Structure -->|"false"| RawFields["raw content fields"]
    LLM --> Parsed{"valid schema?"}
    Parsed -->|"yes"| Fields["structured fields"]
    Parsed -->|"no"| RawFields
    Fields --> Summary["derive summary"]
    RawFields --> SummaryRaw["truncate raw text summary"]
    Summary --> Embed["embed + store memory"]
    SummaryRaw --> Embed
    Embed --> Resp["IngestResponse"]
```

### Ingest OCR upload

```mermaid
flowchart TD
    Req["POST /ingest/ocr/upload<br/>multipart file + doc_type"] --> Upload["Upload::collect"]
    Upload --> Validate["validate doc_type + file field"]
    Validate --> Pipeline["run_ocr_ingest"]
    Pipeline --> OCRFlow["same flow as /ingest/ocr after file read"]
    OCRFlow --> Resp["IngestResponse"]
```

### Template management

```mermaid
flowchart TD
    List["GET /templates<br/>query doc_type, agency"] --> StoreList["SQLite list_templates"]
    StoreList --> ListResp["TemplateResponse[]"]

    Create["POST /templates<br/>TemplateCreateRequest"] --> StoreCreate["SQLite create_template"]
    StoreCreate --> ReadBack["get_template"]
    ReadBack --> CreateResp["TemplateResponse"]

    Upload["POST /templates/upload<br/>multipart .docx"] --> Collect["Upload::collect"]
    Collect --> Save["write file to GOVDOC_TEMPLATES_DIR"]
    Save --> Register["SQLite create_template"]
    Register --> UploadResp["TemplateResponse"]

    Default["GET /templates/default<br/>doc_type, agency"] --> Resolve["resolve_default"]
    Resolve --> DefaultResp["TemplateResponse or 404"]
```

### Saved documents

```mermaid
flowchart TD
    Save["POST /documents<br/>SaveDocumentRequest"] --> Insert["SQLite insert document"]
    Insert --> SaveResp["{ id }"]

    List["GET /documents<br/>optional doc_type"] --> Query["SQLite list_documents"]
    Query --> ListResp["DocumentSummaryResponse[]"]

    Get["GET /documents/:id"] --> Fetch["SQLite get_document"]
    Fetch --> GetResp["DocumentResponse or 404"]

    Delete["DELETE /documents/:id"] --> Remove["SQLite delete_document"]
    Remove --> DeleteResp["204 or 404"]
```

## Runtime data

```mermaid
flowchart LR
    Env[".env / shell env"] --> Paths["runtime paths"]
    Paths --> SQLitePath["SQLITE_PATH<br/>app-data/govdoc.sqlite3"]
    Paths --> IndexPath["HNSW_INDEX_PATH<br/>app-data/hnsw.index"]
    Paths --> TemplateDir["GOVDOC_TEMPLATES_DIR<br/>app-data/templates"]

    SQLitePath --> Memory["gov_doc_memory"]
    SQLitePath --> Templates["doc_template"]
    SQLitePath --> Documents["document"]
    IndexPath --> Cache["derived vector cache"]
    TemplateDir --> DocxTemplates["uploaded .docx templates"]
```

หมายเหตุ: runtime data ควรถูก ignore จาก git เพราะเป็น state ของเครื่องผู้ใช้ ไม่ใช่ source code.
