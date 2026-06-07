// Talks to the govdoc-api sidecar over HTTP. CORS on the API allows this from
// the Tauri webview origin.
const API = "http://127.0.0.1:8000";

let lastDoc = null; // { doc_type, doc_data } for the render button

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

document.getElementById("gen-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const form = e.target;
  const btn = document.getElementById("gen-btn");
  const result = document.getElementById("result");
  const traceWrap = document.getElementById("trace-wrap");
  const renderBtn = document.getElementById("render-btn");

  btn.disabled = true;
  btn.textContent = "กำลังสร้าง…";
  result.className = "result muted";
  result.textContent = "กำลังเรียกโมเดล…";
  renderBtn.disabled = true;

  try {
    const req = formToRequest(form);
    const res = await fetch(`${API}/generate`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(req),
    });
    const data = await res.json();
    if (!res.ok) throw new Error(data.detail || "เกิดข้อผิดพลาด");

    result.className = "result";
    result.replaceChildren(renderDoc(data.doc));
    document.getElementById("trace").textContent = JSON.stringify(data.trace, null, 2);
    traceWrap.hidden = false;

    lastDoc = { doc_type: req.doc_type, doc_data: data.doc };
    renderBtn.disabled = false;
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
  btn.disabled = true;
  try {
    const res = await fetch(`${API}/render`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(lastDoc),
    });
    if (!res.ok) {
      const err = await res.json().catch(() => ({}));
      throw new Error(err.detail || `render ล้มเหลว (${res.status})`);
    }
    const blob = await res.blob();
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = "govdoc.docx";
    a.click();
    URL.revokeObjectURL(url);
  } catch (err) {
    alert(err.message);
  } finally {
    btn.disabled = false;
  }
});

loadStatus();
setInterval(loadStatus, 10000);
