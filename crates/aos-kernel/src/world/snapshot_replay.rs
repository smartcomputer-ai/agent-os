use super::*;
use std::time::Instant;

const REPLAY_BATCH_SIZE: usize = 8192;
const SLOW_REPLAY_LOG_THRESHOLD_MS: u128 = 1_000;

impl<S: Store + 'static> Kernel<S> {
    pub fn create_snapshot(&mut self) -> Result<(), KernelError> {
        self.tick_until_idle()?;
        if !self.workflow_queue.is_empty() {
            return Err(KernelError::SnapshotUnavailable(
                "workflow queue must be idle before snapshot".into(),
            ));
        }
        self.materialize_cell_delta_roots_for_snapshot()?;
        let height = self.journal.next_seq();
        let workflow_state: Vec<WorkflowStateEntry> = Vec::new();
        let recent_receipts: Vec<[u8; 32]> = self.recent_receipts.iter().cloned().collect();
        let queued_effects = self.snapshot_queued_effects();
        let workflow_index_roots = self
            .workflow_index_roots
            .iter()
            .map(|(name, hash)| (name.clone(), *hash.as_bytes()))
            .collect();
        let pending_workflow_receipts = self
            .pending_workflow_receipts
            .iter()
            .map(|(hash, ctx)| WorkflowReceiptSnapshot::from_context(*hash, ctx))
            .collect();
        let workflow_instances = self
            .workflow_instances
            .iter()
            .map(|(instance_id, state)| WorkflowInstanceSnapshot {
                instance_id: instance_id.clone(),
                state_bytes: state.state_bytes.clone(),
                inflight_intents: state
                    .inflight_intents
                    .iter()
                    .map(|(intent_id, meta)| WorkflowInflightIntentSnapshot {
                        intent_id: *intent_id,
                        origin_module_id: meta.origin_module_id.clone(),
                        origin_instance_key: meta.origin_instance_key.clone(),
                        effect_kind: meta.effect_kind.clone(),
                        params_hash: meta.params_hash.clone(),
                        emitted_at_seq: meta.emitted_at_seq,
                        last_stream_seq: meta.last_stream_seq,
                    })
                    .collect(),
                status: match state.status {
                    WorkflowRuntimeStatus::Running => WorkflowStatusSnapshot::Running,
                    WorkflowRuntimeStatus::Waiting => WorkflowStatusSnapshot::Waiting,
                    WorkflowRuntimeStatus::Completed => WorkflowStatusSnapshot::Completed,
                    WorkflowRuntimeStatus::Failed => WorkflowStatusSnapshot::Failed,
                },
                last_processed_event_seq: state.last_processed_event_seq,
                module_version: state.module_version.clone(),
            })
            .collect();
        let logical_now_ns = self.effect_manager.logical_now_ns();
        let mut snapshot = KernelSnapshot::new(
            height,
            workflow_state,
            recent_receipts,
            queued_effects,
            pending_workflow_receipts,
            workflow_instances,
            logical_now_ns,
            Some(*self.manifest_hash.as_bytes()),
        );
        snapshot.set_workflow_index_roots(workflow_index_roots);
        let root_completeness = SnapshotRootCompleteness {
            manifest_hash: Some(self.manifest_hash.as_bytes().to_vec()),
            workflow_state_roots: snapshot
                .workflow_state_entries()
                .iter()
                .map(|entry| entry.state_hash)
                .collect(),
            cell_index_roots: snapshot
                .workflow_index_roots()
                .iter()
                .map(|(_, root)| *root)
                .collect(),
            workspace_roots: self
                .workspace_roots
                .iter()
                .map(|hash| *hash.as_bytes())
                .collect(),
            pinned_roots: self
                .pinned_roots
                .iter()
                .map(|hash| *hash.as_bytes())
                .collect(),
        };
        snapshot.set_root_completeness(root_completeness);
        self.validate_snapshot_root_completeness(&snapshot)?;
        let bytes = serde_cbor::to_vec(&snapshot)
            .map_err(|err| KernelError::SnapshotDecode(err.to_string()))?;
        let hash = self.store.put_blob(&bytes)?;
        let baseline = SnapshotRecord {
            snapshot_ref: hash.to_hex(),
            height,
            universe_id: self.universe_id,
            logical_time_ns: logical_now_ns,
            receipt_horizon_height: self.receipt_horizon_height_for_baseline(height),
            manifest_hash: Some(self.manifest_hash.to_hex()),
        };
        self.append_record(JournalRecord::Snapshot(baseline.clone()))?;
        self.snapshot_index
            .insert(height, (hash, Some(self.manifest_hash)));
        if self.validate_baseline_promotion(&baseline).is_ok() {
            self.active_baseline = Some(baseline);
        }
        self.last_snapshot_hash = Some(hash);
        self.last_snapshot_height = Some(height);
        Ok(())
    }

    pub(super) fn replay_existing_entries(&mut self) -> Result<(), KernelError> {
        let head = self.journal.next_seq();
        if head == 0 {
            return Ok(());
        }
        let mut resume_seq: Option<JournalSeq> = None;
        let mut latest_replay_snapshot: Option<SnapshotRecord> = None;
        let mut latest_promotable_baseline: Option<SnapshotRecord> = None;
        let mut scan_cursor = 0;
        while scan_cursor < head {
            let entries = self
                .journal
                .load_batch_from(scan_cursor, REPLAY_BATCH_SIZE)?;
            if entries.is_empty() {
                break;
            }
            for entry in &entries {
                if matches!(entry.kind, JournalKind::Snapshot) {
                    let record: JournalRecord = serde_cbor::from_slice(&entry.payload)
                        .map_err(|err| KernelError::Journal(err.to_string()))?;
                    if let JournalRecord::Snapshot(snapshot) = record {
                        if let Ok(hash) = Hash::from_hex_str(&snapshot.snapshot_ref) {
                            let manifest_hash = snapshot
                                .manifest_hash
                                .as_ref()
                                .and_then(|s| Hash::from_hex_str(s).ok());
                            self.snapshot_index
                                .insert(snapshot.height, (hash, manifest_hash));
                        }
                        latest_replay_snapshot = Some(snapshot.clone());
                        if self.validate_baseline_promotion(&snapshot).is_ok() {
                            latest_promotable_baseline = Some(snapshot.clone());
                        }
                    }
                }
            }
            scan_cursor = entries
                .last()
                .map(|entry| entry.seq.saturating_add(1))
                .unwrap_or(head);
        }
        if let Some(snapshot) = latest_promotable_baseline {
            self.universe_id = snapshot.universe_id;
            self.active_baseline = Some(snapshot);
        }
        if let Some(snapshot) = latest_replay_snapshot {
            self.universe_id = snapshot.universe_id;
            resume_seq = Some(snapshot.height);
            self.load_snapshot_for_replay(&snapshot)?;
        }
        self.suppress_journal = true;
        self.replay_applying_domain_record = false;
        self.replay_generated_domain_event_hashes.clear();
        self.replay_metrics = Some(ReplayMetrics::default());
        let mut replay_cursor = resume_seq.unwrap_or(0);
        while replay_cursor < head {
            let entries = self
                .journal
                .load_batch_from(replay_cursor, REPLAY_BATCH_SIZE)?;
            if entries.is_empty() {
                break;
            }
            for entry in entries {
                if resume_seq.is_some_and(|seq| entry.seq < seq) {
                    continue;
                }
                let record: JournalRecord = serde_cbor::from_slice(&entry.payload)
                    .map_err(|err| KernelError::Journal(err.to_string()))?;
                replay_cursor = entry.seq.saturating_add(1);
                self.apply_replay_record(record)?;
            }
        }
        self.flush_replay_cell_projection_deltas()?;
        self.tick_until_idle()?;
        self.suppress_journal = false;
        self.replay_applying_domain_record = false;
        self.replay_generated_domain_event_hashes.clear();
        if let Some(metrics) = self.replay_metrics.take() {
            log::info!(
                "kernel replay_existing_entries completed: domain_events={} workflow_invocations={} workflow_invoke_ms={} state_cache_hits={} state_cache_misses={} state_load_ms={}",
                metrics.domain_events,
                metrics.workflow_invocations,
                metrics.workflow_invoke_ns / 1_000_000,
                metrics.state_cache_hits,
                metrics.state_cache_misses,
                metrics.state_load_ns / 1_000_000
            );
        }
        Ok(())
    }

    pub fn restore_snapshot_record(&mut self, record: &SnapshotRecord) -> Result<(), KernelError> {
        self.load_snapshot(record)
    }

    pub fn promote_active_baseline_record(
        &mut self,
        record: &SnapshotRecord,
    ) -> Result<(), KernelError> {
        self.validate_baseline_promotion(record)?;
        self.validate_snapshot_record_manifest_lineage(record, None)?;
        self.universe_id = record.universe_id;
        self.active_baseline = Some(record.clone());
        Ok(())
    }

    pub fn restore_snapshot_record_for_replay(
        &mut self,
        record: &SnapshotRecord,
    ) -> Result<(), KernelError> {
        self.load_snapshot_for_replay(record)
    }

    pub fn replay_entries_from(&mut self, from: JournalSeq) -> Result<(), KernelError> {
        let apply_started = Instant::now();
        let mut load_ms = 0u128;
        self.suppress_journal = true;
        self.replay_applying_domain_record = false;
        self.replay_generated_domain_event_hashes.clear();
        self.replay_metrics = Some(ReplayMetrics::default());
        let mut cursor = from;
        let head = self.journal.next_seq();
        while cursor < head {
            let load_started = Instant::now();
            let entries = self.journal.load_batch_from(cursor, REPLAY_BATCH_SIZE)?;
            load_ms += load_started.elapsed().as_millis();
            if entries.is_empty() {
                break;
            }
            for entry in entries {
                let decode_started = Instant::now();
                let record: JournalRecord = serde_cbor::from_slice(&entry.payload)
                    .map_err(|err| KernelError::Journal(err.to_string()))?;
                if let Some(metrics) = self.replay_metrics.as_mut() {
                    metrics.journal_records = metrics.journal_records.saturating_add(1);
                    metrics.journal_decode_ns += decode_started.elapsed().as_nanos();
                }
                cursor = entry.seq.saturating_add(1);
                self.apply_replay_record(record)?;
            }
        }
        let flush_started = Instant::now();
        self.flush_replay_cell_projection_deltas()?;
        if let Some(metrics) = self.replay_metrics.as_mut() {
            metrics.flush_projection_ns += flush_started.elapsed().as_nanos();
        }
        let tick_started = Instant::now();
        self.tick_until_idle()?;
        if let Some(metrics) = self.replay_metrics.as_mut() {
            metrics.tick_ns += tick_started.elapsed().as_nanos();
        }
        self.suppress_journal = false;
        self.replay_applying_domain_record = false;
        self.replay_generated_domain_event_hashes.clear();
        let replay_metrics = self.replay_metrics.take();
        let apply_ms = apply_started.elapsed().as_millis();
        let replayed_records = replay_metrics
            .as_ref()
            .map(|metrics| metrics.journal_records)
            .unwrap_or_else(|| head.saturating_sub(from));
        let replay_to = if replayed_records == 0 {
            None
        } else {
            Some(cursor.saturating_sub(1))
        };
        if apply_ms >= SLOW_REPLAY_LOG_THRESHOLD_MS {
            log::info!(
                "kernel replay_entries_from completed: replay_from={} replay_to={:?} replayed_records={} load_ms={} apply_ms={}",
                from,
                replay_to,
                replayed_records,
                load_ms,
                apply_ms
            );
        } else {
            log::debug!(
                "kernel replay_entries_from completed: replay_from={} replay_to={:?} replayed_records={} load_ms={} apply_ms={}",
                from,
                replay_to,
                replayed_records,
                load_ms,
                apply_ms
            );
        }
        if let Some(metrics) = replay_metrics {
            if apply_ms >= SLOW_REPLAY_LOG_THRESHOLD_MS {
                log::debug!(
                    "kernel replay_entries_from metrics: journal_records={} domain_events={} workflow_invocations={} workflow_invoke_ms={} journal_decode_ms={} hydrate_blob_count={} hydrate_blob_bytes={} hydrate_blob_ms={} tick_ms={} flush_projection_ms={} state_cache_hits={} state_cache_misses={} state_load_ms={}",
                    metrics.journal_records,
                    metrics.domain_events,
                    metrics.workflow_invocations,
                    metrics.workflow_invoke_ns / 1_000_000,
                    metrics.journal_decode_ns / 1_000_000,
                    metrics.hydrate_blob_count,
                    metrics.hydrate_blob_bytes,
                    metrics.hydrate_blob_ns / 1_000_000,
                    metrics.tick_ns / 1_000_000,
                    metrics.flush_projection_ns / 1_000_000,
                    metrics.state_cache_hits,
                    metrics.state_cache_misses,
                    metrics.state_load_ns / 1_000_000
                );
            } else {
                log::trace!(
                    "kernel replay_entries_from metrics: journal_records={} domain_events={} workflow_invocations={} workflow_invoke_ms={} journal_decode_ms={} hydrate_blob_count={} hydrate_blob_bytes={} hydrate_blob_ms={} tick_ms={} flush_projection_ms={} state_cache_hits={} state_cache_misses={} state_load_ms={}",
                    metrics.journal_records,
                    metrics.domain_events,
                    metrics.workflow_invocations,
                    metrics.workflow_invoke_ns / 1_000_000,
                    metrics.journal_decode_ns / 1_000_000,
                    metrics.hydrate_blob_count,
                    metrics.hydrate_blob_bytes,
                    metrics.hydrate_blob_ns / 1_000_000,
                    metrics.tick_ns / 1_000_000,
                    metrics.flush_projection_ns / 1_000_000,
                    metrics.state_cache_hits,
                    metrics.state_cache_misses,
                    metrics.state_load_ns / 1_000_000
                );
            }
        }
        Ok(())
    }

    fn apply_replay_record(&mut self, record: JournalRecord) -> Result<(), KernelError> {
        match record {
            JournalRecord::DomainEvent(event) => {
                self.sync_logical_from_record(event.logical_now_ns);
                if self.consume_replay_generated_domain_event(&event.event_hash) {
                    return Ok(());
                }
                let stamp = IngressStamp {
                    now_ns: event.now_ns,
                    logical_now_ns: event.logical_now_ns,
                    entropy: event.entropy,
                    journal_height: event.journal_height,
                    manifest_hash: if event.manifest_hash.is_empty() {
                        self.manifest_hash.to_hex()
                    } else {
                        event.manifest_hash
                    },
                };
                let event = DomainEvent {
                    schema: event.schema,
                    value: event.value,
                    key: event.key,
                };
                self.replay_applying_domain_record = true;
                let result = self.process_domain_event_with_ingress(event, stamp);
                self.replay_applying_domain_record = false;
                result?;
                let tick_started = Instant::now();
                self.tick_until_idle()?;
                if let Some(metrics) = self.replay_metrics.as_mut() {
                    metrics.tick_ns += tick_started.elapsed().as_nanos();
                }
            }
            JournalRecord::EffectIntent(record) => {
                let record = self.hydrate_effect_intent_record(record)?;
                self.restore_effect_intent(record)?;
            }
            JournalRecord::EffectReceipt(record) => {
                self.sync_logical_from_record(record.logical_now_ns);
                let record = self.hydrate_effect_receipt_record(record)?;
                let stamp = IngressStamp {
                    now_ns: record.now_ns,
                    logical_now_ns: record.logical_now_ns,
                    entropy: record.entropy,
                    journal_height: record.journal_height,
                    manifest_hash: if record.manifest_hash.is_empty() {
                        self.manifest_hash.to_hex()
                    } else {
                        record.manifest_hash
                    },
                };
                let receipt = EffectReceipt {
                    intent_hash: record.intent_hash,
                    adapter_id: record.adapter_id,
                    status: record.status,
                    payload_cbor: record.payload_cbor,
                    cost_cents: record.cost_cents,
                    signature: record.signature,
                };
                self.handle_receipt_with_ingress(receipt, stamp)?;
                let tick_started = Instant::now();
                self.tick_until_idle()?;
                if let Some(metrics) = self.replay_metrics.as_mut() {
                    metrics.tick_ns += tick_started.elapsed().as_nanos();
                }
            }
            JournalRecord::StreamFrame(record) => {
                self.sync_logical_from_record(record.logical_now_ns);
                let stamp = IngressStamp {
                    now_ns: record.now_ns,
                    logical_now_ns: record.logical_now_ns,
                    entropy: record.entropy,
                    journal_height: record.journal_height,
                    manifest_hash: if record.manifest_hash.is_empty() {
                        self.manifest_hash.to_hex()
                    } else {
                        record.manifest_hash
                    },
                };
                let frame = aos_effects::EffectStreamFrame {
                    intent_hash: record.intent_hash,
                    adapter_id: record.adapter_id,
                    origin_module_id: record.origin_module_id,
                    origin_instance_key: record.origin_instance_key,
                    effect_kind: record.effect_kind,
                    emitted_at_seq: record.emitted_at_seq,
                    seq: record.seq,
                    kind: record.frame_kind,
                    payload_cbor: record.payload_cbor,
                    payload_ref: record.payload_ref,
                    signature: record.signature,
                };
                self.handle_stream_frame_with_ingress(frame, stamp)?;
                let tick_started = Instant::now();
                self.tick_until_idle()?;
                if let Some(metrics) = self.replay_metrics.as_mut() {
                    metrics.tick_ns += tick_started.elapsed().as_nanos();
                }
            }
            JournalRecord::Manifest(record) => {
                let hash = Hash::from_hex_str(&record.manifest_hash).map_err(|err| {
                    KernelError::Manifest(format!("invalid manifest hash: {err}"))
                })?;
                if hash != self.manifest_hash {
                    let loaded = ManifestLoader::load_from_hash(self.store.as_ref(), hash)?;
                    self.apply_loaded_manifest(loaded, false)?;
                }
            }
            JournalRecord::Snapshot(_) => {
                // already handled separately
            }
            JournalRecord::Governance(record) => {
                self.governance.apply_record(&record);
            }
            _ => {}
        }
        Ok(())
    }

    fn hydrate_effect_intent_record(
        &mut self,
        mut record: EffectIntentRecord,
    ) -> Result<EffectIntentRecord, KernelError> {
        record.params_cbor = self.hydrate_externalized_cbor(
            record.params_cbor,
            record.params_ref.as_ref(),
            record.params_size,
            record.params_sha256.as_ref(),
            "effect_intent.params",
        )?;
        Ok(record)
    }

    fn hydrate_effect_receipt_record(
        &mut self,
        mut record: EffectReceiptRecord,
    ) -> Result<EffectReceiptRecord, KernelError> {
        record.payload_cbor = self.hydrate_externalized_cbor(
            record.payload_cbor,
            record.payload_ref.as_ref(),
            record.payload_size,
            record.payload_sha256.as_ref(),
            "effect_receipt.payload",
        )?;
        Ok(record)
    }

    fn load_snapshot_for_replay(&mut self, record: &SnapshotRecord) -> Result<(), KernelError> {
        if record.manifest_hash.is_none() {
            return Err(KernelError::SnapshotUnavailable(
                "snapshot record missing manifest_hash".into(),
            ));
        }
        let hash = Hash::from_hex_str(&record.snapshot_ref)
            .map_err(|err| KernelError::SnapshotDecode(err.to_string()))?;
        let bytes = self.store.get_blob(hash)?;
        let snapshot: KernelSnapshot = serde_cbor::from_slice(&bytes)
            .map_err(|err| KernelError::SnapshotDecode(err.to_string()))?;
        self.validate_snapshot_root_completeness(&snapshot)?;
        self.validate_snapshot_record_manifest_lineage(record, Some(&snapshot))?;
        self.last_snapshot_height = Some(record.height);
        self.last_snapshot_hash = Some(hash);
        if let Some(manifest_hex) = record.manifest_hash.as_ref()
            && let Ok(h) = Hash::from_hex_str(manifest_hex)
            && h != self.manifest_hash
        {
            let loaded = ManifestLoader::load_from_hash(self.store.as_ref(), h)?;
            self.apply_loaded_manifest(loaded, false)?;
        }
        self.snapshot_index.insert(
            record.height,
            (
                hash,
                record
                    .manifest_hash
                    .as_ref()
                    .and_then(|s| Hash::from_hex_str(s).ok()),
            ),
        );
        self.apply_snapshot(snapshot)
    }

    fn load_snapshot(&mut self, record: &SnapshotRecord) -> Result<(), KernelError> {
        self.validate_baseline_promotion(record)?;
        self.universe_id = record.universe_id;
        self.active_baseline = Some(record.clone());
        self.load_snapshot_for_replay(record)
    }

    pub(super) fn ensure_active_baseline(&mut self) -> Result<(), KernelError> {
        if self.active_baseline.is_some() {
            return Ok(());
        }
        self.create_snapshot()?;
        if self.active_baseline.is_none() {
            return Err(KernelError::SnapshotUnavailable(
                "failed to establish active baseline due to receipt-horizon precondition".into(),
            ));
        }
        Ok(())
    }

    fn receipt_horizon_height_for_baseline(&self, height: JournalSeq) -> Option<JournalSeq> {
        Some(height)
    }

    fn validate_baseline_promotion(&self, record: &SnapshotRecord) -> Result<(), KernelError> {
        let Some(horizon) = record.receipt_horizon_height else {
            return Err(KernelError::SnapshotUnavailable(
                "baseline promotion requires receipt_horizon_height".into(),
            ));
        };
        if horizon != record.height {
            return Err(KernelError::SnapshotUnavailable(format!(
                "baseline receipt_horizon_height ({horizon}) must equal baseline height ({})",
                record.height
            )));
        }
        Ok(())
    }

    fn validate_snapshot_root_completeness(
        &self,
        snapshot: &KernelSnapshot,
    ) -> Result<(), KernelError> {
        let roots = snapshot.root_completeness();
        let Some(snapshot_manifest_hash) = snapshot.manifest_hash() else {
            return Err(KernelError::SnapshotUnavailable(
                "snapshot root completeness missing manifest_hash".into(),
            ));
        };
        let Some(roots_manifest_hash) = roots.manifest_hash.as_ref() else {
            return Err(KernelError::SnapshotUnavailable(
                "root completeness missing manifest_hash".into(),
            ));
        };
        if roots_manifest_hash.as_slice() != snapshot_manifest_hash {
            return Err(KernelError::SnapshotUnavailable(
                "root completeness manifest_hash mismatch".into(),
            ));
        }

        let state_roots: HashSet<[u8; 32]> = roots.workflow_state_roots.iter().cloned().collect();
        for entry in snapshot.workflow_state_entries() {
            if !state_roots.contains(&entry.state_hash) {
                return Err(KernelError::SnapshotUnavailable(
                    "root completeness missing workflow state root".into(),
                ));
            }
        }

        let index_roots: HashSet<[u8; 32]> = roots.cell_index_roots.iter().cloned().collect();
        for (_, root) in snapshot.workflow_index_roots() {
            if !index_roots.contains(root) {
                return Err(KernelError::SnapshotUnavailable(
                    "root completeness missing workflow cell_index_root".into(),
                ));
            }
        }
        Ok(())
    }

    fn journal_manifest_hash_at_height(&self, height: JournalSeq) -> Result<Hash, KernelError> {
        let mut cursor = 0;
        let mut current_manifest: Option<Hash> = None;
        while cursor < height {
            let remaining = height.saturating_sub(cursor) as usize;
            let limit = REPLAY_BATCH_SIZE.min(remaining.max(1));
            let entries = self.journal.load_batch_from(cursor, limit)?;
            if entries.is_empty() {
                break;
            }
            let mut next_cursor = cursor;
            for entry in entries {
                if entry.seq >= height {
                    break;
                }
                next_cursor = entry.seq.saturating_add(1);
                if !matches!(entry.kind, JournalKind::Manifest) {
                    continue;
                }
                let record = decode_manifest_record(&entry.payload)?;
                let hash = Hash::from_hex_str(&record.manifest_hash).map_err(|err| {
                    KernelError::Manifest(format!(
                        "invalid journal manifest hash at height {}: {err}",
                        entry.seq
                    ))
                })?;
                current_manifest = Some(hash);
            }
            cursor = next_cursor.max(cursor.saturating_add(1));
        }
        current_manifest.ok_or_else(|| {
            KernelError::SnapshotUnavailable(format!(
                "no journal manifest lineage found before snapshot height {height}"
            ))
        })
    }

    fn validate_snapshot_record_manifest_lineage(
        &self,
        record: &SnapshotRecord,
        snapshot: Option<&KernelSnapshot>,
    ) -> Result<(), KernelError> {
        let record_manifest = record
            .manifest_hash
            .as_deref()
            .ok_or_else(|| {
                KernelError::SnapshotUnavailable("snapshot record missing manifest_hash".into())
            })
            .and_then(|value| {
                Hash::from_hex_str(value).map_err(|err| {
                    KernelError::SnapshotUnavailable(format!(
                        "snapshot record manifest_hash is invalid: {err}"
                    ))
                })
            })?;
        let journal_manifest = match self.journal_manifest_hash_at_height(record.height) {
            Ok(hash) => hash,
            Err(KernelError::Journal(message))
                if message.contains("journal entry missing at height 0") =>
            {
                record_manifest
            }
            Err(KernelError::SnapshotUnavailable(message))
                if message.contains("no journal manifest lineage found before snapshot height") =>
            {
                record_manifest
            }
            Err(err) => return Err(err),
        };
        if record_manifest != journal_manifest {
            return Err(KernelError::SnapshotUnavailable(format!(
                "snapshot manifest hash {} does not match journal manifest lineage {} at height {}",
                record_manifest, journal_manifest, record.height
            )));
        }
        if let Some(snapshot) = snapshot
            && let Some(bytes) = snapshot.manifest_hash()
        {
            let snapshot_manifest = Hash::from_bytes(bytes).map_err(|err| {
                KernelError::SnapshotUnavailable(format!(
                    "snapshot payload manifest_hash is invalid: {err}"
                ))
            })?;
            if snapshot_manifest != journal_manifest {
                return Err(KernelError::SnapshotUnavailable(format!(
                    "snapshot payload manifest hash {} does not match journal manifest lineage {} at height {}",
                    snapshot_manifest, journal_manifest, record.height
                )));
            }
        }
        Ok(())
    }

    fn apply_snapshot(&mut self, snapshot: KernelSnapshot) -> Result<(), KernelError> {
        self.workflow_index_roots = snapshot
            .workflow_index_roots()
            .iter()
            .filter_map(|(name, bytes)| Hash::from_bytes(bytes).ok().map(|h| (name.clone(), h)))
            .collect();

        if let Some(bytes) = snapshot.manifest_hash()
            && let Ok(hash) = Hash::from_bytes(bytes)
        {
            self.manifest_hash = hash;
        }

        let mut restored: HashMap<Name, WorkflowState> = HashMap::new();
        for entry in snapshot.workflow_state_entries().iter().cloned() {
            // Ensure blobs are present in store for deterministic reloads.
            self.store.put_blob(&entry.state)?;
            let state_entry = restored
                .entry(entry.workflow.clone())
                .or_insert_with(|| WorkflowState::new(self.cell_cache_size));
            let state_hash = Hash::from_bytes(&entry.state_hash)
                .unwrap_or_else(|_| Hash::of_bytes(&entry.state));
            let key_bytes = entry.key.unwrap_or_else(|| MONO_KEY.to_vec());
            let key_hash = Hash::of_bytes(&key_bytes);
            let root = self.ensure_cell_index_root(&entry.workflow)?;
            let state_size = entry.state.len() as u64;
            let meta = CellMeta {
                key_hash: *key_hash.as_bytes(),
                key_bytes: key_bytes.clone(),
                state_hash: *state_hash.as_bytes(),
                size: state_size,
                last_active_ns: entry.last_active_ns,
            };
            let index = CellIndex::new(self.store.as_ref());
            let new_root = index.upsert(root, meta)?;
            self.workflow_index_roots
                .insert(entry.workflow.clone(), new_root);
            state_entry.cell_cache.insert(
                key_bytes.clone(),
                CellEntry {
                    state: entry.state.clone(),
                    state_hash,
                    last_active_ns: entry.last_active_ns,
                },
            );
            self.record_cell_projection_delta(CellProjectionDelta {
                workflow: entry.workflow,
                key_hash: key_hash.as_bytes().to_vec(),
                key_bytes,
                state: Some(CellProjectionDeltaState {
                    state_bytes: entry.state,
                    state_hash,
                    size: state_size,
                    last_active_ns: entry.last_active_ns,
                }),
            });
        }
        self.workflow_state = restored;
        let (deque, set) = receipts_to_vecdeque(snapshot.recent_receipts(), RECENT_RECEIPT_CACHE);
        self.recent_receipts = deque;
        self.recent_receipt_index = set;

        self.pending_workflow_receipts = snapshot
            .pending_workflow_receipts()
            .iter()
            .cloned()
            .filter(|snap| !self.recent_receipt_index.contains(&snap.intent_hash))
            .map(|snap| (snap.intent_hash, snap.into_context()))
            .collect();
        self.workflow_instances = snapshot
            .workflow_instances()
            .iter()
            .cloned()
            .map(|snap| {
                let inflight_intents = snap
                    .inflight_intents
                    .into_iter()
                    .filter(|intent| !self.recent_receipt_index.contains(&intent.intent_id))
                    .map(|intent| {
                        (
                            intent.intent_id,
                            WorkflowInflightIntentMeta {
                                origin_module_id: intent.origin_module_id,
                                origin_instance_key: intent.origin_instance_key,
                                effect_kind: intent.effect_kind,
                                params_hash: intent.params_hash,
                                emitted_at_seq: intent.emitted_at_seq,
                                last_stream_seq: intent.last_stream_seq,
                            },
                        )
                    })
                    .collect::<BTreeMap<_, _>>();
                let status = match snap.status {
                    WorkflowStatusSnapshot::Running => WorkflowRuntimeStatus::Running,
                    WorkflowStatusSnapshot::Waiting => WorkflowRuntimeStatus::Waiting,
                    WorkflowStatusSnapshot::Completed => WorkflowRuntimeStatus::Completed,
                    WorkflowStatusSnapshot::Failed => WorkflowRuntimeStatus::Failed,
                };
                let mut state = WorkflowInstanceState {
                    state_bytes: snap.state_bytes,
                    inflight_intents,
                    status,
                    last_processed_event_seq: snap.last_processed_event_seq,
                    module_version: snap.module_version,
                };
                refresh_workflow_status(&mut state);
                (snap.instance_id, state)
            })
            .collect();

        self.effect_manager
            .update_logical_now_ns(snapshot.logical_now_ns());
        self.clock
            .sync_logical_min(self.effect_manager.logical_now_ns());

        self.workflow_queue.clear();

        Ok(())
    }

    /// Return snapshot record (hash + manifest hash) for an exact height, if known.
    pub(super) fn snapshot_at_height(&self, height: JournalSeq) -> Option<(Hash, Option<Hash>)> {
        self.snapshot_index.get(&height).cloned()
    }

    pub fn tail_scan_after(&self, height: JournalSeq) -> Result<TailScan, KernelError> {
        let head = self.journal.next_seq();
        let from_seq = self.tail_scan_start_seq(height);
        self.build_tail_scan(height, head, from_seq)
    }

    pub(crate) fn tail_scan_from(&self, from_seq: JournalSeq) -> Result<TailScan, KernelError> {
        let head = self.journal.next_seq();
        self.build_tail_scan(from_seq, head, from_seq)
    }

    fn build_tail_scan(
        &self,
        reported_from: JournalSeq,
        head: JournalSeq,
        from_seq: JournalSeq,
    ) -> Result<TailScan, KernelError> {
        if from_seq >= head {
            return Ok(TailScan {
                from: reported_from,
                to: head,
                entries: Vec::new(),
                intents: Vec::new(),
                receipts: Vec::new(),
            });
        }

        let entries = self.journal.load_from(from_seq)?;
        let mut scan = TailScan {
            from: reported_from,
            to: head,
            entries: Vec::new(),
            intents: Vec::new(),
            receipts: Vec::new(),
        };

        for entry in entries {
            let record = decode_tail_record(entry.kind, &entry.payload)?;
            scan.entries.push(TailEntry {
                seq: entry.seq,
                kind: entry.kind,
                record: record.clone(),
            });

            match record {
                JournalRecord::EffectIntent(record) => {
                    scan.intents.push(TailIntent {
                        seq: entry.seq,
                        record,
                    });
                }
                JournalRecord::EffectReceipt(record) => {
                    scan.receipts.push(TailReceipt {
                        seq: entry.seq,
                        record,
                    });
                }
                _ => {}
            }
        }

        Ok(scan)
    }

    fn tail_scan_start_seq(&self, cursor_height: JournalSeq) -> JournalSeq {
        if self.last_snapshot_height.is_none() && cursor_height == 0 {
            // Fresh worlds can start at sequence 0 before the first baseline exists.
            return 0;
        }
        cursor_height.saturating_add(1)
    }
}

