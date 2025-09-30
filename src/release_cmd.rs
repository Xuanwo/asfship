use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use git2::{ObjectType, Oid, Repository};
use serde::Serialize;
use tera::{Context as TeraContext, Tera};
use tokio::process::Command;

use crate::discussion;
use crate::github;
use crate::infer::InferredContext;
use crate::rc_release::{RcReleaseInfo, download_assets, fetch_latest_rc_release};
use crate::versioning::rc::upload_assets_with_retry;
use crate::versioning::{Plan, compute_plan};
use reqwest::StatusCode;

const RELEASE_TEMPLATE: &str = include_str!("../templates/release.md");

pub async fn run_release(ctx: &InferredContext, dry_run: bool) -> Result<()> {
    if !github::has_token() {
        bail!("missing ASFSHIP_GITHUB_TOKEN for release command");
    }

    let repo = Repository::discover(&ctx.repo_root)?;
    let plan = compute_plan(&repo, ctx)?;
    if plan.changed_count() == 0 {
        bail!("no changed crates detected; nothing to release");
    }

    let release = fetch_latest_rc_release(&ctx.repo_owner, &ctx.repo_name).await?;
    let stable_tag = release.stable_tag();
    let rc_tag_ref = format!("refs/tags/{}", release.tag);
    let rc_obj = repo
        .revparse_single(&rc_tag_ref)
        .with_context(|| format!("failed to resolve rc tag {}", release.tag))?;
    let rc_commit = rc_obj
        .peel_to_commit()
        .context("rc tag does not point to a commit")?;

    let summaries = collect_summaries(&plan);

    if dry_run {
        println!(
            "release: dry-run (rc_tag={} stable_tag={} crates={})",
            release.tag,
            stable_tag,
            summaries.len()
        );
        for summary in &summaries {
            println!(
                "- {} {} -> {}",
                summary.name, summary.old_version, summary.new_version
            );
        }
        return Ok(());
    }

    ensure_tag_absent(&repo, &stable_tag)?;
    create_stable_tag(&repo, &stable_tag, rc_commit.id()).await?;
    push_tag(&ctx.repo_root, &stable_tag).await?;

    let gh = github::client()?;
    let repos_api = gh.repos(ctx.repo_owner.clone(), ctx.repo_name.clone());
    let releases_api = repos_api.releases();
    match releases_api.get_by_tag(&stable_tag).await {
        Ok(_) => bail!("GitHub release already exists for {}", stable_tag),
        Err(err) => {
            if !is_not_found(&err) {
                return Err(err.into());
            }
        }
    }

    let _ = releases_api
        .create(&stable_tag)
        .name(&stable_tag)
        .prerelease(false)
        .draft(false)
        .body("")
        .send()
        .await?;

    let asset_dir = ctx
        .repo_root
        .join("target")
        .join("asfship")
        .join("release")
        .join(stable_tag.replace('/', "_"));
    let files = download_assets(&release, &asset_dir).await?;
    upload_assets_with_retry(&ctx.repo_owner, &ctx.repo_name, &stable_tag, &files).await?;

    let body = render_release_body(ctx, &release, &summaries)?;
    let title = format!(
        "{} {} released",
        ctx.repo_name,
        release.base_version_string()
    );
    let category = discussion::fetch_default_category(&gh, &ctx.repo_owner, &ctx.repo_name).await?;
    let payload = discussion::CreateDiscussionPayload {
        title: &title,
        body: &body,
        category_id: category.id,
    };
    let discussion: discussion::DiscussionResponse = gh
        .post(
            format!("repos/{}/{}/discussions", ctx.repo_owner, ctx.repo_name),
            Some(&payload),
        )
        .await?;

    println!(
        "release: completed (stable_tag={} discussion={})",
        stable_tag, discussion.html_url
    );

    Ok(())
}

#[derive(Serialize)]
struct ReleaseCrateSummary {
    name: String,
    old_version: String,
    new_version: String,
}

fn collect_summaries(plan: &Plan) -> Vec<ReleaseCrateSummary> {
    let mut result = Vec::new();
    for (name, crate_plan) in plan.iter() {
        result.push(ReleaseCrateSummary {
            name: name.clone(),
            old_version: crate_plan.previous_version().to_string(),
            new_version: crate_plan.new_version().to_string(),
        });
    }
    result
}

fn render_release_body(
    ctx: &InferredContext,
    release: &RcReleaseInfo,
    crates: &[ReleaseCrateSummary],
) -> Result<String> {
    let mut tera_ctx = TeraContext::new();
    tera_ctx.insert("repo", &ctx.repo_name);
    tera_ctx.insert("version", &release.base_version_string());
    tera_ctx.insert("tag", &release.stable_tag());
    tera_ctx.insert("rc_tag", &release.tag);
    tera_ctx.insert("crates", crates);
    Tera::one_off(RELEASE_TEMPLATE, &tera_ctx, false)
        .map_err(|err| anyhow!("failed to render release template: {}", err))
}

fn ensure_tag_absent(repo: &Repository, tag: &str) -> Result<()> {
    if repo.refname_to_id(&format!("refs/tags/{}", tag)).is_ok() {
        bail!("stable tag already exists: {}", tag);
    }
    Ok(())
}

async fn create_stable_tag(repo: &Repository, tag: &str, target: Oid) -> Result<()> {
    let repo_path = repo
        .path()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let tag_name = tag.to_string();
    tokio::task::spawn_blocking(move || {
        let repo = Repository::discover(repo_path)?;
        let object = repo.find_object(target, Some(ObjectType::Commit))?;
        let sig = repo
            .signature()
            .or_else(|_| git2::Signature::now("asfship", "asfship@users.noreply.github.com"))?;
        let msg = format!("asfship release {}", tag_name);
        repo.tag(&tag_name, &object, &sig, &msg, true)?;
        Ok::<_, anyhow::Error>(())
    })
    .await
    .map_err(|e| anyhow!("create_stable_tag task join error: {}", e))??;
    Ok(())
}

async fn push_tag(repo_root: &Path, tag: &str) -> Result<()> {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("push")
        .arg("origin")
        .arg(format!("refs/tags/{}", tag))
        .status()
        .await?;
    if !status.success() {
        bail!("git push tag failed with status: {}", status);
    }
    Ok(())
}

fn is_not_found(err: &octocrab::Error) -> bool {
    if let octocrab::Error::GitHub { source, .. } = err {
        return source.status_code == StatusCode::NOT_FOUND;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infer::InferredContext;
    use crate::rc_release::{RcAsset, RcReleaseInfo};
    use semver::Version;
    use std::path::PathBuf;

    #[test]
    fn render_release_body_lists_crates() {
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
        let crates = vec![ReleaseCrateSummary {
            name: "foo".into(),
            old_version: "0.1.0".into(),
            new_version: "0.1.1".into(),
        }];

        let body = render_release_body(&ctx, &release, &crates).unwrap();
        assert!(body.contains("foo: 0.1.0 → 0.1.1"));
        assert!(body.contains("v0.1.1"));
    }
}
