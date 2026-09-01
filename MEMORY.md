# ReAgent 项目记忆

这份文件记录项目的方向、每一轮做了什么、以及**还没做**的点，防止后续忘了。改架构前先读 `docs/ARCHITECTURE.md`。

## 方向（不可轻易改的意图）

把 ReAgent 做成一个**通用 agent runtime**，而不是"易拉宝生成器"：

- Rust 内核只负责 agent loop + 模型适配 + 工具注册，**不包含任何业务知识**。
- 业务能力是**数据驱动**的 capability：一个 `capabilities/<id>/` 目录 = 一个 `manifest.json` + 一个 `worker.py`。
- 模型通过 **OpenAI 兼容 function-calling**（`tools` 参数）动态看到工具 schema，自己决定调哪个工具、传什么参数。
- 参考架构：`pi`（`pi-ai` 模型适配层 / `pi-agent-core` 通用 loop / 工具）和 `deepseek-harness`（everything-is-a-plugin）。仓库：https://github.com/earendil-works/pi 、 https://github.com/deepseek-ai/deepseek-harness

## 已完成（按轮次）

**第 1 轮 —— 去硬编码，重构成通用 agent**
- `Tool` 带 `name + description + parameters(JSON Schema) + 进程运行时`；registry 的 `schema()` 输出 function-calling schema。
- `ModelProvider` 改成统一 `complete(messages, tools) -> ToolCall | Text`，原生 function-calling。
- `AgentRunConfig` 删除 `width_cm/height_cm`；loop 是通用 ReAct，终止由模型返回文本决定。
- 新增 `Runtime::load(capabilities_dir)` + `build_provider(name)`，CLI/server 共用，删掉两处重复注册。
- 落地 `CapabilityManifest` 加载器（原 `capability/mod.rs` 是死代码）。
- 能力 `capabilities/rollup_banner/`（manifest + worker），删掉旧 `tools/python/rollup_banner.py` 和"飓法 Work"演示文案。
- WebUI 产物面板动态渲染，去掉宽高输入和写死的 PDF/HTML/SPEC 行。
- 文档：README + ARCHITECTURE 重写。

**第 2 轮 —— 流式前后端（本次）**
- 后端 `/api/runs` 改成 SSE 流式返回轨迹事件；前端 `fetch` + `ReadableStream` 实时渲染。

## 还没做（待办，按优先级）

1. **流式轨迹落盘**：目前 `EventStream::flush()` 只在结束时整段重写 `trajectory.jsonl`；SSE 是实时的，但落盘不是增量的。
2. **MemoryStore（持久记忆）**：模块存在但没接入 loop，跨 run 记住用户偏好/上下文。
3. **SandboxRuntime（权限边界）**：模块存在但没接入，worker 当前直接以用户权限跑，无目录/网络限制。
4. **artifact verifier**：校验 PDF 尺寸、页数、文件大小、文字溢出风险（README 旧版提过）。
5. **更多 capability**：海报、PPT、名片等，验证"加场景不改内核"。
6. **provider 扩展**：Claude、Qwen、本地模型（只差一个 `ModelProvider` 实现）。
7. **测试**：目前只有手动的 `cargo check` / heuristic 冒烟，没有单测/集成测试。
8. **DeepSeek 模型名**：`deepseek-v4-flash` 是沿用旧配置，需确认是否真实（`ProviderConfig::deepseek()` 里）。

## 关键约定

- worker 契约：stdin JSON → stdout JSON；内核自动注入 `prompt` + `artifact_dir`，输出 `{"artifacts":[{kind,path,mime,...}]}`。
- 加新能力 = 加 `capabilities/<id>/` 目录，不改内核/CLI/server/WebUI。
- 服务启动见 README：后端 `reagent-server`（8787），前端 `apps/webui`（5173）。
- Node 装在项目内 `.deps/node`，不污染系统环境。
