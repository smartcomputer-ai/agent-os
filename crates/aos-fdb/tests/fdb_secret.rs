#![cfg(feature = "foundationdb-backend")]

mod common;

use aos_fdb::{
    CreateUniverseRequest, PutSecretVersionRequest, SecretAuditAction, SecretAuditRecord,
    SecretBindingRecord, SecretBindingSourceKind, SecretBindingStatus, SecretStore,
    SecretVersionStatus, UniverseStore,
};

fn hosted_binding(binding_id: &str, created_at_ns: u64) -> SecretBindingRecord {
    SecretBindingRecord {
        binding_id: binding_id.into(),
        source_kind: SecretBindingSourceKind::NodeSecretStore,
        env_var: None,
        required_placement_pin: None,
        latest_version: None,
        created_at_ns,
        updated_at_ns: created_at_ns,
        status: SecretBindingStatus::Active,
    }
}

#[test]
fn secret_binding_crud_and_version_round_trip_against_fdb() -> Result<(), Box<dyn std::error::Error>>
{
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    ctx.persistence.create_universe(CreateUniverseRequest {
        universe_id: Some(ctx.universe),
        handle: None,
        created_at_ns: 10,
    })?;

    let created = ctx
        .persistence
        .put_secret_binding(ctx.universe, hosted_binding("app/openai", 100))?;
    assert_eq!(created.binding_id, "app/openai");
    assert_eq!(created.latest_version, None);
    assert_eq!(created.status, SecretBindingStatus::Active);

    let loaded = ctx
        .persistence
        .get_secret_binding(ctx.universe, "app/openai")?
        .expect("binding should exist");
    assert_eq!(loaded, created);

    let version = ctx.persistence.put_secret_version(
        ctx.universe,
        PutSecretVersionRequest {
            binding_id: "app/openai".into(),
            digest: "sha256:1111111111111111111111111111111111111111111111111111111111111111"
                .into(),
            ciphertext: vec![1, 2, 3, 4],
            dek_wrapped: vec![5, 6, 7],
            nonce: vec![9; 24],
            enc_alg: "aes-256-gcm-siv+wrap-v1".into(),
            kek_id: "test-kek".into(),
            created_at_ns: 200,
            created_by: Some("tester".into()),
        },
    )?;
    assert_eq!(version.binding_id, "app/openai");
    assert_eq!(version.version, 1);
    assert_eq!(version.status, SecretVersionStatus::Active);

    let binding_after_version = ctx
        .persistence
        .get_secret_binding(ctx.universe, "app/openai")?
        .expect("binding should exist");
    assert_eq!(binding_after_version.latest_version, Some(1));

    let versions = ctx
        .persistence
        .list_secret_versions(ctx.universe, "app/openai", 10)?;
    assert_eq!(versions, vec![version.clone()]);

    let loaded_version = ctx
        .persistence
        .get_secret_version(ctx.universe, "app/openai", 1)?
        .expect("version should exist");
    assert_eq!(loaded_version, version);

    let disabled = ctx
        .persistence
        .disable_secret_binding(ctx.universe, "app/openai", 300)?;
    assert_eq!(disabled.status, SecretBindingStatus::Disabled);

    Ok(())
}

#[test]
fn secret_records_survive_fdb_reopen() -> Result<(), Box<dyn std::error::Error>> {
    if common::skip_if_cluster_unreachable() {
        return Ok(());
    }

    let ctx = common::open_test_context(common::test_config())?;
    ctx.persistence.create_universe(CreateUniverseRequest {
        universe_id: Some(ctx.universe),
        handle: None,
        created_at_ns: 11,
    })?;

    ctx.persistence.put_secret_binding(
        ctx.universe,
        SecretBindingRecord {
            binding_id: "vault/anthropic".into(),
            source_kind: SecretBindingSourceKind::NodeSecretStore,
            env_var: None,
            required_placement_pin: Some("gpu".into()),
            latest_version: None,
            created_at_ns: 120,
            updated_at_ns: 120,
            status: SecretBindingStatus::Active,
        },
    )?;

    let version = ctx.persistence.put_secret_version(
        ctx.universe,
        PutSecretVersionRequest {
            binding_id: "vault/anthropic".into(),
            digest: "sha256:2222222222222222222222222222222222222222222222222222222222222222"
                .into(),
            ciphertext: vec![0xaa, 0xbb, 0xcc],
            dek_wrapped: vec![0xdd, 0xee],
            nonce: vec![0x11; 24],
            enc_alg: "aes-256-gcm-siv+wrap-v1".into(),
            kek_id: "test-kek-2".into(),
            created_at_ns: 220,
            created_by: Some("tester".into()),
        },
    )?;

    ctx.persistence.append_secret_audit(
        ctx.universe,
        SecretAuditRecord {
            ts_ns: 221,
            action: SecretAuditAction::VersionPut,
            binding_id: "vault/anthropic".into(),
            version: Some(version.version),
            digest: Some(version.digest.clone()),
            actor: Some("tester".into()),
        },
    )?;

    let reopened = common::open_persistence(common::test_config())?;
    let binding = reopened
        .get_secret_binding(ctx.universe, "vault/anthropic")?
        .expect("binding should survive reopen");
    assert_eq!(
        binding.source_kind,
        SecretBindingSourceKind::NodeSecretStore
    );
    assert_eq!(binding.env_var, None);
    assert_eq!(binding.required_placement_pin.as_deref(), Some("gpu"));
    assert_eq!(binding.latest_version, Some(1));

    let loaded_version = reopened
        .get_secret_version(ctx.universe, "vault/anthropic", 1)?
        .expect("version should survive reopen");
    assert_eq!(loaded_version, version);

    Ok(())
}
