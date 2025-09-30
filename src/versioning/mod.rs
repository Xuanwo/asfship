mod apply;
mod plan;
mod rc;

use anyhow::{Result, bail};
use git2::Repository;

use crate::github;
use crate::infer::InferredContext;

pub async fn run_prerelease(ctx: &InferredContext, dry_run: bool) -> Result<()> {
    let repo = Repository::discover(&ctx.repo_root)?;
    let plan = plan::compute_plan(&repo, ctx)?;
    tracing::info!(
        "versioning: plan computed changed_crates={}",
        plan.changed_count()
    );

    if plan.crate_plan(&ctx.main_crate).is_none() {
        bail!("main crate has no changes since base tag; aborting prerelease prep");
    }

    if dry_run {
        tracing::debug!("versioning: dry-run, skip applying changes");
        return Ok(());
    }

    tracing::info!("versioning: applying changes");
    apply::apply_changes(ctx, &plan)?;

    if github::has_token() {
        rc::execute_rc(&repo, ctx, &plan).await?;
    } else {
        tracing::warn!(
            "rc: skip tagging and packaging (set ASFSHIP_GITHUB_TOKEN to enable GitHub integration)"
        );
    }

    Ok(())
}
