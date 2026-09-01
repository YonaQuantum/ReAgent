use std::path::PathBuf;

use anyhow::{anyhow, Context as _, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::event::{Event, EventKind, EventStream};
use crate::model::{ModelProvider, ModelResponse};
use crate::tool::{ToolCall, ToolRegistry, ToolResult};

/// A run request. Deliberately free of domain-specific fields — anything that
/// describes the task's content belongs in the prompt or in tool arguments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunConfig {
    pub user_prompt: String,
    pub artifact_dir: PathBuf,
    pub max_steps: usize,
    /// Absolute paths of files the caller wants the run to use (e.g. uploads),
    /// staged into the workspace `input/` directory before the loop starts.
    #[serde(default)]
    pub input_files: Vec<PathBuf>,
    /// Optional live event sink. When set, every trajectory event is forwarded
    /// here as it happens (in addition to being recorded to the jsonl file).
    #[serde(skip)]
    pub event_tx: Option<tokio::sync::mpsc::UnboundedSender<Event>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunOutput {
    pub run_id: Uuid,
    pub final_message: String,
    pub artifacts: Vec<Value>,
    pub event_log_path: Option<PathBuf>,
}

pub struct AgentLoop<M> {
    model: M,
    tools: ToolRegistry,
    system_prompt: String,
}

impl<M> AgentLoop<M>
where
    M: ModelProvider + Send + Sync,
{
    pub fn new(model: M, tools: ToolRegistry, system_prompt: String) -> Self {
        Self {
            model,
            tools,
            system_prompt,
        }
    }

    pub async fn run(&self, config: AgentRunConfig) -> Result<AgentRunOutput> {
        let run_id = Uuid::new_v4();
        let run_dir = config.artifact_dir.join(run_id.to_string());
        tokio::fs::create_dir_all(&run_dir)
            .await
            .with_context(|| format!("failed to create run dir {}", run_dir.display()))?;

        let event_log_path = run_dir.join("trajectory.jsonl");
        let mut events = match config.event_tx.clone() {
            Some(tx) => EventStream::with_jsonl_and_sink(event_log_path.clone(), tx),
            None => EventStream::with_jsonl(event_log_path.clone()),
        };
        events.push(EventKind::UserMessage {
            content: config.user_prompt.clone(),
        });

        // Stage any caller-supplied files into the workspace so workers and the
        // parse tools can reach them via a stable `input/` relative path.
        let input_names = stage_input_files(&run_dir, &config.input_files).await?;

        let mut messages: Vec<Value> = vec![
            json!({ "role": "system", "content": self.system_prompt }),
            json!({ "role": "user", "content": config.user_prompt }),
        ];
        if !input_names.is_empty() {
            messages.push(json!({
                "role": "user",
                "content": format!(
                    "用户提供了以下输入文件，已放入工作区 input/ 目录：{}。请按需用 parse_pdf / parse_image / read_file 解析其内容。",
                    input_names.join("、")
                ),
            }));
        }

        let mut artifacts = Vec::new();
        let mut progress = RunProgress::default();

        for step in 0..config.max_steps {
            events.push(EventKind::Thought {
                content: format!(
                    "Planning step {} with {} prior events",
                    step + 1,
                    events.len()
                ),
            });

            let response = self.model.complete(&messages, &self.tools.schema()).await?;

            match response {
                ModelResponse::ToolCall {
                    call,
                    reasoning_content,
                } => {
                    events.push(EventKind::ToolCall {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        args: call.args.clone(),
                    });

                    let result = self.run_tool(&call, &config, &run_dir).await?;
                    events.push(EventKind::ToolResult {
                        id: result.id.clone(),
                        ok: result.ok,
                        output: result.output.clone(),
                    });

                    // Echo the assistant tool call, then the tool result, so the
                    // model keeps a coherent transcript across turns. Reasoning
                    // models require their `reasoning_content` be passed back
                    // verbatim, so carry it through when present.
                    messages.push(assistant_tool_call_message(
                        &call,
                        reasoning_content.as_deref(),
                    ));
                    messages.push(tool_result_message(&result));

                    if result.ok {
                        if let Some(created) =
                            result.output.get("artifacts").and_then(Value::as_array)
                        {
                            for artifact in created {
                                artifacts.push(artifact.clone());
                                events.push(EventKind::ArtifactCreated {
                                    artifact: artifact.clone(),
                                });
                            }
                        }

                        progress.observe(&result.output);

                        // Feed a rendered preview back as an image when the model
                        // can see it; diagnostics text is already in the result.
                        if self.model.supports_vision() {
                            self.maybe_feed_preview(&result, &mut messages).await;
                            self.maybe_feed_image(&result, &mut messages).await;
                        }
                    } else {
                        return Err(anyhow!("tool failed: {}", result.output));
                    }
                }
                ModelResponse::Text {
                    content,
                    reasoning_content,
                } => {
                    // Completion gate: the model may claim "done" but must have
                    // actually produced the deliverable. Nudge it to continue.
                    if !progress.satisfied() {
                        events.push(EventKind::Thought {
                            content: "Model returned text but the completion gate is not satisfied"
                                .to_string(),
                        });
                        // Keep the assistant turn in the transcript (with its
                        // reasoning content) so thinking models stay coherent.
                        messages.push(assistant_text_message(
                            &content,
                            reasoning_content.as_deref(),
                        ));
                        messages.push(json!({
                            "role": "user",
                            "content": format!(
                                "任务尚未完成：{}。请继续调用工具，完成后再结束。",
                                progress.missing().join("、")
                            ),
                        }));
                        continue;
                    }

                    events.push(EventKind::Final {
                        content: content.clone(),
                    });
                    events.flush().await?;
                    return Ok(AgentRunOutput {
                        run_id,
                        final_message: content,
                        artifacts,
                        event_log_path: Some(event_log_path),
                    });
                }
            }
        }

        // Ran out of steps: return what we have rather than erroring, so the
        // user can still inspect partial artifacts.
        let final_message = if progress.satisfied() {
            "任务已完成。".to_string()
        } else {
            format!(
                "达到步数上限（{}），任务未完全满足完成条件：{}。已生成的产物见输出列表。",
                config.max_steps,
                progress.missing().join("、")
            )
        };
        events.push(EventKind::Final {
            content: final_message.clone(),
        });
        events.flush().await?;
        Ok(AgentRunOutput {
            run_id,
            final_message,
            artifacts,
            event_log_path: Some(event_log_path),
        })
    }

    /// Run a tool call, injecting the generic runtime context (`prompt` and
    /// `artifact_dir`) that every worker may rely on.
    async fn run_tool(
        &self,
        call: &ToolCall,
        config: &AgentRunConfig,
        run_dir: &std::path::Path,
    ) -> Result<ToolResult> {
        let mut args = call.args.clone();
        if let Some(object) = args.as_object_mut() {
            object
                .entry("prompt")
                .or_insert_with(|| json!(config.user_prompt));
            object
                .entry("artifact_dir")
                .or_insert_with(|| json!(run_dir.display().to_string()));
        }

        self.tools
            .run(ToolCall {
                id: call.id.clone(),
                name: call.name.clone(),
                args,
            })
            .await
    }

    /// If a tool result carries a `preview` image path, append it to the
    /// conversation as a base64 image part so the model can "see" its output.
    async fn maybe_feed_preview(&self, result: &ToolResult, messages: &mut Vec<Value>) {
        let Some(path) = result.output.get("preview").and_then(Value::as_str) else {
            return;
        };
        let bytes = match tokio::fs::read(path).await {
            Ok(bytes) => bytes,
            Err(_) => return,
        };
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        messages.push(json!({
            "role": "user",
            "content": [
                { "type": "text", "text": "这是刚渲染出的预览图，请检查排版、层级、配色与整体效果。若还有明显问题就修改文件后重新渲染；若已经可以，就运行检查工具确认，通过后导出最终产物并结束——不要反复重渲染。" },
                { "type": "image_url", "image_url": { "url": format!("data:image/png;base64,{encoded}") } }
            ]
        }));
    }

    /// If a tool result carries an `image` path (an input image the user
    /// supplied), feed it back as a base64 image part so the model can grasp its
    /// content and style — beyond whatever OCR text was extracted.
    async fn maybe_feed_image(&self, result: &ToolResult, messages: &mut Vec<Value>) {
        let Some(path) = result.output.get("image").and_then(Value::as_str) else {
            return;
        };
        let bytes = match tokio::fs::read(path).await {
            Ok(bytes) => bytes,
            Err(_) => return,
        };
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        let mime = image_mime(path);
        messages.push(json!({
            "role": "user",
            "content": [
                { "type": "text", "text": "这是一张用户提供的图片，请理解它的内容、风格与其中文字，用于后续设计。" },
                { "type": "image_url", "image_url": { "url": format!("data:{mime};base64,{encoded}") } }
            ]
        }));
    }
}

/// Copy `input_files` (absolute paths supplied by the caller) into the run
/// workspace under `input/`, deduplicating name collisions. Returns the
/// workspace-relative paths of the files that were staged successfully.
async fn stage_input_files(
    run_dir: &std::path::Path,
    input_files: &[PathBuf],
) -> Result<Vec<String>> {
    let mut staged = Vec::new();
    if input_files.is_empty() {
        return Ok(staged);
    }
    let input_dir = run_dir.join("input");
    tokio::fs::create_dir_all(&input_dir).await?;
    for src in input_files {
        let Some(name) = src.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let mut dst = input_dir.join(name);
        let mut n = 1;
        while dst.exists() {
            let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
            let ext = src
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| format!(".{e}"))
                .unwrap_or_default();
            dst = input_dir.join(format!("{stem}-{n}{ext}"));
            n += 1;
        }
        if tokio::fs::copy(src, &dst).await.is_ok() {
            if let Some(rel) = dst.strip_prefix(run_dir).ok().and_then(|p| p.to_str()) {
                staged.push(rel.to_string());
            }
        }
    }
    Ok(staged)
}

