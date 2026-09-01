use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context as _, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// A tool invocation requested by the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

/// The result of running a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub id: String,
    pub ok: bool,
    pub output: Value,
}

/// How a tool actually executes. Process tools spawn a subprocess (the
/// capability workers); built-ins are small harness-level operations (file I/O)
/// that run natively in the kernel, sandboxed to the run workspace.
#[derive(Debug, Clone)]
pub enum ToolExecutor {
    Process { command: PathBuf, args: Vec<String> },
    Builtin(BuiltinKind),
}

#[derive(Debug, Clone)]
pub enum BuiltinKind {
    ListFiles,
    ReadFile,
    WriteFile,
    IngestFile,
}

/// A tool: the model-facing description + JSON schema, plus an executor.
///
/// Process tools keep the deliberately boring runtime contract: spawn `command`
/// with `args`, write the tool-call arguments as JSON on stdin, read a JSON
/// object on stdout. The worker owns all domain complexity. Built-ins are
/// generic harness operations shared by every capability.
#[derive(Debug, Clone)]
pub struct Tool {
    name: String,
    description: String,
    parameters: Value,
    executor: ToolExecutor,
}

impl Tool {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
        command: impl Into<PathBuf>,
        args: Vec<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            executor: ToolExecutor::Process {
                command: command.into(),
                args,
            },
        }
    }

    pub fn new_builtin(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
        kind: BuiltinKind,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            executor: ToolExecutor::Builtin(kind),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn parameters(&self) -> &Value {
        &self.parameters
    }

    pub async fn run(&self, call: ToolCall) -> Result<ToolResult> {
        match &self.executor {
            ToolExecutor::Process { command, args } => self.run_process(command, args, call).await,
            ToolExecutor::Builtin(kind) => run_builtin(kind, call),
        }
    }

    async fn run_process(
        &self,
        command: &Path,
        args: &[String],
        call: ToolCall,
    ) -> Result<ToolResult> {
        let mut child = Command::new(command)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to spawn tool {}", self.name))?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("failed to open stdin for tool {}", self.name))?;
        let input = serde_json::to_vec(&call.args)?;
        stdin.write_all(&input).await?;
        drop(stdin);

        let output = child.wait_with_output().await?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            return Ok(ToolResult {
                id: call.id,
                ok: false,
                output: serde_json::json!({
                    "status": output.status.code(),
                    "stderr": stderr.trim(),
                    "stdout": stdout.trim(),
                }),
            });
        }

        let parsed = serde_json::from_str(stdout.trim()).with_context(|| {
            format!(
                "tool {} did not return JSON; stdout={:?} stderr={:?}",
                self.name, stdout, stderr
            )
        })?;

        Ok(ToolResult {
            id: call.id,
            ok: true,
            output: parsed,
        })
    }
}

/// Run a built-in file operation, sandboxed to the per-run workspace. The
/// workspace root is the `artifact_dir` the loop injects into every tool call.
fn run_builtin(kind: &BuiltinKind, call: ToolCall) -> Result<ToolResult> {
    let workspace = call
        .args
        .get("artifact_dir")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("builtin {} requires artifact_dir", call.name))?;

    let output = match kind {
        BuiltinKind::ListFiles => builtin_list_files(Path::new(workspace))?,
        BuiltinKind::ReadFile => {
            let path = required_str(&call.args, "path")?;
            let resolved = sandbox_resolve(Path::new(workspace), path)?;
            builtin_read_file(&resolved)?
        }
        BuiltinKind::WriteFile => {
            let path = required_str(&call.args, "path")?;
            let content = required_str(&call.args, "content")?;
            let resolved = sandbox_resolve(Path::new(workspace), path)?;
            builtin_write_file(&resolved, content)?
        }
        BuiltinKind::IngestFile => {
            let path = required_str(&call.args, "path")?;
            builtin_ingest_file(Path::new(workspace), path)?
        }
    };

    Ok(ToolResult {
        id: call.id,
        ok: true,
        output,
    })
}

