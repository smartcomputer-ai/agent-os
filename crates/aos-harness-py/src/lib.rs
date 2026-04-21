mod compat_harness;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use aos_authoring::WorkflowBuildProfile;
use aos_cbor::{Hash, to_canonical_cbor};
use aos_effects::builtins::{
    BlobGetReceipt, BlobPutReceipt, HashRef, HttpRequestReceipt, LlmFinishReason,
    LlmGenerateReceipt, RequestTimings, TokenUsage,
};
use aos_effects::{EffectIntent, EffectReceipt, ReceiptStatus};
use aos_kernel::MemStore;
use compat_harness::{
    CycleOutcome, EffectMode, HarnessArtifacts, HarnessCell, HostError, NodeRuntimeWorldHarness,
    QuiescenceStatus, RuntimeWorkflowHarness, bootstrap_node_world_harness,
    build_runtime_workflow_harness_from_authored_paths_with_secret_config,
};
use pyo3::exceptions::{PyRuntimeError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyModule};
use serde::Serialize;
use serde_json::{Map as JsonMap, Number as JsonNumber, Value as JsonValue};
use tempfile::TempDir;

fn py_runtime_error(message: impl Into<String>) -> PyErr {
    PyRuntimeError::new_err(message.into())
}

fn host_to_py_error(err: HostError) -> PyErr {
    py_runtime_error(err.to_string())
}

fn parse_effect_mode(value: &str) -> PyResult<EffectMode> {
    match value {
        "scripted" => Ok(EffectMode::Scripted),
        "twin" => Ok(EffectMode::Twin),
        "live" => Ok(EffectMode::Live),
        other => Err(PyValueError::new_err(format!(
            "unsupported effect mode '{other}', expected scripted|twin|live"
        ))),
    }
}

fn parse_build_profile(value: &str) -> PyResult<WorkflowBuildProfile> {
    match value {
        "debug" => Ok(WorkflowBuildProfile::Debug),
        "release" => Ok(WorkflowBuildProfile::Release),
        other => Err(PyValueError::new_err(format!(
            "unsupported build_profile '{other}' (expected 'debug' or 'release')"
        ))),
    }
}

fn parse_receipt_status(value: &str) -> PyResult<ReceiptStatus> {
    match value {
        "ok" => Ok(ReceiptStatus::Ok),
        "error" => Ok(ReceiptStatus::Error),
        "timeout" => Ok(ReceiptStatus::Timeout),
        other => Err(PyValueError::new_err(format!(
            "unsupported receipt status '{other}', expected ok|error|timeout"
        ))),
    }
}

fn py_any_to_json(value: &Bound<'_, PyAny>) -> PyResult<JsonValue> {
    if value.downcast::<PyBytes>().is_ok() {
        return Err(PyTypeError::new_err(
            "bytes are not accepted in JSON-bound methods; use *_bytes or apply_receipt instead",
        ));
    }
    let json = PyModule::import(value.py(), "json")?;
    let dumped = json.call_method1("dumps", (value,))?;
    let dumped: String = dumped.extract()?;
    serde_json::from_str(&dumped)
        .map_err(|err| PyValueError::new_err(format!("failed to decode JSON from Python: {err}")))
}

fn json_to_py(py: Python<'_>, value: &JsonValue) -> PyResult<Py<PyAny>> {
    let json = PyModule::import(py, "json")?;
    let dumped = serde_json::to_string(value).map_err(|err| py_runtime_error(err.to_string()))?;
    Ok(json.call_method1("loads", (dumped,))?.unbind())
}

fn serialize_to_py(py: Python<'_>, value: &impl Serialize) -> PyResult<Py<PyAny>> {
    let json = serde_json::to_value(value).map_err(|err| py_runtime_error(err.to_string()))?;
    json_to_py(py, &json)
}

fn parse_hash_ref(value: &str) -> PyResult<HashRef> {
    HashRef::new(value.to_string())
        .map_err(|err| PyValueError::new_err(format!("invalid hash ref '{value}': {err}")))
}

fn parse_intent_hash(value: Vec<u8>) -> PyResult<[u8; 32]> {
    value
        .try_into()
        .map_err(|_| PyValueError::new_err("intent_hash must be exactly 32 bytes"))
}

fn parse_secret_binding_bytes(value: &Bound<'_, PyAny>, binding_id: &str) -> PyResult<Vec<u8>> {
    if let Ok(bytes) = value.extract::<Vec<u8>>() {
        return Ok(bytes);
    }
    if let Ok(text) = value.extract::<String>() {
        return Ok(text.into_bytes());
    }
    Err(PyTypeError::new_err(format!(
        "secret_bindings['{binding_id}'] must be bytes, bytearray, or str"
    )))
}

fn parse_secret_bindings(
    secret_bindings: Option<&Bound<'_, PyDict>>,
) -> PyResult<Option<HashMap<String, Vec<u8>>>> {
    let Some(secret_bindings) = secret_bindings else {
        return Ok(None);
    };
    let mut bindings = HashMap::new();
    for (key, value) in secret_bindings.iter() {
        let binding_id: String = key
            .extract()
            .map_err(|_| PyTypeError::new_err("secret_bindings keys must be binding_id strings"))?;
        bindings.insert(
            binding_id.clone(),
            parse_secret_binding_bytes(&value, &binding_id)?,
        );
    }
    Ok(Some(bindings))
}

fn cell_meta_to_py(py: Python<'_>, cell: &HarnessCell) -> PyResult<Py<PyAny>> {
    let dict = PyDict::new(py);
    dict.set_item(
        "key_hash",
        Hash::from_bytes(&cell.key_hash)
            .expect("cell key hash")
            .to_hex(),
    )?;
    dict.set_item("key_bytes", PyBytes::new(py, &cell.key_bytes))?;
    dict.set_item("state_hash", &cell.state_hash)?;
    dict.set_item("size", cell.size)?;
    dict.set_item("last_active_ns", cell.last_active_ns)?;
    Ok(dict.into_any().unbind())
}

fn cell_list_to_py(py: Python<'_>, cells: &[HarnessCell]) -> PyResult<Py<PyAny>> {
    let list = pyo3::types::PyList::empty(py);
    for cell in cells {
        list.append(cell_meta_to_py(py, cell)?)?;
    }
    Ok(list.into_any().unbind())
}

#[pyfunction]
fn canonical_cbor(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<Py<PyBytes>> {
    let json = py_any_to_json(value)?;
    let bytes = to_canonical_cbor(&json).map_err(|err| py_runtime_error(err.to_string()))?;
    Ok(PyBytes::new(py, &bytes).unbind())
}

fn receipt_status_name(status: &ReceiptStatus) -> &'static str {
    match status {
        ReceiptStatus::Ok => "ok",
        ReceiptStatus::Error => "error",
        ReceiptStatus::Timeout => "timeout",
    }
}

