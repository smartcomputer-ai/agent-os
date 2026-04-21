use std::time::{SystemTime, UNIX_EPOCH};

use fabric_protocol::{FabricHostProvider, FabricSandboxTarget, HostStatus, HostSummary};

use crate::FabricControllerError;

#[derive(Debug, Clone)]
pub struct ScheduledHost {
    pub host: HostSummary,
}

pub fn schedule_sandbox(
    hosts: &[HostSummary],
    target: &FabricSandboxTarget,
    heartbeat_timeout_ns: u128,
) -> Result<ScheduledHost, FabricControllerError> {
    let now_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| FabricControllerError::Time(error.to_string()))?
        .as_nanos();

    let mut eligible = hosts
        .iter()
        .filter(|host| is_host_eligible(host, target, now_ns, heartbeat_timeout_ns))
        .cloned()
        .collect::<Vec<_>>();

    eligible.sort_by(|left, right| left.host_id.0.cmp(&right.host_id.0));
    eligible
        .into_iter()
        .next()
        .map(|host| ScheduledHost { host })
        .ok_or_else(|| {
            FabricControllerError::NoHealthyHost(
                "no healthy smolvm host can satisfy sandbox target".to_owned(),
            )
        })
}

fn is_host_eligible(
    host: &HostSummary,
    target: &FabricSandboxTarget,
    now_ns: u128,
    heartbeat_timeout_ns: u128,
) -> bool {
    if host.status != HostStatus::Healthy {
        return false;
    }

    let Some(last_heartbeat_ns) = host.last_heartbeat_ns else {
        return false;
    };
    if now_ns.saturating_sub(last_heartbeat_ns) > heartbeat_timeout_ns {
        return false;
    }

    host.providers
        .iter()
        .any(|provider| smolvm_provider_allows(provider, target))
}

fn smolvm_provider_allows(provider: &FabricHostProvider, target: &FabricSandboxTarget) -> bool {
    let FabricHostProvider::Smolvm(info) = provider else {
        return false;
    };

    let runtime_class = target.runtime_class.as_deref().unwrap_or("smolvm");
    if !info.supported_runtime_classes.is_empty()
        && !info
            .supported_runtime_classes
            .iter()
            .any(|class| class == runtime_class)
    {
        return false;
    }

    if !info.allowed_images.is_empty()
        && !info.allowed_images.iter().any(|image| {
            image == "*" || image == target.image.as_str() || image == target.image.trim()
        })
    {
        return false;
    }

    let requested_network = target.network_mode;
    if !info.allowed_network_modes.is_empty()
        && !info
            .allowed_network_modes
            .iter()
            .any(|mode| *mode == requested_network)
    {
        return false;
    }

    resource_fits(
        target.resources.cpu_limit_millis,
        info.resource_max.cpu_limit_millis,
    ) && resource_fits(
        target.resources.memory_limit_bytes,
        info.resource_max.memory_limit_bytes,
    ) && capacity_available(info.capacity.max_sessions, info.capacity.active_sessions)
}

fn resource_fits(requested: Option<u64>, max: Option<u64>) -> bool {
    match (requested, max) {
        (Some(requested), Some(max)) => requested <= max,
        _ => true,
    }
}

fn capacity_available(max_sessions: Option<u64>, active_sessions: u64) -> bool {
    match max_sessions {
        Some(max_sessions) => active_sessions < max_sessions,
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use fabric_protocol::{
        FabricHostProvider, HostId, NetworkMode, ProviderCapacity, ResourceLimits,
        SmolvmProviderInfo,
    };

    use super::*;

    fn host(host_id: &str, active_sessions: u64) -> HostSummary {
        HostSummary {
            host_id: HostId(host_id.to_owned()),
            endpoint: format!("http://{host_id}"),
            status: HostStatus::Healthy,
            providers: vec![FabricHostProvider::Smolvm(SmolvmProviderInfo {
                runtime_version: None,
                supported_runtime_classes: vec!["smolvm".to_owned()],
                allowed_images: vec!["*".to_owned()],
                allowed_network_modes: vec![NetworkMode::Disabled, NetworkMode::Egress],
                resource_defaults: ResourceLimits::default(),
                resource_max: ResourceLimits::default(),
                capacity: ProviderCapacity {
                    max_sessions: Some(10),
                    active_sessions,
                    max_concurrent_execs: None,
                    active_execs: 0,
                },
            })],
            labels: BTreeMap::new(),
            last_heartbeat_ns: Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
            ),
            created_at_ns: 1,
            updated_at_ns: 1,
        }
    }

    fn target() -> FabricSandboxTarget {
        FabricSandboxTarget {
            image: "alpine:latest".to_owned(),
            runtime_class: Some("smolvm".to_owned()),
            workdir: None,
            env: BTreeMap::new(),
            network_mode: NetworkMode::Egress,
            mounts: Vec::new(),
            resources: ResourceLimits::default(),
        }
    }

    #[test]
    fn scheduler_picks_lowest_eligible_host_id() {
        let selected = schedule_sandbox(
            &[host("host-b", 0), host("host-a", 0)],
            &target(),
            30_000_000_000,
        )
        .unwrap();

        assert_eq!(selected.host.host_id.0, "host-a");
    }

    #[test]
    fn scheduler_rejects_capacity_exhausted_hosts() {
        let mut exhausted = host("host-a", 10);
        let FabricHostProvider::Smolvm(info) = &mut exhausted.providers[0] else {
            unreachable!();
        };
        info.capacity.max_sessions = Some(10);

        let selected =
            schedule_sandbox(&[exhausted, host("host-b", 0)], &target(), 30_000_000_000).unwrap();

        assert_eq!(selected.host.host_id.0, "host-b");
    }
}
