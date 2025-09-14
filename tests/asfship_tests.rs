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

fn init_repo(root: &Path, origin: &str) -> Result<Repository> {
    let repo = Repository::init(root)?;
    let mut idx = repo.index()?;
    idx.add_all(["*"].iter(), IndexAddOption::DEFAULT, None)?;
    idx.write()?;
    let oid = idx.write_tree()?;
    let sig = Signature::now("asfship", "asfship@example.com")?;
    let tree = repo.find_tree(oid)?;
    // Create initial commit on main
    let _commit_oid = repo.commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[])?;
    drop(tree);
    repo.remote("origin", origin)?;
    Ok(repo)
}

fn commit_all(repo: &Repository, message: &str) -> Result<()> {
    let mut idx = repo.index()?;
    idx.add_all(["*"].iter(), IndexAddOption::DEFAULT, None)?;
    idx.write()?;
    let oid = idx.write_tree()?;
    let tree = repo.find_tree(oid)?;
    let sig = Signature::now("asfship", "asfship@example.com")?;
    let head = repo.head()?;
    let parent = repo.find_commit(head.target().unwrap())?;
    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])?;
    Ok(())
}

fn read_version(manifest: &Path) -> String {
    let s = fs::read_to_string(manifest).unwrap();
    let doc: toml::Value = toml::from_str(&s).unwrap();
    doc.get("package")
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string()
}

// Snapshot-like smoke tests

#[test]
fn start_snapshot() -> Result<()> {
    let td = TempDir::new()?;
    let root = td.path();
    write_file(
        &root.join("Cargo.toml"),
        r#"[package]
name = "foo"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    write_file(&root.join("src/lib.rs"), "pub fn _noop() {}\n")?;
    let _repo = init_repo(root, "https://github.com/apache/foo.git")?;

    let output = std::process::Command::cargo_bin("asfship")?
        .current_dir(root)
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
    let td = TempDir::new()?;
    let root = td.path();
    write_file(
        &root.join("Cargo.toml"),
        r#"[package]
name = "foo"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    write_file(&root.join("src/lib.rs"), "pub fn _noop() {}\n")?;
    let _repo = init_repo(root, "https://github.com/apache/foo.git")?;

    let output = std::process::Command::cargo_bin("asfship")?
        .current_dir(root)
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

// Versioning tests

#[test]
fn pre1_feat_bumps_patch() -> Result<()> {
    let td = TempDir::new()?;
    let root = td.path();

    write_file(
        &root.join("Cargo.toml"),
        r#"[package]
name = "foo"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    write_file(&root.join("src/lib.rs"), "pub fn f() {}\n")?;
    let repo = init_repo(root, "https://github.com/apache/foo.git")?;

    // A feature commit under pre-1.0 should bump patch
    write_file(&root.join("src/new.rs"), "pub fn g() {}\n")?;
    commit_all(&repo, "feat: add new module")?;

    let status = std::process::Command::cargo_bin("asfship")?
        .current_dir(root)
        .args(["prerelease"])
        .status()?;
    assert!(status.success());
    let v = read_version(&root.join("Cargo.toml"));
    assert_eq!(v, "0.1.1");
    let changelog = fs::read_to_string(root.join("CHANGELOG.md")).unwrap();
    assert!(changelog.contains("## foo v0.1.1"));
    Ok(())
}

#[test]
fn pre1_breaking_bumps_minor() -> Result<()> {
    let td = TempDir::new()?;
    let root = td.path();

    write_file(
        &root.join("Cargo.toml"),
        r#"[package]
name = "foo"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    write_file(&root.join("src/lib.rs"), "pub fn f() {}\n")?;
    let repo = init_repo(root, "https://github.com/apache/foo.git")?;

    write_file(&root.join("src/lib.rs"), "pub fn f() {} pub fn h() {}\n")?;
    commit_all(&repo, "refactor!: breaking change")?;

    let status = std::process::Command::cargo_bin("asfship")?
        .current_dir(root)
        .args(["prerelease"])
        .status()?;
    assert!(status.success());
    let v = read_version(&root.join("Cargo.toml"));
    assert_eq!(v, "0.2.0");
    Ok(())
}

#[test]
fn post1_feat_bumps_minor() -> Result<()> {
    let td = TempDir::new()?;
    let root = td.path();

    write_file(
        &root.join("Cargo.toml"),
        r#"[package]
name = "foo"
version = "1.2.3"
edition = "2021"
"#,
    )?;
    write_file(&root.join("src/lib.rs"), "pub fn f() {}\n")?;
    let repo = init_repo(root, "https://github.com/apache/foo.git")?;

    write_file(&root.join("src/new.rs"), "pub fn g() {}\n")?;
    commit_all(&repo, "feat: exciting feature")?;

    let status = std::process::Command::cargo_bin("asfship")?
        .current_dir(root)
        .args(["prerelease"])
        .status()?;
    assert!(status.success());
    let v = read_version(&root.join("Cargo.toml"));
    assert_eq!(v, "1.3.0");
    Ok(())
}
