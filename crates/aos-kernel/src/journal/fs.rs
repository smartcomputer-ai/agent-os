use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

use aos_cbor::to_canonical_cbor;

use super::{Journal, JournalEntry, JournalError, JournalKind, JournalSeq, OwnedJournalEntry};

const JOURNAL_DIR: &str = "journal";
const JOURNAL_FILE: &str = "journal.log";

/// Filesystem-backed journal that stores length-prefixed canonical CBOR records.
#[derive(Debug)]
pub struct FsJournal {
    path: PathBuf,
    next_seq: JournalSeq,
}

impl FsJournal {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, JournalError> {
        let journal_dir = root.as_ref().join(JOURNAL_DIR);
        fs::create_dir_all(&journal_dir)?;
        let path = journal_dir.join(JOURNAL_FILE);
        if !path.exists() {
            File::create(&path)?;
        }
        let entries = read_all_records(&path)?;
        let next_seq = entries.last().map(|entry| entry.seq + 1).unwrap_or(0);
        Ok(Self { path, next_seq })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Journal for FsJournal {
    fn append(&mut self, entry: JournalEntry<'_>) -> Result<JournalSeq, JournalError> {
        let seq = self.next_seq;
        let record = super::DiskRecord {
            seq,
            kind: entry.kind,
            payload: entry.payload,
        };
        let bytes = to_canonical_cbor(&record)?;
        let len = bytes.len();
        if len > u32::MAX as usize {
            return Err(JournalError::Corrupt("entry larger than 4GiB".into()));
        }
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.path)?;
        file.write_all(&(len as u32).to_le_bytes())?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        self.next_seq += 1;
        Ok(seq)
    }

    fn load_from(&self, from: JournalSeq) -> Result<Vec<OwnedJournalEntry>, JournalError> {
        let mut entries = read_all_records(&self.path)?;
        entries.retain(|entry| entry.seq >= from);
        Ok(entries)
    }

    fn next_seq(&self) -> JournalSeq {
        self.next_seq
    }
}

fn read_all_records(path: &Path) -> Result<Vec<OwnedJournalEntry>, JournalError> {
    let mut file = File::open(path)?;
    let mut entries = Vec::new();
    loop {
        let mut len_buf = [0u8; 4];
        let read = file.read(&mut len_buf)?;
        if read == 0 {
            break;
        }
        if read < len_buf.len() {
            return Err(JournalError::Corrupt(format!(
                "truncated length header (read {read} bytes)"
            )));
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        if let Err(err) = file.read_exact(&mut buf) {
            if err.kind() == ErrorKind::UnexpectedEof {
                return Err(JournalError::Corrupt("truncated entry payload".into()));
            }
            return Err(err.into());
        }
        let entry: OwnedJournalEntry = serde_cbor::from_slice(&buf)?;
        entries.push(entry);
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn writes_and_recovers_entries() {
        let tmp = TempDir::new().unwrap();
        let mut journal = FsJournal::open(tmp.path()).unwrap();
        assert_eq!(journal.next_seq(), 0);
        journal
            .append(JournalEntry::new(JournalKind::EffectIntent, b"a"))
            .unwrap();
        journal
            .append(JournalEntry::new(JournalKind::EffectReceipt, b"b"))
            .unwrap();

        let again = FsJournal::open(tmp.path()).unwrap();
        assert_eq!(again.next_seq(), 2);
        let entries = again.load_from(0).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].seq, 0);
        assert_eq!(entries[0].payload, b"a");
        assert_eq!(entries[1].kind, JournalKind::EffectReceipt);
    }

    #[test]
    fn load_from_filters_sequence() {
        let tmp = TempDir::new().unwrap();
        let mut journal = FsJournal::open(tmp.path()).unwrap();
        for payload in [b"one".as_ref(), b"two", b"three"] {
            journal
                .append(JournalEntry::new(JournalKind::DomainEvent, payload))
                .unwrap();
        }
        let entries = journal.load_from(2).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].payload, b"three");
    }

    #[test]
    fn detects_truncated_entry() {
        let tmp = TempDir::new().unwrap();
        {
            let mut journal = FsJournal::open(tmp.path()).unwrap();
            journal
                .append(JournalEntry::new(JournalKind::EffectIntent, b"payload"))
                .unwrap();
        }

        let log_path = tmp.path().join(JOURNAL_DIR).join(JOURNAL_FILE);
        let len = std::fs::metadata(&log_path).unwrap().len();
        let file = OpenOptions::new().write(true).open(&log_path).unwrap();
        file.set_len(len - 1).unwrap();

        let err = FsJournal::open(tmp.path()).unwrap_err();
        assert!(matches!(err, JournalError::Corrupt(_)));
    }
}
