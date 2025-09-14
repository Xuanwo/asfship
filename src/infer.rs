use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use cargo_metadata::{CargoOpt, Metadata, MetadataCommand, Package, PackageId};
use git2::{Repository, StatusOptions};
use regex::Regex;

use crate::config::load_minimal_config;

#[derive(Debug, Clone)]
pub struct CrateInfo {
    pub name: String,
    pub version: semver::Version,
    pub manifest_path: PathBuf,
    pub package_root: PathBuf,
    pub internal_dep_count: usize,
}

#[derive(Debug, Clone)]
pub struct InferredContext {
    pub repo_root: PathBuf,
    pub repo_owner: String,
    pub repo_name: String,
    pub crates: Vec<CrateInfo>,
    pub main_crate: String,
    pub last_stable_tag: Option<String>,
}

pub async fn repo_root() -> Result<PathBuf> {
    tracing::trace!("infer: discovering repo root");
    tokio::task::spawn_blocking(|| {
        let repo = Repository::discover(".")?;
        Ok::<_, anyhow::Error>(repo.workdir().unwrap_or(repo.path()).to_path_buf())
    })
    .await
    .map_err(|e| anyhow::anyhow!("repo_root task join error: {}", e))?
}

pub async fn ensure_clean_repo(root: &Path) -> Result<()> {
    let root = root.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let repo = Repository::discover(root)?;
        let mut opts = StatusOptions::new();
        opts.include_untracked(true).recurse_untracked_dirs(true);
        let statuses = repo.statuses(Some(&mut opts))?;
        let dirty = statuses.iter().any(|s| {
            s.status().intersects(
                git2::Status::INDEX_NEW
                    | git2::Status::INDEX_MODIFIED
                    | git2::Status::INDEX_DELETED
                    | git2::Status::WT_NEW
                    | git2::Status::WT_MODIFIED
                    | git2::Status::WT_DELETED,
            )
        });
        if dirty {
            bail!("working tree is not clean");
        }
        Ok::<_, anyhow::Error>(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("ensure_clean_repo task join error: {}", e))??;
    Ok(())
}

pub async fn infer_remote(root: &Path) -> Result<(String, String, String)> {
    let root = root.to_path_buf();
    tokio::task::spawn_blocking(move || {
        // returns (owner, name, url)
        let repo = Repository::discover(root)?;
        let remotes = repo.remotes()?;
        let mut chosen: Option<String> = None;
        if let Some(name) = remotes.iter().flatten().find(|r| *r == "origin") {
            chosen = Some(name.to_string());
        } else if let Some(first) = remotes.iter().flatten().next() {
            chosen = Some(first.to_string());
        }
        let name = chosen.ok_or_else(|| anyhow::anyhow!("no git remotes found"))?;
        let remote = repo.find_remote(&name)?;
        let url = remote
            .url()
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("remote has no URL"))?;

        // Parse GitHub owner/repo from SSH or HTTPS URL
        let ssh =
            Regex::new(r"^git@github\.com:(?P<owner>[^/]+)/(?P<repo>[^/]+?)(?:\.git)?$").unwrap();
        let https =
            Regex::new(r"^https?://github\.com/(?P<owner>[^/]+)/(?P<repo>[^/]+?)(?:\.git)?$")
                .unwrap();
        let (owner, repo_name) = if let Some(c) = ssh.captures(&url) {
            (c["owner"].to_string(), c["repo"].to_string())
        } else if let Some(c) = https.captures(&url) {
            (c["owner"].to_string(), c["repo"].to_string())
        } else {
            bail!("unsupported remote URL (expected GitHub): {}", url);
        };

        Ok::<_, anyhow::Error>((owner, repo_name, url))
    })
    .await
    .map_err(|e| anyhow::anyhow!("infer_remote task join error: {}", e))?
}

pub async fn load_metadata() -> Result<Metadata> {
    tokio::task::spawn_blocking(|| {
        let mut cmd = MetadataCommand::new();
        cmd.features(CargoOpt::AllFeatures);
        let meta = cmd.exec()?;
        Ok::<_, anyhow::Error>(meta)
    })
    .await
    .map_err(|e| anyhow::anyhow!("cargo metadata task join error: {}", e))?
}

