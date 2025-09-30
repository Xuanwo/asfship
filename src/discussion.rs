use anyhow::{Context, Result, bail};
use octocrab::Octocrab;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct DiscussionCategory {
    pub id: u64,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct DiscussionResponse {
    pub html_url: String,
}

#[derive(Debug, Serialize)]
pub struct CreateDiscussionPayload<'a> {
    pub title: &'a str,
    pub body: &'a str,
    pub category_id: u64,
}

pub async fn fetch_default_category(
    gh: &Octocrab,
    owner: &str,
    repo: &str,
) -> Result<DiscussionCategory> {
    let categories: Vec<DiscussionCategory> = gh
        .get(
            format!("repos/{}/{}/discussions/categories", owner, repo),
            None::<&()>,
        )
        .await
        .with_context(|| {
            format!(
                "failed to load discussion categories for {}/{}",
                owner, repo
            )
        })?;
    choose_category(&categories)
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
