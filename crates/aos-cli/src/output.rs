use anyhow::Result;
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy)]
pub struct OutputOpts {
    pub json: bool,
    pub pretty: bool,
    pub quiet: bool,
    pub no_meta: bool,
    pub verbose: bool,
}

pub fn print_verbose(opts: OutputOpts, message: impl AsRef<str>) {
    if opts.verbose {
        eprintln!("verbose: {}", message.as_ref());
    }
}

pub fn print_success(
    opts: OutputOpts,
    data: Value,
    meta: Option<Value>,
    mut warnings: Vec<String>,
) -> Result<()> {
    if opts.quiet {
        warnings.clear();
    }
    if opts.json || opts.pretty {
        let mut root = json!({ "data": data });
        if !opts.no_meta {
            if let Some(meta) = meta {
                root.as_object_mut().unwrap().insert("meta".into(), meta);
            }
        }
        if !warnings.is_empty() {
            root.as_object_mut().unwrap().insert(
                "warnings".into(),
                Value::Array(warnings.into_iter().map(Value::String).collect()),
            );
        }
        if opts.pretty {
            println!("{}", serde_json::to_string_pretty(&root)?);
        } else {
            println!("{}", serde_json::to_string(&root)?);
        }
        return Ok(());
    }

    for warning in warnings {
        eprintln!("notice: {warning}");
    }
    match data {
        Value::String(text) => println!("{text}"),
        other => println!("{}", serde_json::to_string_pretty(&other)?),
    }
    Ok(())
}
