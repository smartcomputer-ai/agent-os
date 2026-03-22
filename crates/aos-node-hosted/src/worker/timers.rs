use std::collections::BTreeMap;

use aos_node::WorldId;
use aos_runtime::timer::TimerScheduler;

#[derive(Default)]
pub(super) struct WorldTimerState {
    pub scheduler: TimerScheduler,
    pub rehydrated: bool,
}

#[derive(Default)]
pub(super) struct PartitionTimerState {
    by_world: BTreeMap<WorldId, WorldTimerState>,
}

impl PartitionTimerState {
    pub fn retain_worlds(&mut self, active_world_ids: &[WorldId]) {
        self.by_world
            .retain(|world_id, _| active_world_ids.contains(world_id));
    }

    pub fn world_mut(&mut self, world_id: WorldId) -> &mut WorldTimerState {
        self.by_world.entry(world_id).or_default()
    }

    pub fn reset_world(&mut self, world_id: WorldId) {
        self.by_world.remove(&world_id);
    }
}
