use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::journal::JournalSeq;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelSnapshot {
    reducer_state: Vec<(String, Vec<u8>)>,
    recent_receipts: Vec<[u8; 32]>,
    height: JournalSeq,
}

impl KernelSnapshot {
    pub fn new(
        height: JournalSeq,
        reducer_state: HashMap<String, Vec<u8>>,
        recent_receipts: Vec<[u8; 32]>,
    ) -> Self {
        Self {
            reducer_state: reducer_state.into_iter().collect(),
            recent_receipts,
            height,
        }
    }

    pub fn into_reducer_state(self) -> HashMap<String, Vec<u8>> {
        self.reducer_state.into_iter().collect()
    }

    pub fn recent_receipts(&self) -> &[[u8; 32]] {
        &self.recent_receipts
    }

    pub fn height(&self) -> JournalSeq {
        self.height
    }
}

pub fn receipts_to_vecdeque(
    receipts: &[[u8; 32]],
    cap: usize,
) -> (VecDeque<[u8; 32]>, HashSet<[u8; 32]>) {
    let mut deque = VecDeque::new();
    let mut set = HashSet::new();
    for hash in receipts.iter().cloned().take(cap) {
        deque.push_back(hash);
        set.insert(hash);
    }
    (deque, set)
}
