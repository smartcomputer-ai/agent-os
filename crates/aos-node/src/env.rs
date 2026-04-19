use std::path::PathBuf;

pub fn load_dotenv_candidates() -> anyhow::Result<()> {
    for path in dotenv_candidates() {
        if !path.exists() {
            continue;
        }
        dotenvy::from_path(&path)?;
    }
    Ok(())
}

fn dotenv_candidates() -> Vec<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    vec![
        workspace_root().join(".env"),
        manifest_dir.join(".env"),
        PathBuf::from(".env"),
    ]
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
}
