use anyhow::{anyhow, Context as _, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::tool::ToolCall;

/// What the model decided to do on a single turn.
#[derive(Debug, Clone)]
pub enum ModelResponse {
    /// Call a tool with these arguments.
    ToolCall {
        call: ToolCall,
        reasoning_content: Option<String>,
    },
    /// The task is finished; this is the user-facing final message.
    Text {
        content: String,
        reasoning_content: Option<String>,
    },
}

/// A unified model adapter: takes the conversation so far plus the available
/// tool schemas, and returns the next action.
///
/// This is the "unified LLM API" seam (cf. `pi-ai`). The agent loop is agnostic
/// to which provider backs it; providers are added without touching the loop.
#[async_trait]
pub trait ModelProvider: Send + Sync {
    async fn complete(&self, messages: &[Value], tools: &Value) -> Result<ModelResponse>;

    /// Whether this provider accepts image content parts (multimodal). When
    /// true, the loop feeds rendered previews back to the model as images;
    /// otherwise it relies on the structured text diagnostics only.
    fn supports_vision(&self) -> bool {
        false
    }
}

/// Let `Box<dyn ModelProvider>` be used anywhere a `ModelProvider` is expected,
/// so `build_provider` can return a single boxed type.
#[async_trait]
impl<M: ModelProvider + ?Sized> ModelProvider for Box<M> {
    async fn complete(&self, messages: &[Value], tools: &Value) -> Result<ModelResponse> {
        (**self).complete(messages, tools).await
    }

    fn supports_vision(&self) -> bool {
        (**self).supports_vision()
    }
}

// ---------------------------------------------------------------------------
// Offline heuristic planner
// ---------------------------------------------------------------------------

/// Offline fallback that requires no API key. It picks a tool whose name or
/// description overlaps the request, and otherwise asks the user to be more
/// specific. Intended for demos and testing the toolchain, not production.
#[derive(Debug, Default, Clone)]
pub struct HeuristicPlanner;

#[async_trait]
impl ModelProvider for HeuristicPlanner {
    async fn complete(&self, messages: &[Value], tools: &Value) -> Result<ModelResponse> {
        let prompt = extract_user_text(messages).unwrap_or_default();
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Ok(ModelResponse::Text {
                content: "我在。请告诉我你想生成什么，例如主题、尺寸和输出格式。".to_string(),
                reasoning_content: None,
            });
        }

        let called = called_tool_names(messages);
        if called.iter().any(|name| name == "export_banner") {
            return Ok(ModelResponse::Text {
                content: "设计已完成，PDF 和预览已显示在右侧，可以直接查看或下载。".to_string(),
                reasoning_content: None,
            });
        }
        if called.iter().any(|name| name == "export_page") {
            return Ok(ModelResponse::Text {
                content: "网页已完成，HTML 与整页截图已显示在右侧，可以直接查看。".to_string(),
                reasoning_content: None,
            });
        }

        let candidates = tool_hints(tools);
        let next_banner_tool = if called.iter().any(|name| name == "lint_banner") {
            Some("export_banner")
        } else if called.iter().any(|name| name == "render_banner") {
            Some("lint_banner")
        } else if is_design_request(prompt) {
            Some("render_banner")
        } else {
            None
        };
        if let Some(name) =
            next_banner_tool.filter(|name| candidates.iter().any(|hint| hint.name == *name))
        {
            return Ok(ModelResponse::ToolCall {
                call: ToolCall {
                    id: format!("{name}-{}", called.len() + 1),
                    name: name.to_string(),
                    args: json!({}),
                },
                reasoning_content: None,
            });
        }

        let next_web_tool = if called.iter().any(|name| name == "lint_page") {
            Some("export_page")
        } else if called.iter().any(|name| name == "render_page") {
            Some("lint_page")
        } else if is_web_request(prompt) {
            Some("render_page")
        } else {
            None
        };
        if let Some(name) =
            next_web_tool.filter(|name| candidates.iter().any(|hint| hint.name == *name))
        {
            return Ok(ModelResponse::ToolCall {
                call: ToolCall {
                    id: format!("{name}-{}", called.len() + 1),
                    name: name.to_string(),
                    args: json!({}),
                },
                reasoning_content: None,
            });
        }

        if let Some(hint) = candidates.iter().find(|hint| hint.matches(prompt)) {
            return Ok(ModelResponse::ToolCall {
                call: ToolCall {
                    id: format!("{}-1", hint.name),
                    name: hint.name.clone(),
                    args: json!({}),
                },
                reasoning_content: None,
            });
        }

        if candidates.len() == 1 {
            // Single-tool agent: the only sensible move is to use it.
            return Ok(ModelResponse::ToolCall {
                call: ToolCall {
                    id: format!("{}-1", candidates[0].name),
                    name: candidates[0].name.clone(),
                    args: json!({}),
                },
                reasoning_content: None,
            });
        }

        Ok(ModelResponse::Text {
            content: format!(
                "我目前还不能确定该怎么处理这个需求。可用的工具：{}。",
                candidates
                    .iter()
                    .map(|hint| hint.name.as_str())
                    .collect::<Vec<_>>()
                    .join("、")
            ),
            reasoning_content: None,
        })
    }
}

