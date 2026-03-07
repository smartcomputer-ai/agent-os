use std::fmt;

use aos_cbor::Hash;
use uuid::Uuid;

use crate::{InboxSeq, JournalHeight, ShardId, TimeBucket, UniverseId, WorldId};

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
}

#[derive(Debug, Clone, Copy)]
pub struct UniverseKeyspace {
    universe: UniverseId,
}

impl UniverseKeyspace {
    pub fn cas_meta(&self, hash: Hash) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("cas"),
            KeyPart::text("meta"),
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

    pub fn segment_index(&self, world: WorldId, end_height: JournalHeight) -> TupleKey {
        TupleKey::new(vec![
            KeyPart::text("u"),
            KeyPart::Uuid(self.universe.as_uuid()),
            KeyPart::text("segments"),
            KeyPart::Uuid(world.as_uuid()),
            KeyPart::U64(end_height),
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
}
