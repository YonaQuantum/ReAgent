"""Shared rendering / measurement engine for the rollup_banner tools.

The kernel owns the agent loop and the workspace; these tools own only the
deterministic "print physics" — turning the model's HTML/CSS into a measured
preview, a diagnostic report, and finally a printable PDF/PNG.

There are no third-party dependencies on purpose. Chrome (headless) is the
renderer; a tiny injected script does the DOM measurement via `--dump-dom`.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

_SHARED = Path(__file__).resolve().parent.parent
if str(_SHARED) not in sys.path:
    sys.path.insert(0, str(_SHARED))

from _shared.chrome import extract_node, find_chrome, load_json, read_doc, run_chrome  # noqa: E402

DPI = 96.0  # CSS px per inch; Chrome resolves `cm` units against this.
PX_PER_CM = DPI / 2.54
PX_PER_MM = DPI / 25.4

PROBE_ID = "__reagent_diagnostics"


def cm_to_px(cm: float) -> int:
    return int(round(cm * PX_PER_CM))


def _thresholds(rules: dict) -> dict:
    """Flatten the nested, mm-based capability rules into the px thresholds the
    measurement script consumes. The script measures computed CSS px on a canvas
    fixed to the physical print size, so every value here is *print* px
    (96px/inch), never thumbnail pixels."""
    typography = rules.get("typography", {})
    content = rules.get("content", {})
    composition = rules.get("composition", {})
    print_rules = rules.get("print", {})

    def mm(value: float) -> float:
        return float(value) * PX_PER_MM

    return {
        "min_font_px": round(mm(typography.get("annotation_min_mm", 10))),
        "title_min_px": round(mm(typography.get("title_min_mm", 30))),
        "max_chinese_chars": int(content.get("max_total_chinese_chars", 140)),
        "max_sections": int(content.get("max_sections", 4)),
        "max_boxed_modules": int(composition.get("max_boxed_modules", 2)),
        "min_qr_side_px": round(mm(print_rules.get("qr_min_width_mm", 100))),
        "min_image_dpi": float(print_rules.get("min_image_dpi", 150)),
        "allow_emoji": bool(content.get("allow_emoji", False)),
    }


def _measurement_script(width_px: int, height_px: int, rules: dict) -> str:
    """JS that walks the DOM and reports objective layout problems."""
    rules_js = json.dumps(_thresholds(rules))
    return f"""