fn decode_tail_record(kind: JournalKind, payload: &[u8]) -> Result<JournalRecord, KernelError> {
    if let Ok(record) = serde_cbor::from_slice::<JournalRecord>(payload) {
        return Ok(record);
    }
    // Backward-compatible fallback for older payloads that were encoded as raw records.
    let err = |e: serde_cbor::Error| KernelError::Journal(e.to_string());
    match kind {
        JournalKind::DomainEvent => serde_cbor::from_slice(payload)
            .map(JournalRecord::DomainEvent)
            .map_err(err),
        JournalKind::EffectIntent => serde_cbor::from_slice(payload)
            .map(JournalRecord::EffectIntent)
            .map_err(err),
        JournalKind::EffectReceipt => serde_cbor::from_slice(payload)
            .map(JournalRecord::EffectReceipt)
            .map_err(err),
        JournalKind::StreamFrame => serde_cbor::from_slice(payload)
            .map(JournalRecord::StreamFrame)
            .map_err(err),
        JournalKind::Manifest => serde_cbor::from_slice(payload)
            .map(JournalRecord::Manifest)
            .map_err(err),
        JournalKind::Snapshot => serde_cbor::from_slice(payload)
            .map(JournalRecord::Snapshot)
            .map_err(err),
        JournalKind::Governance => serde_cbor::from_slice(payload)
            .map(JournalRecord::Governance)
            .map_err(err),
        JournalKind::PlanStarted => serde_cbor::from_slice(payload)
            .map(JournalRecord::PlanStarted)
            .map_err(err),
        JournalKind::PlanResult => serde_cbor::from_slice(payload)
            .map(JournalRecord::PlanResult)
            .map_err(err),
        JournalKind::PlanEnded => serde_cbor::from_slice(payload)
            .map(JournalRecord::PlanEnded)
            .map_err(err),
        JournalKind::Custom => serde_cbor::from_slice(payload)
            .map(JournalRecord::Custom)
            .map_err(err),
    }
}

