use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use aos_air_types::{AirNode, DefEffect, DefWorkflow, Manifest, Name, TypeExpr, builtins};

use crate::manifest_loader::parse_air_nodes_from_str;

pub const GENERATED_AIR_DIR: &str = "air/generated";
pub const DEFAULT_AIR_EXPORT_BIN: &str = "aos-air-export";

/// Materialize generated AIR JSON fragments under `<world>/air/generated/`.
///
/// Proc macros emit static metadata; this helper owns the host-side filesystem write. It validates
/// fragments through the normal AIR parser first, then writes stable per-kind JSON files.
pub fn write_generated_air_nodes(
    world_root: &Path,
    node_json_fragments: &[&str],
) -> Result<Vec<PathBuf>> {
    let mut buckets = GeneratedAirBuckets::default();
    for fragment in node_json_fragments {
        for node in parse_air_nodes_from_str(fragment).context("parse generated AIR fragment")? {
            buckets.push(node)?;
        }
    }
    buckets.validate_manifest_schema_closure()?;

    let generated_dir = world_root.join(GENERATED_AIR_DIR);
    fs::create_dir_all(&generated_dir)
        .with_context(|| format!("create generated AIR dir {}", generated_dir.display()))?;

    let mut written = Vec::new();
    write_bucket(
        &generated_dir,
        "schemas.air.json",
        &mut buckets.schemas,
        &mut written,
    )?;
    write_bucket(
        &generated_dir,
        "module.air.json",
        &mut buckets.modules,
        &mut written,
    )?;
    write_bucket(
        &generated_dir,
        "workflows.air.json",
        &mut buckets.workflows,
        &mut written,
    )?;
    write_bucket(
        &generated_dir,
        "effects.air.json",
        &mut buckets.effects,
        &mut written,
    )?;
    write_bucket(
        &generated_dir,
        "secrets.air.json",
        &mut buckets.secrets,
        &mut written,
    )?;
    write_bucket(
        &generated_dir,
        "manifest.air.json",
        &mut buckets.manifests,
        &mut written,
    )?;

    Ok(written)
}

/// Materialize generated AIR from the SDK export payload.
///
/// `aos_wasm_sdk::air_exports_json(AOS_AIR_NODES_JSON)` produces this JSON string array format.
/// Keeping the protocol as plain JSON lets export binaries stay tiny and keeps all filesystem
/// writes in host-side authoring code.
pub fn write_generated_air_export_json(
    world_root: &Path,
    export_json: &str,
) -> Result<Vec<PathBuf>> {
    let fragments: Vec<String> =
        serde_json::from_str(export_json).context("parse generated AIR export JSON")?;
    let refs: Vec<&str> = fragments.iter().map(String::as_str).collect();
    write_generated_air_nodes(world_root, &refs)
}

/// Run a Cargo export binary and materialize its Rust-authored AIR stdout.
///
/// The export binary should print `aos_wasm_sdk::air_exports_json(AOS_AIR_NODES_JSON)` and avoid
/// any other stdout. Cargo stderr is preserved for diagnostics when the command fails.
pub fn write_generated_air_from_cargo_export(
    world_root: &Path,
    manifest_path: &Path,
    package_name: Option<&str>,
    bin_name: Option<&str>,
) -> Result<Vec<PathBuf>> {
    let bin_name = bin_name.unwrap_or(DEFAULT_AIR_EXPORT_BIN);
    let mut command = Command::new("cargo");
    command
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(manifest_path);
    if let Some(package_name) = package_name.filter(|value| !value.trim().is_empty()) {
        command.arg("--package").arg(package_name);
    }
    command.arg("--bin").arg(bin_name);

    let output = command
        .output()
        .with_context(|| format!("run Cargo AIR export for {}", manifest_path.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "cargo AIR export failed for {} --bin {}: {}",
            manifest_path.display(),
            bin_name,
            stderr.trim()
        );
    }

    let stdout = String::from_utf8(output.stdout).context("decode Cargo AIR export stdout")?;
    write_generated_air_export_json(world_root, &stdout)
}

