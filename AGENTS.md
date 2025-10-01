Keep code and comments in English. CLI output and discussions with maintainers may be in Simplified Chinese; repository documentation remains English.

---

## 1. Purpose and Scope

asfship is a Rust CLI that helps Apache (ASF) project maintainers orchestrate releases across multi-crate Rust workspaces that follow Conventional Commits and SemVer. It automates:

- Opening release-related GitHub Discussions from templates.
- Computing per-crate version bumps from commit history and generating changelogs.
- Creating prerelease tags (rc), packaging source artifacts, and uploading them to GitHub Release assets.
- Syncing already-signed artifacts from GitHub to the ASF `dist/dev` SVN repo.
- Pushing final stable tags and opening the vote/release discussions with full per-crate version details.

Non-goals (initially):

- Automated GPG signing of artifacts (optional future add-on).
- Automated closing/summarization of votes.
- Cross-host SCM beyond Git/GitHub.

## 2. Terminology

- Workspace: A Cargo workspace with multiple crates.
- Main crate (aka main project): The crate that defines the project-level tag series (e.g., tag `v0.17.0` mirrors main crate version `0.17.0`).
- RC tag: A prerelease tag `vX.Y.Z-rc.N` attached to the repository.
- Stable tag: A release tag `vX.Y.Z` (no prerelease suffix).
- Discussion: GitHub Discussions created for start, vote, and release phases using templates.
- Release (GitHub Release object): The entity that holds assets for a tag (used for rc and stable tags).
- Repo name: The Git repository name, used for artifact naming and SVN paths.

## 3. Supported Workflow (High-Level)

1) `asfship start`
- Open a “start release” GitHub Discussion from template.

2) `asfship prerelease`
- Compute changes since the last stable tag.
- For each crate with changes, decide bump (SemVer + Conventional Commits, with pre-1.0 rules).
- Apply version bumps and generate/update each crate’s `CHANGELOG.md`.
- Commit with asfship identity and create/push `vX.Y.Z-rc.N` tag (auto-increment N).
- Create a GitHub Release (prerelease=true) for `vX.Y.Z-rc.N` and upload source artifacts.

3) `asfship sync`
- Download already-signed artifacts from the latest rc Release assets and `svn`-commit them to `dist/dev/<repo>/<repo>-<version>-rcN/`.

4) `asfship vote`
- Open a vote GitHub Discussion from template (includes links to SVN dev artifacts, verification steps, closing date, etc.).

5) `asfship release`
- Push stable tag `vX.Y.Z` (promoting the rc commit).
- Create a GitHub Release for `vX.Y.Z` and upload/reuse artifacts.
- Open a release GitHub Discussion from template, including full per-crate versions.

## 4. Versioning Rules

### 4.1 Conventional Commits → SemVer mapping

- Major: any commit marked as “breaking change”.
  - Headers with exclamation (e.g., `refactor!: xyz`, `refactor(!): xyz`).
  - Body footers with `BREAKING CHANGE:` (case-insensitive prefix).
- Minor: `feat:` scope when version >= 1.0.0.
- Patch: `fix:`, `perf:`, `refactor:`, `docs:`, `build:`, `chore:`, etc., when version >= 1.0.0.

### 4.2 Pre-1.0 policy

- When `< 1.0.0`, only “breaking change” increases the minor version.
- All other changes increase the patch version.

### 4.3 Multi-crate decision

- Bumps are computed per crate from commits that touch files under that crate’s directory (path-based mapping) and via an optional `affects:` commit footer.
- Crates with no changes are excluded from this release (no version change, no changelog entry).
- Project tag version is derived from the main crate’s new version. If the main crate has no changes since last stable, asfship does not produce a new rc by default.

## 5. Tagging and RC Handling

- Stable tags: `vX.Y.Z`.
- RC tags: `vX.Y.Z-rc.N`.
- When creating rc for the same base `X.Y.Z`, increment `N` by scanning existing tags.
- Latest stable tag is the most recent annotated tag matching `^v\d+\.\d+\.\d+$` reachable from the current branch.
- Latest rc tag for a base version is the highest `N` present for that `X.Y.Z`.

## 6. Changelog Generation

Per-crate `CHANGELOG.md` is updated with a new section per release using commit history since that crate’s previous version tag, grouped by type:

