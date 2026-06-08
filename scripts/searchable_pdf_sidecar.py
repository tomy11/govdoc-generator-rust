#!/usr/bin/env python3
"""Build a searchable PDF from a source PDF: vector passthrough + OCR fallback.

For each page:
  - if it already has a real text layer  -> keep the ORIGINAL vector page
    untouched (crisp text + images, no rasterise, no OCR);
  - if it is a scan/graphic page (little/no text) -> keep the original page (so
    the image is preserved) and OCR an INVISIBLE text layer on top so the page
    becomes searchable/selectable.

Born-digital documents therefore stay pixel-perfect and tiny; only genuine scans
pay the OCR cost.

Usage:  searchable_pdf_sidecar.py <source.pdf> <out.pdf> [lang] [font_path]
Prints a JSON summary on stdout: {"pages":N,"passthrough":N,"ocr":N}
Requires PyMuPDF (fitz); OCR pages additionally require Tesseract + tessdata.
"""
import json
import os
import sys

import fitz

MIN_CHARS = 8  # a page with fewer real characters than this is treated as a scan


def detect_tessdata():
    env = os.environ.get("TESSDATA_PREFIX")
    if env and os.path.isdir(env):
        return env
    for cand in (
        "/opt/homebrew/share/tessdata",
        "/usr/local/share/tessdata",
        "/usr/share/tesseract-ocr/5/tessdata",
        "/usr/share/tessdata",
    ):
        if os.path.isdir(cand):
            return cand
    return None


def main():
    src, out = sys.argv[1], sys.argv[2]
    lang = sys.argv[3] if len(sys.argv) > 3 and sys.argv[3] else "tha+eng"
    font = sys.argv[4] if len(sys.argv) > 4 and sys.argv[4] else os.path.join(
        os.path.dirname(os.path.abspath(__file__)), "fonts", "thai.ttf"
    )
    font = font if font and os.path.exists(font) else None
    tessdata = detect_tessdata()

    src_doc = fitz.open(src)
    out_doc = fitz.open()
    stats = {"pages": src_doc.page_count, "passthrough": 0, "ocr": 0, "ocr_failed": 0}

    for i in range(src_doc.page_count):
        out_doc.insert_pdf(src_doc, from_page=i, to_page=i)  # original page, untouched
        page = src_doc.load_page(i)
        if len(page.get_text("text").strip()) >= MIN_CHARS:
            stats["passthrough"] += 1
            continue

        # scan/graphic page -> OCR an invisible text layer onto the copied page
        words = []
        try:
            tp = page.get_textpage_ocr(flags=0, language=lang, dpi=200, full=True,
                                       tessdata=tessdata)
            words = page.get_text("words", textpage=tp)
        except Exception as exc:  # noqa: BLE001
            sys.stderr.write(f"OCR failed on page {i + 1}: {exc}\n")
            stats["ocr_failed"] += 1

        if not words:
            stats["passthrough"] += 1
            continue

        newpage = out_doc[out_doc.page_count - 1]
        fontname = None
        if font:
            try:
                newpage.insert_font(fontname="thaiocr", fontfile=font)
                fontname = "thaiocr"
            except Exception:  # noqa: BLE001
                fontname = None
        placed = 0
        for w in words:
            x0, y0, x1, y1, word = w[0], w[1], w[2], w[3], w[4]
            if not word.strip():
                continue
            fs = max(1.0, (y1 - y0) * 0.85)
            try:
                newpage.insert_text((x0, y1 - (y1 - y0) * 0.18), word, fontsize=fs,
                                    fontname=fontname or "helv", render_mode=3)
                placed += 1
            except Exception:  # noqa: BLE001 - e.g. glyph missing without a Thai font
                pass
        stats["ocr"] += 1 if placed else 0
        stats["passthrough"] += 0 if placed else 1

    out_doc.save(out, deflate=True, garbage=4)
    out_doc.close()
    src_doc.close()
    print(json.dumps(stats))


if __name__ == "__main__":
    main()
