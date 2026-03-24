use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};

use crate::models::UpstreamTreeEntry;

#[derive(Debug, Clone)]
pub struct GitCommitMetadata {
    pub sha: String,
    pub parents: Vec<String>,
    pub author_name: String,
    pub author_email: String,
    pub authored_at: String,
    pub committed_at: String,
    pub subject: String,
    pub body: String,
}

fn run_git_raw(cwd: Option<&Path>, args: &[&str]) -> Result<Vec<u8>> {
    let mut command = Command::new("git");
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command
        .args(args)
        .output()
        .with_context(|| format!("failed to execute git {:?}", args))?;
    if !output.status.success() {
        bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

fn run_git_optional(cwd: Option<&Path>, args: &[&str]) -> Option<Vec<u8>> {
    run_git_raw(cwd, args).ok()
}

pub fn clone_repo(repo_url: &str, target_dir: &Path) -> Result<()> {
    let target = target_dir.to_string_lossy().to_string();
    run_git_raw(None, &["clone", "--quiet", repo_url, &target])?;
    Ok(())
}

pub fn rev_parse(repo_dir: &Path, reference: &str) -> Result<String> {
    Ok(
        String::from_utf8(run_git_raw(Some(repo_dir), &["rev-parse", reference])?)?
            .trim()
            .to_string(),
    )
}

pub fn rev_list(repo_dir: &Path, reference: &str) -> Result<Vec<String>> {
    let output = String::from_utf8(run_git_raw(
        Some(repo_dir),
        &["rev-list", "--reverse", reference],
    )?)?;
    Ok(output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

pub fn show_commit_metadata(repo_dir: &Path, sha: &str) -> Result<GitCommitMetadata> {
    let format = "%H%x1f%P%x1f%an%x1f%ae%x1f%aI%x1f%cI%x1f%s%x1f%b";
    let output = String::from_utf8(run_git_raw(
        Some(repo_dir),
        &["show", "-s", &format!("--format={format}"), sha],
    )?)?;
    let mut parts = output.trim_end_matches('\n').split('\x1f');
    let sha = parts
        .next()
        .ok_or_else(|| anyhow!("missing commit sha"))?
        .to_string();
    let parents_raw = parts.next().ok_or_else(|| anyhow!("missing parent data"))?;
    let author_name = parts
        .next()
        .ok_or_else(|| anyhow!("missing author name"))?
        .to_string();
    let author_email = parts
        .next()
        .ok_or_else(|| anyhow!("missing author email"))?
        .to_string();
    let authored_at = parts
        .next()
        .ok_or_else(|| anyhow!("missing authored time"))?
        .to_string();
    let committed_at = parts
        .next()
        .ok_or_else(|| anyhow!("missing committed time"))?
        .to_string();
    let subject = parts.next().unwrap_or_default().to_string();
    let body = parts.collect::<Vec<_>>().join("\x1f");
    Ok(GitCommitMetadata {
        sha,
        parents: parents_raw
            .split_whitespace()
            .map(ToOwned::to_owned)
            .collect(),
        author_name,
        author_email,
        authored_at,
        committed_at,
        subject,
        body,
    })
}

pub fn show_file(repo_dir: &Path, reference: &str, file_path: &str) -> Option<Vec<u8>> {
    run_git_optional(
        Some(repo_dir),
        &["show", &format!("{reference}:{file_path}")],
    )
}

pub fn list_tree(repo_dir: &Path, reference: &str) -> Result<Vec<UpstreamTreeEntry>> {
    let output = String::from_utf8(run_git_raw(
        Some(repo_dir),
        &["ls-tree", "-r", "-z", "--long", reference],
    )?)?;
    let mut entries = Vec::new();
    for raw_entry in output.split('\0') {
        if raw_entry.is_empty() {
            continue;
        }
        let (meta, path) = raw_entry
            .split_once('\t')
            .ok_or_else(|| anyhow!("bad ls-tree entry"))?;
        let mut meta_parts = meta.split_whitespace();
        let mode = meta_parts
            .next()
            .ok_or_else(|| anyhow!("missing ls-tree mode"))?
            .to_string();
        let entry_type = meta_parts
            .next()
            .ok_or_else(|| anyhow!("missing ls-tree type"))?;
        let blob_sha = meta_parts
            .next()
            .ok_or_else(|| anyhow!("missing ls-tree blob sha"))?
            .to_string();
        let size_raw = meta_parts
            .next()
            .ok_or_else(|| anyhow!("missing ls-tree size"))?;
        if entry_type != "blob" {
            continue;
        }
        let size = if size_raw == "-" {
            None
        } else {
            Some(size_raw.parse::<i64>()?)
        };
        entries.push(UpstreamTreeEntry {
            path: path.to_string(),
            blob_sha,
            mode,
            size,
        });
    }
    Ok(entries)
}

pub fn cat_file(repo_dir: &Path, sha: &str) -> Result<Vec<u8>> {
    run_git_raw(Some(repo_dir), &["cat-file", "-p", sha])
}

pub fn head(path: &Path) -> Option<String> {
    run_git_optional(Some(path), &["rev-parse", "HEAD"])
        .and_then(|output| String::from_utf8(output).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn current_branch(path: &Path) -> Option<String> {
    run_git_optional(Some(path), &["branch", "--show-current"])
        .and_then(|output| String::from_utf8(output).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn remote_origin(path: &Path) -> Option<String> {
    run_git_optional(Some(path), &["remote", "get-url", "origin"])
        .and_then(|output| String::from_utf8(output).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn is_dirty(path: &Path) -> bool {
    run_git_optional(Some(path), &["status", "--porcelain"])
        .and_then(|output| String::from_utf8(output).ok())
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

pub fn top_level(path: &Path) -> Option<PathBuf> {
    run_git_optional(Some(path), &["rev-parse", "--show-toplevel"])
        .and_then(|output| String::from_utf8(output).ok())
        .map(|value| PathBuf::from(value.trim()))
}
