#![cfg(feature = "foundationdb-backend")]

mod common;

use std::io::Cursor;

use aos_cbor::Hash;
use aos_fdb::{CasLayoutKind, CasStore, PersistError, UniverseId, WorldStore};
use uuid::Uuid;

fn second_universe() -> UniverseId {
    UniverseId::from(Uuid::new_v4())
}

fn multi_chunk_payload() -> Vec<u8> {
    let len = 64 * 1024 * 3 + 137;
    (0..len).map(|idx| (idx % 251) as u8).collect()
}

#[test]
fn cas_put_verified_is_idempotent_for_small_and_large_blobs()
-> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let inline = b"small";
    let external = b"this object must be externalized";

    let inline_hash = ctx.persistence.cas_put_verified(ctx.universe, inline)?;
    let inline_hash_again = ctx.persistence.cas_put_verified(ctx.universe, inline)?;
    let external_hash = ctx.persistence.cas_put_verified(ctx.universe, external)?;
    let external_hash_again = ctx.persistence.cas_put_verified(ctx.universe, external)?;

    assert_eq!(inline_hash, inline_hash_again);
    assert_eq!(external_hash, external_hash_again);
    assert_eq!(ctx.persistence.cas_get(ctx.universe, inline_hash)?, inline);
    assert_eq!(
        ctx.persistence.cas_get(ctx.universe, external_hash)?,
        external
    );

    Ok(())
}

#[test]
fn cas_stat_reports_direct_layout_and_exact_size() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let bytes = multi_chunk_payload();
    let hash = ctx.persistence.cas_put_verified(ctx.universe, &bytes)?;
    let root = ctx.persistence.cas().stat(ctx.universe, hash)?;

    assert_eq!(root.layout_kind, CasLayoutKind::Direct);
    assert_eq!(root.size_bytes, bytes.len() as u64);
    assert_eq!(root.chunk_size, 64 * 1024);
    assert_eq!(root.chunk_count, 4);

    Ok(())
}

#[test]
fn cas_streaming_write_known_hash_round_trips_across_multiple_chunks()
-> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let bytes = multi_chunk_payload();
    let expected = Hash::of_bytes(&bytes);
    let mut reader = Cursor::new(bytes.as_slice());

    let hash = ctx.persistence.cas().put_reader_known_hash(
        ctx.universe,
        expected,
        bytes.len() as u64,
        &mut reader,
    )?;

    assert_eq!(hash, expected);
    assert!(ctx.persistence.cas_has(ctx.universe, hash)?);
    assert_eq!(ctx.persistence.cas_get(ctx.universe, hash)?, bytes);

    Ok(())
}

#[test]
fn cas_streaming_write_rejects_hash_mismatch() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let bytes = b"known hash stream";
    let mut reader = Cursor::new(bytes.as_slice());
    let err = ctx
        .persistence
        .cas()
        .put_reader_known_hash(
            ctx.universe,
            Hash::of_bytes(b"different"),
            bytes.len() as u64,
            &mut reader,
        )
        .unwrap_err();

    assert!(matches!(err, PersistError::Validation(_)));

    Ok(())
}

#[test]
fn cas_streaming_write_rejects_declared_size_mismatch() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let bytes = b"12345678";
    let expected = Hash::of_bytes(bytes);
    let mut reader = Cursor::new(bytes.as_slice());
    let err = ctx
        .persistence
        .cas()
        .put_reader_known_hash(ctx.universe, expected, bytes.len() as u64 - 1, &mut reader)
        .unwrap_err();

    assert!(matches!(err, PersistError::Validation(_)));

    Ok(())
}

#[test]
fn cas_full_streaming_read_round_trips_bytes_and_root() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let bytes = multi_chunk_payload();
    let hash = ctx.persistence.cas_put_verified(ctx.universe, &bytes)?;
    let mut out = Vec::new();
    let root = ctx
        .persistence
        .cas()
        .read_to_writer(ctx.universe, hash, &mut out)?;

    assert_eq!(out, bytes);
    assert_eq!(root.size_bytes, bytes.len() as u64);
    assert_eq!(root.chunk_count, 4);

    Ok(())
}

