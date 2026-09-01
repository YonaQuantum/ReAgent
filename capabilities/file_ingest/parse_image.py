#!/usr/bin/env python3
"""parse_image: OCR an image in the workspace.

Reads a JSON request on stdin, writes a JSON result on stdout. The kernel
injects `artifact_dir`; the model supplies `entry`. Text is extracted with
tesseract (OCR); the `image` field points back at the original so the loop can
feed it to a vision-capable model.
"""

import json
import shutil
import subprocess
import sys
from pathlib import Path

MAX_BYTES = 16 * 1024

# Language configurations to try in order, most specific first. Not every host
# has the Chinese pack installed; fall back to English, then tesseract's default.
LANG_CHAINS = (["chi_sim+eng"], ["eng"], [])


def _ocr(path: Path) -> str:
    last_err = ""
    for langs in LANG_CHAINS:
        cmd = ["tesseract", str(path), "stdout"]
        if langs:
            cmd += ["-l", langs[0]]
        out = subprocess.run(cmd, capture_output=True, text=True)
        if out.returncode == 0:
            return out.stdout or ""
        last_err = (out.stderr or "").strip() or f"tesseract -l {langs[0] if langs else 'default'} failed"
    raise RuntimeError(last_err or "tesseract failed")


def main() -> int:
    request = json.load(sys.stdin)
    workspace = Path(request["artifact_dir"])
    entry_name = request.get("entry", "")
    entry = (workspace / entry_name).resolve() if entry_name else None

    if entry is None or not entry.is_file():
        print(
            json.dumps(
                {
                    "ok": False,
                    "error": f"文件不存在：{entry_name or '(未指定 entry)'}（请确认它在工作区 input/ 目录）",
                },
                ensure_ascii=False,
            )
        )
        return 0

    if not shutil.which("tesseract"):
        print(
            json.dumps(
                {
                    "ok": False,
                    "error": "未安装 tesseract OCR，无法抽取图片文字。请安装 tesseract 及中文语言包 chi_sim。",
                    "path": str(entry),
                    "image": str(entry),
                },
                ensure_ascii=False,
            )
        )
        return 0

    try:
        text = _ocr(entry)
    except RuntimeError as exc:
        print(
            json.dumps(
                {
                    "ok": False,
                    "error": f"OCR 失败：{exc}",
                    "path": str(entry),
                    "image": str(entry),
                },
                ensure_ascii=False,
            )
        )
        return 0

    text = text.strip()
    raw = text.encode("utf-8")
    truncated = len(raw) > MAX_BYTES
    raw = raw[:MAX_BYTES]
    print(
        json.dumps(
            {
                "ok": True,
                "path": str(entry),
                "text": raw.decode("utf-8", errors="replace"),
                "truncated": truncated,
                "image": str(entry),
            },
            ensure_ascii=False,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