fn receipt_to_py(py: Python<'_>, receipt: &EffectReceipt) -> PyResult<Py<PyAny>> {
    let dict = PyDict::new(py);
    dict.set_item("intent_hash", PyBytes::new(py, &receipt.intent_hash))?;
    dict.set_item("adapter_id", &receipt.adapter_id)?;
    dict.set_item("status", receipt_status_name(&receipt.status))?;
    dict.set_item("payload_cbor", PyBytes::new(py, &receipt.payload_cbor))?;
    dict.set_item("cost_cents", receipt.cost_cents)?;
    dict.set_item("signature", PyBytes::new(py, &receipt.signature))?;
    Ok(dict.into_any().unbind())
}

fn receipt_from_py(receipt: &Bound<'_, PyAny>) -> PyResult<EffectReceipt> {
    let receipt = receipt
        .downcast::<PyDict>()
        .map_err(|_| PyTypeError::new_err("receipt must be a dict"))?;
    let intent_hash: Vec<u8> = receipt
        .get_item("intent_hash")?
        .ok_or_else(|| PyValueError::new_err("receipt.intent_hash is required"))?
        .extract()?;
    let adapter_id: String = receipt
        .get_item("adapter_id")?
        .ok_or_else(|| PyValueError::new_err("receipt.adapter_id is required"))?
        .extract()?;
    let status: String = receipt
        .get_item("status")?
        .ok_or_else(|| PyValueError::new_err("receipt.status is required"))?
        .extract()?;
    let payload_cbor: Vec<u8> = receipt
        .get_item("payload_cbor")?
        .ok_or_else(|| PyValueError::new_err("receipt.payload_cbor is required"))?
        .extract()?;
    let cost_cents: Option<u64> = receipt
        .get_item("cost_cents")?
        .map(|value| value.extract())
        .transpose()?;
    let signature: Vec<u8> = receipt
        .get_item("signature")?
        .map(|value| value.extract())
        .transpose()?
        .unwrap_or_default();
    Ok(EffectReceipt {
        intent_hash: parse_intent_hash(intent_hash)?,
        adapter_id,
        status: parse_receipt_status(&status)?,
        payload_cbor,
        cost_cents,
        signature,
    })
}

trait CommonHarnessOps {
    fn send_event(&mut self, schema: &str, json_value: JsonValue) -> Result<(), HostError>;
    fn send_command(&mut self, schema: &str, json_value: JsonValue) -> Result<(), HostError>;
    fn run_cycle_batch(&mut self) -> Result<CycleOutcome, HostError>;
    fn run_until_kernel_idle(&mut self) -> Result<QuiescenceStatus, HostError>;
    fn run_until_runtime_quiescent(&mut self) -> Result<QuiescenceStatus, HostError>;
    fn quiescence_status(&self) -> QuiescenceStatus;
    fn pull_effects(&mut self) -> Result<Vec<EffectIntent>, HostError>;
    fn apply_receipt(&mut self, receipt: EffectReceipt) -> Result<(), HostError>;
    fn snapshot(&mut self) -> Result<(), HostError>;
    fn trace_summary(&self) -> Result<JsonValue, HostError>;
    fn time_get(&self) -> u64;
    fn time_set(&mut self, now_ns: u64) -> u64;
    fn time_advance(&mut self, delta_ns: u64) -> u64;
    fn time_jump_next_due(&mut self) -> Result<Option<u64>, HostError>;
    fn export_artifacts(&self) -> Result<HarnessArtifacts, HostError>;
    fn receipt_ok(
        &self,
        intent_hash: [u8; 32],
        adapter_id: &str,
        payload: &JsonValue,
    ) -> Result<EffectReceipt, HostError>;
    fn receipt_error(
        &self,
        intent_hash: [u8; 32],
        adapter_id: &str,
        payload: &JsonValue,
    ) -> Result<EffectReceipt, HostError>;
    fn receipt_timeout(
        &self,
        intent_hash: [u8; 32],
        adapter_id: &str,
        payload: &JsonValue,
    ) -> Result<EffectReceipt, HostError>;
    fn receipt_timer_set_ok(
        &self,
        intent_hash: [u8; 32],
        delivered_at_ns: u64,
        key: Option<String>,
    ) -> Result<EffectReceipt, HostError>;
    fn receipt_blob_put_ok(
        &self,
        intent_hash: [u8; 32],
        payload: &BlobPutReceipt,
    ) -> Result<EffectReceipt, HostError>;
    fn receipt_blob_get_ok(
        &self,
        intent_hash: [u8; 32],
        payload: &BlobGetReceipt,
    ) -> Result<EffectReceipt, HostError>;
    fn receipt_llm_generate_ok(
        &self,
        intent_hash: [u8; 32],
        payload: &LlmGenerateReceipt,
    ) -> Result<EffectReceipt, HostError>;
}

impl CommonHarnessOps for NodeRuntimeWorldHarness {
    fn send_event(&mut self, schema: &str, json_value: JsonValue) -> Result<(), HostError> {
        self.send_event(schema, json_value)
    }

    fn send_command(&mut self, schema: &str, json_value: JsonValue) -> Result<(), HostError> {
        self.send_command(schema, json_value)
    }

    fn run_cycle_batch(&mut self) -> Result<CycleOutcome, HostError> {
        self.run_cycle_batch()
    }

    fn run_until_kernel_idle(&mut self) -> Result<QuiescenceStatus, HostError> {
        self.run_until_kernel_idle()
    }

    fn run_until_runtime_quiescent(&mut self) -> Result<QuiescenceStatus, HostError> {
        self.run_until_runtime_quiescent()
    }

    fn quiescence_status(&self) -> QuiescenceStatus {
        self.quiescence_status()
    }

    fn pull_effects(&mut self) -> Result<Vec<EffectIntent>, HostError> {
        self.pull_effects()
    }

    fn apply_receipt(&mut self, receipt: EffectReceipt) -> Result<(), HostError> {
        self.apply_receipt(receipt)
    }

    fn snapshot(&mut self) -> Result<(), HostError> {
        self.snapshot()
    }

    fn trace_summary(&self) -> Result<JsonValue, HostError> {
        self.trace_summary()
    }

    fn time_get(&self) -> u64 {
        self.time_get()
    }

    fn time_set(&mut self, now_ns: u64) -> u64 {
        self.time_set(now_ns)
    }

    fn time_advance(&mut self, delta_ns: u64) -> u64 {
        self.time_advance(delta_ns)
    }

