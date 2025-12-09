use std::collections::BTreeMap;

use aos_cbor::Hash;
use aos_store::{Store, StoreResult};
use serde::{Deserialize, Serialize};

/// Metadata tracked for a single cell.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CellMeta {
    pub key_hash: [u8; 32],
    #[serde(with = "serde_bytes")]
    pub key_bytes: Vec<u8>,
    pub state_hash: [u8; 32],
    pub size: u64,
    pub last_active_ns: u64,
}

/// Internal node representation for the persistent index.
#[derive(Debug, Clone, Serialize, Deserialize)]
enum Node {
    Leaf(Vec<CellMeta>),
    /// fan-out on a single byte of the key_hash
    Branch(Vec<(u8, [u8; 32])>),
}

const LEAF_MAX: usize = 64;

/// CAS-backed persistent index mapping key_hash -> CellMeta.
pub struct CellIndex<'a, S: Store> {
    store: &'a S,
}

impl<'a, S: Store> CellIndex<'a, S> {
    pub fn new(store: &'a S) -> Self {
        Self { store }
    }

    /// Returns the hash of an empty index.
    pub fn empty(&self) -> StoreResult<Hash> {
        self.store.put_node(&Node::Leaf(Vec::new()))
    }

    /// Fetch metadata for the given key hash.
    pub fn get(&self, root: Hash, key_hash: &[u8; 32]) -> StoreResult<Option<CellMeta>> {
        self.get_at(root, key_hash, 0)
    }

    /// Insert or replace metadata for a key, returning the new root hash.
    pub fn upsert(&self, root: Hash, meta: CellMeta) -> StoreResult<Hash> {
        self.insert_at(root, meta, 0)
    }

    /// Delete a key; returns (new_root, removed)
    pub fn delete(&self, root: Hash, key_hash: &[u8; 32]) -> StoreResult<(Hash, bool)> {
        let (maybe_hash, removed) = self.delete_at(root, key_hash, 0)?;
        if let Some(hash) = maybe_hash {
            Ok((hash, removed))
        } else {
            // collapse to empty leaf
            Ok((self.empty()?, removed))
        }
    }

