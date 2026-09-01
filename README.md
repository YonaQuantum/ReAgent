<p align="center">
  <a href="https://picui.ogmua.cn/s1/2026/09/01/6a96d27566a61.webp"><img src="https://picui.ogmua.cn/s1/2026/09/01/6a96d27566a61.webp" alt="ReAgentLogo" /></a>
</p>

# ReAgent

ReAgent 是一个**通用 Agent Runtime**：一个很小的 Rust 内核负责跑通用的 agent loop（模型适配 + 工具注册），所有业务能力都是**数据驱动的 capability**——一个 `manifest.json` 声明工具、一个 `worker.py` 实现，界面是一个浏览器网页。

## 快速开始

### 系统要求

- **Rust**（含 cargo）
- **Python 3**（在 PATH 上；内核自动识别 Windows 的 `python` / 其它平台的 `python3`，可用 `REAGENT_PYTHON` 覆盖）
- **Chrome 或 Edge**（渲染 / 导出用，自动探测常见安装位置）
- **Node**（仅构建 WebUI 时需要）
- 可选：`pdftotext`（poppler）与 `tesseract`，用于 `file_ingest` 的 PDF / 图片解析

### 安装

```bash
git clone https://github.com/YonaQuantum/ReAgent.git
cd ReAgent
cargo build --release
```

### 跑通 CLI（离线，无需任何模型 key）

```bash
cargo run -p reagent-cli -- "做一个网页首页" --provider heuristic
```

产物落在 `artifacts/<run_id>/`（HTML、预览图、诊断、PDF / 截图）。

### 启动 WebUI

先构建一次前端（需要 Node）：

```bash
cd apps/webui && npm install && npm run build && cd ../..
cargo run -p reagent-server   # 浏览器打开 http://localhost:8787
```

> 不构建前端也能起 server，但只会提供 API，`http://localhost:8787` 打不开界面。

### 配置模型（真实使用）

默认 provider 是 `deepseek`：

```bash
# Linux / macOS
export DEEPSEEK_API_KEY=你的_key
# Windows PowerShell
$env:DEEPSEEK_API_KEY="你的_key"
```

其它 OpenAI 兼容模型走 `openai-compatible`，设置 `REAGENT_MODEL_BASE_URL`、`REAGENT_MODEL_API_KEY_ENV`、`REAGENT_MODEL` 三个环境变量即可（模板见 `.env.example`）。

### 故障排查

- `python` / `python3` 找不到 → 安装 Python 并加入 PATH，或设 `REAGENT_PYTHON` 指向解释器。
- 找不到 Chrome → 安装 Chrome 或 Edge（内核会自动探测）。
- WebUI 404 → 先在 `apps/webui` 里 `npm run build`。
- 解析 PDF / 图片报错 → 装 `pdftotext`（poppler）与 `tesseract`（含中文语言包 `chi_sim`），或 `pip install pypdf`。

## 理念

- **内核不识字**：agent loop、system prompt、工具注册都不认识任何具体业务，只认识工具 schema。
- **能力即数据**：加一个新场景 = 在 `capabilities/` 下加一个目录，不改内核一行代码。
- **worker 是普通子进程**：内核把参数 JSON 写进 stdin、从 stdout 读回 JSON，worker 用什么语言、装什么依赖都行。
- **界面解耦**：WebUI 按每次 run 返回的 artifacts 动态渲染，不绑定具体产物类型。

## 架构

[![ReAgent 架构图](https://picui.ogmua.cn/s1/2026/09/01/6a96dd57db34c.webp)](https://picui.ogmua.cn/s1/2026/09/01/6a96dd57db34c.webp)

```text
用户自然语言需求
  → Rust Agent Loop（通用 tool-calling，无业务知识）
  → 模型通过 function-calling 选择工具
  → Capability worker（Python 子进程，stdin JSON → stdout JSON）
  → 渲染 / 检测 / 导出（Chrome Headless）
  → 返回 artifacts 与轨迹

app (CLI / server / WebUI)
  └─ Runtime：加载 capabilities/、构建工具注册表、选择 provider
       ├─ Agent Loop（通用 ReAct）
       ├─ ModelProvider（可替换）
       └─ ToolRegistry（每个 capability 一个工具，带 JSON Schema）
            └─ worker.py（stdin JSON → stdout JSON）
```

## 现有能力

内核一视同仁地对待每一个 capability；易拉宝只是其中一个。

- **`web_design`** —— 网页设计：浏览器视口渲染预览、检测布局问题、导出 HTML / 整页截图。
- **`file_ingest`** —— 文件解析：抽取 PDF 文本、对图片做 OCR，供后续设计使用。
- **`rollup_banner`** —— 印刷品设计（易拉宝 / 海报 / 展架）：渲染、排版检测、导出可打印 PDF / PNG。

## 加一个新能力

1. 建 `capabilities/<id>/manifest.json`，声明 id、描述、参数 JSON Schema。
2. 写 `capabilities/<id>/worker.py`，实现 stdin JSON → stdout JSON 契约。
3. 重启，模型会自动看到新工具。

详细架构与扩展点见 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)。
