use anyhow::{Context, Result};
use octocrab::Octocrab;

/// Return true if ASFSHIP_GITHUB_TOKEN is present and non-empty.
pub fn has_token() -> bool {
    std::env::var("ASFSHIP_GITHUB_TOKEN")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

/// Fetch the GitHub token from the environment.
pub fn token() -> Result<String> {
    match std::env::var("ASFSHIP_GITHUB_TOKEN") {
        Ok(token) if !token.is_empty() => Ok(token),
        _ => Err(anyhow::anyhow!(
            "missing ASFSHIP_GITHUB_TOKEN for GitHub API"
        )),
    }
}

/// Build an authenticated Octocrab client using the token.
pub fn client() -> Result<Octocrab> {
    let token = token()?;
    Octocrab::builder()
        .personal_token(token)
        .build()
        .context("failed to build GitHub client")
}
