// Talks to the govdoc-api sidecar over HTTP. CORS on the API allows this from
// the Tauri webview origin.
const API = "http://127.0.0.1:8000";

let lastDoc = null; // { id?, doc_type, doc_data, title } for save/render/edit buttons
let expandedDocumentId = null;
const resultPanel = document.getElementById("result-section");
let currentGeneralDoc = null;
let currentGeneralPage = 1;
const generalBlocksByPage = new Map();
let selectedGeneralBlock = null;

function switchMenu(target) {
  document.querySelectorAll("[data-menu-view]").forEach((section) => {
    section.classList.toggle("menu-hidden", section.dataset.menuView !== target);
  });
  document.querySelectorAll("[data-menu-target]").forEach((button) => {
    button.classList.toggle("active", button.dataset.menuTarget === target);
  });
  if (target === "general") {
    loadGeneralDocuments();
  } else {
    loadDocuments();
  }
}

document.querySelectorAll("[data-menu-target]").forEach((button) => {
  button.addEventListener("click", () => switchMenu(button.dataset.menuTarget));
});

window.addEventListener("resize", () => {
  if (!currentGeneralDoc) return;
  const page = currentGeneralDoc.pages.find((p) => p.page_number === currentGeneralPage);
  const blocks = generalBlocksByPage.get(currentGeneralPage) || [];
  renderGeneralBlockOverlay(page, blocks, selectedGeneralBlock);
});

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

// ---- general documents -----------------------------------------------------

document.getElementById("general-upload-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const form = e.target;
  const msg = form.querySelector(".upload-msg");
  const btn = form.querySelector("button");
  btn.disabled = true;
  msg.className = "upload-msg muted small";
  msg.textContent = "กำลังอัปโหลดเอกสารทั่วไป…";
  try {
    const res = await fetch(`${API}/general-documents/upload`, {
      method: "POST",
      body: new FormData(form),
    });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) throw new Error(data.detail || `อัปโหลดล้มเหลว (${res.status})`);
    msg.className = "upload-msg ok small";
    msg.textContent = `✓ อัปโหลดแล้ว #${data.id}`;
    form.reset();
    await loadGeneralDocuments();
    await openGeneralDocument(data.id);
  } catch (err) {
    msg.className = "upload-msg err small";
    msg.textContent = `❌ ${err.message}`;
  } finally {
    btn.disabled = false;
  }
});

async function loadGeneralDocuments() {
  const list = document.getElementById("general-doc-list");
  try {
    const docs = await (await fetch(`${API}/general-documents`)).json();
    if (!docs.length) {
      list.innerHTML = '<li class="muted">ยังไม่มีเอกสารทั่วไป</li>';
      return;
    }
    list.replaceChildren(
      ...docs.map((d) => {
        const li = document.createElement("li");
        const title = document.createElement("div");
        title.className = "doc-title";
        title.innerHTML = `${d.filename} <span class="meta">· ${d.page_count} หน้า · ${d.status} · ${(d.updated_at || "").slice(0, 16)}</span>`;
        const actions = document.createElement("div");
        actions.className = "doc-actions";
        const open = document.createElement("button");
        open.className = "ghost";
        open.textContent = "เปิด";
        open.addEventListener("click", () => openGeneralDocument(d.id));
        const ocr = document.createElement("button");
        ocr.textContent = "OCR ใหม่";
        ocr.addEventListener("click", () => runGeneralOcr(d.id));
        actions.append(open, ocr);
        li.append(title, actions);
        return li;
      }),
    );
  } catch {
    list.innerHTML = '<li class="muted">โหลดเอกสารทั่วไปไม่ได้</li>';
  }
}

async function openGeneralDocument(id) {
  const doc = await (await fetch(`${API}/general-documents/${id}`)).json();
  currentGeneralDoc = doc;
  currentGeneralPage = doc.pages[0]?.page_number || 1;
  generalBlocksByPage.clear();
  selectedGeneralBlock = null;
  document.getElementById("general-doc-detail").hidden = false;
  document.getElementById("general-detail-title").textContent = `${doc.filename} (${doc.page_count} หน้า)`;
  document.getElementById("general-search-results").hidden = true;
  document.getElementById("general-search-results").replaceChildren();
  renderGeneralPageList();
  showGeneralPage(currentGeneralPage);
}

