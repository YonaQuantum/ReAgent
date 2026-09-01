# ReAgent

通用 agent runtime：Rust 内核（loop + 模型适配 + 工具注册）+ 数据驱动的 capability（manifest.json + worker.py）。

- 项目方向、已完成、待办：见 [MEMORY.md](MEMORY.md)。
- 架构与扩展点：见 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)。
- 加新能力 = 加 `capabilities/<id>/` 目录，不改内核代码。
