//! Pure module scaffold (`aos.agent/SessionEventEcho@1`).

#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use aos_agent_sdk::SessionEvent;
use aos_wasm_abi::PureContext;
use aos_wasm_sdk::{PureError, PureModule, aos_pure};

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}

aos_pure!(SessionEventEcho);

#[derive(Default)]
struct SessionEventEcho;

impl PureModule for SessionEventEcho {
    type Input = SessionEvent;
    type Output = SessionEvent;

    fn run(
        &mut self,
        input: Self::Input,
        _ctx: Option<&PureContext>,
    ) -> Result<Self::Output, PureError> {
        Ok(input)
    }
}
