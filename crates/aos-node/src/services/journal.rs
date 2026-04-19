use std::sync::{Arc, Mutex};

use aos_node::{WorldId, WorldJournalCursor, WorldLogFrame};

use crate::kafka::{HostedKafkaBackend, KafkaConfig};
use crate::worker::WorkerError;

#[derive(Clone)]
pub struct HostedJournalService {
    refresh: Arc<dyn Fn() -> Result<(), WorkerError> + Send + Sync + 'static>,
    world_frames:
        Arc<dyn Fn(WorldId) -> Result<Vec<WorldLogFrame>, WorkerError> + Send + Sync + 'static>,
    world_tail_frames: Arc<
        dyn Fn(WorldId, u64, Option<WorldJournalCursor>) -> Result<Vec<WorldLogFrame>, WorkerError>
            + Send
            + Sync
            + 'static,
    >,
}

impl HostedJournalService {
    pub fn new(partition_count: u32, kafka_config: KafkaConfig) -> Result<Self, WorkerError> {
        let kafka: Arc<Mutex<HostedKafkaBackend>> = Arc::new(Mutex::new(HostedKafkaBackend::new(
            partition_count,
            kafka_config,
        )?));
        Ok(Self::from_callbacks(
            {
                let kafka = Arc::clone(&kafka);
                move || {
                    kafka
                        .lock()
                        .map_err(|_| WorkerError::RuntimePoisoned)?
                        .recover_from_broker()
                        .map_err(WorkerError::from)
                }
            },
            {
                let kafka = Arc::clone(&kafka);
                move |world_id| {
                    Ok(kafka
                        .lock()
                        .map_err(|_| WorkerError::RuntimePoisoned)?
                        .world_frames(world_id)
                        .to_vec())
                }
            },
            {
                let kafka = Arc::clone(&kafka);
                move |world_id, after_world_seq, cursor| {
                    Ok(kafka
                        .lock()
                        .map_err(|_| WorkerError::RuntimePoisoned)?
                        .world_tail_frames(world_id, after_world_seq, cursor.as_ref()))
                }
            },
        ))
    }

    pub(crate) fn from_callbacks<RF, WFF, WTFF>(
        refresh: RF,
        world_frames: WFF,
        world_tail_frames: WTFF,
    ) -> Self
    where
        RF: Fn() -> Result<(), WorkerError> + Send + Sync + 'static,
        WFF: Fn(WorldId) -> Result<Vec<WorldLogFrame>, WorkerError> + Send + Sync + 'static,
        WTFF: Fn(WorldId, u64, Option<WorldJournalCursor>) -> Result<Vec<WorldLogFrame>, WorkerError>
            + Send
            + Sync
            + 'static,
    {
        Self {
            refresh: Arc::new(refresh),
            world_frames: Arc::new(world_frames),
            world_tail_frames: Arc::new(world_tail_frames),
        }
    }

    pub fn refresh(&self) -> Result<(), WorkerError> {
        (self.refresh)()
    }

    pub fn world_frames(&self, world_id: WorldId) -> Result<Vec<WorldLogFrame>, WorkerError> {
        (self.world_frames)(world_id)
    }

    pub fn world_tail_frames(
        &self,
        world_id: WorldId,
        after_world_seq: u64,
        cursor: Option<WorldJournalCursor>,
    ) -> Result<Vec<WorldLogFrame>, WorkerError> {
        (self.world_tail_frames)(world_id, after_world_seq, cursor)
    }
}
