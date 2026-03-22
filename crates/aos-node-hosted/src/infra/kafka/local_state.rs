use aos_node::{PlaneError, WorldId, WorldLogAppendResult, WorldLogFrame, partition_for_world};
use std::collections::BTreeMap;

use super::types::{PartitionLogEntry, ProjectionTopicEntry};

pub(super) fn append_frame_locally(
    journal_topic: &str,
    world_frames: &mut BTreeMap<WorldId, Vec<WorldLogFrame>>,
    partition_logs: &mut BTreeMap<(String, u32), Vec<PartitionLogEntry>>,
    partition_count: u32,
    frame: WorldLogFrame,
    broker_offset: Option<u64>,
) -> Result<WorldLogAppendResult, PlaneError> {
    let partition = partition_for_world(frame.world_id, partition_count);
    let frames = world_frames.entry(frame.world_id).or_default();
    let expected = frames
        .last()
        .map(|last| last.world_seq_end.saturating_add(1))
        .unwrap_or(0);
    if frame.world_seq_start < expected {
        return Err(PlaneError::NonContiguousWorldSeq {
            universe_id: frame.universe_id,
            world_id: frame.world_id,
            expected,
            actual: frame.world_seq_start,
        });
    }

    frames.push(frame);
    let partition_entries = partition_logs
        .entry((journal_topic.to_owned(), partition))
        .or_default();
    let offset = broker_offset.unwrap_or(partition_entries.len() as u64);
    partition_entries.push(PartitionLogEntry {
        offset,
        frame: frames.last().expect("frame just pushed").clone(),
    });
    Ok(WorldLogAppendResult {
        journal_offset: offset,
    })
}

pub(super) fn world_key_bytes(world_id: WorldId) -> Vec<u8> {
    world_id.to_string().into_bytes()
}

pub(super) fn append_projection_locally(
    topic: &str,
    projection_logs: &mut BTreeMap<(String, u32), Vec<ProjectionTopicEntry>>,
    partition: u32,
    key: Vec<u8>,
    value: Option<Vec<u8>>,
    broker_offset: Option<u64>,
) -> u64 {
    let entries = projection_logs
        .entry((topic.to_owned(), partition))
        .or_default();
    let offset = broker_offset.unwrap_or(entries.len() as u64);
    entries.push(ProjectionTopicEntry { offset, key, value });
    offset
}
