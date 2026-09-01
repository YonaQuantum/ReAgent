mod settings;

use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use axum::extract::{Multipart, State};
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use reagent_core::{
    agent_name, build_provider, build_provider_from_settings, AgentRunConfig, AgentRunOutput,
    Event, EventKind, ModelSettings, Runtime,
};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{mpsc, RwLock};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::StreamExt;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use uuid::Uuid;

use crate::settings::{describe, load_settings, save_settings, SettingsResponse, SettingsUpdate};

#[derive(Debug, Clone)]
struct AppState {
    artifact_dir: PathBuf,
    runtime: Arc<Runtime>,
    /// Saved WebUI model config. `None` means nothing configured yet, so the
    /// server falls back to environment variables.
    settings: Arc<RwLock<Option<ModelSettings>>>,
}

#[derive(Debug, Deserialize)]
struct RunRequest {
    prompt: String,
    provider: Option<String>,
    /// Workspace-relative paths (as returned by `/api/upload`) of files the user
    /// attached to this run.
    #[serde(default)]
    files: Vec<String>,
}

/// Messages the run task forwards to the SSE stream.
enum StreamMsg {
    Event(Event),
    Done(AgentRunOutput),
    Error(String),
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let repo_root = std::env::var("REAGENT_REPO_ROOT")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let artifact_dir = repo_root.join("artifacts");
    let capabilities_dir = repo_root.join("capabilities");
    let webui_dist = repo_root.join("apps").join("webui").join("dist");
    tokio::fs::create_dir_all(&artifact_dir).await?;

    let runtime = Arc::new(Runtime::load(capabilities_dir)?);

    let settings = Arc::new(RwLock::new(load_settings().unwrap_or_else(|err| {
        eprintln!("warning: failed to load settings ({err}); starting unconfigured");
        None
    })));

    let state = Arc::new(AppState {
        artifact_dir: artifact_dir.clone(),
        runtime,
        settings,
    });

    let app = Router::new()
        .route("/health", get(health))
        .route("/api/settings", get(get_settings).put(update_settings))
        .route("/api/runs", post(stream_run))
        .route("/api/upload", post(upload_file))
        .nest_service("/artifacts", ServeDir::new(artifact_dir))
        .fallback_service(ServeDir::new(webui_dist.clone()))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let bind = std::env::var("REAGENT_SERVER_BIND").unwrap_or_else(|_| "0.0.0.0:8787".to_string());
    let addr: SocketAddr = bind.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("reagent-server listening on http://{addr}");
    if webui_dist.join("index.html").is_file() {
        println!("webui: serving {}", webui_dist.display());
    } else {
        println!(
            "webui: {} not found — serving API only (build the frontend with `npm run build`)",
            webui_dist.display()
        );
    }
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({"ok": true, "name": agent_name()}))
}

/// Current model config, with the API key masked (never returned in full).
async fn get_settings(State(state): State<Arc<AppState>>) -> Json<SettingsResponse> {
    let guard = state.settings.read().await;
    Json(match &*guard {
        Some(settings) => describe(settings),
        None => describe(&ModelSettings::default()),
    })
}

/// Save model config: non-secret fields to a config file, the API key to the OS
/// keychain. When the request omits a key and doesn't ask to clear it, the
/// existing stored key is carried forward.
async fn update_settings(
    State(state): State<Arc<AppState>>,
    Json(mut update): Json<SettingsUpdate>,
) -> Result<Json<SettingsResponse>, (StatusCode, String)> {
    if update.settings.provider.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "provider is required".to_string()));
    }
    if update.settings.api_key.is_none() && !update.clear_api_key {
        let guard = state.settings.read().await;
        if let Some(current) = &*guard {
            update.settings.api_key = current.api_key.clone();
        }
    }

    save_settings(&update.settings, update.clear_api_key)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    if update.clear_api_key {
        update.settings.api_key = None;
    }
    *state.settings.write().await = Some(update.settings.clone());

    Ok(Json(describe(&update.settings)))
}

