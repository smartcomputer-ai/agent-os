use alloc::vec::Vec;
use aos_wasm_abi::{AbiDecodeError, AbiEncodeError, PureContext, PureInput, PureOutput};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{read_input, write_back};

/// Trait implemented by every pure module.
pub trait PureModule: Default {
    /// Input payload; decoded from canonical CBOR.
    type Input: DeserializeOwned;

    /// Output payload; encoded as canonical CBOR.
    type Output: Serialize;

    /// Core pure logic.
    fn run(
        &mut self,
        input: Self::Input,
        ctx: Option<&PureContext>,
    ) -> Result<Self::Output, PureError>;
}

/// Deterministic pure-module failure.
#[derive(Debug, Clone, Copy)]
pub struct PureError {
    msg: &'static str,
}

impl PureError {
    pub const fn new(msg: &'static str) -> Self {
        Self { msg }
    }

    pub fn message(&self) -> &'static str {
        self.msg
    }
}

impl core::fmt::Display for PureError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.msg)
    }
}

/// Errors surfaced while running the pure entrypoint.
#[derive(Debug)]
pub enum RunError {
    AbiDecode(AbiDecodeError),
    InputDecode(serde_cbor::Error),
    CtxDecode(serde_cbor::Error),
    OutputEncode(serde_cbor::Error),
    OutputEnvelope(AbiEncodeError),
    Run(&'static str),
}

impl RunError {
    #[cold]
    pub fn trap(self, module: &'static str) -> ! {
        panic!("aos-wasm-sdk({module}): {self}");
    }
}

impl core::fmt::Display for RunError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RunError::AbiDecode(err) => write!(f, "abi decode failed: {err}"),
            RunError::InputDecode(err) => write!(f, "input decode failed: {err}"),
            RunError::CtxDecode(err) => write!(f, "context decode failed: {err}"),
            RunError::OutputEncode(err) => write!(f, "output encode failed: {err}"),
            RunError::OutputEnvelope(err) => write!(f, "output envelope encode failed: {err}"),
            RunError::Run(msg) => write!(f, "run error: {msg}"),
        }
    }
}

/// Execute a pure module against a pure input envelope (used by tests).
pub fn run_bytes<M: PureModule>(input: &[u8]) -> Result<Vec<u8>, RunError>
where
    M::Output: Serialize,
{
    let env = PureInput::decode(input).map_err(RunError::AbiDecode)?;
    run_module::<M>(env)
}

fn run_module<M: PureModule>(input: PureInput) -> Result<Vec<u8>, RunError>
where
    M::Output: Serialize,
{
    let _module_name = core::any::type_name::<M>();
    let payload = serde_cbor::from_slice(&input.input).map_err(RunError::InputDecode)?;
    let ctx = match &input.ctx {
        Some(bytes) => Some(PureContext::decode(bytes).map_err(RunError::CtxDecode)?),
        None => None,
    };
    let mut module = M::default();
    let output = module
        .run(payload, ctx.as_ref())
        .map_err(|err| RunError::Run(err.message()))?;
    let output_bytes = serde_cbor::to_vec(&output).map_err(RunError::OutputEncode)?;
    let envelope = PureOutput {
        output: output_bytes,
    };
    envelope.encode().map_err(RunError::OutputEnvelope)
}

/// Entry helper for exported `run`.
pub fn dispatch_pure<M: PureModule>(ptr: i32, len: i32) -> (i32, i32)
where
    M::Output: Serialize,
{
    let module_name = core::any::type_name::<M>();
    let bytes = unsafe { read_input(ptr, len) };
    match run_bytes::<M>(bytes) {
        Ok(output) => write_back(&output),
        Err(err) => err.trap(module_name),
    }
}

/// Macro wiring the pure entrypoints.
#[macro_export]
macro_rules! aos_pure {
    ($ty:ty) => {
        #[cfg_attr(target_arch = "wasm32", unsafe(export_name = "alloc"))]
        pub extern "C" fn aos_wasm_alloc(len: i32) -> i32 {
            $crate::exported_alloc(len)
        }

        #[cfg_attr(target_arch = "wasm32", unsafe(export_name = "run"))]
        pub extern "C" fn aos_wasm_run(ptr: i32, len: i32) -> (i32, i32) {
            $crate::dispatch_pure::<$ty>(ptr, len)
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Default)]
    struct TestPure;

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Input {
        value: u64,
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Output {
        doubled: u64,
    }

    impl PureModule for TestPure {
        type Input = Input;
        type Output = Output;

        fn run(
            &mut self,
            input: Self::Input,
            _ctx: Option<&PureContext>,
        ) -> Result<Self::Output, PureError> {
            Ok(Output {
                doubled: input.value * 2,
            })
        }
    }

    #[test]
    fn run_bytes_round_trip() {
        let ctx = PureContext {
            logical_now_ns: 5,
            journal_height: 9,
            manifest_hash: "sha256:2222222222222222222222222222222222222222222222222222222222222222"
                .into(),
            module: "com.acme/TestPure@1".into(),
        };
        let ctx_bytes = serde_cbor::to_vec(&ctx).unwrap();
        let env = PureInput {
            version: aos_wasm_abi::ABI_VERSION,
            input: serde_cbor::to_vec(&Input { value: 7 }).unwrap(),
            ctx: Some(ctx_bytes),
        };
        let bytes = env.encode().unwrap();
        let output = run_bytes::<TestPure>(&bytes).expect("run");
        let decoded = PureOutput::decode(&output).expect("decode");
        let payload: Output = serde_cbor::from_slice(&decoded.output).unwrap();
        assert_eq!(payload.doubled, 14);
    }
}
