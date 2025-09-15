use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use flate2::Compression;
use flate2::write::GzEncoder;
use git2::{Repository, Sort};
use octocrab::Octocrab;
use regex::Regex;
use sha2::{Digest, Sha512};
use std::io::Cursor;
use std::io::Write;
use tar::Builder as TarBuilder;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use toml_edit::{DocumentMut, value};
use zip::CompressionMethod as ZipCompression;
use zip::write::FileOptions as ZipOptions;
// url encoding for upload_url name param
use reqwest::StatusCode;
use reqwest::header;
use urlencoding::encode as url_encode;

use crate::infer::{CrateInfo, InferredContext};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum BumpKind {
    Major,
    Minor,
    Patch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommitKind {
    Breaking,
    Feat,
    Fix,
    Perf,
    Refactor,
    Docs,
    Build,
    Chore,
    Other,
}

#[derive(Debug, Clone)]
struct ChangeEntry {
    kind: CommitKind,
    subject: String,
    sha: String,
    breaking: bool,
}

#[derive(Debug, Clone)]
struct CratePlan {
    new_version: semver::Version,
    changes: Vec<ChangeEntry>,
}

#[derive(Debug, Clone)]
struct Plan {
    per_crate: BTreeMap<String, CratePlan>,
}

pub async fn run_prerelease(ctx: &InferredContext, dry_run: bool) -> Result<()> {
    let repo = Repository::discover(&ctx.repo_root)?;
    let plan = compute_plan(&repo, ctx)?;
    tracing::info!(
        "versioning: plan computed changed_crates={}",
        plan.per_crate.len()
    );

    // Ensure main crate has changes; otherwise abort.
    if !plan.per_crate.contains_key(&ctx.main_crate) {
        bail!("main crate has no changes since base tag; aborting prerelease prep");
    }

    if dry_run {
        // Log minimal line to keep current UX; detailed logs can be added later.
        tracing::debug!("versioning: dry-run, skip applying changes");
        return Ok(());
    }

    // Apply: update versions, dependent versions, changelogs, and commit.
    tracing::info!("versioning: applying changes");
    apply_changes(ctx, &plan)?;

    // Phase 3 — RC Tagging & Packaging (skip if no GitHub auth found)
    if has_github_auth() {
        let base_version = &plan.per_crate[&ctx.main_crate].new_version;
        let (rc_tag, rc_n) = next_rc_tag(&repo, base_version)?;
        tracing::info!("rc: choosing tag={} (rc={})", rc_tag, rc_n);

        ensure_tag_absent(&repo, &rc_tag)?;
        create_rc_tag(&repo, &rc_tag).await?;
        push_head_and_tag(&ctx.repo_root, &rc_tag).await?;

        // Create prerelease on GitHub and upload assets.
        create_github_prerelease(&ctx.repo_owner, &ctx.repo_name, &rc_tag).await?;
        let artifacts = package_changed_crates(ctx, &plan, &rc_tag, rc_n).await?;
        upload_assets(&ctx.repo_owner, &ctx.repo_name, &rc_tag, &artifacts).await?;
    } else {
        tracing::info!("rc: skip tagging and packaging (no GitHub auth detected)");
    }
    Ok(())
}

fn compute_plan(repo: &Repository, ctx: &InferredContext) -> Result<Plan> {
    let base_oid = if let Some(tag) = &ctx.last_stable_tag {
        // peel tag to commit
        let obj = repo
            .revparse_single(&format!("refs/tags/{}", tag))
            .or_else(|_| repo.revparse_single(tag))
            .context("failed to resolve last stable tag")?;
        let commit = obj
            .peel_to_commit()
            .context("tag does not point to a commit")?;
        Some(commit.id())
    } else {
        None
    };

    // Map package root prefix -> crate info
    let mut roots: Vec<(PathBuf, &CrateInfo)> = ctx
        .crates
        .iter()
        .map(|c| (c.package_root.clone(), c))
        .collect();
    // Sort longer paths first to prefer deeper matches
    roots.sort_by(|a, b| b.0.components().count().cmp(&a.0.components().count()));

    let mut per_crate_changes: HashMap<String, Vec<ChangeEntry>> = HashMap::new();

    // Walk commits from base..HEAD (exclusive of base)
    let mut walk = repo.revwalk()?;
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::REVERSE)?; // oldest..newest
    walk.push_head()?;
    if let Some(base) = base_oid {
        walk.hide(base)?; // exclude base commit
    }

    for oid in walk {
        let oid = oid?;
        let commit = repo.find_commit(oid)?;
        let subject = commit
            .summary()
            .map(|s| s.to_string())
            .unwrap_or_else(|| String::from("<no subject>"));
        let message = commit.message().unwrap_or("");
        let short = oid.to_string()[..7].to_string();

        // Detect breaking
        let breaking_header = subject.contains("!:")
            || subject.contains("(!):")
            || subject.starts_with(|c: char| c.is_alphabetic())
                && subject
                    .split(':')
                    .next()
                    .map(|t| t.ends_with('!'))
                    .unwrap_or(false);
        let breaking_body = message.to_ascii_uppercase().contains("BREAKING CHANGE:");
        let breaking = breaking_header || breaking_body;

        let kind = classify_commit(&subject, breaking);

        // Diff against first parent (or empty tree)
        let diffs = if commit.parent_count() > 0 {
            let parent = commit.parent(0)?;
            repo.diff_tree_to_tree(Some(&parent.tree()?), Some(&commit.tree()?), None)?
        } else {
            repo.diff_tree_to_tree(None, Some(&commit.tree()?), None)?
        };

        let mut touched: HashSet<String> = HashSet::new();
        diffs.foreach(
            &mut |delta, _| {
                if let Some(path) = delta.new_file().path().or_else(|| delta.old_file().path())
                    && let Some(name) = crate_for_path(&ctx.repo_root, &roots, path)
                {
                    touched.insert(name.to_string());
                }
                true
            },
            None,
            None,
            None,
        )?;

        for name in touched {
            per_crate_changes
                .entry(name)
                .or_default()
                .push(ChangeEntry {
                    kind,
                    subject: subject.clone(),
                    sha: short.clone(),
                    breaking,
                });
        }
    }

    let mut per_crate: BTreeMap<String, CratePlan> = BTreeMap::new();
    for c in &ctx.crates {
        if let Some(changes) = per_crate_changes.get(&c.name) {
            if changes.is_empty() {
                continue;
            }
            let bump = decide_bump(&c.version, changes);
            let mut new = c.version.clone();
            match bump {
                BumpKind::Major => {
                    new.major += 1;
                    new.minor = 0;
                    new.patch = 0;
                }
                BumpKind::Minor => {
                    new.minor += 1;
                    new.patch = 0;
                }
                BumpKind::Patch => {
                    new.patch += 1;
                }
            }
            per_crate.insert(
                c.name.clone(),
                CratePlan {
                    new_version: new,
                    changes: changes.clone(),
                },
            );
        }
    }

    Ok(Plan { per_crate })
}

