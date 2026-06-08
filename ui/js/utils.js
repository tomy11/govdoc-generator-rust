// Pure-ish helpers shared across the official and general document flows:
// form serialisation, document rendering, and bbox/page-range parsing.

// The Tauri webview (WKWebView on macOS) does not implement the synchronous
// window.confirm()/alert(), so they return falsy without showing anything —
// which silently aborted the delete buttons. Use a native <dialog> instead
// (showModal works in the webview, same as the create-document modal) and
// resolve a Promise with the user's choice.
function confirmDialog(message, { okLabel = "ลบ", okClass = "del" } = {}) {
  return new Promise((resolve) => {
    const dialog = document.createElement("dialog");
    dialog.className = "confirm-dialog";
    dialog.innerHTML = `
      <section class="card modal-card">
        <p class="confirm-message"></p>
        <div class="row between confirm-actions">
          <button type="button" class="ghost" data-confirm="cancel">ยกเลิก</button>
          <button type="button" data-confirm="ok"></button>
        </div>
      </section>`;
    dialog.querySelector(".confirm-message").textContent = message;
    const okBtn = dialog.querySelector('[data-confirm="ok"]');
    okBtn.textContent = okLabel;
    if (okClass) okBtn.className = okClass;
    document.body.appendChild(dialog);

    const finish = (result) => {
      if (dialog.open) dialog.close();
      dialog.remove();
      resolve(result);
    };
    dialog.querySelector('[data-confirm="cancel"]').addEventListener("click", () => finish(false));
    okBtn.addEventListener("click", () => finish(true));
    // Esc / backdrop click count as cancel.
    dialog.addEventListener("cancel", (e) => {
      e.preventDefault();
      finish(false);
    });
    dialog.addEventListener("click", (e) => {
      if (e.target === dialog) finish(false);
    });
    dialog.showModal();
  });
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