#[test]
fn cas_get_survives_backend_reopen() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let bytes = b"this payload is large enough to span the FDB CAS path";
    let hash = ctx.persistence.cas_put_verified(ctx.universe, bytes)?;

    let reopened = common::open_persistence(common::test_config())?;
    assert!(reopened.cas_has(ctx.universe, hash)?);
    assert_eq!(reopened.cas_get(ctx.universe, hash)?, bytes);

    Ok(())
}

#[test]
fn cas_read_range_returns_expected_slice_within_single_chunk()
-> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let bytes = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let hash = ctx.persistence.cas_put_verified(ctx.universe, bytes)?;
    let mut out = Vec::new();
    ctx.persistence
        .cas()
        .read_range_to_writer(ctx.universe, hash, 10, 6, &mut out)?;
    assert_eq!(out, b"abcdef");

    Ok(())
}

#[test]
fn cas_read_range_crosses_chunk_boundaries() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let bytes = multi_chunk_payload();
    let hash = ctx.persistence.cas_put_verified(ctx.universe, &bytes)?;

    let offset = (64 * 1024 - 12) as u64;
    let len = (64 * 1024 + 29) as u64;
    let mut out = Vec::new();
    ctx.persistence
        .cas()
        .read_range_to_writer(ctx.universe, hash, offset, len, &mut out)?;

    assert_eq!(out, bytes[offset as usize..(offset + len) as usize]);

    Ok(())
}

#[test]
fn cas_read_range_rejects_out_of_bounds_requests() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let bytes = b"short payload";
    let hash = ctx.persistence.cas_put_verified(ctx.universe, bytes)?;

    let mut out = Vec::new();
    let err = ctx
        .persistence
        .cas()
        .read_range_to_writer(ctx.universe, hash, bytes.len() as u64 + 1, 1, &mut out)
        .unwrap_err();

    assert!(matches!(err, PersistError::Validation(_)));

    Ok(())
}

#[test]
fn cas_zero_byte_blob_round_trips() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let hash = ctx.persistence.cas_put_verified(ctx.universe, b"")?;
    let root = ctx.persistence.cas().stat(ctx.universe, hash)?;
    let mut out = Vec::new();
    ctx.persistence
        .cas()
        .read_to_writer(ctx.universe, hash, &mut out)?;

    assert_eq!(hash, Hash::of_bytes(b""));
    assert_eq!(root.size_bytes, 0);
    assert_eq!(root.chunk_count, 0);
    assert!(out.is_empty());

    Ok(())
}

#[test]
fn cas_is_isolated_by_universe() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let other_universe = second_universe();
    let bytes = b"universe isolated";
    let hash = ctx.persistence.cas_put_verified(ctx.universe, bytes)?;

    assert!(ctx.persistence.cas_has(ctx.universe, hash)?);
    assert!(!ctx.persistence.cas_has(other_universe, hash)?);
    let err = ctx.persistence.cas_get(other_universe, hash).unwrap_err();
    assert!(matches!(err, PersistError::NotFound(_)));

    Ok(())
}

#[test]
fn cas_streaming_write_is_idempotent_when_blob_already_exists()
-> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let bytes = multi_chunk_payload();
    let expected = Hash::of_bytes(&bytes);

    let first = ctx.persistence.cas_put_verified(ctx.universe, &bytes)?;
    let mut reader = Cursor::new(bytes.as_slice());
    let second = ctx.persistence.cas().put_reader_known_hash(
        ctx.universe,
        expected,
        bytes.len() as u64,
        &mut reader,
    )?;

    assert_eq!(first, expected);
    assert_eq!(second, expected);

    let reopened = common::open_persistence(common::test_config())?;
    assert_eq!(reopened.cas_get(ctx.universe, expected)?, bytes);

    Ok(())
}
