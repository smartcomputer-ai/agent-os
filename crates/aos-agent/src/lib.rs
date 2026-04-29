//! Agent contracts for `aos.agent/*`.
//!
//! Public API is contract-first: domain-event/state/config types in `contracts`.
//! `SessionState::default()` is intentionally chat/no-tools: embedding worlds opt into
//! inspect, host, workspace, or custom tools by installing explicit registries and profiles.
//!
//! Built-in bundle constructors and assembly helpers are re-exported from `contracts`:
//! `tool_bundle_inspect()`, `tool_bundle_host_local()`, `tool_bundle_host_sandbox()`,
//! `tool_bundle_workspace()`, `ToolRegistryBuilder`, and `ToolProfileBuilder`.
//!
//! The evented [`SessionWorkflow`] remains the broad reusable adapter workflow. Its generated AIR
//! declares the full host, workspace, introspect, blob, and LLM effect surface so it can host
//! multiple agent shapes, but importing this crate or creating a default state does not grant any
//! tool access. Tools become visible only through the registry/profile installed by the embedding
//! world or by `aos.agent/SessionIngress@1` events.
//!
//! Host auto-open is also opt-in. A session or run must provide `HostSessionOpenConfig`; local and
//! sandbox targets both flow through the same `sys/host.session.open@1` path.
//!
//! Example assembly shapes:
//!
//! ```ignore
//! // Chat-only: keep SessionState::default() and do not install tools.
//! let state = SessionState::default();
//! ```
//!
//! ```ignore
//! // Workspace-only: no host tools and no host auto-open config.
//! let registry = ToolRegistryBuilder::new()
//!     .with_bundle(tool_bundle_workspace())
//!     .build()?;
//! let profile = ToolProfileBuilder::new()
//!     .with_bundle(tool_bundle_workspace())
//!     .build_for_registry(&registry)?;
//! ```
//!
//! ```ignore
//! // Local coding: opt into the compatibility preset explicitly.
//! state.tool_registry = local_coding_agent_tool_registry();
//! state.tool_profiles = local_coding_agent_tool_profiles();
//! state.tool_profile = local_coding_agent_tool_profile_for_provider("openai");
//! state.session_config.default_host_session_open = Some(local_host_config);
//! ```
//!
//! Helper reducers/mappers remain available under `helpers` for internal/runtime use.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod contracts;
#[doc(hidden)]
pub mod helpers;
#[doc(hidden)]
pub mod tools;
mod workflow;
mod world;

pub use contracts::*;
pub use workflow::SessionWorkflow;
pub use world::aos_air_nodes;
