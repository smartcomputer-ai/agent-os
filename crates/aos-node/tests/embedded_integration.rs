mod support;

use std::ops::Deref;
use std::sync::{Mutex, MutexGuard};

use aos_cbor::to_canonical_cbor;
use aos_effect_types::{GovPatchInput, GovProposeParams};
use aos_kernel::Store;
use aos_kernel::governance::ManifestPatch;
use aos_node::{
    CborPayload, DomainEventIngress, EmbeddedWorldHarness, ForkPendingEffectPolicy,
    ForkWorldRequest, FsCas,
};
use tempfile::TempDir;
use uuid::Uuid;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    _guard: MutexGuard<'static, ()>,
    saved: Vec<(&'static str, Option<String>)>,
}

impl EnvGuard {
    fn set(vars: &[(&'static str, Option<&str>)]) -> Self {
        let guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let saved = vars
            .iter()
            .map(|(key, value)| {
                let prior = std::env::var(key).ok();
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
                (*key, prior)
            })
            .collect();
        Self {
            _guard: guard,
            saved,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..) {
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

struct HarnessFixture {
    _temp: TempDir,
    harness: EmbeddedWorldHarness,
    _env_guard: EnvGuard,
}

impl Deref for HarnessFixture {
    type Target = EmbeddedWorldHarness;

    fn deref(&self) -> &Self::Target {
        &self.harness
    }
}

fn harness() -> Result<HarnessFixture, Box<dyn std::error::Error>> {
    harness_with_env(&[
        ("AOS_CHECKPOINT_INTERVAL_MS", None),
        ("AOS_CHECKPOINT_EVERY_EVENTS", None),
    ])
}

fn eager_checkpoint_harness() -> Result<HarnessFixture, Box<dyn std::error::Error>> {
    harness_with_env(&[
        ("AOS_CHECKPOINT_INTERVAL_MS", Some("120000")),
        ("AOS_CHECKPOINT_EVERY_EVENTS", Some("1")),
    ])
}

fn harness_with_env(
    vars: &[(&'static str, Option<&str>)],
) -> Result<HarnessFixture, Box<dyn std::error::Error>> {
    let env_guard = EnvGuard::set(vars);
    let temp = TempDir::new()?;
    let harness = EmbeddedWorldHarness::open(temp.path().join(".aos").as_path())?;
    Ok(HarnessFixture {
        _temp: temp,
        harness,
        _env_guard: env_guard,
    })
}

fn has_cached_modules(cache_root: &std::path::Path) -> Result<bool, std::io::Error> {
    if !cache_root.exists() {
        return Ok(false);
    }
    let mut pending = vec![cache_root.to_path_buf()];
    while let Some(path) = pending.pop() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let entry_path = entry.path();
            if entry.file_type()?.is_dir() {
                pending.push(entry_path);
            } else if entry.file_name() == std::ffi::OsStr::new("module.cmod") {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[test]
fn embedded_harness_batches_event_into_authoritative_frame()
-> Result<(), Box<dyn std::error::Error>> {
    let harness = eager_checkpoint_harness()?;
    let store = support::new_mem_store();
    let loaded = support::simple_state_manifest(&store);
    let world = Uuid::new_v4().into();

    harness.create_world_from_loaded_manifest(store.as_ref(), &loaded, world, 123)?;
    let seq = harness.control().enqueue_event(
        world,
        DomainEventIngress {
            schema: support::START_SCHEMA.into(),
            value: CborPayload::inline(to_canonical_cbor(&support::start_event("frame"))?),
            key: None,
            correlation_id: Some("frame".into()),
        },
    )?;
    assert_eq!(seq.to_string(), "0000000000000000");

    let state = harness
        .control()
        .state_get(world, "com.acme/Simple@1", None, None)?;
    assert_eq!(state.state_b64.as_deref(), Some("qg=="));

    let stepped = harness.control().step_world(world)?;
    assert!(!stepped.runtime.has_pending_maintenance);

    let journal = harness.control().journal_entries(world, 0, 128)?;
    assert!(journal.entries.is_empty());
    let head = harness.control().journal_head(world)?;
    assert!(head.journal_head > 0);
    assert_eq!(head.retained_from, head.journal_head);
    Ok(())
}

#[test]
fn embedded_harness_replays_world_from_authoritative_frames_on_reopen()
-> Result<(), Box<dyn std::error::Error>> {
    let harness = harness()?;
    let store = support::new_mem_store();
    let loaded = support::simple_state_manifest(&store);
    let world = Uuid::new_v4().into();

    harness.create_world_from_loaded_manifest(store.as_ref(), &loaded, world, 123)?;
    harness.control().enqueue_event(
        world,
        DomainEventIngress {
            schema: support::START_SCHEMA.into(),
            value: CborPayload::inline(to_canonical_cbor(&support::start_event("reopen"))?),
            key: None,
            correlation_id: Some("reopen".into()),
        },
    )?;

    let reopened = harness.reopen()?;
    let state = reopened
        .control()
        .state_get(world, "com.acme/Simple@1", None, None)?;
    assert_eq!(state.state_b64.as_deref(), Some("qg=="));
    Ok(())
}

#[test]
fn embedded_harness_create_and_reopen_populate_wasmtime_cache()
-> Result<(), Box<dyn std::error::Error>> {
    let harness = harness()?;
    let store = support::new_mem_store();
    let loaded = support::simple_state_manifest(&store);
    let world = Uuid::new_v4().into();

    harness.create_world_from_loaded_manifest(store.as_ref(), &loaded, world, 123)?;
    let cache_dir = harness.paths().wasmtime_cache_dir();
    assert!(has_cached_modules(&cache_dir)?);

    if cache_dir.exists() {
        std::fs::remove_dir_all(&cache_dir)?;
    }
    let reopened = harness.reopen()?;
    let reopened_cache_dir = reopened.paths().wasmtime_cache_dir();
    assert!(has_cached_modules(&reopened_cache_dir)?);
    Ok(())
}

#[test]
fn embedded_harness_forks_world_from_active_baseline() -> Result<(), Box<dyn std::error::Error>> {
    let harness = harness()?;
    let store = support::new_mem_store();
    let loaded = support::simple_state_manifest(&store);
    let src_world = Uuid::new_v4().into();
    let fork_world = Uuid::new_v4().into();

    harness.create_world_from_loaded_manifest(store.as_ref(), &loaded, src_world, 10)?;
    harness.control().enqueue_event(
        src_world,
        DomainEventIngress {
            schema: support::START_SCHEMA.into(),
            value: CborPayload::inline(to_canonical_cbor(&support::start_event("src"))?),
            key: None,
            correlation_id: Some("src".into()),
        },
    )?;

    let forked = harness.control().fork_world(ForkWorldRequest {
        src_world_id: src_world,
        src_snapshot: aos_node::SnapshotSelector::ActiveBaseline,
        new_world_id: Some(fork_world),
        forked_at_ns: 12,
        pending_effect_policy: ForkPendingEffectPolicy::ClearAllPendingExternalState,
    })?;
    assert_eq!(forked.record.world_id, fork_world);

    harness.control().enqueue_event(
        fork_world,
        DomainEventIngress {
            schema: support::START_SCHEMA.into(),
            value: CborPayload::inline(to_canonical_cbor(&support::start_event("fork"))?),
            key: None,
            correlation_id: Some("fork".into()),
        },
    )?;

    let state = harness
        .control()
        .state_get(fork_world, "com.acme/Simple@1", None, None)?;
    assert_eq!(state.state_b64.as_deref(), Some("qg=="));
    Ok(())
}

#[test]
fn embedded_harness_processes_governance_command_submission()
-> Result<(), Box<dyn std::error::Error>> {
    let harness = harness()?;
    let store = support::new_mem_store();
    let loaded = support::simple_state_manifest(&store);
    let world = Uuid::new_v4().into();

    harness.create_world_from_loaded_manifest(store.as_ref(), &loaded, world, 0)?;
    let manifest = harness.control().manifest(world)?;
    let manifest_hash = aos_cbor::Hash::from_hex_str(&manifest.manifest_hash)?;
    let cas = FsCas::open_with_paths(harness.paths())?;
    let payload = serde_cbor::to_vec(&GovProposeParams {
        patch: GovPatchInput::PatchCbor(serde_cbor::to_vec(&ManifestPatch {
            manifest: cas.get_node(manifest_hash)?,
            nodes: Vec::new(),
        })?),
        summary: None,
        manifest_base: None,
        description: Some("embedded harness proposal".into()),
    })?;

    let response = harness.control().submit_command(
        world,
        "gov-propose",
        Some("proposal-1".into()),
        None,
        &serde_cbor::from_slice::<serde_cbor::Value>(&payload)?,
    )?;
    assert_eq!(response.command_id, "proposal-1");

    let record = harness.control().get_command(world, "proposal-1")?;
    assert!(matches!(record.status, aos_node::CommandStatus::Succeeded));
    assert!(record.journal_height.is_some());
    Ok(())
}

#[test]
fn embedded_harness_drives_timer_receipts_through_same_local_scheduler()
-> Result<(), Box<dyn std::error::Error>> {
    let harness = harness()?;
    let store = support::new_mem_store();
    let loaded = support::timer_manifest(&store);
    let world = Uuid::new_v4().into();

    harness.create_world_from_loaded_manifest(store.as_ref(), &loaded, world, 1)?;
    harness.control().enqueue_event(
        world,
        DomainEventIngress {
            schema: support::START_SCHEMA.into(),
            value: CborPayload::inline(to_canonical_cbor(&support::start_event("timer"))?),
            key: None,
            correlation_id: Some("timer".into()),
        },
    )?;

    let _ = harness.control().step_world(world)?;
    let runtime = harness.control().runtime(world)?;
    assert!(!runtime.has_pending_inbox);
    assert!(runtime.has_pending_effects);
    assert!(runtime.has_pending_maintenance);
    Ok(())
}

#[test]
fn embedded_harness_checkpoint_advances_retained_tail_and_survives_reopen()
-> Result<(), Box<dyn std::error::Error>> {
    let harness = harness()?;
    let store = support::new_mem_store();
    let loaded = support::simple_state_manifest(&store);
    let world = Uuid::new_v4().into();

    harness.create_world_from_loaded_manifest(store.as_ref(), &loaded, world, 123)?;
    assert!(!harness.control().runtime(world)?.has_pending_maintenance);

    harness.control().enqueue_event(
        world,
        DomainEventIngress {
            schema: support::START_SCHEMA.into(),
            value: CborPayload::inline(to_canonical_cbor(&support::start_event("checkpoint"))?),
            key: None,
            correlation_id: Some("checkpoint".into()),
        },
    )?;

    let checkpointed = harness.control().checkpoint_world(world)?;
    assert!(!checkpointed.runtime.has_pending_maintenance);

    let after = harness.control().journal_head(world)?;
    assert_eq!(after.retained_from, after.journal_head);

    let reopened = harness.reopen()?;
    assert!(!reopened.control().runtime(world)?.has_pending_maintenance);
    let state = reopened
        .control()
        .state_get(world, "com.acme/Simple@1", None, None)?;
    assert_eq!(state.state_b64.as_deref(), Some("qg=="));
    Ok(())
}

#[test]
fn embedded_harness_step_world_auto_checkpoints_idle_worlds()
-> Result<(), Box<dyn std::error::Error>> {
    let harness = eager_checkpoint_harness()?;
    let store = support::new_mem_store();
    let loaded = support::simple_state_manifest(&store);
    let world = Uuid::new_v4().into();

    harness.create_world_from_loaded_manifest(store.as_ref(), &loaded, world, 123)?;
    harness.control().enqueue_event(
        world,
        DomainEventIngress {
            schema: support::START_SCHEMA.into(),
            value: CborPayload::inline(to_canonical_cbor(&support::start_event("checkpoint"))?),
            key: None,
            correlation_id: Some("checkpoint".into()),
        },
    )?;

    let stepped = harness.control().step_world(world)?;
    assert!(!stepped.runtime.has_pending_maintenance);
    assert!(stepped.runtime.active_baseline_height.unwrap_or_default() > 1);

    let reopened = harness.reopen()?;
    assert!(!reopened.control().runtime(world)?.has_pending_maintenance);
    Ok(())
}