fn crate_for_path<'a>(
    repo_root: &Path,
    roots: &'a [(PathBuf, &CrateInfo)],
    path: &Path,
) -> Option<&'a str> {
    // Make absolute to compare prefixes reliably
    let abs = repo_root.join(path);
    for (root, info) in roots {
        if abs.starts_with(root) {
            return Some(&info.name);
        }
    }
    None
}

fn classify_commit(subject: &str, breaking: bool) -> CommitKind {
    if breaking {
        return CommitKind::Breaking;
    }
    let lower = subject.to_ascii_lowercase();
    let ty = lower.split(':').next().unwrap_or("");
    match ty {
        t if t.starts_with("feat") => CommitKind::Feat,
        t if t.starts_with("fix") => CommitKind::Fix,
        t if t.starts_with("perf") => CommitKind::Perf,
        t if t.starts_with("refactor") => CommitKind::Refactor,
        t if t.starts_with("docs") => CommitKind::Docs,
        t if t.starts_with("build") => CommitKind::Build,
        t if t.starts_with("chore") => CommitKind::Chore,
        _ => CommitKind::Other,
    }
}

fn decide_bump(current: &semver::Version, changes: &[ChangeEntry]) -> BumpKind {
    let breaking = changes.iter().any(|c| c.breaking);
    if current.major >= 1 {
        if breaking {
            return BumpKind::Major;
        }
        if changes.iter().any(|c| matches!(c.kind, CommitKind::Feat)) {
            return BumpKind::Minor;
        }
        return BumpKind::Patch;
    }
    // pre-1.0 policy
    if breaking {
        BumpKind::Minor
    } else {
        BumpKind::Patch
    }
}

