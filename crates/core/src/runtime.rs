use std::path::PathBuf;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::agent::{AgentLoop, AgentRunConfig, AgentRunOutput};
use crate::capability::load_capabilities;
use crate::model::{HeuristicPlanner, ModelProvider, OpenAiCompatibleChatProvider, ProviderConfig};
use crate::tool::ToolRegistry;

/// A ready-to-run agent: capabilities loaded from a directory, plus a provider
/// name resolved into a model adapter. This is the single entry point shared by
/// the CLI and the server so neither needs to know how tools are discovered.
#[derive(Debug, Clone)]
pub struct Runtime {
    tools: ToolRegistry,
    system_prompt: String,
}

impl Runtime {
    /// Load every capability under `capabilities_dir` into a tool registry.
    pub fn load(capabilities_dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = capabilities_dir.into();
        let capabilities = load_capabilities(&dir)?;
        if capabilities.is_empty() {
            return Err(anyhow!(
                "no capabilities found under {}; add a subdirectory with a manifest.json",
                dir.display()
            ));
        }

        let mut tools = ToolRegistry::new();
        tools.register_builtins(allow_local_fs());
        for capability in capabilities {
            for tool in capability.into_tools() {
                tools.register(tool);
            }
        }

        Ok(Self {
            system_prompt: build_system_prompt(&tools),
            tools,
        })
    }

    pub fn tools(&self) -> &ToolRegistry {
        &self.tools
    }

    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    pub async fn run<M>(&self, model: M, config: AgentRunConfig) -> Result<AgentRunOutput>
    where
        M: ModelProvider + Send + Sync,
    {
        AgentLoop::new(model, self.tools.clone(), self.system_prompt.clone())
            .run(config)
            .await
    }
}

/// User-facing model configuration (provider + credentials) as entered in the
/// WebUI settings. Non-secret fields are persisted to a plain config file; the
/// `api_key` is held in the OS keychain and never serialized back out.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelSettings {
    /// `deepseek`, `openai-compatible`, or `heuristic`.
    pub provider: String,
    #[serde(default, skip_serializing)]
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub api_key_env: Option<String>,
}

/// Resolve a provider from explicit user settings, falling back to environment
/// variables for any field the settings leave empty. This is the path the server
/// uses once the user has configured a model in the WebUI.
pub fn build_provider_from_settings(
    settings: &ModelSettings,
) -> Result<Box<dyn ModelProvider + Send + Sync>> {
    match settings.provider.as_str() {
        "heuristic" => Ok(Box::new(HeuristicPlanner)),
        "deepseek" => {
            let api_key = settings
                .api_key
                .clone()
                .or_else(|| std::env::var("DEEPSEEK_API_KEY").ok())
                .ok_or_else(|| {
                    anyhow!(
                        "DEEPSEEK_API_KEY is not set. Use --provider heuristic for offline demo."
                    )
                })?;
            let mut config = ProviderConfig::deepseek();
            if let Some(model) = &settings.model {
                config.model = model.clone();
            } else if let Ok(model) = std::env::var("DEEPSEEK_MODEL") {
                config.model = model;
            }
            config.api_key = Some(api_key);
            // A model whose name advertises vision gets previews fed back as
            // images; `REAGENT_SUPPORTS_VISION` remains an explicit override for
            // names that don't say so.
            config.supports_vision =
                vision_enabled() || config.model.to_lowercase().contains("vision");
            Ok(Box::new(OpenAiCompatibleChatProvider::new(config)))
        }
        "openai-compatible" => {
            let base_url = settings
                .base_url
                .clone()
                .or_else(|| std::env::var("REAGENT_MODEL_BASE_URL").ok())
                .ok_or_else(|| {
                    anyhow!("REAGENT_MODEL_BASE_URL is required for openai-compatible")
                })?;
            let api_key_env = settings
                .api_key_env
                .clone()
                .unwrap_or_else(|| "OPENAI_API_KEY".to_string());
            let model = settings
                .model
                .clone()
                .or_else(|| std::env::var("REAGENT_MODEL").ok())
                .ok_or_else(|| anyhow!("REAGENT_MODEL is required"))?;
            let api_key = settings
                .api_key
                .clone()
                .or_else(|| std::env::var(&api_key_env).ok());
            let mut config = ProviderConfig::openai_compatible(
                "openai-compatible",
                base_url,
                api_key_env,
                model,
            );
            config.api_key = api_key;
            config.supports_vision = vision_enabled();
            Ok(Box::new(OpenAiCompatibleChatProvider::new(config)))
        }
        other => Err(anyhow!(
            "unknown provider {other}; use deepseek, openai-compatible, or heuristic"
        )),
    }
}

/// Resolve a provider name into a model adapter using environment variables
/// only. Provider-specific env vars are read here, once, so callers don't
/// duplicate the branching.
pub fn build_provider(name: &str) -> Result<Box<dyn ModelProvider + Send + Sync>> {
    build_provider_from_settings(&ModelSettings {
        provider: name.to_string(),
        ..Default::default()
    })
}

/// A generic system prompt that describes the workspace workflow, without
/// naming any specific capability. Tool names and schemas are supplied
/// separately via the function-calling `tools` parameter (and summarized inline
/// for models that need it).
fn build_system_prompt(tools: &ToolRegistry) -> String {
    let mut summary = String::new();
    for tool in tools.iter() {
        summary.push_str(&format!("- {}: {}\n", tool.name(), tool.description()));
    }

    let name = agent_name();
    format!(
        r#"You are {name}, a general agent that turns natural-language requests into concrete artifacts.

You work inside a per-run workspace. The runtime injects `artifact_dir` into every tool call; treat it as your project folder and use workspace-relative paths only.

Available tools:
{summary}
Workflow:
- Create and edit files with the file tools (`write_file`, `read_file`, `list_files`). Build the artifact incrementally rather than in one shot.
- Use the capability's render/inspect tools to actually see and measure your work, read the returned preview and diagnostics, then fix files and re-render. Iterate until it is right.
- Read any capability guidance files (e.g. `instructions.md`, `print_rules.json`) with `read_file` when you are unsure of the requirements.
- Files the user attached or a previous step ingested are staged in the workspace `input/` subdirectory; consume them with `list_files` / `read_file` or the parse tools as needed.
- Only call tools from the list above. Do not invent local filesystem paths; everything you write lives in the workspace.

Completion: do not claim the task is finished until the deliverable is fully produced — the runtime will hold you to it. If it tells you something is still missing, keep working. Once a render looks acceptable, run the check tool and then export to finish, rather than re-rendering forever. When it is truly done, reply with a short final message in the user's language."#
    )
}

/// The human-facing agent name, used in the system prompt and surfaced by the
/// server for the UI. Decoupled from code via env so a rename is a config
/// change rather than an edit.
pub fn agent_name() -> String {
    std::env::var("REAGENT_NAME").unwrap_or_else(|_| "ReAgent".to_string())
}

/// Whether the current model should receive rendered previews as image input.
/// Read from env because vision support cannot be auto-detected from a config
/// name.
fn vision_enabled() -> bool {
    std::env::var("REAGENT_SUPPORTS_VISION")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Whether to register the `ingest_file` built-in that reads arbitrary local
/// absolute paths into the workspace. Off by default: the model is a cloud API,
/// so reading local files exfiltrates their contents to the model provider.
fn allow_local_fs() -> bool {
    std::env::var("REAGENT_ALLOW_LOCAL_FS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}