    /// Depth-first iterator over all entries.
    pub fn iter(&'a self, root: Hash) -> CellIndexIter<'a, S> {
        CellIndexIter {
            store: self.store,
            stack: vec![Frame::Node(root)],
            leaf_iter: None,
        }
    }

    fn get_at(
        &self,
        node_hash: Hash,
        key_hash: &[u8; 32],
        depth: usize,
    ) -> StoreResult<Option<CellMeta>> {
        let node: Node = self.store.get_node(node_hash)?;
        match node {
            Node::Leaf(entries) => Ok(entries.into_iter().find(|m| &m.key_hash == key_hash)),
            Node::Branch(children) => {
                let Some(byte) = key_hash.get(depth).copied() else {
                    return Ok(None);
                };
                let child = children.into_iter().find(|(b, _)| *b == byte);
                if let Some((_, child_hash)) = child {
                    let child_hash = Hash::from_bytes(&child_hash)
                        .unwrap_or_else(|_| Hash::of_bytes(&child_hash));
                    self.get_at(child_hash, key_hash, depth + 1)
                } else {
                    Ok(None)
                }
            }
        }
    }

    fn insert_at(&self, node_hash: Hash, meta: CellMeta, depth: usize) -> StoreResult<Hash> {
        let node: Node = self.store.get_node(node_hash)?;
        match node {
            Node::Leaf(mut entries) => {
                if let Some(existing) = entries.iter_mut().find(|m| m.key_hash == meta.key_hash) {
                    *existing = meta;
                    return self.store.put_node(&Node::Leaf(entries));
                }
                entries.push(meta);
                if entries.len() > LEAF_MAX && depth < 32 {
                    self.split_leaf(entries, depth)
                } else {
                    self.store.put_node(&Node::Leaf(entries))
                }
            }
            Node::Branch(mut children) => {
                let byte = meta.key_hash.get(depth).copied().unwrap_or_default();
                let mut updated = false;
                for (b, child_bytes) in children.iter_mut() {
                    if *b == byte {
                        let child_hash = Hash::from_bytes(child_bytes)
                            .unwrap_or_else(|_| Hash::of_bytes(child_bytes));
                        let new_hash = self.insert_at(child_hash, meta.clone(), depth + 1)?;
                        *child_bytes = *new_hash.as_bytes();
                        updated = true;
                        break;
                    }
                }
                if !updated {
                    let leaf = Node::Leaf(vec![meta]);
                    let leaf_hash = self.store.put_node(&leaf)?;
                    children.push((byte, *leaf_hash.as_bytes()));
                    children.sort_by_key(|(b, _)| *b);
                }
                self.store.put_node(&Node::Branch(children))
            }
        }
    }

    fn delete_at(
        &self,
        node_hash: Hash,
        key_hash: &[u8; 32],
        depth: usize,
    ) -> StoreResult<(Option<Hash>, bool)> {
        let node: Node = self.store.get_node(node_hash)?;
        match node {
            Node::Leaf(mut entries) => {
                let len_before = entries.len();
                entries.retain(|m| m.key_hash != *key_hash);
                let len_after = entries.len();
                if entries.is_empty() {
                    Ok((None, len_before != 0))
                } else {
                    let hash = self.store.put_node(&Node::Leaf(entries))?;
                    Ok((Some(hash), len_before != len_after))
                }
            }
            Node::Branch(mut children) => {
                let Some(byte) = key_hash.get(depth).copied() else {
                    return Ok((Some(node_hash), false));
                };
                let pos = children.iter().position(|(b, _)| *b == byte);
                let Some(idx) = pos else {
                    return Ok((Some(node_hash), false));
                };
                let child_hash = Hash::from_bytes(&children[idx].1)
                    .unwrap_or_else(|_| Hash::of_bytes(&children[idx].1));
                let (new_child, removed) = self.delete_at(child_hash, key_hash, depth + 1)?;
                if !removed {
                    return Ok((Some(node_hash), false));
                }
                if let Some(hash) = new_child {
                    children[idx].1 = *hash.as_bytes();
                } else {
                    children.remove(idx);
                }
                if children.is_empty() {
                    Ok((None, true))
                } else {
                    let hash = self.store.put_node(&Node::Branch(children))?;
                    Ok((Some(hash), true))
                }
            }
        }
    }

    fn split_leaf(&self, entries: Vec<CellMeta>, depth: usize) -> StoreResult<Hash> {
        let mut buckets: BTreeMap<u8, Vec<CellMeta>> = BTreeMap::new();
        for meta in entries {
            let byte = meta.key_hash.get(depth).copied().unwrap_or_default();
            buckets.entry(byte).or_default().push(meta);
        }
        let mut children = Vec::with_capacity(buckets.len());
        for (byte, metas) in buckets {
            let hash = self.store.put_node(&Node::Leaf(metas))?;
            children.push((byte, *hash.as_bytes()));
        }
        self.store.put_node(&Node::Branch(children))
    }
}

pub struct CellIndexIter<'a, S: Store> {
    store: &'a S,
    stack: Vec<Frame>,
    leaf_iter: Option<std::vec::IntoIter<CellMeta>>,
}

enum Frame {
    Node(Hash),
}

impl<'a, S: Store> Iterator for CellIndexIter<'a, S> {
    type Item = StoreResult<CellMeta>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(iter) = &mut self.leaf_iter {
                if let Some(meta) = iter.next() {
                    return Some(Ok(meta));
                }
                self.leaf_iter = None;
            }

            let frame = self.stack.pop()?;
            match frame {
                Frame::Node(hash) => {
                    let node: StoreResult<Node> = self.store.get_node(hash);
                    match node {
                        Ok(Node::Leaf(entries)) => {
                            self.leaf_iter = Some(entries.into_iter());
                        }
                        Ok(Node::Branch(children)) => {
                            // push children in reverse to walk in ascending byte order
                            for (b, child_bytes) in children.into_iter().rev() {
                                let _ = b; // byte is unused in traversal ordering
                                let child_hash = Hash::from_bytes(&child_bytes)
                                    .unwrap_or_else(|_| Hash::of_bytes(&child_bytes));
                                self.stack.push(Frame::Node(child_hash));
                            }
                        }
                        Err(err) => return Some(Err(err)),
                    }
                }
            }
        }
    }
}
