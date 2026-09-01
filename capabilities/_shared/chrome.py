"""Generic headless-Chrome helpers shared by every render-capability.

Nothing here knows about print physics (cm/DPI) or web layout (viewport) — it
just drives Chrome and pulls text/DOM back out. Capability workers own the
domain-specific measurement and export on top of these primitives, so a new
capability never copies a whole `render.py`; it reuses these and adds its own
strategy.
"""

from __future__ import annotations

import html
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path


def find_chrome() -> str | None:
    # Names resolvable from PATH first (Linux / CI). Then platform-specific fixed
    # install locations, since Windows/macOS browsers are not on PATH.
    for name in ("google-chrome-stable", "chromium", "chromium-browser", "google-chrome"):
        found = shutil.which(name)
        if found:
            return found

    if os.name == "nt":
        candidates = []
        for base in (
            os.environ.get("ProgramFiles"),
            os.environ.get("ProgramFiles(x86)"),
            os.environ.get("LocalAppData"),
        ):
            if not base:
                continue
            candidates.append(Path(base) / "Google" / "Chrome" / "Application" / "chrome.exe")
            candidates.append(Path(base) / "Microsoft" / "Edge" / "Application" / "msedge.exe")
        return next((str(path) for path in candidates if path.is_file()), None)

    if sys.platform == "darwin":
        candidates = (
            Path("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
            Path("/Applications/Chromium.app/Contents/MacOS/Chromium"),
        )
        return next((str(path) for path in candidates if path.is_file()), None)

    return None


def run_chrome(args: list[str], timeout: int = 60) -> subprocess.CompletedProcess:
    env = os.environ.copy()
    env.setdefault("LANG", "zh_CN.UTF-8")
    return subprocess.run(args, capture_output=True, text=True, env=env, timeout=timeout)


def read_doc(entry: Path) -> str:
    if not entry.is_file():
        raise FileNotFoundError(f"entry file not found: {entry}")
    return entry.read_text(encoding="utf-8", errors="replace")


def load_json(cap_dir: Path, filename: str) -> dict:
    """Load a JSON rules/config file from a capability dir, tolerating absence."""
    path = cap_dir / filename
    if path.is_file():
        try:
            return json.loads(path.read_text(encoding="utf-8"))
        except json.JSONDecodeError:
            pass
    return {}


def extract_node(dumped: str, node_id: str) -> dict:
    """Pull the JSON blob a measurement script wrote into `<pre id=node_id>`."""
    m = re.search(rf'<pre id="{node_id}">(.*?)</pre>', dumped, re.S)
    if not m:
        return {"viewport": {"width": 0, "height": 0}, "issues": [], "_error": "no diagnostics node"}
    try:
        return json.loads(html.unescape(m.group(1)))
    except json.JSONDecodeError:
        return {"viewport": {"width": 0, "height": 0}, "issues": [], "_error": "bad diagnostics json"}
