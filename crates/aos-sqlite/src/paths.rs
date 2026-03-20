use std::path::{Path, PathBuf};

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

    pub fn ensure_root(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)
    }

    pub fn sqlite_db(&self) -> PathBuf {
        self.root.join("local-node.sqlite3")
    }

    pub fn cas_root(&self) -> PathBuf {
        self.root.join("cas")
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

    pub fn legacy_store_root(&self) -> PathBuf {
        self.root.join("store")
    }

    pub fn legacy_journal_dir(&self) -> PathBuf {
        self.root.join("journal")
    }

    pub fn legacy_snapshots_dir(&self) -> PathBuf {
        self.root.join("snapshots")
    }

    pub fn legacy_receipts_dir(&self) -> PathBuf {
        self.root.join("receipts")
    }

    pub fn legacy_modules_dir(&self) -> PathBuf {
        self.root.join("modules")
    }

    pub fn purge_legacy_state(&self) -> std::io::Result<()> {
        remove_path_if_exists(&self.legacy_store_root())?;
        remove_path_if_exists(&self.legacy_journal_dir())?;
        remove_path_if_exists(&self.legacy_snapshots_dir())?;
        remove_path_if_exists(&self.legacy_receipts_dir())?;
        remove_path_if_exists(&self.legacy_modules_dir())?;
        Ok(())
    }

    pub fn reset_runtime_state(&self) -> std::io::Result<()> {
        remove_path_if_exists(&self.sqlite_db())?;
        remove_path_if_exists(&self.cas_root())?;
        remove_path_if_exists(&self.run_dir())?;
        remove_path_if_exists(&self.logs_dir())?;
        self.purge_legacy_state()?;
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
