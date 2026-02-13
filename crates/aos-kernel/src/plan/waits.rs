use aos_air_exec::{Value as ExprValue, eval_expr};
use aos_air_types::{Expr, value_normalize::normalize_cbor_by_name};
use aos_wasm_abi::DomainEvent;

use crate::error::KernelError;
use crate::schema_value::cbor_to_expr_value;

use super::codec::{decode_receipt_value, value_to_bool};
use super::{PlanInstance, ReceiptWait, StepState};

impl PlanInstance {
    pub fn deliver_receipt(
        &mut self,
        intent_hash: [u8; 32],
        payload: &[u8],
    ) -> Result<bool, KernelError> {
        if let Some(wait) = self.receipt_waits.remove(&intent_hash) {
            let value = decode_receipt_value(payload);
            self.receipt_values.insert(wait.step_id.clone(), value);
            self.step_states
                .insert(wait.step_id.clone(), StepState::Pending);
            return Ok(true);
        }
        Ok(false)
    }

    pub fn pending_receipt_hash(&self) -> Option<[u8; 32]> {
        self.receipt_waits.keys().next().copied()
    }

    pub fn pending_receipt_hashes(&self) -> Vec<[u8; 32]> {
        self.receipt_waits.keys().copied().collect()
    }

    pub fn override_pending_receipt_hash(&mut self, hash: [u8; 32]) {
        if self.receipt_waits.contains_key(&hash) {
            return;
        }
        if self.receipt_waits.len() == 1 {
            if let Some(mut wait) = self.receipt_waits.values().next().cloned() {
                self.receipt_waits.clear();
                wait.intent_hash = hash;
                self.receipt_waits.insert(hash, wait);
            }
        }
    }

    pub fn waiting_on_receipt(&self, hash: [u8; 32]) -> bool {
        self.receipt_waits.contains_key(&hash)
    }

    pub fn deliver_event(&mut self, event: &DomainEvent) -> Result<bool, KernelError> {
        if let Some(wait) = &self.event_wait {
            if wait.schema == event.schema {
                let normalized =
                    normalize_cbor_by_name(&self.schema_index, wait.schema.as_str(), &event.value)
                        .map_err(|err| {
                            KernelError::Manifest(format!("await_event payload decode error: {err}"))
                        })?;
                let schema = self.schema_index.get(wait.schema.as_str()).ok_or_else(|| {
                    KernelError::Manifest(format!("schema '{}' not found for await_event", wait.schema))
                })?;
                let value = cbor_to_expr_value(&normalized.value, schema, &self.schema_index)?;
                if let Some(predicate) = &wait.where_clause {
                    let prev = self.env.push_event(value.clone());
                    let passes = eval_expr(predicate, &self.env).map_err(|err| {
                        KernelError::Manifest(format!("await_event where eval error: {err}"))
                    })?;
                    self.env.restore_event(prev);
                    if !value_to_bool(passes)? {
                        return Ok(false);
                    }
                }
                self.event_value = Some(value);
                self.step_states
                    .insert(wait.step_id.clone(), StepState::Pending);
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn waiting_event_schema(&self) -> Option<&str> {
        self.event_wait.as_ref().map(|w| w.schema.as_str())
    }

    pub(super) fn register_receipt_wait(
        &mut self,
        step_id: String,
        handle_expr: &Expr,
    ) -> Result<[u8; 32], KernelError> {
        let handle_value = eval_expr(handle_expr, &self.env)
            .map_err(|err| KernelError::Manifest(format!("plan await eval error: {err}")))?;
        let handle = match handle_value {
            ExprValue::Text(s) => s,
            _ => {
                return Err(KernelError::Manifest(
                    "await_receipt expects handle text".into(),
                ));
            }
        };
        let intent_hash = *self
            .effect_handles
            .get(&handle)
            .ok_or_else(|| KernelError::Manifest(format!("unknown effect handle '{handle}'")))?;
        let wait = ReceiptWait {
            step_id: step_id.clone(),
            intent_hash,
        };
        self.receipt_waits.insert(intent_hash, wait);
        self.step_states.insert(step_id, StepState::WaitingReceipt);
        Ok(intent_hash)
    }
}