    fn time_jump_next_due(&mut self) -> Result<Option<u64>, HostError> {
        self.time_jump_next_due()
    }

    fn export_artifacts(&self) -> Result<HarnessArtifacts, HostError> {
        self.export_artifacts()
    }

    fn receipt_ok(
        &self,
        intent_hash: [u8; 32],
        adapter_id: &str,
        payload: &JsonValue,
    ) -> Result<EffectReceipt, HostError> {
        self.receipt_ok(intent_hash, adapter_id, payload)
    }

    fn receipt_error(
        &self,
        intent_hash: [u8; 32],
        adapter_id: &str,
        payload: &JsonValue,
    ) -> Result<EffectReceipt, HostError> {
        self.receipt_error(intent_hash, adapter_id, payload)
    }

    fn receipt_timeout(
        &self,
        intent_hash: [u8; 32],
        adapter_id: &str,
        payload: &JsonValue,
    ) -> Result<EffectReceipt, HostError> {
        self.receipt_timeout(intent_hash, adapter_id, payload)
    }

    fn receipt_timer_set_ok(
        &self,
        intent_hash: [u8; 32],
        delivered_at_ns: u64,
        key: Option<String>,
    ) -> Result<EffectReceipt, HostError> {
        self.receipt_timer_set_ok(intent_hash, delivered_at_ns, key)
    }

    fn receipt_blob_put_ok(
        &self,
        intent_hash: [u8; 32],
        payload: &BlobPutReceipt,
    ) -> Result<EffectReceipt, HostError> {
        self.receipt_blob_put_ok(intent_hash, payload)
    }

    fn receipt_blob_get_ok(
        &self,
        intent_hash: [u8; 32],
        payload: &BlobGetReceipt,
    ) -> Result<EffectReceipt, HostError> {
        self.receipt_blob_get_ok(intent_hash, payload)
    }

    fn receipt_llm_generate_ok(
        &self,
        intent_hash: [u8; 32],
        payload: &LlmGenerateReceipt,
    ) -> Result<EffectReceipt, HostError> {
        self.receipt_llm_generate_ok(intent_hash, payload)
    }
}

impl CommonHarnessOps for RuntimeWorkflowHarness<MemStore> {
    fn send_event(&mut self, schema: &str, json_value: JsonValue) -> Result<(), HostError> {
        self.send_event(schema, json_value)
    }

    fn send_command(&mut self, schema: &str, json_value: JsonValue) -> Result<(), HostError> {
        self.send_command(schema, json_value)
    }

    fn run_cycle_batch(&mut self) -> Result<CycleOutcome, HostError> {
        self.run_cycle_batch()
    }

    fn run_until_kernel_idle(&mut self) -> Result<QuiescenceStatus, HostError> {
        self.run_until_kernel_idle()
    }

    fn run_until_runtime_quiescent(&mut self) -> Result<QuiescenceStatus, HostError> {
        self.run_until_runtime_quiescent()
    }

    fn quiescence_status(&self) -> QuiescenceStatus {
        self.quiescence_status()
    }

    fn pull_effects(&mut self) -> Result<Vec<EffectIntent>, HostError> {
        self.pull_effects()
    }

    fn apply_receipt(&mut self, receipt: EffectReceipt) -> Result<(), HostError> {
        self.apply_receipt(receipt)
    }

    fn snapshot(&mut self) -> Result<(), HostError> {
        self.snapshot()
    }

    fn trace_summary(&self) -> Result<JsonValue, HostError> {
        self.trace_summary()
    }

    fn time_get(&self) -> u64 {
        self.time_get()
    }

    fn time_set(&mut self, now_ns: u64) -> u64 {
        self.time_set(now_ns)
    }

    fn time_advance(&mut self, delta_ns: u64) -> u64 {
        self.time_advance(delta_ns)
    }

    fn time_jump_next_due(&mut self) -> Result<Option<u64>, HostError> {
        self.time_jump_next_due()
    }

    fn export_artifacts(&self) -> Result<HarnessArtifacts, HostError> {
        self.export_artifacts()
    }

    fn receipt_ok(
        &self,
        intent_hash: [u8; 32],
        adapter_id: &str,
        payload: &JsonValue,
    ) -> Result<EffectReceipt, HostError> {
        self.receipt_ok(intent_hash, adapter_id, payload)
    }

    fn receipt_error(
        &self,
        intent_hash: [u8; 32],
        adapter_id: &str,
        payload: &JsonValue,
    ) -> Result<EffectReceipt, HostError> {
        self.receipt_error(intent_hash, adapter_id, payload)
    }

    fn receipt_timeout(
        &self,
        intent_hash: [u8; 32],
        adapter_id: &str,
        payload: &JsonValue,
    ) -> Result<EffectReceipt, HostError> {
        self.receipt_timeout(intent_hash, adapter_id, payload)
    }

    fn receipt_timer_set_ok(
        &self,
        intent_hash: [u8; 32],
        delivered_at_ns: u64,
        key: Option<String>,
    ) -> Result<EffectReceipt, HostError> {
        self.receipt_timer_set_ok(intent_hash, delivered_at_ns, key)
    }

    fn receipt_blob_put_ok(
        &self,
        intent_hash: [u8; 32],
        payload: &BlobPutReceipt,
    ) -> Result<EffectReceipt, HostError> {
        self.receipt_blob_put_ok(intent_hash, payload)
    }

    fn receipt_blob_get_ok(
        &self,
        intent_hash: [u8; 32],
        payload: &BlobGetReceipt,
    ) -> Result<EffectReceipt, HostError> {
        self.receipt_blob_get_ok(intent_hash, payload)
    }

    fn receipt_llm_generate_ok(
        &self,
        intent_hash: [u8; 32],
        payload: &LlmGenerateReceipt,
    ) -> Result<EffectReceipt, HostError> {
        self.receipt_llm_generate_ok(intent_hash, payload)
    }
}

fn lock_harness<'a, H>(mutex: &'a Mutex<H>, label: &'static str) -> PyResult<MutexGuard<'a, H>> {
    mutex
        .lock()
        .map_err(|_| py_runtime_error(format!("{label} lock poisoned")))
}

fn common_send_event<H: CommonHarnessOps>(
    mutex: &Mutex<H>,
    label: &'static str,
    schema: &str,
    value: &Bound<'_, PyAny>,
) -> PyResult<()> {
    lock_harness(mutex, label)?
        .send_event(schema, py_any_to_json(value)?)
        .map_err(host_to_py_error)
}

fn common_send_command<H: CommonHarnessOps>(
    mutex: &Mutex<H>,
    label: &'static str,
    schema: &str,
    value: &Bound<'_, PyAny>,
) -> PyResult<()> {
    lock_harness(mutex, label)?
        .send_command(schema, py_any_to_json(value)?)
        .map_err(host_to_py_error)
}

