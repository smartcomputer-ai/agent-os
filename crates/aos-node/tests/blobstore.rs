use aos_cbor::Hash;
use aos_kernel::Store;
use aos_node::{FsCas, LocalStatePaths, PersistError};
use tempfile::{TempDir, tempdir};

fn multi_chunk_payload() -> Vec<u8> {
    let len = 64 * 1024 * 3 + 137;
    (0..len).map(|idx| (idx % 251) as u8).collect()
}

#[test]
fn fs_cas_put_verified_is_idempotent_and_survives_reopen() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = tempdir()?;
    let paths = LocalStatePaths::new(temp.path());
    let store = FsCas::open_with_paths(&paths)?;

    let small = b"small";
    let empty = b"";
    let large = multi_chunk_payload();

    let small_hash = store.put_verified(small)?;
    let small_hash_again = store.put_verified(small)?;
    let empty_hash = store.put_verified(empty)?;
    let large_hash = store.put_verified(&large)?;
    let large_hash_again = store.put_verified(&large)?;

    assert_eq!(small_hash, small_hash_again);
    assert_eq!(large_hash, large_hash_again);
    assert_eq!(empty_hash, Hash::of_bytes(empty));
    assert_eq!(store.get(small_hash)?, small);
    assert_eq!(store.get(empty_hash)?, empty);
    assert_eq!(store.get(large_hash)?, large);

    drop(store);

    let reopened = FsCas::open_with_paths(&paths)?;
    assert!(reopened.has(small_hash));
    assert!(reopened.has(large_hash));
    assert_eq!(reopened.get(small_hash)?, small);
    assert_eq!(reopened.get(large_hash)?, large);
    Ok(())
}

#[test]
fn fs_cas_store_trait_round_trips_blob_and_node_access() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let paths = LocalStatePaths::new(temp.path());
    let store = FsCas::open_with_paths(&paths)?;

    let blob = multi_chunk_payload();
    let blob_hash = store.put_blob(&blob)?;
    let node_hash = store.put_node(&vec!["alpha".to_string(), "beta".to_string()])?;

    assert_eq!(blob_hash, Hash::of_bytes(&blob));
    assert!(store.has_blob(blob_hash)?);
    assert_eq!(store.get_blob(blob_hash)?, blob);
    assert!(store.has_node(node_hash)?);
    assert_eq!(
        store.get_node::<Vec<String>>(node_hash)?,
        vec!["alpha".to_string(), "beta".to_string()],
    );
    Ok(())
}

#[test]
fn fs_cas_get_reports_not_found_for_missing_blob() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let store = FsCas::open_cas_root(temp.path())?;
    let missing = Hash::of_bytes(b"missing");

    assert!(!store.has(missing));
    match store.get(missing) {
        Ok(_) => panic!("expected missing blob lookup to fail"),
        Err(PersistError::NotFound(message)) => {
            assert!(message.contains("blob"));
            assert!(message.contains(&missing.to_hex()));
        }
        Err(other) => panic!("expected not found error, got {other:?}"),
    }

    Ok(())
}

#[test]
fn fs_cas_shards_by_digest_hex_without_hash_prefix() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let cas = FsCas::open_cas_root(temp.path())?;
    let payload = b"hello world";
    let hash = cas.put_blob(payload)?;
    let digest_hex = hex::encode(hash.as_bytes());
    let expected = temp.path().join(&digest_hex[..2]).join(&digest_hex[2..]);

    assert!(
        expected.exists(),
        "expected CAS blob at {}",
        expected.display()
    );
    assert!(!temp.path().join("sh").exists());
    Ok(())
}

#[test]
fn fs_cas_open_with_paths_uses_local_cas_root() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let paths = LocalStatePaths::new(temp.path());
    let cas = FsCas::open_with_paths(&paths)?;
    let payload = b"local-cas";
    let hash = cas.put_verified(payload)?;

    assert_eq!(cas.root(), paths.cas_root());
    assert_eq!(cas.get(hash)?, payload);
    assert!(paths.cas_root().join(&hash.to_hex()[7..9]).exists());
    Ok(())
}
