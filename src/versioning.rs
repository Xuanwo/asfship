use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use git2::{Repository, Sort};
use toml_edit::{DocumentMut, value};

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
