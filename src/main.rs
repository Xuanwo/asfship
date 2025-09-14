mod config;
mod infer;
mod preflight;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

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
    let cli = Cli::parse();

    // Shared preflight and inference used by all commands in Phase 1
    let ctx = preflight::run_preflight()
        .await
        .context("preflight checks failed")?;

    match cli.command {
        Commands::Start => {
            println!(
                "start: ready (repo={}/{} main_crate={})",
                ctx.repo_owner, ctx.repo_name, ctx.main_crate
            );
        }
        Commands::Prerelease => {
            println!(
                "prerelease: ready (base_tag={} changed_crates={})",
                ctx.last_stable_tag.as_deref().unwrap_or("<none>"),
                ctx.crates.len()
            );
        }
        Commands::Sync => {
            println!(
                "sync: ready (latest_rc_base={})",
                ctx.last_stable_tag.as_deref().unwrap_or("<none>")
            );
        }
        Commands::Vote => {
            println!(
                "vote: ready (repo={}/{} tag_base={})",
                ctx.repo_owner,
                ctx.repo_name,
                ctx.last_stable_tag.as_deref().unwrap_or("<none>")
            );
        }
        Commands::Release => {
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
