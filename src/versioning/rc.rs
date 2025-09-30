use std::collections::BTreeSet;
use std::fs;
use std::io::Cursor;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, bail};
use flate2::Compression;
use flate2::write::GzEncoder;
use git2::{Commit, Repository};
use reqwest::StatusCode;
use reqwest::header;
use sha2::{Digest, Sha512};
use tar::Builder as TarBuilder;
use tokio::fs as async_fs;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::sleep;
use urlencoding::encode as url_encode;
use zip::CompressionMethod as ZipCompression;
use zip::write::FileOptions as ZipOptions;

use crate::github;
use crate::infer::InferredContext;

use super::plan::Plan;

const UPLOAD_RETRIES: usize = 3;

pub(crate) enum RcMode {
    Remote,
    LocalOnly,
}

pub(crate) struct RcOutcome {
    pub rc_tag: String,
    pub artifact_dir: PathBuf,
}

pub(crate) struct PackagedCrate {
    pub name: String,
    pub files: Vec<PathBuf>,
}

pub(crate) async fn execute_rc(
    repo: &Repository,
    ctx: &InferredContext,
    plan: &Plan,
    artifact_dir: Option<&Path>,
    mode: RcMode,
) -> Result<RcOutcome> {
    let base_version = plan
        .main_crate_version(&ctx.main_crate)
        .expect("main crate plan must exist before RC steps");
    let (rc_tag, rc_n) = next_rc_tag(repo, base_version)?;
    tracing::info!("rc: choosing tag={} (rc={})", rc_tag, rc_n);

    ensure_tag_absent(repo, &rc_tag)?;

    let commit = repo.head()?.peel_to_commit()?;

    create_rc_tag(repo, &rc_tag).await?;

    if matches!(mode, RcMode::Remote) {
        push_head_and_tag(&ctx.repo_root, &rc_tag).await?;
        create_github_prerelease(&ctx.repo_owner, &ctx.repo_name, &rc_tag).await?;
    }

    let artifact_root = resolve_artifact_root(ctx, artifact_dir);
    let run_dir = artifact_root.join(rc_tag.replace('/', "_"));
    async_fs::create_dir_all(&run_dir).await?;

    let packaged = package_changed_crates(repo, ctx, plan, &commit, &run_dir, rc_n).await?;
    validate_packaged(plan, &packaged)?;

    if matches!(mode, RcMode::Remote) {
        let mut all_files: Vec<PathBuf> = packaged
            .iter()
            .flat_map(|p| p.files.iter().cloned())
            .collect();
        all_files.sort();
        upload_assets_with_retry(&ctx.repo_owner, &ctx.repo_name, &rc_tag, &all_files).await?;
    }

    Ok(RcOutcome {
        rc_tag,
        artifact_dir: run_dir,
    })
}

fn resolve_artifact_root(ctx: &InferredContext, artifact_dir: Option<&Path>) -> PathBuf {
    match artifact_dir {
        Some(p) if p.is_absolute() => p.to_path_buf(),
        Some(p) => ctx.repo_root.join(p),
        None => ctx.repo_root.join("target").join("asfship"),
    }
}

fn next_rc_tag(repo: &Repository, base: &semver::Version) -> Result<(String, u32)> {
    let pat = format!(
        r"^v{}\.{}\.{}-rc\.(\d+)$",
        base.major, base.minor, base.patch
    );
    let re = regex::Regex::new(&pat).unwrap();
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
        repo.tag(&tag_name, commit.as_object(), &sig, &msg, true)?;
        Ok::<_, anyhow::Error>(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("create_rc_tag task join error: {}", e))??;
    tracing::info!("rc: created tag {} (annotated)", tag);
    Ok(())
}

