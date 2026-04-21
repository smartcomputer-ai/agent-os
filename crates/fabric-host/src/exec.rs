//! Exec request helpers and NDJSON stream conversion.
//!
//! Runtime-specific process execution stays behind `FabricRuntime`; this module
//! owns host-level validation and HTTP stream shaping.
