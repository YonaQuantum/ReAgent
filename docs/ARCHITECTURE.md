# ReAgent Kernel Architecture

ReAgent 是一个通用 agent runtime：一个很小的 Rust 内核负责"跑 agent loop"，所有业务能力（易拉宝、海报、PPT…）都是**数据驱动的 capability**，通过 manifest 声明、通过 worker 实现。

## 分层

```
┌──────────────────────────────────────────────────────┐
│  apps (CLI / HTTP server / WebUI)                     │
│  只做：解析入口参数 → Runtime → 输出                     │
└──────────────────────┬───────────────────────────────┘
                       │
┌──────────────────────▼───────────────────────────────┐
│  Runtime (crates/core/src/runtime.rs)                 │
│  · 从 capabilities/ 目录加载所有 manifest              │
│  · 构建 ToolRegistry + 生成通用 system prompt          │
│  · build_provider(name) → Box<dyn ModelProvider>      │
└──────┬──────────────────────────────┬────────────────┘
       │                              │
┌──────▼──────────────┐      ┌────────▼────────────────┐
│  Agent Loop (agent/) │      │  Model (model/)          │
│  通用 tool-calling    │◄────►│  ModelProvider trait     │
│  ReAct 循环           │      │  · OpenAI-compatible     │
│  · 无任何业务知识      │      │  · DeepSeek             │
│  · 事件轨迹           │      │  · Heuristic(离线)       │
└──────┬──────────────┘      └─────────────────────────┘
       │  function-calling schema
┌──────▼───────────────────────────────────────────────┐
│  Tool Registry (tool/)                                 │
│  Tool { name, description, parameters(JSON Schema),    │
│         command, args }                                │
└──────┬───────────────────────────────────────────────┘
       │  子进程 stdin JSON → stdout JSON
┌──────▼───────────────────────────────────────────────┐
│  Capabilities (capabilities/<id>/)                     │
│  manifest.json + worker.py — 领域逻辑全在这里          │
└──────────────────────────────────────────────────────┘
```

## 核心抽象

### 1. ModelProvider（统一模型适配层）

对应 pi 的 `pi-ai`。内核只依赖一个 trait，不关心背后是 DeepSeek、OpenAI-compatible 还是离线启发式：

```rust
#[async_trait]
pub trait ModelProvider: Send + Sync {
    async fn complete(&self, messages: &[Value], tools: &Value) -> Result<ModelResponse>;
}

pub enum ModelResponse {
    ToolCall(ToolCall),
    Text(String),
}
```

- `tools` 是 function-calling 格式的工具 schema，由 registry 动态生成。
- 接新 provider（Qwen、Claude、本地模型、公司网关）只需要新增一个 `ModelProvider` 实现，**不改 loop、不改 prompt**。

### 2. Tool（工具 = 描述 + JSON Schema + 进程运行时）

```rust
pub struct Tool {
    name: String,          // 模型看到的工具名
    description: String,   // 路由提示
    parameters: Value,     // stdin 契约的 JSON Schema
    command: PathBuf,      // 例如 python3
    args: Vec<String>,     // 例如 [worker.py]
}
```

运行时契约刻意保持平庸：把参数 JSON 写进子进程 stdin，从 stdout 读回 JSON。worker 承担全部领域复杂度，内核只需要 schema 就能把工具"推销"给模型。

### 3. Agent Loop（通用 ReAct 循环）

`AgentLoop::run` 完全不认识"易拉宝"：

1. 构建 `[system, user]` 消息。
2. 调 `model.complete(messages, tools_schema)`。
3. `ToolCall` → 注入通用上下文（`prompt`、`artifact_dir`）→ 执行工具 → 把 assistant tool_calls 和 tool 结果追加回消息 → 回到第 2 步。
4. `Text` → 结束。

终止条件由模型决定（返回文本），`max_steps` 兜底。**没有** `has_artifact_kind("pdf")` 之类的业务判断。

### 4. Capability（能力清单 = 数据）

每个能力一个目录，目录里有 `manifest.json` 和 worker：

```json
{
  "id": "rollup_banner",
  "name": "易拉宝设计",
  "description": "根据主题与文案生成可打印的易拉宝 HTML 并导出 PDF…",
  "match_keywords": ["易拉宝", "海报", "banner", "pdf"],
  "command": "python3",
  "entry": "worker.py",
  "parameters": { "type": "object", "properties": { ... } },
  "artifact_kinds": ["pdf", "html", "design_spec"]
}
```

内核启动时 `Runtime::load(capabilities_dir)` 扫目录，把每个 manifest 变成注册的 `Tool`。**加一个新场景 = 加一个目录，不改任何内核代码。**

## 通用上下文契约

每次工具调用，内核会自动注入两个字段：

- `prompt`：用户原始需求
- `artifact_dir`：本次 run 的输出目录（`artifacts/<run_id>/`）

模型不需要、也不应该伪造本地路径；它只负责填 manifest 里声明的业务参数（topic、width_cm、copy…）。

## 事件轨迹

`EventStream` 记录 `user_message / thought / tool_call / tool_result / artifact_created / final`，每次 run 落一份 `trajectory.jsonl`。`ContextEngine` 目前只取最近若干条拼给模型；`MemoryStore`、`SandboxRuntime` 是预留的扩展点（持久记忆、权限边界），尚未接入 loop。

## 加一个新能力的步骤

1. 建 `capabilities/<id>/manifest.json`，声明 id/描述/参数 schema。
2. 写 `capabilities/<id>/worker.py`，实现 stdin JSON → stdout JSON。
3. 重启 CLI/server，完事。

模型会通过 function-calling 自动看到新工具，无需改 prompt、loop、CLI、server 或 WebUI。