async fn push_head_and_tag(repo_root: &Path, tag: &str) -> Result<()> {
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
    let gh = github::client()?;
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
    repo: &Repository,
    ctx: &InferredContext,
    plan: &Plan,
    commit: &Commit<'_>,
    out_dir: &Path,
    rc_n: u32,
) -> Result<Vec<PackagedCrate>> {
    let tree = commit.tree()?;
    let mut packaged = Vec::new();
    for c in &ctx.crates {
        if let Some(crate_plan) = plan.crate_plan(&c.name) {
            let base = if c.name == ctx.main_crate {
                format!(
                    "apache-{}-{}-rc{}-src",
                    ctx.repo_name,
                    crate_plan.new_version(),
                    rc_n
                )
            } else {
                format!(
                    "apache-{}-{}-{}-rc{}-src",
                    ctx.repo_name,
                    c.name,
                    crate_plan.new_version(),
                    rc_n
                )
            };

            let crate_rel = c
                .package_root
                .strip_prefix(&ctx.repo_root)
                .unwrap_or(&c.package_root)
                .to_path_buf();

            let tar_gz = out_dir.join(format!("{}.tar.gz", base));
            let zip = out_dir.join(format!("{}.zip", base));

            package_from_tree(repo, &tree, &crate_rel, &tar_gz, &zip)?;
            let mut files = vec![tar_gz.clone(), zip.clone()];

            for f in [tar_gz, zip] {
                let sha = compute_sha512(&f).await?;
                let sha_path = f.with_file_name(format!(
                    "{}.sha512",
                    f.file_name().and_then(|n| n.to_str()).unwrap_or("artifact")
                ));
                async_fs::write(&sha_path, format!("{}\n", sha)).await?;
                files.push(sha_path);
            }

            packaged.push(PackagedCrate {
                name: c.name.clone(),
                files,
            });
        }
    }
    Ok(packaged)
}

fn validate_packaged(plan: &Plan, packaged: &[PackagedCrate]) -> Result<()> {
    if packaged.len() != plan.changed_count() {
        bail!(
            "packaged crate count {} does not match plan {}",
            packaged.len(),
            plan.changed_count()
        );
    }
    let expected: BTreeSet<_> = plan.iter().map(|(name, _)| name.clone()).collect();
    let actual: BTreeSet<_> = packaged.iter().map(|p| p.name.clone()).collect();
    if expected != actual {
        bail!(
            "packaged crates {:?} do not match plan {:?}",
            actual,
            expected
        );
    }
    for entry in packaged {
        let has_tar = entry
            .files
            .iter()
            .any(|f| f.extension().and_then(|e| e.to_str()) == Some("gz"));
        let has_zip = entry
            .files
            .iter()
            .any(|f| f.extension().and_then(|e| e.to_str()) == Some("zip"));
        if !has_tar || !has_zip {
            bail!(
                "crate {} missing expected archive variants (tar.gz={}, zip={})",
                entry.name,
                has_tar,
                has_zip
            );
        }
    }
    Ok(())
}

async fn compute_sha512(path: &Path) -> Result<String> {
    let mut file = async_fs::File::open(path).await?;
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

pub(crate) async fn upload_assets_with_retry(
    owner: &str,
    repo: &str,
    tag: &str,
    files: &[PathBuf],
) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }
    tracing::info!("github: uploading {} assets", files.len());
    let gh = github::client()?;
    let repos = gh.repos(owner.to_string(), repo.to_string());
    let rh = repos.releases();
    let release = rh.get_by_tag(tag).await?;
    let token = github::token()?;
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
        let bytes = async_fs::read(f).await?;
        let mut attempt = 0;
        loop {
            attempt += 1;
            let resp = client
                .post(&url)
                .bearer_auth(&token)
                .header(header::CONTENT_TYPE, ct)
                .body(bytes.clone())
                .send()
                .await;
            match resp {
                Ok(resp) if resp.status().is_success() => {
                    tracing::debug!("uploaded asset {}", name);
                    break;
                }
                Ok(resp) => {
                    if attempt >= UPLOAD_RETRIES {
                        bail!("upload asset failed for {}: {}", name, resp.status());
                    }
                    tracing::warn!(
                        "upload {} failed with status {} (attempt {}/{})",
                        name,
                        resp.status(),
                        attempt,
                        UPLOAD_RETRIES
                    );
                }
                Err(err) => {
                    if attempt >= UPLOAD_RETRIES {
                        return Err(err.into());
                    }
                    tracing::warn!(
                        "upload {} errored: {} (attempt {}/{})",
                        name,
                        err,
                        attempt,
                        UPLOAD_RETRIES
                    );
                }
            }
            sleep(Duration::from_millis(200 * attempt as u64)).await;
        }
    }
    Ok(())
}