- Breaking Changes
- Features
- Fixes
- Refactor/Perf
- Docs/Build/Chore/Other

Entries include commit subject, short SHA, and optional PR reference if present.

Optional root-level release summary can be generated for Discussions using templates.

## 7. Packaging and Assets

- Packaging scope: per released crate (including the main crate when changed). Each changed crate produces its own source archive at the tag revision.
  - Method: `git archive` targeting the crate directory (excludes VCS metadata; excludes `target/`, `.github/` and other standard ignores).
  - Artifact naming (no configuration required):
    - Main crate: `apache-<repo>-<X.Y.Z>[-rcN]-src.tar.gz` and `.zip`.
    - Sub-crates: `apache-<repo>-<crate>-<X.Y.Z>[-rcN]-src.tar.gz` and `.zip`.
- Checksums: `.sha512` generated for each artifact.
- Signing: Optional future feature. For now, `sync` expects that signed files (`.asc`) are already present in GitHub Release assets.
- Upload: Attach all artifacts to the GitHub Release corresponding to the tag (rc or stable).

## 8. Git and GitHub Integration

- Read tags and commits via libgit2 (`git2`) wrapped in async helpers.
- Create tags (annotated), commits, and pushes via `git2` or `tokio::process::Command` for `git` when needed.
- GitHub API via `octocrab` using `ASFSHIP_GITHUB_TOKEN`. If missing, fallback to `gh` CLI when available.
- Discussions: created in a category named "Releases" (or the first available category if not present) with titles and bodies rendered from built-in templates.
- Releases: created for both rc and stable tags; rc releases marked `prerelease=true`.
- Rate limits and retries handled by `octocrab` with exponential backoff.

## 9. ASF `dist/dev` SVN Sync

- `asfship sync` downloads artifacts from the latest rc Release assets that match standard patterns: `.tar.gz`, `.zip`, `.sha512`, and optionally `.asc`.
- Destination path pattern:
  - `https://dist.apache.org/repos/dist/dev/<repo>/<repo>-<X.Y.Z>-rcN/`
- Use `tokio::process::Command` to run `svn checkout/add/commit`. Credentials must be configured in the environment.
- Commit message:
  - `Add <repo> <X.Y.Z>-rcN artifacts (uploaded by asfship)`

## 10. Configuration

Zero-config by default. asfship relies on conventions and repository introspection.

- Repo owner/name inferred from `git remote origin` URL.
- Crates discovered via `cargo metadata`.
- Main crate inferred as:
  1) Root `package` if present; else
  2) Crate whose name matches the repo; else
  3) The crate most depended upon by other workspace crates.
  If still ambiguous, asfship aborts with suggestions to add a minimal config file.

Optional minimal config file (only used to break ties):

Locations checked (first found wins):
- `./.asfship.toml`

Schema:

```toml
# .asfship.toml (optional)
main_crate = "reqsign"   # Only needed when inference is ambiguous
```

### 10.1 Template Variables

Available variables for built-in templates:

- `{repo}`: Repository name derived from git remote.
- `{version}`: `X.Y.Z`.
- `{rc_suffix}`: empty for stable, `-rcN` for rc.
- `{tag}`: `vX.Y.Z` or `vX.Y.Z-rc.N`.
- `{main_crate}`: Main crate name.
- `{release_date}`: ISO date.
- `{changelog}`: Combined workspace changelog (summary).
- `{crates}`: List of changed crates with `{name}`, `{old_version}`, `{new_version}`, `{changelog}`.
- `{artifacts}`: List of artifact tuples `{name}`, `{size}`, `{sha512}`, `{url}` (when available).
- `{svn_url}`: Destination SVN dev URL for this rc.
- `{vote_close_date}`: Vote end date (auto-suggested or omitted).

## 11. CLI Surface

Only one global option is supported: `--dry-run`.

```text
asfship start [--dry-run]
asfship prerelease [--dry-run]
asfship sync [--dry-run]
asfship vote [--dry-run]
asfship release [--dry-run]
```

Exit codes:

- `0` success; `2` repo state errors; `3` external tool/API errors; `4` tag conflicts.

## 12. Command Behaviors

### 12.1 `start`

