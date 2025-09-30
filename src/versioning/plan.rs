use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use git2::{Repository, Sort};

use crate::infer::{CrateInfo, InferredContext};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum BumpKind {
    Major,
    Minor,
    Patch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommitKind {
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
pub(crate) struct ChangeEntry {
    kind: CommitKind,
    subject: String,
    sha: String,
    breaking: bool,
}

impl ChangeEntry {
    pub(crate) fn kind(&self) -> CommitKind {
        self.kind
    }

    pub(crate) fn subject(&self) -> &str {
        &self.subject
    }

    pub(crate) fn sha(&self) -> &str {
        &self.sha
    }

    pub(crate) fn is_breaking(&self) -> bool {
        self.breaking
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CratePlan {
    new_version: semver::Version,
    changes: Vec<ChangeEntry>,
}

impl CratePlan {
    pub(crate) fn new_version(&self) -> &semver::Version {
        &self.new_version
    }

    pub(crate) fn changes(&self) -> &[ChangeEntry] {
        &self.changes
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Plan {
    per_crate: BTreeMap<String, CratePlan>,
}

impl Plan {
    pub(crate) fn changed_count(&self) -> usize {
        self.per_crate.len()
    }

    pub(crate) fn crate_plan(&self, name: &str) -> Option<&CratePlan> {
        self.per_crate.get(name)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&String, &CratePlan)> {
        self.per_crate.iter()
    }

    pub(crate) fn main_crate_version(&self, main: &str) -> Option<&semver::Version> {
        self.crate_plan(main).map(|cp| cp.new_version())
    }
}

pub(crate) fn compute_plan(repo: &Repository, ctx: &InferredContext) -> Result<Plan> {
    let base_oid = if let Some(tag) = &ctx.last_stable_tag {
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

    let mut roots: Vec<(PathBuf, &CrateInfo)> = ctx
        .crates
        .iter()
        .map(|c| (c.package_root.clone(), c))
        .collect();
    roots.sort_by(|a, b| b.0.components().count().cmp(&a.0.components().count()));

    let mut per_crate_changes: HashMap<String, Vec<ChangeEntry>> = HashMap::new();

    let mut walk = repo.revwalk()?;
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::REVERSE)?;
    walk.push_head()?;
    if let Some(base) = base_oid {
        walk.hide(base)?;
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
    let breaking = changes.iter().any(|c| c.is_breaking());
    if current.major >= 1 {
        if breaking {
            return BumpKind::Major;
        }
        if changes.iter().any(|c| matches!(c.kind(), CommitKind::Feat)) {
            return BumpKind::Minor;
        }
        return BumpKind::Patch;
    }
    if breaking {
        BumpKind::Minor
    } else {
        BumpKind::Patch
    }
}
