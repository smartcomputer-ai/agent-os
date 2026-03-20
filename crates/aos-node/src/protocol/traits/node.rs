use crate::protocol::{
    CommandIngress, CommandRecord, InboxItem, InboxSeq, PersistError, UniverseId,
    WorldAdminLifecycle, WorldId, WorldRuntimeInfo, WorldStore,
};

/// Node-level world catalog and mutable runtime metadata shared by local and
/// hosted nodes.
pub trait NodeCatalog: WorldStore {
    fn world_runtime_info(
        &self,
        universe: UniverseId,
        world: WorldId,
        now_ns: u64,
    ) -> Result<WorldRuntimeInfo, PersistError>;

    fn world_runtime_info_by_handle(
        &self,
        universe: UniverseId,
        handle: &str,
        now_ns: u64,
    ) -> Result<WorldRuntimeInfo, PersistError>;

    fn list_worlds(
        &self,
        universe: UniverseId,
        now_ns: u64,
        after: Option<WorldId>,
        limit: u32,
    ) -> Result<Vec<WorldRuntimeInfo>, PersistError>;

    fn set_world_placement_pin(
        &self,
        universe: UniverseId,
        world: WorldId,
        placement_pin: Option<String>,
    ) -> Result<(), PersistError>;

    fn set_world_admin_lifecycle(
        &self,
        universe: UniverseId,
        world: WorldId,
        admin: WorldAdminLifecycle,
    ) -> Result<(), PersistError>;
}

pub trait WorldIngressStore: WorldStore {
    fn enqueue_ingress(
        &self,
        universe: UniverseId,
        world: WorldId,
        item: InboxItem,
    ) -> Result<InboxSeq, PersistError>;
}

pub trait CommandStore: WorldStore {
    fn command_record(
        &self,
        universe: UniverseId,
        world: WorldId,
        command_id: &str,
    ) -> Result<Option<CommandRecord>, PersistError>;

    fn submit_command(
        &self,
        universe: UniverseId,
        world: WorldId,
        ingress: CommandIngress,
        initial_record: CommandRecord,
    ) -> Result<CommandRecord, PersistError>;

    fn update_command_record(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: CommandRecord,
    ) -> Result<(), PersistError>;
}

pub trait BaseNodeStore: WorldStore + NodeCatalog + WorldIngressStore + CommandStore {}

impl<T> BaseNodeStore for T where T: WorldStore + NodeCatalog + WorldIngressStore + CommandStore {}
