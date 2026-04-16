use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::sync::Arc;

use aos_effect_adapters::config::EffectAdapterConfig;
use aos_kernel::journal::JournalRecord;
use aos_kernel::{KernelConfig, Store, StoreError};
use aos_node::{LocalStatePaths, WorldConfig, WorldId, WorldLogFrame};
use thiserror::Error;

use crate::kafka::PartitionLogEntry;
use crate::kafka::{CellProjectionUpsert, ProjectionKey, ProjectionRecord, ProjectionValue};

use super::sqlite::{
    MaterializedCellRow, MaterializedJournalEntryRow, MaterializerSourceOffsetRow,
    MaterializerSqliteConfig, MaterializerSqliteStore, MaterializerStoreError,
};

#[derive(Debug, Clone)]
pub struct MaterializerConfig {
    pub journal_topic: String,
    pub projection_topic: String,
    pub sqlite: MaterializerSqliteConfig,
    pub retained_journal_entries_per_world: Option<u64>,
    pub world_config: WorldConfig,
    pub adapter_config: EffectAdapterConfig,
    pub kernel_config: KernelConfig,
}

impl MaterializerConfig {
    pub fn from_paths(paths: &LocalStatePaths, journal_topic: impl Into<String>) -> Self {
        let mut world_config =
            WorldConfig::from_env_with_fallback_module_cache_dir(Some(paths.wasmtime_cache_dir()));
        world_config.eager_module_load = true;
        let kernel_config = world_config.apply_kernel_defaults(KernelConfig::default());
        Self {
            journal_topic: journal_topic.into(),
            projection_topic: crate::kafka::KafkaConfig::default().projection_topic,
            sqlite: MaterializerSqliteConfig::from_paths(paths),
            retained_journal_entries_per_world: None,
            world_config,
            adapter_config: EffectAdapterConfig::default(),
            kernel_config,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MaterializePartitionOutcome {
    pub partition: u32,
    pub processed_entries: usize,
    pub touched_worlds: usize,
    pub journal_entries_indexed: usize,
    pub cells_materialized: usize,
    pub workspaces_materialized: usize,
    pub last_offset: Option<u64>,
}

#[derive(Debug, Error)]
pub enum MaterializerError {
    #[error(transparent)]
    LogFirst(#[from] aos_node::BackendError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Sqlite(#[from] MaterializerStoreError),
    #[error(transparent)]
    Cbor(#[from] serde_cbor::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("projection record/value mismatch for world {1}: {0}")]
    ProjectionMismatch(String, WorldId),
}

pub struct Materializer<S: Store + 'static> {
    config: MaterializerConfig,
    sqlite: MaterializerSqliteStore,
    _store: PhantomData<S>,
}

impl<S: Store + 'static> Materializer<S> {
    pub fn from_config(config: MaterializerConfig) -> Result<Self, MaterializerError> {
        Ok(Self {
            sqlite: MaterializerSqliteStore::new(config.sqlite.clone())?,
            config,
            _store: PhantomData,
        })
    }

    pub fn from_paths(
        paths: &LocalStatePaths,
        journal_topic: impl Into<String>,
        _store: Arc<S>,
    ) -> Result<Self, MaterializerError> {
        Self::new(MaterializerConfig::from_paths(paths, journal_topic), _store)
    }

    pub fn new(config: MaterializerConfig, _store: Arc<S>) -> Result<Self, MaterializerError> {
        Self::from_config(config)
    }

    pub fn new_with_store_provider(
        config: MaterializerConfig,
        _provider: Arc<
            dyn Fn(aos_node::UniverseId) -> Result<Arc<S>, MaterializerError> + Send + Sync,
        >,
    ) -> Result<Self, MaterializerError> {
        Self::from_config(config)
    }

    pub fn config(&self) -> &MaterializerConfig {
        &self.config
    }

    pub fn sqlite(&self) -> &MaterializerSqliteStore {
        &self.sqlite
    }

    pub fn load_source_offset(
        &self,
        topic: &str,
        partition: u32,
    ) -> Result<Option<i64>, MaterializerError> {
        Ok(self
            .sqlite
            .load_source_offset(topic, partition)?
            .map(|row| row.last_offset))
    }

    pub fn materialize_partition(
        &mut self,
        partition: u32,
        entries: &[PartitionLogEntry],
    ) -> Result<MaterializePartitionOutcome, MaterializerError> {
        let last_offset = self.load_source_offset(&self.config.journal_topic, partition)?;
        let mut outcome = MaterializePartitionOutcome {
            partition,
            last_offset: last_offset.map(|offset| offset as u64),
            ..MaterializePartitionOutcome::default()
        };
        for entry in entries
            .iter()
            .filter(|entry| last_offset.is_none_or(|offset| entry.offset as i64 > offset))
        {
            outcome = accumulate_outcome(outcome, self.index_journal_entry(partition, entry)?);
        }
        Ok(outcome)
    }

    pub fn apply_projection_record(
        &mut self,
        partition: u32,
        offset: u64,
        record: &ProjectionRecord,
    ) -> Result<MaterializePartitionOutcome, MaterializerError> {
        let last_offset = self.load_source_offset(&self.config.projection_topic, partition)?;
        if last_offset.is_some_and(|existing| offset as i64 <= existing) {
            return Ok(MaterializePartitionOutcome {
                partition,
                last_offset: last_offset.map(|value| value as u64),
                ..MaterializePartitionOutcome::default()
            });
        }

        let outcome = self.apply_projection_record_inner(partition, Some(offset), record)?;

        self.sqlite
            .persist_source_offset(&MaterializerSourceOffsetRow {
                journal_topic: self.config.projection_topic.clone(),
                partition,
                last_offset: offset as i64,
                updated_at_ns: 0,
            })?;
        Ok(outcome)
    }

    pub fn bootstrap_projection_partition(
        &mut self,
        partition: u32,
        entries: &[(u64, ProjectionRecord)],
    ) -> Result<MaterializePartitionOutcome, MaterializerError> {
        let last_offset = self.load_source_offset(&self.config.projection_topic, partition)?;
        if last_offset.is_some() {
            return Ok(MaterializePartitionOutcome {
                partition,
                last_offset: last_offset.map(|value| value as u64),
                ..MaterializePartitionOutcome::default()
            });
        }

        let mut latest_entries = BTreeMap::<Vec<u8>, (u64, ProjectionRecord)>::new();
        let mut latest_world_meta = BTreeMap::<WorldId, (u64, ProjectionRecord)>::new();
        let mut latest_offset: Option<u64> = None;
        for (offset, record) in entries {
            latest_offset = Some(latest_offset.map_or(*offset, |existing| existing.max(*offset)));
            latest_entries.insert(serde_cbor::to_vec(&record.key)?, (*offset, record.clone()));
            if matches!(
                (&record.key, &record.value),
                (
                    ProjectionKey::WorldMeta { .. },
                    Some(ProjectionValue::WorldMeta(_))
                )
            ) {
                latest_world_meta.insert(record.key.world_id(), (*offset, record.clone()));
            }
        }

        let mut outcome = MaterializePartitionOutcome {
            partition,
            last_offset: latest_offset,
            ..MaterializePartitionOutcome::default()
        };
        let mut latest_world_meta = latest_world_meta.into_iter().collect::<Vec<_>>();
        latest_world_meta.sort_by_key(|(_world_id, (offset, _record))| *offset);
        for (_world_id, (_offset, record)) in latest_world_meta {
            outcome = accumulate_outcome(
                outcome,
                self.apply_projection_record_inner(partition, None, &record)?,
            );
        }
        let mut latest_entries = latest_entries.into_iter().collect::<Vec<_>>();
        latest_entries.sort_by_key(|(_key, (offset, _record))| *offset);
        for (_key_bytes, (_offset, record)) in latest_entries {
            if matches!(record.key, ProjectionKey::WorldMeta { .. }) {
                continue;
            }
            outcome = accumulate_outcome(
                outcome,
                self.apply_projection_record_inner(partition, None, &record)?,
            );
        }

        if let Some(offset) = latest_offset {
            self.sqlite
                .persist_source_offset(&MaterializerSourceOffsetRow {
                    journal_topic: self.config.projection_topic.clone(),
                    partition,
                    last_offset: offset as i64,
                    updated_at_ns: 0,
                })?;
        }
        Ok(outcome)
    }

    fn apply_projection_record_inner(
        &mut self,
        partition: u32,
        last_offset: Option<u64>,
        record: &ProjectionRecord,
    ) -> Result<MaterializePartitionOutcome, MaterializerError> {
        Ok(match (&record.key, &record.value) {
            (ProjectionKey::WorldMeta { world_id }, Some(ProjectionValue::WorldMeta(meta))) => {
                let _token_changed = self.sqlite.apply_world_meta_projection(*world_id, meta)?;
                MaterializePartitionOutcome {
                    partition,
                    processed_entries: 1,
                    touched_worlds: 1,
                    last_offset,
                    ..MaterializePartitionOutcome::default()
                }
            }
            (
                ProjectionKey::Workspace { world_id, .. },
                Some(ProjectionValue::Workspace(value)),
            ) => {
                let applied = self.sqlite.apply_workspace_projection(
                    *world_id,
                    &value.projection_token,
                    &value.record,
                )?;
                MaterializePartitionOutcome {
                    partition,
                    processed_entries: 1,
                    touched_worlds: 1,
                    workspaces_materialized: usize::from(applied),
                    last_offset,
                    ..MaterializePartitionOutcome::default()
                }
            }
            (
                ProjectionKey::Workspace {
                    world_id,
                    workspace,
                },
                None,
            ) => {
                let applied = self
                    .sqlite
                    .apply_workspace_tombstone(*world_id, workspace)?;
                MaterializePartitionOutcome {
                    partition,
                    processed_entries: 1,
                    touched_worlds: 1,
                    workspaces_materialized: usize::from(applied),
                    last_offset,
                    ..MaterializePartitionOutcome::default()
                }
            }
            (
                ProjectionKey::Cell { world_id, .. },
                Some(ProjectionValue::Cell(CellProjectionUpsert {
                    projection_token,
                    record,
                    state_payload,
                })),
            ) => {
                let applied = self.sqlite.apply_cell_projection(
                    *world_id,
                    projection_token,
                    &MaterializedCellRow {
                        cell: record.clone(),
                        state_payload: state_payload.clone(),
                    },
                )?;
                MaterializePartitionOutcome {
                    partition,
                    processed_entries: 1,
                    touched_worlds: 1,
                    cells_materialized: usize::from(applied),
                    last_offset,
                    ..MaterializePartitionOutcome::default()
                }
            }
            (
                ProjectionKey::Cell {
                    world_id,
                    workflow,
                    key_hash,
                },
                None,
            ) => {
                let applied = self
                    .sqlite
                    .apply_cell_tombstone(*world_id, workflow, key_hash)?;
                MaterializePartitionOutcome {
                    partition,
                    processed_entries: 1,
                    touched_worlds: 1,
                    cells_materialized: usize::from(applied),
                    last_offset,
                    ..MaterializePartitionOutcome::default()
                }
            }
            (ProjectionKey::WorldMeta { world_id }, value) => {
                return Err(MaterializerError::ProjectionMismatch(
                    format!("expected world/meta value, got {value:?}"),
                    *world_id,
                ));
            }
            (ProjectionKey::Workspace { world_id, .. }, value) => {
                return Err(MaterializerError::ProjectionMismatch(
                    format!("expected workspace value or tombstone, got {value:?}"),
                    *world_id,
                ));
            }
            (ProjectionKey::Cell { world_id, .. }, value) => {
                return Err(MaterializerError::ProjectionMismatch(
                    format!("expected cell value or tombstone, got {value:?}"),
                    *world_id,
                ));
            }
        })
    }

    pub fn index_journal_entry(
        &mut self,
        partition: u32,
        entry: &PartitionLogEntry,
    ) -> Result<MaterializePartitionOutcome, MaterializerError> {
        let last_offset = self.load_source_offset(&self.config.journal_topic, partition)?;
        if last_offset.is_some_and(|existing| entry.offset as i64 <= existing) {
            return Ok(MaterializePartitionOutcome {
                partition,
                last_offset: last_offset.map(|value| value as u64),
                ..MaterializePartitionOutcome::default()
            });
        }

        let rows = journal_rows_from_frame(&entry.frame)?;
        let manifest_hash = manifest_hash_hint_from_frame(&entry.frame);
        self.sqlite.append_journal_entries(
            entry.frame.universe_id,
            entry.frame.world_id,
            entry.frame.world_seq_end,
            manifest_hash,
            &rows,
            self.config.retained_journal_entries_per_world,
        )?;
        self.sqlite
            .persist_source_offset(&MaterializerSourceOffsetRow {
                journal_topic: self.config.journal_topic.clone(),
                partition,
                last_offset: entry.offset as i64,
                updated_at_ns: 0,
            })?;

        Ok(MaterializePartitionOutcome {
            partition,
            processed_entries: 1,
            touched_worlds: 1,
            journal_entries_indexed: rows.len(),
            last_offset: Some(entry.offset),
            ..MaterializePartitionOutcome::default()
        })
    }
}

fn accumulate_outcome(
    mut left: MaterializePartitionOutcome,
    right: MaterializePartitionOutcome,
) -> MaterializePartitionOutcome {
    left.processed_entries += right.processed_entries;
    left.touched_worlds += right.touched_worlds;
    left.journal_entries_indexed += right.journal_entries_indexed;
    left.cells_materialized += right.cells_materialized;
    left.workspaces_materialized += right.workspaces_materialized;
    left.last_offset = right.last_offset.or(left.last_offset);
    left
}

fn journal_rows_from_frame(
    frame: &WorldLogFrame,
) -> Result<Vec<MaterializedJournalEntryRow>, MaterializerError> {
    frame
        .records
        .iter()
        .enumerate()
        .map(|(index, record)| {
            Ok(MaterializedJournalEntryRow {
                seq: frame.world_seq_start + index as u64,
                kind: serde_json::to_value(record.kind())?
                    .as_str()
                    .unwrap_or("unknown")
                    .to_owned(),
                record: serde_json::to_value(record)?,
                raw_cbor: serde_cbor::to_vec(record)?,
            })
        })
        .collect()
}

fn manifest_hash_hint_from_frame(frame: &WorldLogFrame) -> Option<String> {
    frame.records.iter().rev().find_map(|record| match record {
        JournalRecord::Snapshot(snapshot) => snapshot.manifest_hash.clone(),
        _ => None,
    })
}