fn is_not_found(err: &octocrab::Error) -> bool {
    if let octocrab::Error::GitHub { source, .. } = err {
        return source.status_code == StatusCode::NOT_FOUND;
    }
    false
}

fn package_from_tree(
    repo: &Repository,
    tree: &git2::Tree,
    crate_rel: &Path,
    tar_gz: &Path,
    zip_path: &Path,
) -> Result<()> {
    let tar_file = fs::File::create(tar_gz)?;
    let enc = GzEncoder::new(tar_file, Compression::default());
    let mut tar = TarBuilder::new(enc);

    let zip_file = fs::File::create(zip_path)?;
    let mut zip = zip::ZipWriter::new(zip_file);
    let zopt = ZipOptions::default()
        .compression_method(ZipCompression::Deflated)
        .unix_permissions(0o644);

    let crate_rel = normalize_relative(crate_rel);
    let mut error: Option<anyhow::Error> = None;

    tree.walk(git2::TreeWalkMode::PreOrder, |root, entry| {
        let name = match entry.name() {
            Some(n) => n,
            None => return 0,
        };

        let mut full_path = PathBuf::from(root);
        full_path.push(name);

        if !crate_rel.as_os_str().is_empty() && !full_path.starts_with(&crate_rel) {
            return 0;
        }

        if should_skip_artifact_path(&full_path) {
            return 0;
        }

        if let Some(git2::ObjectType::Blob) = entry.kind()
            && let Ok(obj) = entry.to_object(repo)
            && let Ok(blob) = obj.into_blob()
        {
            let archive_path = full_path.as_path();

            if let Err(err) = append_tar_entry(&mut tar, archive_path, blob.content()) {
                let msg = err.to_string();
                tracing::warn!(path=%display_path(archive_path), error=%msg, "tar append failed");
                if error.is_none() {
                    error = Some(err);
                }
                return 1;
            }

            let path_str = to_unix_path(archive_path);
            if let Err(err) = zip.start_file(&path_str, zopt) {
                let msg = err.to_string();
                tracing::warn!(path=%path_str, error=%msg, "zip start_file failed");
                if error.is_none() {
                    error = Some(err.into());
                }
                return 1;
            }
            if let Err(err) = zip.write_all(blob.content()) {
                let msg = err.to_string();
                tracing::warn!(path=%path_str, error=%msg, "zip write failed");
                if error.is_none() {
                    error = Some(err.into());
                }
                return 1;
            }
        }
        0
    })?;

    if let Some(err) = error {
        return Err(err);
    }

    tar.into_inner()?.finish()?;
    zip.finish()?;
    Ok(())
}

fn normalize_relative(path: &Path) -> PathBuf {
    if path == Path::new(".") {
        PathBuf::new()
    } else {
        path.to_path_buf()
    }
}

fn should_skip_artifact_path(path: &Path) -> bool {
    path.components().any(|c| {
        matches!(
            c.as_os_str().to_str(),
            Some(".git") | Some(".github") | Some("target")
        )
    })
}

fn append_tar_entry(
    tar: &mut TarBuilder<GzEncoder<fs::File>>,
    path: &Path,
    data: &[u8],
) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_path(path)?;
    header.set_size(data.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    let mut cursor = Cursor::new(data);
    tar.append(&header, &mut cursor)?;
    Ok(())
}

fn to_unix_path(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
