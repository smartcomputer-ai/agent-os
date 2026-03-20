use std::fmt;

use aos_cbor::Hash;
use uuid::Uuid;

use crate::{InboxSeq, JournalHeight, ShardId, TimeBucket, UniverseId, WorldId};

#[cfg(feature = "foundationdb-backend")]
use crate::PersistError;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KeyPart {
    Text(String),
    Uuid(Uuid),
    U64(u64),
    Bytes(Vec<u8>),
    Hash(Hash),
}

impl KeyPart {
    fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TupleKey {
    parts: Vec<KeyPart>,
}

impl TupleKey {
    pub fn new(parts: impl Into<Vec<KeyPart>>) -> Self {
        Self {
            parts: parts.into(),
        }
    }

    pub fn parts(&self) -> &[KeyPart] {
        &self.parts
    }

    #[cfg(feature = "foundationdb-backend")]
    pub(crate) fn pack_for_fdb(&self) -> Result<Vec<u8>, PersistError> {
        use std::borrow::Cow;

        use foundationdb::tuple::{Element, pack};

        let elements: Vec<Element<'static>> = self
            .parts
            .iter()
            .map(|part| -> Result<Element<'static>, PersistError> {
                match part {
                    KeyPart::Text(value) => Ok(Element::String(Cow::Owned(value.clone()))),
                    // Keep current tuple encoding stable: UUIDs are stored as text segments today.
                    KeyPart::Uuid(value) => Ok(Element::String(Cow::Owned(value.to_string()))),
                    KeyPart::U64(value) => {
                        let value = i64::try_from(*value).map_err(|_| {
                            PersistError::validation(format!(
                                "key part integer {value} exceeds supported FDB tuple i64 range"
                            ))
                        })?;
                        Ok(Element::Int(value))
                    }
                    KeyPart::Bytes(value) => Ok(Element::Bytes(value.clone().into())),
                    // Keep current tuple encoding stable: hashes are stored as hex text today.
                    KeyPart::Hash(value) => Ok(Element::String(Cow::Owned(value.to_hex()))),
                }
            })
            .collect::<Result<_, _>>()?;
        Ok(pack(&elements))
    }
}

impl fmt::Display for TupleKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (idx, part) in self.parts.iter().enumerate() {
            if idx > 0 {
                f.write_str("/")?;
            }
            match part {
                KeyPart::Text(value) => f.write_str(value)?,
                KeyPart::Uuid(value) => write!(f, "{value}")?,
                KeyPart::U64(value) => write!(f, "{value}")?,
                KeyPart::Bytes(value) => write!(f, "0x{}", hex::encode(value))?,
                KeyPart::Hash(value) => write!(f, "{value}")?,
            }
        }
        Ok(())
    }
}

pub struct FdbKeyspace;

impl FdbKeyspace {
    pub fn universe(universe: UniverseId) -> UniverseKeyspace {
        UniverseKeyspace { universe }
    }

    pub fn ready(priority: u16, shard: ShardId, universe: UniverseId, world: WorldId) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("ready"),
            KeyPart::U64(priority as u64),
            KeyPart::U64(shard as u64),
            KeyPart::Uuid(universe.as_uuid()),
            KeyPart::Uuid(world.as_uuid()),
        ])
    }

    pub fn lease_by_worker(worker_id: &str, universe: UniverseId, world: WorldId) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("lease"),
            KeyPart::text("by_worker"),
            KeyPart::text(worker_id),
            KeyPart::Uuid(universe.as_uuid()),
            KeyPart::Uuid(world.as_uuid()),
        ])
    }

    pub fn worker_heartbeat(worker_id: &str) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("workers"),
            KeyPart::text("heartbeat"),
            KeyPart::text(worker_id),
        ])
    }
}

#[derive(Debug, Clone, Copy)]
pub struct UniverseKeyspace {
    universe: UniverseId,
}

