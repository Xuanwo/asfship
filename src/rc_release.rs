use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use octocrab::models::repos::Release;
use regex::Regex;
use semver::Version;
use tokio::fs as async_fs;

use crate::github;

#[derive(Debug, Clone)]
pub struct RcReleaseInfo {
    pub tag: String,
    pub version: Version,
    pub rc_number: u32,
    pub assets: Vec<RcAsset>,
}

impl RcReleaseInfo {
    pub fn rc_suffix(&self) -> String {
        format!("-rc{}", self.rc_number)
    }

    pub fn base_version_string(&self) -> String {
        self.version.to_string()
    }

    pub fn stable_tag(&self) -> String {
        format!("v{}", self.base_version_string())
    }

    pub fn svn_path_component(&self, repo_name: &str) -> String {
        format!(
            "{}-{}{}",
            repo_name,
            self.base_version_string(),
            self.rc_suffix()
        )
    }
}

#[derive(Debug, Clone)]
pub struct RcAsset {
    pub name: String,
    pub download_url: String,
    pub size: u64,
}

impl RcAsset {
    pub fn is_checksum(&self) -> bool {
        self.name.ends_with(".sha512")
    }
}

pub async fn fetch_latest_rc_release(owner: &str, repo: &str) -> Result<RcReleaseInfo> {
    let gh = github::client()?;
    let releases = gh
        .repos(owner.to_string(), repo.to_string())
        .releases()
        .list()
        .per_page(25)
        .send()
        .await?;

    let mut page = releases;
    loop {
        if let Some(info) = select_rc_release(&page.items)? {
            return Ok(info);
        }
        if let Some(next) = gh.get_page::<Release>(&page.next).await? {
            page = next;
        } else {
            break;
        }
    }

    bail!("no rc release found for {}/{}", owner, repo)
}

fn select_rc_release(releases: &[Release]) -> Result<Option<RcReleaseInfo>> {
    for release in releases {
        if let Some(info) = try_build_rc_release(release)? {
            return Ok(Some(info));
        }
    }
    Ok(None)
}

fn try_build_rc_release(release: &Release) -> Result<Option<RcReleaseInfo>> {
    if release.draft {
        return Ok(None);
    }
    let tag = release.tag_name.clone();
    let rc_re = Regex::new(r"^v(?P<version>\d+\.\d+\.\d+)-rc\.(?P<rc>\d+)$").unwrap();
    let caps = match rc_re.captures(&tag) {
        Some(caps) => caps,
        None => return Ok(None),
    };
    let version_str = caps
        .name("version")
        .map(|m| m.as_str())
        .context("rc capture missing version")?;
    let rc_number: u32 = caps
        .name("rc")
        .map(|m| m.as_str())
        .context("rc capture missing rc number")?
        .parse()?;
    let version = Version::parse(version_str)?;

    let assets = release
        .assets
        .iter()
        .map(|asset| RcAsset {
            name: asset.name.clone(),
            download_url: asset.browser_download_url.to_string(),
            size: asset.size as u64,
        })
        .collect();

    Ok(Some(RcReleaseInfo {
        tag,
        version,
        rc_number,
        assets,
    }))
}

pub async fn download_assets(info: &RcReleaseInfo, dir: &Path) -> Result<Vec<PathBuf>> {
    let client = reqwest::Client::new();
    async_fs::create_dir_all(dir).await?;
    let mut paths = Vec::new();
    for asset in &info.assets {
        let target = dir.join(&asset.name);
        let resp = client.get(&asset.download_url).send().await?;
        if !resp.status().is_success() {
            bail!("failed to download {}: {}", asset.name, resp.status());
        }
        let bytes = resp.bytes().await?;
        async_fs::write(&target, &bytes).await?;
        paths.push(target);
    }
    Ok(paths)
}