fn required_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing required string argument `{key}`"))
}

/// Resolve a user-supplied relative path against the workspace, rejecting
/// absolute paths and anything that would escape the workspace via `..`.
fn sandbox_resolve(workspace: &Path, requested: &str) -> Result<PathBuf> {
    let req = Path::new(requested);
    if req.is_absolute() {
        return Err(anyhow!(
            "path must be relative to the workspace: {requested}"
        ));
    }

    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let joined = workspace.join(req);

    // The target (for writes) may not exist yet, so canonicalize the nearest
    // existing ancestor and re-append the missing tail. This catches `..` that
    // would climb out of the workspace while still allowing new files.
    let mut existing = joined.as_path();
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    while !existing.exists() {
        match existing.parent() {
            Some(parent) => {
                tail.push(
                    existing
                        .file_name()
                        .ok_or_else(|| anyhow!("bad path: {requested}"))?,
                );
                existing = parent;
            }
            None => return Err(anyhow!("cannot resolve path: {requested}")),
        }
    }
    let mut resolved = existing
        .canonicalize()
        .with_context(|| format!("failed to resolve {requested}"))?;
    for segment in tail.into_iter().rev() {
        resolved = resolved.join(segment);
    }

    if !resolved.starts_with(&workspace) {
        return Err(anyhow!("path escapes the workspace: {requested}"));
    }
    Ok(resolved)
}

fn builtin_list_files(workspace: &Path) -> Result<Value> {
    let mut files = Vec::new();
    collect_files(workspace, workspace, &mut files)?;
    files.sort_by(|a, b| a.cmp(b));
    Ok(serde_json::json!({
        "ok": true,
        "files": files,
    }))
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("read_dir {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            out.push(rel);
        }
    }
    Ok(())
}

fn builtin_read_file(path: &Path) -> Result<Value> {
    const MAX_BYTES: usize = 16 * 1024;
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let truncated = bytes.len() > MAX_BYTES;
    let slice = &bytes[..bytes.len().min(MAX_BYTES)];
    let content = String::from_utf8_lossy(slice).to_string();
    Ok(serde_json::json!({
        "ok": true,
        "path": path.to_string_lossy(),
        "bytes": bytes.len(),
        "truncated": truncated,
        "content": content,
    }))
}

/// Ingest a file from an absolute local path into the workspace `input/`
/// directory. This is the *only* built-in that reads outside the workspace; it
/// is registered only when the host opts in via `REAGENT_ALLOW_LOCAL_FS`, since
/// anything it copies can flow onward to the (cloud) model provider.
fn builtin_ingest_file(workspace: &Path, abs_path: &str) -> Result<Value> {
    let src = Path::new(abs_path);
    if !src.is_absolute() {
        return Err(anyhow!("path must be an absolute local path: {abs_path}"));
    }
    if !src.is_file() {
        return Err(anyhow!("no such file: {abs_path}"));
    }
    let name = src
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("bad file name: {abs_path}"))?;

    let input_dir = workspace.join("input");
    std::fs::create_dir_all(&input_dir)
        .with_context(|| format!("create_dir_all {}", input_dir.display()))?;
    let dst = input_dir.join(name);
    let bytes = std::fs::copy(src, &dst)
        .with_context(|| format!("copy {} -> {}", src.display(), dst.display()))?;
    Ok(serde_json::json!({
        "ok": true,
        "path": format!("input/{}", name),
        "name": name,
        "bytes": bytes,
    }))
}

fn builtin_write_file(path: &Path, content: &str) -> Result<Value> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create_dir_all {}", parent.display()))?;
    }
    let bytes = content.as_bytes();
    std::fs::write(path, bytes).with_context(|| format!("write {}", path.display()))?;
    Ok(serde_json::json!({
        "ok": true,
        "path": path.to_string_lossy(),
        "bytes": bytes.len(),
    }))
}
