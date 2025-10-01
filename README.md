# asfship

## Overview
asfship is a Rust command-line tool that helps Apache Software Foundation (ASF) project maintainers run consistent release cycles across multi-crate workspaces. It automates the repetitive pieces of start discussions, prerelease preparation, artifact packaging, rc distribution, votes, and final releases while following SemVer and Conventional Commit conventions.

## Key Features
- Shared preflight that infers repository metadata, validates the workspace state, and prepares context for every command.
- `start`, `prerelease`, `sync`, `vote`, and `release` subcommands that map to the ASF release flow from initial coordination through publication.
- Automatic per-crate version planning with SemVer rules (including pre-1.0 semantics) and Conventional Commit parsing.
- Changelog generation, workspace dependency updates, and release tagging with rc iteration support.
- Artifact packaging for each changed crate, checksum generation, and optional upload to GitHub Releases.
- GitHub Discussions, Releases, and ASF `dist/dev` SVN integration with dry-run previews for review before mutation.

## Architecture Highlights
- **Preflight and inference** (`preflight`, `infer`): discover workspace crates, infer the main crate, confirm clean git state, and record last stable tags.
- **Version planning and packaging** (`versioning`, `rc_release`): compute bump plans, edit manifests and changelogs, and emit rc/stable release artifacts.
- **Collaboration surfaces** (`discussion`, `github`, `start`, `vote`, `release_cmd`): render Tera templates and talk to GitHub APIs for Discussions and Releases.
- **Distribution sync** (`sync`): replicate release candidate artifacts into the ASF `dist/dev` SVN tree with safeguards for dry-run review.
- **CLI entrypoint** (`main`): wires global flags such as `--dry-run`, `--artifact-dir`, and `--local-assets`, delegating to async command implementations.

## Installation
- Requires the Rust stable toolchain with `rustfmt` and `clippy` components (see `rust-toolchain.toml`).
- Ensure `svn` and the GitHub CLI `gh` are installed if you plan to sync artifacts or fall back to shell commands.
- Install from source with `cargo install --path .` or build locally using `cargo build --release` and run `target/release/asfship`.
- Provide a GitHub personal access token via `ASFSHIP_GITHUB_TOKEN` for API access; commands that call GitHub or upload assets require it.
- Configure ASF SVN credentials in your environment before running `asfship sync`.

## Quick Start
1. Clone the repository and make sure your workspace has the desired Conventional Commit history since the last stable tag.
2. Export `ASFSHIP_GITHUB_TOKEN` and verify that `svn` access to `https://dist.apache.org/repos/dist/dev` is configured on your machine.
3. Run `asfship start --dry-run` to preview the kickoff discussion body before posting it.
4. Execute `asfship prerelease` to generate version bumps, changelog updates, rc tags, and release artifacts. Use `--dry-run` to inspect the plan without mutating git or GitHub.
5. Use `asfship sync` to push rc artifacts into the ASF `dist/dev` tree, `asfship vote` to open the vote discussion, and `asfship release` to promote the rc to a stable release when the vote succeeds.
6. Refer to the advanced topics below for customization, template details, and contribution guidance.

## Additional Resources
- Advanced configuration and customization: see [docs/advanced-configuration.md](docs/advanced-configuration.md).
- Template variables and generated artifacts: see [docs/templates-and-artifacts.md](docs/templates-and-artifacts.md).
- Development workflow and contribution guidelines: see [CONTRIBUTING.md](CONTRIBUTING.md).
- Known limitations and future work notes: see [AGENTS.md](AGENTS.md).

## License
License information has not been finalized. Please verify the repository status or reach out to the maintainers before relying on a specific license.
