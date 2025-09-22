use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tera::{Context as TeraContext, Tera};

use crate::github;
use crate::infer::InferredContext;

const START_TEMPLATE: &str = include_str!("../templates/start.md");

#[derive(Debug)]
pub struct StartResult {
    pub title: String,
    pub body: String,
    pub category: String,
    pub discussion_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct DiscussionCategory {
    pub id: u64,
    pub name: String,
}

#[derive(Debug, Deserialize)]
struct DiscussionResponse {
    pub html_url: String,
}

#[derive(Debug, Serialize)]
struct CreateDiscussionPayload<'a> {
    title: &'a str,
    body: &'a str,
    category_id: u64,
}

#[derive(Debug, Serialize)]
struct TemplateCrate<'a> {
    name: &'a str,
    version: String,
}

pub async fn run_start(ctx: &InferredContext, dry_run: bool) -> Result<StartResult> {
    let title = format!("{} Release Kickoff", ctx.repo_name);
    let body = render_body(ctx)?;

    if dry_run {
        return Ok(StartResult {
            title,
            body,
            category: String::from("Releases"),
            discussion_url: None,
        });
    }

    if !github::has_token() {
        bail!("missing ASFSHIP_GITHUB_TOKEN for GitHub Discussions");
    }

    let gh = github::client()?;
    let categories: Vec<DiscussionCategory> = gh
        .get(
            format!(
                "repos/{}/{}/discussions/categories",
                ctx.repo_owner, ctx.repo_name
            ),
            None::<&()>,
        )
        .await
        .with_context(|| {
            format!(
                "failed to load discussion categories for {}/{}",
                ctx.repo_owner, ctx.repo_name
            )
        })?;

    let category = choose_category(&categories)?;
    tracing::info!(category=%category.name, "start: using discussion category");

    let payload = CreateDiscussionPayload {
        title: &title,
        body: &body,
        category_id: category.id,
    };

    let discussion: DiscussionResponse = gh
        .post(
            format!("repos/{}/{}/discussions", ctx.repo_owner, ctx.repo_name),
            Some(&payload),
        )
        .await
        .with_context(|| {
            format!(
                "failed to create discussion in {}/{}",
                ctx.repo_owner, ctx.repo_name
            )
        })?;

    Ok(StartResult {
        title,
        body,
        category: category.name,
        discussion_url: Some(discussion.html_url),
    })
}

fn render_body(ctx: &InferredContext) -> Result<String> {
    let base_tag = ctx
        .last_stable_tag
        .clone()
        .unwrap_or_else(|| String::from("<none>"));
    let mut tera_ctx = TeraContext::new();
    tera_ctx.insert("repo", &ctx.repo_name);
    tera_ctx.insert("owner", &ctx.repo_owner);
    tera_ctx.insert("main_crate", &ctx.main_crate);
    tera_ctx.insert("base_tag", &base_tag);
    tera_ctx.insert("release_date", &String::from("TBD"));

    let crates: Vec<TemplateCrate<'_>> = ctx
        .crates
        .iter()
        .map(|c| TemplateCrate {
            name: &c.name,
            version: c.version.to_string(),
        })
        .collect();
    tera_ctx.insert("crates", &crates);

    Tera::one_off(START_TEMPLATE, &tera_ctx, false)
        .map_err(|err| anyhow::anyhow!("failed to render start template: {}", err))
}

fn choose_category(categories: &[DiscussionCategory]) -> Result<DiscussionCategory> {
    if categories.is_empty() {
        bail!("repository has no discussion categories; enable GitHub Discussions first");
    }
    let choice = categories
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case("Releases"))
        .or_else(|| categories.iter().next())
        .expect("non-empty categories");
    Ok(choice.clone())
}
