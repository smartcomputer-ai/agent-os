use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::time::{Duration, Instant};

use aos_node::PlaneError;
use rdkafka::client::ClientContext;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{BaseConsumer, Consumer, ConsumerContext};
use rdkafka::error::{KafkaError, RDKafkaErrorCode};
use rdkafka::message::Message;
use rdkafka::metadata::Metadata;
use rdkafka::producer::{
    BaseRecord, DeliveryResult, NoCustomPartitioner, Producer, ProducerContext, ThreadedProducer,
};
use rdkafka::topic_partition_list::{Offset, TopicPartitionList};
use rdkafka::util::Timeout;
use uuid::Uuid;

use super::types::{FetchedRecord, KafkaConfig};

type DeliveryReportResult = Result<(i32, i64), KafkaError>;
pub(super) type DeliveryReportRx = Receiver<DeliveryReportResult>;
pub(super) type ConsumerHandle = BaseConsumer<QuietConsumerContext>;

pub(super) enum DeliveryOpaque {
    Ignore,
    Notify(SyncSender<DeliveryReportResult>),
}

#[derive(Clone)]
pub(super) struct DeliveryTrackingProducerContext;

impl ClientContext for DeliveryTrackingProducerContext {}

#[derive(Clone)]
pub(super) struct QuietConsumerContext;

impl ClientContext for QuietConsumerContext {
    fn error(&self, error: KafkaError, reason: &str) {
        if is_partition_eof_error(&error) {
            return;
        }
        tracing::error!(target: "rdkafka::client", error = %error, reason, "librdkafka client error");
    }
}

impl ConsumerContext for QuietConsumerContext {}

fn is_partition_eof_error(error: &KafkaError) -> bool {
    matches!(
        error,
        KafkaError::PartitionEOF(_)
            | KafkaError::Global(RDKafkaErrorCode::PartitionEOF)
            | KafkaError::MessageConsumption(RDKafkaErrorCode::PartitionEOF)
    )
}

impl ProducerContext<NoCustomPartitioner> for DeliveryTrackingProducerContext {
    type DeliveryOpaque = Box<DeliveryOpaque>;

    fn delivery(
        &self,
        delivery_result: &DeliveryResult<'_>,
        delivery_opaque: Self::DeliveryOpaque,
    ) {
        if let DeliveryOpaque::Notify(tx) = *delivery_opaque {
            let result = match delivery_result {
                Ok(message) => Ok((message.partition(), message.offset())),
                Err((err, _message)) => Err(err.clone()),
            };
            let _ = tx.send(result);
        }
    }
}

pub(super) type ProducerHandle = ThreadedProducer<DeliveryTrackingProducerContext>;

pub(super) fn create_producer(
    config: &KafkaConfig,
    transactional: bool,
    partition: Option<u32>,
) -> Result<ProducerHandle, PlaneError> {
    let mut client = ClientConfig::new();
    client
        .set("bootstrap.servers", broker_hosts(config)?)
        .set(
            "message.timeout.ms",
            config.producer_message_timeout_ms.to_string(),
        )
        .set("request.required.acks", "all");
    if transactional {
        let suffix = partition
            .map(|partition| format!("-p{partition}"))
            .unwrap_or_default();
        client.set("enable.idempotence", "true").set(
            "transactional.id",
            format!("{}-journal{suffix}", config.transactional_id),
        );
    }
    client
        .create_with_context(DeliveryTrackingProducerContext)
        .map_err(kafka_backend_err(if transactional {
            "create transactional Kafka producer"
        } else {
            "create Kafka producer"
        }))
}

pub(super) fn create_group_consumer(config: &KafkaConfig) -> Result<ConsumerHandle, PlaneError> {
    let consumer: ConsumerHandle = ClientConfig::new()
        .set("bootstrap.servers", broker_hosts(config)?)
        .set("group.id", config.submission_group_prefix.clone())
        .set("enable.auto.commit", "false")
        .set("enable.auto.offset.store", "false")
        // Long-lived hosted consumers should idle quietly when a partition is empty.
        .set("enable.partition.eof", "false")
        .set("auto.offset.reset", "earliest")
        .set("isolation.level", "read_committed")
        .set(
            "session.timeout.ms",
            config.group_session_timeout_ms.to_string(),
        )
        .set(
            "heartbeat.interval.ms",
            config.group_heartbeat_interval_ms.to_string(),
        )
        .create_with_context(QuietConsumerContext)
        .map_err(kafka_backend_err("create Kafka consumer group member"))?;
    consumer
        .subscribe(&[&config.ingress_topic])
        .map_err(kafka_backend_err("subscribe Kafka consumer group member"))?;
    Ok(consumer)
}

