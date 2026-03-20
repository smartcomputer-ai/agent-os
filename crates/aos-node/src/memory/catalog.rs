use super::*;

impl NodeCatalog for MemoryWorldPersistence {
    fn world_runtime_info(
        &self,
        universe: UniverseId,
        world: WorldId,
        now_ns: u64,
    ) -> Result<WorldRuntimeInfo, PersistError> {
        MemoryWorldPersistence::world_runtime_info(self, universe, world, now_ns)
    }

    fn world_runtime_info_by_handle(
        &self,
        universe: UniverseId,
        handle: &str,
        now_ns: u64,
    ) -> Result<WorldRuntimeInfo, PersistError> {
        MemoryWorldPersistence::world_runtime_info_by_handle(self, universe, handle, now_ns)
    }

    fn list_worlds(
        &self,
        universe: UniverseId,
        now_ns: u64,
        after: Option<WorldId>,
        limit: u32,
    ) -> Result<Vec<WorldRuntimeInfo>, PersistError> {
        MemoryWorldPersistence::list_worlds(self, universe, now_ns, after, limit)
    }

    fn set_world_placement_pin(
        &self,
        universe: UniverseId,
        world: WorldId,
        placement_pin: Option<String>,
    ) -> Result<(), PersistError> {
        MemoryWorldPersistence::set_world_placement_pin(self, universe, world, placement_pin)
    }

    fn set_world_admin_lifecycle(
        &self,
        universe: UniverseId,
        world: WorldId,
        admin: WorldAdminLifecycle,
    ) -> Result<(), PersistError> {
        MemoryWorldPersistence::set_world_admin_lifecycle(self, universe, world, admin)
    }
}

impl WorldIngressStore for MemoryWorldPersistence {
    fn enqueue_ingress(
        &self,
        universe: UniverseId,
        world: WorldId,
        item: InboxItem,
    ) -> Result<InboxSeq, PersistError> {
        MemoryWorldPersistence::enqueue_ingress(self, universe, world, item)
    }
}

impl CommandStore for MemoryWorldPersistence {
    fn command_record(
        &self,
        universe: UniverseId,
        world: WorldId,
        command_id: &str,
    ) -> Result<Option<CommandRecord>, PersistError> {
        MemoryWorldPersistence::command_record(self, universe, world, command_id)
    }

    fn submit_command(
        &self,
        universe: UniverseId,
        world: WorldId,
        ingress: CommandIngress,
        initial_record: CommandRecord,
    ) -> Result<CommandRecord, PersistError> {
        MemoryWorldPersistence::submit_command(self, universe, world, ingress, initial_record)
    }

    fn update_command_record(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: CommandRecord,
    ) -> Result<(), PersistError> {
        MemoryWorldPersistence::update_command_record(self, universe, world, record)
    }
}
