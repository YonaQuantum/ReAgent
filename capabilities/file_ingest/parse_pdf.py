#!/usr/bin/env python3
"""parse_pdf: extract text from a PDF in the workspace.

Reads a JSON request on stdin, writes a JSON result on stdout. The kernel
injects `artifact_dir`; the model supplies `entry` (workspace-relative path).
Text extraction degrades gracefully: poppler's pdftotext first (best CJK
fidelity), then pypdf as a pure-Python fallback.
"""

import json
import shutil
import subprocess
import sys
from pathlib import Path

MAX_BYTES = 16 * 1024


def _pypdf(path: Path) -> tuple[int, str]:
    from pypdf import PdfReader

    reader = PdfReader(str(path))
    pages = len(reader.pages)
    text = "\n".join((page.extract_text() or "") for page in reader.pages)
    return pages, text


def _pdftotext(path: Path) -> tuple[int, str]:
    out = subprocess.run(
        ["pdftotext", "-layout", str(path), "-"],
        capture_output=True,
        text=True,
    )
    if out.returncode != 0:
        raise RuntimeError((out.stderr or "").strip() or "pdftotext failed")
    text = out.stdout
    pages = text.count("\f") + 1
    return pages, text


def extract_text(path: Path) -> tuple[int, str]:
    """Chain of extractors: pdftotext first (best CJK), then pypdf."""
    failures = []
    if shutil.which("pdftotext"):
        try:
            return _pdftotext(path)
        except Exception as exc:
            failures.append(f"pdftotext: {exc}")
    try:
        return _pypdf(path)
    except Exception as exc:  # ImportError or parse error
        failures.append(f"pypdf: {exc}")
    raise RuntimeError("；".join(failures) or "no PDF extractor")


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

    try:
        pages, text = extract_text(entry)
    except RuntimeError as exc:
        hint = "请安装 `pip install pypdf` 或 poppler（pdftotext）。"
        print(
            json.dumps(
                {
                    "ok": False,
                    "error": f"无法解析 PDF：{exc}。{hint}",
                    "path": str(entry),
                },
                ensure_ascii=False,
            )
        )
        return 0

    raw = text.encode("utf-8")
    truncated = len(raw) > MAX_BYTES
    raw = raw[:MAX_BYTES]
    print(
        json.dumps(
            {
                "ok": True,
                "path": str(entry),
                "pages": pages,
                "truncated": truncated,
                "text": raw.decode("utf-8", errors="replace"),
            },
            ensure_ascii=False,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
