use std::collections::HashMap;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{Duration, Utc};
use reqwest::Client;
use serde::Serialize;
use tera::{Context as TeraContext, Tera};

use crate::discussion;
use crate::github;
use crate::infer::InferredContext;
use crate::rc_release::{RcAsset, RcReleaseInfo, fetch_latest_rc_release};

const VOTE_TEMPLATE: &str = include_str!("../templates/vote.md");

pub async fn run_vote(ctx: &InferredContext, dry_run: bool) -> Result<()> {
    if !github::has_token() {
        bail!("missing ASFSHIP_GITHUB_TOKEN for vote command");
    }

    let release = fetch_latest_rc_release(&ctx.repo_owner, &ctx.repo_name).await?;
    let artifacts = build_artifact_rows(&release).await?;
    let body = render_vote_body(ctx, &release, &artifacts)?;
    let title = format!(
        "[VOTE] {} {}{}",
        ctx.repo_name,
        release.base_version_string(),
        release.rc_suffix()
    );

    if dry_run {
        println!("vote: dry-run (title={})", title);
        println!("---\n{}", body);
        return Ok(());
    }

    let gh = github::client()?;
    let category = discussion::fetch_default_category(&gh, &ctx.repo_owner, &ctx.repo_name).await?;
    let payload = discussion::CreateDiscussionPayload {
        title: &title,
        body: &body,
        category_id: category.id,
    };

    let created: discussion::DiscussionResponse = gh
        .post(
            format!("repos/{}/{}/discussions", ctx.repo_owner, ctx.repo_name),
            Some(&payload),
        )
        .await?;

    println!(
        "vote: discussion created (category={} url={})",
        category.name, created.html_url
    );
    Ok(())
}

#[derive(Debug, Serialize)]
struct VoteTemplateArtifact {
    name: String,
    url: String,
    sha512: Option<String>,
}

async fn build_artifact_rows(release: &RcReleaseInfo) -> Result<Vec<VoteTemplateArtifact>> {
    let mut sha_map = fetch_sha512_map(&release.assets).await?;
    let mut rows = Vec::new();
    for asset in &release.assets {
        if asset.is_checksum() {
            continue;
        }
        rows.push(VoteTemplateArtifact {
            name: asset.name.clone(),
            url: asset.download_url.clone(),
            sha512: sha_map.remove(&asset.name),
        });
    }
    Ok(rows)
}

async fn fetch_sha512_map(assets: &[RcAsset]) -> Result<HashMap<String, String>> {
    let client = Client::new();
    let mut map = HashMap::new();
    for asset in assets {
        if !asset.is_checksum() {
            continue;
        }
        let base = asset
            .name
            .strip_suffix(".sha512")
            .context("invalid sha512 asset name")?
            .to_string();
        let text = client.get(&asset.download_url).send().await?.text().await?;
        map.insert(base, text.trim().to_string());
    }
    Ok(map)
}

fn render_vote_body(
    ctx: &InferredContext,
    release: &RcReleaseInfo,
    artifacts: &[VoteTemplateArtifact],
) -> Result<String> {
    let mut tera_ctx = TeraContext::new();
    let vote_close = (Utc::now() + Duration::days(3)).date_naive();
    tera_ctx.insert("repo", &ctx.repo_name);
    tera_ctx.insert("version", &release.base_version_string());
    tera_ctx.insert("rc_suffix", &release.rc_suffix());
    tera_ctx.insert(
        "svn_url",
        &format!(
            "https://dist.apache.org/repos/dist/dev/{}/{}",
            ctx.repo_name,
            release.svn_path_component(&ctx.repo_name)
        ),
    );
    tera_ctx.insert("artifacts", artifacts);
    tera_ctx.insert("vote_close_date", &vote_close.to_string());

    Tera::one_off(VOTE_TEMPLATE, &tera_ctx, false)
        .map_err(|err| anyhow!("failed to render vote template: {}", err))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infer::InferredContext;
    use crate::rc_release::{RcAsset, RcReleaseInfo};
    use semver::Version;
    use std::path::PathBuf;

    #[test]
    fn render_vote_body_formats_artifacts() {
        let ctx = InferredContext {
            repo_root: PathBuf::from("."),
            repo_owner: "apache".into(),
            repo_name: "foo".into(),
            crates: Vec::new(),
            main_crate: "foo".into(),
            last_stable_tag: Some("v0.1.0".into()),
        };
        let release = RcReleaseInfo {
            tag: "v0.1.1-rc.1".into(),
            version: Version::parse("0.1.1").unwrap(),
            rc_number: 1,
            assets: vec![RcAsset {
                name: "apache-foo-0.1.1-rc1-src.tar.gz".into(),
                download_url: "https://example.com/tar".into(),
                size: 10,
            }],
        };
        let artifacts = vec![VoteTemplateArtifact {
            name: "apache-foo-0.1.1-rc1-src.tar.gz".into(),
            url: "https://example.com/tar".into(),
            sha512: Some("abcd".into()),
        }];

        let rendered = render_vote_body(&ctx, &release, &artifacts).unwrap();
        assert!(rendered.contains("sha512=abcd"));
        assert!(rendered.contains("[VOTE]"));
    }
}
