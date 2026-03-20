mod common;

use aos_cbor::Hash;
use aos_kernel::Store;
use aos_node::{UniverseId, WorldStore};
use aos_sqlite::FsCas;
use tempfile::TempDir;
use uuid::Uuid;

use common::{open_store, temp_state_root, universe};

fn multi_chunk_payload() -> Vec<u8> {
    let len = 64 * 1024 * 3 + 137;
    (0..len).map(|idx| (idx % 251) as u8).collect()
}

fn second_universe() -> UniverseId {
    UniverseId::from(Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap())
}

#[test]
fn cas_put_verified_is_idempotent_and_survives_reopen() {
    let (_temp, paths) = temp_state_root();
    let store = open_store(&paths);

    let small = b"small";
    let empty = b"";
    let large = multi_chunk_payload();

    let small_hash = store.cas_put_verified(universe(), small).unwrap();
    let small_hash_again = store.cas_put_verified(universe(), small).unwrap();
    let empty_hash = store.cas_put_verified(universe(), empty).unwrap();
    let large_hash = store.cas_put_verified(universe(), &large).unwrap();
    let large_hash_again = store.cas_put_verified(universe(), &large).unwrap();

    assert_eq!(small_hash, small_hash_again);
    assert_eq!(large_hash, large_hash_again);
    assert_eq!(empty_hash, Hash::of_bytes(empty));
    assert_eq!(store.cas_get(universe(), small_hash).unwrap(), small);
    assert_eq!(store.cas_get(universe(), empty_hash).unwrap(), empty);
    assert_eq!(store.cas_get(universe(), large_hash).unwrap(), large);

    drop(store);
    let reopened = open_store(&paths);
    assert!(reopened.cas_has(universe(), small_hash).unwrap());
    assert!(reopened.cas_has(universe(), large_hash).unwrap());
    assert_eq!(reopened.cas_get(universe(), small_hash).unwrap(), small);
    assert_eq!(reopened.cas_get(universe(), large_hash).unwrap(), large);
}

#[test]
fn cas_rejects_non_local_universe_ids() {
    let (_temp, paths) = temp_state_root();
    let store = open_store(&paths);

    let hash = store.cas_put_verified(universe(), b"shared-bytes").unwrap();

    assert!(store.cas_has(universe(), hash).unwrap());
    assert!(matches!(
        store.cas_has(second_universe(), hash),
        Err(aos_node::PersistError::NotFound(_))
    ));
    assert!(matches!(
        store.cas_get(second_universe(), hash),
        Err(aos_node::PersistError::NotFound(_))
    ));
}

#[test]
fn fs_cas_shards_by_digest_hex_without_hash_prefix() {
    let temp = TempDir::new().unwrap();
    let cas = FsCas::open_cas_root(temp.path()).unwrap();
    let payload = b"hello world";
    let hash = cas.put_blob(payload).unwrap();
    let digest_hex = hex::encode(hash.as_bytes());
    let expected = temp.path().join(&digest_hex[..2]).join(&digest_hex[2..]);

    assert!(
        expected.exists(),
        "expected CAS blob at {}",
        expected.display()
    );
    assert!(!temp.path().join("sh").exists());
}
