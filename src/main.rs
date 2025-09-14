mod config;
mod infer;
mod preflight;
mod versioning;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::{EnvFilter, fmt};

#[derive(Parser, Debug)]
#[command(name = "asfship", version, about = "ASF release helper", long_about = None)]
struct Cli {
    /// Perform a dry run without mutating repo or network state
    #[arg(global = true, long = "dry-run", default_value_t = false)]
    dry_run: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start a release Discussion
    Start,
    /// Prepare a prerelease: bump versions, changelogs, tag rc, upload assets
    Prerelease,
    /// Sync latest rc assets to ASF dist/dev SVN
    Sync,
    /// Open a vote Discussion
    Vote,
    /// Push final tag and open release Discussion
    Release,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();

    // Shared preflight and inference used by all commands in Phase 1
    let ctx = preflight::run_preflight()
        .await
        .context("preflight checks failed")?;

    match cli.command {
        Commands::Start => {
            tracing::info!(
                "start: preflight ok repo={}/{} main={}",
                ctx.repo_owner,
                ctx.repo_name,
                ctx.main_crate
            );
            println!(
                "start: ready (repo={}/{} main_crate={})",
                ctx.repo_owner, ctx.repo_name, ctx.main_crate
            );
        }
        Commands::Prerelease => {
            tracing::info!("prerelease: begin base_tag={:?}", ctx.last_stable_tag);
            if let Err(e) = versioning::run_prerelease(&ctx, cli.dry_run).await {
                eprintln!("Error: {}", e);
                tracing::error!(error=%e, "prerelease failed");
                std::process::exit(1);
            }
            println!(
                "prerelease: ready (base_tag={} changed_crates={})",
                ctx.last_stable_tag.as_deref().unwrap_or("<none>"),
                ctx.crates.len()
            );
        }
        Commands::Sync => {
            tracing::info!("sync: preflight ok base={:?}", ctx.last_stable_tag);
            println!(
                "sync: ready (latest_rc_base={})",
                ctx.last_stable_tag.as_deref().unwrap_or("<none>")
            );
        }
        Commands::Vote => {
            tracing::info!("vote: preflight ok");
            println!(
                "vote: ready (repo={}/{} tag_base={})",
                ctx.repo_owner,
                ctx.repo_name,
                ctx.last_stable_tag.as_deref().unwrap_or("<none>")
            );
        }
        Commands::Release => {
            tracing::info!("release: preflight ok base={:?}", ctx.last_stable_tag);
            println!(
                "release: ready (repo={}/{} main_crate={} base_tag={})",
                ctx.repo_owner,
                ctx.repo_name,
                ctx.main_crate,
                ctx.last_stable_tag.as_deref().unwrap_or("<none>")
            );
        }
    }

    Ok(())
}

fn init_tracing() {
    // Only initialize if RUST_LOG (or env filter) is set; otherwise keep logs off by default.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("off"));
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