pub(super) fn create_direct_consumer(
    config: &KafkaConfig,
    partitions: &std::collections::BTreeSet<u32>,
) -> Result<ConsumerHandle, PlaneError> {
    let consumer: ConsumerHandle = ClientConfig::new()
        .set("bootstrap.servers", broker_hosts(config)?)
        .set(
            "group.id",
            format!(
                "{}-direct-{}",
                config.submission_group_prefix,
                Uuid::new_v4()
            ),
        )
        .set("enable.auto.commit", "false")
        .set("enable.auto.offset.store", "false")
        // Direct-assigned long-lived workers should also idle quietly.
        .set("enable.partition.eof", "false")
        .set("auto.offset.reset", "earliest")
        .set("isolation.level", "read_committed")
        .create_with_context(QuietConsumerContext)
        .map_err(kafka_backend_err("create Kafka direct consumer"))?;
    let mut assignment = TopicPartitionList::new();
    let start_offset = if config.direct_assignment_start_from_end {
        Offset::End
    } else {
        Offset::Beginning
    };
    for partition in partitions {
        assignment
            .add_partition_offset(&config.ingress_topic, *partition as i32, start_offset)
            .map_err(kafka_backend_err("assign Kafka direct consumer partition"))?;
    }
    consumer
        .assign(&assignment)
        .map_err(kafka_backend_err("assign Kafka direct consumer"))?;
    Ok(consumer)
}

pub(super) fn topic_partitions(
    config: &KafkaConfig,
    producer: &ProducerHandle,
    topic: &str,
) -> Result<Vec<i32>, PlaneError> {
    let metadata = producer
        .client()
        .fetch_metadata(
            Some(topic),
            Timeout::After(Duration::from_millis(u64::from(config.metadata_timeout_ms))),
        )
        .map_err(kafka_backend_err(format!(
            "fetch metadata for topic {topic}"
        )))?;
    Ok(partitions_from_metadata(&metadata, topic))
}

pub(crate) fn fetch_partition_records(
    config: &KafkaConfig,
    topic: &str,
    partition: i32,
    start_offset: Option<i64>,
    read_committed: bool,
) -> Result<Vec<FetchedRecord>, PlaneError> {
    let consumer: ConsumerHandle = ClientConfig::new()
        .set("bootstrap.servers", broker_hosts(config)?)
        .set(
            "group.id",
            format!(
                "{}-recovery-{topic}-{partition}-{}",
                config.submission_group_prefix,
                Uuid::new_v4()
            ),
        )
        .set("enable.partition.eof", "true")
        .set("enable.auto.commit", "false")
        .set("auto.offset.reset", "earliest")
        .set(
            "fetch.wait.max.ms",
            config.recovery_fetch_wait_ms.to_string(),
        )
        .set(
            "isolation.level",
            if read_committed {
                "read_committed"
            } else {
                "read_uncommitted"
            },
        )
        .create_with_context(QuietConsumerContext)
        .map_err(kafka_backend_err(format!(
            "create Kafka recovery consumer for {topic}[{partition}]"
        )))?;

    let (_low_watermark, high_watermark) = match consumer.fetch_watermarks(
        topic,
        partition,
        Timeout::After(Duration::from_millis(u64::from(config.metadata_timeout_ms))),
    ) {
        Ok(watermarks) => watermarks,
        Err(KafkaError::MetadataFetch(RDKafkaErrorCode::UnknownTopicOrPartition))
        | Err(KafkaError::MessageConsumption(RDKafkaErrorCode::UnknownTopicOrPartition)) => {
            return Ok(Vec::new());
        }
        Err(err) => {
            return Err(kafka_backend_err(format!(
                "fetch watermarks for Kafka recovery consumer {topic}[{partition}]"
            ))(err));
        }
    };

    // Recovery readers should replay a stable snapshot of the partition as it existed when the
    // read started. If the caller is already at or beyond that cut, return immediately instead of
    // waiting for an EOF/idle timeout.
    if start_offset.is_some_and(|offset| offset >= high_watermark) {
        return Ok(Vec::new());
    }

    let mut assignment = TopicPartitionList::new();
    assignment
        .add_partition_offset(
            topic,
            partition,
            start_offset
                .map(Offset::Offset)
                .unwrap_or(Offset::Beginning),
        )
        .map_err(kafka_backend_err(format!(
            "assign recovery offset for {topic}[{partition}]"
        )))?;
    consumer
        .assign(&assignment)
        .map_err(kafka_backend_err(format!(
            "assign Kafka recovery consumer for {topic}[{partition}]"
        )))?;

    let mut records = Vec::new();
    let mut last_progress = Instant::now();
    loop {
        match consumer.poll(Duration::from_millis(u64::from(
            config.recovery_poll_interval_ms,
        ))) {
            Some(Ok(message)) => {
                last_progress = Instant::now();
                let offset = message.offset();
                records.push(FetchedRecord {
                    offset,
                    key: message.key().map(|key| key.to_vec()),
                    value: message.payload().map(|payload| payload.to_vec()),
                });
                if offset.saturating_add(1) >= high_watermark {
                    break;
                }
            }
            Some(Err(KafkaError::PartitionEOF(_))) => break,
            Some(Err(KafkaError::MessageConsumption(
                RDKafkaErrorCode::UnknownTopicOrPartition,
            ))) => break,
            Some(Err(err)) => {
                return Err(kafka_backend_err(format!(
                    "poll Kafka recovery consumer for {topic}[{partition}]"
                ))(err));
            }
            None => {
                if last_progress.elapsed()
                    >= Duration::from_millis(u64::from(config.recovery_idle_timeout_ms))
                {
                    break;
                }
            }
        }
    }
    Ok(records)
}

