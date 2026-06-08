// General-document flow: upload, the page list + page preview with block
// overlay, OCR, semantic block search, AI editing, and export.

function updateGeneralBlockEditButton() {
  const btn = document.getElementById("general-edit-block-btn");
  btn.disabled = !selectedGeneralBlock || selectedGeneralBlock.page_number !== currentGeneralPage;
}

window.addEventListener("resize", () => {
  if (!currentGeneralDoc) return;
  const page = currentGeneralDoc.pages.find((p) => p.page_number === currentGeneralPage);
  const blocks = generalBlocksByPage.get(currentGeneralPage) || [];
  renderGeneralBlockOverlay(page, blocks, selectedGeneralBlock);
});

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
        const del = document.createElement("button");
        del.className = "del";
        del.textContent = "ลบ";
        del.addEventListener("click", () => deleteGeneralDocument(d.id, d.filename));
        actions.append(open, ocr, del);
        li.append(title, actions);
        return li;
      }),
    );
  } catch {
    list.innerHTML = '<li class="muted">โหลดเอกสารทั่วไปไม่ได้</li>';
  }
}

async function deleteGeneralDocument(id, filename) {
  if (!window.confirm(`ลบเอกสารทั่วไป "${filename}" พร้อมไฟล์ทั้งหมด?`)) return;
  let res = await fetch(`${API}/general-documents/${id}`, { method: "DELETE" });
  if (!res.ok && [404, 405, 501].includes(res.status)) {
    res = await fetch(`${API}/general-documents/${id}/delete`, { method: "POST" });
  }
  if (!res.ok) {
    const data = await res.json().catch(() => ({}));
    alert(data.detail || `ลบไม่สำเร็จ (${res.status})`);
    return;
  }
  if (currentGeneralDoc?.id === id) {
    currentGeneralDoc = null;
    currentGeneralPage = 1;
    generalBlocksByPage.clear();
    selectedGeneralBlock = null;
    document.getElementById("general-doc-detail").hidden = true;
  }
  await loadGeneralDocuments();
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
  updateGeneralBlockEditButton();
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
        updateGeneralBlockEditButton();
      });
      return li;
    }),
  );
}

async function showGeneralPage(pageNumber, selectedBlock = null) {
  const page = currentGeneralDoc.pages.find((p) => p.page_number === pageNumber);
  if (selectedBlock) selectedGeneralBlock = selectedBlock;
  updateGeneralBlockEditButton();
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
        updateGeneralBlockEditButton();
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

async function editGeneralDocument(mode) {
  if (!currentGeneralDoc) return;
  const instruction = document.getElementById("general-edit-instruction").value.trim();
  const msg = document.getElementById("general-action-msg");
  if (!instruction) {
    msg.className = "action-msg err small";
    msg.textContent = "❌ กรุณาใส่คำสั่งแก้ไข";
    return;
  }
  if (mode === "block" && (!selectedGeneralBlock || selectedGeneralBlock.page_number !== currentGeneralPage)) {
    msg.className = "action-msg err small";
    msg.textContent = "❌ กรุณาเลือก block จากผลค้นหาก่อน";
    return;
  }
  msg.className = "action-msg muted small";
  msg.textContent = mode === "block" ? "กำลังแก้ block ที่เลือก…" : "กำลังแก้ข้อความ…";
  const payload = {
    instruction,
    page: currentGeneralPage,
    all_pages: mode === "all",
  };
  if (mode === "block") payload.block_index = selectedGeneralBlock.block_index;
  const res = await fetch(`${API}/general-documents/${currentGeneralDoc.id}/edit`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(payload),
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) {
    msg.className = "action-msg err small";
    msg.textContent = `❌ ${data.detail || res.status}`;
    return;
  }
  msg.className = "action-msg ok small";
  msg.textContent = mode === "block" ? "✓ แก้ block แล้ว" : "✓ แก้ไขแล้ว";
  generalBlocksByPage.delete(currentGeneralPage);
  selectedGeneralBlock = null;
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
document.getElementById("general-edit-page-btn").addEventListener("click", () => editGeneralDocument("page"));
document.getElementById("general-edit-block-btn").addEventListener("click", () => editGeneralDocument("block"));
document.getElementById("general-edit-all-btn").addEventListener("click", () => editGeneralDocument("all"));
document.getElementById("general-export-docx-btn").addEventListener("click", () => exportGeneralDocument("docx"));
document.getElementById("general-export-pdf-btn").addEventListener("click", () => exportGeneralDocument("pdf"));
