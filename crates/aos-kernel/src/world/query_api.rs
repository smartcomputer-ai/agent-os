use super::*;

impl<S: Store + 'static> StateReader for Kernel<S> {
    fn get_reducer_state(
        &self,
        module: &str,
        key: Option<&[u8]>,
        consistency: Consistency,
    ) -> Result<StateRead<Option<Vec<u8>>>, KernelError> {
        let head = self.journal.next_seq();
        match consistency {
            Consistency::Head => {
                return Ok(StateRead {
                    meta: self.read_meta(),
                    value: self.reducer_state_bytes(module, key)?,
                });
            }
            Consistency::AtLeast(h) => {
                if head < h {
                    return Err(KernelError::SnapshotUnavailable(format!(
                        "requested at least height {h}, but head is {head}"
                    )));
                }
                return Ok(StateRead {
                    meta: self.read_meta(),
                    value: self.reducer_state_bytes(module, key)?,
                });
            }
            Consistency::Exact(h) => {
                if h == head {
                    return Ok(StateRead {
                        meta: self.read_meta(),
                        value: self.reducer_state_bytes(module, key)?,
                    });
                }
                if let Some((snap_hash, snap_manifest)) = self.snapshot_at_height(h) {
                    let snapshot = self.load_snapshot_blob(snap_hash)?;
                    let value = self.read_reducer_state_from_snapshot(&snapshot, module, key)?;
                    let meta = ReadMeta {
                        journal_height: h,
                        snapshot_hash: Some(snap_hash),
                        manifest_hash: snap_manifest.unwrap_or(self.manifest_hash),
                        active_baseline_height: self.active_baseline.as_ref().map(|b| b.height),
                        active_baseline_receipt_horizon_height: self
                            .active_baseline
                            .as_ref()
                            .and_then(|b| b.receipt_horizon_height),
                    };
                    return Ok(StateRead { meta, value });
                }
                Err(KernelError::SnapshotUnavailable(format!(
                    "exact height {h} not available; no snapshot and head is {head}"
                )))
            }
        }
    }

    fn get_manifest(&self, consistency: Consistency) -> Result<StateRead<Manifest>, KernelError> {
        let head = self.journal.next_seq();
        match consistency {
            Consistency::Head => {
                return Ok(StateRead {
                    meta: self.read_meta(),
                    value: self.manifest.clone(),
                });
            }
            Consistency::AtLeast(h) => {
                if head < h {
                    return Err(KernelError::SnapshotUnavailable(format!(
                        "requested at least height {h}, but head is {head}"
                    )));
                }
                return Ok(StateRead {
                    meta: self.read_meta(),
                    value: self.manifest.clone(),
                });
            }
            Consistency::Exact(h) => {
                if h == head {
                    return Ok(StateRead {
                        meta: self.read_meta(),
                        value: self.manifest.clone(),
                    });
                }
                if let Some((snap_hash, snap_manifest)) = self.snapshot_at_height(h) {
                    let manifest_hash = snap_manifest.ok_or_else(|| {
                        KernelError::SnapshotUnavailable(
                            "snapshot missing manifest_hash; cannot serve manifest".into(),
                        )
                    })?;
                    let manifest: Manifest = self
                        .store
                        .get_node(manifest_hash)
                        .map_err(|e| KernelError::SnapshotDecode(e.to_string()))?;
                    let meta = ReadMeta {
                        journal_height: h,
                        snapshot_hash: Some(snap_hash),
                        manifest_hash,
                        active_baseline_height: self.active_baseline.as_ref().map(|b| b.height),
                        active_baseline_receipt_horizon_height: self
                            .active_baseline
                            .as_ref()
                            .and_then(|b| b.receipt_horizon_height),
                    };
                    return Ok(StateRead {
                        meta,
                        value: manifest,
                    });
                }
                Err(KernelError::SnapshotUnavailable(format!(
                    "exact height {h} not available; no snapshot and head is {head}"
                )))
            }
        }
    }

    fn get_journal_head(&self) -> ReadMeta {
        self.read_meta()
    }
}

impl<S: Store + 'static> Kernel<S> {
    fn load_snapshot_blob(&self, hash: Hash) -> Result<KernelSnapshot, KernelError> {
        let bytes = self.store.get_blob(hash)?;
        let snapshot: KernelSnapshot = serde_cbor::from_slice(&bytes)
            .map_err(|err| KernelError::SnapshotDecode(err.to_string()))?;
        Ok(snapshot)
    }

    fn read_reducer_state_from_snapshot(
        &self,
        snapshot: &KernelSnapshot,
        reducer: &str,
        key: Option<&[u8]>,
    ) -> Result<Option<Vec<u8>>, KernelError> {
        let key_bytes = key.unwrap_or(MONO_KEY);
        // Preferred path: use index root recorded in snapshot to find cell state in CAS.
        if let Some(root) = snapshot
            .reducer_index_roots()
            .iter()
            .find(|(name, _)| name == reducer)
            .and_then(|(_, bytes)| Hash::from_bytes(bytes).ok())
        {
            let index = CellIndex::new(self.store.as_ref());
            let meta = index.get(root, Hash::of_bytes(key_bytes).as_bytes())?;
            if let Some(meta) = meta {
                let state_hash = Hash::from_bytes(&meta.state_hash)
                    .unwrap_or_else(|_| Hash::of_bytes(&meta.state_hash));
                let state = self.store.get_blob(state_hash)?;
                return Ok(Some(state));
            }
        }

        // Legacy snapshots: fall back to inline entries (monolithic or keyed).
        for entry in snapshot.reducer_state_entries() {
            let entry_key = entry.key.as_deref().unwrap_or(MONO_KEY);
            if entry.reducer == reducer && entry_key == key_bytes {
                return Ok(Some(entry.state.clone()));
            }
        }
        Ok(None)
    }
}