impl UniverseKeyspace {
    pub fn ready(&self, priority: u16, shard: ShardId, world: WorldId) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("ready"),
            KeyPart::U64(priority as u64),
            KeyPart::U64(shard as u64),
            KeyPart::Uuid(world.as_uuid()),
        ])
    }

    pub fn lease_by_worker(&self, worker_id: &str, world: WorldId) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("lease"),
            KeyPart::text("by_worker"),
            KeyPart::text(worker_id),
            KeyPart::Uuid(world.as_uuid()),
        ])
    }

    pub fn cas_meta(&self, hash: Hash) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("cas"),
            KeyPart::text("meta"),
            KeyPart::Hash(hash),
        ])
    }

    pub fn cas_chunk(&self, hash: Hash, index: u64) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("cas"),
            KeyPart::text("chunk"),
            KeyPart::Hash(hash),
            KeyPart::U64(index),
        ])
    }

    pub fn cas_present(&self, hash: Hash, page: u64) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("cas"),
            KeyPart::text("present"),
            KeyPart::Hash(hash),
            KeyPart::U64(page),
        ])
    }

    pub fn cas_upload(&self, hash: Hash) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("cas"),
            KeyPart::text("upload"),
            KeyPart::Hash(hash),
        ])
    }

    pub fn effects_pending(&self, shard: ShardId, seq: &InboxSeq) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("effects"),
            KeyPart::text("pending"),
            KeyPart::U64(shard as u64),
            KeyPart::Bytes(seq.as_bytes().to_vec()),
        ])
    }

    pub fn effects_inflight(&self, shard: ShardId, seq: &InboxSeq) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("effects"),
            KeyPart::text("inflight"),
            KeyPart::U64(shard as u64),
            KeyPart::Bytes(seq.as_bytes().to_vec()),
        ])
    }

    pub fn effects_dedupe(&self, intent_hash: &[u8]) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("effects"),
            KeyPart::text("dedupe"),
            KeyPart::Bytes(intent_hash.to_vec()),
        ])
    }

    pub fn effects_dedupe_gc(&self, gc_bucket: u64, intent_hash: &[u8]) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("effects"),
            KeyPart::text("dedupe_gc"),
            KeyPart::U64(gc_bucket),
            KeyPart::Bytes(intent_hash.to_vec()),
        ])
    }

    pub fn timers_due(
        &self,
        shard: ShardId,
        time_bucket: TimeBucket,
        deliver_at_ns: u64,
        intent_hash: &[u8],
    ) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("timers"),
            KeyPart::text("due"),
            KeyPart::U64(shard as u64),
            KeyPart::U64(time_bucket),
            KeyPart::U64(deliver_at_ns),
            KeyPart::Bytes(intent_hash.to_vec()),
        ])
    }

    pub fn timers_inflight(&self, shard: ShardId, intent_hash: &[u8]) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("timers"),
            KeyPart::text("inflight"),
            KeyPart::U64(shard as u64),
            KeyPart::Bytes(intent_hash.to_vec()),
        ])
    }

    pub fn timers_dedupe(&self, intent_hash: &[u8]) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("timers"),
            KeyPart::text("dedupe"),
            KeyPart::Bytes(intent_hash.to_vec()),
        ])
    }

    pub fn timers_dedupe_gc(&self, gc_bucket: u64, intent_hash: &[u8]) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("timers"),
            KeyPart::text("dedupe_gc"),
            KeyPart::U64(gc_bucket),
            KeyPart::Bytes(intent_hash.to_vec()),
        ])
    }

    pub fn portal_dedupe_gc(&self, gc_bucket: u64, world: WorldId, message_id: &[u8]) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("portal"),
            KeyPart::text("dedupe_gc"),
            KeyPart::U64(gc_bucket),
            KeyPart::Uuid(world.as_uuid()),
            KeyPart::Bytes(message_id.to_vec()),
        ])
    }

    pub fn segment_index(&self, world: WorldId, end_height: JournalHeight) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("segments"),
            KeyPart::Uuid(world.as_uuid()),
            KeyPart::U64(end_height),
        ])
    }

    pub fn worker_heartbeat(&self, worker_id: &str) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("workers"),
            KeyPart::text("heartbeat"),
            KeyPart::text(worker_id),
        ])
    }

    pub fn world(&self, world: WorldId) -> WorldKeyspace {
        WorldKeyspace {
            universe: self.universe,
            world,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct WorldKeyspace {
    universe: UniverseId,
    world: WorldId,
}

impl WorldKeyspace {
    pub fn meta(&self) -> TupleKey {
        self.base_with("meta")
    }

    pub fn ready_state(&self) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("w"),
            KeyPart::Uuid(self.world.as_uuid()),
            KeyPart::text("runtime"),
            KeyPart::text("ready_state"),
        ])
    }

    pub fn journal_head(&self) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("w"),
            KeyPart::Uuid(self.world.as_uuid()),
            KeyPart::text("journal"),
            KeyPart::text("head"),
        ])
    }

    pub fn journal_entry(&self, height: JournalHeight) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("w"),
            KeyPart::Uuid(self.world.as_uuid()),
            KeyPart::text("journal"),
            KeyPart::text("e"),
            KeyPart::U64(height),
        ])
    }

    pub fn snapshot_by_height(&self, height: JournalHeight) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("w"),
            KeyPart::Uuid(self.world.as_uuid()),
            KeyPart::text("snapshot"),
            KeyPart::text("by_height"),
            KeyPart::U64(height),
        ])
    }

    pub fn baseline_active(&self) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("w"),
            KeyPart::Uuid(self.world.as_uuid()),
            KeyPart::text("baseline"),
            KeyPart::text("active"),
        ])
    }

    pub fn projection_head(&self) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("w"),
            KeyPart::Uuid(self.world.as_uuid()),
            KeyPart::text("projection"),
            KeyPart::text("head"),
        ])
    }

    pub fn projection_cell(&self, workflow: &str, key_hash: &[u8]) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("w"),
            KeyPart::Uuid(self.world.as_uuid()),
            KeyPart::text("projection"),
            KeyPart::text("cell"),
            KeyPart::text(workflow),
            KeyPart::Bytes(key_hash.to_vec()),
        ])
    }

    pub fn projection_workspace(&self, workspace: &str) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("w"),
            KeyPart::Uuid(self.world.as_uuid()),
            KeyPart::text("projection"),
            KeyPart::text("workspace"),
            KeyPart::text(workspace),
        ])
    }

    pub fn inbox_entry(&self, seq: &InboxSeq) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("w"),
            KeyPart::Uuid(self.world.as_uuid()),
            KeyPart::text("inbox"),
            KeyPart::text("e"),
            KeyPart::Bytes(seq.as_bytes().to_vec()),
        ])
    }

    pub fn inbox_cursor(&self) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("w"),
            KeyPart::Uuid(self.world.as_uuid()),
            KeyPart::text("inbox"),
            KeyPart::text("cursor"),
        ])
    }

    pub fn notify_counter(&self) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("w"),
            KeyPart::Uuid(self.world.as_uuid()),
            KeyPart::text("notify"),
            KeyPart::text("counter"),
        ])
    }

    pub fn lease_current(&self) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("w"),
            KeyPart::Uuid(self.world.as_uuid()),
            KeyPart::text("lease"),
            KeyPart::text("current"),
        ])
    }

    pub fn gc_pin(&self, hash: Hash) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("w"),
            KeyPart::Uuid(self.world.as_uuid()),
            KeyPart::text("gc"),
            KeyPart::text("pin"),
            KeyPart::Hash(hash),
        ])
    }

    pub fn portal_dedupe(&self, message_id: &[u8]) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("w"),
            KeyPart::Uuid(self.world.as_uuid()),
            KeyPart::text("portal"),
            KeyPart::text("dedupe"),
            KeyPart::Bytes(message_id.to_vec()),
        ])
    }

    fn base_with(&self, terminal: &'static str) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("w"),
            KeyPart::Uuid(self.world.as_uuid()),
            KeyPart::text(terminal),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn universe() -> UniverseId {
        UniverseId::from(Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap())
    }

    fn world() -> WorldId {
        WorldId::from(Uuid::parse_str("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb").unwrap())
    }

    #[test]
    fn journal_entry_key_matches_authoritative_layout() {
        let key = FdbKeyspace::universe(universe())
            .world(world())
            .journal_entry(42);
        assert_eq!(
            key.parts(),
            &[
                KeyPart::Text("u".into()),
                KeyPart::Uuid(universe().as_uuid()),
                KeyPart::Text("w".into()),
                KeyPart::Uuid(world().as_uuid()),
                KeyPart::Text("journal".into()),
                KeyPart::Text("e".into()),
                KeyPart::U64(42),
            ]
        );
    }

    #[test]
    fn queue_keys_include_shard_dimension_from_start() {
        let seq = InboxSeq::from_u64(9);
        let key = FdbKeyspace::universe(universe()).effects_pending(3, &seq);
        assert_eq!(
            key.parts(),
            &[
                KeyPart::Text("u".into()),
                KeyPart::Uuid(universe().as_uuid()),
                KeyPart::Text("effects".into()),
                KeyPart::Text("pending".into()),
                KeyPart::U64(3),
                KeyPart::Bytes(seq.as_bytes().to_vec()),
            ]
        );
    }

    #[test]
    fn inbox_cursor_and_notify_keys_remain_distinct() {
        let world_keys = FdbKeyspace::universe(universe()).world(world());
        assert_ne!(world_keys.inbox_cursor(), world_keys.notify_counter());
        assert_eq!(
            world_keys.inbox_cursor().to_string(),
            "u/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa/w/bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb/inbox/cursor"
        );
    }

    #[test]
    fn worker_and_lease_keys_use_runtime_prefixes() {
        let universe_keys = FdbKeyspace::universe(universe());
        let world_keys = universe_keys.world(world());
        assert_eq!(
            universe_keys.worker_heartbeat("worker-a").to_string(),
            "u/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa/workers/heartbeat/worker-a"
        );
        assert_eq!(
            world_keys.lease_current().to_string(),
            "u/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa/w/bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb/lease/current"
        );
    }

    #[test]
    fn projection_keys_live_under_world_projection_prefix() {
        let world_keys = FdbKeyspace::universe(universe()).world(world());
        assert_eq!(
            world_keys.projection_head().to_string(),
            "u/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa/w/bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb/projection/head"
        );
        assert_eq!(
            world_keys
                .projection_cell("com.acme/Simple@1", &[0xAA, 0xBB])
                .to_string(),
            "u/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa/w/bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb/projection/cell/com.acme/Simple@1/0xaabb"
        );
        assert_eq!(
            world_keys.projection_workspace("shell").to_string(),
            "u/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa/w/bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb/projection/workspace/shell"
        );
    }

    #[test]
    fn cas_keys_use_canonical_prefixes() {
        let hash = Hash::of_bytes(b"cas-key");
        let universe_keys = FdbKeyspace::universe(universe());
        assert_eq!(
            universe_keys.cas_meta(hash).to_string(),
            format!("u/{}/cas/meta/{hash}", universe())
        );
        assert_eq!(
            universe_keys.cas_chunk(hash, 7).to_string(),
            format!("u/{}/cas/chunk/{hash}/7", universe())
        );
        assert_eq!(
            universe_keys.cas_present(hash, 1).to_string(),
            format!("u/{}/cas/present/{hash}/1", universe())
        );
        assert_eq!(
            universe_keys.cas_upload(hash).to_string(),
            format!("u/{}/cas/upload/{hash}", universe())
        );
    }

    #[test]
    fn portal_dedupe_key_uses_world_scoped_portal_prefix() {
        let key = FdbKeyspace::universe(universe())
            .world(world())
            .portal_dedupe(&[1, 2, 3]);
        assert_eq!(
            key.to_string(),
            "u/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa/w/bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb/portal/dedupe/0x010203"
        );
    }

    #[cfg(feature = "foundationdb-backend")]
    #[test]
    fn cas_keys_pack_using_current_fdb_tuple_encoding() {
        let hash = Hash::of_bytes(b"cas-key");
        let universe_keys = FdbKeyspace::universe(universe());

        assert_eq!(
            universe_keys.cas_meta(hash).pack_for_fdb().unwrap(),
            foundationdb::tuple::pack(&("u", universe().to_string(), "cas", "meta", hash.to_hex()))
        );
        assert_eq!(
            universe_keys.cas_chunk(hash, 7).pack_for_fdb().unwrap(),
            foundationdb::tuple::pack(&(
                "u",
                universe().to_string(),
                "cas",
                "chunk",
                hash.to_hex(),
                7i64
            ))
        );
        assert_eq!(
            universe_keys.cas_present(hash, 1).pack_for_fdb().unwrap(),
            foundationdb::tuple::pack(&(
                "u",
                universe().to_string(),
                "cas",
                "present",
                hash.to_hex(),
                1i64
            ))
        );
        assert_eq!(
            universe_keys.cas_upload(hash).pack_for_fdb().unwrap(),
            foundationdb::tuple::pack(&(
                "u",
                universe().to_string(),
                "cas",
                "upload",
                hash.to_hex()
            ))
        );
    }
}
