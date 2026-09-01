"""Shared rendering / measurement engine for the web_design tools.

Web pages are measured in *viewport pixels*, not print centimetres. The checks
are the things that make a real homepage feel right: no horizontal overflow,
readable text, images that load, and no emoji (SVG icons instead).
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

_SHARED = Path(__file__).resolve().parent.parent
if str(_SHARED) not in sys.path:
    sys.path.insert(0, str(_SHARED))

from _shared.chrome import extract_node, find_chrome, load_json, read_doc, run_chrome  # noqa: E402

PROBE_ID = "__reagent_web_diagnostics"


def _thresholds(rules: dict) -> dict:
    return {
        "min_font_px": float(rules.get("min_font_px", 12)),
        "allow_emoji": bool(rules.get("allow_emoji", False)),
    }


def _measurement_script(width_px: int, height_px: int, rules: dict) -> str:
    """JS that walks the DOM and reports objective web-layout problems."""
    rules_js = json.dumps(_thresholds(rules))
    return f"""
(function () {{
  var W = {width_px}, H = {height_px};
  var R = {rules_js};
  var issues = [];
  function path(el) {{
    if (el.id) return '#' + el.id;
    var p = [];
    while (el && el !== document.body) {{
      var n = el.tagName.toLowerCase();
      if (el.id) {{ p.unshift('#' + el.id); break; }}
      var s = el.parentElement ? Array.prototype.indexOf.call(el.parentElement.children, el) + 1 : 0;
      p.unshift(n + ':nth-child(' + s + ')');
      el = el.parentElement;
    }}
    return p.join(' > ');
  }}

  var els = Array.prototype.slice.call(document.querySelectorAll('body *'));

  // horizontal overflow (vertical scroll is normal on the web)
  var overflow = null;
  for (var i = 0; i < els.length; i++) {{
    var el = els[i];
    var r = el.getBoundingClientRect();
    if (r.width === 0 && r.height === 0) continue;
    if (r.right > W + 1 || r.left < -1) {{
      overflow = {{ type: 'overflow', selector: path(el), message:
        '元素横向越界 right=' + Math.round(r.right) + '（视口宽 ' + W + '）——页面出现横向滚动' }};
      break;
    }}
  }}
  if (overflow) issues.push(overflow);

  // readable text floor — measure only elements with direct text; containers
  // inherit the body default (16px) and would otherwise report a bogus floor.
  function hasDirectText(el) {{
    var tag = el.tagName;
    if (tag === 'SCRIPT' || tag === 'STYLE' || tag === 'NOSCRIPT' || tag === 'TEMPLATE') return false;
    for (var i = 0; i < el.childNodes.length; i++) {{
      var n = el.childNodes[i];
      if (n.nodeType === 3 && (n.nodeValue || '').trim()) return true;
    }}
    return false;
  }}
  var minFont = null;
  for (var j = 0; j < els.length; j++) {{
    var e = els[j];
    if (!hasDirectText(e)) continue;
    var fs = parseFloat(getComputedStyle(e).fontSize);
    if (isNaN(fs) || fs <= 0) continue;
    if (minFont === null || fs < minFont.fs) minFont = {{ fs: fs, selector: path(e) }};
  }}
  if (minFont && minFont.fs < R.min_font_px) {{
    issues.push({{ type: 'font_too_small', selector: minFont.selector,
      message: '最小字号 ' + Math.round(minFont.fs) + 'px，低于网页可读下限 ' + R.min_font_px + 'px' }});
  }}

  // emoji: cheap, use SVG icons instead
  var emojiCount = 0;
  if (!R.allow_emoji) {{
    var emojiRe = /[\\u{{1F000}}-\\u{{1FAFF}}\\u{{2600}}-\\u{{27BF}}\\u{{2B00}}-\\u{{2BFF}}\\u{{FE0F}}\\u{{1F1E6}}-\\u{{1F1FF}}]/gu;
    emojiCount = ((document.body.innerText || '').match(emojiRe) || []).length;
    if (emojiCount > 0) {{
      issues.push({{ type: 'emoji_used', selector: 'body',
        message: '检测到 ' + emojiCount + ' 个 emoji——网页用 emoji 显得廉价，改用 SVG 图标' }});
    }}
  }}

  // missing images
  var imgs = document.querySelectorAll('img');
  for (var k = 0; k < imgs.length; k++) {{
    var img = imgs[k];
    if (img.naturalWidth === 0) {{
      issues.push({{ type: 'missing_image', selector: path(img), message: '图片加载失败或缺失' }});
    }}
  }}

  var result = {{
    viewport: {{ width: W, height: H }},
    issues: issues,
    metrics: {{
      min_font_px: minFont ? Math.round(minFont.fs) : 0,
      emoji: emojiCount,
      images: imgs.length
    }}
  }};
  var node = document.createElement('pre');
  node.id = '{PROBE_ID}';
  node.textContent = JSON.stringify(result);
  document.body.appendChild(node);
}})();
"""


def _wrap(doc: str, script: str, style: str) -> str:
    injected = f"<style>{style}</style>{script}"
    if "</body>" in doc:
        return doc.replace("</body>", injected + "</body>", 1)
    return doc + injected


def measure(
    entry: Path,
    width_px: int,
    height_px: int,
    out_dir: Path,
    rules: dict | None = None,
) -> dict:
    """Render the page at a viewport, produce a preview screenshot + diagnostics."""
    chrome = find_chrome()
    if chrome is None:
        return {
            "preview": None,
            "diagnostics": {"viewport": {"width": 0, "height": 0}, "issues": []},
            "summary": "未找到 Chrome，无法渲染预览与诊断。",
        }

    doc = read_doc(entry)

    # Measurement probe at full size.
    probe_doc = _wrap(
        doc,
        f"<script>{_measurement_script(width_px, height_px, rules or {})}</script>",
        "html,body{margin:0;padding:0;}",
    )
    probe_path = out_dir / "_probe.html"
    probe_path.write_text(probe_doc, encoding="utf-8")
    dumped = run_chrome(
        [chrome, "--headless=new", "--disable-gpu", "--no-sandbox",
         "--dump-dom", "--virtual-time-budget=3000",
         f"--window-size={width_px},{height_px}", f"file://{probe_path.resolve()}"]
    )
    diagnostics = extract_node(dumped.stdout, PROBE_ID)

    # Preview screenshot at the same viewport (no measurement node).
    preview_doc = _wrap(doc, "", "html,body{margin:0;padding:0;}")
    preview_probe = out_dir / "_preview.html"
    preview_probe.write_text(preview_doc, encoding="utf-8")
    preview_path = out_dir / "preview.png"
    run_chrome(
        [chrome, "--headless=new", "--disable-gpu", "--no-sandbox",
         "--screenshot=" + str(preview_path), "--hide-scrollbars",
         f"--window-size={width_px},{height_px}", f"file://{preview_probe.resolve()}"]
    )

    summary = summarize(diagnostics)
    return {
        "preview": str(preview_path) if preview_path.is_file() else None,
        "diagnostics": diagnostics,
        "summary": summary,
    }


def summarize(diagnostics: dict) -> str:
    issues = diagnostics.get("issues") or []
    if not issues:
        return "网页正常：无横向溢出、字号可读、图片加载、无 emoji。"
    parts = [f"{i['type']}: {i.get('message', '')}" for i in issues[:5]]
    return "发现 " + str(len(issues)) + " 个问题：" + "；".join(parts)


def export_png(entry: Path, width_px: int, height_px: int, out_dir: Path) -> dict:
    """Take a full-page screenshot of the current page."""
    chrome = find_chrome()
    if chrome is None:
        return {"png": None, "error": "未找到 Chrome"}

    doc = _wrap(read_doc(entry), "", "html,body{margin:0;padding:0;}")
    shot_probe = out_dir / "_export.html"
    shot_probe.write_text(doc, encoding="utf-8")
    png_path = out_dir / "site.png"
    result = run_chrome(
        [chrome, "--headless=new", "--disable-gpu", "--no-sandbox",
         "--screenshot=" + str(png_path), "--hide-scrollbars",
         f"--window-size={width_px},{height_px}", f"file://{shot_probe.resolve()}"]
    )
    if result.returncode != 0 or not png_path.is_file():
        return {"png": None, "error": (result.stderr or "")[:500]}
    return {"png": str(png_path)}


def load_web_rules(cap_dir: Path) -> dict:
    return load_json(cap_dir, "web_rules.json")


def seed_if_missing(cap_dir: Path, entry: Path) -> None:
    """If the model hasn't written an entry file, seed it from the template."""
    if entry.is_file():
        return
    index_src = cap_dir / "template" / "index.html"
    if index_src.is_file():
        entry.parent.mkdir(parents=True, exist_ok=True)
        entry.write_text(index_src.read_text(encoding="utf-8"), encoding="utf-8")
    style_src = cap_dir / "template" / "style.css"
    if style_src.is_file():
        style_dst = entry.parent / "style.css"
        if not style_dst.is_file():
            style_dst.write_text(style_src.read_text(encoding="utf-8"), encoding="utf-8")
