//! Profile catalog reducer scaffold (`aos.agent/ProfileCatalogReducer@1`).

#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use aos_agent_sdk::{ProfileCatalogEvent, ProfileCatalogState};
use aos_wasm_sdk::{ReduceError, Reducer, ReducerCtx, Value, aos_reducer};

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}

aos_reducer!(ProfileCatalogReducer);

#[derive(Default)]
struct ProfileCatalogReducer;

impl Reducer for ProfileCatalogReducer {
    type State = ProfileCatalogState;
    type Event = ProfileCatalogEvent;
    type Ann = Value;

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut ReducerCtx<Self::State, Self::Ann>,
    ) -> Result<(), ReduceError> {
        match event {
            ProfileCatalogEvent::UpsertProfile { profile } => {
                ctx.state
                    .profiles
                    .insert(profile.profile_id.0.clone(), profile);
            }
            ProfileCatalogEvent::DeleteProfile { profile_id } => {
                ctx.state.profiles.remove(&profile_id.0);
            }
            ProfileCatalogEvent::LookupRequested(_) | ProfileCatalogEvent::Noop => {}
        }
        Ok(())
    }
}