fn common_run_cycle_batch<H: CommonHarnessOps>(
    mutex: &Mutex<H>,
    label: &'static str,
    py: Python<'_>,
) -> PyResult<Py<PyAny>> {
    let cycle = lock_harness(mutex, label)?
        .run_cycle_batch()
        .map_err(host_to_py_error)?;
    serialize_to_py(py, &cycle)
}

fn common_run_to_idle<H: CommonHarnessOps>(
    mutex: &Mutex<H>,
    label: &'static str,
    py: Python<'_>,
) -> PyResult<Py<PyAny>> {
    let status = lock_harness(mutex, label)?
        .run_until_kernel_idle()
        .map_err(host_to_py_error)?;
    serialize_to_py(py, &status)
}

fn common_run_until_runtime_quiescent<H: CommonHarnessOps>(
    mutex: &Mutex<H>,
    label: &'static str,
    py: Python<'_>,
) -> PyResult<Py<PyAny>> {
    let status = lock_harness(mutex, label)?
        .run_until_runtime_quiescent()
        .map_err(host_to_py_error)?;
    serialize_to_py(py, &status)
}

fn common_quiescence_status<H: CommonHarnessOps>(
    mutex: &Mutex<H>,
    label: &'static str,
    py: Python<'_>,
) -> PyResult<Py<PyAny>> {
    let status = lock_harness(mutex, label)?.quiescence_status();
    serialize_to_py(py, &status)
}

fn common_pull_effects<H: CommonHarnessOps>(
    mutex: &Mutex<H>,
    label: &'static str,
    py: Python<'_>,
) -> PyResult<Py<PyAny>> {
    let intents = lock_harness(mutex, label)?
        .pull_effects()
        .map_err(host_to_py_error)?;
    let effects = intents
        .iter()
        .map(effect_intent_to_py_json)
        .collect::<PyResult<Vec<_>>>()?;
    json_to_py(py, &JsonValue::Array(effects))
}

fn effect_intent_to_py_json(intent: &EffectIntent) -> PyResult<JsonValue> {
    let params = serde_cbor::from_slice::<serde_cbor::Value>(&intent.params_cbor)
        .map(cbor_value_to_json)
        .map_err(|err| py_runtime_error(format!("decode effect params: {err}")))?;
    let mut object = JsonMap::new();
    object.insert(
        "kind".to_string(),
        JsonValue::String(intent.kind.to_string()),
    );
    object.insert(
        "intent_hash".to_string(),
        JsonValue::Array(
            intent
                .intent_hash
                .iter()
                .map(|byte| JsonValue::Number(JsonNumber::from(*byte)))
                .collect(),
        ),
    );
    object.insert("params".to_string(), params);
    Ok(JsonValue::Object(object))
}

fn cbor_value_to_json(value: serde_cbor::Value) -> JsonValue {
    match value {
        serde_cbor::Value::Null => JsonValue::Null,
        serde_cbor::Value::Bool(value) => JsonValue::Bool(value),
        serde_cbor::Value::Integer(value) => i64::try_from(value)
            .ok()
            .map(JsonNumber::from)
            .map(JsonValue::Number)
            .unwrap_or_else(|| JsonValue::String(value.to_string())),
        serde_cbor::Value::Float(value) => JsonNumber::from_f64(value)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
        serde_cbor::Value::Bytes(bytes) => JsonValue::Array(
            bytes
                .into_iter()
                .map(|byte| JsonValue::Number(JsonNumber::from(byte)))
                .collect(),
        ),
        serde_cbor::Value::Text(value) => JsonValue::String(value),
        serde_cbor::Value::Array(items) => {
            JsonValue::Array(items.into_iter().map(cbor_value_to_json).collect())
        }
        serde_cbor::Value::Map(entries) => {
            let mut object = JsonMap::new();
            for (key, value) in entries {
                object.insert(cbor_map_key_to_string(key), cbor_value_to_json(value));
            }
            JsonValue::Object(object)
        }
        serde_cbor::Value::Tag(_, inner) => cbor_value_to_json(*inner),
        _ => JsonValue::Null,
    }
}

fn cbor_map_key_to_string(key: serde_cbor::Value) -> String {
    match key {
        serde_cbor::Value::Text(value) => value,
        other => cbor_value_to_json(other).to_string(),
    }
}

fn common_apply_receipt_object<H: CommonHarnessOps>(
    mutex: &Mutex<H>,
    label: &'static str,
    receipt: &Bound<'_, PyAny>,
) -> PyResult<()> {
    let receipt = receipt_from_py(receipt)?;
    lock_harness(mutex, label)?
        .apply_receipt(receipt)
        .map_err(host_to_py_error)
}

fn common_apply_receipt<H: CommonHarnessOps>(
    mutex: &Mutex<H>,
    label: &'static str,
    intent_hash: Vec<u8>,
    adapter_id: &str,
    status: &str,
    payload_cbor: Vec<u8>,
    cost_cents: Option<u64>,
    signature: Option<Vec<u8>>,
) -> PyResult<()> {
    let receipt = EffectReceipt {
        intent_hash: parse_intent_hash(intent_hash)?,
        adapter_id: adapter_id.to_string(),
        status: parse_receipt_status(status)?,
        payload_cbor,
        cost_cents,
        signature: signature.unwrap_or_default(),
    };
    lock_harness(mutex, label)?
        .apply_receipt(receipt)
        .map_err(host_to_py_error)
}

fn common_receipt_ok<H: CommonHarnessOps>(
    mutex: &Mutex<H>,
    label: &'static str,
    py: Python<'_>,
    intent_hash: Vec<u8>,
    adapter_id: &str,
    payload: &Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    let receipt = lock_harness(mutex, label)?
        .receipt_ok(
            parse_intent_hash(intent_hash)?,
            adapter_id,
            &py_any_to_json(payload)?,
        )
        .map_err(host_to_py_error)?;
    receipt_to_py(py, &receipt)
}

fn common_receipt_error<H: CommonHarnessOps>(
    mutex: &Mutex<H>,
    label: &'static str,
    py: Python<'_>,
    intent_hash: Vec<u8>,
    adapter_id: &str,
    payload: &Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    let receipt = lock_harness(mutex, label)?
        .receipt_error(
            parse_intent_hash(intent_hash)?,
            adapter_id,
            &py_any_to_json(payload)?,
        )
        .map_err(host_to_py_error)?;
    receipt_to_py(py, &receipt)
}

