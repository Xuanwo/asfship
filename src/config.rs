use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
pub struct MinimalConfig {
    pub main_crate: Option<String>,
}

pub async fn load_minimal_config(repo_root: &Path) -> Result<MinimalConfig> {
    let path = repo_root.join(".asfship.toml");
    if !path.exists() {
        return Ok(MinimalConfig::default());
    }
    let content = tokio::fs::read_to_string(&path)
        .await
        .with_context(|| format!("failed to read {}", path.display()))?;
    let cfg: MinimalConfig =
        toml::from_str(&content).with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(cfg)
}
