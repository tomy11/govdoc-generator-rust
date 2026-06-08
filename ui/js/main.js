// Top-level wiring: the menu switcher, backend status polling, and bootstrap.
// This file must load last — it kicks off the initial render.

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

loadStatus();
switchMenu("official");
setInterval(loadStatus, 10000);