fn common_receipt_timeout<H: CommonHarnessOps>(
    mutex: &Mutex<H>,
    label: &'static str,
    py: Python<'_>,
    intent_hash: Vec<u8>,
    adapter_id: &str,
    payload: &Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    let receipt = lock_harness(mutex, label)?
        .receipt_timeout(
            parse_intent_hash(intent_hash)?,
            adapter_id,
            &py_any_to_json(payload)?,
        )
        .map_err(host_to_py_error)?;
    receipt_to_py(py, &receipt)
}

fn common_receipt_timer_set_ok<H: CommonHarnessOps>(
    mutex: &Mutex<H>,
    label: &'static str,
    py: Python<'_>,
    intent_hash: Vec<u8>,
    delivered_at_ns: u64,
    key: Option<String>,
) -> PyResult<Py<PyAny>> {
    let receipt = lock_harness(mutex, label)?
        .receipt_timer_set_ok(parse_intent_hash(intent_hash)?, delivered_at_ns, key)
        .map_err(host_to_py_error)?;
    receipt_to_py(py, &receipt)
}

fn common_receipt_blob_put_ok<H: CommonHarnessOps>(
    mutex: &Mutex<H>,
    label: &'static str,
    py: Python<'_>,
    intent_hash: Vec<u8>,
    blob_ref: &str,
    edge_ref: &str,
    size: u64,
) -> PyResult<Py<PyAny>> {
    let payload = BlobPutReceipt {
        blob_ref: parse_hash_ref(blob_ref)?,
        edge_ref: parse_hash_ref(edge_ref)?,
        size,
    };
    let receipt = lock_harness(mutex, label)?
        .receipt_blob_put_ok(parse_intent_hash(intent_hash)?, &payload)
        .map_err(host_to_py_error)?;
    receipt_to_py(py, &receipt)
}

fn common_receipt_blob_get_ok<H: CommonHarnessOps>(
    mutex: &Mutex<H>,
    label: &'static str,
    py: Python<'_>,
    intent_hash: Vec<u8>,
    blob_ref: &str,
    bytes: Vec<u8>,
    size: Option<u64>,
) -> PyResult<Py<PyAny>> {
    let payload = BlobGetReceipt {
        blob_ref: parse_hash_ref(blob_ref)?,
        size: size.unwrap_or(bytes.len() as u64),
        bytes,
    };
    let receipt = lock_harness(mutex, label)?
        .receipt_blob_get_ok(parse_intent_hash(intent_hash)?, &payload)
        .map_err(host_to_py_error)?;
    receipt_to_py(py, &receipt)
}

fn common_receipt_http_request_ok<H: CommonHarnessOps>(
    mutex: &Mutex<H>,
    label: &'static str,
    py: Python<'_>,
    intent_hash: Vec<u8>,
    status: i32,
    adapter_id: &str,
    headers: Option<&Bound<'_, PyAny>>,
    body_ref: Option<&str>,
    start_ns: Option<u64>,
    end_ns: Option<u64>,
) -> PyResult<Py<PyAny>> {
    let current_time_ns = { lock_harness(mutex, label)?.time_get() };
    let headers = headers
        .map(py_any_to_json)
        .transpose()?
        .map(serde_json::from_value)
        .transpose()
        .map_err(|err| PyValueError::new_err(format!("invalid http headers: {err}")))?
        .unwrap_or_default();
    let payload = HttpRequestReceipt {
        status,
        headers,
        body_ref: body_ref.map(parse_hash_ref).transpose()?,
        timings: RequestTimings {
            start_ns: start_ns.unwrap_or(current_time_ns),
            end_ns: end_ns.unwrap_or(current_time_ns),
        },
        adapter_id: adapter_id.to_string(),
    };
    let receipt = lock_harness(mutex, label)?
        .receipt_ok(
            parse_intent_hash(intent_hash)?,
            adapter_id,
            &serde_json::to_value(payload).map_err(|err| py_runtime_error(err.to_string()))?,
        )
        .map_err(host_to_py_error)?;
    receipt_to_py(py, &receipt)
}

fn common_receipt_llm_generate_ok<H: CommonHarnessOps>(
    mutex: &Mutex<H>,
    label: &'static str,
    py: Python<'_>,
    intent_hash: Vec<u8>,
    output_ref: &str,
    provider_id: &str,
    finish_reason: &str,
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: Option<u64>,
    raw_output_ref: Option<&str>,
    provider_response_id: Option<String>,
    cost_cents: Option<u64>,
    warnings_ref: Option<&str>,
    rate_limit_ref: Option<&str>,
) -> PyResult<Py<PyAny>> {
    let payload = LlmGenerateReceipt {
        output_ref: parse_hash_ref(output_ref)?,
        raw_output_ref: raw_output_ref.map(parse_hash_ref).transpose()?,
        provider_response_id,
        finish_reason: LlmFinishReason {
            reason: finish_reason.to_string(),
            raw: None,
        },
        token_usage: TokenUsage {
            prompt: prompt_tokens,
            completion: completion_tokens,
            total: total_tokens,
        },
        usage_details: None,
        warnings_ref: warnings_ref.map(parse_hash_ref).transpose()?,
        rate_limit_ref: rate_limit_ref.map(parse_hash_ref).transpose()?,
        cost_cents,
        provider_id: provider_id.to_string(),
    };
    let receipt = lock_harness(mutex, label)?
        .receipt_llm_generate_ok(parse_intent_hash(intent_hash)?, &payload)
        .map_err(host_to_py_error)?;
    receipt_to_py(py, &receipt)
}

fn common_trace_summary<H: CommonHarnessOps>(
    mutex: &Mutex<H>,
    label: &'static str,
    py: Python<'_>,
) -> PyResult<Py<PyAny>> {
    let summary = lock_harness(mutex, label)?
        .trace_summary()
        .map_err(host_to_py_error)?;
    json_to_py(py, &summary)
}

fn common_snapshot_create<H: CommonHarnessOps>(
    mutex: &Mutex<H>,
    label: &'static str,
) -> PyResult<()> {
    lock_harness(mutex, label)?
        .snapshot()
        .map_err(host_to_py_error)
}

fn common_time_get<H: CommonHarnessOps>(mutex: &Mutex<H>, label: &'static str) -> PyResult<u64> {
    Ok(lock_harness(mutex, label)?.time_get())
}

fn common_time_set<H: CommonHarnessOps>(
    mutex: &Mutex<H>,
    label: &'static str,
    now_ns: u64,
) -> PyResult<u64> {
    Ok(lock_harness(mutex, label)?.time_set(now_ns))
}

fn common_time_advance<H: CommonHarnessOps>(
    mutex: &Mutex<H>,
    label: &'static str,
    delta_ns: u64,
) -> PyResult<u64> {
    Ok(lock_harness(mutex, label)?.time_advance(delta_ns))
}