function renderGeneralPageList() {
  const list = document.getElementById("general-page-list");
  list.replaceChildren(
    ...currentGeneralDoc.pages.map((p) => {
      const li = document.createElement("li");
      li.textContent = `หน้า ${p.page_number} · ${p.status}`;
      li.className = p.page_number === currentGeneralPage ? "selected" : "";
      li.addEventListener("click", () => {
        currentGeneralPage = p.page_number;
        selectedGeneralBlock = null;
        renderGeneralPageList();
        showGeneralPage(currentGeneralPage);
      });
      return li;
    }),
  );
}

async function showGeneralPage(pageNumber, selectedBlock = null) {
  const page = currentGeneralDoc.pages.find((p) => p.page_number === pageNumber);
  if (selectedBlock) selectedGeneralBlock = selectedBlock;
  document.getElementById("general-page-text").value =
    page?.edited_text || page?.ocr_text || page?.error || "ยังไม่มีข้อความ OCR";
  const msg = document.getElementById("general-action-msg");
  if (page?.layout_warning) {
    msg.className = "action-msg muted small";
    msg.textContent = page.layout_warning;
  }
  renderGeneralPreviewShell(page);
  const blocks = await loadGeneralPageBlocks(pageNumber);
  renderGeneralBlockOverlay(page, blocks, selectedGeneralBlock);
}

async function loadGeneralPageBlocks(pageNumber) {
  if (!currentGeneralDoc) return [];
  if (generalBlocksByPage.has(pageNumber)) return generalBlocksByPage.get(pageNumber);
  try {
    const res = await fetch(`${API}/general-documents/${currentGeneralDoc.id}/pages/${pageNumber}/blocks`);
    if (!res.ok) throw new Error(String(res.status));
    const blocks = await res.json();
    generalBlocksByPage.set(pageNumber, blocks);
    return blocks;
  } catch {
    generalBlocksByPage.set(pageNumber, []);
    return [];
  }
}

function renderGeneralPreviewShell(page) {
  const title = document.getElementById("general-preview-title");
  const meta = document.getElementById("general-preview-meta");
  const img = document.getElementById("general-page-image");
  const empty = document.getElementById("general-preview-empty");
  const overlay = document.getElementById("general-block-overlay");
  overlay.replaceChildren();
  overlay.style.width = "";
  overlay.style.height = "";
  title.textContent = page ? `หน้า ${page.page_number}` : "ตัวอย่างหน้า";
  meta.textContent = page?.page_width && page?.page_height ? `${page.page_width}×${page.page_height}` : "";
  if (!page?.page_image_path) {
    img.hidden = true;
    img.removeAttribute("src");
    empty.hidden = false;
    return;
  }
  empty.hidden = true;
  img.hidden = false;
  img.src = `${API}/general-documents/${currentGeneralDoc.id}/pages/${page.page_number}/image?v=${encodeURIComponent(page.updated_at || "")}`;
  img.onload = () => {
    const blocks = generalBlocksByPage.get(page.page_number) || [];
    renderGeneralBlockOverlay(page, blocks, selectedGeneralBlock);
  };
  img.onerror = () => {
    img.hidden = true;
    empty.hidden = false;
  };
}

function renderGeneralBlockOverlay(page, blocks, selectedBlock) {
  const overlay = document.getElementById("general-block-overlay");
  const img = document.getElementById("general-page-image");
  overlay.replaceChildren();
  if (!page || img.hidden || !img.complete || !img.naturalWidth) return;
  overlay.style.width = `${img.clientWidth}px`;
  overlay.style.height = `${img.clientHeight}px`;
  const pageWidth = page.page_width || img.naturalWidth;
  const pageHeight = page.page_height || img.naturalHeight;
  for (const block of blocks) {
    const box = normalizeBbox(block.bbox, pageWidth, pageHeight);
    if (!box) continue;
    const el = document.createElement("div");
    el.className = "block-box";
    if (selectedBlock && block.block_index === selectedBlock.block_index && block.page_number === selectedBlock.page_number) {
      el.classList.add("active");
    }
    el.style.left = `${box.left}%`;
    el.style.top = `${box.top}%`;
    el.style.width = `${box.width}%`;
    el.style.height = `${box.height}%`;
    el.title = `${block.block_type} #${block.block_index}`;
    overlay.appendChild(el);
  }
}

