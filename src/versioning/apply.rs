use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use git2::Repository;
use toml_edit::{DocumentMut, value};

use crate::infer::InferredContext;

use super::plan::{ChangeEntry, CommitKind, Plan};

pub(crate) fn apply_changes(ctx: &InferredContext, plan: &Plan) -> Result<()> {
    let mut changed_versions: HashMap<&str, semver::Version> = HashMap::new();
    for (name, crate_plan) in plan.iter() {
        changed_versions.insert(name.as_str(), crate_plan.new_version().clone());
    }

    for c in &ctx.crates {
        if let Some(crate_plan) = plan.crate_plan(&c.name) {
            tracing::debug!(
                "update version + changelog crate={} old={} new={}",
                c.name,
                c.version,
                crate_plan.new_version()
            );
            update_package_version(&c.manifest_path, crate_plan.new_version())?;
            update_changelog(
                &c.package_root,
                &c.name,
                crate_plan.new_version(),
                crate_plan.changes(),
            )?;
        }
    }

    for c in &ctx.crates {
        let path = &c.manifest_path;
        let mut doc = read_toml(path)?;
        let mut modified = false;
        modified |= update_deps_in_doc(&mut doc, &changed_versions);
        if modified {
            tracing::debug!(manifest=%path.display().to_string(), "update dependent versions");
            fs::write(path, doc.to_string())?;
        }
    }

    let new_main = plan
        .main_crate_version(&ctx.main_crate)
        .expect("main crate must be present once we reach apply_changes");
    commit_all(&ctx.repo_root, new_main)
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
    }
    Ok(())
}

fn update_deps_in_doc(doc: &mut DocumentMut, changed: &HashMap<&str, semver::Version>) -> bool {
    let mut modified = false;
    for sect in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(tbl) = doc.get_mut(sect).and_then(|v| v.as_table_like_mut()) {
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
            .filter(|c| matches!(c.kind(), CommitKind::Breaking)),
    );
    write_group(
        &mut out,
        "Features",
        changes
            .iter()
            .filter(|c| matches!(c.kind(), CommitKind::Feat)),
    );
    write_group(
        &mut out,
        "Fixes",
        changes
            .iter()
            .filter(|c| matches!(c.kind(), CommitKind::Fix)),
    );
    write_group(
        &mut out,
        "Refactor/Perf",
        changes
            .iter()
            .filter(|c| matches!(c.kind(), CommitKind::Refactor | CommitKind::Perf)),
    );
    write_group(
        &mut out,
        "Others",
        changes.iter().filter(|c| {
            matches!(
                c.kind(),
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
        out.push_str(&format!("- {} ({})\n", c.subject(), c.sha()));
    }
    out.push('\n');
}

fn commit_all(repo_root: &Path, new_version: &semver::Version) -> Result<()> {
    let repo = Repository::discover(repo_root)?;
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