fn common_time_jump_next_due<H: CommonHarnessOps>(
    mutex: &Mutex<H>,
    label: &'static str,
) -> PyResult<Option<u64>> {
    lock_harness(mutex, label)?
        .time_jump_next_due()
        .map_err(host_to_py_error)
}

fn common_artifact_export<H: CommonHarnessOps>(
    mutex: &Mutex<H>,
    label: &'static str,
    py: Python<'_>,
) -> PyResult<Py<PyAny>> {
    let artifacts = lock_harness(mutex, label)?
        .export_artifacts()
        .map_err(host_to_py_error)?;
    serialize_to_py(py, &artifacts)
}

#[pyclass(name = "WorldHarness", unsendable)]
struct PyWorldHarness {
    inner: Mutex<NodeRuntimeWorldHarness>,
    world_id: String,
    warnings: Vec<String>,
}

impl PyWorldHarness {
    const LABEL: &'static str = "world harness";

    fn lock(&self) -> PyResult<MutexGuard<'_, NodeRuntimeWorldHarness>> {
        lock_harness(&self.inner, Self::LABEL)
    }

    fn from_world_dir_inner(
        world_root: &str,
        reset: bool,
        force_build: bool,
        sync_secrets: bool,
        effect_mode: EffectMode,
    ) -> PyResult<Self> {
        let world_root = Path::new(world_root);
        let boot =
            bootstrap_node_world_harness(world_root, reset, force_build, sync_secrets, effect_mode)
                .map_err(|err| py_runtime_error(err.to_string()))?;
        Ok(Self {
            inner: Mutex::new(boot.harness),
            world_id: boot.world_id.to_string(),
            warnings: boot.warnings,
        })
    }
}

#[pymethods]
impl PyWorldHarness {
    #[staticmethod]
    #[pyo3(signature=(world_root, reset=false, force_build=false, sync_secrets=false, effect_mode="scripted"))]
    fn from_world_dir(
        world_root: &str,
        reset: bool,
        force_build: bool,
        sync_secrets: bool,
        effect_mode: &str,
    ) -> PyResult<Self> {
        Self::from_world_dir_inner(
            world_root,
            reset,
            force_build,
            sync_secrets,
            parse_effect_mode(effect_mode)?,
        )
    }

    #[getter]
    fn world_id(&self) -> String {
        self.world_id.clone()
    }

    #[getter]
    fn warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }

    fn send_event(&self, schema: &str, value: &Bound<'_, PyAny>) -> PyResult<()> {
        common_send_event(&self.inner, Self::LABEL, schema, value)
    }

    fn send_command(&self, schema: &str, value: &Bound<'_, PyAny>) -> PyResult<()> {
        common_send_command(&self.inner, Self::LABEL, schema, value)
    }

    fn run_cycle_batch(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        common_run_cycle_batch(&self.inner, Self::LABEL, py)
    }

    fn run_to_idle(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        common_run_to_idle(&self.inner, Self::LABEL, py)
    }

    fn run_until_runtime_quiescent(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        common_run_until_runtime_quiescent(&self.inner, Self::LABEL, py)
    }

    fn quiescence_status(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        common_quiescence_status(&self.inner, Self::LABEL, py)
    }

    #[pyo3(signature=(workflow, key=None))]
    fn state_get(
        &self,
        py: Python<'_>,
        workflow: &str,
        key: Option<Vec<u8>>,
    ) -> PyResult<Py<PyAny>> {
        let state = self
            .lock()?
            .state_json(workflow, key.as_deref())
            .map_err(host_to_py_error)?;
        json_to_py(py, &state)
    }

    #[pyo3(signature=(workflow, key=None))]
    fn state_bytes(
        &self,
        py: Python<'_>,
        workflow: &str,
        key: Option<Vec<u8>>,
    ) -> PyResult<Option<Py<PyBytes>>> {
        let bytes = self
            .lock()?
            .state_bytes(workflow, key.as_deref())
            .map_err(host_to_py_error)?;
        Ok(bytes.map(|bytes| PyBytes::new(py, &bytes).unbind()))
    }

    fn list_cells(&self, py: Python<'_>, workflow: &str) -> PyResult<Py<PyAny>> {
        let cells = self
            .lock()?
            .list_cells(workflow)
            .map_err(host_to_py_error)?;
        cell_list_to_py(py, &cells)
    }

    fn blob_get_bytes(&self, py: Python<'_>, blob_ref: &str) -> PyResult<Py<PyBytes>> {
        let bytes = self
            .lock()?
            .blob_bytes(blob_ref)
            .map_err(host_to_py_error)?;
        Ok(PyBytes::new(py, &bytes).unbind())
    }

    fn blob_get_text(&self, blob_ref: &str) -> PyResult<String> {
        String::from_utf8(
            self.lock()?
                .blob_bytes(blob_ref)
                .map_err(host_to_py_error)?,
        )
        .map_err(|err| py_runtime_error(format!("blob '{blob_ref}' is not valid UTF-8: {err}")))
    }

    fn blob_get_json(&self, py: Python<'_>, blob_ref: &str) -> PyResult<Py<PyAny>> {
        let bytes = self
            .lock()?
            .blob_bytes(blob_ref)
            .map_err(host_to_py_error)?;
        let value: JsonValue = serde_json::from_slice(&bytes)
            .map_err(|err| py_runtime_error(format!("decode blob json '{blob_ref}': {err}")))?;
        json_to_py(py, &value)
    }

    fn trace_summary(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        common_trace_summary(&self.inner, Self::LABEL, py)
    }

    fn snapshot_create(&self) -> PyResult<()> {
        common_snapshot_create(&self.inner, Self::LABEL)
    }

    fn reopen(&self) -> PyResult<Self> {
        let reopened = self.lock()?.reopen().map_err(host_to_py_error)?;
        Ok(Self {
            inner: Mutex::new(reopened),
            world_id: self.world_id.clone(),
            warnings: self.warnings.clone(),
        })
    }

    fn artifact_export(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        common_artifact_export(&self.inner, Self::LABEL, py)
    }
}

#[pyclass(name = "WorkflowHarness", unsendable)]
struct PyWorkflowHarness {
    inner: Mutex<RuntimeWorkflowHarness<MemStore>>,
    workflow: String,
    warnings: Vec<String>,
    scratch_root: Arc<TempDir>,
}

impl PyWorkflowHarness {
    const LABEL: &'static str = "workflow harness";