function normalizeBbox(bbox, pageWidth, pageHeight) {
  if (!bbox || !pageWidth || !pageHeight) return null;
  let x;
  let y;
  let width;
  let height;
  if (Array.isArray(bbox) && bbox.length >= 4) {
    x = Number(bbox[0]);
    y = Number(bbox[1]);
    const third = Number(bbox[2]);
    const fourth = Number(bbox[3]);
    width = third > x ? third - x : third;
    height = fourth > y ? fourth - y : fourth;
  } else if (typeof bbox === "object") {
    x = Number(bbox.x ?? bbox.left ?? bbox.x1 ?? 0);
    y = Number(bbox.y ?? bbox.top ?? bbox.y1 ?? 0);
    if (bbox.width != null || bbox.height != null) {
      width = Number(bbox.width ?? 0);
      height = Number(bbox.height ?? 0);
    } else {
      const right = Number(bbox.right ?? bbox.x2 ?? 0);
      const bottom = Number(bbox.bottom ?? bbox.y2 ?? 0);
      width = right - x;
      height = bottom - y;
    }
  }
  if (![x, y, width, height].every(Number.isFinite) || width <= 0 || height <= 0) return null;
  return {
    left: clampPercent((x / pageWidth) * 100),
    top: clampPercent((y / pageHeight) * 100),
    width: clampPercent((width / pageWidth) * 100),
    height: clampPercent((height / pageHeight) * 100),
  };
}

function clampPercent(value) {
  return Math.max(0, Math.min(100, value));
}

function parseGeneralPageRange(raw) {
  const value = raw.trim();
  if (!value) return {};
  const match = value.match(/^(\d+)(?:\s*-\s*(\d+))?$/);
  if (!match) throw new Error("รูปแบบหน้าต้องเป็นเลขหน้า เช่น 2 หรือ 1-3");
  const start = Number(match[1]);
  const end = Number(match[2] || match[1]);
  if (end < start) throw new Error("ช่วงหน้าต้องเรียงจากน้อยไปมาก");
  return { page_start: start, page_end: end };
}

function renderGeneralSearchResults(hits) {
  const box = document.getElementById("general-search-results");
  if (!hits.length) {
    box.hidden = false;
    box.textContent = "ไม่พบ block ที่ตรงกับคำค้น";
    return;
  }
  box.hidden = false;
  box.replaceChildren(
    ...hits.map((hit) => {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "search-hit";
      const meta = document.createElement("div");
      meta.className = "meta";
      meta.textContent = `หน้า ${hit.block.page_number} · ${hit.block.block_type} · ${hit.score.toFixed(2)}`;
      const snippet = document.createElement("div");
      snippet.className = "snippet";
      snippet.textContent = (hit.block.text || hit.block.image_path || "").slice(0, 220);
      btn.append(meta, snippet);
      btn.addEventListener("click", () => {
        currentGeneralPage = hit.block.page_number;
        selectedGeneralBlock = hit.block;
        renderGeneralPageList();
        showGeneralPage(currentGeneralPage, hit.block);
        document.getElementById("general-action-msg").className = "action-msg ok small";
        document.getElementById("general-action-msg").textContent =
          `เลือก block #${hit.block.block_index} (${hit.block.block_type})`;
      });
      return btn;
    }),
  );
}