fn apply_changes(ctx: &InferredContext, plan: &Plan) -> Result<()> {
    // Map crate -> new version
    let mut changed_versions: HashMap<&str, semver::Version> = HashMap::new();
    for (name, cp) in &plan.per_crate {
        changed_versions.insert(name.as_str(), cp.new_version.clone());
    }

    // Update each changed crate's Cargo.toml and CHANGELOG.md
    for c in &ctx.crates {
        if let Some(cp) = plan.per_crate.get(&c.name) {
            tracing::debug!(
                "update version + changelog crate={} old={} new={}",
                c.name,
                c.version,
                cp.new_version
            );
            update_package_version(&c.manifest_path, &cp.new_version)?;
            update_changelog(&c.package_root, &c.name, &cp.new_version, &cp.changes)?;
        }
    }

    // Update dependencies across workspace crates
    for c in &ctx.crates {
        let path = &c.manifest_path;
        let mut did = false;
        let mut doc = read_toml(path)?;
        did |= update_deps_in_doc(&mut doc, &changed_versions);
        if did {
            tracing::debug!(manifest=%path.display().to_string(), "update dependent versions");
            fs::write(path, doc.to_string())?;
        }
    }

    // Commit all changes
    commit_all(
        &ctx.repo_root,
        &ctx.main_crate,
        &plan.per_crate[&ctx.main_crate].new_version,
    )
}

fn read_toml(path: &Path) -> Result<DocumentMut> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let doc = content
        .parse::<DocumentMut>()
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(doc)
}

fn update_package_version(manifest_path: &Path, new_version: &semver::Version) -> Result<()> {
    let mut doc = read_toml(manifest_path)?;
    if let Some(pkg) = doc.get_mut("package").and_then(|it| it.as_table_mut()) {
        pkg.insert("version", value(new_version.to_string()));
        fs::write(manifest_path, doc.to_string())?;
        Ok(())
    } else {
        // package-less manifest (e.g., virtual workspace) — ignore
        Ok(())
    }
}

fn update_deps_in_doc(doc: &mut DocumentMut, changed: &HashMap<&str, semver::Version>) -> bool {
    let mut modified = false;
    for sect in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(tbl) = doc.get_mut(sect).and_then(|v| v.as_table_like_mut()) {
            // Collect keys to avoid borrow issues
            let keys: Vec<String> = tbl.iter().map(|(k, _)| k.to_string()).collect();
            for k in keys {
                if let Some(newv) = changed.get(k.as_str())
                    && let Some(item) = tbl.get_mut(&k)
                {
                    if item.is_str() {
                        *item = value(newv.to_string());
                        modified = true;
                    } else if let Some(dep_tbl) = item.as_inline_table_mut() {
                        if dep_tbl.contains_key("version") {
                            dep_tbl.insert("version", toml_edit::Value::from(newv.to_string()));
                            modified = true;
                        }
                    } else if let Some(dep_tbl) = item.as_table_mut()
                        && dep_tbl.contains_key("version")
                    {
                        dep_tbl["version"] = value(newv.to_string());
                        modified = true;
                    }
                }
            }
        }
    }
    modified
}

