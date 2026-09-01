#!/usr/bin/env python3
"""lint_banner: deterministic layout checks against the print rules.

Returns `passed` plus a list of issues. This is a *successful* tool run even
when `passed` is false — the issues are feedback the model should act on, not a
tool crash.
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

    rules = engine.load_print_rules(CAP_DIR)

    result = engine.measure(entry, width_cm, height_cm, workspace, rules=rules)
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
