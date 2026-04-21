use std::{
    env, fs,
    net::{SocketAddr, TcpListener},
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, anyhow};
use fabric_client::FabricHostClient;
use tokio::time::{Instant, sleep};

pub struct E2eContext {
    pub client: FabricHostClient,
    pub state_root: PathBuf,
    child: Child,
}

impl E2eContext {
    pub async fn start() -> anyhow::Result<Option<Self>> {
        if env::var("FABRIC_SMOLVM_E2E").as_deref() != Ok("1") {
            eprintln!("skipping smolvm e2e: set FABRIC_SMOLVM_E2E=1 to enable");
            return Ok(None);
        }

        if let Some(rootfs_path) = smolvm_rootfs_path()
            && !rootfs_path.exists()
        {
            eprintln!(
                "skipping smolvm e2e: smolvm agent rootfs not found at {}",
                rootfs_path.display()
            );
            return Ok(None);
        }

        let host_bin = host_bin()?;
        let addr = free_loopback_addr()?;
        let nonce = unique_nonce();
        let state_root = env::temp_dir().join(format!("fabric-host-smolvm-e2e-{nonce}"));
        fs::create_dir_all(&state_root)
            .with_context(|| format!("create state root {}", state_root.display()))?;

        let child = Command::new(&host_bin)
            .arg("--bind")
            .arg(addr.to_string())
            .arg("--state-root")
            .arg(&state_root)
            .arg("--host-id")
            .arg(format!("e2e-{nonce}"))
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("spawn {}", host_bin.display()))?;

        let client = FabricHostClient::new(format!("http://{addr}"));
        wait_for_health(&client).await?;

        Ok(Some(Self {
            client,
            state_root,
            child,
        }))
    }
}

impl Drop for E2eContext {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.state_root);
    }
}

fn host_bin() -> anyhow::Result<PathBuf> {
    if let Some(path) = env::var_os("FABRIC_HOST_BIN") {
        return Ok(PathBuf::from(path));
    }

    if let Some(path) = option_env!("CARGO_BIN_EXE_fabric-host") {
        return Ok(PathBuf::from(path));
    }

    Err(anyhow!(
        "FABRIC_HOST_BIN is not set and Cargo did not provide CARGO_BIN_EXE_fabric-host"
    ))
}

fn smolvm_rootfs_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("FABRIC_SMOLVM_AGENT_ROOTFS") {
        return Some(PathBuf::from(path));
    }

    if let Some(path) = env::var_os("SMOLVM_AGENT_ROOTFS") {
        return Some(PathBuf::from(path));
    }

    if cfg!(target_os = "macos") {
        return env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join("Library/Application Support/smolvm/agent-rootfs"));
    }

    None
}

fn free_loopback_addr() -> anyhow::Result<SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0").context("bind test port probe")?;
    listener.local_addr().context("read test port probe addr")
}

fn unique_nonce() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}-{nanos}", std::process::id())
}

async fn wait_for_health(client: &FabricHostClient) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match client.health().await {
            Ok(response) if response.ok => return Ok(()),
            Ok(_) | Err(_) if Instant::now() < deadline => sleep(Duration::from_millis(100)).await,
            Ok(response) => return Err(anyhow!("hostd health check was unhealthy: {response:?}")),
            Err(error) => return Err(error).context("wait for hostd health"),
        }
    }
}