fn update_changelog(
    crate_root: &Path,
    crate_name: &str,
    new_version: &semver::Version,
    changes: &[ChangeEntry],
) -> Result<()> {
    let path = crate_root.join("CHANGELOG.md");
    let old = fs::read_to_string(&path).unwrap_or_default();
    let date = Utc::now().date_naive();
    let mut out = String::new();
    out.push_str(&format!(
        "## {} v{} - {}\n\n",
        crate_name, new_version, date
    ));

    write_group(
        &mut out,
        "Breaking Changes",
        changes
            .iter()
            .filter(|c| matches!(c.kind, CommitKind::Breaking)),
    );
    write_group(
        &mut out,
        "Features",
        changes
            .iter()
            .filter(|c| matches!(c.kind, CommitKind::Feat)),
    );
    write_group(
        &mut out,
        "Fixes",
        changes.iter().filter(|c| matches!(c.kind, CommitKind::Fix)),
    );
    write_group(
        &mut out,
        "Refactor/Perf",
        changes
            .iter()
            .filter(|c| matches!(c.kind, CommitKind::Refactor | CommitKind::Perf)),
    );
    write_group(
        &mut out,
        "Others",
        changes.iter().filter(|c| {
            matches!(
                c.kind,
                CommitKind::Docs | CommitKind::Build | CommitKind::Chore | CommitKind::Other
            )
        }),
    );

    out.push('\n');
    out.push_str(&old);
    fs::write(&path, out)?;
    Ok(())
}

fn write_group<'a, I: Iterator<Item = &'a ChangeEntry>>(out: &mut String, title: &str, iter: I) {
    let list: Vec<&ChangeEntry> = iter.collect();
    if list.is_empty() {
        return;
    }
    out.push_str(&format!("### {}\n", title));
    for c in list {
        out.push_str(&format!("- {} ({})\n", c.subject, c.sha));
    }
    out.push('\n');
}

fn commit_all(repo_root: &Path, _main_crate: &str, new_version: &semver::Version) -> Result<()> {
    let repo = Repository::discover(repo_root)?;
    // Stage everything under workspace (simpler for Phase 2)
    let mut idx = repo.index()?;
    idx.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
    idx.write()?;
    let tree_oid = idx.write_tree()?;
    let tree = repo.find_tree(tree_oid)?;
    let sig = repo
        .signature()
        .or_else(|_| git2::Signature::now("asfship", "asfship@users.noreply.github.com"))
        .context("failed to build git signature")?;
    let head = repo.head().ok();
    let parents = if let Some(h) = head {
        vec![repo.find_commit(h.target().unwrap())?]
    } else {
        vec![]
    };
    let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        &format!("chore(release): prepare v{}", new_version),
        &tree,
        &parent_refs,
    )?;
    tracing::info!("versioning: committed release prep version={}", new_version);
    Ok(())
}

fn next_rc_tag(repo: &Repository, base: &semver::Version) -> Result<(String, u32)> {
    // Scan existing rc tags vX.Y.Z-rc.N and pick next N.
    let pat = format!(
        r"^v{}\.{}\.{}-rc\.(\d+)$",
        base.major, base.minor, base.patch
    );
    let re = Regex::new(&pat).unwrap();
    let mut max_n = 0u32;
    for r in repo.references_glob("refs/tags/*")?.flatten() {
        if let Some(name) = r.shorthand()
            && let Some(m) = re.captures(name).and_then(|c| c.get(1))
            && let Ok(n) = m.as_str().parse::<u32>()
        {
            max_n = max_n.max(n);
        }
    }
    let next = max_n + 1;
    let tag = format!("v{}.{}.{}-rc.{}", base.major, base.minor, base.patch, next);
    Ok((tag, next))
}

fn ensure_tag_absent(repo: &Repository, tag: &str) -> Result<()> {
    if repo.refname_to_id(&format!("refs/tags/{}", tag)).is_ok() {
        bail!("rc tag already exists: {} (idempotency guard)", tag);
    }
    Ok(())
}

