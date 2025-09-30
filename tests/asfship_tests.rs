use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use assert_cmd::Command;
use git2::{IndexAddOption, Repository, Signature, build::CheckoutBuilder};
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
    // Create initial commit on the repository's default HEAD
    let _commit_oid = repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])?;
    repo.checkout_head(Some(CheckoutBuilder::new().force()))?;
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

fn asfship_cmd(root: &Path) -> Result<Command> {
    let mut cmd = Command::cargo_bin("asfship")?;
    cmd.current_dir(root);
    cmd.env_remove("ASFSHIP_GITHUB_TOKEN");
    Ok(cmd)
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

    let mut cmd = asfship_cmd(root)?;
    cmd.args(["start", "--dry-run"]);
    let output = cmd.output()?;
    assert!(
        output.status.success(),
        "status: {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)?;
    insta::assert_snapshot!(stdout, @r###"start: dry-run (category=Releases title=foo Release Kickoff)
---
# foo Release Kickoff

- Base tag: <none>
- Main crate: foo
- Proposed release date: TBD

Workspace crates:
- foo 0.1.0


Please add agenda items, blockers, and verification tasks below. Once scope is agreed, run `asfship prerelease` to prepare the first release candidate.
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

    let mut cmd = asfship_cmd(root)?;
    cmd.args(["prerelease", "--dry-run"]);
    let output = cmd.output()?;
    assert!(
        output.status.success(),
        "status: {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)?;
    insta::assert_snapshot!(stdout, @r###"prerelease summary
mode: dry-run
base tag: <none>
main crate: foo
rc tag: <pending>
artifacts dir: <pending>
changed crates:
* foo 0.1.0 -> 0.1.1
  Others:
    - init
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

    let mut cmd = asfship_cmd(root)?;
    cmd.args(["prerelease"]);
    cmd.assert().success();
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

    let mut cmd = asfship_cmd(root)?;
    cmd.args(["prerelease"]);
    cmd.assert().success();
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

    let mut cmd = asfship_cmd(root)?;
    cmd.args(["prerelease"]);
    cmd.assert().success();
    let v = read_version(&root.join("Cargo.toml"));
    assert_eq!(v, "1.3.0");
    Ok(())
}

#[test]
fn prerelease_local_assets_creates_artifacts() -> Result<()> {
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

    write_file(&root.join("src/new.rs"), "pub fn g() {}\n")?;
    commit_all(&repo, "feat: add local packaging")?;

    let mut cmd = asfship_cmd(root)?;
    cmd.args(["prerelease", "--local-assets"]);
    let output = cmd.output()?;
    assert!(
        output.status.success(),
        "status: {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let artifact_root = root.join("target").join("asfship");
    assert!(
        artifact_root.exists(),
        "artifact root missing: {:?}",
        artifact_root
    );

    fn collect(dir: &Path, acc: &mut Vec<PathBuf>) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                collect(&path, acc)?;
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("gz"))
                .unwrap_or(false)
            {
                acc.push(path);
            }
        }
        Ok(())
    }

    let mut archives = Vec::new();
    collect(&artifact_root, &mut archives)?;
    assert!(
        !archives.is_empty(),
        "expected archives under {:?}",
        artifact_root
    );

    Ok(())
}
