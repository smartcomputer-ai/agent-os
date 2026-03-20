use std::collections::BTreeMap;

use aos_cbor::Hash;
use aos_kernel::snapshot::KernelSnapshot;
use serde::Deserialize;

use aos_node::{
    CellStateProjectionRecord, HeadProjectionRecord, PersistError, QueryProjectionMaterialization,
    SnapshotRecord, WorkflowCellStateProjection, WorkspaceRegistryProjectionRecord,
    WorkspaceVersionProjectionRecord,
};

#[derive(Debug, Deserialize)]
struct SnapshotWorkspaceCommitMeta {
    root_hash: String,
    owner: String,
    created_at: u64,
}

#[derive(Debug, Deserialize, Default)]
struct SnapshotWorkspaceHistory {
    latest: u64,
    versions: BTreeMap<u64, SnapshotWorkspaceCommitMeta>,
}

pub fn materialization_from_snapshot(
    record: &SnapshotRecord,
    snapshot_bytes: &[u8],
    updated_at_ns: u64,
) -> Result<QueryProjectionMaterialization, PersistError> {
    let snapshot = decode_snapshot(snapshot_bytes)?;

    if let Some(snapshot_manifest_hash) = snapshot.manifest_hash()
        && let Ok(snapshot_manifest_hash) = Hash::from_bytes(snapshot_manifest_hash)
        && record.manifest_hash.as_deref() != Some(snapshot_manifest_hash.to_hex().as_str())
    {
        return Err(PersistError::validation(format!(
            "snapshot manifest hash {} does not match snapshot record {}",
            snapshot_manifest_hash,
            record.manifest_hash.as_deref().unwrap_or_default()
        )));
    }

    let mut by_workflow: BTreeMap<String, Vec<CellStateProjectionRecord>> = BTreeMap::new();
    for entry in snapshot.workflow_state_entries() {
        let key_bytes = entry.key.clone().unwrap_or_default();
        let key_hash = Hash::of_bytes(&key_bytes);
        let state_hash = Hash::from_bytes(&entry.state_hash)
            .unwrap_or_else(|_| Hash::of_bytes(&entry.state))
            .to_hex();
        by_workflow
            .entry(entry.workflow.clone())
            .or_default()
            .push(CellStateProjectionRecord {
                journal_head: record.height,
                workflow: entry.workflow.clone(),
                key_hash: key_hash.as_bytes().to_vec(),
                key_bytes,
                state_hash,
                size: entry.state.len() as u64,
                last_active_ns: entry.last_active_ns,
            });
    }

    let workflows = by_workflow
        .into_iter()
        .map(|(workflow, mut cells)| {
            cells.sort_by(|left, right| left.key_hash.cmp(&right.key_hash));
            WorkflowCellStateProjection { workflow, cells }
        })
        .collect();
    let mut workspaces = Vec::new();
    for entry in snapshot.workflow_state_entries() {
        if entry.workflow != "sys/Workspace@1" {
            continue;
        }
        let Some(key_bytes) = entry.key.as_ref() else {
            continue;
        };
        let workspace: String = serde_cbor::from_slice(key_bytes)
            .map_err(|err| PersistError::backend(format!("decode workspace key: {err}")))?;
        let history: SnapshotWorkspaceHistory = serde_cbor::from_slice(&entry.state)
            .map_err(|err| PersistError::backend(format!("decode workspace history: {err}")))?;
        if history.versions.is_empty() {
            continue;
        }
        let versions = history
            .versions
            .into_iter()
            .map(|(version, meta)| {
                (
                    version,
                    WorkspaceVersionProjectionRecord {
                        root_hash: meta.root_hash,
                        owner: meta.owner,
                        created_at_ns: meta.created_at,
                    },
                )
            })
            .collect();
        workspaces.push(WorkspaceRegistryProjectionRecord {
            journal_head: record.height,
            workspace,
            latest_version: history.latest,
            versions,
            updated_at_ns,
        });
    }
    workspaces.sort_by(|left, right| left.workspace.cmp(&right.workspace));

    Ok(QueryProjectionMaterialization {
        head: HeadProjectionRecord {
            journal_head: record.height,
            manifest_hash: record
                .manifest_hash
                .clone()
                .expect("validated snapshot record includes manifest_hash"),
            updated_at_ns,
        },
        workflows,
        workspaces,
    })
}

pub fn state_blobs_from_snapshot(
    snapshot_bytes: &[u8],
) -> Result<Vec<(Hash, Vec<u8>)>, PersistError> {
    let snapshot = decode_snapshot(snapshot_bytes)?;
    Ok(snapshot
        .workflow_state_entries()
        .iter()
        .map(|entry| {
            (
                Hash::from_bytes(&entry.state_hash)
                    .unwrap_or_else(|_| Hash::of_bytes(&entry.state)),
                entry.state.clone(),
            )
        })
        .collect())
}

fn decode_snapshot(snapshot_bytes: &[u8]) -> Result<KernelSnapshot, PersistError> {
    serde_cbor::from_slice(snapshot_bytes)
        .map_err(|err| PersistError::backend(format!("decode kernel snapshot: {err}")))
}
