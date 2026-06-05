#!/usr/bin/env python3
"""Render GovDoc JSON to DOCX bytes through the Python docxtpl renderer.

Input is JSON on stdin:
{
  "doc_type": "ภายนอก",
  "doc_data": {...},
  "template_path": "optional/template.docx",
  "python_source": "/path/to/govdoc-generator"
}

The rendered .docx bytes are written to stdout. Diagnostics go to stderr.
"""

from __future__ import annotations

import json
import sys
import tempfile
from pathlib import Path


def main() -> int:
    payload = json.load(sys.stdin)
    python_source = payload.get("python_source")
    if python_source:
        sys.path.insert(0, str(Path(python_source)))

    try:
        from src.adapters.docx.renderer import render_docx
        from src.domain.schemas import AnnouncementDoc, ExternalDoc, InternalDoc, OrderDoc
    except Exception as exc:  # pragma: no cover - depends on local Python env
        print(f"failed to import Python renderer: {exc}", file=sys.stderr)
        return 2

    doc_type = payload["doc_type"]
    doc_data = payload["doc_data"]
    template_path = payload.get("template_path")

    model_map = {
        "ภายนอก": ExternalDoc,
        "ภายใน": InternalDoc,
        "คำสั่ง": OrderDoc,
        "ประกาศ": AnnouncementDoc,
    }
    model_cls = model_map.get(doc_type)
    if model_cls is None:
        print(f"unknown doc_type: {doc_type}", file=sys.stderr)
        return 3

    try:
        doc = model_cls(**doc_data)
        template_file = Path(template_path).read_bytes() if template_path else None
        with tempfile.NamedTemporaryFile(suffix=".docx") as tmp:
            render_docx(doc, tmp.name, template_file=template_file)
            sys.stdout.buffer.write(Path(tmp.name).read_bytes())
        return 0
    except Exception as exc:  # pragma: no cover - depends on renderer/templates
        print(f"failed to render DOCX: {exc}", file=sys.stderr)
        return 4


if __name__ == "__main__":
    raise SystemExit(main())