#[derive(Default)]
struct GeneratedAirBuckets {
    schemas: Vec<AirNode>,
    modules: Vec<AirNode>,
    workflows: Vec<AirNode>,
    effects: Vec<AirNode>,
    secrets: Vec<AirNode>,
    manifests: Vec<AirNode>,
}

impl GeneratedAirBuckets {
    fn push(&mut self, node: AirNode) -> Result<()> {
        match &node {
            AirNode::Defschema(_) => self.schemas.push(node),
            AirNode::Defmodule(_) => self.modules.push(node),
            AirNode::Defworkflow(_) => self.workflows.push(node),
            AirNode::Defeffect(_) => self.effects.push(node),
            AirNode::Defsecret(_) => self.secrets.push(node),
            AirNode::Manifest(_) => {
                if !self.manifests.is_empty() {
                    bail!("generated AIR may contain at most one manifest node");
                }
                self.manifests.push(node);
            }
        }
        Ok(())
    }

    fn validate_manifest_schema_closure(&self) -> Result<()> {
        let Some(AirNode::Manifest(manifest)) = self.manifests.first() else {
            return Ok(());
        };

        let schema_defs: HashMap<&str, &TypeExpr> = self
            .schemas
            .iter()
            .filter_map(|node| match node {
                AirNode::Defschema(schema) => Some((schema.name.as_str(), &schema.ty)),
                _ => None,
            })
            .collect();
        let workflow_defs: HashMap<&str, &DefWorkflow> = self
            .workflows
            .iter()
            .filter_map(|node| match node {
                AirNode::Defworkflow(workflow) => Some((workflow.name.as_str(), workflow)),
                _ => None,
            })
            .collect();
        let effect_defs: HashMap<&str, &DefEffect> = self
            .effects
            .iter()
            .filter_map(|node| match node {
                AirNode::Defeffect(effect) => Some((effect.name.as_str(), effect)),
                _ => None,
            })
            .collect();
        validate_manifest_schema_closure(manifest, &schema_defs, &workflow_defs, &effect_defs)
    }
}

fn validate_manifest_schema_closure(
    manifest: &Manifest,
    schema_defs: &HashMap<&str, &TypeExpr>,
    workflow_defs: &HashMap<&str, &DefWorkflow>,
    effect_defs: &HashMap<&str, &DefEffect>,
) -> Result<()> {
    let active_schemas: BTreeSet<&str> = manifest
        .schemas
        .iter()
        .map(|reference| reference.name.as_str())
        .collect();
    let active_workflows: BTreeSet<&str> = manifest
        .workflows
        .iter()
        .map(|reference| reference.name.as_str())
        .collect();
    let active_effects: BTreeSet<&str> = manifest
        .effects
        .iter()
        .map(|reference| reference.name.as_str())
        .collect();

    let mut required = SchemaClosure::new(schema_defs, &active_schemas);

    for schema_name in &active_schemas {
        required.require(schema_name, "manifest.schemas");
    }
    for workflow_name in &active_workflows {
        let Some(workflow) = workflow_defs.get(workflow_name) else {
            continue;
        };
        required.require(
            workflow.state.as_str(),
            format!("workflow '{workflow_name}' state"),
        );
        required.require(
            workflow.event.as_str(),
            format!("workflow '{workflow_name}' event"),
        );
        if let Some(schema) = &workflow.context {
            required.require(
                schema.as_str(),
                format!("workflow '{workflow_name}' context"),
            );
        }
        if let Some(schema) = &workflow.annotations {
            required.require(
                schema.as_str(),
                format!("workflow '{workflow_name}' annotations"),
            );
        }
        if let Some(schema) = &workflow.key_schema {
            required.require(
                schema.as_str(),
                format!("workflow '{workflow_name}' key_schema"),
            );
        }
    }
    for effect_name in &active_effects {
        let Some(effect) = effect_defs.get(effect_name) else {
            continue;
        };
        required.require(
            effect.params.as_str(),
            format!("effect '{effect_name}' params"),
        );
        required.require(
            effect.receipt.as_str(),
            format!("effect '{effect_name}' receipt"),
        );
    }
    if let Some(routing) = manifest.routing.as_ref() {
        for route in &routing.subscriptions {
            required.require(
                route.event.as_str(),
                format!("routing subscription for workflow '{}'", route.workflow),
            );
        }
    }

    required.walk();
    required.finish()
}

