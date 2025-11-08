use std::sync::{Arc, Mutex};

use super::{Journal, JournalEntry, JournalError, JournalKind, JournalSeq, OwnedJournalEntry};

/// Simple in-memory journal useful for unit tests and TestWorld scenarios.
#[derive(Debug, Default, Clone)]
pub struct MemJournal {
    entries: Arc<Mutex<Vec<OwnedJournalEntry>>>,
}

impl MemJournal {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn from_entries(entries: &[OwnedJournalEntry]) -> Self {
        Self {
            entries: Arc::new(Mutex::new(entries.to_vec())),
        }
    }

    pub fn entries(&self) -> Vec<OwnedJournalEntry> {
        self.entries.lock().unwrap().clone()
    }
}

impl Journal for MemJournal {
    fn append(&mut self, entry: JournalEntry<'_>) -> Result<JournalSeq, JournalError> {
        let mut guard = self.entries.lock().unwrap();
        let seq = guard.len() as JournalSeq;
        guard.push(OwnedJournalEntry {
            seq,
            kind: entry.kind,
            payload: entry.payload.to_vec(),
        });
        Ok(seq)
    }

    fn load_from(&self, from: JournalSeq) -> Result<Vec<OwnedJournalEntry>, JournalError> {
        Ok(self
            .entries()
            .into_iter()
            .filter(|entry| entry.seq >= from)
            .collect())
    }

    fn next_seq(&self) -> JournalSeq {
        self.entries.lock().unwrap().len() as JournalSeq
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_and_load_round_trip() {
        let mut journal = MemJournal::new();
        journal
            .append(JournalEntry::new(JournalKind::DomainEvent, b"first"))
            .unwrap();
        journal
            .append(JournalEntry::new(JournalKind::EffectIntent, b"second"))
            .unwrap();

        let all = journal.load_from(0).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].seq, 0);
        assert_eq!(all[0].payload, b"first");
        assert_eq!(all[1].seq, 1);
        assert_eq!(all[1].kind, JournalKind::EffectIntent);

        let tail = journal.load_from(1).unwrap();
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].payload, b"second");
    }
}
