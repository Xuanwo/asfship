use anyhow::Result;

use crate::infer::{InferredContext, build_context};

pub async fn run_preflight() -> Result<InferredContext> {
    // Phase 1 preflight: ensure clean repo, infer remote, owner/name, workspace crates,
    // main crate, and the last stable tag. Execute blocking work off the async runtime.
    tracing::debug!("preflight: start");
    let ctx = build_context().await?;
    tracing::debug!(
        "preflight: done repo={}/{} main={}",
        ctx.repo_owner,
        ctx.repo_name,
        ctx.main_crate
    );
    Ok(ctx)
}
