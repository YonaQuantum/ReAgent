use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct SandboxRuntime {
    root: PathBuf,
}

impl SandboxRuntime {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn allows(&self, path: &Path) -> bool {
        path.starts_with(&self.root)
    }
}
