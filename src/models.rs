use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HostKind {
    Claude,
    Codex,
    Unknown,
}

impl HostKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Unknown => "unknown",
        }
    }

    pub fn from_db(value: &str) -> Self {
        match value {
            "claude" => Self::Claude,
            "codex" => Self::Codex,
            _ => Self::Unknown,
        }
    }
}

impl Display for HostKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstallType {
    GlobalGit,
    GlobalMaterialized,
    RepoGit,
    RepoMaterialized,
}

impl InstallType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::GlobalGit => "global_git",
            Self::GlobalMaterialized => "global_materialized",
            Self::RepoGit => "repo_git",
            Self::RepoMaterialized => "repo_materialized",
        }
    }

    pub fn from_db(value: &str) -> Self {
        match value {
            "global_git" => Self::GlobalGit,
            "global_materialized" => Self::GlobalMaterialized,
            "repo_git" => Self::RepoGit,
            _ => Self::RepoMaterialized,
        }
    }
}

impl Display for InstallType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamTreeEntry {
    pub path: String,
    pub blob_sha: String,
    pub mode: String,
    pub size: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitSnapshotFile {
    pub path: String,
    pub blob_sha: String,
    pub mode: String,
    pub size: Option<i64>,
    pub content: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamCommitRecord {
    pub sha: String,
    pub source_id: i64,
    pub parents: Vec<String>,
    pub author_name: String,
    pub author_email: String,
    pub authored_at: String,
    pub committed_at: String,
    pub subject: String,
    pub body: String,
    pub version: Option<String>,
    pub manifest_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredRepo {
    pub canonical_path: String,
    pub name: String,
    pub git_remote: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredInstall {
    pub observed_path: String,
    pub resolved_path: String,
    pub repository_path: Option<String>,
    pub host: HostKind,
    pub install_type: InstallType,
    pub is_symlink: bool,
    pub has_git: bool,
    pub local_version: Option<String>,
    pub local_commit: Option<String>,
    pub browse_commit: Option<String>,
    pub manifest_hash: Option<String>,
    pub origin_url: Option<String>,
    pub branch: Option<String>,
    pub dirty: bool,
    pub matched_upstream_commit_sha: Option<String>,
    pub matched_upstream_version: Option<String>,
    pub is_outdated: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanResult {
    pub started_at: String,
    pub finished_at: String,
    pub roots: Vec<String>,
    pub max_depth: usize,
    pub source_head_sha: Option<String>,
    pub source_head_version: Option<String>,
    pub repositories: Vec<DiscoveredRepo>,
    pub projects: Vec<DiscoveredProject>,
    pub installs: Vec<DiscoveredInstall>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredProject {
    pub canonical_path: String,
    pub name: String,
    pub git_remote: Option<String>,
    pub has_claude_md: bool,
    pub has_claude_dir: bool,
    pub has_claude_settings: bool,
    pub claude_settings_paths: Vec<String>,
    pub gstack_install_observed_path: Option<String>,
    pub effective_gstack_version: Option<String>,
    pub effective_gstack_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogInstall {
    pub id: i64,
    pub observed_path: String,
    pub resolved_path: String,
    pub repository_id: Option<i64>,
    pub repository_path: Option<String>,
    pub repository_name: Option<String>,
    pub repository_remote: Option<String>,
    pub host: HostKind,
    pub install_type: InstallType,
    pub is_symlink: bool,
    pub has_git: bool,
    pub local_version: Option<String>,
    pub local_commit: Option<String>,
    pub browse_commit: Option<String>,
    pub manifest_hash: Option<String>,
    pub origin_url: Option<String>,
    pub branch: Option<String>,
    pub dirty: bool,
    pub matched_upstream_commit_sha: Option<String>,
    pub matched_upstream_version: Option<String>,
    pub is_outdated: Option<bool>,
    pub first_seen_at: String,
    pub last_seen_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogProject {
    pub id: i64,
    pub canonical_path: String,
    pub name: String,
    pub git_remote: Option<String>,
    pub has_claude_md: bool,
    pub has_claude_dir: bool,
    pub has_claude_settings: bool,
    pub claude_settings_paths: Vec<String>,
    pub gstack_install_id: Option<i64>,
    pub gstack_install_observed_path: Option<String>,
    pub effective_gstack_version: Option<String>,
    pub effective_gstack_source: String,
    pub first_seen_at: String,
    pub last_seen_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogObservation {
    pub observed_at: String,
    pub local_version: Option<String>,
    pub local_commit: Option<String>,
    pub manifest_hash: Option<String>,
    pub matched_upstream_commit_sha: Option<String>,
    pub is_outdated: Option<bool>,
    pub dirty: bool,
    pub summary: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogSyncEvent {
    pub id: i64,
    pub commit_sha: String,
    pub version: Option<String>,
    pub created_at: String,
    pub dry_run: bool,
    pub status: String,
    pub backup_path: Option<String>,
    pub changed_files: Vec<String>,
    pub details: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallDetail {
    pub install: CatalogInstall,
    pub observations: Vec<CatalogObservation>,
    pub sync_events: Vec<CatalogSyncEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectDetail {
    pub project: CatalogProject,
    pub install: Option<CatalogInstall>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceState {
    pub id: i64,
    pub name: String,
    pub repo_url: String,
    pub default_ref: String,
    pub head_commit_sha: Option<String>,
    pub head_version: Option<String>,
    pub last_ingested_at: Option<String>,
    pub last_ingest_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogSummary {
    pub source: SourceState,
    pub total_installs: i64,
    pub outdated_installs: i64,
    pub git_backed_installs: i64,
    pub total_projects: i64,
    pub projects_with_local_gstack: i64,
    pub by_type: Vec<(String, i64)>,
    pub last_scan_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogVersion {
    pub version: String,
    pub commit_sha: String,
    pub committed_at: String,
    pub subject: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogCommitNote {
    pub commit_sha: String,
    pub version: Option<String>,
    pub committed_at: String,
    pub subject: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogCommitDiff {
    pub added_paths: Vec<String>,
    pub updated_paths: Vec<String>,
    pub removed_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogVersionContext {
    pub selected: CatalogCommitNote,
    pub direction: String,
    pub path_commits: Vec<CatalogCommitNote>,
    pub diff: CatalogCommitDiff,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Idea {
    pub severity: String,
    pub title: String,
    pub rationale: String,
    pub action: String,
    pub install_ids: Vec<i64>,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestSummary {
    pub repo_url: String,
    pub reference: String,
    pub head_sha: String,
    pub head_version: Option<String>,
    pub commit_count: usize,
    pub hydrated_blob_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncChangeSet {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub removed: Vec<String>,
    pub unchanged_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncResult {
    pub target: CatalogInstall,
    pub commit_sha: String,
    pub version: Option<String>,
    pub dry_run: bool,
    pub changes: SyncChangeSet,
    pub backup_path: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyResult {
    pub project: CatalogProject,
    pub install_path: String,
    pub commit_sha: String,
    pub version: Option<String>,
    pub dry_run: bool,
    pub applied_files: Vec<String>,
    pub preserved_local_files: Vec<String>,
    pub merged_files: Vec<String>,
    pub conflict_files: Vec<String>,
    pub removed_files: Vec<String>,
    pub backup_path: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoveResult {
    pub project: CatalogProject,
    pub install_path: String,
    pub dry_run: bool,
    pub removed_files: Vec<String>,
    pub backup_path: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevertResult {
    pub project: CatalogProject,
    pub install_path: String,
    pub restored_from_backup_path: Option<String>,
    pub dry_run: bool,
    pub restored_files: Vec<String>,
    pub removed_files: Vec<String>,
    pub backup_path: Option<String>,
    pub status: String,
}