struct SchemaClosure<'a> {
    schema_defs: &'a HashMap<&'a str, &'a TypeExpr>,
    active_schemas: &'a BTreeSet<&'a str>,
    queue: Vec<&'a str>,
    visited: HashSet<&'a str>,
    missing: BTreeMap<String, BTreeSet<String>>,
}

impl<'a> SchemaClosure<'a> {
    fn new(
        schema_defs: &'a HashMap<&'a str, &'a TypeExpr>,
        active_schemas: &'a BTreeSet<&'a str>,
    ) -> Self {
        Self {
            schema_defs,
            active_schemas,
            queue: Vec::new(),
            visited: HashSet::new(),
            missing: BTreeMap::new(),
        }
    }

    fn require(&mut self, schema_name: &'a str, context: impl Into<String>) {
        if builtins::find_builtin_schema(schema_name).is_some() {
            return;
        }
        if !self.active_schemas.contains(schema_name) {
            self.missing
                .entry(schema_name.to_string())
                .or_default()
                .insert(context.into());
            return;
        }
        if !self.schema_defs.contains_key(schema_name) {
            // External import: the generated package can reference schemas provided by another
            // source. The merged AIR loader validates that those imports are present later.
            return;
        }
        if self.visited.insert(schema_name) {
            self.queue.push(schema_name);
        }
    }

    fn walk(&mut self) {
        while let Some(schema_name) = self.queue.pop() {
            let Some(schema) = self.schema_defs.get(schema_name) else {
                continue;
            };
            self.walk_type(schema_name, schema);
        }
    }

    fn walk_type(&mut self, owner: &'a str, ty: &'a TypeExpr) {
        match ty {
            TypeExpr::Primitive(_) => {}
            TypeExpr::Record(record) => {
                for field in record.record.values() {
                    self.walk_type(owner, field);
                }
            }
            TypeExpr::Variant(variant) => {
                for arm in variant.variant.values() {
                    self.walk_type(owner, arm);
                }
            }
            TypeExpr::List(list) => self.walk_type(owner, &list.list),
            TypeExpr::Set(set) => self.walk_type(owner, &set.set),
            TypeExpr::Map(map) => self.walk_type(owner, &map.map.value),
            TypeExpr::Option(option) => self.walk_type(owner, &option.option),
            TypeExpr::Ref(reference) => {
                self.require(reference.reference.as_str(), format!("schema '{owner}'"));
            }
        }
    }

    fn finish(self) -> Result<()> {
        if self.missing.is_empty() {
            return Ok(());
        }
        let details = self
            .missing
            .into_iter()
            .map(|(schema, contexts)| {
                let contexts = contexts.into_iter().collect::<Vec<_>>().join(", ");
                format!("{schema} referenced by {contexts}")
            })
            .collect::<Vec<_>>()
            .join("; ");
        bail!("generated AIR manifest schema closure is incomplete: {details}")
    }
}

fn write_bucket(
    generated_dir: &Path,
    file_name: &str,
    nodes: &mut Vec<AirNode>,
    written: &mut Vec<PathBuf>,
) -> Result<()> {
    if nodes.is_empty() {
        return Ok(());
    }
    nodes.sort_by(|left, right| node_sort_name(left).cmp(&node_sort_name(right)));
    let path = generated_dir.join(file_name);
    let mut bytes = serde_json::to_vec_pretty(nodes).context("encode generated AIR JSON")?;
    bytes.push(b'\n');
    fs::write(&path, bytes).with_context(|| format!("write generated AIR {}", path.display()))?;
    written.push(path);
    Ok(())
}

