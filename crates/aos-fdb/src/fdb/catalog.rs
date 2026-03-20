use super::*;

impl NodeCatalog for FdbWorldPersistence {
    fn world_runtime_info(
        &self,
        universe: UniverseId,
        world: WorldId,
        now_ns: u64,
    ) -> Result<WorldRuntimeInfo, PersistError> {
        FdbWorldPersistence::world_runtime_info(self, universe, world, now_ns)
    }

    fn world_runtime_info_by_handle(
        &self,
        universe: UniverseId,
        handle: &str,
        now_ns: u64,
    ) -> Result<WorldRuntimeInfo, PersistError> {
        FdbWorldPersistence::world_runtime_info_by_handle(self, universe, handle, now_ns)
    }

    fn list_worlds(
        &self,
        universe: UniverseId,
        now_ns: u64,
        after: Option<WorldId>,
        limit: u32,
    ) -> Result<Vec<WorldRuntimeInfo>, PersistError> {
        FdbWorldPersistence::list_worlds(self, universe, now_ns, after, limit)
    }

    fn set_world_placement_pin(
        &self,
        universe: UniverseId,
        world: WorldId,
        placement_pin: Option<String>,
    ) -> Result<(), PersistError> {
        FdbWorldPersistence::set_world_placement_pin(self, universe, world, placement_pin)
    }

    fn set_world_admin_lifecycle(
        &self,
        universe: UniverseId,
        world: WorldId,
        admin: WorldAdminLifecycle,
    ) -> Result<(), PersistError> {
        FdbWorldPersistence::set_world_admin_lifecycle(self, universe, world, admin)
    }
}

impl WorldIngressStore for FdbWorldPersistence {
    fn enqueue_ingress(
        &self,
        universe: UniverseId,
        world: WorldId,
        item: InboxItem,
    ) -> Result<InboxSeq, PersistError> {
        FdbWorldPersistence::enqueue_ingress(self, universe, world, item)
    }
}

impl CommandStore for FdbWorldPersistence {
    fn command_record(
        &self,
        universe: UniverseId,
        world: WorldId,
        command_id: &str,
    ) -> Result<Option<CommandRecord>, PersistError> {
        FdbWorldPersistence::command_record(self, universe, world, command_id)
    }

    fn submit_command(
        &self,
        universe: UniverseId,
        world: WorldId,
        ingress: CommandIngress,
        initial_record: CommandRecord,
    ) -> Result<CommandRecord, PersistError> {
        FdbWorldPersistence::submit_command(self, universe, world, ingress, initial_record)
    }

    fn update_command_record(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: CommandRecord,
    ) -> Result<(), PersistError> {
        FdbWorldPersistence::update_command_record(self, universe, world, record)
    }
}
