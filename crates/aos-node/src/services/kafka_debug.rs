use std::sync::Arc;

use crate::kafka::PartitionLogEntry;
use crate::worker::{HostedWorkerRuntime, WorkerError};

#[derive(Clone)]
pub struct KafkaDebugService {
    partition_count: Arc<dyn Fn() -> Result<u32, WorkerError> + Send + Sync + 'static>,
    journal_topic: Arc<dyn Fn() -> Result<String, WorkerError> + Send + Sync + 'static>,
    partition_entries:
        Arc<dyn Fn(u32) -> Result<Vec<PartitionLogEntry>, WorkerError> + Send + Sync + 'static>,
    recover_partition: Arc<dyn Fn(u32) -> Result<(), WorkerError> + Send + Sync + 'static>,
}

impl KafkaDebugService {
    pub fn from_runtime(runtime: HostedWorkerRuntime) -> Self {
        Self::from_callbacks(
            {
                let runtime = runtime.clone();
                move || runtime.partition_count()
            },
            {
                let runtime = runtime.clone();
                move || runtime.journal_topic()
            },
            {
                let runtime = runtime.clone();
                move |partition| runtime.partition_entries(partition)
            },
            move |partition| runtime.recover_partition(partition),
        )
    }

    pub(crate) fn from_callbacks<PCF, JTF, PEF, RPF>(
        partition_count: PCF,
        journal_topic: JTF,
        partition_entries: PEF,
        recover_partition: RPF,
    ) -> Self
    where
        PCF: Fn() -> Result<u32, WorkerError> + Send + Sync + 'static,
        JTF: Fn() -> Result<String, WorkerError> + Send + Sync + 'static,
        PEF: Fn(u32) -> Result<Vec<PartitionLogEntry>, WorkerError> + Send + Sync + 'static,
        RPF: Fn(u32) -> Result<(), WorkerError> + Send + Sync + 'static,
    {
        Self {
            partition_count: Arc::new(partition_count),
            journal_topic: Arc::new(journal_topic),
            partition_entries: Arc::new(partition_entries),
            recover_partition: Arc::new(recover_partition),
        }
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

    pub fn recover_partition(&self, partition: u32) -> Result<(), WorkerError> {
        (self.recover_partition)(partition)
    }
}
