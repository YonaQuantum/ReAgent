#!/usr/bin/env python3
"""lint_page: deterministic web-layout checks. `passed` is false while any
issue remains — it's feedback for the model, not a tool crash."""

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
    if not entry.is_file():
        print(
            json.dumps(
                {
                    "passed": False,
                    "issues": [{"type": "missing_entry", "message": f"{entry_name} 不存在"}],
                    "summary": "入口文件缺失，请先 write_file 创建 HTML。",
                },
                ensure_ascii=False,
            )
        )
        return 0

    result = engine.measure(entry, width_px, height_px, workspace, rules=rules)
    issues = list(result["diagnostics"].get("issues") or [])
    passed = len(issues) == 0

    print(
        json.dumps(
            {
                "passed": passed,
                "issues": issues,
                "summary": result["summary"],
                "diagnostics": result["diagnostics"],
            },
            ensure_ascii=False,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