    fn lock(&self) -> PyResult<MutexGuard<'_, RuntimeWorkflowHarness<MemStore>>> {
        lock_harness(&self.inner, Self::LABEL)
    }

    fn from_authored_paths_inner(
        workflow: &str,
        air_dir: &Path,
        workflow_dir: Option<&Path>,
        import_roots: &[PathBuf],
        force_build: bool,
        sync_secrets: bool,
        secret_bindings: Option<HashMap<String, Vec<u8>>>,
        build_profile: WorkflowBuildProfile,
        effect_mode: EffectMode,
    ) -> PyResult<Self> {
        let scratch_root =
            Arc::new(TempDir::new().map_err(|err| py_runtime_error(err.to_string()))?);
        let sync_root = if sync_secrets {
            Some(air_dir.parent().ok_or_else(|| {
                PyValueError::new_err(
                    "air_dir has no parent; disable sync_secrets or pass secret_bindings explicitly",
                )
            })?)
        } else {
            None
        };
        let harness = build_runtime_workflow_harness_from_authored_paths_with_secret_config(
            workflow.to_string(),
            air_dir,
            workflow_dir,
            import_roots,
            scratch_root.path(),
            force_build,
            build_profile,
            effect_mode,
            sync_root,
            None,
            secret_bindings,
        )
        .map_err(|err| py_runtime_error(err.to_string()))?;
        Ok(Self {
            inner: Mutex::new(harness),
            workflow: workflow.to_string(),
            warnings: Vec::new(),
            scratch_root,
        })
    }
}

#[pymethods]
impl PyWorkflowHarness {
    #[staticmethod]
    #[pyo3(signature=(workflow, air_dir, workflow_dir=None, import_roots=None, force_build=false, sync_secrets=false, secret_bindings=None, build_profile="debug", effect_mode="scripted"))]
    fn from_air_dir(
        workflow: &str,
        air_dir: &str,
        workflow_dir: Option<&str>,
        import_roots: Option<Vec<String>>,
        force_build: bool,
        sync_secrets: bool,
        secret_bindings: Option<&Bound<'_, PyDict>>,
        build_profile: &str,
        effect_mode: &str,
    ) -> PyResult<Self> {
        let import_roots: Vec<PathBuf> = import_roots
            .unwrap_or_default()
            .into_iter()
            .map(PathBuf::from)
            .collect();
        Self::from_authored_paths_inner(
            workflow,
            Path::new(air_dir),
            workflow_dir.map(Path::new),
            &import_roots,
            force_build,
            sync_secrets,
            parse_secret_bindings(secret_bindings)?,
            parse_build_profile(build_profile)?,
            parse_effect_mode(effect_mode)?,
        )
    }

    #[staticmethod]
    #[pyo3(signature=(workflow, workflow_dir, air_dir=None, import_roots=None, force_build=false, sync_secrets=false, secret_bindings=None, build_profile="debug", effect_mode="scripted"))]
    fn from_workflow_dir(
        workflow: &str,
        workflow_dir: &str,
        air_dir: Option<&str>,
        import_roots: Option<Vec<String>>,
        force_build: bool,
        sync_secrets: bool,
        secret_bindings: Option<&Bound<'_, PyDict>>,
        build_profile: &str,
        effect_mode: &str,
    ) -> PyResult<Self> {
        let workflow_dir = Path::new(workflow_dir);
        let default_air = workflow_dir
            .parent()
            .map(|parent| parent.join("air"))
            .ok_or_else(|| {
                PyValueError::new_err("workflow_dir has no parent; pass air_dir explicitly")
            })?;
        let air_dir = air_dir.map(PathBuf::from).unwrap_or(default_air);
        let import_roots: Vec<PathBuf> = import_roots
            .unwrap_or_default()
            .into_iter()
            .map(PathBuf::from)
            .collect();
        Self::from_authored_paths_inner(
            workflow,
            &air_dir,
            Some(workflow_dir),
            &import_roots,
            force_build,
            sync_secrets,
            parse_secret_bindings(secret_bindings)?,
            parse_build_profile(build_profile)?,
            parse_effect_mode(effect_mode)?,
        )
    }

    #[getter]
    fn workflow(&self) -> String {
        self.workflow.clone()
    }