fn delivery_timeout(config: &KafkaConfig) -> Duration {
    Duration::from_millis(u64::from(
        config
            .producer_message_timeout_ms
            .max(config.transaction_timeout_ms),
    ))
}

fn send_record_with_opaque<'a>(
    config: &KafkaConfig,
    producer: &ProducerHandle,
    topic: &'a str,
    partition: i32,
    key: &'a [u8],
    payload: Option<&'a [u8]>,
    delivery_opaque: Box<DeliveryOpaque>,
    label: &str,
) -> Result<(), PlaneError> {
    let mut record = BaseRecord::with_opaque_to(topic, delivery_opaque)
        .partition(partition)
        .key(key);
    if let Some(payload) = payload {
        record = record.payload(payload);
    }
    loop {
        match producer.send(record) {
            Ok(()) => return Ok(()),
            Err((KafkaError::MessageProduction(RDKafkaErrorCode::QueueFull), owned_record)) => {
                flush_producer(config, producer, format!("{label}: flush producer queue"))?;
                record = owned_record;
            }
            Err((err, _)) => {
                return Err(kafka_backend_err(label.to_owned())(err));
            }
        }
    }
}

pub(super) fn send_record(
    config: &KafkaConfig,
    producer: &ProducerHandle,
    topic: &str,
    partition: i32,
    key: &[u8],
    payload: &[u8],
    label: &str,
) -> Result<(), PlaneError> {
    send_record_with_opaque(
        config,
        producer,
        topic,
        partition,
        key,
        Some(payload),
        Box::new(DeliveryOpaque::Ignore),
        label,
    )
}

pub(super) fn send_record_with_delivery(
    config: &KafkaConfig,
    producer: &ProducerHandle,
    topic: &str,
    partition: i32,
    key: &[u8],
    payload: &[u8],
    label: &str,
) -> Result<DeliveryReportRx, PlaneError> {
    let (tx, rx) = sync_channel(1);
    send_record_with_opaque(
        config,
        producer,
        topic,
        partition,
        key,
        Some(payload),
        Box::new(DeliveryOpaque::Notify(tx)),
        label,
    )?;
    Ok(rx)
}

pub(super) fn send_tombstone_with_delivery(
    config: &KafkaConfig,
    producer: &ProducerHandle,
    topic: &str,
    partition: i32,
    key: &[u8],
    label: &str,
) -> Result<DeliveryReportRx, PlaneError> {
    let (tx, rx) = sync_channel(1);
    send_record_with_opaque(
        config,
        producer,
        topic,
        partition,
        key,
        None,
        Box::new(DeliveryOpaque::Notify(tx)),
        label,
    )?;
    Ok(rx)
}

pub(super) fn await_delivery(
    config: &KafkaConfig,
    rx: DeliveryReportRx,
    label: &str,
) -> Result<(i32, i64), PlaneError> {
    match rx.recv_timeout(delivery_timeout(config)) {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(err)) => Err(kafka_backend_err(label.to_owned())(err)),
        Err(RecvTimeoutError::Timeout) => {
            Err(PlaneError::Persist(aos_node::PersistError::backend(
                format!("{label}: timed out waiting for Kafka delivery report"),
            )))
        }
        Err(RecvTimeoutError::Disconnected) => {
            Err(PlaneError::Persist(aos_node::PersistError::backend(
                format!("{label}: Kafka delivery report channel disconnected"),
            )))
        }
    }
}

pub(super) fn flush_producer(
    config: &KafkaConfig,
    producer: &ProducerHandle,
    label: impl Into<String>,
) -> Result<(), PlaneError> {
    producer
        .flush(Timeout::After(Duration::from_millis(u64::from(
            config.producer_flush_timeout_ms,
        ))))
        .map_err(kafka_backend_err(label.into()))
}

fn broker_hosts(config: &KafkaConfig) -> Result<String, PlaneError> {
    config
        .bootstrap_servers
        .clone()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            PlaneError::Persist(aos_node::PersistError::validation(
                "AOS_KAFKA_BOOTSTRAP_SERVERS must be set for broker-backed Kafka planes",
            ))
        })
}

fn partitions_from_metadata(metadata: &Metadata, topic: &str) -> Vec<i32> {
    metadata
        .topics()
        .iter()
        .find(|item| item.name() == topic)
        .map(|item| {
            item.partitions()
                .iter()
                .map(|partition| partition.id())
                .collect()
        })
        .unwrap_or_default()
}

fn kafka_backend_err<T: Into<String>>(label: T) -> impl FnOnce(KafkaError) -> PlaneError {
    let label = label.into();
    move |err| PlaneError::Persist(aos_node::PersistError::backend(format!("{label}: {err}")))
}
