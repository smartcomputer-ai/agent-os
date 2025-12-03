//! Test utilities for exercising the AgentOS kernel with deterministic fixtures.
//!
//! **NOTE**: This crate is a re-export shim. The canonical implementations now live in `aos-host`.
//! Consider depending on `aos-host` with the `test-fixtures` feature directly.
//!
//! This crate provides two main abstractions:
//! - `fixtures` module: Helpers for building manifests, stub reducers, and test data
//! - `TestWorld`: Low-level kernel wrapper for synchronous testing
//! - `TestHost`: High-level host wrapper for async testing with adapters
//!
//! For new code, prefer importing from `aos-host::fixtures` and `aos-host::testhost` directly.

// Re-export everything from aos-host (canonical source)
pub use aos_host::fixtures;
pub use aos_host::fixtures::TestStore;
pub use aos_host::fixtures::TestWorld;
pub use aos_host::testhost::TestHost;

// Re-export mock adapters
pub use aos_host::adapters::mock::{LlmRequestContext, MockLlmHarness};

// Re-export commonly used fixtures items at crate root for convenience
pub use fixtures::{
    START_SCHEMA, SYS_TIMER_FIRED, build_loaded_manifest, domain_event, effect_params_text,
    fake_hash, new_mem_store, routing_event, schema, start_trigger, stub_event_emitting_reducer,
    stub_reducer_module, timer_trigger, zero_hash,
};
