use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use aos_cbor::Hash;
use aos_kernel::{Store, StoreError, StoreResult};

use crate::PersistError;
use crate::paths::LocalStatePaths;

#[derive(Debug, Clone)]
pub struct FsCas {
    root: PathBuf,
}

impl FsCas {
    pub fn open_with_paths(paths: &LocalStatePaths) -> Result<Self, PersistError> {
        Self::open_cas_root(paths.cas_root())
    }

    pub fn open_cas_root(root: impl Into<PathBuf>) -> Result<Self, PersistError> {
        let root = root.into();
        fs::create_dir_all(&root)
            .map_err(|err| PersistError::backend(format!("create local CAS dir: {err}")))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    pub fn put_verified(&self, bytes: &[u8]) -> Result<Hash, PersistError> {
        let hash = Hash::of_bytes(bytes);
        let path = self.blob_path(hash);
        if path.exists() {
            return Ok(hash);
        }

        let parent = path
            .parent()
            .ok_or_else(|| PersistError::backend("invalid local CAS blob path"))?;
        fs::create_dir_all(parent)
            .map_err(|err| PersistError::backend(format!("create local CAS shard dir: {err}")))?;

        let temp_name = format!(
            ".{}.tmp-{}-{}",
            hash.to_hex(),
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|err| PersistError::backend(format!(
                    "clock error while writing CAS: {err}"
                )))?
                .as_nanos()
        );
        let temp_path = parent.join(temp_name);
        {
            let mut file = fs::File::create(&temp_path).map_err(|err| {
                PersistError::backend(format!("create local CAS temp file: {err}"))
            })?;
            file.write_all(bytes).map_err(|err| {
                PersistError::backend(format!("write local CAS temp file: {err}"))
            })?;
            file.sync_all()
                .map_err(|err| PersistError::backend(format!("sync local CAS temp file: {err}")))?;
        }
        fs::rename(&temp_path, &path).map_err(|err| {
            if path.exists() {
                let _ = fs::remove_file(&temp_path);
                PersistError::backend("local CAS write raced with existing blob")
            } else {
                PersistError::backend(format!("rename local CAS temp file: {err}"))
            }
        })?;
        Ok(hash)
    }

    pub fn has(&self, hash: Hash) -> bool {
        self.blob_path(hash).exists()
    }

    pub fn get(&self, hash: Hash) -> Result<Vec<u8>, PersistError> {
        let path = self.blob_path(hash);
        let bytes = fs::read(&path).map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                PersistError::not_found(format!("blob {}", hash.to_hex()))
            } else {
                PersistError::backend(format!("read local CAS blob: {err}"))
            }
        })?;
        let actual = Hash::of_bytes(&bytes);
        if actual != hash {
            return Err(PersistError::backend(format!(
                "local CAS hash mismatch for {}: actual {}",
                hash.to_hex(),
                actual.to_hex()
            )));
        }
        Ok(bytes)
    }

    pub fn all_hashes(&self) -> Result<Vec<Hash>, PersistError> {
        let mut hashes = Vec::new();
        for shard in fs::read_dir(&self.root)
            .map_err(|err| PersistError::backend(format!("list local CAS shards: {err}")))?
        {
            let shard = shard
                .map_err(|err| PersistError::backend(format!("read local CAS shard: {err}")))?;
            if !shard
                .file_type()
                .map_err(|err| PersistError::backend(format!("read local CAS shard type: {err}")))?
                .is_dir()
            {
                continue;
            }
            let prefix = shard.file_name().to_string_lossy().to_string();
            if prefix.len() != 2 || !prefix.chars().all(|ch| ch.is_ascii_hexdigit()) {
                continue;
            }
            for entry in fs::read_dir(shard.path()).map_err(|err| {
                PersistError::backend(format!("list local CAS shard entries: {err}"))
            })? {
                let entry = entry.map_err(|err| {
                    PersistError::backend(format!("read local CAS shard entry: {err}"))
                })?;
                if !entry
                    .file_type()
                    .map_err(|err| {
                        PersistError::backend(format!("read local CAS entry type: {err}"))
                    })?
                    .is_file()
                {
                    continue;
                }
                let suffix = entry.file_name().to_string_lossy().to_string();
                if suffix.starts_with('.') {
                    continue;
                }
                let hex = format!("{prefix}{suffix}");
                let hash = Hash::from_hex_str(&format!("sha256:{hex}")).map_err(|err| {
                    PersistError::backend(format!("decode local CAS hash '{hex}': {err}"))
                })?;
                hashes.push(hash);
            }
        }
        hashes.sort();
        hashes.dedup();
        Ok(hashes)
    }

    fn blob_path(&self, hash: Hash) -> PathBuf {
        let digest_hex = hex::encode(hash.as_bytes());
        let (prefix, rest) = digest_hex.split_at(2);
        self.root.join(prefix).join(rest)
    }
}

impl Store for FsCas {
    fn put_node<T: serde::Serialize>(&self, value: &T) -> StoreResult<Hash> {
        let bytes = aos_cbor::to_canonical_cbor(value)?;
        self.put_blob(&bytes)
    }

    fn get_node<T: serde::de::DeserializeOwned>(&self, hash: Hash) -> StoreResult<T> {
        let bytes = self.get_blob(hash)?;
        serde_cbor::from_slice(&bytes).map_err(StoreError::from)
    }

    fn has_node(&self, hash: Hash) -> StoreResult<bool> {
        self.has_blob(hash)
    }

    fn put_blob(&self, bytes: &[u8]) -> StoreResult<Hash> {
        self.put_verified(bytes)
            .map_err(|err| err.into_store_error(".aos/cas", "local CAS entry missing"))
    }

    fn get_blob(&self, hash: Hash) -> StoreResult<Vec<u8>> {
        self.get(hash)
            .map_err(|err| err.into_store_error(".aos/cas", "local CAS entry missing"))
    }

    fn has_blob(&self, hash: Hash) -> StoreResult<bool> {
        Ok(self.has(hash))
    }
}
