use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

use crate::config;

static TEMP_WORKDIR_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .with_context(|| format!("failed to create directory {}", path.display()))
}

pub fn read_trimmed_file(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .and_then(|value| {
            let filtered = value.lines().map(str::trim).find(|line| {
                !line.is_empty()
                    && !line.starts_with("<<<<<<<")
                    && !line.starts_with("=======")
                    && !line.starts_with(">>>>>>>")
            })?;
            Some(filtered.to_string())
        })
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
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    nanos.to_string()
}

pub fn default_db_path() -> PathBuf {
    std::env::var_os("GSTACKQLITE_HYPERVISOR_DB")
        .or_else(|| std::env::var_os("GSTACK_HYPERVISOR_DB"))
        .map(PathBuf::from)
        .unwrap_or_else(config::default_database_path)
}

pub struct TempWorkdir {
    path: PathBuf,
}

impl TempWorkdir {
    pub fn new(prefix: &str) -> Result<Self> {
        let temp_root = std::env::temp_dir();

        for _ in 0..64 {
            let path = temp_root.join(format!(
                "{prefix}-{}-{}-{}",
                std::process::id(),
                timestamp_slug(),
                TEMP_WORKDIR_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("failed to create temp workdir {}", path.display())
                    });
                }
            }
        }

        bail!("failed to allocate unique temp workdir for prefix {prefix}")
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

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{TempWorkdir, read_trimmed_file};

    #[test]
    fn temp_workdirs_are_unique() {
        let first = TempWorkdir::new("gstackqlite-hypervisor-util-test").expect("first temp dir");
        let second = TempWorkdir::new("gstackqlite-hypervisor-util-test").expect("second temp dir");

        assert_ne!(first.path(), second.path());
        assert!(first.path().exists());
        assert!(second.path().exists());
    }

    #[test]
    fn read_trimmed_file_skips_merge_markers() {
        let temp = TempWorkdir::new("gstackqlite-hypervisor-read-trimmed-test").expect("temp dir");
        let file = temp.path().join("VERSION");
        fs::write(
            &file,
            "<<<<<<< local customization\n0.11.11.0\n=======\n0.11.14.0\n>>>>>>> gstack 0.11.14.0\n",
        )
        .expect("write version fixture");

        let value = read_trimmed_file(&file).expect("parsed version");
        assert_eq!(value, "0.11.11.0");
    }
}
