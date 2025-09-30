use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use tokio::fs as async_fs;
use tokio::process::Command;

use crate::github;
use crate::infer::InferredContext;
use crate::rc_release::{RcReleaseInfo, download_assets, fetch_latest_rc_release};

const SVN_BASE: &str = "https://dist.apache.org/repos/dist/dev";

pub async fn run_sync(ctx: &InferredContext, dry_run: bool) -> Result<()> {
    if !github::has_token() {
        bail!("missing ASFSHIP_GITHUB_TOKEN for sync command");
    }

    let release = fetch_latest_rc_release(&ctx.repo_owner, &ctx.repo_name).await?;
    let svn_target = format!(
        "{}/{}/{}",
        SVN_BASE,
        ctx.repo_name,
        release.svn_path_component(&ctx.repo_name)
    );

    if dry_run {
        println!(
            "sync: dry-run (tag={} assets={} svn_target={})",
            release.tag,
            release.assets.len(),
            svn_target
        );
        for asset in &release.assets {
            println!("- {} ({} bytes)", asset.name, asset.size);
        }
        return Ok(());
    }

    let download_dir = ctx
        .repo_root
        .join("target")
        .join("asfship")
        .join("sync")
        .join(release.tag.replace('/', "_"));
    let files = download_assets(&release, &download_dir).await?;
    perform_svn_sync(&svn_target, &download_dir, &files, &release, ctx).await?;
    Ok(())
}

async fn perform_svn_sync(
    svn_url: &str,
    download_dir: &Path,
    files: &[PathBuf],
    release: &RcReleaseInfo,
    ctx: &InferredContext,
) -> Result<()> {
    let checkout_dir = download_dir.join("svn");
    async_fs::create_dir_all(&checkout_dir).await?;

    run_svn([
        "checkout",
        "--depth",
        "empty",
        svn_url,
        checkout_dir.to_str().unwrap(),
    ])
    .await?;
    run_svn(["update", checkout_dir.to_str().unwrap()]).await?;

    for file in files {
        let file_name = file
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow!("invalid file name"))?;
        let dest = checkout_dir.join(file_name);
        async_fs::copy(file, dest).await?;
    }

    run_svn_in(&checkout_dir, ["add", "--force", "."]).await?;

    let message = format!(
        "Add {} {}{} artifacts (uploaded by asfship)",
        ctx.repo_name,
        release.base_version_string(),
        release.rc_suffix()
    );
    run_svn_in(&checkout_dir, ["commit", "-m", &message]).await?;

    println!("sync: committed {} assets to {}", files.len(), svn_url);
    Ok(())
}

async fn run_svn<I, S>(args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let status = Command::new("svn").args(args).status().await?;
    if !status.success() {
        bail!("svn command failed with status: {}", status);
    }
    Ok(())
}

async fn run_svn_in<I, S>(dir: &Path, args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let status = Command::new("svn")
        .current_dir(dir)
        .args(args)
        .status()
        .await?;
    if !status.success() {
        bail!("svn command failed with status: {}", status);
    }
    Ok(())
}
