//! Shared output helpers for human and JSON modes.
//!
//! Human mode prints primary data to stdout and meta/notices to stderr.
//! JSON mode wraps responses in `{ data, meta?, warnings? }` and respects
//! `--pretty`, `--no-meta`, and `--quiet`.

use std::io::Write;

use anyhow::Result;
use serde_json::{Value, json};

use crate::opts::WorldOpts;

pub fn print_success(
    opts: &WorldOpts,
    data: Value,
    meta: Option<Value>,
    mut warnings: Vec<String>,
) -> Result<()> {
    if opts.quiet {
        warnings.clear();
    }
    if opts.pretty || opts.json {
        print_json(opts, data, meta, warnings)
    } else {
        print_human(opts, data, meta, warnings)
    }
}

fn print_json(
    opts: &WorldOpts,
    data: Value,
    meta: Option<Value>,
    warnings: Vec<String>,
) -> Result<()> {
    let mut root = json!({ "data": data });
    if !opts.no_meta {
        if let Some(m) = meta {
            root.as_object_mut().unwrap().insert("meta".into(), m);
        }
    }
    if !warnings.is_empty() {
        root.as_object_mut().unwrap().insert(
            "warnings".into(),
            warnings.into_iter().map(Value::String).collect(),
        );
    }
    if opts.pretty {
        println!("{}", serde_json::to_string_pretty(&root)?);
    } else {
        println!("{}", serde_json::to_string(&root)?);
    }
    Ok(())
}

fn print_human(
    opts: &WorldOpts,
    data: Value,
    meta: Option<Value>,
    warnings: Vec<String>,
) -> Result<()> {
    if let Some(m) = meta {
        if !opts.no_meta && opts.json {
            // Only emit meta in human mode when explicitly in JSON output.
            let mut stderr = std::io::stderr();
            writeln!(stderr, "meta: {}", serde_json::to_string_pretty(&m)?)?;
        }
    }
    for w in warnings {
        let mut stderr = std::io::stderr();
        writeln!(stderr, "notice: {}", w)?;
    }
    print_value(data)?;
    Ok(())
}

fn print_value(value: Value) -> Result<()> {
    match value {
        Value::String(s) => println!("{s}"),
        other => println!("{}", serde_json::to_string_pretty(&other)?),
    }
    Ok(())
}
