use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use aos_cbor::Hash;
use aos_kernel::{
    CellProjectionDelta, Store, WorkspaceProjectionDelta, WorkspaceProjectionDeltaState,
};
use aos_node::{CborPayload, SnapshotRecord, SnapshotSelector, WorldId};
use aos_runtime::WorldHost;
use serde::Deserialize;

use crate::kafka::{
    CellProjectionUpsert, ProjectionKey, ProjectionRecord, ProjectionValue,
    WorkspaceProjectionUpsert, WorldMetaProjection,
};
use crate::materializer::{
    CellStateProjectionRecord, WorkspaceRegistryProjectionRecord, WorkspaceVersionProjectionRecord,
};

use super::types::{HostedWorkerRuntimeInner, WorkerError};

#[derive(Debug, Default, Deserialize)]
struct WorkspaceHistoryState {
    latest: u64,
    versions: BTreeMap<u64, WorkspaceCommitMetaState>,
}

#[derive(Debug, Deserialize)]
struct WorkspaceCommitMetaState {
    root_hash: String,
    owner: String,
    #[serde(alias = "created_at")]
    created_at: u64,
}

impl HostedWorkerRuntimeInner {
    pub(super) fn emit_projection_updates_for_worlds(
        &mut self,
        world_ids: &[WorldId],
    ) -> Result<(), WorkerError> {
        let mut records = Vec::new();
        let mut published = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for world_id in world_ids {
            if !seen.insert(*world_id) {
                continue;
            }
            let plan = self.build_projection_snapshot_for_world(*world_id)?;
            records.extend(plan.records);
            published.push(plan.published);
        }
        if records.is_empty() {
            return Ok(());
        }
        if let Err(err) = self.infra.kafka.publish_projection_records(records) {
            for published in published {
                let _ = self.invalidate_projection_continuity(published.world_id);
            }
            return Err(WorkerError::LogFirst(err));
        }
        for published in published {
            self.record_projection_publish_success(
                published.world_id,
                published.journal_head,
                published.active_baseline,
            )?;
        }
        Ok(())
    }

    fn build_projection_snapshot_for_world(
        &mut self,
        world_id: WorldId,
    ) -> Result<ProjectionEmissionPlan, WorkerError> {
        let (universe_id, world_epoch, projection_token, workflow_modules, store) =
            {
                let world = self.state.registered_worlds.get(&world_id).ok_or(
                    WorkerError::UnknownWorld {
                        universe_id: self.infra.default_universe_id,
                        world_id,
                    },
                )?;
                (
                    world.universe_id,
                    world.world_epoch,
                    world.projection_token.clone(),
                    world.metadata.workflow_modules.clone(),
                    Arc::clone(&world.store),
                )
            };
        let workflow_modules = workflow_modules.into_iter().collect::<BTreeSet<_>>();
        let active_baseline =
            self.select_source_snapshot(universe_id, world_id, &SnapshotSelector::ActiveBaseline)?;
        let world =
            self.state
                .active_worlds
                .get_mut(&world_id)
                .ok_or(WorkerError::UnknownWorld {
                    universe_id,
                    world_id,
                })?;
        let journal_head = world.host.heights().head;
        let manifest_hash = world.host.kernel().manifest_hash().to_hex();

        let mut records = Vec::new();
        records.push(ProjectionRecord {
            key: ProjectionKey::WorldMeta { world_id },
            value: Some(ProjectionValue::WorldMeta(WorldMetaProjection {
                universe_id,
                projection_token: projection_token.clone(),
                world_epoch,
                journal_head,
                manifest_hash,
                active_baseline: active_baseline.clone(),
                updated_at_ns: 0,
            })),
        });

        if !world.projection_bootstrapped {
            records.extend(
                materialize_workspaces(&world.host, journal_head)?
                    .into_iter()
                    .map(|workspace| ProjectionRecord {
                        key: ProjectionKey::Workspace {
                            world_id,
                            workspace: workspace.workspace.clone(),
                        },
                        value: Some(ProjectionValue::Workspace(WorkspaceProjectionUpsert {
                            projection_token: projection_token.clone(),
                            record: workspace,
                        })),
                    }),
            );

            for cell in materialize_cells(&world.host, &store, &workflow_modules)? {
                records.push(ProjectionRecord {
                    key: ProjectionKey::Cell {
                        world_id,
                        workflow: cell.record.workflow.clone(),
                        key_hash: cell.record.key_hash.clone(),
                    },
                    value: Some(ProjectionValue::Cell(CellProjectionUpsert {
                        projection_token: projection_token.clone(),
                        record: cell.record,
                        state_payload: cell.state_payload,
                    })),
                });
            }

            let _ = world.host.drain_workspace_projection_deltas()?;
            let _ = world.host.drain_cell_projection_deltas();
            return Ok(ProjectionEmissionPlan {
                records,
                published: ProjectionPublishedState {
                    world_id,
                    journal_head,
                    active_baseline,
                },
            });
        }

        records.extend(workspace_delta_projection_records(
            world_id,
            &projection_token,
            journal_head,
            world.host.drain_workspace_projection_deltas()?,
        ));
        records.extend(cell_delta_projection_records(
            world_id,
            &projection_token,
            &workflow_modules,
            &store,
            journal_head,
            world.host.drain_cell_projection_deltas(),
        )?);

        Ok(ProjectionEmissionPlan {
            records,
            published: ProjectionPublishedState {
                world_id,
                journal_head,
                active_baseline,
            },
        })
    }
}

