#![cfg(feature = "foundationdb-backend")]

mod common;

use std::fs;

use aos_fdb::{PersistCorruption, PersistError, WorldPersistence, cas_object_key};

#[test]
fn cas_put_verified_is_idempotent_for_inline_and_external_blobs()
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

    let object_path = ctx
        .object_store
        .path()
        .join(cas_object_key(ctx.universe, external_hash));
    assert!(object_path.exists());

    Ok(())
}

#[test]
fn cas_get_survives_backend_reopen() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let bytes = b"this payload is large enough to live in object storage";
    let hash = ctx.persistence.cas_put_verified(ctx.universe, bytes)?;

    let reopened = common::open_persistence(ctx.object_store.path(), common::test_config())?;
    assert!(reopened.cas_has(ctx.universe, hash)?);
    assert_eq!(reopened.cas_get(ctx.universe, hash)?, bytes);

    Ok(())
}

#[test]
fn cas_get_detects_tampered_object_store_body() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    let bytes = b"this payload is definitely externalized";
    let hash = ctx.persistence.cas_put_verified(ctx.universe, bytes)?;
    let object_path = ctx
        .object_store
        .path()
        .join(cas_object_key(ctx.universe, hash));
    fs::write(&object_path, b"tampered-body")?;

    let err = common::open_persistence(ctx.object_store.path(), common::test_config())?
        .cas_get(ctx.universe, hash)
        .unwrap_err();
    assert!(matches!(
        err,
        PersistError::Corrupt(PersistCorruption::CasBodyHashMismatch { .. })
    ));

    Ok(())
}
