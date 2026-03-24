use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::config;

pub fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .with_context(|| format!("failed to create directory {}", path.display()))
}

pub fn read_trimmed_file(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn real_path_or_original(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub fn now_iso() -> String {
    let output = std::process::Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output();
    match output {
        Ok(result) if result.status.success() => {
            String::from_utf8_lossy(&result.stdout).trim().to_string()
        }
        _ => "1970-01-01T00:00:00Z".to_string(),
    }
}

pub fn compare_versions(left: Option<&str>, right: Option<&str>) -> Option<std::cmp::Ordering> {
    let left = left?;
    let right = right?;
    let left_parts: Vec<u64> = left
        .split('.')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    let right_parts: Vec<u64> = right
        .split('.')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    let length = left_parts.len().max(right_parts.len());

    for index in 0..length {
        let a = *left_parts.get(index).unwrap_or(&0);
        let b = *right_parts.get(index).unwrap_or(&0);
        match a.cmp(&b) {
            std::cmp::Ordering::Equal => {}
            ordering => return Some(ordering),
        }
    }

    Some(std::cmp::Ordering::Equal)
}

pub fn hex_encode(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

pub fn timestamp_slug() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    millis.to_string()
}

pub fn default_db_path() -> PathBuf {
    std::env::var_os("GSTACK_HYPERVISOR_DB")
        .map(PathBuf::from)
        .unwrap_or_else(config::default_database_path)
}

pub struct TempWorkdir {
    path: PathBuf,
}

impl TempWorkdir {
    pub fn new(prefix: &str) -> Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            timestamp_slug()
        ));
        ensure_dir(&path)?;
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempWorkdir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
