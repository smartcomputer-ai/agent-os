use std::{collections::BTreeMap, sync::Arc, time::Duration};

use fabric_client::FabricControllerClient;
use fabric_protocol::{
    FabricHostProvider, HostHeartbeatRequest, HostId, HostRegisterRequest, NetworkMode,
    ProviderCapacity, SessionStatus, SmolvmProviderInfo,
};
use tokio::time::sleep;
use tracing::{info, warn};

use crate::FabricHostService;

pub fn spawn_controller_registration(service: Arc<FabricHostService>) {
    let Some(controller_url) = service.config().controller_url.clone() else {
        return;
    };
    let advertise_url = service
        .config()
        .advertise_url
        .clone()
        .unwrap_or_else(|| format!("http://{}", service.config().bind_addr));
    let configured_interval = ns_to_duration(service.config().heartbeat_interval_ns);

    tokio::spawn(async move {
        let client = FabricControllerClient::new(controller_url);
        let mut interval = loop {
            match register_once(&client, &service, &advertise_url).await {
                Ok(next_interval) => {
                    break next_interval.unwrap_or(configured_interval);
                }
                Err(error) => {
                    warn!(%error, "fabric host controller registration failed");
                    sleep(configured_interval).await;
                }
            }
        };

        loop {
            sleep(interval).await;
            match heartbeat_once(&client, &service, &advertise_url).await {
                Ok(next_interval) => {
                    if let Some(next_interval) = next_interval {
                        interval = next_interval;
                    }
                }
                Err(error) => {
                    warn!(%error, "fabric host controller heartbeat failed");
                }
            }
        }
    });
}

async fn register_once(
    client: &FabricControllerClient,
    service: &FabricHostService,
    advertise_url: &str,
) -> anyhow::Result<Option<Duration>> {
    let host_id = HostId(service.config().host_id.clone());
    let response = client
        .register_host(&HostRegisterRequest {
            host_id: host_id.clone(),
            endpoint: advertise_url.to_owned(),
            providers: provider_records(service).await?,
            labels: BTreeMap::new(),
        })
        .await?;

    info!(host_id = %host_id.0, "fabric host registered with controller");
    Ok(Some(ns_to_duration(response.heartbeat_interval_ns)))
}

async fn heartbeat_once(
    client: &FabricControllerClient,
    service: &FabricHostService,
    advertise_url: &str,
) -> anyhow::Result<Option<Duration>> {
    let host_id = HostId(service.config().host_id.clone());
    let inventory = service.inventory().await?;
    let response = client
        .heartbeat_host(
            &host_id,
            &HostHeartbeatRequest {
                host_id: host_id.clone(),
                endpoint: Some(advertise_url.to_owned()),
                providers: provider_records_from_inventory(service, Some(&inventory)),
                inventory: Some(inventory),
                labels: BTreeMap::new(),
            },
        )
        .await?;

    Ok(Some(ns_to_duration(response.heartbeat_interval_ns)))
}

async fn provider_records(service: &FabricHostService) -> anyhow::Result<Vec<FabricHostProvider>> {
    let inventory = service.inventory().await.ok();
    Ok(provider_records_from_inventory(service, inventory.as_ref()))
}

fn provider_records_from_inventory(
    service: &FabricHostService,
    inventory: Option<&fabric_protocol::HostInventoryResponse>,
) -> Vec<FabricHostProvider> {
    let host_info = service.host_info();
    let active_sessions = inventory
        .map(|inventory| {
            inventory
                .sessions
                .iter()
                .filter(|session| !matches!(session.status, SessionStatus::Closed))
                .count() as u64
        })
        .unwrap_or(0);

    vec![FabricHostProvider::Smolvm(SmolvmProviderInfo {
        runtime_version: host_info.runtime_version,
        supported_runtime_classes: vec!["smolvm".to_owned()],
        allowed_images: host_info.allowed_images,
        allowed_network_modes: if host_info.allowed_network_modes.is_empty() {
            vec![NetworkMode::Disabled]
        } else {
            host_info.allowed_network_modes
        },
        resource_defaults: host_info.resource_defaults,
        resource_max: host_info.resource_max,
        capacity: ProviderCapacity {
            max_sessions: None,
            active_sessions,
            max_concurrent_execs: None,
            active_execs: 0,
        },
    })]
}

fn ns_to_duration(ns: u128) -> Duration {
    let ns = u64::try_from(ns).unwrap_or(u64::MAX);
    Duration::from_nanos(ns.max(1))
}
