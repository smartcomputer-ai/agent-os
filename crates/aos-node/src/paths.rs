use std::path::{Path, PathBuf};

use crate::UniverseId;

#[derive(Debug, Clone)]
pub struct LocalStatePaths {
    root: PathBuf,
}

impl LocalStatePaths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn from_world_root(world_root: impl AsRef<Path>) -> Self {
        Self::new(world_root.as_ref().join(".aos"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn for_universe(&self, universe_id: UniverseId) -> Self {
        Self::new(self.root.join("universes").join(universe_id.to_string()))
    }

    pub fn world_root(&self) -> Option<&Path> {
        self.root
            .file_name()
            .filter(|name| *name == ".aos")
            .and_then(|_| self.root.parent())
    }

    pub fn ensure_root(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)
    }

    pub fn runtime_db(&self) -> PathBuf {
        self.root.join("runtime.sqlite3")
    }

    pub fn cas_root(&self) -> PathBuf {
        self.root.join("cas")
    }

    pub fn vault_root(&self) -> PathBuf {
        self.root.join("vault")
    }

    pub fn cache_root(&self) -> PathBuf {
        self.root.join("cache")
    }

    pub fn module_cache_dir(&self) -> PathBuf {
        self.cache_root().join("modules")
    }

    pub fn wasmtime_cache_dir(&self) -> PathBuf {
        self.cache_root().join("wasmtime")
    }

    pub fn run_dir(&self) -> PathBuf {
        self.root.join("run")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn runtime_state_file(&self) -> PathBuf {
        self.run_dir().join("node.json")
    }

    pub fn runtime_log_file(&self) -> PathBuf {
        self.logs_dir().join("node.log")
    }

    pub fn reset_runtime_state(&self) -> std::io::Result<()> {
        remove_path_if_exists(&self.runtime_db())?;
        remove_path_if_exists(&self.cas_root())?;
        remove_path_if_exists(&self.vault_root())?;
        remove_path_if_exists(&self.run_dir())?;
        remove_path_if_exists(&self.logs_dir())?;
        Ok(())
    }
}

fn remove_path_if_exists(path: &Path) -> std::io::Result<()> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}