pub fn collect_crates(meta: &Metadata) -> Result<Vec<CrateInfo>> {
    let workspace_ids: Vec<_> = meta.workspace_members.to_vec();
    let ws_set: std::collections::HashSet<_> = workspace_ids.iter().collect();
    let mut result = Vec::new();

    // Precompute internal dependency counts
    let mut internal_counts: std::collections::HashMap<PackageId, usize> =
        std::collections::HashMap::new();
    for pkg in &meta.packages {
        if !ws_set.contains(&pkg.id) {
            continue;
        }
        for dep in &pkg.dependencies {
            if let Some(dep_pkg) = meta.packages.iter().find(|p| p.name == dep.name)
                && ws_set.contains(&dep_pkg.id)
            {
                *internal_counts.entry(dep_pkg.id.clone()).or_default() += 1;
            }
        }
    }

    for pkg in &meta.packages {
        if !ws_set.contains(&pkg.id) {
            continue;
        }
        let count = internal_counts.get(&pkg.id).copied().unwrap_or(0);
        let manifest_path = PathBuf::from(&pkg.manifest_path);
        let package_root = manifest_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        result.push(CrateInfo {
            name: pkg.name.clone(),
            version: semver::Version::parse(&pkg.version.to_string())
                .unwrap_or_else(|_| semver::Version::new(0, 1, 0)),
            manifest_path,
            package_root,
            internal_dep_count: count,
        });
    }

    Ok(result)
}

fn root_package(meta: &Metadata) -> Option<&Package> {
    meta.root_package()
}

// async variant defined below

pub async fn find_last_stable_tag(root: &Path) -> Result<Option<String>> {
    let root = root.to_path_buf();
    tokio::task::spawn_blocking(move || {
        // Find the latest annotated tag that matches vX.Y.Z
        // Fallback: none
        let repo = Repository::discover(root)?;
        let tag_re = Regex::new(r"^v\d+\.\d+\.\d+$").unwrap();
        let mut tags = Vec::new();
        for r in repo.references()?.flatten() {
            if let Some(name) = r
                .shorthand()
                .filter(|name| r.is_tag() && tag_re.is_match(name))
            {
                tags.push(name.to_string());
            }
        }
        // Sort by tag name semver descending
        tags.sort_by(|a, b| semver_cmp(b, a));
        Ok::<_, anyhow::Error>(tags.first().cloned())
    })
    .await
    .map_err(|e| anyhow::anyhow!("find_last_stable_tag task join error: {}", e))?
}

fn semver_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let pa = a.trim_start_matches('v');
    let pb = b.trim_start_matches('v');
    let va = semver::Version::parse(pa);
    let vb = semver::Version::parse(pb);
    match (va, vb) {
        (Ok(va), Ok(vb)) => va.cmp(&vb),
        _ => a.cmp(b),
    }
}

pub async fn build_context() -> Result<InferredContext> {
    let root = repo_root().await?;
    ensure_clean_repo(&root).await?;
    let (owner, name, _remote_url) = infer_remote(&root).await?;
    let meta = load_metadata().await?;
    let crates = collect_crates(&meta)?;
    let main_crate = infer_main_crate(&crates, &meta, &name, &root).await?;
    let last = find_last_stable_tag(&root).await?;
    tracing::info!(
        "infer: ok owner={} repo={} crates={} main={} base_tag={:?}",
        owner,
        name,
        crates.len(),
        main_crate,
        last
    );
    Ok(InferredContext {
        repo_root: root,
        repo_owner: owner,
        repo_name: name,
        crates,
        main_crate,
        last_stable_tag: last,
    })
}

pub async fn infer_main_crate(
    crates: &[CrateInfo],
    meta: &Metadata,
    repo_name: &str,
    repo_root: &Path,
) -> Result<String> {
    let cfg = load_minimal_config(repo_root).await.unwrap_or_default();
    if let Some(name) = cfg.main_crate {
        if crates.iter().any(|c| c.name == name) {
            return Ok(name);
        } else {
            bail!("main_crate specified but not found in workspace: {}", name);
        }
    }

    if let Some(root) = root_package(meta)
        && crates.iter().any(|c| c.name == root.name)
    {
        return Ok(root.name.clone());
    }

    if let Some(by_name) = crates.iter().find(|c| c.name == repo_name) {
        return Ok(by_name.name.clone());
    }

    // Pick the crate with the highest number of internal dependents
    if let Some(max) = crates.iter().max_by_key(|c| c.internal_dep_count) {
        let top = crates
            .iter()
            .filter(|c| c.internal_dep_count == max.internal_dep_count)
            .min_by(|a, b| a.name.cmp(&b.name))
            .expect("at least one crate");
        return Ok(top.name.clone());
    }

    bail!("failed to infer main crate")
}
