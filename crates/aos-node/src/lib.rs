#![forbid(unsafe_code)]

//! Shared node protocol and contracts for daemonized runtime backends.

pub mod control;
mod control_openapi;

mod fork_snapshot;
mod hosted;
mod hot_world;
mod memory;
mod memory_cas;
mod projection;
mod protocol;
mod segment;

pub use fork_snapshot::rewrite_snapshot_for_fork_policy;
pub use hosted::{
    HostedJournal, HostedStore, HostedStoreStatsSnapshot, SharedBlobCache,
    open_hosted_from_manifest_hash, open_hosted_world, snapshot_hosted_world,
    sync_hosted_snapshot_state,
};
pub use hot_world::{
    HotWorld, HotWorldDrainOutcome, HotWorldError, HotWorldReplayOpen,
    apply_ingress_item_to_hot_world, encode_ingress_as_journal_entry, open_hot_world,
    parse_hash_ref, parse_intent_hash, resolve_cbor_payload,
};
pub use memory::MemoryWorldPersistence;
pub use memory_cas::MemoryCasStore;
pub use projection::{materialization_from_snapshot, state_blobs_from_snapshot};
pub use protocol::*;
pub use segment::{
    decode_segment_entries, encode_segment_entries, segment_checksum,
    validate_segment_export_request,
};
