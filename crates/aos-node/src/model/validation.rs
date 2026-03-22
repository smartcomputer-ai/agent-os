use super::types::*;

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

#[allow(dead_code)]
pub fn validate_create_world_request(request: &CreateWorldRequest) -> Result<(), PersistError> {
    match &request.source {
        CreateWorldSource::Seed { seed } => validate_world_seed(seed),
        CreateWorldSource::Manifest { manifest_hash } if manifest_hash.trim().is_empty() => Err(
            PersistError::validation("manifest source requires a non-empty manifest_hash"),
        ),
        CreateWorldSource::Manifest { .. } => Ok(()),
    }
}

pub fn validate_fork_world_request(request: &ForkWorldRequest) -> Result<(), PersistError> {
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
