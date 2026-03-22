use std::sync::{Arc, Mutex};

use aos_node::{SubmissionEnvelope, WorldId, WorldLogFrame};

use crate::kafka::{HostedKafkaBackend, KafkaConfig, PartitionLogEntry};
use crate::worker::WorkerError;

#[derive(Clone)]
pub struct HostedJournalService {
    refresh: Arc<dyn Fn() -> Result<(), WorkerError> + Send + Sync + 'static>,
    partition_count: Arc<dyn Fn() -> Result<u32, WorkerError> + Send + Sync + 'static>,
    journal_topic: Arc<dyn Fn() -> Result<String, WorkerError> + Send + Sync + 'static>,
    partition_entries:
        Arc<dyn Fn(u32) -> Result<Vec<PartitionLogEntry>, WorkerError> + Send + Sync + 'static>,
    world_frames:
        Arc<dyn Fn(WorldId) -> Result<Vec<WorldLogFrame>, WorkerError> + Send + Sync + 'static>,
    submit: Arc<dyn Fn(SubmissionEnvelope) -> Result<u64, WorkerError> + Send + Sync + 'static>,
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
                move || {
                    Ok(kafka
                        .lock()
                        .map_err(|_| WorkerError::RuntimePoisoned)?
                        .partition_count())
                }
            },
            {
                let kafka = Arc::clone(&kafka);
                move || {
                    Ok(kafka
                        .lock()
                        .map_err(|_| WorkerError::RuntimePoisoned)?
                        .config()
                        .journal_topic
                        .clone())
                }
            },
            {
                let kafka = Arc::clone(&kafka);
                move |partition| {
                    let kafka = kafka.lock().map_err(|_| WorkerError::RuntimePoisoned)?;
                    let topic = kafka.config().journal_topic.clone();
                    Ok(kafka.partition_entries(&topic, partition).to_vec())
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
                move |submission| {
                    kafka
                        .lock()
                        .map_err(|_| WorkerError::RuntimePoisoned)?
                        .submit(submission)
                        .map_err(WorkerError::from)
                }
            },
        ))
    }

    pub(crate) fn from_callbacks<RF, PCF, JTF, PEF, WFF, SF>(
        refresh: RF,
        partition_count: PCF,
        journal_topic: JTF,
        partition_entries: PEF,
        world_frames: WFF,
        submit: SF,
    ) -> Self
    where
        RF: Fn() -> Result<(), WorkerError> + Send + Sync + 'static,
        PCF: Fn() -> Result<u32, WorkerError> + Send + Sync + 'static,
        JTF: Fn() -> Result<String, WorkerError> + Send + Sync + 'static,
        PEF: Fn(u32) -> Result<Vec<PartitionLogEntry>, WorkerError> + Send + Sync + 'static,
        WFF: Fn(WorldId) -> Result<Vec<WorldLogFrame>, WorkerError> + Send + Sync + 'static,
        SF: Fn(SubmissionEnvelope) -> Result<u64, WorkerError> + Send + Sync + 'static,
    {
        Self {
            refresh: Arc::new(refresh),
            partition_count: Arc::new(partition_count),
            journal_topic: Arc::new(journal_topic),
            partition_entries: Arc::new(partition_entries),
            world_frames: Arc::new(world_frames),
            submit: Arc::new(submit),
        }
    }

    pub fn refresh(&self) -> Result<(), WorkerError> {
        (self.refresh)()
    }

    pub fn partition_count(&self) -> Result<u32, WorkerError> {
        (self.partition_count)()
    }

    pub fn journal_topic(&self) -> Result<String, WorkerError> {
        (self.journal_topic)()
    }

    pub fn partition_entries(&self, partition: u32) -> Result<Vec<PartitionLogEntry>, WorkerError> {
        (self.partition_entries)(partition)
    }

    pub fn world_frames(&self, world_id: WorldId) -> Result<Vec<WorldLogFrame>, WorkerError> {
        (self.world_frames)(world_id)
    }

    pub fn submit(&self, submission: SubmissionEnvelope) -> Result<u64, WorkerError> {
        (self.submit)(submission)
    }
}