struct ProjectionEmissionPlan {
    records: Vec<ProjectionRecord>,
    published: ProjectionPublishedState,
}

struct ProjectionPublishedState {
    world_id: WorldId,
    journal_head: u64,
    active_baseline: SnapshotRecord,
}

struct MaterializedCellProjection {
    record: CellStateProjectionRecord,
    state_payload: CborPayload,
}

fn materialize_cells<S: Store + 'static>(
    host: &WorldHost<S>,
    store: &Arc<S>,
    workflow_modules: &BTreeSet<String>,
) -> Result<Vec<MaterializedCellProjection>, WorkerError> {
    let mut rows = Vec::new();
    for workflow in workflow_modules {
        let listed = host.list_cells(workflow)?;
        if listed.is_empty() {
            if let Some(state_bytes) = host.state(workflow, Some(&[])) {
                let state_hash = Hash::of_bytes(&state_bytes);
                rows.push(MaterializedCellProjection {
                    record: CellStateProjectionRecord {
                        journal_head: host.heights().head,
                        workflow: workflow.clone(),
                        key_hash: Hash::of_bytes(&[]).as_bytes().to_vec(),
                        key_bytes: Vec::new(),
                        state_hash: state_hash.to_hex(),
                        size: state_bytes.len() as u64,
                        last_active_ns: 0,
                    },
                    state_payload: state_payload_for_bytes(
                        store.as_ref(),
                        state_hash,
                        state_bytes,
                    )?,
                });
            }
            continue;
        }

        for cell in listed {
            let Some(state_bytes) = host.state(workflow, Some(&cell.key_bytes)) else {
                continue;
            };
            let state_hash = Hash::from(cell.state_hash);
            rows.push(MaterializedCellProjection {
                record: CellStateProjectionRecord {
                    journal_head: host.heights().head,
                    workflow: workflow.clone(),
                    key_hash: cell.key_hash.to_vec(),
                    key_bytes: cell.key_bytes,
                    state_hash: state_hash.to_hex(),
                    size: cell.size,
                    last_active_ns: cell.last_active_ns,
                },
                state_payload: state_payload_for_bytes(store.as_ref(), state_hash, state_bytes)?,
            });
        }
    }
    rows.sort_by(|left, right| {
        left.record
            .workflow
            .cmp(&right.record.workflow)
            .then_with(|| left.record.key_bytes.cmp(&right.record.key_bytes))
            .then_with(|| left.record.key_hash.cmp(&right.record.key_hash))
    });
    Ok(rows)
}