/// Accept a single multipart file, store it under `artifacts/uploads/<uuid>/`,
/// and return its workspace-relative path so the client can attach it to a run.
async fn upload_file(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let field = multipart
        .next_field()
        .await
        .map_err(upload_err)?
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing file field".to_string()))?;
    let name = field.file_name().unwrap_or("upload").to_string();
    let data = field.bytes().await.map_err(upload_err)?;

    let dir = state
        .artifact_dir
        .join("uploads")
        .join(Uuid::new_v4().to_string());
    tokio::fs::create_dir_all(&dir).await.map_err(upload_err)?;
    let path = dir.join(&name);
    tokio::fs::write(&path, &data).await.map_err(upload_err)?;

    let rel = path
        .strip_prefix(&state.artifact_dir)
        .map_err(upload_err)?
        .to_string_lossy()
        .to_string();
    Ok(Json(
        json!({ "path": rel, "name": name, "bytes": data.len() }),
    ))
}

fn upload_err<E: std::fmt::Display>(error: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

/// Resolve client-supplied upload paths (from `/api/upload`) to absolute paths,
/// rejecting anything that escapes the `uploads/` directory or does not exist.
fn resolve_input_files(artifact_dir: &std::path::Path, files: &[String]) -> Vec<PathBuf> {
    let uploads = artifact_dir.join("uploads");
    let uploads_canonical = match uploads.canonicalize() {
        Ok(path) => path,
        Err(_) => return Vec::new(),
    };
    files
        .iter()
        .filter_map(|rel| {
            let canonical = artifact_dir.join(rel).canonicalize().ok()?;
            if canonical.is_file() && canonical.starts_with(&uploads_canonical) {
                Some(canonical)
            } else {
                None
            }
        })
        .collect()
}

/// Run an agent turn and stream its trajectory over SSE (`text/event-stream`).
///
/// Each trajectory event is emitted as it happens (`user_message`, `thought`,
/// `tool_call`, `tool_result`, `artifact_created`, `final`), followed by a
/// terminal `done` (with the full `AgentRunOutput`) or `error` event.
async fn stream_run(
    State(state): State<Arc<AppState>>,
    Json(request): Json<RunRequest>,
) -> Sse<impl tokio_stream::Stream<Item = Result<SseEvent, Infallible>>> {
    let provider_name = request.provider.unwrap_or_else(|| "deepseek".to_string());

    // Use saved WebUI settings when present; otherwise fall back to the env-var
    // path keyed by the request's provider name.
    let settings = state.settings.read().await.clone();
    let provider_result = match &settings {
        Some(s) => build_provider_from_settings(s),
        None => build_provider(&provider_name),
    };

    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<Event>();
    let (msg_tx, msg_rx) = mpsc::unbounded_channel::<StreamMsg>();

    // Forward core events into the outgoing message channel.
    let forward = msg_tx.clone();
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            if forward.send(StreamMsg::Event(event)).is_err() {
                break;
            }
        }
    });

    // Run the agent; send a terminal message when it finishes or fails.
    let runtime = state.runtime.clone();
    let artifact_dir = state.artifact_dir.clone();
    let input_files = resolve_input_files(&artifact_dir, &request.files);
    let done = msg_tx.clone();
    tokio::spawn(async move {
        let provider = match provider_result {
            Ok(provider) => provider,
            Err(error) => {
                let _ = done.send(StreamMsg::Error(error.to_string()));
                return;
            }
        };
        let config = AgentRunConfig {
            user_prompt: request.prompt,
            artifact_dir,
            max_steps: 24,
            input_files,
            event_tx: Some(event_tx),
        };
        match runtime.run(provider, config).await {
            Ok(output) => {
                let _ = done.send(StreamMsg::Done(output));
            }
            Err(error) => {
                let _ = done.send(StreamMsg::Error(error.to_string()));
            }
        }
    });

    let stream = UnboundedReceiverStream::new(msg_rx).map(to_sse_event);
    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn to_sse_event(msg: StreamMsg) -> Result<SseEvent, Infallible> {
    match msg {
        StreamMsg::Event(event) => Ok(SseEvent::default()
            .event(event_name(&event.kind))
            .data(serde_json::to_string(&event).unwrap_or_default())),
        StreamMsg::Done(output) => Ok(SseEvent::default()
            .event("done")
            .data(serde_json::to_string(&output).unwrap_or_default())),
        StreamMsg::Error(error) => Ok(SseEvent::default().event("error").data(error)),
    }
}

fn event_name(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::UserMessage { .. } => "user_message",
        EventKind::Thought { .. } => "thought",
        EventKind::ToolCall { .. } => "tool_call",
        EventKind::ToolResult { .. } => "tool_result",
        EventKind::ArtifactCreated { .. } => "artifact_created",
        EventKind::Final { .. } => "final",
    }
}
