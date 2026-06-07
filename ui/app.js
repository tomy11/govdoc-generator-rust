// Talks to the govdoc-api sidecar over HTTP. CORS on the API allows this from
// the Tauri webview origin.
const API = "http://127.0.0.1:8000";

let lastDoc = null; // { id?, doc_type, doc_data, title } for save/render/edit buttons
let expandedDocumentId = null;
const resultPanel = document.getElementById("result-section");

async function loadStatus() {
  const el = document.getElementById("status");
  try {
    const s = await (await fetch(`${API}/status`)).json();
    const llm = s.llm.ready ? s.llm.backend : `${s.llm.backend} (ยังไม่พร้อม)`;
    el.textContent = `LLM: ${llm} · embedding: ${s.embedding.backend} · OCR: ${s.ocr.ready ? "พร้อม" : "ปิด"}`;
    el.classList.toggle("error", !s.llm.ready);
  } catch {
    el.textContent = "เชื่อมต่อ backend ไม่ได้ (127.0.0.1:8000)";
    el.classList.add("error");
  }
}

function formToRequest(form) {
  const fd = new FormData(form);
  const req = {};
  for (const [k, v] of fd.entries()) {
    if (k === "use_critic") continue;
    if (String(v).trim() !== "") req[k] = v;
  }
  req.use_critic = form.use_critic.checked;
  return req;
}

const FIELD_LABELS = {
  number: "เลขที่",
  reference_number: "อ้างถึง",
  agency: "ส่วนราชการ",
  date: "วันที่",
  subject: "เรื่อง",
  title: "เรื่อง",
  recipient: "เรียน",
  salutation: "คำขึ้นต้น",
  closing: "คำลงท้าย",
  signer_name: "ลงชื่อ",
  signer_position: "ตำแหน่ง",
};

function renderDoc(doc) {
  const box = document.createElement("div");
  for (const [key, label] of Object.entries(FIELD_LABELS)) {
    if (doc[key] == null || doc[key] === "") continue;
    const f = document.createElement("div");
    f.className = "doc-field";
    f.innerHTML = `<div class="k">${label}</div><div class="v"></div>`;
    f.querySelector(".v").textContent = doc[key];
    box.appendChild(f);
  }
  if (Array.isArray(doc.body) && doc.body.length) {
    const body = document.createElement("div");
    body.className = "doc-field doc-body";
    body.innerHTML = `<div class="k">เนื้อความ</div>`;
    for (const p of doc.body) {
      const para = document.createElement("p");
      para.textContent = p;
      body.appendChild(para);
    }
    box.appendChild(body);
  }
  return box;
}

function openCreateModal() {
  const modal = document.getElementById("create-modal");
  if (typeof modal.showModal === "function") {
    modal.showModal();
  } else {
    modal.setAttribute("open", "");
  }
}

function closeCreateModal() {
  const modal = document.getElementById("create-modal");
  if (modal.open) modal.close();
}

function placeResultPanel({ afterRow = null } = {}) {
  if (afterRow) {
    let detail = afterRow.nextElementSibling;
    if (!detail?.classList.contains("doc-detail-row")) {
      detail = document.createElement("li");
      detail.className = "doc-detail-row";
      afterRow.after(detail);
    }
    detail.replaceChildren(resultPanel);
  } else {
    document.getElementById("draft-slot").replaceChildren(resultPanel);
  }
  resultPanel.hidden = false;
}

function parkResultPanelBeforeListRefresh() {
  if (resultPanel.parentElement?.classList.contains("doc-detail-row")) {
    resultPanel.hidden = true;
    document.getElementById("draft-slot").replaceChildren(resultPanel);
  }
}