fn state_payload_for_bytes(
    store: &impl Store,
    state_hash: Hash,
    state_bytes: Vec<u8>,
) -> Result<CborPayload, WorkerError> {
    if store.has_blob(state_hash)? {
        Ok(CborPayload::externalized(
            state_hash,
            state_bytes.len() as u64,
        ))
    } else {
        Ok(CborPayload::inline(state_bytes))
    }
}

fn materialize_workspaces<S: Store + 'static>(
    host: &WorldHost<S>,
    journal_head: u64,
) -> Result<Vec<WorkspaceRegistryProjectionRecord>, WorkerError> {
    let mut workspaces = host
        .list_cells("sys/Workspace@1")?
        .into_iter()
        .filter_map(|cell| {
            host.state("sys/Workspace@1", Some(&cell.key_bytes))
                .map(|bytes| (cell.key_bytes, bytes))
        })
        .map(|(key_bytes, bytes)| {
            let workspace = serde_cbor::from_slice::<String>(&key_bytes)?;
            let history: WorkspaceHistoryState = serde_cbor::from_slice(&bytes)?;
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
                .collect::<BTreeMap<_, _>>();
            Ok(WorkspaceRegistryProjectionRecord {
                journal_head,
                workspace,
                latest_version: history.latest,
                versions,
                updated_at_ns: 0,
            })
        })
        .collect::<Result<Vec<_>, WorkerError>>()?;
    workspaces.sort_by(|left, right| left.workspace.cmp(&right.workspace));
    Ok(workspaces)
}

fn workspace_delta_projection_records(
    world_id: WorldId,
    projection_token: &str,
    journal_head: u64,
    deltas: Vec<WorkspaceProjectionDelta>,
) -> Vec<ProjectionRecord> {
    deltas
        .into_iter()
        .map(|delta| ProjectionRecord {
            key: ProjectionKey::Workspace {
                world_id,
                workspace: delta.workspace.clone(),
            },
            value: delta.state.map(|state| {
                ProjectionValue::Workspace(WorkspaceProjectionUpsert {
                    projection_token: projection_token.to_owned(),
                    record: workspace_projection_record(delta.workspace, journal_head, state),
                })
            }),
        })
        .collect()
}

fn workspace_projection_record(
    workspace: String,
    journal_head: u64,
    state: WorkspaceProjectionDeltaState,
) -> WorkspaceRegistryProjectionRecord {
    WorkspaceRegistryProjectionRecord {
        journal_head,
        workspace,
        latest_version: state.latest_version,
        versions: state
            .versions
            .into_iter()
            .map(|(version, meta)| {
                (
                    version,
                    WorkspaceVersionProjectionRecord {
                        root_hash: meta.root_hash,
                        owner: meta.owner,
                        created_at_ns: meta.created_at_ns,
                    },
                )
            })
            .collect(),
        updated_at_ns: 0,
    }
}