    #[getter]
    fn warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }

    fn send_event(&self, schema: &str, value: &Bound<'_, PyAny>) -> PyResult<()> {
        common_send_event(&self.inner, Self::LABEL, schema, value)
    }

    fn send_command(&self, schema: &str, value: &Bound<'_, PyAny>) -> PyResult<()> {
        common_send_command(&self.inner, Self::LABEL, schema, value)
    }

    fn run_cycle_batch(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        common_run_cycle_batch(&self.inner, Self::LABEL, py)
    }

    fn run_to_idle(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        common_run_to_idle(&self.inner, Self::LABEL, py)
    }

    fn run_until_runtime_quiescent(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        common_run_until_runtime_quiescent(&self.inner, Self::LABEL, py)
    }

    fn quiescence_status(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        common_quiescence_status(&self.inner, Self::LABEL, py)
    }

    fn pull_effects(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        common_pull_effects(&self.inner, Self::LABEL, py)
    }

    fn apply_receipt_object(&self, receipt: &Bound<'_, PyAny>) -> PyResult<()> {
        common_apply_receipt_object(&self.inner, Self::LABEL, receipt)
    }

    #[pyo3(signature=(intent_hash, adapter_id, status, payload_cbor, cost_cents=None, signature=None))]
    fn apply_receipt(
        &self,
        intent_hash: Vec<u8>,
        adapter_id: &str,
        status: &str,
        payload_cbor: Vec<u8>,
        cost_cents: Option<u64>,
        signature: Option<Vec<u8>>,
    ) -> PyResult<()> {
        common_apply_receipt(
            &self.inner,
            Self::LABEL,
            intent_hash,
            adapter_id,
            status,
            payload_cbor,
            cost_cents,
            signature,
        )
    }

    fn receipt_ok(
        &self,
        py: Python<'_>,
        intent_hash: Vec<u8>,
        adapter_id: &str,
        payload: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        common_receipt_ok(
            &self.inner,
            Self::LABEL,
            py,
            intent_hash,
            adapter_id,
            payload,
        )
    }

    fn receipt_error(
        &self,
        py: Python<'_>,
        intent_hash: Vec<u8>,
        adapter_id: &str,
        payload: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        common_receipt_error(
            &self.inner,
            Self::LABEL,
            py,
            intent_hash,
            adapter_id,
            payload,
        )
    }

    fn receipt_timeout(
        &self,
        py: Python<'_>,
        intent_hash: Vec<u8>,
        adapter_id: &str,
        payload: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        common_receipt_timeout(
            &self.inner,
            Self::LABEL,
            py,
            intent_hash,
            adapter_id,
            payload,
        )
    }

    #[pyo3(signature=(intent_hash, delivered_at_ns, key=None))]
    fn receipt_timer_set_ok(
        &self,
        py: Python<'_>,
        intent_hash: Vec<u8>,
        delivered_at_ns: u64,
        key: Option<String>,
    ) -> PyResult<Py<PyAny>> {
        common_receipt_timer_set_ok(
            &self.inner,
            Self::LABEL,
            py,
            intent_hash,
            delivered_at_ns,
            key,
        )
    }

    fn receipt_blob_put_ok(
        &self,
        py: Python<'_>,
        intent_hash: Vec<u8>,
        blob_ref: &str,
        edge_ref: &str,
        size: u64,
    ) -> PyResult<Py<PyAny>> {
        common_receipt_blob_put_ok(
            &self.inner,
            Self::LABEL,
            py,
            intent_hash,
            blob_ref,
            edge_ref,
            size,
        )
    }

    #[pyo3(signature=(intent_hash, blob_ref, bytes, size=None))]
    fn receipt_blob_get_ok(
        &self,
        py: Python<'_>,
        intent_hash: Vec<u8>,
        blob_ref: &str,
        bytes: Vec<u8>,
        size: Option<u64>,
    ) -> PyResult<Py<PyAny>> {
        common_receipt_blob_get_ok(
            &self.inner,
            Self::LABEL,
            py,
            intent_hash,
            blob_ref,
            bytes,
            size,
        )
    }

    #[pyo3(signature=(intent_hash, status, adapter_id="adapter.http.harness", headers=None, body_ref=None, start_ns=None, end_ns=None))]
    fn receipt_http_request_ok(
        &self,
        py: Python<'_>,
        intent_hash: Vec<u8>,
        status: i32,
        adapter_id: &str,
        headers: Option<&Bound<'_, PyAny>>,
        body_ref: Option<&str>,
        start_ns: Option<u64>,
        end_ns: Option<u64>,
    ) -> PyResult<Py<PyAny>> {
        common_receipt_http_request_ok(
            &self.inner,
            Self::LABEL,
            py,
            intent_hash,
            status,
            adapter_id,
            headers,
            body_ref,
            start_ns,
            end_ns,
        )
    }

    #[pyo3(signature=(intent_hash, output_ref, provider_id, finish_reason="stop", prompt_tokens=0, completion_tokens=0, total_tokens=None, raw_output_ref=None, provider_response_id=None, cost_cents=None, warnings_ref=None, rate_limit_ref=None))]
    fn receipt_llm_generate_ok(
        &self,
        py: Python<'_>,
        intent_hash: Vec<u8>,
        output_ref: &str,
        provider_id: &str,
        finish_reason: &str,
        prompt_tokens: u64,
        completion_tokens: u64,
        total_tokens: Option<u64>,
        raw_output_ref: Option<&str>,
        provider_response_id: Option<String>,
        cost_cents: Option<u64>,
        warnings_ref: Option<&str>,
        rate_limit_ref: Option<&str>,
    ) -> PyResult<Py<PyAny>> {
        common_receipt_llm_generate_ok(
            &self.inner,
            Self::LABEL,
            py,
            intent_hash,
            output_ref,
            provider_id,
            finish_reason,
            prompt_tokens,
            completion_tokens,
            total_tokens,
            raw_output_ref,
            provider_response_id,
            cost_cents,
            warnings_ref,
            rate_limit_ref,
        )
    }

    #[pyo3(signature=(key=None))]
    fn state_get(&self, py: Python<'_>, key: Option<Vec<u8>>) -> PyResult<Py<PyAny>> {
        let state = self
            .lock()?
            .state_json(key.as_deref())
            .map_err(host_to_py_error)?;
        json_to_py(py, &state)
    }

    #[pyo3(signature=(key=None))]
    fn state_bytes(&self, py: Python<'_>, key: Option<Vec<u8>>) -> PyResult<Option<Py<PyBytes>>> {
        let bytes = self.lock()?.state_bytes(key.as_deref());
        Ok(bytes.map(|bytes| PyBytes::new(py, &bytes).unbind()))
    }

    fn list_cells(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let cells = self.lock()?.list_cells().map_err(host_to_py_error)?;
        cell_list_to_py(py, &cells)
    }

    fn blob_get_bytes(&self, py: Python<'_>, blob_ref: &str) -> PyResult<Py<PyBytes>> {
        let bytes = self
            .lock()?
            .blob_bytes(blob_ref)
            .map_err(host_to_py_error)?;
        Ok(PyBytes::new(py, &bytes).unbind())
    }

    fn blob_get_text(&self, blob_ref: &str) -> PyResult<String> {
        String::from_utf8(
            self.lock()?
                .blob_bytes(blob_ref)
                .map_err(host_to_py_error)?,
        )
        .map_err(|err| py_runtime_error(format!("blob '{blob_ref}' is not valid UTF-8: {err}")))
    }

    fn blob_get_json(&self, py: Python<'_>, blob_ref: &str) -> PyResult<Py<PyAny>> {
        let bytes = self
            .lock()?
            .blob_bytes(blob_ref)
            .map_err(host_to_py_error)?;
        let value: JsonValue = serde_json::from_slice(&bytes)
            .map_err(|err| py_runtime_error(format!("decode blob json '{blob_ref}': {err}")))?;
        json_to_py(py, &value)
    }

    fn trace_summary(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        common_trace_summary(&self.inner, Self::LABEL, py)
    }

    fn snapshot_create(&self) -> PyResult<()> {
        common_snapshot_create(&self.inner, Self::LABEL)
    }

    fn reopen(&self) -> PyResult<Self> {
        let reopened = self.lock()?.reopen().map_err(host_to_py_error)?;
        Ok(Self {
            inner: Mutex::new(reopened),
            workflow: self.workflow.clone(),
            warnings: self.warnings.clone(),
            scratch_root: Arc::clone(&self.scratch_root),
        })
    }

    fn time_get(&self) -> PyResult<u64> {
        common_time_get(&self.inner, Self::LABEL)
    }

    fn time_set(&self, now_ns: u64) -> PyResult<u64> {
        common_time_set(&self.inner, Self::LABEL, now_ns)
    }

    fn time_advance(&self, delta_ns: u64) -> PyResult<u64> {
        common_time_advance(&self.inner, Self::LABEL, delta_ns)
    }

    fn time_jump_next_due(&self) -> PyResult<Option<u64>> {
        common_time_jump_next_due(&self.inner, Self::LABEL)
    }

    fn artifact_export(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        common_artifact_export(&self.inner, Self::LABEL, py)
    }
}

#[pymodule]
#[pyo3(name = "_core")]
fn _core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(canonical_cbor, m)?)?;
    m.add_class::<PyWorldHarness>()?;
    m.add_class::<PyWorkflowHarness>()?;
    Ok(())
}
