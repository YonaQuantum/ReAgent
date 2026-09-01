use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use serde_json::json;

use super::{BuiltinKind, Tool, ToolCall, ToolResult};

#[derive(Debug, Default, Clone)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Tool>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: Tool) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Register the generic workspace file tools available to every run. These
    /// are harness-level operations, not capability-specific ones. `ingest_file`
    /// (read from an absolute local path) is registered only when the host opts
    /// in via `allow_local_fs`, since its contents can flow to the model.
    pub fn register_builtins(&mut self, allow_local_fs: bool) {
        self.register(Tool::new_builtin(
            "list_files",
            "List every file in the run workspace (relative paths).",
            json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            BuiltinKind::ListFiles,
        ));
        self.register(Tool::new_builtin(
            "read_file",
            "Read a text file from the run workspace (truncated if very large).",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace-relative path, e.g. index.html" }
                },
                "required": ["path"]
            }),
            BuiltinKind::ReadFile,
        ));
        self.register(Tool::new_builtin(
            "write_file",
            "Write (create or overwrite) a text file in the run workspace.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace-relative path, e.g. index.html or style.css" },
                    "content": { "type": "string", "description": "Full file contents to write" }
                },
                "required": ["path", "content"]
            }),
            BuiltinKind::WriteFile,
        ));
        if allow_local_fs {
            self.register(Tool::new_builtin(
                "ingest_file",
                "读取本机绝对路径的文件，复制到工作区 input/ 目录供后续解析。仅在显式开启本地文件访问时可用。",
                json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "本机文件的绝对路径，如 /home/user/report.pdf" }
                    },
                    "required": ["path"]
                }),
                BuiltinKind::IngestFile,
            ));
        }
    }

    pub fn get(&self, name: &str) -> Option<&Tool> {
        self.tools.get(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Tool> {
        self.tools.values()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Advertise every tool as an OpenAI-style function-calling schema.
    ///
    /// The model receives this list and can choose which tool to call with what
    /// arguments — no tool-specific knowledge lives in the kernel or prompt.
    pub fn schema(&self) -> serde_json::Value {
        let tools = self
            .tools
            .values()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name(),
                        "description": tool.description(),
                        "parameters": tool.parameters(),
                    }
                })
            })
            .collect::<Vec<_>>();
        json!(tools)
    }

    pub async fn run(&self, call: ToolCall) -> Result<ToolResult> {
        let tool = self
            .tools
            .get(&call.name)
            .ok_or_else(|| anyhow!("unknown tool: {}", call.name))?;
        tool.run(call).await
    }
}