fn cell_delta_projection_records<S: Store + 'static>(
    world_id: WorldId,
    projection_token: &str,
    workflow_modules: &BTreeSet<String>,
    store: &Arc<S>,
    journal_head: u64,
    deltas: Vec<CellProjectionDelta>,
) -> Result<Vec<ProjectionRecord>, WorkerError> {
    let mut records = Vec::new();
    for delta in deltas {
        if !workflow_modules.contains(&delta.workflow) {
            continue;
        }
        match delta.state {
            Some(state) => records.push(ProjectionRecord {
                key: ProjectionKey::Cell {
                    world_id,
                    workflow: delta.workflow.clone(),
                    key_hash: delta.key_hash.clone(),
                },
                value: Some(ProjectionValue::Cell(CellProjectionUpsert {
                    projection_token: projection_token.to_owned(),
                    record: CellStateProjectionRecord {
                        journal_head,
                        workflow: delta.workflow,
                        key_hash: delta.key_hash,
                        key_bytes: delta.key_bytes,
                        state_hash: state.state_hash.to_hex(),
                        size: state.size,
                        last_active_ns: state.last_active_ns,
                    },
                    state_payload: state_payload_for_bytes(
                        store.as_ref(),
                        state.state_hash,
                        state.state_bytes,
                    )?,
                })),
            }),
            None => records.push(ProjectionRecord {
                key: ProjectionKey::Cell {
                    world_id,
                    workflow: delta.workflow,
                    key_hash: delta.key_hash,
                },
                value: None,
            }),
        }
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_kernel::{MemStore, WorkspaceProjectionVersion};
    use uuid::Uuid;

    #[test]
    fn workspace_delta_projection_records_emit_upserts_and_tombstones() {
        let world_id = WorldId::from(Uuid::new_v4());
        let records = workspace_delta_projection_records(
            world_id,
            "tok-1",
            12,
            vec![
                WorkspaceProjectionDelta {
                    workspace: "alpha".into(),
                    state: Some(WorkspaceProjectionDeltaState {
                        latest_version: 2,
                        versions: BTreeMap::from([(
                            2,
                            WorkspaceProjectionVersion {
                                root_hash: "sha256:feed".into(),
                                owner: "lukas".into(),
                                created_at_ns: 22,
                            },
                        )]),
                    }),
                },
                WorkspaceProjectionDelta {
                    workspace: "beta".into(),
                    state: None,
                },
            ],
        );

        assert_eq!(records.len(), 2);
        assert!(matches!(
            &records[0].value,
            Some(ProjectionValue::Workspace(WorkspaceProjectionUpsert { record, .. }))
                if record.workspace == "alpha" && record.latest_version == 2
        ));
        assert!(records[1].value.is_none());
    }

    #[test]
    fn cell_delta_projection_records_emit_upserts_and_tombstones() {
        let world_id = WorldId::from(Uuid::new_v4());
        let workflow_modules = BTreeSet::from(["demo/Counter@1".to_owned()]);
        let store = Arc::new(MemStore::new());
        let state_bytes = vec![0xA1, 0x61, 0x6E, 0x01];
        let state_hash = Hash::of_bytes(&state_bytes);
        let records = cell_delta_projection_records(
            world_id,
            "tok-1",
            &workflow_modules,
            &store,
            7,
            vec![
                CellProjectionDelta {
                    workflow: "demo/Counter@1".into(),
                    key_hash: b"hash-a".to_vec(),
                    key_bytes: b"a".to_vec(),
                    state: Some(aos_kernel::CellProjectionDeltaState {
                        state_bytes: state_bytes.clone(),
                        state_hash,
                        size: state_bytes.len() as u64,
                        last_active_ns: 5,
                    }),
                },
                CellProjectionDelta {
                    workflow: "demo/Counter@1".into(),
                    key_hash: b"hash-b".to_vec(),
                    key_bytes: b"b".to_vec(),
                    state: None,
                },
                CellProjectionDelta {
                    workflow: "sys/Workspace@1".into(),
                    key_hash: b"ignored".to_vec(),
                    key_bytes: b"ignored".to_vec(),
                    state: None,
                },
            ],
        )
        .expect("cell delta records");

        assert_eq!(records.len(), 2);
        assert!(matches!(
            &records[0].value,
            Some(ProjectionValue::Cell(CellProjectionUpsert { record, state_payload, .. }))
                if record.workflow == "demo/Counter@1"
                    && record.journal_head == 7
                    && state_payload.inline_cbor.as_ref() == Some(&state_bytes)
        ));
        assert!(records[1].value.is_none());
    }
}