#[derive(Debug, Clone)]
struct ToolHint {
    name: String,
    description: String,
}

impl ToolHint {
    fn matches(&self, prompt: &str) -> bool {
        let haystack = prompt.to_lowercase();
        tokens(&self.name)
            .chain(tokens(&self.description))
            .any(|token| !token.is_empty() && haystack.contains(&token.to_lowercase()))
    }
}

/// Extract `{name, description}` pairs from a function-calling schema.
fn tool_hints(tools: &Value) -> Vec<ToolHint> {
    tools
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|tool| {
                    let function = tool.get("function")?;
                    Some(ToolHint {
                        name: function.get("name")?.as_str()?.to_string(),
                        description: function
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Split a name or description into searchable tokens, keeping CJK runs intact.
fn tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_alphanumeric()) && !('\u{4e00}'..='\u{9fff}').contains(&c))
        .filter(|token| token.chars().count() >= 2)
}

fn extract_user_text(messages: &[Value]) -> Option<&str> {
    messages
        .iter()
        .find(|m| m.get("role").and_then(Value::as_str) == Some("user"))
        .and_then(|m| m.get("content").and_then(Value::as_str))
}

fn called_tool_names(messages: &[Value]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| message.get("tool_calls").and_then(Value::as_array))
        .flatten()
        .filter_map(|call| call.pointer("/function/name").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

fn is_design_request(prompt: &str) -> bool {
    let prompt = prompt.to_lowercase();
    [
        "易拉宝",
        "展架",
        "海报",
        "banner",
        "rollup",
        "roll-up",
        "poster",
    ]
    .iter()
    .any(|keyword| prompt.contains(keyword))
}

fn is_web_request(prompt: &str) -> bool {
    let prompt = prompt.to_lowercase();
    [
        "网站",
        "网页",
        "首页",
        "落地页",
        "官网",
        "站点",
        "landing",
        "homepage",
        "web",
        "site",
        "page",
    ]
    .iter()
    .any(|keyword| prompt.contains(keyword))
}

// ---------------------------------------------------------------------------
// OpenAI-compatible Chat Completions provider (DeepSeek and others)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatApiKind {
    ChatCompletions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub base_url: String,
    pub api_key_env: String,
    /// Optional explicit API key. When set (e.g. from WebUI settings stored in
    /// the OS keychain), it takes precedence over reading `api_key_env` from the
    /// environment. `skip_serializing` ensures a key is never written out.
    #[serde(default, skip_serializing)]
    pub api_key: Option<String>,
    pub model: String,
    pub api_kind: ChatApiKind,
    pub enable_thinking: bool,
    /// Whether the model accepts image content parts. Defaults to false; set
    /// from `REAGENT_SUPPORTS_VISION` in `build_provider` since it can't be
    /// auto-detected.
    #[serde(default)]
    pub supports_vision: bool,
}

impl ProviderConfig {
    pub fn deepseek() -> Self {
        Self {
            name: "deepseek".to_string(),
            base_url: "https://api.deepseek.com".to_string(),
            api_key_env: "DEEPSEEK_API_KEY".to_string(),
            api_key: None,
            model: "deepseek-v4-flash".to_string(),
            api_kind: ChatApiKind::ChatCompletions,
            enable_thinking: false,
            supports_vision: false,
        }
    }

    pub fn openai_compatible(
        name: impl Into<String>,
        base_url: impl Into<String>,
        api_key_env: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            base_url: base_url.into(),
            api_key_env: api_key_env.into(),
            api_key: None,
            model: model.into(),
            api_kind: ChatApiKind::ChatCompletions,
            enable_thinking: false,
            supports_vision: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OpenAiCompatibleChatProvider {
    config: ProviderConfig,
    client: reqwest::Client,
}

impl OpenAiCompatibleChatProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }

    fn endpoint(&self) -> String {
        match self.config.api_kind {
            ChatApiKind::ChatCompletions => {
                format!(
                    "{}/chat/completions",
                    self.config.base_url.trim_end_matches('/')
                )
            }
        }
    }

    fn parse_response(&self, value: &Value) -> Result<ModelResponse> {
        let message = value
            .pointer("/choices/0/message")
            .ok_or_else(|| anyhow!("missing choices[0].message: {value}"))?;

        let reasoning_content = message
            .get("reasoning_content")
            .and_then(Value::as_str)
            .map(str::to_string);

        if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
            if let Some(first) = calls.first() {
                let function = first
                    .get("function")
                    .ok_or_else(|| anyhow!("tool_call missing function: {first}"))?;
                let name = function
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("tool_call function missing name: {first}"))?
                    .to_string();
                let id = first
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("tool-0")
                    .to_string();
                let args = function
                    .get("arguments")
                    .and_then(Value::as_str)
                    .map(|raw| {
                        if raw.trim().is_empty() {
                            Ok(json!({}))
                        } else {
                            serde_json::from_str(raw)
                                .with_context(|| format!("tool_call arguments are not JSON: {raw}"))
                        }
                    })
                    .transpose()?
                    .unwrap_or_else(|| json!({}));

                return Ok(ModelResponse::ToolCall {
                    call: ToolCall { id, name, args },
                    reasoning_content,
                });
            }
        }

        let content = message
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default();
        Ok(ModelResponse::Text {
            content: content.to_string(),
            reasoning_content,
        })
    }
}

