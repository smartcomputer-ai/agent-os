use std::collections::BTreeMap;

use aos_cbor::Hash;

use super::config::*;
use super::identity::*;
use super::model::*;

pub fn gc_bucket_for(timestamp_ns: u64, bucket_width_ns: u64) -> u64 {
    if bucket_width_ns == 0 {
        0
    } else {
        timestamp_ns / bucket_width_ns
    }
}

pub fn maintenance_due(
    journal_head: JournalHeight,
    active_baseline_height: Option<JournalHeight>,
    first_hot_journal_height: Option<JournalHeight>,
    config: SnapshotMaintenanceConfig,
) -> bool {
    let Some(active_baseline_height) = active_baseline_height else {
        return false;
    };

    let tail_entries_after_baseline =
        journal_head.saturating_sub(active_baseline_height.saturating_add(1));
    let snapshot_due = tail_entries_after_baseline >= config.snapshot_after_journal_entries;
    if snapshot_due {
        return true;
    }

    let safe_exclusive_end = active_baseline_height.saturating_sub(config.segment_hot_tail_margin);
    first_hot_journal_height.is_some_and(|first_hot| first_hot < safe_exclusive_end)
}

const MAX_HANDLE_LEN: usize = 63;

pub fn normalize_handle(value: &str) -> Result<String, PersistError> {
    let handle = value.trim().to_ascii_lowercase();
    if handle.is_empty() {
        return Err(PersistError::validation("handle must be non-empty"));
    }
    if handle.len() > MAX_HANDLE_LEN {
        return Err(PersistError::validation(format!(
            "handle must be at most {MAX_HANDLE_LEN} characters",
        )));
    }
    let bytes = handle.as_bytes();
    let first = bytes[0];
    let last = bytes[bytes.len() - 1];
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(PersistError::validation(
            "handle must start with a lowercase letter or digit",
        ));
    }
    if !last.is_ascii_lowercase() && !last.is_ascii_digit() {
        return Err(PersistError::validation(
            "handle must end with a lowercase letter or digit",
        ));
    }
    if !bytes
        .iter()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
    {
        return Err(PersistError::validation(
            "handle may only contain lowercase letters, digits, and hyphens",
        ));
    }
    Ok(handle)
}

pub fn default_universe_handle(universe: UniverseId) -> String {
    format!("u-{}", universe.as_uuid().simple())
}

pub fn default_world_handle(world: WorldId) -> String {
    format!("w-{}", world.as_uuid().simple())
}

pub fn sample_world_meta(world: WorldId) -> WorldMeta {
    WorldMeta {
        handle: default_world_handle(world),
        manifest_hash: None,
        active_baseline_height: None,
        placement_pin: None,
        created_at_ns: 0,
        lineage: None,
        admin: WorldAdminLifecycle::default(),
    }
}

pub fn validate_world_seed(seed: &WorldSeed) -> Result<(), PersistError> {
    validate_baseline_promotion_record(&seed.baseline)?;
    match (&seed.seed_kind, &seed.imported_from) {
        (SeedKind::Genesis, _) => Ok(()),
        (SeedKind::Import, Some(imported_from)) if !imported_from.source.trim().is_empty() => {
            Ok(())
        }
        (SeedKind::Import, Some(_)) => Err(PersistError::validation(
            "import seeds require a non-empty imported_from.source",
        )),
        (SeedKind::Import, None) => Err(PersistError::validation(
            "import seeds require imported_from provenance",
        )),
    }
}

pub fn validate_create_world_seed_request(
    request: &CreateWorldSeedRequest,
) -> Result<(), PersistError> {
    validate_world_seed(&request.seed)?;
    if let Some(handle) = &request.handle {
        let _ = normalize_handle(handle)?;
    }
    if request
        .placement_pin
        .as_ref()
        .is_some_and(|pin| pin.trim().is_empty())
    {
        return Err(PersistError::validation(
            "placement_pin must be non-empty when provided",
        ));
    }
    Ok(())
}

#[allow(dead_code)]
pub fn validate_create_world_request(request: &CreateWorldRequest) -> Result<(), PersistError> {
    if let Some(handle) = &request.handle {
        let _ = normalize_handle(handle)?;
    }
    if request
        .placement_pin
        .as_ref()
        .is_some_and(|pin| pin.trim().is_empty())
    {
        return Err(PersistError::validation(
            "placement_pin must be non-empty when provided",
        ));
    }
    match &request.source {
        CreateWorldSource::Seed { seed } => validate_world_seed(seed),
        CreateWorldSource::Manifest { manifest_hash } if manifest_hash.trim().is_empty() => Err(
            PersistError::validation("manifest source requires a non-empty manifest_hash"),
        ),
        CreateWorldSource::Manifest { .. } => Ok(()),
    }
}