function setCurrentDoc(
  doc,
  { id = null, doc_type, title = "", showTrace = false, editing = true, afterRow = null } = {},
) {
  placeResultPanel({ afterRow });
  const result = document.getElementById("result");
  result.className = "result";
  result.replaceChildren(renderDoc(doc));

  lastDoc = { id, doc_type, doc_data: doc, title };
  document.getElementById("render-btn").disabled = false;
  document.getElementById("save-btn").disabled = false;
  document.getElementById("overwrite-btn").disabled = id == null;
  const editWrap = document.getElementById("edit-wrap");
  editWrap.hidden = !editing;
  editWrap.open = editing;
  document.getElementById("doc-json-editor").value = JSON.stringify(doc, null, 2);
  document.getElementById("edit-msg").className = "action-msg muted small";
  document.getElementById("edit-msg").textContent =
    id == null ? "เอกสารนี้ยังไม่ถูกบันทึก ถ้าต้องการบันทึกทับ ให้บันทึกเก็บไว้ก่อน" : `กำลังแก้เอกสาร #${id}`;
  if (!showTrace) document.getElementById("trace-wrap").hidden = true;
}

document.getElementById("new-doc-btn").addEventListener("click", () => {
  lastDoc = null;
  document.getElementById("gen-form").reset();
  document.getElementById("result").className = "result muted";
  document.getElementById("result").textContent = "ยังไม่มีผลลัพธ์ — กรอกฟอร์มแล้วกด “สร้างหนังสือ”";
  document.getElementById("render-msg").textContent = "";
  document.getElementById("edit-wrap").hidden = true;
  document.getElementById("trace-wrap").hidden = true;
  document.getElementById("render-btn").disabled = true;
  document.getElementById("save-btn").disabled = true;
  openCreateModal();
});

document.getElementById("close-create-btn").addEventListener("click", closeCreateModal);
document.getElementById("create-modal").addEventListener("click", (e) => {
  if (e.target.id === "create-modal") closeCreateModal();
});

document.getElementById("gen-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const form = e.target;
  const btn = document.getElementById("gen-btn");
  const result = document.getElementById("result");
  const traceWrap = document.getElementById("trace-wrap");
  const renderBtn = document.getElementById("render-btn");

  const saveBtn = document.getElementById("save-btn");
  btn.disabled = true;
  btn.textContent = "กำลังสร้าง…";
  result.className = "result muted";
  result.textContent = "กำลังเรียกโมเดล…";
  renderBtn.disabled = true;
  saveBtn.disabled = true;

  try {
    const req = formToRequest(form);
    const res = await fetch(`${API}/generate`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(req),
    });
    const data = await res.json();
    if (!res.ok) throw new Error(data.detail || "เกิดข้อผิดพลาด");

    document.getElementById("trace").textContent = JSON.stringify(data.trace, null, 2);
    traceWrap.hidden = false;

    setCurrentDoc(data.doc, {
      doc_type: req.doc_type,
      title: req.subject || "",
      showTrace: true,
      editing: true,
    });
    closeCreateModal();
  } catch (err) {
    result.className = "result error";
    result.textContent = `❌ ${err.message}`;
  } finally {
    btn.disabled = false;
    btn.textContent = "สร้างหนังสือ";
  }
});

document.getElementById("render-btn").addEventListener("click", async () => {
  if (!lastDoc) return;
  const btn = document.getElementById("render-btn");
  const msg = document.getElementById("render-msg");
  btn.disabled = true;
  btn.textContent = "กำลังสร้าง .docx…";
  msg.className = "action-msg muted small";
  msg.textContent = "กำลังเรียก /render/save และบันทึกไฟล์ .docx…";
  try {
    const res = await fetch(`${API}/render/save`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(lastDoc),
    });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) throw new Error(data.detail || `render ล้มเหลว (${res.status})`);
    msg.className = "action-msg ok small";
    msg.textContent = `✓ บันทึกไฟล์แล้ว (${Math.round(data.bytes / 1024)} KB): ${data.file_path}`;
  } catch (err) {
    msg.className = "action-msg err small";
    msg.textContent = `❌ ${err.message}`;
    alert(err.message);
  } finally {
    btn.disabled = false;
    btn.textContent = "เป็น .docx";
  }
});

// ---- saved documents -------------------------------------------------------

