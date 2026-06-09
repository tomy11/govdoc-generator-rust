#!/usr/bin/env python3
"""Extract per-line text + bounding boxes from a born-digital PDF using PyMuPDF.

This gives the layout coordinates that Typhoon OCR does not return, so the UI can
draw block overlays on the page image. Boxes are emitted in the SAME pixel space
as the rendered page images (page-NNN.png in <pages_dir>) so the frontend's
normaliseBbox(box, page_width || img.naturalWidth) lines up exactly.

Pages with no text layer (scans) yield no lines (caller keeps its OCR path).

Usage: pdf_layout_sidecar.py <source.pdf> <pages_dir>
Output (stdout): {"pages": {"1": [{"text","bbox":[x0,y0,x1,y1],"type"}, ...]}}
"""
import json
import os
import sys

import fitz


def page_image_scale(pages_dir, page_no, page_rect):
    """px-per-point scale matching the rendered page image; default 2.0 (the
    Matrix(2,2) used by render_pdf_pages_with_fitz) when the image is absent."""
    png = os.path.join(pages_dir, f"page-{page_no:03}.png")
    if os.path.exists(png):
        try:
            from PIL import Image

            with Image.open(png) as im:
                iw, ih = im.width, im.height
            return iw / page_rect.width, ih / page_rect.height
        except Exception:  # noqa: BLE001
            pass
    return 2.0, 2.0


def main():
    src, pages_dir = sys.argv[1], sys.argv[2]
    doc = fitz.open(src)
    out = {}
    for i in range(doc.page_count):
        page = doc.load_page(i)
        sx, sy = page_image_scale(pages_dir, i + 1, page.rect)
        data = page.get_text("dict")
        # body font size = most common rounded span size, for heading detection
        sizes = {}
        for blk in data.get("blocks", []):
            for line in blk.get("lines", []):
                for span in line.get("spans", []):
                    s = round(span.get("size", 0))
                    sizes[s] = sizes.get(s, 0) + len(span.get("text", ""))
        body = max(sizes, key=sizes.get) if sizes else 0

        lines = []
        for blk in data.get("blocks", []):
            for line in blk.get("lines", []):
                spans = line.get("spans", [])
                text = "".join(sp.get("text", "") for sp in spans).strip()
                if not text:
                    continue
                x0, y0, x1, y1 = line["bbox"]
                max_size = max((sp.get("size", 0) for sp in spans), default=0)
                kind = "heading" if body and max_size >= body * 1.3 else "text"
                lines.append({
                    "text": text,
                    "bbox": [round(x0 * sx, 1), round(y0 * sy, 1),
                             round(x1 * sx, 1), round(y1 * sy, 1)],
                    "type": kind,
                })
        if lines:
            out[str(i + 1)] = lines
    print(json.dumps({"pages": out}, ensure_ascii=False))


if __name__ == "__main__":
    main()
