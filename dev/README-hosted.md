# Hosted Local Dev

This directory contains the local Docker environment for the Kafka/blobstore node runtime.

## Services

- Redpanda Kafka broker on `localhost:19092`
- Redpanda Console on `http://localhost:8080`
- MinIO S3-compatible API on `http://localhost:19000`
- MinIO Console on `http://localhost:19001`

## Start

```bash
dev/hosted/hosted-up.sh
```

## Stop

```bash
dev/hosted/hosted-down.sh
```

To also remove volumes:

```bash
dev/hosted/hosted-down.sh -v
```

## Reset Kafka Topics

```bash
dev/hosted/hosted-topics-reset.sh
```

This deletes and recreates:

- `aos-ingress`
- `aos-journal`
- `aos-route`

By default `aos-ingress` and `aos-journal` are created with `AOS_PARTITION_COUNT=1` for local dev.

`dev/hosted/hosted-topics-ensure.sh` also recreates existing topics when their partition count does not match the requested value, so ingress and journal cannot silently drift apart.

## Reset Blobstore Prefix

```bash
dev/hosted/hosted-blobstore-reset.sh
```

This removes all objects under the configured `AOS_BLOBSTORE_PREFIX` and leaves the bucket in place.

## Runtime Environment

Export these before running `aos node up --journal-backend kafka --blob-backend object-store`
against the local stack:

```bash
export AOS_KAFKA_BOOTSTRAP_SERVERS=localhost:19092
export AOS_KAFKA_INGRESS_TOPIC=aos-ingress
export AOS_KAFKA_JOURNAL_TOPIC=aos-journal
export AOS_KAFKA_ROUTE_TOPIC=aos-route

export AOS_BLOBSTORE_BUCKET=aos-dev
export AOS_BLOBSTORE_ENDPOINT=http://localhost:19000
export AOS_BLOBSTORE_REGION=us-east-1
export AOS_BLOBSTORE_PREFIX=aos
export AOS_BLOBSTORE_FORCE_PATH_STYLE=true

export AWS_ACCESS_KEY_ID=minioadmin
export AWS_SECRET_ACCESS_KEY=minioadmin
```

Optional local tuning:

```bash
export AOS_PARTITION_COUNT=1
export MINIO_ROOT_USER=minioadmin
export MINIO_ROOT_PASSWORD=minioadmin
```