fn decode_manifest_record(payload: &[u8]) -> Result<ManifestRecord, KernelError> {
    if let Ok(JournalRecord::Manifest(record)) = serde_cbor::from_slice::<JournalRecord>(payload) {
        return Ok(record);
    }
    serde_cbor::from_slice(payload).map_err(|err| KernelError::Journal(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemStore;
    use crate::journal::{Journal, JournalEntry, JournalKind};
    use crate::receipts::WorkflowEffectContext;
    use crate::world::test_support::{
        append_record, empty_manifest, event_record, kernel_with_store_and_journal,
        loaded_manifest_with_schema, minimal_kernel_non_keyed, write_manifest,
    };
    use aos_air_types::{
        HashRef, ModuleAbi, ModuleKind, SchemaRef, WorkflowAbi, catalog::EffectCatalog,
    };
    use aos_effects::EffectStreamFrame;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;
    use uuid::Uuid;

    fn install_stream_module(kernel: &mut Kernel<MemStore>, module_name: &str) {
        let module = aos_air_types::DefModule {
            name: module_name.into(),
            module_kind: ModuleKind::Workflow,
            wasm_hash: HashRef::new(format!("sha256:{}", "1".repeat(64))).unwrap(),
            key_schema: None,
            abi: ModuleAbi {
                workflow: Some(WorkflowAbi {
                    state: SchemaRef::new("sys/PendingEffect@1").unwrap(),
                    event: SchemaRef::new(crate::receipts::SYS_EFFECT_STREAM_FRAME_SCHEMA).unwrap(),
                    context: None,
                    annotations: None,
                    effects_emitted: vec![],
                }),
                pure: None,
            },
        };
        kernel.module_defs.insert(module_name.into(), module);
    }

    #[test]
    fn replay_applies_manifest_records_in_order() {
        let store = Arc::new(MemStore::default());
        let (loaded_a, hash_a) = loaded_manifest_with_schema(store.as_ref(), "com.acme/EventA@1");
        let (_loaded_b, hash_b) = loaded_manifest_with_schema(store.as_ref(), "com.acme/EventB@1");

        let mut journal = Journal::new();
        append_record(
            &mut journal,
            JournalRecord::Manifest(ManifestRecord {
                manifest_hash: hash_a.to_hex(),
            }),
        );
        append_record(
            &mut journal,
            JournalRecord::DomainEvent(event_record("com.acme/EventA@1", 1)),
        );
        append_record(
            &mut journal,
            JournalRecord::Manifest(ManifestRecord {
                manifest_hash: hash_b.to_hex(),
            }),
        );
        append_record(
            &mut journal,
            JournalRecord::DomainEvent(event_record("com.acme/EventB@1", 3)),
        );

        let kernel = Kernel::from_loaded_manifest_with_config(
            store,
            loaded_a,
            journal,
            KernelConfig::default(),
        )
        .expect("replay");
        assert_eq!(kernel.manifest_hash().to_hex(), hash_b.to_hex());
    }

    #[test]
    fn replay_applies_manifest_after_snapshot() {
        let store = Arc::new(MemStore::default());
        let (loaded_a, hash_a) = loaded_manifest_with_schema(store.as_ref(), "com.acme/EventA@1");
        let (_loaded_b, hash_b) = loaded_manifest_with_schema(store.as_ref(), "com.acme/EventB@1");

        let mut journal = Journal::new();
        append_record(
            &mut journal,
            JournalRecord::Manifest(ManifestRecord {
                manifest_hash: hash_a.to_hex(),
            }),
        );
        append_record(
            &mut journal,
            JournalRecord::DomainEvent(event_record("com.acme/EventA@1", 1)),
        );

        let snapshot_height = 2;
        let mut snapshot = KernelSnapshot::new(
            snapshot_height,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            0,
            Some(*hash_a.as_bytes()),
        );
        snapshot.set_root_completeness(SnapshotRootCompleteness {
            manifest_hash: Some(hash_a.as_bytes().to_vec()),
            ..SnapshotRootCompleteness::default()
        });
        let snap_bytes = serde_cbor::to_vec(&snapshot).expect("encode snapshot");
        let snap_hash = store.put_blob(&snap_bytes).expect("store snapshot");
        append_record(
            &mut journal,
            JournalRecord::Snapshot(SnapshotRecord {
                snapshot_ref: snap_hash.to_hex(),
                height: snapshot_height,
                universe_id: Uuid::nil(),
                logical_time_ns: 0,
                receipt_horizon_height: Some(snapshot_height),
                manifest_hash: Some(hash_a.to_hex()),
            }),
        );

        append_record(
            &mut journal,
            JournalRecord::DomainEvent(event_record("com.acme/EventA@1", 3)),
        );
        append_record(
            &mut journal,
            JournalRecord::Manifest(ManifestRecord {
                manifest_hash: hash_b.to_hex(),
            }),
        );
        append_record(
            &mut journal,
            JournalRecord::DomainEvent(event_record("com.acme/EventB@1", 5)),
        );

        let kernel = Kernel::from_loaded_manifest_with_config(
            store,
            loaded_a,
            journal,
            KernelConfig::default(),
        )
        .expect("replay");
        assert_eq!(kernel.manifest_hash().to_hex(), hash_b.to_hex());
    }

    #[test]
    fn replay_fails_closed_when_snapshot_manifest_disagrees_with_journal_lineage() {
        let store = Arc::new(MemStore::default());
        let (loaded_a, hash_a) = loaded_manifest_with_schema(store.as_ref(), "com.acme/EventA@1");
        let (_loaded_b, hash_b) = loaded_manifest_with_schema(store.as_ref(), "com.acme/EventB@1");

        let mut journal = Journal::new();
        append_record(
            &mut journal,
            JournalRecord::Manifest(ManifestRecord {
                manifest_hash: hash_a.to_hex(),
            }),
        );
        append_record(
            &mut journal,
            JournalRecord::DomainEvent(event_record("com.acme/EventA@1", 1)),
        );

        let snapshot_height = 2;
        let mut snapshot = KernelSnapshot::new(
            snapshot_height,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            0,
            Some(*hash_b.as_bytes()),
        );
        snapshot.set_root_completeness(SnapshotRootCompleteness {
            manifest_hash: Some(hash_b.as_bytes().to_vec()),
            ..SnapshotRootCompleteness::default()
        });
        let snap_bytes = serde_cbor::to_vec(&snapshot).expect("encode snapshot");
        let snap_hash = store.put_blob(&snap_bytes).expect("store snapshot");
        append_record(
            &mut journal,
            JournalRecord::Snapshot(SnapshotRecord {
                snapshot_ref: snap_hash.to_hex(),
                height: snapshot_height,
                universe_id: Uuid::nil(),
                logical_time_ns: 0,
                receipt_horizon_height: Some(snapshot_height),
                manifest_hash: Some(hash_b.to_hex()),
            }),
        );

        let err = match Kernel::from_loaded_manifest_with_config(
            store,
            loaded_a,
            journal,
            KernelConfig::default(),
        ) {
            Ok(_) => panic!("mismatched snapshot manifest should fail closed"),
            Err(err) => err,
        };
        let rendered = err.to_string();
        assert!(
            rendered.contains("journal manifest lineage"),
            "unexpected error: {rendered}"
        );
    }

    #[test]
    fn world_initialization_persists_active_baseline() {
        let kernel = minimal_kernel_non_keyed();
        let heights = kernel.heights();
        assert!(
            heights.snapshot.is_some(),
            "new worlds should always have an active baseline"
        );
        let entries = kernel.dump_journal().expect("journal");
        let baseline = entries
            .iter()
            .find_map(
                |entry| match serde_cbor::from_slice::<JournalRecord>(&entry.payload).ok() {
                    Some(JournalRecord::Snapshot(record)) => Some(record),
                    _ => None,
                },
            )
            .expect("baseline snapshot record");
        assert!(
            baseline.receipt_horizon_height.is_some(),
            "initial baseline should carry a receipt horizon when no pending receipts exist"
        );
    }

    #[test]
    fn snapshot_with_pending_workflow_receipts_still_promotes_active_baseline() {
        let mut kernel = minimal_kernel_non_keyed();
        let initial_baseline_height = kernel
            .active_baseline
            .as_ref()
            .expect("initial baseline")
            .height;

        kernel.pending_workflow_receipts.insert(
            [9u8; 32],
            WorkflowEffectContext::new(
                "com.acme/Workflow@1".into(),
                None,
                "http.request".into(),
                vec![],
                [9u8; 32],
                None,
                [9u8; 32],
                0,
                None,
            ),
        );
        kernel.create_snapshot().expect("snapshot still written");

        let active_height = kernel
            .active_baseline
            .as_ref()
            .expect("active baseline promoted")
            .height;
        assert!(
            active_height > initial_baseline_height,
            "snapshot with persisted continuation state should promote active baseline"
        );
    }

    #[test]
    fn replay_prefers_latest_snapshot_and_promotes_it_when_restorable() {
        let mut kernel = minimal_kernel_non_keyed();
        let initial_baseline_height = kernel
            .active_baseline
            .as_ref()
            .expect("initial baseline")
            .height;

        kernel.pending_workflow_receipts.insert(
            [7u8; 32],
            WorkflowEffectContext::new(
                "com.acme/Workflow@1".into(),
                None,
                "http.request".into(),
                vec![],
                [7u8; 32],
                None,
                [7u8; 32],
                0,
                None,
            ),
        );
        kernel.create_snapshot().expect("snapshot still written");
        let unsafe_snapshot_height = kernel.last_snapshot_height.expect("latest snapshot height");
        assert!(
            unsafe_snapshot_height > initial_baseline_height,
            "new snapshot should advance last_snapshot_height"
        );
        assert_eq!(
            kernel
                .active_baseline
                .as_ref()
                .expect("active baseline")
                .height,
            unsafe_snapshot_height,
            "latest snapshot should now promote active baseline"
        );

        let store = kernel.store.clone();
        let loaded = LoadedManifest {
            manifest: kernel.manifest.clone(),
            secrets: kernel.secrets.clone(),
            modules: kernel.module_defs.clone(),
            ops: kernel.effect_defs.clone(),
            schemas: kernel.schema_defs.clone(),
            effect_catalog: EffectCatalog::from_defs(kernel.effect_defs.values().cloned()),
        };
        let entries = kernel.dump_journal().expect("journal entries");
        let reopened = Kernel::from_loaded_manifest_with_config(
            store,
            loaded,
            Journal::from_entries(&entries).unwrap(),
            KernelConfig::default(),
        )
        .expect("replay from latest snapshot");

        assert_eq!(
            reopened.last_snapshot_height,
            Some(unsafe_snapshot_height),
            "startup replay should restore from the newest snapshot"
        );
        assert_eq!(
            reopened
                .active_baseline
                .as_ref()
                .expect("active baseline")
                .height,
            unsafe_snapshot_height,
            "startup replay should restore the promoted active baseline"
        );
    }

    #[test]
    fn snapshot_root_completeness_missing_required_root_fails_closed() {
        let store = Arc::new(MemStore::default());
        let (loaded, manifest_hash) =
            loaded_manifest_with_schema(store.as_ref(), "com.acme/Event@1");

        let snapshot = KernelSnapshot::new(1, vec![], vec![], vec![], vec![], vec![], 0, None);
        let snap_bytes = serde_cbor::to_vec(&snapshot).unwrap();
        let snap_hash = store.put_blob(&snap_bytes).unwrap();

        let mut bad_journal = Journal::new();
        append_record(
            &mut bad_journal,
            JournalRecord::Manifest(ManifestRecord {
                manifest_hash: manifest_hash.to_hex(),
            }),
        );
        append_record(
            &mut bad_journal,
            JournalRecord::Snapshot(SnapshotRecord {
                snapshot_ref: snap_hash.to_hex(),
                height: 1,
                universe_id: Uuid::nil(),
                logical_time_ns: 0,
                receipt_horizon_height: Some(1),
                manifest_hash: Some(manifest_hash.to_hex()),
            }),
        );

        let err = match Kernel::from_loaded_manifest(store, loaded, bad_journal) {
            Ok(_) => panic!("incomplete roots should fail"),
            Err(err) => err,
        };
        let rendered = err.to_string();
        assert!(
            rendered.contains("root completeness") || rendered.contains("snapshot"),
            "unexpected error: {rendered}"
        );
    }

    #[test]
    fn snapshot_restores_workflow_instance_waiting_state() {
        let mut kernel = minimal_kernel_non_keyed();
        let mut snapshot = KernelSnapshot::new(
            1,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![WorkflowInstanceSnapshot {
                instance_id: "com.acme/Workflow@1::".into(),
                state_bytes: vec![1, 2, 3],
                inflight_intents: vec![WorkflowInflightIntentSnapshot {
                    intent_id: [9u8; 32],
                    origin_module_id: "com.acme/Workflow@1".into(),
                    origin_instance_key: None,
                    effect_kind: "http.request".into(),
                    params_hash: Some(
                        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                            .into(),
                    ),
                    emitted_at_seq: 7,
                    last_stream_seq: 3,
                }],
                status: WorkflowStatusSnapshot::Waiting,
                last_processed_event_seq: 7,
                module_version: Some(
                    "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                        .into(),
                ),
            }],
            0,
            Some(*kernel.manifest_hash().as_bytes()),
        );
        snapshot.set_root_completeness(SnapshotRootCompleteness {
            manifest_hash: Some(kernel.manifest_hash().as_bytes().to_vec()),
            ..SnapshotRootCompleteness::default()
        });
        kernel.apply_snapshot(snapshot).expect("apply snapshot");
        let instances = kernel.workflow_instances_snapshot();
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].instance_id, "com.acme/Workflow@1::");
        assert_eq!(instances[0].state_bytes, vec![1, 2, 3]);
        assert_eq!(instances[0].status, WorkflowStatusSnapshot::Waiting);
        assert_eq!(instances[0].inflight_intents.len(), 1);
        assert_eq!(instances[0].inflight_intents[0].intent_id, [9u8; 32]);
        assert_eq!(instances[0].inflight_intents[0].last_stream_seq, 3);
    }

    #[test]
    fn snapshot_restores_stream_cursor_acceptance_behavior() {
        let store = Arc::new(MemStore::default());
        let journal = Journal::new();
        let mut kernel = kernel_with_store_and_journal(store.clone(), journal);
        install_stream_module(&mut kernel, "com.acme/Workflow@1");

        let intent_hash = [4u8; 32];
        let context = WorkflowEffectContext::new(
            "com.acme/Workflow@1".into(),
            None,
            "http.request".into(),
            vec![1, 2, 3],
            [4u8; 32],
            None,
            intent_hash,
            5,
            None,
        );
        let mut snapshot = KernelSnapshot::new(
            1,
            vec![],
            vec![],
            vec![],
            vec![WorkflowReceiptSnapshot::from_context(intent_hash, &context)],
            vec![WorkflowInstanceSnapshot {
                instance_id: "com.acme/Workflow@1::".into(),
                state_bytes: vec![0xAA],
                inflight_intents: vec![WorkflowInflightIntentSnapshot {
                    intent_id: intent_hash,
                    origin_module_id: "com.acme/Workflow@1".into(),
                    origin_instance_key: None,
                    effect_kind: "http.request".into(),
                    params_hash: Some(Hash::of_bytes(&context.params_cbor).to_hex()),
                    emitted_at_seq: 5,
                    last_stream_seq: 2,
                }],
                status: WorkflowStatusSnapshot::Waiting,
                last_processed_event_seq: 5,
                module_version: None,
            }],
            0,
            Some(*kernel.manifest_hash().as_bytes()),
        );
        snapshot.set_root_completeness(SnapshotRootCompleteness {
            manifest_hash: Some(kernel.manifest_hash().as_bytes().to_vec()),
            ..SnapshotRootCompleteness::default()
        });
        kernel.apply_snapshot(snapshot).expect("apply snapshot");

        let duplicate = EffectStreamFrame {
            intent_hash,
            adapter_id: "adapter.stream".into(),
            origin_module_id: "com.acme/Workflow@1".into(),
            origin_instance_key: None,
            effect_kind: "http.request".into(),
            emitted_at_seq: 5,
            seq: 2,
            kind: "progress".into(),
            payload_cbor: vec![1],
            payload_ref: None,
            signature: vec![],
        };
        kernel
            .accept(WorldInput::StreamFrame(duplicate))
            .expect("duplicate stream frame should be dropped");
        assert!(kernel.workflow_queue.is_empty());

        let next = EffectStreamFrame {
            intent_hash,
            adapter_id: "adapter.stream".into(),
            origin_module_id: "com.acme/Workflow@1".into(),
            origin_instance_key: None,
            effect_kind: "http.request".into(),
            emitted_at_seq: 5,
            seq: 3,
            kind: "progress".into(),
            payload_cbor: vec![2],
            payload_ref: None,
            signature: vec![],
        };
        kernel
            .accept(WorldInput::StreamFrame(next))
            .expect("next stream frame should be accepted");
        assert_eq!(kernel.workflow_queue.len(), 1);
        let instances = kernel.workflow_instances_snapshot();
        assert_eq!(instances[0].inflight_intents[0].last_stream_seq, 3);
    }

    #[test]
    fn snapshot_restore_scrubs_receipts_already_marked_recent() {
        let mut kernel = minimal_kernel_non_keyed();
        let intent_hash = [0x44u8; 32];
        let context = WorkflowEffectContext::new(
            "com.acme/Workflow@1".into(),
            None,
            "http.request".into(),
            vec![1, 2, 3],
            [0x44u8; 32],
            None,
            intent_hash,
            5,
            None,
        );
        let mut snapshot = KernelSnapshot::new(
            1,
            vec![],
            vec![intent_hash],
            vec![],
            vec![WorkflowReceiptSnapshot::from_context(intent_hash, &context)],
            vec![WorkflowInstanceSnapshot {
                instance_id: "com.acme/Workflow@1::".into(),
                state_bytes: vec![0xAA],
                inflight_intents: vec![WorkflowInflightIntentSnapshot {
                    intent_id: intent_hash,
                    origin_module_id: "com.acme/Workflow@1".into(),
                    origin_instance_key: None,
                    effect_kind: "http.request".into(),
                    params_hash: Some(Hash::of_bytes(&context.params_cbor).to_hex()),
                    emitted_at_seq: 5,
                    last_stream_seq: 0,
                }],
                status: WorkflowStatusSnapshot::Waiting,
                last_processed_event_seq: 5,
                module_version: None,
            }],
            0,
            Some(*kernel.manifest_hash().as_bytes()),
        );
        snapshot.set_root_completeness(SnapshotRootCompleteness {
            manifest_hash: Some(kernel.manifest_hash().as_bytes().to_vec()),
            ..SnapshotRootCompleteness::default()
        });

        kernel.apply_snapshot(snapshot).expect("apply snapshot");

        assert!(
            kernel.pending_workflow_receipts_snapshot().is_empty(),
            "stale pending receipt context should be scrubbed on restore"
        );
        assert_eq!(
            kernel.workflow_instances_snapshot()[0]
                .inflight_intents
                .len(),
            0,
            "stale inflight intent should be scrubbed on restore"
        );
    }

    #[test]
    fn baseline_plus_tail_replay_matches_full_replay_state() {
        let store_full = Arc::new(MemStore::default());
        let (loaded_full, _) =
            loaded_manifest_with_schema(store_full.as_ref(), "com.acme/EventA@1");
        let mut kernel_full =
            Kernel::from_loaded_manifest(store_full.clone(), loaded_full, Journal::new()).unwrap();
        kernel_full
            .accept(WorldInput::DomainEvent(DomainEvent::new(
                "com.acme/EventA@1",
                serde_cbor::to_vec(&json!({ "id": "1" })).unwrap(),
            )))
            .unwrap();
        kernel_full
            .accept(WorldInput::DomainEvent(DomainEvent::new(
                "com.acme/EventA@1",
                serde_cbor::to_vec(&json!({ "id": "2" })).unwrap(),
            )))
            .unwrap();
        kernel_full
            .accept(WorldInput::DomainEvent(DomainEvent::new(
                "com.acme/EventA@1",
                serde_cbor::to_vec(&json!({ "id": "3" })).unwrap(),
            )))
            .unwrap();
        kernel_full.create_snapshot().unwrap();

        let store_baseline = Arc::new(MemStore::default());
        let (loaded_baseline, _) =
            loaded_manifest_with_schema(store_baseline.as_ref(), "com.acme/EventA@1");
        let mut kernel_baseline =
            Kernel::from_loaded_manifest(store_baseline.clone(), loaded_baseline, Journal::new())
                .unwrap();
        kernel_baseline
            .accept(WorldInput::DomainEvent(DomainEvent::new(
                "com.acme/EventA@1",
                serde_cbor::to_vec(&json!({ "id": "1" })).unwrap(),
            )))
            .unwrap();
        kernel_baseline.create_snapshot().unwrap();
        kernel_baseline
            .accept(WorldInput::DomainEvent(DomainEvent::new(
                "com.acme/EventA@1",
                serde_cbor::to_vec(&json!({ "id": "2" })).unwrap(),
            )))
            .unwrap();
        kernel_baseline
            .accept(WorldInput::DomainEvent(DomainEvent::new(
                "com.acme/EventA@1",
                serde_cbor::to_vec(&json!({ "id": "3" })).unwrap(),
            )))
            .unwrap();
        kernel_baseline.create_snapshot().unwrap();

        assert_eq!(
            kernel_full.manifest_hash, kernel_baseline.manifest_hash,
            "manifest hash must be identical after baseline+tail and full replay"
        );
        assert_eq!(
            kernel_full.workflow_index_roots, kernel_baseline.workflow_index_roots,
            "cell index roots must be identical after baseline+tail and full replay"
        );
    }

    #[test]
    fn snapshot_restores_cell_index_root_and_cells() {
        let store = Arc::new(MemStore::default());
        let journal = Journal::new();
        let mut kernel = kernel_with_store_and_journal(store.clone(), journal);
        let workflow = "com.acme/Workflow@1".to_string();
        let key = b"k".to_vec();
        let state_bytes = vec![9u8, 9u8];

        kernel
            .handle_workflow_output(
                workflow.clone(),
                Some(key.clone()),
                true,
                WorkflowOutput {
                    state: Some(state_bytes.clone()),
                    ..Default::default()
                },
            )
            .unwrap();
        let root_before_snapshot = *kernel
            .workflow_index_roots
            .get(&workflow)
            .expect("pre-snapshot root");
        let head_state = kernel
            .workflow_state_bytes(&workflow, Some(&key))
            .unwrap()
            .expect("pre-snapshot head state");
        assert_eq!(head_state, state_bytes);

        kernel.create_snapshot().unwrap();
        let root_after_snapshot = *kernel.workflow_index_roots.get(&workflow).unwrap();
        assert_ne!(root_before_snapshot, root_after_snapshot);
        let entries = kernel.journal.load_from(0).expect("load journal entries");

        let mut kernel2 = {
            let journal = Journal::from_entries(&entries).unwrap();
            kernel_with_store_and_journal(store.clone(), journal)
        };
        kernel2.tick_until_idle().unwrap();

        let root_after = *kernel2.workflow_index_roots.get(&workflow).unwrap();
        assert_eq!(root_after_snapshot, root_after);

        let index = CellIndex::new(store.as_ref());
        let meta = index
            .get(root_after, Hash::of_bytes(&key).as_bytes())
            .unwrap()
            .expect("restored meta");
        assert_eq!(meta.state_hash, *Hash::of_bytes(&state_bytes).as_bytes());
        let restored_state = store
            .get_blob(Hash::from_bytes(&meta.state_hash).unwrap())
            .unwrap();
        assert_eq!(restored_state, state_bytes);
    }

    #[test]
    fn tail_scan_returns_entries_after_height() {
        let tmp = TempDir::new().unwrap();
        let manifest_path = tmp.path().join("manifest.cbor");
        write_manifest(&manifest_path, &empty_manifest());

        let store = Arc::new(MemStore::new());
        let mut kernel = KernelBuilder::new(store)
            .from_manifest_path(&manifest_path)
            .expect("kernel");

        let intent = EffectIntentRecord {
            intent_hash: [1u8; 32],
            kind: "http.request".into(),
            params_cbor: vec![1],
            params_ref: None,
            params_size: None,
            params_sha256: None,
            idempotency_key: [2u8; 32],
            origin: IntentOriginRecord::Workflow {
                name: "example/Workflow@1".into(),
                instance_key: None,
                issuer_ref: None,
                emitted_at_seq: Some(0),
            },
        };
        let intent_bytes = serde_cbor::to_vec(&intent).unwrap();
        kernel
            .journal
            .append(JournalEntry::new(JournalKind::EffectIntent, &intent_bytes))
            .unwrap();

        let receipt = EffectReceiptRecord {
            intent_hash: [1u8; 32],
            adapter_id: "stub.http".into(),
            status: aos_effects::ReceiptStatus::Ok,
            payload_cbor: vec![],
            payload_ref: None,
            payload_size: None,
            payload_sha256: None,
            cost_cents: None,
            signature: vec![],
            now_ns: 0,
            logical_now_ns: 0,
            journal_height: 0,
            entropy: Vec::new(),
            manifest_hash: String::new(),
        };
        let receipt_bytes = serde_cbor::to_vec(&receipt).unwrap();
        kernel
            .journal
            .append(JournalEntry::new(
                JournalKind::EffectReceipt,
                &receipt_bytes,
            ))
            .unwrap();

        let scan = kernel.tail_scan_after(0).expect("tail scan");
        assert!(scan.entries.len() >= 2);
        assert_eq!(scan.intents.len(), 1);
        assert_eq!(scan.receipts.len(), 1);
        assert!(
            scan.entries
                .iter()
                .any(|entry| entry.kind == JournalKind::EffectIntent)
        );
        assert!(
            scan.entries
                .iter()
                .any(|entry| entry.kind == JournalKind::EffectReceipt)
        );
        assert_eq!(scan.intents[0].seq, 2);
        assert_eq!(scan.receipts[0].seq, 3);
        assert_eq!(scan.intents[0].record.intent_hash, [1u8; 32]);
        assert_eq!(scan.receipts[0].record.intent_hash, [1u8; 32]);
    }
}
