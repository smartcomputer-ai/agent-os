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
    pub active_baseline_height: Option<JournalSeq>,
    pub active_baseline_receipt_horizon_height: Option<JournalSeq>,
}

/// Envelope for read responses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateRead<T> {
    pub meta: ReadMeta,
    pub value: T,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consistency_variants() {
        let c1 = Consistency::Head;
        let c2 = Consistency::Exact(5);
        let c3 = Consistency::AtLeast(10);
        assert!(matches!(c1, Consistency::Head));
        assert!(matches!(c2, Consistency::Exact(5)));
        assert!(matches!(c3, Consistency::AtLeast(10)));
    }

    #[test]
    fn read_meta_fields() {
        let h = Hash::of_bytes(b"abc");
        let meta = ReadMeta {
            journal_height: 7,
            snapshot_hash: Some(h),
            manifest_hash: h,
            active_baseline_height: Some(5),
            active_baseline_receipt_horizon_height: Some(5),
        };
        assert_eq!(meta.journal_height, 7);
        assert_eq!(meta.snapshot_hash, Some(h));
        assert_eq!(meta.manifest_hash, h);
        assert_eq!(meta.active_baseline_height, Some(5));
    }

    #[test]
    fn state_read_wraps_value() {
        let meta = ReadMeta {
            journal_height: 0,
            snapshot_hash: None,
            manifest_hash: Hash::of_bytes(b"m"),
            active_baseline_height: None,
            active_baseline_receipt_horizon_height: None,
        };
        let sr = StateRead {
            meta: meta.clone(),
            value: Some(vec![1, 2, 3]),
        };
        assert_eq!(sr.meta.journal_height, 0);
        assert_eq!(sr.value, Some(vec![1, 2, 3]));
    }
}

/// Read-only surface exposed by the kernel for observational queries.
pub trait StateReader {
    /// Fetch reducer state (non-keyed or keyed cell) according to consistency preference.
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
