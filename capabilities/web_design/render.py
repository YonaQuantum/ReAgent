#!/usr/bin/env python3
"""render_page: render the workspace HTML at a viewport into a preview + web
diagnostics. Reads JSON on stdin, writes JSON on stdout."""

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
    height_px = int(request.get("height_px", viewport.get("height_px", 960)))

    entry = (workspace / entry_name).resolve()
    engine.seed_if_missing(CAP_DIR, entry)
    if not entry.is_file():
        print(json.dumps({"error": f"entry {entry_name} not found and no template to seed"}))
        return 1

    result = engine.measure(entry, width_px, height_px, workspace, rules=rules)

    diagnostics_path = workspace / "diagnostics.json"
    diagnostics_path.write_text(
        json.dumps(result["diagnostics"], ensure_ascii=False, indent=2), encoding="utf-8"
    )

    artifacts = [{"kind": "diagnostics", "path": str(diagnostics_path), "mime": "application/json"}]
    if result["preview"]:
        artifacts.append({"kind": "preview", "path": result["preview"], "mime": "image/png"})

    print(
        json.dumps(
            {
                "preview": result["preview"],
                "diagnostics": result["diagnostics"],
                "summary": result["summary"],
                "artifacts": artifacts,
            },
            ensure_ascii=False,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