document.getElementById("save-btn").addEventListener("click", async () => {
  if (!lastDoc) return;
  const btn = document.getElementById("save-btn");
  btn.disabled = true;
  try {
    const res = await fetch(`${API}/documents`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(lastDoc),
    });
    if (!res.ok) {
      const err = await res.json().catch(() => ({}));
      throw new Error(err.detail || `บันทึกล้มเหลว (${res.status})`);
    }
    const saved = await res.json();
    lastDoc.id = saved.id;
    document.getElementById("overwrite-btn").disabled = false;
    document.getElementById("edit-msg").className = "action-msg ok small";
    document.getElementById("edit-msg").textContent = `✓ บันทึกเป็นเอกสาร #${saved.id} แล้ว`;
    btn.textContent = "บันทึกแล้ว ✓";
    setTimeout(() => (btn.textContent = "บันทึกเก็บไว้"), 1500);
    loadDocuments();
  } catch (err) {
    alert(err.message);
  } finally {
    btn.disabled = false;
  }
});

async function openDocument(id, { editing = false, row = null } = {}) {
  try {
    const doc = await (await fetch(`${API}/documents/${id}`)).json();
    expandedDocumentId = doc.id;
    setCurrentDoc(doc.doc_data, {
      id: doc.id,
      doc_type: doc.doc_type,
      title: doc.title || "",
      editing,
      afterRow: row,
    });
  } catch {
    alert("เปิดเอกสารไม่ได้");
  }
}

async function deleteDocument(id) {
  if (!confirm("ลบเอกสารนี้?")) return;
  await fetch(`${API}/documents/${id}`, { method: "DELETE" });
  loadDocuments();
}

async function loadDocuments() {
  const list = document.getElementById("doc-list");
  const empty = document.getElementById("doc-empty");
  try {
    const docs = await (await fetch(`${API}/documents`)).json();
    parkResultPanelBeforeListRefresh();
    if (!docs.length) {
      empty.hidden = false;
      list.hidden = true;
      list.replaceChildren();
      return;
    }
    empty.hidden = true;
    list.hidden = false;
    list.replaceChildren(
      ...docs.map((d) => {
        const li = document.createElement("li");
        li.className = "doc-row";
        li.dataset.id = d.id;
        const title = document.createElement("div");
        title.className = "doc-title";
        const when = (d.created_at || "").replace("T", " ").slice(0, 16);
        title.innerHTML = `${d.title || "(ไม่มีชื่อเรื่อง)"} <span class="meta">· ${d.doc_type} · ${when}</span>`;
        const actions = document.createElement("div");
        actions.className = "doc-actions";
        const open = document.createElement("button");
        open.className = "ghost";
        open.textContent = "เปิด";
        open.addEventListener("click", () => openDocument(d.id, { row: li }));
        const edit = document.createElement("button");
        edit.textContent = "แก้ไข";
        edit.addEventListener("click", () => openDocument(d.id, { editing: true, row: li }));
        const del = document.createElement("button");
        del.className = "del";
        del.textContent = "ลบ";
        del.addEventListener("click", () => deleteDocument(d.id));
        actions.append(open, edit, del);
        li.append(title, actions);
        return li;
      }),
    );
  } catch {
    empty.hidden = true;
    list.hidden = false;
    list.innerHTML = '<li class="muted">โหลดรายการไม่ได้</li>';
  }
}

document.getElementById("refresh-docs").addEventListener("click", loadDocuments);

// ---- document editing ------------------------------------------------------

document.getElementById("apply-json-btn").addEventListener("click", () => {
  if (!lastDoc) return;
  const msg = document.getElementById("edit-msg");
  try {
    const doc = JSON.parse(document.getElementById("doc-json-editor").value);
    setCurrentDoc(doc, {
      id: lastDoc.id ?? null,
      doc_type: lastDoc.doc_type,
      title: doc.subject || doc.title || lastDoc.title || "",
      afterRow: expandedDocumentId ? document.querySelector(`[data-id="${expandedDocumentId}"]`) : null,
    });
    msg.className = "action-msg ok small";
    msg.textContent = "✓ อัปเดต preview จาก JSON แล้ว";
  } catch (err) {
    msg.className = "action-msg err small";
    msg.textContent = `❌ JSON ไม่ถูกต้อง: ${err.message}`;
  }
});

