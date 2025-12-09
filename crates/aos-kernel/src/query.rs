use aos_air_types::Manifest;
use aos_cbor::Hash;

use crate::error::KernelError;
use crate::journal::JournalSeq;

/// Caller’s freshness preference for read-only queries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Consistency {
    /// Return the latest available state (may be slightly stale if replay is running).
    Head,
    /// Require the snapshot/journal to be exactly this height.
    Exact(JournalSeq),
    /// Serve the newest available state at or above this height.
    AtLeast(JournalSeq),
}

/// Metadata attached to every read response so callers can reason about what they saw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadMeta {
    pub journal_height: JournalSeq,
    pub snapshot_hash: Option<Hash>,
    pub manifest_hash: Hash,
}

/// Envelope for read responses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateRead<T> {
    pub meta: ReadMeta,
    pub value: T,
}

/// Read-only surface exposed by the kernel for observational queries.
pub trait StateReader {
    /// Fetch reducer state (monolithic or keyed cell) according to consistency preference.
    fn get_reducer_state(
        &self,
        module: &str,
        key: Option<&[u8]>,
        consistency: Consistency,
    ) -> Result<StateRead<Option<Vec<u8>>>, KernelError>;

    /// Fetch the manifest for inspection.
    fn get_manifest(&self, consistency: Consistency) -> Result<StateRead<Manifest>, KernelError>;

    /// Return only consistency metadata (height + hashes).
    fn get_journal_head(&self) -> ReadMeta;
}