pub fn validate_fork_world_request(request: &ForkWorldRequest) -> Result<(), PersistError> {
    if let Some(handle) = &request.handle {
        let _ = normalize_handle(handle)?;
    }
    if request
        .placement_pin
        .as_ref()
        .is_some_and(|pin| pin.trim().is_empty())
    {
        return Err(PersistError::validation(
            "placement_pin must be non-empty when provided",
        ));
    }
    match &request.src_snapshot {
        SnapshotSelector::ActiveBaseline | SnapshotSelector::ByHeight { .. } => {}
        SnapshotSelector::ByRef { snapshot_ref } if !snapshot_ref.trim().is_empty() => {}
        SnapshotSelector::ByRef { .. } => {
            return Err(PersistError::validation(
                "snapshot selector by_ref requires a non-empty snapshot_ref",
            ));
        }
    }
    Ok(())
}

pub fn ensure_monotonic_snapshot_records(
    records: &BTreeMap<JournalHeight, SnapshotRecord>,
    record: &SnapshotRecord,
) -> Result<(), PersistError> {
    validate_snapshot_record(record)?;
    if let Some(existing) = records.get(&record.height) {
        if existing == record {
            return Ok(());
        }
        return Err(PersistConflict::SnapshotExists {
            height: record.height,
        }
        .into());
    }
    Ok(())
}

pub fn validate_snapshot_record(record: &SnapshotRecord) -> Result<(), PersistError> {
    if record.snapshot_ref.is_empty() {
        return Err(PersistError::validation(
            "snapshot_ref must be non-empty for indexed snapshots",
        ));
    }
    match record.manifest_hash.as_deref() {
        Some(hash) if !hash.is_empty() => Ok(()),
        _ => Err(PersistError::validation(
            "snapshot index requires manifest_hash for restore root completeness",
        )),
    }
}

pub fn validate_baseline_promotion_record(record: &SnapshotRecord) -> Result<(), PersistError> {
    validate_snapshot_record(record)?;
    let Some(horizon) = record.receipt_horizon_height else {
        return Err(PersistError::validation(
            "baseline promotion requires receipt_horizon_height",
        ));
    };
    if horizon != record.height {
        return Err(PersistError::validation(format!(
            "baseline receipt_horizon_height ({horizon}) must equal baseline height ({})",
            record.height
        )));
    }
    Ok(())
}

pub fn can_upgrade_snapshot_record(existing: &SnapshotRecord, desired: &SnapshotRecord) -> bool {
    existing.height == desired.height
        && existing.snapshot_ref == desired.snapshot_ref
        && existing.receipt_horizon_height.is_none()
        && desired.receipt_horizon_height == Some(desired.height)
}

pub fn validate_snapshot_commit_request(
    request: &SnapshotCommitRequest,
) -> Result<(), PersistError> {
    validate_snapshot_record(&request.record)?;
    if request.snapshot_journal_entry.is_empty() {
        return Err(PersistError::validation(
            "snapshot commit requires a snapshot journal entry payload",
        ));
    }
    match (
        request.promote_baseline,
        request.baseline_journal_entry.as_ref(),
    ) {
        (true, Some(bytes)) if !bytes.is_empty() => {
            validate_baseline_promotion_record(&request.record)
        }
        (true, _) => Err(PersistError::validation(
            "baseline promotion requires a baseline journal entry payload",
        )),
        (false, Some(_)) => Err(PersistError::validation(
            "baseline journal entry payload requires promote_baseline=true",
        )),
        (false, None) => Ok(()),
    }
}

pub fn validate_head_projection_record(record: &HeadProjectionRecord) -> Result<(), PersistError> {
    if record.manifest_hash.trim().is_empty() {
        return Err(PersistError::validation(
            "head projection requires a non-empty manifest_hash",
        ));
    }
    Hash::from_hex_str(&record.manifest_hash).map_err(|err| {
        PersistError::validation(format!(
            "invalid head projection manifest_hash '{}': {err}",
            record.manifest_hash
        ))
    })?;
    Ok(())
}

