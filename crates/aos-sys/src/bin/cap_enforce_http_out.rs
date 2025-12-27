//! Cap enforcer for HTTP outbound effects (`sys/CapEnforceHttpOut@1`).

#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use aos_sys::{CapCheckInput, CapCheckOutput, CapDenyReason};
use aos_wasm_sdk::{PureError, PureModule, aos_pure};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_cbor::Value as CborValue;

// Required for WASM binary entry point
#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}

aos_pure!(CapEnforceHttpOut);

#[derive(Default)]
struct CapEnforceHttpOut;

#[derive(Deserialize)]
struct HttpCapParams {
    hosts: Option<Vec<String>>,
    schemes: Option<Vec<String>>,
    methods: Option<Vec<String>>,
    ports: Option<Vec<u64>>,
    path_prefixes: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct HttpRequestParams {
    method: String,
    url: String,
}

#[derive(Debug)]
struct ParsedUrl {
    scheme: String,
    host: String,
    port: Option<u64>,
    path: String,
}

impl PureModule for CapEnforceHttpOut {
    type Input = CapCheckInput;
    type Output = CapCheckOutput;

    fn run(&mut self, input: Self::Input) -> Result<Self::Output, PureError> {
        if input.effect_kind != "http.request" {
            return Ok(deny(
                "effect_kind_mismatch",
                format!(
                    "enforcer 'sys/CapEnforceHttpOut@1' cannot handle '{}'",
                    input.effect_kind
                ),
            ));
        }
        let cap_params: HttpCapParams = match decode_cbor(&input.cap_params) {
            Ok(value) => value,
            Err(err) => {
                return Ok(deny("cap_params_invalid", err.to_string()));
            }
        };
        let effect_params: HttpRequestParams = match decode_cbor(&input.effect_params) {
            Ok(value) => value,
            Err(err) => {
                return Ok(deny("effect_params_invalid", err.to_string()));
            }
        };
        let parsed = match parse_url(&effect_params.url) {
            Ok(parsed) => parsed,
            Err(reason) => {
                return Ok(CapCheckOutput {
                    constraints_ok: false,
                    deny: Some(reason),
                });
            }
        };

        if !allowlist_contains(&cap_params.hosts, &parsed.host, |v| v.to_lowercase()) {
            return Ok(deny(
                "host_not_allowed",
                format!("host '{}' not allowed", parsed.host),
            ));
        }
        if !allowlist_contains(&cap_params.schemes, &parsed.scheme, |v| v.to_lowercase()) {
            return Ok(deny(
                "scheme_not_allowed",
                format!("scheme '{}' not allowed", parsed.scheme),
            ));
        }
        if !allowlist_contains(&cap_params.methods, &effect_params.method, |v| {
            v.to_uppercase()
        }) {
            return Ok(deny(
                "method_not_allowed",
                format!("method '{}' not allowed", effect_params.method),
            ));
        }
        if let Some(ports) = &cap_params.ports {
            if !ports.is_empty() {
                let port = match parsed.port {
                    Some(port) => port,
                    None => {
                        return Ok(deny("port_not_allowed", "missing port"));
                    }
                };
                if !ports.iter().any(|p| *p == port) {
                    return Ok(deny(
                        "port_not_allowed",
                        format!("port '{port}' not allowed"),
                    ));
                }
            }
        }
        if let Some(prefixes) = &cap_params.path_prefixes {
            if !prefixes.is_empty()
                && !prefixes
                    .iter()
                    .any(|prefix| parsed.path.starts_with(prefix))
            {
                return Ok(deny(
                    "path_not_allowed",
                    format!("path '{}' not allowed", parsed.path),
                ));
            }
        }

        Ok(CapCheckOutput {
            constraints_ok: true,
            deny: None,
        })
    }
}

fn deny(code: &str, message: impl Into<String>) -> CapCheckOutput {
    CapCheckOutput {
        constraints_ok: false,
        deny: Some(CapDenyReason {
            code: code.into(),
            message: message.into(),
        }),
    }
}

fn parse_url(input: &str) -> Result<ParsedUrl, CapDenyReason> {
    let (scheme, rest) = input
        .split_once("://")
        .ok_or_else(|| deny_reason("invalid_url", "missing scheme"))?;
    if scheme.is_empty() {
        return Err(deny_reason("invalid_url", "missing scheme"));
    }
    let rest = rest.split('#').next().unwrap_or(rest);
    let (host_port, path_raw) = split_host_and_path(rest);
    let host_port = host_port.rsplit('@').next().unwrap_or(host_port);
    if host_port.is_empty() {
        return Err(deny_reason("invalid_url", "missing host"));
    }
    if host_port.starts_with('[') {
        return Err(deny_reason("invalid_url", "ipv6 hosts not supported"));
    }
    let (host, port) = split_host_port(host_port)?;
    if host.is_empty() {
        return Err(deny_reason("invalid_url", "missing host"));
    }
    let scheme = scheme.to_ascii_lowercase();
    let port = port.or_else(|| default_port(&scheme));
    let path = if path_raw.is_empty() {
        "/".to_string()
    } else {
        path_raw.to_string()
    };
    Ok(ParsedUrl {
        scheme,
        host: host.to_string(),
        port,
        path,
    })
}

fn split_host_and_path(rest: &str) -> (&str, &str) {
    let path_start = rest
        .find('/')
        .or_else(|| rest.find('?'))
        .unwrap_or(rest.len());
    let host_port = &rest[..path_start];
    let mut path = &rest[path_start..];
    if let Some(idx) = path.find('?') {
        path = &path[..idx];
    }
    (host_port, path)
}

fn split_host_port(host_port: &str) -> Result<(&str, Option<u64>), CapDenyReason> {
    if let Some(idx) = host_port.rfind(':') {
        let (host, port_str) = host_port.split_at(idx);
        let port_str = &port_str[1..];
        if !port_str.is_empty() && port_str.chars().all(|c| c.is_ascii_digit()) {
            let port = port_str
                .parse::<u64>()
                .map_err(|_| deny_reason("invalid_url", "invalid port"))?;
            return Ok((host, Some(port)));
        }
    }
    Ok((host_port, None))
}

fn default_port(scheme: &str) -> Option<u64> {
    match scheme {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    }
}

fn deny_reason(code: &str, message: impl Into<String>) -> CapDenyReason {
    CapDenyReason {
        code: code.into(),
        message: message.into(),
    }
}

fn decode_cbor<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, serde_cbor::Error> {
    const CBOR_SELF_DESCRIBE_TAG: u64 = 55799;
    let value: CborValue = serde_cbor::from_slice(bytes)?;
    let value = match value {
        CborValue::Tag(tag, inner) if tag == CBOR_SELF_DESCRIBE_TAG => *inner,
        other => other,
    };
    serde_cbor::value::from_value(value)
}

fn allowlist_contains(
    list: &Option<Vec<String>>,
    value: &str,
    normalize: impl Fn(&str) -> String,
) -> bool {
    let Some(list) = list else {
        return true;
    };
    if list.is_empty() {
        return true;
    }
    let value = normalize(value);
    list.iter().any(|entry| normalize(entry) == value)
}