document.getElementById("ai-edit-btn").addEventListener("click", async () => {
  if (!lastDoc) return;
  const instructions = document.getElementById("ai-edit-instructions").value.trim();
  const msg = document.getElementById("edit-msg");
  const btn = document.getElementById("ai-edit-btn");
  if (!instructions) {
    msg.className = "action-msg err small";
    msg.textContent = "❌ กรุณาใส่คำสั่งที่ต้องการแก้";
    return;
  }
  btn.disabled = true;
  msg.className = "action-msg muted small";
  msg.textContent = "กำลังเรียก /edit เพื่อแก้เอกสาร…";
  try {
    const res = await fetch(`${API}/edit`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        doc_type: lastDoc.doc_type,
        doc_data: lastDoc.doc_data,
        edit_instructions: instructions,
      }),
    });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) throw new Error(data.detail || `แก้ด้วย AI ล้มเหลว (${res.status})`);
    setCurrentDoc(data, {
      id: lastDoc.id ?? null,
      doc_type: lastDoc.doc_type,
      title: data.subject || data.title || lastDoc.title || "",
      afterRow: expandedDocumentId ? document.querySelector(`[data-id="${expandedDocumentId}"]`) : null,
    });
    msg.className = "action-msg ok small";
    msg.textContent = "✓ แก้ด้วย AI แล้ว ตรวจ preview หรือ JSON ก่อนบันทึกทับ";
  } catch (err) {
    msg.className = "action-msg err small";
    msg.textContent = `❌ ${err.message}`;
  } finally {
    btn.disabled = false;
  }
});

document.getElementById("overwrite-btn").addEventListener("click", async () => {
  if (!lastDoc?.id) return;
  const msg = document.getElementById("edit-msg");
  const btn = document.getElementById("overwrite-btn");
  btn.disabled = true;
  msg.className = "action-msg muted small";
  msg.textContent = `กำลังบันทึกทับเอกสาร #${lastDoc.id}…`;
  try {
    const res = await fetch(`${API}/documents/${lastDoc.id}`, {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(lastDoc),
    });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) throw new Error(data.detail || `บันทึกทับล้มเหลว (${res.status})`);
    msg.className = "action-msg ok small";
    msg.textContent = `✓ บันทึกทับเอกสาร #${data.id} แล้ว`;
    loadDocuments();
  } catch (err) {
    msg.className = "action-msg err small";
    msg.textContent = `❌ ${err.message}`;
  } finally {
    btn.disabled = false;
  }
});

// ---- uploads (reference example via OCR, and .docx render template) --------

function wireUpload(formId, url, onOk) {
  const form = document.getElementById(formId);
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const msg = form.querySelector(".upload-msg");
    const btn = form.querySelector("button");
    btn.disabled = true;
    msg.className = "upload-msg muted small";
    msg.textContent = "กำลังอัปโหลด…";
    try {
      // FormData sets the multipart boundary; don't set content-type manually.
      const res = await fetch(`${API}${url}`, { method: "POST", body: new FormData(form) });
      const data = await res.json().catch(() => ({}));
      if (!res.ok) throw new Error(data.detail || `อัปโหลดล้มเหลว (${res.status})`);
      msg.className = "upload-msg ok small";
      msg.textContent = onOk(data);
      form.reset();
    } catch (err) {
      msg.className = "upload-msg err small";
      msg.textContent = `❌ ${err.message}`;
    } finally {
      btn.disabled = false;
    }
  });
}

wireUpload("example-form", "/ingest/ocr/upload", (d) => {
  const vec = d.embedded ? "มี vector" : "ไม่มี vector (fallback recency)";
  const shaped = d.structured ? "แยกเป็น schema แล้ว" : "เก็บข้อความดิบ";
  return `✓ เพิ่มตัวอย่าง #${d.id} — ${shaped}, ${vec}`;
});

wireUpload("template-form", "/templates/upload", (d) => `✓ บันทึกแม่แบบ #${d.id} (${d.name})`);

loadStatus();
loadDocuments();
setInterval(loadStatus, 10000);
