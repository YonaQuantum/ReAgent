use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context as _, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tool::Tool;

/// One tool exposed by a capability. Tools are the model-facing unit: a name,
/// a description, a JSON Schema for the arguments, and a subprocess to run
/// (the worker that owns the domain logic).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityTool {
    /// Tool name the model sees and calls. Must match `^[a-zA-Z0-9_-]+$`.
    pub name: String,
    /// What the tool does; surfaced to the model for routing.
    pub description: String,
    /// JSON Schema of the worker's stdin contract (model-facing arguments).
    #[serde(default)]
    pub parameters: Value,
    /// Executable to invoke, e.g. `python3`.
    pub command: String,
    /// Worker script path, relative to the manifest directory.
    pub entry: String,
}

/// A declarative description of a capability: a bundle of related tools plus
/// routing metadata. Each capability lives in its own directory next to a
/// `manifest.json`. The kernel loads these at startup and registers every tool,
/// so a new use case is a new manifest + workers — no kernel code changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityManifest {
    /// Stable capability id, e.g. `rollup_banner`. Informational; tool names are
    /// what the model calls.
    pub id: String,
    /// Short human-readable name.
    #[serde(default)]
    pub name: String,
    /// What the capability does, as a whole.
    #[serde(default)]
    pub description: String,
    /// Extra routing hints (e.g. for an offline planner).
    #[serde(default)]
    pub match_keywords: Vec<String>,
    /// The tools this capability exposes.
    pub tools: Vec<CapabilityTool>,
}

/// A capability loaded from disk, together with the directory it came from so
/// worker entry paths can be resolved.
#[derive(Debug, Clone)]
pub struct Capability {
    pub manifest: CapabilityManifest,
    pub dir: PathBuf,
}

impl Capability {
    pub fn into_tools(&self) -> Vec<Tool> {
        self.manifest
            .tools
            .iter()
            .map(|tool| {
                let entry = self.dir.join(&tool.entry);
                Tool::new(
                    tool.name.clone(),
                    tool.description.clone(),
                    tool.parameters.clone(),
                    resolve_command(&tool.command),
                    vec![entry.display().to_string()],
                )
            })
            .collect()
    }
}

/// Load every capability under `root`: each direct child directory containing a
/// `manifest.json` contributes one capability (with one or more tools).
pub fn load_capabilities(root: &Path) -> Result<Vec<Capability>> {
    let mut capabilities = Vec::new();

    for entry in fs::read_dir(root)
        .with_context(|| format!("failed to read capabilities dir {}", root.display()))?
    {
        let entry = entry?;
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let manifest_path = dir.join("manifest.json");
        if !manifest_path.is_file() {
            continue;
        }

        let raw = fs::read_to_string(&manifest_path)
            .with_context(|| format!("failed to read {}", manifest_path.display()))?;
        let manifest: CapabilityManifest = serde_json::from_str(&raw)
            .with_context(|| format!("invalid manifest {}", manifest_path.display()))?;

        for tool in &manifest.tools {
            if !is_valid_tool_id(&tool.name) {
                return Err(anyhow!(
                    "invalid tool name {:?} in {}: must match ^[a-zA-Z0-9_-]+$ (no dots or spaces)",
                    tool.name,
                    manifest_path.display()
                ));
            }
        }

        capabilities.push(Capability { manifest, dir });
    }

    capabilities.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
    Ok(capabilities)
}

/// Resolve the executable for a process tool. Manifests author the portable
/// name `python3`; on Windows that interpreter is `python`, and users can
/// override either with `REAGENT_PYTHON`. Kept here (rather than per-tool) so
/// every capability resolves identically.
fn resolve_command(command: &str) -> String {
    if command == "python3" || command == "python" {
        python_interpreter()
    } else {
        command.to_string()
    }
}

fn python_interpreter() -> String {
    if let Ok(explicit) = std::env::var("REAGENT_PYTHON") {
        return explicit;
    }
    if cfg!(windows) {
        "python".to_string()
    } else {
        "python3".to_string()
    }
}

/// Tool names are exposed as OpenAI function-calling names, which only allow
/// `[a-zA-Z0-9_-]`. Validate here so a bad manifest fails at load, not at
/// request time with a confusing model-API error.
fn is_valid_tool_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}
