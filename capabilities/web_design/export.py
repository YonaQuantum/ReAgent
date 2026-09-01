#!/usr/bin/env python3
"""export_page: finalize the webpage — keep the HTML and capture a full-page
screenshot. A website's deliverable is the HTML, not a print PDF."""

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
    rules = engine.load_web_rules(CAP_DIR)
    viewport = rules.get("viewport", {})
    width_px = int(request.get("width_px", viewport.get("width_px", 1440)))
    height_px = int(request.get("height_px", 2400))

    entry = (workspace / entry_name).resolve()
    engine.seed_if_missing(CAP_DIR, entry)
    if not entry.is_file():
        print(json.dumps({"exported": False, "error": f"entry {entry_name} not found"}))
        return 1

    png = engine.export_png(entry, width_px, height_px, workspace)

    artifacts = []
    if entry.is_file():
        artifacts.append({"kind": "html", "path": str(entry), "mime": "text/html"})
    if png.get("png"):
        artifacts.append({"kind": "png", "path": png["png"], "mime": "image/png"})

    exported = any(a["kind"] == "html" for a in artifacts)
    print(
        json.dumps(
            {
                "exported": exported,
                "html": str(entry) if entry.is_file() else None,
                "png": png.get("png"),
                "error": png.get("error"),
                "artifacts": artifacts,
            },
            ensure_ascii=False,
        )
    )
    return 0 if exported else 1


if __name__ == "__main__":
    raise SystemExit(main())
