use std::{
    collections::BTreeMap,
    path::{PathBuf, absolute},
    sync::Arc,
};

use anyhow::{Context, Result};
use cargo_toml::Manifest;
use tokio::sync::RwLock;

static MANIFEST_CACHE: RwLock<BTreeMap<String, Arc<Manifest>>> = RwLock::const_new(BTreeMap::new());

pub async fn get_workspace_manifest() -> Result<Arc<Manifest>> {
    get_manifest("").await.context("get workspace manifest")
}

pub async fn get_manifest(crate_path: &str) -> Result<Arc<Manifest>> {
    let cache = MANIFEST_CACHE.read().await;
    if let Some(manifest) = cache.get(crate_path) {
        return Ok(manifest.clone());
    }
    drop(cache);

    let manifest = Arc::new(
        load_manifest(crate_path)
            .await
            .with_context(|| format!("load manifest for crate at {crate_path}"))?,
    );

    let mut write_cache = MANIFEST_CACHE.write().await;
    write_cache.insert(crate_path.to_owned(), manifest.clone());

    return Ok(manifest);
}

async fn load_manifest(crate_path: &str) -> Result<Manifest> {
    let toml_path: PathBuf = [crate_path, "Cargo.toml"].iter().collect();
    let toml_path = absolute(toml_path)?;

    Ok(Manifest::from_path(dbg!(toml_path))?)
}