pub fn validate_cell_state_projection_record(
    record: &CellStateProjectionRecord,
) -> Result<(), PersistError> {
    if record.workflow.trim().is_empty() {
        return Err(PersistError::validation(
            "cell projection requires a non-empty workflow name",
        ));
    }
    if record.key_hash.len() != 32 {
        return Err(PersistError::validation(format!(
            "cell projection key_hash must be 32 bytes, got {}",
            record.key_hash.len()
        )));
    }
    Hash::from_hex_str(&record.state_hash).map_err(|err| {
        PersistError::validation(format!(
            "invalid cell projection state_hash '{}': {err}",
            record.state_hash
        ))
    })?;
    Ok(())
}

pub fn validate_query_projection_materialization(
    materialization: &QueryProjectionMaterialization,
) -> Result<(), PersistError> {
    validate_head_projection_record(&materialization.head)?;
    for workflow in &materialization.workflows {
        if workflow.workflow.trim().is_empty() {
            return Err(PersistError::validation(
                "workflow cell projection requires a non-empty workflow name",
            ));
        }
        for cell in &workflow.cells {
            validate_cell_state_projection_record(cell)?;
            if cell.workflow != workflow.workflow {
                return Err(PersistError::validation(format!(
                    "workflow cell projection mismatch: container '{}' contains '{}'",
                    workflow.workflow, cell.workflow
                )));
            }
            if cell.journal_head > materialization.head.journal_head {
                return Err(PersistError::validation(format!(
                    "cell projection journal_head {} exceeds head projection {}",
                    cell.journal_head, materialization.head.journal_head
                )));
            }
        }
    }
    for workspace in &materialization.workspaces {
        validate_workspace_registry_projection_record(workspace)?;
        if workspace.journal_head > materialization.head.journal_head {
            return Err(PersistError::validation(format!(
                "workspace projection journal_head {} exceeds head projection {}",
                workspace.journal_head, materialization.head.journal_head
            )));
        }
    }
    Ok(())
}

pub fn validate_query_projection_delta(delta: &QueryProjectionDelta) -> Result<(), PersistError> {
    validate_head_projection_record(&delta.head)?;
    for cell in &delta.cell_upserts {
        validate_cell_state_projection_record(cell)?;
        if cell.journal_head > delta.head.journal_head {
            return Err(PersistError::validation(format!(
                "cell projection journal_head {} exceeds head projection {}",
                cell.journal_head, delta.head.journal_head
            )));
        }
    }
    for cell in &delta.cell_deletes {
        if cell.workflow.trim().is_empty() {
            return Err(PersistError::validation(
                "cell projection delete requires a non-empty workflow name",
            ));
        }
        if cell.key_hash.len() != 32 {
            return Err(PersistError::validation(format!(
                "cell projection delete key_hash must be 32 bytes, got {}",
                cell.key_hash.len()
            )));
        }
    }
    for workspace in &delta.workspace_upserts {
        validate_workspace_registry_projection_record(workspace)?;
        if workspace.journal_head > delta.head.journal_head {
            return Err(PersistError::validation(format!(
                "workspace projection journal_head {} exceeds head projection {}",
                workspace.journal_head, delta.head.journal_head
            )));
        }
    }
    for workspace in &delta.workspace_deletes {
        if workspace.workspace.trim().is_empty() {
            return Err(PersistError::validation(
                "workspace projection delete requires a non-empty workspace name",
            ));
        }
    }
    Ok(())
}

pub fn validate_workspace_registry_projection_record(
    record: &WorkspaceRegistryProjectionRecord,
) -> Result<(), PersistError> {
    if record.workspace.trim().is_empty() {
        return Err(PersistError::validation(
            "workspace projection requires a non-empty workspace name",
        ));
    }
    if !record.versions.contains_key(&record.latest_version) {
        return Err(PersistError::validation(format!(
            "workspace projection latest_version {} missing from versions map",
            record.latest_version
        )));
    }
    for version in record.versions.values() {
        if version.root_hash.trim().is_empty() {
            return Err(PersistError::validation(
                "workspace version projection requires a non-empty root_hash",
            ));
        }
        Hash::from_hex_str(&version.root_hash).map_err(|err| {
            PersistError::validation(format!(
                "invalid workspace projection root_hash '{}': {err}",
                version.root_hash
            ))
        })?;
    }
    Ok(())
}