1) Preflight (async): infer repo, remote, main crate, last stable tag.
2) Create a GitHub Discussion from built-in template.
3) Output the Discussion URL.

_Current implementation_: Shared preflight drives inference; dry-run prints the rendered template preview, and real runs create the Discussion via the GitHub API using the "Releases" category (falls back to the first available category when missing).

### 12.2 `prerelease`

1) Resolve last stable tag (`vX.Y.Z`).
2) Collect commits since base tag. Parse Conventional Commits; detect breaking changes (header `!` or body `BREAKING CHANGE:`).
3) Determine changed crates by file path touch and optional `affects:` footer; compute bump per crate using SemVer + pre-1.0 rules.
4) For each changed crate:
   - Update `Cargo.toml` version using `toml_edit`.
   - If other workspace crates depend on it, update dependency version constraints accordingly.
   - Update crate `CHANGELOG.md` by appending a section for the new version with grouped entries.
5) Compute main crate’s new version. If the main crate is unchanged, abort (no rc output).
6) Create a single commit `chore(release): prepare vX.Y.Z-rc.N` authored by asfship identity.
7) Create/push annotated tag `vX.Y.Z-rc.N`.
8) Create GitHub Release `prerelease=true` for the tag.
9) Package per-crate source artifacts and upload to the Release. Generate `.sha512` files. If signing is off, skip `.asc`.
10) Print summary (changed crates; new versions; assets).

Idempotency: If the exact rc tag already exists, abort with instructions and do not overwrite.

### 12.3 `sync`

1) Resolve target rc tag (default latest rc for the main version).
2) Fetch the tag’s GitHub Release assets.
3) Use `svn` to place assets under `dist/dev/<repo>/<repo>-<X.Y.Z>-rcN/`, commit with the default message.
4) Print committed paths.

### 12.4 `vote`

1) Resolve target rc tag and SVN dev URL for artifacts.
2) Render template with artifacts checksums, SVN URLs, verification steps, proposed close date.
3) Create the GitHub Discussion and print the URL.

### 12.5 `release`

1) Select rc tag to promote (or compute the latest rc for a base version).
2) Create stable tag `vX.Y.Z` at the same commit as the rc tag.
3) Create GitHub Release for `vX.Y.Z` (prerelease=false). Reuse rc assets when tag commit is identical.
4) Render and open release Discussion summarizing changed crates and versions.

## 13. Implementation Plan (Phased)

Phase 1 — CLI & Inference (MVP) — Status: implemented
- Skeleton CLI with subcommands and minimal config (optional).
- Shared preflight builds `InferredContext` (clean repo enforcement, remote detection, crate discovery, main crate inference, last stable tag lookup).
- `start` command renders the built-in template and opens GitHub Discussions (uses dry-run preview when requested).

Phase 2 — Versioning & Changelog — Status: implemented
- Per-crate change detection, SemVer bumping, and `Cargo.toml` edits via `toml_edit`.
- Dependent version updates across workspace manifests.
- Per-crate `CHANGELOG.md` regeneration with grouped entries.
- Release preparation commit created when not running in dry-run mode.
- `asfship prerelease --dry-run` now emits a human-readable summary of planned version bumps and commit highlights per crate.

Phase 3 — RC Tagging & Packaging — Status: implemented (prerelease path)
- Auto-increment rc tags, ensure idempotency, and create annotated tags.
- Push branch + tag, create GitHub prerelease, and upload per-crate archives with `.sha512` checksums.
- `--local-assets` skips push/upload while still producing local archives; `--artifact-dir` overrides output location.
- Asset upload uses bounded retries and packaging validates that all planned crates produce both tar/zip variants.

Phase 4 — Sync & Vote — Status: implemented
- `sync` downloads the latest rc release assets and shells out to `svn` for publishing to `dist/dev` (respects `--dry-run`).
- `vote` renders discussion bodies from `templates/vote.md`, pairs artifacts with sha512 values, and opens Discussions via the GitHub API.

Phase 5 — Stable Release — Status: implemented
- `release` promotes the latest rc tag to a stable tag, re-uploads assets, and posts a release Discussion from `templates/release.md`.
- Shared helpers cover retries for asset uploads and reuse the prerelease plan to summarize changed crates.

## 14. Libraries and Tools

