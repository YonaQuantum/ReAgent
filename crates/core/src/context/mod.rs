use crate::event::{EventKind, EventStream};

#[derive(Debug, Default, Clone)]
pub struct ContextEngine;

impl ContextEngine {
    pub fn build(&self, events: &EventStream) -> String {
        let mut lines = Vec::new();
        for event in events.iter().rev().take(12).rev() {
            let line = match &event.kind {
                EventKind::UserMessage { content } => format!("user: {content}"),
                EventKind::Thought { content } => format!("thought: {content}"),
                EventKind::ToolCall { name, .. } => format!("tool_call: {name}"),
                EventKind::ToolResult { ok, output, .. } => {
                    format!("tool_result: ok={ok} output={output}")
                }
                EventKind::ArtifactCreated { artifact } => format!("artifact: {artifact}"),
                EventKind::Final { content } => format!("final: {content}"),
            };
            lines.push(line);
        }
        lines.join("\n")
    }
}
