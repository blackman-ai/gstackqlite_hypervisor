use std::fs;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use sha1::{Digest as Sha1Digest, Sha1};
use sha2::Sha256;
use walkdir::WalkDir;

use crate::config::LOCAL_MANIFEST_EXCLUDES;
use crate::util::hex_encode;

#[derive(Debug, Clone)]
pub enum LocalManifestKind {
    File,
    Symlink(String),
}

#[derive(Debug, Clone)]
pub struct LocalManifestEntry {
    pub path: String,
    pub blob_sha: String,
    pub mode: String,
    pub size: u64,
    pub kind: LocalManifestKind,
}

pub fn should_skip_local_path(relative_path: &str) -> bool {
    let normalized = relative_path.replace('\\', "/");
    LOCAL_MANIFEST_EXCLUDES
        .iter()
        .any(|prefix| normalized == *prefix || normalized.starts_with(&format!("{prefix}/")))
}

pub fn git_blob_sha(bytes: &[u8]) -> String {
    let mut digest = Sha1::new();
    digest.update(format!("blob {}\0", bytes.len()).as_bytes());
    digest.update(bytes);
    hex_encode(&digest.finalize())
}

pub fn manifest_hash(entries: &[(String, String, String)]) -> String {
    let mut sorted = entries.to_vec();
    sorted.sort_by(|left, right| left.0.cmp(&right.0));
    let mut digest = Sha256::new();
    for (path, blob_sha, mode) in sorted {
        digest.update(format!("{mode}\t{blob_sha}\t{path}\n").as_bytes());
    }
    hex_encode(&digest.finalize())
}

pub fn collect_local_manifest(root: &Path) -> Result<Vec<LocalManifestEntry>> {
    let mut entries = Vec::new();
    if !root.exists() {
        return Ok(entries);
    }

    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            if entry.path() == root {
                return true;
            }
            let Ok(relative) = entry.path().strip_prefix(root) else {
                return true;
            };
            !should_skip_local_path(&relative.to_string_lossy())
        });

    for entry in walker {
        let entry = entry?;
        if entry.file_type().is_dir() {
            continue;
        }

        let relative = entry
            .path()
            .strip_prefix(root)
            .with_context(|| format!("failed to strip prefix {}", root.display()))?
            .to_string_lossy()
            .replace('\\', "/");

        if should_skip_local_path(&relative) {
            continue;
        }

        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() {
            let link_target = fs::read_link(entry.path())?.to_string_lossy().to_string();
            let blob_bytes = link_target.as_bytes();
            entries.push(LocalManifestEntry {
                path: relative,
                blob_sha: git_blob_sha(blob_bytes),
                mode: "120000".to_string(),
                size: blob_bytes.len() as u64,
                kind: LocalManifestKind::Symlink(link_target),
            });
            continue;
        }

        if !metadata.is_file() {
            continue;
        }

        let mut file = fs::File::open(entry.path())?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)?;
        let executable = metadata.permissions().mode() & 0o111 != 0;
        entries.push(LocalManifestEntry {
            path: relative,
            blob_sha: git_blob_sha(&buffer),
            mode: if executable { "100755" } else { "100644" }.to_string(),
            size: buffer.len() as u64,
            kind: LocalManifestKind::File,
        });
    }

    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(entries)
}

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