(function () {{
  var W = {width_px}, H = {height_px};
  var R = {rules_js};
  var PXMM = {PX_PER_MM};
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

  // --- overflow ---
  var overflow = null;
  for (var i = 0; i < els.length; i++) {{
    var el = els[i];
    var r = el.getBoundingClientRect();
    if (r.width === 0 && r.height === 0) continue;
    if (r.right > W + 1 || r.bottom > H + 1 || r.left < -1 || r.top < -1) {{
      overflow = {{ type: 'overflow', selector: path(el), message:
        '元素越界 right=' + Math.round(r.right) + ' bottom=' + Math.round(r.bottom) +
        '（画布 ' + W + 'x' + H + '）' }};
      break;
    }}
  }}
  if (overflow) issues.push(overflow);

  // --- typography: absolute floor + a hero ceiling ---
  // Only measure elements that *directly* contain text. Container divs inherit
  // the body default (16px) and would otherwise report a bogus floor.
  function hasDirectText(el) {{
    var tag = el.tagName;
    if (tag === 'SCRIPT' || tag === 'STYLE' || tag === 'NOSCRIPT' || tag === 'TEMPLATE') return false;
    for (var i = 0; i < el.childNodes.length; i++) {{
      var n = el.childNodes[i];
      if (n.nodeType === 3 && (n.nodeValue || '').trim()) return true;
    }}
    return false;
  }}
  var minFont = null, maxFont = null;
  for (var j = 0; j < els.length; j++) {{
    var e = els[j];
    if (!hasDirectText(e)) continue;
    var fs = parseFloat(getComputedStyle(e).fontSize);
    if (isNaN(fs) || fs <= 0) continue;
    if (minFont === null || fs < minFont.fs) minFont = {{ fs: fs, selector: path(e) }};
    if (maxFont === null || fs > maxFont.fs) maxFont = {{ fs: fs, selector: path(e) }};
  }}
  if (minFont && minFont.fs < R.min_font_px) {{
    issues.push({{ type: 'font_too_small', selector: minFont.selector,
      message: '最小字号 ' + Math.round(minFont.fs) + 'px（约 ' + Math.round(minFont.fs / PXMM) + 'mm），低于 ' + R.min_font_px + 'px 下限' }});
  }}
  if (!maxFont || maxFont.fs < R.title_min_px) {{
    issues.push({{ type: 'no_hero_typography', selector: maxFont ? maxFont.selector : 'body',
      message: '最大字号仅 ' + (maxFont ? Math.round(maxFont.fs) : 0) + 'px，低于主标题要求的 ' + R.title_min_px + 'px（约 ' + Math.round(R.title_min_px / PXMM) + 'mm）——缺少一个真正的视觉重心' }});
  }}

  // --- content budget: too much text is the real "AI 感" ---
  var cjk = ((document.body.innerText || '').match(/[\\u4e00-\\u9fff\\u3400-\\u4dbf]/g) || []).length;
  if (cjk > R.max_chinese_chars) {{
    issues.push({{ type: 'too_much_content', selector: 'body',
      message: '中文正文 ' + cjk + ' 字，超过 ' + R.max_chinese_chars + ' 字上限——删内容，不要缩字号' }});
  }}

  // --- section count: headings as a proxy for content modules ---
  var headings = document.querySelectorAll('h1,h2,h3,h4,h5,h6').length;
  if (headings > R.max_sections) {{
    issues.push({{ type: 'too_many_sections', selector: 'body',
      message: '检测到 ' + headings + ' 个标题区块，超过 ' + R.max_sections + ' 个——合并或删除' }});
  }}

  // --- boxed "card" modules: the template-repeat smell ---
  var boxes = 0;
  for (var b = 0; b < els.length; b++) {{
    var be = els[b];
    var br = be.getBoundingClientRect();
    if (br.width < 40 || br.height < 40) continue;
    var cs = getComputedStyle(be);
    var bg = cs.backgroundColor || 'rgba(0, 0, 0, 0)';
    var hasBg = bg !== 'rgba(0, 0, 0, 0)' && bg !== 'transparent';
    var hasBorder = cs.borderTopWidth && cs.borderTopWidth !== '0px';
    var hasShadow = cs.boxShadow && cs.boxShadow !== 'none';
    if (hasBg && (hasBorder || hasShadow)) boxes++;
  }}
  if (boxes > R.max_boxed_modules) {{
    issues.push({{ type: 'too_many_cards', selector: 'body',
      message: '检测到 ' + boxes + ' 个盒状卡片模块，超过 ' + R.max_boxed_modules + ' 个上限——合并或删除' }});
  }}

  // --- QR (if present, must be big enough to scan from distance) ---
  var qrSide = 0;
  var qrImgs = document.querySelectorAll('img');
  for (var q = 0; q < qrImgs.length; q++) {{
    var qi = qrImgs[q];
    var sig = ((qi.getAttribute('src') || '') + ' ' + (qi.getAttribute('alt') || '') + ' ' +
               (qi.getAttribute('class') || '') + ' ' + (qi.getAttribute('id') || '')).toLowerCase();
    if (sig.indexOf('qr') < 0 && sig.indexOf('二维码') < 0) continue;
    var qr = qi.getBoundingClientRect();
    if (qr.width > qrSide) qrSide = qr.width;
    if (qr.height > qrSide) qrSide = qr.height;
  }}
  if (qrSide > 0 && qrSide < R.min_qr_side_px) {{
    issues.push({{ type: 'qr_too_small', selector: 'body',
      message: '二维码最小边 ' + Math.round(qrSide) + 'px（约 ' + Math.round(qrSide / PXMM) + 'mm），低于 ' + R.min_qr_side_px + 'px（100mm）' }});
  }}

  // --- emoji: cheap, no real designer uses them ---
  var emojiCount = 0;
  if (!R.allow_emoji) {{
    var emojiRe = /[\\u{{1F000}}-\\u{{1FAFF}}\\u{{2600}}-\\u{{27BF}}\\u{{2B00}}-\\u{{2BFF}}\\u{{FE0F}}\\u{{1F1E6}}-\\u{{1F1FF}}]/gu;
    emojiCount = ((document.body.innerText || '').match(emojiRe) || []).length;
    if (emojiCount > 0) {{
      issues.push({{ type: 'emoji_used', selector: 'body',
        message: '检测到 ' + emojiCount + ' 个 emoji——易拉宝用 emoji 显得廉价，改用 SVG 图标或纯排版' }});
    }}
  }}

  // --- images: missing + DPI ---
  var imgs = document.querySelectorAll('img');
  for (var k = 0; k < imgs.length; k++) {{
    var img = imgs[k];
    if (img.naturalWidth === 0) {{
      issues.push({{ type: 'missing_image', selector: path(img), message: '图片加载失败或缺失' }});
      continue;
    }}
    var rect = img.getBoundingClientRect();
    if (rect.width > 0) {{
      var dpi = img.naturalWidth / (rect.width / 96);
      if (dpi < R.min_image_dpi) issues.push({{ type: 'low_dpi_image', selector: path(img),
        message: '有效 DPI ' + Math.round(dpi) + '，低于 ' + R.min_image_dpi }});
    }}
  }}

  var result = {{
    viewport: {{ width: W, height: H }},
    issues: issues,
    metrics: {{
      cjk_chars: cjk,
      min_font_px: minFont ? Math.round(minFont.fs) : 0,
      max_font_px: maxFont ? Math.round(maxFont.fs) : 0,
      sections: headings,
      boxed_modules: boxes,
      emoji: emojiCount
    }}
  }};
  var node = document.createElement('pre');
  node.id = '{PROBE_ID}';
  node.textContent = JSON.stringify(result);
  document.body.appendChild(node);
}})();
"""


def build_probe(
    doc: str,
    width_cm: float,
    height_cm: float,
    scale: float = 1.0,
    rules: dict | None = None,
) -> tuple[str, int, int]:
    """Wrap the user HTML into a probe that fixes a pixel canvas and injects the
    measurement script. `scale` shrinks the output for a compact preview."""
    width_px = cm_to_px(width_cm)
    height_px = cm_to_px(height_cm)
    probe_style = (
        "<style>"
        f"html,body{{margin:0;padding:0;width:{width_px}px;height:{height_px}px;"
        f"overflow:hidden;transform:scale({scale});transform-origin:0 0;}}"
        "</style>"
    )
    script = f"<script>{_measurement_script(width_px, height_px, rules or {})}</script>"
    if "</body>" in doc:
        doc = doc.replace("</body>", probe_style + script + "</body>", 1)
    else:
        doc = doc + probe_style + script
    return doc, width_px, height_px


def extract_diagnostics(dumped: str) -> dict:
    return extract_node(dumped, PROBE_ID)


def inject_print_rules(doc: str, width_cm: float, height_cm: float) -> str:
    """Remove any @page the model wrote and inject a canonical one, so the PDF
    is always the exact physical size regardless of what the model remembers."""
    doc = re.sub(r"@page\s*\{[^}]*\}", "", doc)
    page_rule = f"<style>@page{{size:{width_cm}cm {height_cm}cm;margin:0;}}</style>"
    if "</head>" in doc:
        doc = doc.replace("</head>", page_rule + "</head>", 1)
    else:
        doc = page_rule + doc
    return doc


def measure(
    entry: Path,
    width_cm: float,
    height_cm: float,
    out_dir: Path,
    rules: dict | None = None,
) -> dict:
    """Render the model's HTML, produce a preview screenshot + diagnostics."""
    chrome = find_chrome()
    if chrome is None:
        return {
            "preview": None,
            "diagnostics": {"viewport": {"width": 0, "height": 0}, "issues": []},
            "summary": "未找到 Chrome，无法渲染预览与诊断。",
        }

    doc = read_doc(entry)
    width_px, height_px = cm_to_px(width_cm), cm_to_px(height_cm)

    # Probe for DOM measurement at full size.
    probe_doc, _, _ = build_probe(doc, width_cm, height_cm, scale=1.0, rules=rules)
    probe_path = out_dir / "_probe.html"
    probe_path.write_text(probe_doc, encoding="utf-8")
    dumped = run_chrome(
        [chrome, "--headless=new", "--disable-gpu", "--no-sandbox",
         "--dump-dom", "--virtual-time-budget=3000",
         f"--window-size={width_px},{height_px}", f"file://{probe_path.resolve()}"]
    )
    diagnostics = extract_diagnostics(dumped.stdout)

    # Compact preview screenshot for the model / human.
    preview_scale = min(1.0, 800.0 / width_px)
    preview_doc, _, _ = build_probe(doc, width_cm, height_cm, scale=preview_scale, rules=rules)
    preview_probe = out_dir / "_preview.html"
    preview_probe.write_text(preview_doc, encoding="utf-8")
    pw, ph = max(1, int(width_px * preview_scale)), max(1, int(height_px * preview_scale))
    preview_path = out_dir / "preview.png"
    run_chrome(
        [chrome, "--headless=new", "--disable-gpu", "--no-sandbox",
         "--screenshot=" + str(preview_path), "--hide-scrollbars",
         f"--window-size={pw},{ph}", f"file://{preview_probe.resolve()}"]
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
        return "排版正常：无溢出、无字号过小、无图片问题。"
    parts = [f"{i['type']}: {i.get('message', '')}" for i in issues[:5]]
    return "发现 " + str(len(issues)) + " 个问题：" + "；".join(parts)


def export_pdf(entry: Path, width_cm: float, height_cm: float, out_dir: Path) -> dict:
    """Export a printable PDF (page size via injected @page)."""
    chrome = find_chrome()
    if chrome is None:
        return {"pdf": None, "engine": "none", "error": "未找到 Chrome"}

    doc = inject_print_rules(read_doc(entry), width_cm, height_cm)
    print_path = out_dir / "_print.html"
    print_path.write_text(doc, encoding="utf-8")
    pdf_path = out_dir / "banner.pdf"

    result = run_chrome(
        [chrome, "--headless=new", "--disable-gpu", "--no-sandbox",
         "--no-pdf-header-footer", f"--print-to-pdf={pdf_path}",
         f"file://{print_path.resolve()}"]
    )
    if result.returncode != 0 or not pdf_path.is_file() or pdf_path.stat().st_size < 100:
        return {"pdf": None, "engine": "chrome-error", "error": (result.stderr or "")[:500]}
    return {"pdf": str(pdf_path), "engine": "chrome-headless"}


def export_png(entry: Path, width_cm: float, height_cm: float, out_dir: Path) -> dict:
    """Export a high-resolution PNG at full print size."""
    chrome = find_chrome()
    if chrome is None:
        return {"png": None, "error": "未找到 Chrome"}

    doc = read_doc(entry)
    width_px, height_px = cm_to_px(width_cm), cm_to_px(height_cm)
    probe_doc, _, _ = build_probe(doc, width_cm, height_cm, scale=1.0)
    shot_probe = out_dir / "_shot.html"
    shot_probe.write_text(probe_doc, encoding="utf-8")
    png_path = out_dir / "banner.png"
    result = run_chrome(
        [chrome, "--headless=new", "--disable-gpu", "--no-sandbox",
         "--screenshot=" + str(png_path), "--hide-scrollbars",
         f"--window-size={width_px},{height_px}", f"file://{shot_probe.resolve()}"]
    )
    if result.returncode != 0 or not png_path.is_file():
        return {"png": None, "error": (result.stderr or "")[:500]}
    return {"png": str(png_path)}


def load_print_rules(cap_dir: Path) -> dict:
    return load_json(cap_dir, "print_rules.json")


def seed_if_missing(cap_dir: Path, entry: Path, width_cm: float, height_cm: float) -> None:
    """If the model hasn't written an entry file, seed it from the shipped
    template so render/lint/export always have something concrete to work with."""
    if entry.is_file():
        return
    template = cap_dir / "template"
    index_src = template / "index.html"
    if index_src.is_file():
        doc = inject_print_rules(index_src.read_text(encoding="utf-8"), width_cm, height_cm)
        entry.parent.mkdir(parents=True, exist_ok=True)
        entry.write_text(doc, encoding="utf-8")
    style_src = template / "style.css"
    if style_src.is_file():
        style_dst = entry.parent / "style.css"
        if not style_dst.is_file():
            style_dst.write_text(style_src.read_text(encoding="utf-8"), encoding="utf-8")
