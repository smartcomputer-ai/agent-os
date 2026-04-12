use aos_node::{CborPayload, SnapshotRecord, UniverseId, WorldId};
use serde::{Deserialize, Serialize};

use crate::materializer::{CellStateProjectionRecord, WorkspaceRegistryProjectionRecord};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectionKey {
    WorldMeta {
        world_id: WorldId,
    },
    Workspace {
        world_id: WorldId,
        workspace: String,
    },
    Cell {
        world_id: WorldId,
        workflow: String,
        #[serde(with = "serde_bytes")]
        key_hash: Vec<u8>,
    },
}

impl ProjectionKey {
    pub fn world_id(&self) -> WorldId {
        match self {
            Self::WorldMeta { world_id }
            | Self::Workspace { world_id, .. }
            | Self::Cell { world_id, .. } => *world_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldMetaProjection {
    pub universe_id: UniverseId,
    pub projection_token: String,
    pub world_epoch: u64,
    pub journal_head: u64,
    pub manifest_hash: String,
    pub active_baseline: SnapshotRecord,
    #[serde(default)]
    pub updated_at_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceProjectionUpsert {
    pub projection_token: String,
    pub record: WorkspaceRegistryProjectionRecord,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellProjectionUpsert {
    pub projection_token: String,
    pub record: CellStateProjectionRecord,
    pub state_payload: CborPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectionValue {
    WorldMeta(WorldMetaProjection),
    Workspace(WorkspaceProjectionUpsert),
    Cell(CellProjectionUpsert),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectionRecord {
    pub key: ProjectionKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<ProjectionValue>,
}
