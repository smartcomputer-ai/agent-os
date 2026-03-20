use aos_kernel::snapshot::{KernelSnapshot, WorkflowInstanceSnapshot, WorkflowStatusSnapshot};

use aos_node::{ForkPendingEffectPolicy, PersistError};

fn manifest_hash_bytes(snapshot: &KernelSnapshot) -> Result<Option<[u8; 32]>, PersistError> {
    snapshot
        .manifest_hash()
        .map(|bytes| {
            <[u8; 32]>::try_from(bytes).map_err(|_| {
                PersistError::backend("kernel snapshot manifest hash must be 32 bytes")
            })
        })
        .transpose()
}

fn sanitize_workflow_instance(
    mut snapshot: WorkflowInstanceSnapshot,
) -> (WorkflowInstanceSnapshot, bool) {
    let mut changed = false;
    if !snapshot.inflight_intents.is_empty() {
        snapshot.inflight_intents.clear();
        changed = true;
    }
    if matches!(snapshot.status, WorkflowStatusSnapshot::Waiting) {
        snapshot.status = WorkflowStatusSnapshot::Running;
        changed = true;
    }
    (snapshot, changed)
}

pub(crate) fn rewrite_snapshot_for_fork_policy(
    bytes: &[u8],
    policy: &ForkPendingEffectPolicy,
) -> Result<Option<Vec<u8>>, PersistError> {
    match policy {
        ForkPendingEffectPolicy::ClearAllPendingExternalState => {}
    }

    let snapshot: KernelSnapshot = serde_cbor::from_slice(bytes)
        .map_err(|err| PersistError::backend(format!("decode kernel snapshot for fork: {err}")))?;

    let mut changed =
        !snapshot.queued_effects().is_empty() || !snapshot.pending_workflow_receipts().is_empty();
    let workflow_instances: Vec<_> = snapshot
        .workflow_instances()
        .iter()
        .cloned()
        .map(|instance| {
            let (instance, instance_changed) = sanitize_workflow_instance(instance);
            changed |= instance_changed;
            instance
        })
        .collect();

    if !changed {
        return Ok(None);
    }

    let mut sanitized = KernelSnapshot::new(
        snapshot.height(),
        snapshot.workflow_state_entries().to_vec(),
        snapshot.recent_receipts().to_vec(),
        Vec::new(),
        Vec::new(),
        workflow_instances,
        snapshot.logical_now_ns(),
        manifest_hash_bytes(&snapshot)?,
    );
    sanitized.set_workflow_index_roots(snapshot.workflow_index_roots().to_vec());
    sanitized.set_root_completeness(snapshot.root_completeness().clone());
    serde_cbor::to_vec(&sanitized)
        .map(Some)
        .map_err(|err| PersistError::backend(format!("encode fork-sanitized snapshot: {err}")))
}
