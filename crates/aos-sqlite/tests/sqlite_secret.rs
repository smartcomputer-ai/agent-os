mod common;

use aos_cbor::Hash;
use aos_node::{
    PutSecretVersionRequest, SecretAuditAction, SecretAuditRecord, SecretBindingRecord,
    SecretBindingSourceKind, SecretBindingStatus, SecretStore, SecretVersionStatus,
};

use common::{open_store, temp_state_root, universe};

#[test]
fn secret_binding_crud_and_reopen_round_trip() {
    let (_temp, paths) = temp_state_root();
    let store = open_store(&paths);

    let created = store
        .put_secret_binding(
            universe(),
            SecretBindingRecord {
                binding_id: "app/openai".into(),
                source_kind: SecretBindingSourceKind::NodeSecretStore,
                env_var: None,
                required_placement_pin: Some("gpu".into()),
                latest_version: None,
                created_at_ns: 100,
                updated_at_ns: 100,
                status: SecretBindingStatus::Active,
            },
        )
        .unwrap();
    assert_eq!(created.binding_id, "app/openai");

    let version = store
        .put_secret_version(
            universe(),
            PutSecretVersionRequest {
                binding_id: "app/openai".into(),
                digest: Hash::of_bytes(b"secret").to_hex(),
                ciphertext: vec![1, 2, 3, 4],
                dek_wrapped: vec![5, 6, 7],
                nonce: vec![8; 24],
                enc_alg: "aes-256-gcm-siv+wrap-v1".into(),
                kek_id: "test-kek".into(),
                created_at_ns: 200,
                created_by: Some("tester".into()),
            },
        )
        .unwrap();
    assert_eq!(version.status, SecretVersionStatus::Active);

    store
        .append_secret_audit(
            universe(),
            SecretAuditRecord {
                ts_ns: 201,
                action: SecretAuditAction::VersionPut,
                binding_id: "app/openai".into(),
                version: Some(version.version),
                digest: Some(version.digest.clone()),
                actor: Some("tester".into()),
            },
        )
        .unwrap();
    store
        .disable_secret_binding(universe(), "app/openai", 300)
        .unwrap();
    drop(store);

    let reopened = open_store(&paths);
    let binding = reopened
        .get_secret_binding(universe(), "app/openai")
        .unwrap()
        .unwrap();
    assert_eq!(binding.required_placement_pin.as_deref(), Some("gpu"));
    assert_eq!(binding.latest_version, Some(1));
    assert_eq!(binding.status, SecretBindingStatus::Disabled);
    assert_eq!(
        reopened
            .get_secret_version(universe(), "app/openai", 1)
            .unwrap()
            .unwrap(),
        version
    );
    assert_eq!(
        reopened
            .list_secret_versions(universe(), "app/openai", 10)
            .unwrap()
            .len(),
        1
    );
}
