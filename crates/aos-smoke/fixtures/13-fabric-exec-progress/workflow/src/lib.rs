#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec;
use aos_wasm_sdk::{
    EffectReceiptEnvelope, EffectStreamFrameEnvelope, HashRef, HostExecParams,
    HostExecProgressFrame, HostExecReceipt, ReduceError, Workflow, WorkflowCtx, aos_variant,
    aos_workflow,
};
use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use serde_cbor::Value as CborValue;

aos_workflow!(FabricExecProgressSm);

#[derive(Default)]
struct FabricExecProgressSm;

impl Workflow for FabricExecProgressSm {
    type State = FabricExecProgressState;
    type Event = FabricExecProgressEvent;
    type Ann = ();

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut WorkflowCtx<Self::State, ()>,
    ) -> Result<(), ReduceError> {
        match event {
            FabricExecProgressEvent::Start(start) => handle_start(ctx, start),
            FabricExecProgressEvent::StreamFrame(frame) => handle_stream(ctx, frame)?,
            FabricExecProgressEvent::Receipt(receipt) => handle_receipt(ctx, receipt)?,
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct FabricExecProgressState {
    pc: FabricExecProgressPc,
    progress_frames: u64,
    last_stream_seq: Option<u64>,
    last_stream_kind: Option<String>,
    stdout_bytes: u64,
    last_stdout_delta_len: u64,
    exit_code: Option<i64>,
    receipt_status: Option<String>,
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum FabricExecProgressPc {
        Idle,
        Running,
        Done,
    }
}

impl Default for FabricExecProgressPc {
    fn default() -> Self {
        FabricExecProgressPc::Idle
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StartEvent {
    session_id: String,
}

#[derive(Debug, Clone, Serialize)]
enum FabricExecProgressEvent {
    Start(StartEvent),
    StreamFrame(EffectStreamFrameEnvelope),
    Receipt(EffectReceiptEnvelope),
}

impl<'de> Deserialize<'de> for FabricExecProgressEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = CborValue::deserialize(deserializer)?;
        if let Some((tag, inner)) = decode_tagged_event(&value) {
            return match tag.as_str() {
                "Start" | "start" => decode_event_inner(inner, Self::Start),
                "StreamFrame" | "streamframe" | "stream_frame" => {
                    decode_event_inner(inner, Self::StreamFrame)
                }
                "Receipt" | "receipt" => decode_event_inner(inner, Self::Receipt),
                _ => Err(D::Error::custom("unknown fabric exec progress event tag")),
            };
        }

        if let CborValue::Map(map) = &value {
            if map.len() == 1
                && let Some((CborValue::Text(tag), inner)) = map.iter().next()
            {
                return match tag.as_str() {
                    "Start" | "start" => decode_event_inner(inner.clone(), Self::Start),
                    "StreamFrame" | "streamframe" | "stream_frame" => {
                        decode_event_inner(inner.clone(), Self::StreamFrame)
                    }
                    "Receipt" | "receipt" => decode_event_inner(inner.clone(), Self::Receipt),
                    _ => Err(D::Error::custom("unknown externally tagged fabric exec event")),
                };
            }
            if map.contains_key(&CborValue::Text("receipt_payload".into())) {
                return serde_cbor::value::from_value(value)
                    .map(Self::Receipt)
                    .map_err(D::Error::custom);
            }
            if map.contains_key(&CborValue::Text("payload".into()))
                && map.contains_key(&CborValue::Text("seq".into()))
            {
                return serde_cbor::value::from_value(value)
                    .map(Self::StreamFrame)
                    .map_err(D::Error::custom);
            }
        }

        Err(D::Error::custom("unsupported fabric exec progress event"))
    }
}

fn decode_tagged_event(value: &CborValue) -> Option<(String, CborValue)> {
    let CborValue::Map(map) = value else {
        return None;
    };
    let Some(CborValue::Text(tag)) = map.get(&CborValue::Text("$tag".into())) else {
        return None;
    };
    let inner = map
        .get(&CborValue::Text("$value".into()))
        .cloned()
        .unwrap_or(CborValue::Null);
    Some((tag.to_string(), inner))
}

fn decode_event_inner<T, E>(
    value: CborValue,
    wrap: impl FnOnce(T) -> FabricExecProgressEvent,
) -> Result<FabricExecProgressEvent, E>
where
    T: for<'de> Deserialize<'de>,
    E: serde::de::Error,
{
    serde_cbor::value::from_value(value)
        .map(wrap)
        .map_err(E::custom)
}

fn handle_start(ctx: &mut WorkflowCtx<FabricExecProgressState, ()>, event: StartEvent) {
    if matches!(ctx.state.pc, FabricExecProgressPc::Running) {
        return;
    }
    ctx.state = FabricExecProgressState {
        pc: FabricExecProgressPc::Running,
        ..Default::default()
    };

    let params = HostExecParams {
        session_id: event.session_id,
        argv: vec!["slow".into()],
        cwd: None,
        timeout_ns: None,
        env_patch: None,
        stdin_ref: None::<HashRef>,
        output_mode: Some("require_inline".into()),
    };
    ctx.effects().sys().host_exec(&params, "default");
}

fn handle_stream(
    ctx: &mut WorkflowCtx<FabricExecProgressState, ()>,
    frame: EffectStreamFrameEnvelope,
) -> Result<(), ReduceError> {
    if frame.effect_op != "sys/host.exec@1" || frame.kind != "host.exec.progress" {
        return Ok(());
    }
    ctx.state.progress_frames = ctx.state.progress_frames.saturating_add(1);
    ctx.state.last_stream_seq = Some(frame.seq);
    ctx.state.last_stream_kind = Some(frame.kind.clone());
    if let Ok(progress) = frame.decode_payload::<HostExecProgressFrame>() {
        ctx.state.stdout_bytes = progress.stdout_bytes;
        ctx.state.last_stdout_delta_len = progress.stdout_delta.len() as u64;
    }
    Ok(())
}

fn handle_receipt(
    ctx: &mut WorkflowCtx<FabricExecProgressState, ()>,
    envelope: EffectReceiptEnvelope,
) -> Result<(), ReduceError> {
    if envelope.effect_op != "sys/host.exec@1" {
        return Ok(());
    }
    ctx.state.pc = FabricExecProgressPc::Done;
    if let Ok(receipt) = envelope.decode_receipt_payload::<HostExecReceipt>() {
        ctx.state.exit_code = Some(receipt.exit_code as i64);
        ctx.state.receipt_status = Some(receipt.status);
    } else {
        ctx.state.receipt_status = Some(envelope.status);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_wasm_abi::{ABI_VERSION, DomainEvent, WorkflowContext, WorkflowInput, WorkflowOutput};
    use aos_wasm_sdk::{HostInlineText, HostOutput, step_bytes};

    fn context_bytes() -> alloc::vec::Vec<u8> {
        let ctx = WorkflowContext {
            now_ns: 1,
            logical_now_ns: 2,
            journal_height: 3,
            entropy: vec![0x11; 64],
            event_hash: "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                .into(),
            manifest_hash:
                "sha256:1111111111111111111111111111111111111111111111111111111111111111"
                    .into(),
            workflow: "demo/FabricExecProgress@1".into(),
            key: None,
            cell_mode: false,
        };
        serde_cbor::to_vec(&ctx).expect("context bytes")
    }

    fn step_with(
        state: Option<alloc::vec::Vec<u8>>,
        event: &FabricExecProgressEvent,
    ) -> WorkflowOutput {
        let input = WorkflowInput {
            version: ABI_VERSION,
            state,
            event: DomainEvent::new(
                "demo/FabricExecProgressEvent@1",
                serde_cbor::to_vec(event).expect("event bytes"),
            ),
            ctx: Some(context_bytes()),
        };
        let input_bytes = input.encode().expect("encode input");
        let output_bytes = step_bytes::<FabricExecProgressSm>(&input_bytes).expect("step");
        WorkflowOutput::decode(&output_bytes).expect("decode output")
    }

    #[test]
    fn start_stream_and_receipt_round_trip() {
        let start = FabricExecProgressEvent::Start(StartEvent {
            session_id: "fabric-session-1".into(),
        });
        let start_out = step_with(None, &start);
        assert_eq!(start_out.effects.len(), 1);
        let state = start_out.state;

        let stream = FabricExecProgressEvent::StreamFrame(EffectStreamFrameEnvelope {
            effect_op: "sys/host.exec@1".into(),
            seq: 1,
            kind: "host.exec.progress".into(),
            payload: serde_cbor::to_vec(&HostExecProgressFrame {
                exec_id: Some("fake-exec".into()),
                elapsed_ns: 1,
                stdout_delta: b"e2e-progress\n".to_vec(),
                stderr_delta: vec![],
                stdout_bytes: "e2e-progress\n".len() as u64,
                stderr_bytes: 0,
            })
            .unwrap(),
            ..Default::default()
        });
        let stream_out = step_with(state, &stream);
        let state = stream_out.state;

        let receipt = FabricExecProgressEvent::Receipt(EffectReceiptEnvelope {
            effect_op: "sys/host.exec@1".into(),
            receipt_payload: serde_cbor::to_vec(&HostExecReceipt {
                exit_code: 0,
                status: "ok".into(),
                stdout: Some(HostOutput::InlineText {
                    inline_text: HostInlineText {
                        text: "e2e-progress\n".into(),
                    },
                }),
                stderr: None,
                started_at_ns: 1,
                ended_at_ns: 2,
                error_code: None,
                error_message: None,
            })
            .unwrap(),
            status: "ok".into(),
            ..Default::default()
        });
        let receipt_out = step_with(state, &receipt);
        assert!(receipt_out.state.is_some());
    }
}