#[async_trait]
impl ModelProvider for OpenAiCompatibleChatProvider {
    async fn complete(&self, messages: &[Value], tools: &Value) -> Result<ModelResponse> {
        let api_key = match &self.config.api_key {
            Some(key) => key.clone(),
            None => std::env::var(&self.config.api_key_env).with_context(|| {
                format!(
                    "missing {}, set it or run with --provider heuristic",
                    self.config.api_key_env
                )
            })?,
        };

        let mut body = json!({
            "model": self.config.model,
            "messages": messages,
            "tools": tools,
            "stream": false,
            "temperature": 0.2
        });

        if self.config.enable_thinking {
            body["thinking"] = json!({"type": "enabled"});
            body["reasoning_effort"] = json!("medium");
        }

        let response = self
            .client
            .post(self.endpoint())
            .bearer_auth(api_key)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("failed to call {}", self.config.name))?;

        let status = response.status();
        let value: Value = response
            .json()
            .await
            .with_context(|| format!("failed to parse JSON response from {}", self.config.name))?;

        if !status.is_success() {
            return Err(anyhow!(
                "{} API error {}: {}",
                self.config.name,
                status,
                value
            ));
        }

        self.parse_response(&value)
    }

    fn supports_vision(&self) -> bool {
        self.config.supports_vision
    }
}
