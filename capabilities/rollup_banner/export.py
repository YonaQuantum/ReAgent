#!/usr/bin/env python3
"""export_banner: produce the final printable PDF and a high-res PNG.

Only handles physical output (page size, bleed, export). It makes no design
decisions — the model's HTML/CSS is already the design.
"""

import json
import sys
from pathlib import Path

import layout_engine as engine

CAP_DIR = Path(__file__).resolve().parent


def main() -> int:
    request = json.load(sys.stdin)
    workspace = Path(request["artifact_dir"])
    workspace.mkdir(parents=True, exist_ok=True)

    entry_name = request.get("entry", "index.html")
    width_cm = float(request.get("width_cm", 80))
    height_cm = float(request.get("height_cm", 200))

    entry = (workspace / entry_name).resolve()
    engine.seed_if_missing(CAP_DIR, entry, width_cm, height_cm)
    if not entry.is_file():
        print(json.dumps({"error": f"entry {entry_name} not found and no template to seed"}))
        return 1

    pdf = engine.export_pdf(entry, width_cm, height_cm, workspace)
    png = engine.export_png(entry, width_cm, height_cm, workspace)

    artifacts = []
    if pdf.get("pdf"):
        artifacts.append({
            "kind": "pdf",
            "path": pdf["pdf"],
            "mime": "application/pdf",
            "engine": pdf.get("engine"),
            "width_cm": width_cm,
            "height_cm": height_cm,
        })
    if png.get("png"):
        artifacts.append({"kind": "png", "path": png["png"], "mime": "image/png"})

    exported = any(a["kind"] == "pdf" for a in artifacts)
    print(
        json.dumps(
            {
                "exported": exported,
                "pdf": pdf.get("pdf"),
                "png": png.get("png"),
                "engine": pdf.get("engine"),
                "error": pdf.get("error") or png.get("error"),
                "artifacts": artifacts,
            },
            ensure_ascii=False,
        )
    )
    return 0 if exported else 1


if __name__ == "__main__":
    raise SystemExit(main())
