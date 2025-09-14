use anyhow::Result;

use crate::infer::{InferredContext, build_context};

pub async fn run_preflight() -> Result<InferredContext> {
    // Phase 1 preflight: ensure clean repo, infer remote, owner/name, workspace crates,
    // main crate, and the last stable tag. Execute blocking work off the async runtime.
    let ctx = build_context().await?;
    Ok(ctx)
}
