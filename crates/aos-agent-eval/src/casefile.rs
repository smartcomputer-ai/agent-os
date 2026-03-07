use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct EvalCase {
    pub id: String,
    #[serde(skip)]
    pub source_file: String,
    #[serde(default)]
    pub description: String,
    pub prompt: String,
    #[serde(default)]
    pub setup: SetupSpec,
    #[serde(default)]
    pub expect: ExpectSpec,
    #[serde(default)]
    pub eval: EvalSpec,
    #[serde(default)]
    pub run: RunSpec,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SetupSpec {
    #[serde(default)]
    pub files: Vec<SetupFile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SetupFile {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ExpectSpec {
    #[serde(default)]
    pub tool_called: Vec<String>,
    #[serde(default)]
    pub assistant_contains: Vec<String>,
    #[serde(default)]
    pub tool_output_contains: Vec<String>,
    #[serde(default)]
    pub files: Vec<FileExpectation>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileExpectation {
    pub path: String,
    #[serde(default)]
    pub equals: Option<String>,
    #[serde(default)]
    pub contains: Option<String>,
    #[serde(default)]
    pub exists: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EvalSpec {
    #[serde(default)]
    pub runs: Option<u32>,
    #[serde(default)]
    pub min_pass_rate: Option<f64>,
    #[serde(default)]
    pub max_steps: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RunSpec {
    #[serde(default)]
    pub max_tokens: Option<u64>,
    #[serde(default)]
    pub tool_profile: Option<String>,
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default)]
    pub tool_enable: Option<Vec<String>>,
    #[serde(default)]
    pub tool_disable: Option<Vec<String>>,
    #[serde(default)]
    pub tool_force: Option<Vec<String>>,
    #[serde(default)]
    pub bootstrap_session: Option<bool>,
}

pub fn load_cases(cases_dir: &Path) -> Result<Vec<EvalCase>> {
    let mut files = fs::read_dir(cases_dir)
        .with_context(|| format!("read cases dir {}", cases_dir.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|s| s.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    files.sort();

    let mut out = Vec::new();
    for path in files {
        let bytes = fs::read(&path).with_context(|| format!("read case {}", path.display()))?;
        let mut case: EvalCase =
            serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
        if case.id.trim().is_empty() {
            bail!("case file {} has empty id", path.display());
        }
        case.source_file = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.to_string())
            .unwrap_or_else(|| path.display().to_string());
        out.push(case);
    }

    if out.is_empty() {
        bail!("no case files found in {}", cases_dir.display());
    }

    let mut seen = std::collections::BTreeSet::new();
    for case in &out {
        if !seen.insert(case.id.clone()) {
            bail!("duplicate case id '{}'", case.id);
        }
    }

    Ok(out)
}
