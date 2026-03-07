#![cfg(feature = "foundationdb-backend")]

use std::env;
use std::fs;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use aos_fdb::{
    CasConfig, FdbRuntime, FdbWorldPersistence, InboxConfig, PersistenceConfig, UniverseId, WorldId,
};
use tempfile::TempDir;
use uuid::Uuid;

static RUNTIME: OnceLock<Result<Arc<FdbRuntime>, String>> = OnceLock::new();

#[allow(dead_code)]
pub struct TestContext {
    pub persistence: FdbWorldPersistence,
    pub universe: UniverseId,
    pub world: WorldId,
    pub object_store: TempDir,
}

pub fn cluster_is_reachable() -> bool {
    let cluster_file = env::var_os("FDB_CLUSTER_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/local/etc/foundationdb/fdb.cluster"));
    let cluster_line = match fs::read_to_string(&cluster_file) {
        Ok(contents) => contents
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string(),
        Err(_) => return false,
    };
    let Some(coord_part) = cluster_line.split('@').nth(1) else {
        return false;
    };
    let Some(first_coord) = coord_part.split(',').next() else {
        return false;
    };
    let addresses: Vec<SocketAddr> = match first_coord.to_socket_addrs() {
        Ok(addresses) => addresses.collect(),
        Err(_) => return false,
    };
    addresses
        .into_iter()
        .any(|address| TcpStream::connect_timeout(&address, Duration::from_secs(1)).is_ok())
}

pub fn runtime() -> Result<Arc<FdbRuntime>, Box<dyn std::error::Error>> {
    let runtime = RUNTIME
        .get_or_init(|| {
            FdbRuntime::boot()
                .map(Arc::new)
                .map_err(|err| err.to_string())
        })
        .as_ref()
        .map_err(|err| err.clone())?;
    Ok(Arc::clone(runtime))
}

pub fn test_config() -> PersistenceConfig {
    PersistenceConfig {
        cas: CasConfig {
            inline_threshold_bytes: 8,
            verify_reads: true,
        },
        inbox: InboxConfig {
            inline_payload_threshold_bytes: 8,
        },
        ..PersistenceConfig::default()
    }
}

pub fn open_persistence(
    object_store_root: &std::path::Path,
    config: PersistenceConfig,
) -> Result<FdbWorldPersistence, Box<dyn std::error::Error>> {
    let runtime = runtime()?;
    match env::var_os("FDB_CLUSTER_FILE") {
        Some(cluster_file) => Ok(FdbWorldPersistence::open(
            runtime,
            Some(PathBuf::from(cluster_file)),
            object_store_root,
            config,
        )?),
        None => Ok(FdbWorldPersistence::open_default(
            runtime,
            object_store_root,
            config,
        )?),
    }
}

pub fn open_test_context(
    config: PersistenceConfig,
) -> Result<TestContext, Box<dyn std::error::Error>> {
    let object_store = tempfile::tempdir()?;
    let persistence = open_persistence(object_store.path(), config)?;
    Ok(TestContext {
        persistence,
        universe: UniverseId::from(Uuid::new_v4()),
        world: WorldId::from(Uuid::new_v4()),
        object_store,
    })
}

pub fn skip_if_cluster_unreachable() -> bool {
    if cluster_is_reachable() {
        return false;
    }
    eprintln!("skipping FoundationDB integration test because no local cluster is reachable");
    true
}
