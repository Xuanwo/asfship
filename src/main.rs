mod config;
mod discussion;
mod github;
mod infer;
mod preflight;
mod rc_release;
mod release_cmd;
mod start;
mod sync;
mod versioning;
mod vote;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::{EnvFilter, fmt};

#[derive(Parser, Debug)]
#[command(name = "asfship", version, about = "ASF release helper", long_about = None)]
struct Cli {
    /// Perform a dry run without mutating repo or network state
    #[arg(global = true, long = "dry-run", default_value_t = false)]
    dry_run: bool,

    /// Override artifact output directory (defaults to target/asfship/<tag>)
    #[arg(global = true, long = "artifact-dir")]
    artifact_dir: Option<PathBuf>,

    /// Skip pushing/uploads and only produce local artifacts
    #[arg(global = true, long = "local-assets", default_value_t = false)]
    local_assets: bool,

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
            match start::run_start(&ctx, cli.dry_run).await {
                Ok(result) => {
                    if let Some(url) = result.discussion_url {
                        println!(
                            "start: discussion created (category={} url={})",
                            result.category, url
                        );
                    } else {
                        println!(
                            "start: dry-run (category={} title={})",
                            result.category, result.title
                        );
                        println!("---\n{}", result.body);
                    }
                }
                Err(err) => {
                    eprintln!("Error: {}", err);
                    tracing::error!(error=%err, "start command failed");
                    std::process::exit(1);
                }
            }
        }
        Commands::Prerelease => {
            tracing::info!("prerelease: begin base_tag={:?}", ctx.last_stable_tag);
            let opts = versioning::PrereleaseOptions {
                dry_run: cli.dry_run,
                artifact_dir: cli.artifact_dir.as_deref(),
                upload: !cli.local_assets,
            };
            match versioning::run_prerelease(&ctx, opts).await {
                Ok(report) => {
                    println!("{}", report.render_text());
                }
                Err(e) => {
                    eprintln!("Error: {}", e);
                    tracing::error!(error=%e, "prerelease failed");
                    std::process::exit(1);
                }
            }
        }
        Commands::Sync => {
            tracing::info!("sync: begin");
            if let Err(e) = sync::run_sync(&ctx, cli.dry_run).await {
                eprintln!("Error: {}", e);
                tracing::error!(error=%e, "sync failed");
                std::process::exit(1);
            }
        }
        Commands::Vote => {
            tracing::info!("vote: begin");
            if let Err(e) = vote::run_vote(&ctx, cli.dry_run).await {
                eprintln!("Error: {}", e);
                tracing::error!(error=%e, "vote failed");
                std::process::exit(1);
            }
        }
        Commands::Release => {
            tracing::info!("release: begin");
            if let Err(e) = release_cmd::run_release(&ctx, cli.dry_run).await {
                eprintln!("Error: {}", e);
                tracing::error!(error=%e, "release failed");
                std::process::exit(1);
            }
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