/// Best-effort image MIME from a path's extension, for the vision feed data URI.
fn image_mime(path: &str) -> &'static str {
    match path
        .rsplit('.')
        .next()
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "image/png",
    }
}

/// Tracks whether the run has satisfied the generic completion contract,
/// inferred from tool output fields rather than specific tool names:
/// a `preview` means something was rendered, a `passed` flag is a check result,
/// and `exported` marks the final deliverable.
#[derive(Debug, Default)]
struct RunProgress {
    rendered: bool,
    lint_passed: bool,
    exported: bool,
}

impl RunProgress {
    fn observe(&mut self, output: &Value) {
        if output.get("preview").and_then(Value::as_str).is_some() {
            self.rendered = true;
        }
        if let Some(passed) = output.get("passed").and_then(Value::as_bool) {
            self.lint_passed = passed;
        }
        if output.get("exported").and_then(Value::as_bool) == Some(true) {
            self.exported = true;
        }
    }

    fn satisfied(&self) -> bool {
        self.rendered && self.lint_passed && self.exported
    }

    fn missing(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if !self.rendered {
            missing.push("尚未渲染预览");
        }
        if !self.lint_passed {
            missing.push("尚未通过排版检查");
        }
        if !self.exported {
            missing.push("尚未导出最终产物");
        }
        missing
    }
}

fn assistant_tool_call_message(call: &ToolCall, reasoning_content: Option<&str>) -> Value {
    let mut message = json!({
        "role": "assistant",
        "content": null,
        "tool_calls": [{
            "id": call.id,
            "type": "function",
            "function": {
                "name": call.name,
                "arguments": serde_json::to_string(&call.args).unwrap_or_else(|_| "{}".to_string()),
            }
        }]
    });
    if let Some(reasoning) = reasoning_content {
        message["reasoning_content"] = json!(reasoning);
    }
    message
}

fn assistant_text_message(content: &str, reasoning_content: Option<&str>) -> Value {
    let mut message = json!({ "role": "assistant", "content": content });
    if let Some(reasoning) = reasoning_content {
        message["reasoning_content"] = json!(reasoning);
    }
    message
}

fn tool_result_message(result: &ToolResult) -> Value {
    json!({
        "role": "tool",
        "tool_call_id": result.id,
        "content": serde_json::to_string(&result.output).unwrap_or_else(|_| "{}".to_string()),
    })
}
