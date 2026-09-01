use serde_json::Value;

#[derive(Debug, Default, Clone)]
pub struct MemoryStore {
    facts: Vec<Value>,
}

impl MemoryStore {
    pub fn remember(&mut self, value: Value) {
        self.facts.push(value);
    }

    pub fn all(&self) -> &[Value] {
        &self.facts
    }
}
