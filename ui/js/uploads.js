// Reference-example (OCR) and .docx render-template uploads. Both forms share
// the same multipart-upload wiring.

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
