mod apply;
mod plan;
mod rc;

use std::collections::BTreeMap;
use std::fmt::Write as _;

use anyhow::{Result, bail};
use git2::Repository;

use crate::github;
use crate::infer::InferredContext;

pub async fn run_prerelease(ctx: &InferredContext, dry_run: bool) -> Result<PrereleaseReport> {
    let repo = Repository::discover(&ctx.repo_root)?;
    let plan = plan::compute_plan(&repo, ctx)?;
    tracing::info!(
        "versioning: plan computed changed_crates={}",
        plan.changed_count()
    );

    if plan.crate_plan(&ctx.main_crate).is_none() {
        bail!("main crate has no changes since base tag; aborting prerelease prep");
    }

    let mut report = build_report(ctx, &plan, dry_run);

    if dry_run {
        tracing::debug!("versioning: dry-run, skip applying changes");
        return Ok(report);
    }

    tracing::info!("versioning: applying changes");
    apply::apply_changes(ctx, &plan)?;

    report.mark_applied();

    if github::has_token() {
        let rc_tag = rc::execute_rc(&repo, ctx, &plan).await?;
        report.set_rc_tag(Some(rc_tag));
    } else {
        tracing::warn!(
            "rc: skip tagging and packaging (set ASFSHIP_GITHUB_TOKEN to enable GitHub integration)"
        );
    }

    Ok(report)
}

#[derive(Debug, Clone)]
pub struct PrereleaseReport {
    base_tag: Option<String>,
    main_crate: String,
    dry_run: bool,
    changed_crates: Vec<ReportCrate>,
    rc_tag: Option<String>,
}

impl PrereleaseReport {
    pub fn render_text(&self) -> String {
        let mut out = String::new();
        writeln!(&mut out, "prerelease summary").unwrap();
        writeln!(
            &mut out,
            "mode: {}",
            if self.dry_run { "dry-run" } else { "applied" }
        )
        .unwrap();
        writeln!(
            &mut out,
            "base tag: {}",
            self.base_tag.as_deref().unwrap_or("<none>")
        )
        .unwrap();
        writeln!(&mut out, "main crate: {}", self.main_crate).unwrap();
        let rc_status = if self.dry_run {
            "<pending>"
        } else if let Some(tag) = &self.rc_tag {
            tag.as_str()
        } else {
            "<skipped>"
        };
        writeln!(&mut out, "rc tag: {}", rc_status).unwrap();

        if self.changed_crates.is_empty() {
            writeln!(&mut out, "changed crates: <none>").unwrap();
            return out;
        }

        writeln!(&mut out, "changed crates:").unwrap();
        for crate_plan in &self.changed_crates {
            writeln!(
                &mut out,
                "* {} {} -> {}",
                crate_plan.name, crate_plan.old_version, crate_plan.new_version
            )
            .unwrap();

            let mut grouped: BTreeMap<&'static str, Vec<&ReportChange>> = BTreeMap::new();
            for change in &crate_plan.changes {
                grouped
                    .entry(group_label(change.kind))
                    .or_default()
                    .push(change);
            }
            for label in GROUP_ORDER {
                if let Some(entries) = grouped.get(label) {
                    if entries.is_empty() {
                        continue;
                    }
                    writeln!(&mut out, "  {}:", label).unwrap();
                    for change in entries {
                        writeln!(&mut out, "    - {}", change.subject).unwrap();
                    }
                }
            }
        }

        out
    }

    fn mark_applied(&mut self) {
        self.dry_run = false;
    }

    fn set_rc_tag(&mut self, tag: Option<String>) {
        self.rc_tag = tag;
    }
}

#[derive(Debug, Clone)]
struct ReportCrate {
    name: String,
    old_version: semver::Version,
    new_version: semver::Version,
    changes: Vec<ReportChange>,
}

#[derive(Debug, Clone)]
struct ReportChange {
    kind: plan::CommitKind,
    subject: String,
}

const GROUP_ORDER: [&str; 5] = [
    "Breaking Changes",
    "Features",
    "Fixes",
    "Refactor/Perf",
    "Others",
];

fn build_report(ctx: &InferredContext, plan: &plan::Plan, dry_run: bool) -> PrereleaseReport {
    let mut changed_crates = Vec::new();
    for (name, crate_plan) in plan.iter() {
        let mut changes = Vec::new();
        for change in crate_plan.changes() {
            changes.push(ReportChange {
                kind: change.kind(),
                subject: change.subject().to_string(),
            });
        }
        changed_crates.push(ReportCrate {
            name: name.clone(),
            old_version: crate_plan.previous_version().clone(),
            new_version: crate_plan.new_version().clone(),
            changes,
        });
    }

    PrereleaseReport {
        base_tag: ctx.last_stable_tag.clone(),
        main_crate: ctx.main_crate.clone(),
        dry_run,
        changed_crates,
        rc_tag: None,
    }
}

fn group_label(kind: plan::CommitKind) -> &'static str {
    match kind {
        plan::CommitKind::Breaking => "Breaking Changes",
        plan::CommitKind::Feat => "Features",
        plan::CommitKind::Fix => "Fixes",
        plan::CommitKind::Perf | plan::CommitKind::Refactor => "Refactor/Perf",
        plan::CommitKind::Docs
        | plan::CommitKind::Build
        | plan::CommitKind::Chore
        | plan::CommitKind::Other => "Others",
    }
}

#[cfg(test)]
mod tests {
    use super::GROUP_ORDER;
    use super::group_label;
    use super::plan::CommitKind;

    #[test]
    fn group_order_contains_all_labels() {
        assert!(GROUP_ORDER.contains(&group_label(CommitKind::Breaking)));
        assert!(GROUP_ORDER.contains(&group_label(CommitKind::Feat)));
        assert!(GROUP_ORDER.contains(&group_label(CommitKind::Fix)));
        assert!(GROUP_ORDER.contains(&group_label(CommitKind::Refactor)));
        assert!(GROUP_ORDER.contains(&group_label(CommitKind::Docs)));
    }
}
