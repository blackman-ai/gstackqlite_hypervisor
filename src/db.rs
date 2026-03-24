use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use crate::config::{DEFAULT_SOURCE_NAME, DEFAULT_UPSTREAM_REF, DEFAULT_UPSTREAM_URL};
use crate::models::{
    CatalogCommitDiff, CatalogCommitNote, CatalogInstall, CatalogObservation, CatalogProject,
    CatalogSummary, CatalogSyncEvent, CatalogVersion, CatalogVersionContext, CommitSnapshotFile,
    DiscoveredInstall, DiscoveredProject, HostKind, InstallDetail, InstallType, ProjectDetail,
    ScanResult, SourceState, UpstreamCommitRecord, UpstreamTreeEntry,
};
use crate::util::{ensure_dir, now_iso};

pub struct Catalog {
    pub path: PathBuf,
    conn: Connection,
}

fn bool_to_sql(value: bool) -> i64 {
    if value { 1 } else { 0 }
}

fn sql_to_bool(value: Option<i64>) -> Option<bool> {
    value.map(|flag| flag != 0)
}

impl Catalog {
    pub fn new(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            ensure_dir(parent)?;
        }
        let conn =
            Connection::open(path).with_context(|| format!("failed to open {}", path.display()))?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
        let catalog = Self {
            path: path.to_path_buf(),
            conn,
        };
        catalog.initialize_schema()?;
        catalog.migrate_schema()?;
        catalog.ensure_default_source()?;
        Ok(catalog)
    }

    fn initialize_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS upstream_sources (
              id INTEGER PRIMARY KEY,
              name TEXT NOT NULL UNIQUE,
              repo_url TEXT NOT NULL,
              default_ref TEXT NOT NULL,
              head_commit_sha TEXT,
              head_version TEXT,
              last_ingested_at TEXT,
              last_ingest_error TEXT
            );
            CREATE TABLE IF NOT EXISTS upstream_commits (
              sha TEXT PRIMARY KEY,
              source_id INTEGER NOT NULL REFERENCES upstream_sources(id),
              parents_json TEXT NOT NULL,
              author_name TEXT NOT NULL,
              author_email TEXT NOT NULL,
              authored_at TEXT NOT NULL,
              committed_at TEXT NOT NULL,
              subject TEXT NOT NULL,
              body TEXT NOT NULL,
              version TEXT,
              manifest_hash TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS upstream_commit_files (
              commit_sha TEXT NOT NULL REFERENCES upstream_commits(sha),
              path TEXT NOT NULL,
              blob_sha TEXT NOT NULL,
              mode TEXT NOT NULL,
              size INTEGER,
              PRIMARY KEY (commit_sha, path)
            );
            CREATE TABLE IF NOT EXISTS upstream_blobs (
              sha TEXT PRIMARY KEY,
              size INTEGER NOT NULL,
              content BLOB,
              hydrated_at TEXT
            );
            CREATE TABLE IF NOT EXISTS repositories (
              id INTEGER PRIMARY KEY,
              canonical_path TEXT NOT NULL UNIQUE,
              name TEXT NOT NULL,
              git_remote TEXT,
              first_seen_at TEXT NOT NULL,
              last_seen_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS local_installs (
              id INTEGER PRIMARY KEY,
              observed_path TEXT NOT NULL UNIQUE,
              resolved_path TEXT NOT NULL,
              repository_id INTEGER REFERENCES repositories(id),
              host TEXT NOT NULL,
              install_type TEXT NOT NULL,
              is_symlink INTEGER NOT NULL,
              has_git INTEGER NOT NULL,
              local_version TEXT,
              local_commit TEXT,
              browse_commit TEXT,
              manifest_hash TEXT,
              origin_url TEXT,
              branch TEXT,
              dirty INTEGER NOT NULL,
              matched_upstream_commit_sha TEXT,
              matched_upstream_version TEXT,
              is_outdated INTEGER,
              first_seen_at TEXT NOT NULL,
              last_seen_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS scan_runs (
              id INTEGER PRIMARY KEY,
              started_at TEXT NOT NULL,
              finished_at TEXT NOT NULL,
              roots_json TEXT NOT NULL,
              max_depth INTEGER NOT NULL,
              source_head_sha TEXT,
              source_head_version TEXT,
              install_count INTEGER NOT NULL,
              repo_count INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS projects (
              id INTEGER PRIMARY KEY,
              canonical_path TEXT NOT NULL UNIQUE,
              name TEXT NOT NULL,
              git_remote TEXT,
              has_git_repo INTEGER NOT NULL DEFAULT 0,
              has_claude_md INTEGER NOT NULL,
              has_claude_dir INTEGER NOT NULL,
              has_claude_settings INTEGER NOT NULL,
              claude_settings_paths_json TEXT NOT NULL,
              has_agents_md INTEGER NOT NULL DEFAULT 0,
              has_agents_dir INTEGER NOT NULL DEFAULT 0,
              has_codex_dir INTEGER NOT NULL DEFAULT 0,
              has_codex_settings INTEGER NOT NULL DEFAULT 0,
              codex_settings_paths_json TEXT NOT NULL DEFAULT '[]',
              gstack_install_id INTEGER REFERENCES local_installs(id),
              gstack_install_observed_path TEXT,
              effective_gstack_version TEXT,
              effective_gstack_source TEXT NOT NULL,
              first_seen_at TEXT NOT NULL,
              last_seen_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS install_observations (
              id INTEGER PRIMARY KEY,
              scan_run_id INTEGER NOT NULL REFERENCES scan_runs(id),
              install_id INTEGER NOT NULL REFERENCES local_installs(id),
              observed_at TEXT NOT NULL,
              local_version TEXT,
              local_commit TEXT,
              manifest_hash TEXT,
              matched_upstream_commit_sha TEXT,
              is_outdated INTEGER,
              dirty INTEGER NOT NULL,
              summary_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS project_observations (
              id INTEGER PRIMARY KEY,
              scan_run_id INTEGER NOT NULL REFERENCES scan_runs(id),
              project_id INTEGER NOT NULL REFERENCES projects(id),
              observed_at TEXT NOT NULL,
              gstack_install_id INTEGER REFERENCES local_installs(id),
              effective_gstack_version TEXT,
              summary_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS sync_events (
              id INTEGER PRIMARY KEY,
              install_id INTEGER NOT NULL REFERENCES local_installs(id),
              commit_sha TEXT NOT NULL REFERENCES upstream_commits(sha),
              version TEXT,
              created_at TEXT NOT NULL,
              dry_run INTEGER NOT NULL,
              changed_files_json TEXT NOT NULL,
              backup_path TEXT,
              status TEXT NOT NULL,
              details_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS app_settings (
              key TEXT PRIMARY KEY,
              value TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_upstream_commit_version ON upstream_commits(version);
            CREATE INDEX IF NOT EXISTS idx_upstream_commit_manifest_hash ON upstream_commits(manifest_hash);
            CREATE INDEX IF NOT EXISTS idx_local_installs_outdated ON local_installs(is_outdated);
            CREATE INDEX IF NOT EXISTS idx_projects_effective_gstack_version ON projects(effective_gstack_version);
            "#,
        )?;
        Ok(())
    }

    fn migrate_schema(&self) -> Result<()> {
        self.ensure_column_exists("projects", "has_git_repo", "INTEGER NOT NULL DEFAULT 0")?;
        self.ensure_column_exists("projects", "has_agents_md", "INTEGER NOT NULL DEFAULT 0")?;
        self.ensure_column_exists("projects", "has_agents_dir", "INTEGER NOT NULL DEFAULT 0")?;
        self.ensure_column_exists("projects", "has_codex_dir", "INTEGER NOT NULL DEFAULT 0")?;
        self.ensure_column_exists(
            "projects",
            "has_codex_settings",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        self.ensure_column_exists(
            "projects",
            "codex_settings_paths_json",
            "TEXT NOT NULL DEFAULT '[]'",
        )?;
        Ok(())
    }

    fn ensure_column_exists(&self, table: &str, column: &str, definition: &str) -> Result<()> {
        let pragma = format!("PRAGMA table_info({table})");
        let mut statement = self.conn.prepare(&pragma)?;
        let rows = statement.query_map([], |row| row.get::<_, String>(1))?;
        for row in rows {
            if row? == column {
                return Ok(());
            }
        }

        let sql = format!("ALTER TABLE {table} ADD COLUMN {column} {definition}");
        self.conn.execute(&sql, [])?;
        Ok(())
    }

    fn ensure_default_source(&self) -> Result<()> {
        let exists: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM upstream_sources WHERE name = ?1",
                params![DEFAULT_SOURCE_NAME],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_none() {
            self.conn.execute(
                "INSERT INTO upstream_sources (name, repo_url, default_ref) VALUES (?1, ?2, ?3)",
                params![
                    DEFAULT_SOURCE_NAME,
                    DEFAULT_UPSTREAM_URL,
                    DEFAULT_UPSTREAM_REF
                ],
            )?;
        }
        Ok(())
    }

    pub fn source_state(&self) -> Result<SourceState> {
        self.conn.query_row(
            "SELECT id, name, repo_url, default_ref, head_commit_sha, head_version, last_ingested_at, last_ingest_error
             FROM upstream_sources
             WHERE name = ?1",
            params![DEFAULT_SOURCE_NAME],
            |row| {
                Ok(SourceState {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    repo_url: row.get(2)?,
                    default_ref: row.get(3)?,
                    head_commit_sha: row.get(4)?,
                    head_version: row.get(5)?,
                    last_ingested_at: row.get(6)?,
                    last_ingest_error: row.get(7)?,
                })
            },
        )
        .with_context(|| "failed to load source state")
    }

    pub fn update_source_state(
        &self,
        repo_url: &str,
        default_ref: &str,
        head_commit_sha: Option<&str>,
        head_version: Option<&str>,
        last_ingest_error: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE upstream_sources
             SET repo_url = ?1,
                 default_ref = ?2,
                 head_commit_sha = ?3,
                 head_version = ?4,
                 last_ingested_at = ?5,
                 last_ingest_error = ?6
             WHERE name = ?7",
            params![
                repo_url,
                default_ref,
                head_commit_sha,
                head_version,
                now_iso(),
                last_ingest_error,
                DEFAULT_SOURCE_NAME
            ],
        )?;
        Ok(())
    }

    pub fn upsert_commit(&self, record: &UpstreamCommitRecord) -> Result<()> {
        self.conn.execute(
            "INSERT INTO upstream_commits (
                sha, source_id, parents_json, author_name, author_email, authored_at, committed_at, subject, body, version, manifest_hash
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(sha) DO UPDATE SET
                source_id = excluded.source_id,
                parents_json = excluded.parents_json,
                author_name = excluded.author_name,
                author_email = excluded.author_email,
                authored_at = excluded.authored_at,
                committed_at = excluded.committed_at,
                subject = excluded.subject,
                body = excluded.body,
                version = excluded.version,
                manifest_hash = excluded.manifest_hash",
            params![
                record.sha,
                record.source_id,
                serde_json::to_string(&record.parents)?,
                record.author_name,
                record.author_email,
                record.authored_at,
                record.committed_at,
                record.subject,
                record.body,
                record.version,
                record.manifest_hash
            ],
        )?;
        Ok(())
    }

    pub fn replace_commit_files(
        &self,
        commit_sha: &str,
        entries: &[UpstreamTreeEntry],
    ) -> Result<()> {
        self.conn.execute(
            "DELETE FROM upstream_commit_files WHERE commit_sha = ?1",
            params![commit_sha],
        )?;
        for entry in entries {
            self.conn.execute(
                "INSERT INTO upstream_commit_files (commit_sha, path, blob_sha, mode, size) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![commit_sha, entry.path, entry.blob_sha, entry.mode, entry.size],
            )?;
        }
        Ok(())
    }

    pub fn upsert_blob(&self, sha: &str, size: i64, content: &[u8]) -> Result<()> {
        self.conn.execute(
            "INSERT INTO upstream_blobs (sha, size, content, hydrated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(sha) DO UPDATE SET size = excluded.size, content = excluded.content, hydrated_at = excluded.hydrated_at",
            params![sha, size, content, now_iso()],
        )?;
        Ok(())
    }

    pub fn missing_blob_shas(&self, blob_shas: &[String]) -> Result<Vec<String>> {
        let mut missing = Vec::new();
        for blob_sha in blob_shas {
            let content_exists: Option<i64> = self
                .conn
                .query_row(
                    "SELECT 1 FROM upstream_blobs WHERE sha = ?1 AND content IS NOT NULL",
                    params![blob_sha],
                    |row| row.get(0),
                )
                .optional()?;
            if content_exists.is_none() {
                missing.push(blob_sha.clone());
            }
        }
        Ok(missing)
    }

    pub fn get_commit_by_sha(&self, sha: &str) -> Result<Option<(String, Option<String>)>> {
        self.conn
            .query_row(
                "SELECT sha, version FROM upstream_commits WHERE sha = ?1",
                params![sha],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn get_commit_by_version(&self, version: &str) -> Result<Option<(String, Option<String>)>> {
        self.conn
            .query_row(
                "SELECT sha, version FROM upstream_commits WHERE version = ?1 ORDER BY committed_at DESC LIMIT 1",
                params![version],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn get_commit_by_manifest_hash(
        &self,
        manifest_hash: &str,
    ) -> Result<Option<(String, Option<String>)>> {
        self.conn
            .query_row(
                "SELECT sha, version FROM upstream_commits WHERE manifest_hash = ?1 ORDER BY committed_at DESC LIMIT 1",
                params![manifest_hash],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn match_upstream_commit(
        &self,
        manifest_hash: Option<&str>,
        commit_sha: Option<&str>,
        browse_commit: Option<&str>,
        version: Option<&str>,
    ) -> Result<Option<(String, Option<String>)>> {
        if let Some(manifest_hash) = manifest_hash {
            if let Some(record) = self.get_commit_by_manifest_hash(manifest_hash)? {
                return Ok(Some(record));
            }
        }
        if let Some(commit_sha) = commit_sha {
            if let Some(record) = self.get_commit_by_sha(commit_sha)? {
                return Ok(Some(record));
            }
        }
        if let Some(browse_commit) = browse_commit {
            if let Some(record) = self.get_commit_by_sha(browse_commit)? {
                return Ok(Some(record));
            }
        }
        if let Some(version) = version {
            return self.get_commit_by_version(version);
        }
        Ok(None)
    }

    pub fn resolve_commit_ref(
        &self,
        commit_sha: Option<&str>,
        version: Option<&str>,
    ) -> Result<Option<(String, Option<String>)>> {
        if let Some(commit_sha) = commit_sha {
            return self.get_commit_by_sha(commit_sha);
        }
        if let Some(version) = version {
            return self.get_commit_by_version(version);
        }
        let source = self.source_state()?;
        match source.head_commit_sha {
            Some(ref sha) => self.get_commit_by_sha(sha),
            None => Ok(None),
        }
    }

    pub fn commit_blob_shas(&self, commit_sha: &str) -> Result<Vec<String>> {
        let mut statement = self.conn.prepare(
            "SELECT blob_sha FROM upstream_commit_files WHERE commit_sha = ?1 ORDER BY path",
        )?;
        let rows = statement.query_map(params![commit_sha], |row| row.get(0))?;
        let mut shas = Vec::new();
        for row in rows {
            shas.push(row?);
        }
        Ok(shas)
    }

    pub fn commit_files(&self, commit_sha: &str) -> Result<Vec<CommitSnapshotFile>> {
        let mut statement = self.conn.prepare(
            "SELECT f.path, f.blob_sha, f.mode, f.size, b.content
             FROM upstream_commit_files f
             LEFT JOIN upstream_blobs b ON b.sha = f.blob_sha
             WHERE f.commit_sha = ?1
             ORDER BY f.path",
        )?;
        let rows = statement.query_map(params![commit_sha], |row| {
            Ok(CommitSnapshotFile {
                path: row.get(0)?,
                blob_sha: row.get(1)?,
                mode: row.get(2)?,
                size: row.get(3)?,
                content: row.get(4)?,
            })
        })?;
        let mut files = Vec::new();
        for row in rows {
            files.push(row?);
        }
        Ok(files)
    }

    fn with_transaction<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T>,
    {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        match f(&self.conn) {
            Ok(value) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(value)
            }
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub fn record_scan(&self, result: &ScanResult) -> Result<i64> {
        self.with_transaction(|conn| {
            conn.execute(
                "INSERT INTO scan_runs (
                    started_at, finished_at, roots_json, max_depth, source_head_sha, source_head_version, install_count, repo_count
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    result.started_at,
                    result.finished_at,
                    serde_json::to_string(&result.roots)?,
                    result.max_depth as i64,
                    result.source_head_sha,
                    result.source_head_version,
                    result.installs.len() as i64,
                    result.repositories.len() as i64
                ],
            )?;
            let scan_run_id = conn.last_insert_rowid();
            let mut install_id_by_observed_path = std::collections::HashMap::new();

            for repo in &result.repositories {
                let existing_id: Option<i64> = conn
                    .query_row(
                        "SELECT id FROM repositories WHERE canonical_path = ?1",
                        params![repo.canonical_path],
                        |row| row.get(0),
                    )
                    .optional()?;
                let repo_id = match existing_id {
                    Some(id) => {
                        conn.execute(
                            "UPDATE repositories SET name = ?1, git_remote = ?2, last_seen_at = ?3 WHERE id = ?4",
                            params![repo.name, repo.git_remote, result.finished_at, id],
                        )?;
                        id
                    }
                    None => {
                        conn.execute(
                            "INSERT INTO repositories (canonical_path, name, git_remote, first_seen_at, last_seen_at)
                             VALUES (?1, ?2, ?3, ?4, ?5)",
                            params![repo.canonical_path, repo.name, repo.git_remote, result.finished_at, result.finished_at],
                        )?;
                        conn.last_insert_rowid()
                    }
                };

                for install in result.installs.iter().filter(|install| install.repository_path.as_deref() == Some(repo.canonical_path.as_str())) {
                    let install_id =
                        self.upsert_install(conn, install, Some(repo_id), &result.finished_at, scan_run_id)?;
                    install_id_by_observed_path.insert(install.observed_path.clone(), install_id);
                }
            }

            for install in result.installs.iter().filter(|install| install.repository_path.is_none()) {
                let install_id =
                    self.upsert_install(conn, install, None, &result.finished_at, scan_run_id)?;
                install_id_by_observed_path.insert(install.observed_path.clone(), install_id);
            }

            for install in &result.installs {
                if install_id_by_observed_path.contains_key(&install.observed_path) {
                    continue;
                }
                let install_id =
                    self.upsert_install(conn, install, None, &result.finished_at, scan_run_id)?;
                install_id_by_observed_path.insert(install.observed_path.clone(), install_id);
            }

            for project in &result.projects {
                let gstack_install_id = project
                    .gstack_install_observed_path
                    .as_ref()
                    .and_then(|path| install_id_by_observed_path.get(path).copied());
                self.upsert_project(conn, project, gstack_install_id, &result.finished_at, scan_run_id)?;
            }

            Ok(scan_run_id)
        })
    }

    fn upsert_install(
        &self,
        conn: &Connection,
        install: &DiscoveredInstall,
        repository_id: Option<i64>,
        seen_at: &str,
        scan_run_id: i64,
    ) -> Result<i64> {
        let existing_id: Option<i64> = conn
            .query_row(
                "SELECT id FROM local_installs WHERE observed_path = ?1",
                params![install.observed_path],
                |row| row.get(0),
            )
            .optional()?;
        let install_id = match existing_id {
            Some(id) => {
                conn.execute(
                    "UPDATE local_installs
                     SET resolved_path = ?1, repository_id = ?2, host = ?3, install_type = ?4, is_symlink = ?5, has_git = ?6,
                         local_version = ?7, local_commit = ?8, browse_commit = ?9, manifest_hash = ?10, origin_url = ?11,
                         branch = ?12, dirty = ?13, matched_upstream_commit_sha = ?14, matched_upstream_version = ?15,
                         is_outdated = ?16, last_seen_at = ?17
                     WHERE id = ?18",
                    params![
                        install.resolved_path,
                        repository_id,
                        install.host.as_str(),
                        install.install_type.as_str(),
                        bool_to_sql(install.is_symlink),
                        bool_to_sql(install.has_git),
                        install.local_version,
                        install.local_commit,
                        install.browse_commit,
                        install.manifest_hash,
                        install.origin_url,
                        install.branch,
                        bool_to_sql(install.dirty),
                        install.matched_upstream_commit_sha,
                        install.matched_upstream_version,
                        install.is_outdated.map(bool_to_sql),
                        seen_at,
                        id
                    ],
                )?;
                id
            }
            None => {
                conn.execute(
                    "INSERT INTO local_installs (
                        observed_path, resolved_path, repository_id, host, install_type, is_symlink, has_git,
                        local_version, local_commit, browse_commit, manifest_hash, origin_url, branch, dirty,
                        matched_upstream_commit_sha, matched_upstream_version, is_outdated, first_seen_at, last_seen_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
                    params![
                        install.observed_path,
                        install.resolved_path,
                        repository_id,
                        install.host.as_str(),
                        install.install_type.as_str(),
                        bool_to_sql(install.is_symlink),
                        bool_to_sql(install.has_git),
                        install.local_version,
                        install.local_commit,
                        install.browse_commit,
                        install.manifest_hash,
                        install.origin_url,
                        install.branch,
                        bool_to_sql(install.dirty),
                        install.matched_upstream_commit_sha,
                        install.matched_upstream_version,
                        install.is_outdated.map(bool_to_sql),
                        seen_at,
                        seen_at
                    ],
                )?;
                conn.last_insert_rowid()
            }
        };

        conn.execute(
            "INSERT INTO install_observations (
                scan_run_id, install_id, observed_at, local_version, local_commit, manifest_hash,
                matched_upstream_commit_sha, is_outdated, dirty, summary_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                scan_run_id,
                install_id,
                seen_at,
                install.local_version,
                install.local_commit,
                install.manifest_hash,
                install.matched_upstream_commit_sha,
                install.is_outdated.map(bool_to_sql),
                bool_to_sql(install.dirty),
                serde_json::to_string(&json!({
                    "observed_path": install.observed_path,
                    "resolved_path": install.resolved_path,
                    "host": install.host.as_str(),
                    "install_type": install.install_type.as_str(),
                    "browse_commit": install.browse_commit,
                    "matched_upstream_version": install.matched_upstream_version,
                }))?
            ],
        )?;
        Ok(install_id)
    }

    fn upsert_project(
        &self,
        conn: &Connection,
        project: &DiscoveredProject,
        gstack_install_id: Option<i64>,
        seen_at: &str,
        scan_run_id: i64,
    ) -> Result<()> {
        let existing_id: Option<i64> = conn
            .query_row(
                "SELECT id FROM projects WHERE canonical_path = ?1",
                params![project.canonical_path],
                |row| row.get(0),
            )
            .optional()?;
        let project_id = match existing_id {
            Some(id) => {
                conn.execute(
                    "UPDATE projects
                     SET name = ?1, git_remote = ?2, has_git_repo = ?3, has_claude_md = ?4, has_claude_dir = ?5,
                         has_claude_settings = ?6, claude_settings_paths_json = ?7, has_agents_md = ?8,
                         has_agents_dir = ?9, has_codex_dir = ?10, has_codex_settings = ?11,
                         codex_settings_paths_json = ?12, gstack_install_id = ?13,
                         gstack_install_observed_path = ?14, effective_gstack_version = ?15,
                         effective_gstack_source = ?16, last_seen_at = ?17
                     WHERE id = ?18",
                    params![
                        project.name,
                        project.git_remote,
                        bool_to_sql(project.has_git_repo),
                        bool_to_sql(project.has_claude_md),
                        bool_to_sql(project.has_claude_dir),
                        bool_to_sql(project.has_claude_settings),
                        serde_json::to_string(&project.claude_settings_paths)?,
                        bool_to_sql(project.has_agents_md),
                        bool_to_sql(project.has_agents_dir),
                        bool_to_sql(project.has_codex_dir),
                        bool_to_sql(project.has_codex_settings),
                        serde_json::to_string(&project.codex_settings_paths)?,
                        gstack_install_id,
                        project.gstack_install_observed_path,
                        project.effective_gstack_version,
                        project.effective_gstack_source,
                        seen_at,
                        id
                    ],
                )?;
                id
            }
            None => {
                conn.execute(
                    "INSERT INTO projects (
                        canonical_path, name, git_remote, has_git_repo, has_claude_md, has_claude_dir,
                        has_claude_settings, claude_settings_paths_json, has_agents_md, has_agents_dir,
                        has_codex_dir, has_codex_settings, codex_settings_paths_json, gstack_install_id,
                        gstack_install_observed_path, effective_gstack_version, effective_gstack_source,
                        first_seen_at, last_seen_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
                    params![
                        project.canonical_path,
                        project.name,
                        project.git_remote,
                        bool_to_sql(project.has_git_repo),
                        bool_to_sql(project.has_claude_md),
                        bool_to_sql(project.has_claude_dir),
                        bool_to_sql(project.has_claude_settings),
                        serde_json::to_string(&project.claude_settings_paths)?,
                        bool_to_sql(project.has_agents_md),
                        bool_to_sql(project.has_agents_dir),
                        bool_to_sql(project.has_codex_dir),
                        bool_to_sql(project.has_codex_settings),
                        serde_json::to_string(&project.codex_settings_paths)?,
                        gstack_install_id,
                        project.gstack_install_observed_path,
                        project.effective_gstack_version,
                        project.effective_gstack_source,
                        seen_at,
                        seen_at
                    ],
                )?;
                conn.last_insert_rowid()
            }
        };

        conn.execute(
            "INSERT INTO project_observations (
                scan_run_id, project_id, observed_at, gstack_install_id, effective_gstack_version, summary_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                scan_run_id,
                project_id,
                seen_at,
                gstack_install_id,
                project.effective_gstack_version,
                serde_json::to_string(&json!({
                    "canonical_path": project.canonical_path,
                    "has_git_repo": project.has_git_repo,
                    "claude_settings_paths": project.claude_settings_paths,
                    "codex_settings_paths": project.codex_settings_paths,
                    "has_agents_md": project.has_agents_md,
                    "has_agents_dir": project.has_agents_dir,
                    "has_codex_dir": project.has_codex_dir,
                    "has_codex_settings": project.has_codex_settings,
                    "effective_gstack_source": project.effective_gstack_source,
                    "gstack_install_observed_path": project.gstack_install_observed_path,
                }))?
            ],
        )?;
        Ok(())
    }

    pub fn list_installs(
        &self,
        outdated_only: bool,
        host: Option<&str>,
        install_type: Option<&str>,
    ) -> Result<Vec<CatalogInstall>> {
        let mut query = String::from(
            "SELECT
                li.id, li.observed_path, li.resolved_path, li.repository_id, r.canonical_path, r.name, r.git_remote,
                li.host, li.install_type, li.is_symlink, li.has_git, li.local_version, li.local_commit, li.browse_commit,
                li.manifest_hash, li.origin_url, li.branch, li.dirty, li.matched_upstream_commit_sha,
                li.matched_upstream_version, li.is_outdated, li.first_seen_at, li.last_seen_at
             FROM local_installs li
             LEFT JOIN repositories r ON r.id = li.repository_id",
        );
        let mut clauses = Vec::new();
        let mut params_vec: Vec<String> = Vec::new();
        if outdated_only {
            clauses.push("li.is_outdated = 1".to_string());
        }
        if let Some(host) = host {
            clauses.push("li.host = ?".to_string());
            params_vec.push(host.to_string());
        }
        if let Some(install_type) = install_type {
            clauses.push("li.install_type = ?".to_string());
            params_vec.push(install_type.to_string());
        }
        if !clauses.is_empty() {
            query.push_str(" WHERE ");
            query.push_str(&clauses.join(" AND "));
        }
        query.push_str(" ORDER BY li.observed_path");

        let mut statement = self.conn.prepare(&query)?;
        let rows = statement.query_map(rusqlite::params_from_iter(params_vec.iter()), |row| {
            Ok(CatalogInstall {
                id: row.get(0)?,
                observed_path: row.get(1)?,
                resolved_path: row.get(2)?,
                repository_id: row.get(3)?,
                repository_path: row.get(4)?,
                repository_name: row.get(5)?,
                repository_remote: row.get(6)?,
                host: HostKind::from_db(&row.get::<_, String>(7)?),
                install_type: InstallType::from_db(&row.get::<_, String>(8)?),
                is_symlink: row.get::<_, i64>(9)? != 0,
                has_git: row.get::<_, i64>(10)? != 0,
                local_version: row.get(11)?,
                local_commit: row.get(12)?,
                browse_commit: row.get(13)?,
                manifest_hash: row.get(14)?,
                origin_url: row.get(15)?,
                branch: row.get(16)?,
                dirty: row.get::<_, i64>(17)? != 0,
                matched_upstream_commit_sha: row.get(18)?,
                matched_upstream_version: row.get(19)?,
                is_outdated: sql_to_bool(row.get(20)?),
                first_seen_at: row.get(21)?,
                last_seen_at: row.get(22)?,
            })
        })?;
        let mut installs = Vec::new();
        for row in rows {
            installs.push(row?);
        }
        Ok(installs)
    }

    pub fn install_detail(&self, identifier: &str) -> Result<Option<InstallDetail>> {
        let install = if let Ok(id) = identifier.parse::<i64>() {
            self.list_installs(false, None, None)?
                .into_iter()
                .find(|install| install.id == id)
        } else {
            self.list_installs(false, None, None)?
                .into_iter()
                .find(|install| {
                    install.observed_path == identifier || install.resolved_path == identifier
                })
        };

        let Some(install) = install else {
            return Ok(None);
        };

        let mut observations_statement = self.conn.prepare(
            "SELECT observed_at, local_version, local_commit, manifest_hash, matched_upstream_commit_sha, is_outdated, dirty, summary_json
             FROM install_observations
             WHERE install_id = ?1
             ORDER BY observed_at DESC
             LIMIT 20",
        )?;
        let observation_rows = observations_statement.query_map(params![install.id], |row| {
            let summary_json: String = row.get(7)?;
            Ok(CatalogObservation {
                observed_at: row.get(0)?,
                local_version: row.get(1)?,
                local_commit: row.get(2)?,
                manifest_hash: row.get(3)?,
                matched_upstream_commit_sha: row.get(4)?,
                is_outdated: sql_to_bool(row.get(5)?),
                dirty: row.get::<_, i64>(6)? != 0,
                summary: serde_json::from_str(&summary_json).unwrap_or(Value::Null),
            })
        })?;
        let mut observations = Vec::new();
        for row in observation_rows {
            observations.push(row?);
        }

        let mut sync_statement = self.conn.prepare(
            "SELECT id, commit_sha, version, created_at, dry_run, changed_files_json, backup_path, status, details_json
             FROM sync_events
             WHERE install_id = ?1
             ORDER BY created_at DESC
             LIMIT 20",
        )?;
        let sync_rows = sync_statement.query_map(params![install.id], |row| {
            let changed_files_json: String = row.get(5)?;
            let details_json: String = row.get(8)?;
            Ok(CatalogSyncEvent {
                id: row.get(0)?,
                commit_sha: row.get(1)?,
                version: row.get(2)?,
                created_at: row.get(3)?,
                dry_run: row.get::<_, i64>(4)? != 0,
                changed_files: serde_json::from_str(&changed_files_json).unwrap_or_default(),
                backup_path: row.get(6)?,
                status: row.get(7)?,
                details: serde_json::from_str(&details_json).unwrap_or(Value::Null),
            })
        })?;
        let mut sync_events = Vec::new();
        for row in sync_rows {
            sync_events.push(row?);
        }

        Ok(Some(InstallDetail {
            install,
            observations,
            sync_events,
        }))
    }

    pub fn list_projects(&self) -> Result<Vec<CatalogProject>> {
        let mut statement = self.conn.prepare(
            "SELECT
                id, canonical_path, name, git_remote, has_git_repo, has_claude_md, has_claude_dir,
                has_claude_settings, claude_settings_paths_json, has_agents_md, has_agents_dir,
                has_codex_dir, has_codex_settings, codex_settings_paths_json, gstack_install_id,
                gstack_install_observed_path, effective_gstack_version, effective_gstack_source,
                first_seen_at, last_seen_at
             FROM projects
             ORDER BY canonical_path",
        )?;
        let rows = statement.query_map([], |row| {
            let paths_json: String = row.get(8)?;
            let codex_paths_json: String = row.get(13)?;
            Ok(CatalogProject {
                id: row.get(0)?,
                canonical_path: row.get(1)?,
                name: row.get(2)?,
                git_remote: row.get(3)?,
                has_git_repo: row.get::<_, i64>(4)? != 0,
                has_claude_md: row.get::<_, i64>(5)? != 0,
                has_claude_dir: row.get::<_, i64>(6)? != 0,
                has_claude_settings: row.get::<_, i64>(7)? != 0,
                claude_settings_paths: serde_json::from_str(&paths_json).unwrap_or_default(),
                has_agents_md: row.get::<_, i64>(9)? != 0,
                has_agents_dir: row.get::<_, i64>(10)? != 0,
                has_codex_dir: row.get::<_, i64>(11)? != 0,
                has_codex_settings: row.get::<_, i64>(12)? != 0,
                codex_settings_paths: serde_json::from_str(&codex_paths_json).unwrap_or_default(),
                gstack_install_id: row.get(14)?,
                gstack_install_observed_path: row.get(15)?,
                effective_gstack_version: row.get(16)?,
                effective_gstack_source: row.get(17)?,
                first_seen_at: row.get(18)?,
                last_seen_at: row.get(19)?,
            })
        })?;
        let mut projects = Vec::new();
        for row in rows {
            projects.push(row?);
        }
        Ok(projects)
    }

    pub fn find_project(&self, identifier: &str) -> Result<Option<CatalogProject>> {
        let project = if let Ok(id) = identifier.parse::<i64>() {
            self.list_projects()?
                .into_iter()
                .find(|project| project.id == id)
        } else {
            self.list_projects()?
                .into_iter()
                .find(|project| project.canonical_path == identifier || project.name == identifier)
        };
        Ok(project)
    }

    pub fn project_detail(&self, identifier: &str) -> Result<Option<ProjectDetail>> {
        let Some(project) = self.find_project(identifier)? else {
            return Ok(None);
        };
        let install = match project.gstack_install_id {
            Some(id) => self
                .list_installs(false, None, None)?
                .into_iter()
                .find(|install| install.id == id),
            None => None,
        };
        Ok(Some(ProjectDetail { project, install }))
    }

    pub fn latest_project_install_history(&self, project_id: i64) -> Result<Option<(i64, String)>> {
        let row: Option<(i64, String)> = self
            .conn
            .query_row(
                "SELECT gstack_install_id, summary_json
                 FROM project_observations
                 WHERE project_id = ?1 AND gstack_install_id IS NOT NULL
                 ORDER BY observed_at DESC
                 LIMIT 1",
                params![project_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((install_id, summary_json)) = row else {
            return Ok(None);
        };
        let summary: Value = serde_json::from_str(&summary_json).unwrap_or(Value::Null);
        let Some(path) = summary
            .get("gstack_install_observed_path")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
        else {
            return Ok(None);
        };
        Ok(Some((install_id, path)))
    }

    fn commit_note_row(&self, sha: &str) -> Result<Option<(CatalogCommitNote, Vec<String>)>> {
        self.conn
            .query_row(
                "SELECT sha, version, committed_at, subject, body, parents_json
                 FROM upstream_commits
                 WHERE sha = ?1",
                params![sha],
                |row| {
                    let parents_json: String = row.get(5)?;
                    let parents: Vec<String> =
                        serde_json::from_str(&parents_json).unwrap_or_default();
                    Ok((
                        CatalogCommitNote {
                            commit_sha: row.get(0)?,
                            version: row.get(1)?,
                            committed_at: row.get(2)?,
                            subject: row.get(3)?,
                            body: row.get(4)?,
                        },
                        parents,
                    ))
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn commit_note(&self, sha: &str) -> Result<Option<CatalogCommitNote>> {
        Ok(self.commit_note_row(sha)?.map(|(note, _)| note))
    }

    fn walk_first_parent_path(
        &self,
        start_sha: &str,
        stop_sha: &str,
        max_commits: usize,
    ) -> Result<(Vec<CatalogCommitNote>, bool)> {
        let mut commits = Vec::new();
        let mut current = Some(start_sha.to_string());
        let mut reached_stop = false;

        while let Some(sha) = current {
            if commits.len() >= max_commits {
                break;
            }
            let Some((note, parents)) = self.commit_note_row(&sha)? else {
                break;
            };
            if note.commit_sha == stop_sha {
                reached_stop = true;
                break;
            }
            current = parents.first().cloned();
            commits.push(note);
        }

        Ok((commits, reached_stop))
    }

    fn diff_commit_files(&self, from_sha: Option<&str>, to_sha: &str) -> Result<CatalogCommitDiff> {
        let from_files = if let Some(from_sha) = from_sha {
            self.commit_files(from_sha)?
        } else {
            Vec::new()
        };
        let to_files = self.commit_files(to_sha)?;

        let from_map = from_files
            .iter()
            .map(|file| (file.path.clone(), file))
            .collect::<std::collections::HashMap<_, _>>();
        let to_map = to_files
            .iter()
            .map(|file| (file.path.clone(), file))
            .collect::<std::collections::HashMap<_, _>>();

        let mut added_paths = Vec::new();
        let mut updated_paths = Vec::new();
        let mut removed_paths = Vec::new();

        for (path, target) in &to_map {
            match from_map.get(path) {
                None => added_paths.push(path.clone()),
                Some(source)
                    if source.blob_sha != target.blob_sha || source.mode != target.mode =>
                {
                    updated_paths.push(path.clone())
                }
                Some(_) => {}
            }
        }

        for path in from_map.keys() {
            if !to_map.contains_key(path) {
                removed_paths.push(path.clone());
            }
        }

        added_paths.sort();
        updated_paths.sort();
        removed_paths.sort();

        Ok(CatalogCommitDiff {
            added_paths,
            updated_paths,
            removed_paths,
        })
    }

    pub fn version_context(
        &self,
        current_commit_sha: Option<&str>,
        target_commit_sha: &str,
        max_commits: usize,
    ) -> Result<Option<CatalogVersionContext>> {
        let Some(selected) = self.commit_note(target_commit_sha)? else {
            return Ok(None);
        };

        let (direction, path_commits) = match current_commit_sha {
            None => ("preview".to_string(), vec![selected.clone()]),
            Some(current_sha) if current_sha == target_commit_sha => {
                ("current".to_string(), vec![])
            }
            Some(current_sha) => {
                let (mut upgrade_path, reached_current) =
                    self.walk_first_parent_path(target_commit_sha, current_sha, max_commits)?;
                if reached_current {
                    upgrade_path.reverse();
                    ("upgrade".to_string(), upgrade_path)
                } else {
                    let (mut downgrade_path, reached_target) =
                        self.walk_first_parent_path(current_sha, target_commit_sha, max_commits)?;
                    if reached_target {
                        downgrade_path.reverse();
                        ("downgrade".to_string(), downgrade_path)
                    } else {
                        ("unrelated".to_string(), vec![selected.clone()])
                    }
                }
            }
        };

        let diff = self.diff_commit_files(current_commit_sha, target_commit_sha)?;

        Ok(Some(CatalogVersionContext {
            selected,
            direction,
            path_commits,
            diff,
        }))
    }

    pub fn list_versions(&self, search: Option<&str>) -> Result<Vec<CatalogVersion>> {
        let like = search.map(|value| format!("%{value}%"));
        let sql = if like.is_some() {
            "SELECT version, sha, committed_at, subject, body
             FROM (
                SELECT
                    version,
                    sha,
                    committed_at,
                    subject,
                    body,
                    ROW_NUMBER() OVER (
                        PARTITION BY version
                        ORDER BY committed_at DESC, sha DESC
                    ) AS row_number
                FROM upstream_commits
                WHERE version IS NOT NULL
                  AND (version LIKE ?1 OR sha LIKE ?1 OR subject LIKE ?1)
             )
             WHERE row_number = 1
             ORDER BY committed_at DESC"
        } else {
            "SELECT version, sha, committed_at, subject, body
             FROM (
                SELECT
                    version,
                    sha,
                    committed_at,
                    subject,
                    body,
                    ROW_NUMBER() OVER (
                        PARTITION BY version
                        ORDER BY committed_at DESC, sha DESC
                    ) AS row_number
                FROM upstream_commits
                WHERE version IS NOT NULL
             )
             WHERE row_number = 1
             ORDER BY committed_at DESC"
        };
        let mut statement = self.conn.prepare(sql)?;
        let mapper = |row: &rusqlite::Row<'_>| {
            Ok(CatalogVersion {
                version: row.get(0)?,
                commit_sha: row.get(1)?,
                committed_at: row.get(2)?,
                subject: row.get(3)?,
                body: row.get(4)?,
            })
        };
        let rows = if let Some(like) = like {
            statement.query_map(params![like], mapper)?
        } else {
            statement.query_map([], mapper)?
        };
        let mut versions = Vec::new();
        for row in rows {
            versions.push(row?);
        }
        Ok(versions)
    }

    pub fn record_sync_event(
        &self,
        install_id: i64,
        commit_sha: &str,
        version: Option<&str>,
        dry_run: bool,
        changed_files: &[String],
        backup_path: Option<&str>,
        status: &str,
        details: &Value,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sync_events (
                install_id, commit_sha, version, created_at, dry_run, changed_files_json, backup_path, status, details_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                install_id,
                commit_sha,
                version,
                now_iso(),
                bool_to_sql(dry_run),
                serde_json::to_string(changed_files)?,
                backup_path,
                status,
                serde_json::to_string(details)?
            ],
        )?;
        Ok(())
    }

    pub fn app_setting(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .with_context(|| format!("failed to read app setting {key}"))
    }

    pub fn set_app_setting(&self, key: &str, value: Option<&str>) -> Result<()> {
        match value {
            Some(value) => {
                self.conn.execute(
                    "INSERT INTO app_settings (key, value, updated_at)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                    params![key, value, now_iso()],
                )?;
            }
            None => {
                self.conn
                    .execute("DELETE FROM app_settings WHERE key = ?1", params![key])?;
            }
        }
        Ok(())
    }

    pub fn summary(&self) -> Result<CatalogSummary> {
        let source = self.source_state()?;
        let total_installs: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM local_installs", [], |row| row.get(0))?;
        let outdated_installs: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM local_installs WHERE is_outdated = 1",
            [],
            |row| row.get(0),
        )?;
        let git_backed_installs: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM local_installs WHERE has_git = 1",
            [],
            |row| row.get(0),
        )?;
        let total_projects: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))?;
        let projects_with_local_gstack: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM projects WHERE effective_gstack_source = 'local_install'",
            [],
            |row| row.get(0),
        )?;
        let mut statement = self
            .conn
            .prepare("SELECT install_type, COUNT(*) FROM local_installs GROUP BY install_type ORDER BY install_type")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        let mut by_type = Vec::new();
        for row in rows {
            by_type.push(row?);
        }
        let last_scan_at: Option<String> = self
            .conn
            .query_row(
                "SELECT finished_at FROM scan_runs ORDER BY finished_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(CatalogSummary {
            source,
            total_installs,
            outdated_installs,
            git_backed_installs,
            total_projects,
            projects_with_local_gstack,
            by_type,
            last_scan_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Catalog;
    use crate::util::TempWorkdir;

    #[test]
    fn app_settings_round_trip() {
        let temp = TempWorkdir::new("gstack-hypervisor-db-test").expect("temp dir");
        let db_path = temp.path().join("catalog.sqlite");
        let catalog = Catalog::new(&db_path).expect("catalog");

        assert_eq!(catalog.app_setting("tui.theme_id").expect("read"), None);

        catalog
            .set_app_setting("tui.theme_id", Some("shoreditch_neon"))
            .expect("write");
        assert_eq!(
            catalog.app_setting("tui.theme_id").expect("read"),
            Some("shoreditch_neon".to_string())
        );

        catalog
            .set_app_setting("tui.theme_id", None)
            .expect("delete");
        assert_eq!(catalog.app_setting("tui.theme_id").expect("read"), None);
    }
}
