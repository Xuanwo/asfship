use std::fs;
use std::path::Path;

use anyhow::Result;
use assert_cmd::prelude::*;
use git2::{IndexAddOption, Repository, Signature};
use tempfile::TempDir;

fn write_file(path: &Path, content: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, content)?;
    Ok(())
}

struct Fixture {
    _td: TempDir,
}

impl Fixture {
    fn new() -> Result<(Self, std::path::PathBuf)> {
        let td = tempfile::tempdir()?;
        let root = td.path().to_path_buf();

        // Minimal crate
        write_file(
            &root.join("Cargo.toml"),
            r#"[package]
name = "foo"
version = "0.1.0"
edition = "2021"
"#,
        )?;
        write_file(&root.join("src/lib.rs"), "pub fn _noop() {}\n")?;

        // Init git repo and commit
        let repo = Repository::init(&root)?;
        let mut idx = repo.index()?;
        idx.add_all(["*"].iter(), IndexAddOption::DEFAULT, None)?;
        idx.write()?;
        let oid = idx.write_tree()?;
        let tree = repo.find_tree(oid)?;
        let sig = Signature::now("asfship", "asfship@example.com")?;
        let commit_oid = repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])?;
        let commit = repo.find_commit(commit_oid)?;
        let head_name = repo
            .head()
            .ok()
            .and_then(|h| h.name().map(|s| s.to_string()));
        if head_name.as_deref() != Some("refs/heads/main") {
            // Create branch `main` pointing to the initial commit and switch to it
            repo.branch("main", &commit, true)?;
            repo.set_head("refs/heads/main")?;
        }
        repo.remote("origin", "https://github.com/apache/foo.git")?;

        Ok((Self { _td: td }, root))
    }
}

#[test]
fn start_snapshot() -> Result<()> {
    let (_fx, root) = Fixture::new()?;
    let output = std::process::Command::cargo_bin("asfship")?
        .current_dir(&root)
        .args(["start", "--dry-run"])
        .output()?;
    assert!(
        output.status.success(),
        "status: {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)?;
    insta::assert_snapshot!(stdout, @r###"start: ready (repo=apache/foo main_crate=foo)
"###);
    Ok(())
}

#[test]
fn prerelease_snapshot() -> Result<()> {
    let (_fx, root) = Fixture::new()?;
    let output = std::process::Command::cargo_bin("asfship")?
        .current_dir(&root)
        .args(["prerelease", "--dry-run"])
        .output()?;
    assert!(
        output.status.success(),
        "status: {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)?;
    insta::assert_snapshot!(stdout, @r###"prerelease: ready (base_tag=<none> changed_crates=1)
"###);
    Ok(())
}