document.getElementById("general-search-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  if (!currentGeneralDoc) return;
  const msg = document.getElementById("general-action-msg");
  const query = document.getElementById("general-search-query").value.trim();
  if (!query) {
    msg.className = "action-msg err small";
    msg.textContent = "❌ กรุณาใส่คำค้นหา";
    return;
  }
  let pageRange;
  try {
    pageRange = parseGeneralPageRange(document.getElementById("general-search-pages").value);
  } catch (err) {
    msg.className = "action-msg err small";
    msg.textContent = `❌ ${err.message}`;
    return;
  }
  msg.className = "action-msg muted small";
  msg.textContent = "กำลังค้นหา block…";
  const blockType = document.getElementById("general-search-type").value;
  const res = await fetch(`${API}/general-documents/${currentGeneralDoc.id}/search`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      query,
      ...pageRange,
      block_type: blockType || null,
      limit: 12,
    }),
  });
  const data = await res.json().catch(() => []);
  if (!res.ok) {
    msg.className = "action-msg err small";
    msg.textContent = `❌ ${data.detail || res.status}`;
    return;
  }
  msg.className = "action-msg ok small";
  msg.textContent = `✓ พบ ${data.length} block`;
  renderGeneralSearchResults(data);
});

async function runGeneralOcr(id = currentGeneralDoc?.id) {
  if (!id) return;
  const msg = document.getElementById("general-action-msg");
  msg.className = "action-msg muted small";
  msg.textContent = "กำลัง OCR รายหน้า อาจใช้เวลาหลายนาที…";
  const res = await fetch(`${API}/general-documents/${id}/ocr`, { method: "POST" });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) {
    msg.className = "action-msg err small";
    msg.textContent = `❌ ${data.detail || res.status}`;
    return;
  }
  msg.className = "action-msg ok small";
  msg.textContent = `✓ OCR เสร็จ สถานะ: ${data.status}`;
  await loadGeneralDocuments();
  await openGeneralDocument(id);
}

async function editGeneralDocument(allPages) {
  if (!currentGeneralDoc) return;
  const instruction = document.getElementById("general-edit-instruction").value.trim();
  const msg = document.getElementById("general-action-msg");
  if (!instruction) {
    msg.className = "action-msg err small";
    msg.textContent = "❌ กรุณาใส่คำสั่งแก้ไข";
    return;
  }
  msg.className = "action-msg muted small";
  msg.textContent = "กำลังแก้ข้อความ…";
  const res = await fetch(`${API}/general-documents/${currentGeneralDoc.id}/edit`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ instruction, page: currentGeneralPage, all_pages: allPages }),
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) {
    msg.className = "action-msg err small";
    msg.textContent = `❌ ${data.detail || res.status}`;
    return;
  }
  msg.className = "action-msg ok small";
  msg.textContent = "✓ แก้ไขแล้ว";
  await openGeneralDocument(currentGeneralDoc.id);
}

async function exportGeneralDocument(kind) {
  if (!currentGeneralDoc) return;
  const msg = document.getElementById("general-action-msg");
  msg.className = "action-msg muted small";
  msg.textContent = `กำลัง export ${kind.toUpperCase()}…`;
  const res = await fetch(`${API}/general-documents/${currentGeneralDoc.id}/export/${kind}`, {
    method: "POST",
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) {
    msg.className = "action-msg err small";
    msg.textContent = `❌ ${data.detail || res.status}`;
    return;
  }
  msg.className = "action-msg ok small";
  msg.textContent = `✓ บันทึกไฟล์แล้ว (${Math.round(data.bytes / 1024)} KB): ${data.file_path}`;
}

document.getElementById("refresh-general-docs").addEventListener("click", loadGeneralDocuments);
document.getElementById("general-ocr-btn").addEventListener("click", () => runGeneralOcr());
document.getElementById("general-edit-page-btn").addEventListener("click", () => editGeneralDocument(false));
document.getElementById("general-edit-all-btn").addEventListener("click", () => editGeneralDocument(true));
document.getElementById("general-export-docx-btn").addEventListener("click", () => exportGeneralDocument("docx"));
document.getElementById("general-export-pdf-btn").addEventListener("click", () => exportGeneralDocument("pdf"));

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
switchMenu("official");
setInterval(loadStatus, 10000);
