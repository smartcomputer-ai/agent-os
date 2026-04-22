use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use aos_air_types::{AirNode, Name};

use crate::manifest_loader::parse_air_nodes_from_str;

pub const GENERATED_AIR_DIR: &str = "air/generated";

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
}
