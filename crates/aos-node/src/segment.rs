use std::io::{Cursor, Read};

use aos_cbor::Hash;

use crate::{
    JournalHeight, PersistCorruption, PersistError, SegmentExportRequest, SegmentId,
    SegmentIndexRecord,
};

pub fn segment_checksum(bytes: &[u8]) -> String {
    format!("sha256:{}", Hash::of_bytes(bytes).to_hex())
}

pub fn validate_segment_export_request(request: &SegmentExportRequest) -> Result<(), PersistError> {
    if request.delete_chunk_entries == 0 {
        return Err(PersistError::validation(
            "segment export requires delete_chunk_entries > 0",
        ));
    }
    Ok(())
}

pub fn encode_segment_entries(
    segment: SegmentId,
    entries: &[(JournalHeight, Vec<u8>)],
) -> Result<Vec<u8>, PersistError> {
    let expected_len = (segment.end - segment.start + 1) as usize;
    if entries.len() != expected_len {
        return Err(PersistError::validation(format!(
            "segment {}-{} expected {expected_len} entries, got {}",
            segment.start,
            segment.end,
            entries.len()
        )));
    }

    let mut out = Vec::new();
    let mut expected_height = segment.start;
    for (height, entry) in entries {
        if *height != expected_height {
            return Err(PersistError::validation(format!(
                "segment entry height {height} is not contiguous from expected {expected_height}"
            )));
        }
        out.extend_from_slice(&(entry.len() as u64).to_be_bytes());
        out.extend_from_slice(entry);
        expected_height += 1;
    }
    Ok(out)
}

pub fn decode_segment_entries(
    record: &SegmentIndexRecord,
    bytes: &[u8],
) -> Result<Vec<(JournalHeight, Vec<u8>)>, PersistError> {
    let actual_checksum = segment_checksum(bytes);
    if actual_checksum != record.checksum {
        return Err(PersistCorruption::SegmentChecksumMismatch {
            segment: record.segment,
            expected: record.checksum.clone(),
            actual: actual_checksum,
        }
        .into());
    }

    let mut reader = Cursor::new(bytes);
    let mut out = Vec::new();
    let mut height = record.segment.start;
    while (reader.position() as usize) < bytes.len() {
        if height > record.segment.end {
            return Err(PersistCorruption::MalformedSegment {
                segment: record.segment,
                detail: "segment object contains more entries than index range".into(),
            }
            .into());
        }

        let mut len_buf = [0u8; 8];
        reader
            .read_exact(&mut len_buf)
            .map_err(|_| PersistCorruption::MalformedSegment {
                segment: record.segment,
                detail: "segment object ended before entry length".into(),
            })?;
        let len = u64::from_be_bytes(len_buf);
        let mut entry = vec![0u8; len as usize];
        reader
            .read_exact(&mut entry)
            .map_err(|_| PersistCorruption::MalformedSegment {
                segment: record.segment,
                detail: "segment object ended before entry body".into(),
            })?;
        out.push((height, entry));
        height += 1;
    }

    if height != record.segment.end + 1 {
        return Err(PersistCorruption::MalformedSegment {
            segment: record.segment,
            detail: "segment object entry count does not match index range".into(),
        }
        .into());
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_codec_round_trips() {
        let segment = SegmentId::new(3, 4).unwrap();
        let entries = vec![(3, b"a".to_vec()), (4, b"bc".to_vec())];
        let bytes = encode_segment_entries(segment, &entries).unwrap();
        let record = SegmentIndexRecord {
            segment,
            body_ref: Hash::of_bytes(&bytes).to_hex(),
            checksum: segment_checksum(&bytes),
        };

        assert_eq!(decode_segment_entries(&record, &bytes).unwrap(), entries);
    }
}
