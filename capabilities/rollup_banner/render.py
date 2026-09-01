#!/usr/bin/env python3
"""render_banner: render the workspace HTML into a preview + diagnostics.

Reads a JSON request on stdin, writes a JSON result on stdout. The kernel
injects `artifact_dir`; the model supplies `entry`/`width_cm`/`height_cm`.
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

    rules = engine.load_print_rules(CAP_DIR)
    result = engine.measure(entry, width_cm, height_cm, workspace, rules=rules)

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