fn node_sort_name(node: &AirNode) -> Name {
    match node {
        AirNode::Defschema(value) => value.name.clone(),
        AirNode::Defmodule(value) => value.name.clone(),
        AirNode::Defworkflow(value) => value.name.clone(),
        AirNode::Defeffect(value) => value.name.clone(),
        AirNode::Defsecret(value) => value.name.clone(),
        AirNode::Manifest(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_generated_air_nodes_materializes_schemas_under_air_generated() {
        let temp = tempfile::tempdir().expect("tempdir");
        let written = write_generated_air_nodes(
            temp.path(),
            &[r#"{"$kind":"defschema","name":"demo/Generated@1","type":{"record":{"task":{"text":{}}}}}"#],
        )
        .expect("write generated AIR");

        assert_eq!(
            written,
            vec![temp.path().join("air/generated/schemas.air.json")]
        );
        let contents = fs::read_to_string(&written[0]).expect("read generated AIR");
        assert!(contents.contains(r#""$kind": "defschema""#));
        assert!(contents.contains(r#""name": "demo/Generated@1""#));
    }

    #[test]
    fn write_generated_air_export_json_materializes_collected_fragments() {
        let temp = tempfile::tempdir().expect("tempdir");
        let fragments = vec![
            r#"{"$kind":"defschema","name":"demo/State@1","type":{"record":{}}}"#,
            r#"{"$kind":"defmodule","name":"demo/Workflow_wasm@1","runtime":{"kind":"wasm","artifact":{"kind":"wasm_module"}}}"#,
            r#"{"$kind":"defworkflow","name":"demo/Workflow@1","state":"demo/State@1","event":"demo/Event@1","effects_emitted":[],"impl":{"module":"demo/Workflow_wasm@1","entrypoint":"step"}}"#,
        ];
        let export_json = serde_json::to_string(&fragments).expect("encode export payload");

        let written = write_generated_air_export_json(temp.path(), &export_json)
            .expect("write generated AIR");

        assert_eq!(
            written,
            vec![
                temp.path().join("air/generated/schemas.air.json"),
                temp.path().join("air/generated/module.air.json"),
                temp.path().join("air/generated/workflows.air.json"),
            ]
        );
        assert!(
            fs::read_to_string(temp.path().join("air/generated/module.air.json"))
                .expect("read module")
                .contains(r#""name": "demo/Workflow_wasm@1""#)
        );
        assert!(
            fs::read_to_string(temp.path().join("air/generated/workflows.air.json"))
                .expect("read workflow")
                .contains(r#""effects_emitted": []"#)
        );
    }

    #[test]
    fn write_generated_air_nodes_rejects_incomplete_manifest_schema_closure() {
        let temp = tempfile::tempdir().expect("tempdir");
        let err = write_generated_air_nodes(
            temp.path(),
            &[
                r#"{"$kind":"defschema","name":"demo/Outer@1","type":{"record":{"inner":{"ref":"demo/Inner@1"}}}}"#,
                r#"{"$kind":"defschema","name":"demo/Inner@1","type":{"record":{}}}"#,
                r#"{"$kind":"manifest","air_version":"2","schemas":[{"name":"demo/Outer@1"}],"modules":[],"workflows":[],"effects":[]}"#,
            ],
        )
        .expect_err("closure should be rejected");

        let message = err.to_string();
        assert!(message.contains("schema closure is incomplete"));
        assert!(message.contains("demo/Inner@1"));
    }

    #[test]
    fn write_generated_air_nodes_allows_external_schema_import_refs() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_generated_air_nodes(
            temp.path(),
            &[
                r#"{"$kind":"defschema","name":"demo/Outer@1","type":{"record":{"imported":{"ref":"external/Imported@1"}}}}"#,
                r#"{"$kind":"manifest","air_version":"2","schemas":[{"name":"demo/Outer@1"},{"name":"external/Imported@1"}],"modules":[],"workflows":[],"effects":[]}"#,
            ],
        )
        .expect("external refs are validated after sources are merged");
    }

    #[test]
    fn write_generated_air_nodes_rejects_unlisted_external_schema_refs() {
        let temp = tempfile::tempdir().expect("tempdir");
        let err = write_generated_air_nodes(
            temp.path(),
            &[
                r#"{"$kind":"defschema","name":"demo/Outer@1","type":{"record":{"imported":{"ref":"external/Imported@1"}}}}"#,
                r#"{"$kind":"manifest","air_version":"2","schemas":[{"name":"demo/Outer@1"}],"modules":[],"workflows":[],"effects":[]}"#,
            ],
        )
        .expect_err("unlisted external ref should be rejected");

        assert!(err.to_string().contains("external/Imported@1"));
    }
}
