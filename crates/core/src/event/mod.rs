use std::path::PathBuf;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: Uuid,
    pub at: DateTime<Utc>,
    pub kind: EventKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    UserMessage {
        content: String,
    },
    Thought {
        content: String,
    },
    ToolCall {
        id: String,
        name: String,
        args: Value,
    },
    ToolResult {
        id: String,
        ok: bool,
        output: Value,
    },
    ArtifactCreated {
        artifact: Value,
    },
    Final {
        content: String,
    },
}

#[derive(Debug, Default, Clone)]
pub struct EventStream {
    events: Vec<Event>,
    jsonl_path: Option<PathBuf>,
    sink: Option<UnboundedSender<Event>>,
}

impl EventStream {
    pub fn with_jsonl(path: PathBuf) -> Self {
        Self {
            events: Vec::new(),
            jsonl_path: Some(path),
            sink: None,
        }
    }

    /// Like `with_jsonl`, but also forwards every event to `sink` as it happens,
    /// so callers can stream live progress (e.g. to an SSE response).
    pub fn with_jsonl_and_sink(path: PathBuf, sink: UnboundedSender<Event>) -> Self {
        Self {
            events: Vec::new(),
            jsonl_path: Some(path),
            sink: Some(sink),
        }
    }

    pub fn push(&mut self, kind: EventKind) {
        let event = Event {
            id: Uuid::new_v4(),
            at: Utc::now(),
            kind,
        };
        if let Some(sink) = &self.sink {
            let _ = sink.send(event.clone());
        }
        self.events.push(event);
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Event> {
        self.events.iter()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn has_artifact_kind(&self, kind: &str) -> bool {
        self.events.iter().any(|event| match &event.kind {
            EventKind::ArtifactCreated { artifact } => {
                artifact.get("kind").and_then(Value::as_str) == Some(kind)
            }
            _ => false,
        })
    }

    pub async fn flush(&self) -> Result<()> {
        if let Some(path) = &self.jsonl_path {
            let mut body = String::new();
            for event in &self.events {
                body.push_str(&serde_json::to_string(event)?);
                body.push('\n');
            }
            tokio::fs::write(path, body).await?;
        }
        Ok(())
    }
}
