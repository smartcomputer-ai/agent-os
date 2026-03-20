use aos_cbor::Hash;

use crate::protocol::{
    CreateUniverseRequest, CreateWorldSeedRequest, ForkWorldRequest, PersistError,
    PutSecretVersionRequest, SecretAuditRecord, SecretBindingRecord, SecretVersionRecord,
    UniverseCreateResult, UniverseId, UniverseRecord, WorldCreateResult, WorldForkResult, WorldId,
    WorldLineage, WorldStore,
};

pub trait WorldAdminStore: WorldStore {
    fn world_create_from_seed(
        &self,
        universe: UniverseId,
        request: CreateWorldSeedRequest,
    ) -> Result<WorldCreateResult, PersistError>;

    fn world_prepare_manifest_bootstrap(
        &self,
        universe: UniverseId,
        world: WorldId,
        manifest_hash: Hash,
        handle: String,
        placement_pin: Option<String>,
        created_at_ns: u64,
        lineage: WorldLineage,
    ) -> Result<(), PersistError>;

    fn world_drop_manifest_bootstrap(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<(), PersistError>;

    fn world_fork(
        &self,
        universe: UniverseId,
        request: ForkWorldRequest,
    ) -> Result<WorldForkResult, PersistError>;

    fn set_world_handle(
        &self,
        universe: UniverseId,
        world: WorldId,
        handle: String,
    ) -> Result<(), PersistError>;
}

pub trait UniverseStore: WorldStore {
    fn create_universe(
        &self,
        request: CreateUniverseRequest,
    ) -> Result<UniverseCreateResult, PersistError>;

    fn delete_universe(
        &self,
        universe: UniverseId,
        deleted_at_ns: u64,
    ) -> Result<UniverseRecord, PersistError>;

    fn get_universe(&self, universe: UniverseId) -> Result<UniverseRecord, PersistError>;

    fn get_universe_by_handle(&self, handle: &str) -> Result<UniverseRecord, PersistError>;

    fn list_universes(
        &self,
        after: Option<UniverseId>,
        limit: u32,
    ) -> Result<Vec<UniverseRecord>, PersistError>;

    fn set_universe_handle(
        &self,
        universe: UniverseId,
        handle: String,
    ) -> Result<UniverseRecord, PersistError>;
}

pub trait SecretStore: WorldStore {
    fn put_secret_binding(
        &self,
        universe: UniverseId,
        record: SecretBindingRecord,
    ) -> Result<SecretBindingRecord, PersistError>;

    fn get_secret_binding(
        &self,
        universe: UniverseId,
        binding_id: &str,
    ) -> Result<Option<SecretBindingRecord>, PersistError>;

    fn list_secret_bindings(
        &self,
        universe: UniverseId,
        limit: u32,
    ) -> Result<Vec<SecretBindingRecord>, PersistError>;

    fn disable_secret_binding(
        &self,
        universe: UniverseId,
        binding_id: &str,
        updated_at_ns: u64,
    ) -> Result<SecretBindingRecord, PersistError>;

    fn put_secret_version(
        &self,
        universe: UniverseId,
        request: PutSecretVersionRequest,
    ) -> Result<SecretVersionRecord, PersistError>;

    fn get_secret_version(
        &self,
        universe: UniverseId,
        binding_id: &str,
        version: u64,
    ) -> Result<Option<SecretVersionRecord>, PersistError>;

    fn list_secret_versions(
        &self,
        universe: UniverseId,
        binding_id: &str,
        limit: u32,
    ) -> Result<Vec<SecretVersionRecord>, PersistError>;

    fn append_secret_audit(
        &self,
        universe: UniverseId,
        record: SecretAuditRecord,
    ) -> Result<(), PersistError>;
}
