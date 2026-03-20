#![cfg(feature = "foundationdb-backend")]

use std::env;
use std::fs;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Once, OnceLock};
use std::thread::sleep;
use std::time::Duration;

use aos_fdb::{
    CasConfig, FdbRuntime, FdbWorldPersistence, InboxConfig, PersistenceConfig, UniverseId, WorldId,
};
use uuid::Uuid;

static RUNTIME: OnceLock<Result<Arc<FdbRuntime>, String>> = OnceLock::new();
static DOTENV: Once = Once::new();

#[allow(dead_code)]
pub struct TestContext {
    pub persistence: FdbWorldPersistence,
    pub universe: UniverseId,
    pub world: WorldId,
}

pub fn load_repo_dotenv() {
    DOTENV.call_once(|| {
        for path in dotenv_candidates() {
            if !path.exists() {
                continue;
            }
            if let Ok(iter) = dotenvy::from_path_iter(&path) {
                for item in iter.flatten() {
                    let (key, value) = item;
                    if std::env::var_os(&key).is_none() {
                        unsafe {
                            std::env::set_var(&key, &value);
                        }
                    }
                }
            }
        }
    });
}

pub fn cluster_is_reachable() -> bool {
    load_repo_dotenv();
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
    let socket_reachable = addresses
        .into_iter()
        .any(|address| TcpStream::connect_timeout(&address, Duration::from_secs(1)).is_ok());
    socket_reachable && fdbcli_status_ok(&cluster_file, Duration::from_secs(3))
}

pub fn runtime() -> Result<Arc<FdbRuntime>, Box<dyn std::error::Error>> {
    load_repo_dotenv();
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
            verify_reads: true,
            ..CasConfig::default()
        },
        inbox: InboxConfig {
            inline_payload_threshold_bytes: 8,
        },
        ..PersistenceConfig::default()
    }
}

#[allow(dead_code)]
pub fn open_persistence(
    config: PersistenceConfig,
) -> Result<FdbWorldPersistence, Box<dyn std::error::Error>> {
    let runtime = runtime()?;
    match env::var_os("FDB_CLUSTER_FILE") {
        Some(cluster_file) => Ok(FdbWorldPersistence::open(
            runtime,
            Some(PathBuf::from(cluster_file)),
            config,
        )?),
        None => Ok(FdbWorldPersistence::open_default(runtime, config)?),
    }
}

#[allow(dead_code)]
pub fn open_test_context(
    config: PersistenceConfig,
) -> Result<TestContext, Box<dyn std::error::Error>> {
    let persistence = open_persistence(config)?;
    Ok(TestContext {
        persistence,
        universe: UniverseId::from(Uuid::new_v4()),
        world: WorldId::from(Uuid::new_v4()),
    })
}

pub fn skip_if_cluster_unreachable() -> bool {
    if cluster_is_reachable() {
        return false;
    }
    eprintln!(
        "skipping FoundationDB integration test because no responsive local cluster is reachable"
    );
    true
}

fn dotenv_candidates() -> Vec<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    vec![
        workspace_root().join(".env"),
        manifest_dir.join(".env"),
        PathBuf::from(".env"),
    ]
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

fn fdbcli_status_ok(cluster_file: &Path, timeout: Duration) -> bool {
    let mut child = match Command::new("fdbcli")
        .arg("-C")
        .arg(cluster_file)
        .arg("--exec")
        .arg("status minimal")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return false,
    };
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
            Ok(None) => sleep(Duration::from_millis(50)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}