- CLI: `clap` (derive) with global `--dry-run`.
- Runtime: `tokio` (multi-thread) — async-first.
- Git: `git2` wrapped in async functions (use `spawn_blocking` internally when needed). For pushes/tags if shelling out, use `tokio::process::Command`.
- GitHub: `octocrab` (async).
- SemVer: `semver`.
- Conventional Commits: light custom parser or `conventional_commit_parser` if suitable.
- TOML edits: `toml_edit`.
- Templates: `tera`.
- Checksums: `sha2` + `hex` or `tokio::process::Command` calling `shasum -a 512`.
- SVN: prefer `tokio::process::Command` for `svn` CLI invocations.

### 14.1 Async-First Guidelines

- Provide async functions as the public API for all I/O-heavy operations.
- Inside those functions, wrap blocking libraries (e.g., `git2`, `cargo_metadata::MetadataCommand::exec`) using `tokio::task::spawn_blocking` at the smallest viable granularity.
- Prefer native async clients when available (e.g., `octocrab` for GitHub, `tokio::process` for external commands, `tokio::fs` for filesystem).
- Do not `spawn_blocking` at the command entry; keep it localized to the actual blocking calls inside utilities.
- Ensure cancellation safety by avoiding long critical sections inside `spawn_blocking` and by chunking work when feasible.

## 15. Validation & Safety

- Each command performs a preflight check:
  - Git repo is clean and on a branch that tracks a remote.
  - Last stable tag is discoverable; warn if none.
  - Main crate can be inferred; if ambiguous, suggest adding `.asfship.toml` with `main_crate`.
  - `svn` and required CLIs present when needed.
  - GitHub auth available before network actions.

## 16. Logging and UX

- Log levels via `RUST_LOG` (default info) with concise progress messages.
- Clear failure hints (e.g., how to resolve tag conflicts, missing templates, ambiguous main crate).

## 17. Testing Strategy

- Unit tests: commit parsing, SemVer bump logic, rc numbering.
- Fixture-based tests: small git repos in `tests/fixtures` to cover multi-crate diffs and pre-1.0 rules.
- No network tests by default; GitHub/SVN calls behind traits with mock implementations.

## 18. Open Questions (to confirm)

1) When the main crate is unchanged but sub-crates changed, we abort prerelease by default. Is this acceptable as a hard rule (no override), or should we allow a minimal config flag in the future if needed?
2) Signing: keep as an external step for now; revisit once flows stabilize.
3) Per-crate artifacts: current spec produces one artifact per changed crate; confirm this aligns with ASF expectations for multi-crate Rust projects.

## 19. Example Templates (sketch)

`templates/start.md`
```markdown
# {repo} Release {version}{rc_suffix}: Start Discussion

This discussion tracks the start of the release process for {repo} {version}{rc_suffix}.

Planned scope:

{changelog}

Changed crates:
{% for c in crates %}
- {{ c.name }}: {{ c.old_version }} → {{ c.new_version }}
{% endfor %}
```

`templates/vote.md`
```markdown
# [VOTE] {repo} {version}{rc_suffix}

Artifacts are available at:
- SVN: {svn_url}

Artifacts and checksums:
{% for a in artifacts %}
- {{ a.name }} (sha512={{ a.sha512 }}) — {{ a.url }}
{% endfor %}

Please vote within the specified period. Proposed close date: {vote_close_date}.
```

`templates/release.md`
```markdown
# {repo} {version} Released

Summary:
{changelog}

Changed crates:
{% for c in crates %}
- {{ c.name }}: {{ c.old_version }} → {{ c.new_version }}
{% endfor %}
```

## 20. Known Limitations and Future Work

- GitHub and ASF SVN credentials must be provided out-of-band; no helper flows exist yet for interactive authentication or token validation.
- `asfship prerelease` aborts when the main crate has no changes even if leaf crates differ. Evaluate whether an override flag is warranted for patch-only utility releases.
- Artifact signing remains a manual step. We expect maintainers to upload `.asc` files before running `asfship sync`; automation is tracked as a future enhancement.
- Network interactions are untested in CI. Add integration tests or contract tests once we have hosted fixtures or a reliable sandbox for GitHub and SVN APIs.
- Template customization requires editing repository files directly. Consider adding a user config layer or template discovery rules if downstream projects need overrides outside the workspace root.