async fn create_rc_tag(repo: &Repository, tag: &str) -> Result<()> {
    let repo_path = repo
        .path()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let tag_name = tag.to_string();
    tokio::task::spawn_blocking(move || {
        let repo = Repository::discover(repo_path)?;
        let obj = repo.head()?.peel(git2::ObjectType::Commit)?;
        let commit = obj
            .into_commit()
            .map_err(|_| anyhow::anyhow!("HEAD is not a commit"))?;
        let sig = repo
            .signature()
            .or_else(|_| git2::Signature::now("asfship", "asfship@users.noreply.github.com"))?;
        let msg = format!("asfship prerelease {}", tag_name);
        repo.tag(&tag_name, commit.as_object(), &sig, &msg, true)?; // annotated
        Ok::<_, anyhow::Error>(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("create_rc_tag task join error: {}", e))??;
    tracing::info!("rc: created tag {} (annotated)", tag);
    Ok(())
}

async fn push_head_and_tag(repo_root: &Path, tag: &str) -> Result<()> {
    // Push current branch and the tag to origin.
    let root = repo_root.to_path_buf();
    let branch = tokio::task::spawn_blocking(move || -> Result<String> {
        let repo = Repository::discover(root)?;
        let head = repo.head()?;
        let name = head
            .shorthand()
            .ok_or_else(|| anyhow::anyhow!("HEAD has no shorthand name"))?;
        Ok(name.to_string())
    })
    .await
    .map_err(|e| anyhow::anyhow!("branch detect task join error: {}", e))??;

    tracing::info!("git: pushing branch={} and tag={} to origin", branch, tag);
    // Push branch
    let status = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("push")
        .arg("origin")
        .arg(&branch)
        .status()
        .await?;
    if !status.success() {
        bail!("git push branch failed with status: {}", status);
    }
    // Push tag
    let status = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("push")
        .arg("origin")
        .arg(format!("refs/tags/{}", tag))
        .status()
        .await?;
    if !status.success() {
        bail!("git push tag failed with status: {}", status);
    }
    Ok(())
}

async fn create_github_prerelease(owner: &str, repo: &str, tag: &str) -> Result<()> {
    tracing::info!("github: creating prerelease for tag={}", tag);
    let gh = github_client()?;
    let repos = gh.repos(owner.to_string(), repo.to_string());
    let rh = repos.releases();
    match rh.get_by_tag(tag).await {
        Ok(_) => {
            tracing::info!("github: release already exists for {}", tag);
            return Ok(());
        }
        Err(err) => {
            if !is_not_found(&err) {
                return Err(err.into());
            }
        }
    }
    let _ = rh
        .create(tag)
        .name(tag)
        .prerelease(true)
        .draft(false)
        .body("")
        .send()
        .await?;
    Ok(())
}

async fn package_changed_crates(
    ctx: &InferredContext,
    plan: &Plan,
    rc_tag: &str,
    rc_n: u32,
) -> Result<Vec<PathBuf>> {
    let out_dir = ctx
        .repo_root
        .join("target")
        .join("asfship")
        .join(rc_tag.replace('/', "_"));
    tokio::fs::create_dir_all(&out_dir).await?;

    // Resolve tree for tag
    let repo = Repository::discover(&ctx.repo_root)?;
    let obj = repo
        .revparse_single(&format!("refs/tags/{}", rc_tag))
        .or_else(|_| repo.revparse_single(rc_tag))
        .context("failed to resolve rc tag for packaging")?;
    let commit = obj
        .peel_to_commit()
        .context("rc tag does not point to a commit")?;
    let tree = commit.tree()?;

    let mut files: Vec<PathBuf> = Vec::new();
    for c in &ctx.crates {
        if let Some(cp) = plan.per_crate.get(&c.name) {
            let base = if c.name == ctx.main_crate {
                format!("apache-{}-{}-rc{}-src", ctx.repo_name, cp.new_version, rc_n)
            } else {
                format!(
                    "apache-{}-{}-{}-rc{}-src",
                    ctx.repo_name, c.name, cp.new_version, rc_n
                )
            };

            let crate_rel = c
                .package_root
                .strip_prefix(&ctx.repo_root)
                .unwrap_or(&c.package_root)
                .to_path_buf();

            let tar_gz = out_dir.join(format!("{}.tar.gz", base));
            let zip = out_dir.join(format!("{}.zip", base));

            package_from_tree(&repo, &tree, &crate_rel, &tar_gz, &zip)?;
            files.push(tar_gz.clone());
            files.push(zip.clone());

            // sha512 for each artifact
            for f in [tar_gz, zip] {
                let sha = compute_sha512(&f).await?;
                let sha_path = f.with_file_name(format!(
                    "{}.sha512",
                    f.file_name().and_then(|n| n.to_str()).unwrap_or("artifact")
                ));
                tokio::fs::write(&sha_path, format!("{}\n", sha)).await?;
                files.push(sha_path);
            }
        }
    }
    Ok(files)
}

async fn compute_sha512(path: &Path) -> Result<String> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha512::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    Ok(hex::encode(digest))
}

async fn upload_assets(owner: &str, repo: &str, tag: &str, files: &[PathBuf]) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }
    tracing::info!("github: uploading {} assets", files.len());
    let gh = github_client()?;
    let repos = gh.repos(owner.to_string(), repo.to_string());
    let rh = repos.releases();
    let release = rh.get_by_tag(tag).await?;
    let token = github_token()?;
    let client = reqwest::Client::new();
    let base_upload_url = release
        .upload_url
        .split('{')
        .next()
        .unwrap_or(&release.upload_url)
        .to_string();
    for f in files {
        let name = f
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("asset")
            .to_string();
        let ct = match f.extension().and_then(|e| e.to_str()) {
            Some("gz") => "application/gzip",
            Some("zip") => "application/zip",
            Some("sha512") => "text/plain",
            _ => "application/octet-stream",
        };
        let url = format!("{}?name={}", base_upload_url, url_encode(&name));
        let bytes = tokio::fs::read(f).await?;
        let resp = client
            .post(&url)
            .bearer_auth(&token)
            .header(header::CONTENT_TYPE, ct)
            .body(bytes)
            .send()
            .await?;
        if !resp.status().is_success() {
            bail!("upload asset failed for {}: {}", name, resp.status());
        }
        tracing::debug!("uploaded asset {}", name);
    }
    Ok(())
}

