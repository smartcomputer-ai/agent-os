use super::*;

impl<S: Store + 'static> Kernel<S> {
    pub fn create_snapshot(&mut self) -> Result<(), KernelError> {
        self.tick_until_idle()?;
        if !self.reducer_queue.is_empty() {
            return Err(KernelError::SnapshotUnavailable(
                "reducer queue must be idle before snapshot".into(),
            ));
        }
        let height = self.journal.next_seq();
        let reducer_state: Vec<ReducerStateEntry> = Vec::new();
        let recent_receipts: Vec<[u8; 32]> = self.recent_receipts.iter().cloned().collect();
        let queued_effects = self
            .effect_manager
            .queued()
            .iter()
            .map(EffectIntentSnapshot::from_intent)
            .collect();
        let reducer_index_roots = self
            .reducer_index_roots
            .iter()
            .map(|(name, hash)| (name.clone(), *hash.as_bytes()))
            .collect();
        let pending_reducer_receipts = self
            .pending_reducer_receipts
            .iter()
            .map(|(hash, ctx)| ReducerReceiptSnapshot::from_context(*hash, ctx))
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
            reducer_state,
            recent_receipts,
            queued_effects,
            pending_reducer_receipts,
            workflow_instances,
            logical_now_ns,
            Some(*self.manifest_hash.as_bytes()),
        );
        snapshot.set_reducer_index_roots(reducer_index_roots);
        let root_completeness = SnapshotRootCompleteness {
            manifest_hash: Some(self.manifest_hash.as_bytes().to_vec()),
            reducer_state_roots: snapshot
                .reducer_state_entries()
                .iter()
                .map(|entry| entry.state_hash)
                .collect(),
            cell_index_roots: snapshot
                .reducer_index_roots()
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
        let entries = self.journal.load_from(0)?;
        if entries.is_empty() {
            return Ok(());
        }
        let mut resume_seq: Option<JournalSeq> = None;
        let mut latest_promotable_baseline: Option<SnapshotRecord> = None;
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
                    if self.validate_baseline_promotion(&snapshot).is_ok() {
                        latest_promotable_baseline = Some(snapshot.clone());
                    }
                }
            }
        }
        if let Some(snapshot) = latest_promotable_baseline {
            resume_seq = Some(snapshot.height);
            self.active_baseline = Some(snapshot.clone());
            self.last_snapshot_height = Some(snapshot.height);
            self.load_snapshot(&snapshot)?;
        }
        self.suppress_journal = true;
        self.replay_applying_domain_record = false;
        self.replay_generated_domain_event_hashes.clear();
        for entry in entries {
            if resume_seq.is_some_and(|seq| entry.seq < seq) {
                continue;
            }
            let record: JournalRecord = serde_cbor::from_slice(&entry.payload)
                .map_err(|err| KernelError::Journal(err.to_string()))?;
            self.apply_replay_record(record)?;
        }
        self.tick_until_idle()?;
        self.suppress_journal = false;
        self.replay_applying_domain_record = false;
        self.replay_generated_domain_event_hashes.clear();
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
                self.tick_until_idle()?;
            }
            JournalRecord::EffectIntent(record) => {
                self.restore_effect_intent(record)?;
            }
            JournalRecord::EffectReceipt(record) => {
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
                let receipt = EffectReceipt {
                    intent_hash: record.intent_hash,
                    adapter_id: record.adapter_id,
                    status: record.status,
                    payload_cbor: record.payload_cbor,
                    cost_cents: record.cost_cents,
                    signature: record.signature,
                };
                self.handle_receipt_with_ingress(receipt, stamp)?;
                self.tick_until_idle()?;
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
                self.tick_until_idle()?;
            }
            JournalRecord::CapDecision(_) => {
                // Cap decisions are audit-only; runtime state is rebuilt via replay.
            }
            JournalRecord::PolicyDecision(_) => {
                // Policy decisions are audit-only; runtime state is rebuilt via replay.
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

    fn load_snapshot(&mut self, record: &SnapshotRecord) -> Result<(), KernelError> {
        self.validate_baseline_promotion(record)?;
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
        self.last_snapshot_height = Some(record.height);
        self.last_snapshot_hash = Some(hash);
        self.active_baseline = Some(record.clone());
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
        let has_inflight_workflow_intents = self
            .workflow_instances
            .values()
            .any(|instance| !instance.inflight_intents.is_empty());
        if self.pending_reducer_receipts.is_empty() && !has_inflight_workflow_intents {
            Some(height)
        } else {
            None
        }
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

        let state_roots: HashSet<[u8; 32]> = roots.reducer_state_roots.iter().cloned().collect();
        for entry in snapshot.reducer_state_entries() {
            if !state_roots.contains(&entry.state_hash) {
                return Err(KernelError::SnapshotUnavailable(
                    "root completeness missing reducer state root".into(),
                ));
            }
        }

        let index_roots: HashSet<[u8; 32]> = roots.cell_index_roots.iter().cloned().collect();
        for (_, root) in snapshot.reducer_index_roots() {
            if !index_roots.contains(root) {
                return Err(KernelError::SnapshotUnavailable(
                    "root completeness missing reducer cell_index_root".into(),
                ));
            }
        }
        Ok(())
    }

    fn apply_snapshot(&mut self, snapshot: KernelSnapshot) -> Result<(), KernelError> {
        self.reducer_index_roots = snapshot
            .reducer_index_roots()
            .iter()
            .filter_map(|(name, bytes)| Hash::from_bytes(bytes).ok().map(|h| (name.clone(), h)))
            .collect();

        if let Some(bytes) = snapshot.manifest_hash()
            && let Ok(hash) = Hash::from_bytes(bytes)
        {
            self.manifest_hash = hash;
        }

        let mut restored: HashMap<Name, ReducerState> = HashMap::new();
        for entry in snapshot.reducer_state_entries().iter().cloned() {
            // Ensure blobs are present in store for deterministic reloads.
            self.store.put_blob(&entry.state)?;
            let state_entry = restored.entry(entry.reducer.clone()).or_default();
            let state_hash = Hash::from_bytes(&entry.state_hash)
                .unwrap_or_else(|_| Hash::of_bytes(&entry.state));
            let key_bytes = entry.key.unwrap_or_else(|| MONO_KEY.to_vec());
            let key_hash = Hash::of_bytes(&key_bytes);
            let root = self.ensure_cell_index_root(&entry.reducer)?;
            let meta = CellMeta {
                key_hash: *key_hash.as_bytes(),
                key_bytes: key_bytes.clone(),
                state_hash: *state_hash.as_bytes(),
                size: entry.state.len() as u64,
                last_active_ns: entry.last_active_ns,
            };
            let index = CellIndex::new(self.store.as_ref());
            let new_root = index.upsert(root, meta)?;
            self.reducer_index_roots
                .insert(entry.reducer.clone(), new_root);
            state_entry.cell_cache.insert(
                key_bytes,
                CellEntry {
                    state: entry.state.clone(),
                    state_hash,
                    last_active_ns: entry.last_active_ns,
                },
            );
        }
        self.reducer_state = restored;
        let (deque, set) = receipts_to_vecdeque(snapshot.recent_receipts(), RECENT_RECEIPT_CACHE);
        self.recent_receipts = deque;
        self.recent_receipt_index = set;

        self.pending_reducer_receipts = snapshot
            .pending_reducer_receipts()
            .iter()
            .cloned()
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
                (
                    snap.instance_id,
                    WorkflowInstanceState {
                        state_bytes: snap.state_bytes,
                        inflight_intents,
                        status,
                        last_processed_event_seq: snap.last_processed_event_seq,
                        module_version: snap.module_version,
                    },
                )
            })
            .collect();

        self.effect_manager.restore_queue(
            snapshot
                .queued_effects()
                .iter()
                .cloned()
                .map(|snap| snap.into_intent())
                .collect(),
        );
        self.effect_manager
            .update_logical_now_ns(snapshot.logical_now_ns());
        self.clock
            .sync_logical_min(self.effect_manager.logical_now_ns());

        self.reducer_queue.clear();

        Ok(())
    }

    /// Return snapshot record (hash + manifest hash) for an exact height, if known.
    pub(super) fn snapshot_at_height(&self, height: JournalSeq) -> Option<(Hash, Option<Hash>)> {
        self.snapshot_index.get(&height).cloned()
    }

    pub fn tail_scan_after(&self, height: JournalSeq) -> Result<TailScan, KernelError> {
        let head = self.journal.next_seq();
        let from_seq = self.tail_scan_start_seq(height);
        if from_seq >= head {
            return Ok(TailScan {
                from: height,
                to: head,
                entries: Vec::new(),
                intents: Vec::new(),
                receipts: Vec::new(),
            });
        }

        let entries = self.journal.load_from(from_seq)?;
        let mut scan = TailScan {
            from: height,
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
        JournalKind::CapDecision => serde_cbor::from_slice(payload)
            .map(JournalRecord::CapDecision)
            .map_err(err),
        JournalKind::PolicyDecision => serde_cbor::from_slice(payload)
            .map(JournalRecord::PolicyDecision)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{JournalEntry, JournalKind, mem::MemJournal};
    use crate::receipts::ReducerEffectContext;
    use crate::world::test_support::{
        append_record, empty_manifest, event_record, kernel_with_store_and_journal,
        loaded_manifest_with_schema, minimal_kernel_non_keyed, write_manifest,
    };
    use aos_air_types::{HashRef, ModuleAbi, ModuleKind, ReducerAbi, SchemaRef};
    use aos_effects::EffectStreamFrame;
    use aos_store::MemStore;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn install_stream_module(kernel: &mut Kernel<MemStore>, module_name: &str) {
        let module = aos_air_types::DefModule {
            name: module_name.into(),
            module_kind: ModuleKind::Workflow,
            wasm_hash: HashRef::new(format!("sha256:{}", "1".repeat(64))).unwrap(),
            key_schema: None,
            abi: ModuleAbi {
                reducer: Some(ReducerAbi {
                    state: SchemaRef::new("sys/PlanError@1").unwrap(),
                    event: SchemaRef::new(crate::receipts::SYS_EFFECT_STREAM_FRAME_SCHEMA).unwrap(),
                    context: None,
                    annotations: None,
                    effects_emitted: vec![],
                    cap_slots: Default::default(),
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

        let mut journal = MemJournal::default();
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
            Box::new(journal),
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

        let mut journal = MemJournal::default();
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
            Box::new(journal),
            KernelConfig::default(),
        )
        .expect("replay");
        assert_eq!(kernel.manifest_hash().to_hex(), hash_b.to_hex());
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
    fn unsafe_baseline_promotion_fails_receipt_horizon_precondition() {
        let mut kernel = minimal_kernel_non_keyed();
        let initial_baseline_height = kernel
            .active_baseline
            .as_ref()
            .expect("initial baseline")
            .height;

        kernel.pending_reducer_receipts.insert(
            [9u8; 32],
            ReducerEffectContext::new(
                "com.acme/Workflow@1".into(),
                None,
                "http.request".into(),
                vec![],
                [9u8; 32],
                0,
                None,
            ),
        );
        kernel.create_snapshot().expect("snapshot still written");

        let active_height = kernel
            .active_baseline
            .as_ref()
            .expect("active baseline retained")
            .height;
        assert_eq!(
            active_height, initial_baseline_height,
            "unsafe snapshot must not promote active baseline"
        );
    }

    #[test]
    fn snapshot_root_completeness_missing_required_root_fails_closed() {
        let store = Arc::new(MemStore::default());
        let (loaded, manifest_hash) =
            loaded_manifest_with_schema(store.as_ref(), "com.acme/Event@1");

        let snapshot = KernelSnapshot::new(
            1,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            0,
            None,
        );
        let snap_bytes = serde_cbor::to_vec(&snapshot).unwrap();
        let snap_hash = store.put_blob(&snap_bytes).unwrap();

        let mut bad_journal = MemJournal::default();
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
                logical_time_ns: 0,
                receipt_horizon_height: Some(1),
                manifest_hash: Some(manifest_hash.to_hex()),
            }),
        );

        let err = match Kernel::from_loaded_manifest(store, loaded, Box::new(bad_journal)) {
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
        let journal = Box::new(MemJournal::new());
        let mut kernel = kernel_with_store_and_journal(store.clone(), journal);
        install_stream_module(&mut kernel, "com.acme/Workflow@1");

        let intent_hash = [4u8; 32];
        let context = ReducerEffectContext::new(
            "com.acme/Workflow@1".into(),
            None,
            "http.request".into(),
            vec![1, 2, 3],
            intent_hash,
            5,
            None,
        );
        let mut snapshot = KernelSnapshot::new(
            1,
            vec![],
            vec![],
            vec![],
            vec![ReducerReceiptSnapshot::from_context(intent_hash, &context)],
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
            .handle_stream_frame(duplicate)
            .expect("duplicate stream frame should be dropped");
        assert!(kernel.reducer_queue.is_empty());

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
            .handle_stream_frame(next)
            .expect("next stream frame should be accepted");
        assert_eq!(kernel.reducer_queue.len(), 1);
        let instances = kernel.workflow_instances_snapshot();
        assert_eq!(instances[0].inflight_intents[0].last_stream_seq, 3);
    }

    #[test]
    fn baseline_plus_tail_replay_matches_full_replay_state() {
        let store_full = Arc::new(MemStore::default());
        let (loaded_full, _) =
            loaded_manifest_with_schema(store_full.as_ref(), "com.acme/EventA@1");
        let mut kernel_full = Kernel::from_loaded_manifest(
            store_full.clone(),
            loaded_full,
            Box::new(MemJournal::default()),
        )
        .unwrap();
        kernel_full
            .submit_domain_event_result(
                "com.acme/EventA@1",
                serde_cbor::to_vec(&json!({ "id": "1" })).unwrap(),
            )
            .unwrap();
        kernel_full
            .submit_domain_event_result(
                "com.acme/EventA@1",
                serde_cbor::to_vec(&json!({ "id": "2" })).unwrap(),
            )
            .unwrap();
        kernel_full
            .submit_domain_event_result(
                "com.acme/EventA@1",
                serde_cbor::to_vec(&json!({ "id": "3" })).unwrap(),
            )
            .unwrap();
        kernel_full.create_snapshot().unwrap();

        let store_baseline = Arc::new(MemStore::default());
        let (loaded_baseline, _) =
            loaded_manifest_with_schema(store_baseline.as_ref(), "com.acme/EventA@1");
        let mut kernel_baseline = Kernel::from_loaded_manifest(
            store_baseline.clone(),
            loaded_baseline,
            Box::new(MemJournal::default()),
        )
        .unwrap();
        kernel_baseline
            .submit_domain_event_result(
                "com.acme/EventA@1",
                serde_cbor::to_vec(&json!({ "id": "1" })).unwrap(),
            )
            .unwrap();
        kernel_baseline.create_snapshot().unwrap();
        kernel_baseline
            .submit_domain_event_result(
                "com.acme/EventA@1",
                serde_cbor::to_vec(&json!({ "id": "2" })).unwrap(),
            )
            .unwrap();
        kernel_baseline
            .submit_domain_event_result(
                "com.acme/EventA@1",
                serde_cbor::to_vec(&json!({ "id": "3" })).unwrap(),
            )
            .unwrap();
        kernel_baseline.create_snapshot().unwrap();

        assert_eq!(
            kernel_full.manifest_hash, kernel_baseline.manifest_hash,
            "manifest hash must be identical after baseline+tail and full replay"
        );
        assert_eq!(
            kernel_full.reducer_index_roots, kernel_baseline.reducer_index_roots,
            "cell index roots must be identical after baseline+tail and full replay"
        );
    }

    #[test]
    fn snapshot_restores_cell_index_root_and_cells() {
        let store = Arc::new(MemStore::default());
        let journal = Box::new(MemJournal::new());
        let mut kernel = kernel_with_store_and_journal(store.clone(), journal);
        let reducer = "com.acme/Reducer@1".to_string();
        let key = b"k".to_vec();
        let state_bytes = vec![9u8, 9u8];

        kernel
            .handle_reducer_output(
                reducer.clone(),
                Some(key.clone()),
                true,
                ReducerOutput {
                    state: Some(state_bytes.clone()),
                    ..Default::default()
                },
            )
            .unwrap();
        let root_before = *kernel.reducer_index_roots.get(&reducer).unwrap();

        kernel.create_snapshot().unwrap();
        let entries = kernel.journal.load_from(0).expect("load journal entries");

        let mut kernel2 = {
            let journal = Box::new(MemJournal::from_entries(&entries));
            kernel_with_store_and_journal(store.clone(), journal)
        };
        kernel2.tick_until_idle().unwrap();

        let root_after = *kernel2.reducer_index_roots.get(&reducer).unwrap();
        assert_eq!(root_before, root_after);

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
            cap_name: "cap/http@1".into(),
            params_cbor: vec![1],
            idempotency_key: [2u8; 32],
            origin: IntentOriginRecord::Reducer {
                name: "example/Reducer@1".into(),
                instance_key: None,
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