fn github_client() -> Result<Octocrab> {
    let token = github_token()?;
    let gh = Octocrab::builder().personal_token(token).build()?;
    Ok(gh)
}

fn is_not_found(err: &octocrab::Error) -> bool {
    if let octocrab::Error::GitHub { source, .. } = err {
        return source.status_code == StatusCode::NOT_FOUND;
    }
    false
}

fn github_token() -> Result<String> {
    std::env::var("GITHUB_TOKEN")
        .or_else(|_| std::env::var("GH_TOKEN"))
        .map_err(|_| anyhow::anyhow!("missing GITHUB_TOKEN or GH_TOKEN for GitHub API"))
}

fn has_github_auth() -> bool {
    std::env::var("GITHUB_TOKEN").is_ok() || std::env::var("GH_TOKEN").is_ok()
}

fn package_from_tree(
    repo: &Repository,
    tree: &git2::Tree,
    crate_rel: &Path,
    tar_gz: &Path,
    zip_path: &Path,
) -> Result<()> {
    // Prepare writers
    let tar_file = fs::File::create(tar_gz)?;
    let enc = GzEncoder::new(tar_file, Compression::default());
    let mut tar = TarBuilder::new(enc);

    let zip_file = fs::File::create(zip_path)?;
    let mut zip = zip::ZipWriter::new(zip_file);
    let zopt = ZipOptions::default()
        .compression_method(ZipCompression::Deflated)
        .unix_permissions(0o644);

    let crate_prefix = crate_rel.to_string_lossy().replace('\\', "/");
    let norm_prefix = if crate_prefix.ends_with('/') {
        crate_prefix
    } else {
        format!("{}/", crate_prefix)
    };

    tree.walk(git2::TreeWalkMode::PreOrder, |root, entry| {
        let name = match entry.name() {
            Some(n) => n,
            None => return 0,
        };
        let full = format!("{}{}", root, name);
        if !full.starts_with(&norm_prefix) {
            return 0;
        }
        if let Some(git2::ObjectType::Blob) = entry.kind()
            && let Ok(obj) = entry.to_object(repo)
            && let Ok(blob) = obj.into_blob()
        {
            // Paths use the repository-relative path
            let path_in = Path::new(&full);
            // tar
            let mut header = tar::Header::new_gnu();
            if header.set_path(path_in).is_err() {
                return 0;
            }
            header.set_size(blob.content().len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            let mut cursor = Cursor::new(blob.content());
            if tar.append(&header, &mut cursor).is_err() {
                return 0;
            }
            // zip
            if zip.start_file(full.clone(), zopt).is_err() {
                return 0;
            }
            if zip.write_all(blob.content()).is_err() {
                return 0;
            }
        }
        0
    })?;

    tar.into_inner()?.finish()?;
    zip.finish()?;
    Ok(())
}
