use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config::{
    DEFAULT_MAX_DEPTH, LOCAL_INSTALL_RELATIVE_PATHS, REPO_SCAN_SKIP_DIRS, known_install_locations,
};
use crate::db::Catalog;
use crate::git;
use crate::ingest::ingest_upstream;
use crate::manifest::{collect_local_manifest, manifest_hash};
use crate::models::{
    DiscoveredInstall, DiscoveredProject, DiscoveredRepo, HostKind, InstallType, ScanResult,
};
use crate::util::{compare_versions, now_iso, read_trimmed_file, real_path_or_original};

fn detect_host(path: &Path) -> HostKind {
    let value = path.to_string_lossy().replace('\\', "/");
    if value.contains("/.claude/") {
        HostKind::Claude
    } else if value.contains("/.agents/") || value.contains("/.codex/") {
        HostKind::Codex
    } else {
        HostKind::Unknown
    }
}

fn classify_install_type(repository_path: Option<&Path>, has_git: bool) -> InstallType {
    match (repository_path.is_some(), has_git) {
        (true, true) => InstallType::RepoGit,
        (true, false) => InstallType::RepoMaterialized,
        (false, true) => InstallType::GlobalGit,
        (false, false) => InstallType::GlobalMaterialized,
    }
}

fn unique_existing_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut output = Vec::new();
    for path in paths {
        if !path.exists() {
            continue;
        }
        let canonical = real_path_or_original(path);
        if seen.insert(canonical.clone()) {
            output.push(canonical);
        }
    }
    output
}

fn has_project_markers(path: &Path) -> bool {
    path.join("CLAUDE.md").exists()
        || path.join(".claude").exists()
        || path.join(".agents").exists()
        || LOCAL_INSTALL_RELATIVE_PATHS
            .iter()
            .any(|relative| path.join(relative).exists())
}

fn find_project_roots(roots: &[PathBuf], max_depth: usize) -> Vec<PathBuf> {
    let mut projects = HashSet::new();
    let mut seen = HashSet::new();
    let mut queue = VecDeque::new();
    for root in unique_existing_paths(roots) {
        queue.push_back((root, 0usize));
    }

    while let Some((path, depth)) = queue.pop_front() {
        let canonical = real_path_or_original(&path);
        if !seen.insert(canonical.clone()) {
            continue;
        }
        let is_git_repo = canonical.join(".git").exists();
        if is_git_repo || has_project_markers(&canonical) {
            projects.insert(canonical.clone());
        }
        if is_git_repo || depth >= max_depth {
            continue;
        }
        let Ok(entries) = fs::read_dir(&canonical) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if name.starts_with('.') || REPO_SCAN_SKIP_DIRS.contains(&name) {
                continue;
            }
            queue.push_back((path, depth + 1));
        }
    }

    let mut projects = projects.into_iter().collect::<Vec<_>>();
    projects.sort();
    projects
}

fn inspect_repo(path: &Path) -> DiscoveredRepo {
    DiscoveredRepo {
        canonical_path: path.to_string_lossy().to_string(),
        name: path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("repo")
            .to_string(),
        git_remote: git::remote_origin(path),
    }
}

fn infer_project_root(install_path: &Path) -> Option<PathBuf> {
    let normalized = install_path.to_string_lossy().replace('\\', "/");
    for suffix in LOCAL_INSTALL_RELATIVE_PATHS {
        let needle = format!("/{suffix}");
        if let Some(prefix) = normalized.strip_suffix(&needle) {
            let candidate = PathBuf::from(prefix);
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
    }
    None
}

fn discover_project(
    project_path: &Path,
    installs: &[DiscoveredInstall],
    global_claude_install: Option<&DiscoveredInstall>,
    global_codex_install: Option<&DiscoveredInstall>,
) -> Option<DiscoveredProject> {
    let claude_md = project_path.join("CLAUDE.md");
    let claude_dir = project_path.join(".claude");
    let settings_candidates = [
        project_path.join(".claude").join("settings.json"),
        project_path.join(".claude").join("settings.local.json"),
        project_path.join(".claude").join("settings.yaml"),
        project_path.join(".claude").join("settings.yml"),
    ];
    let settings_paths = settings_candidates
        .iter()
        .filter(|path| path.exists())
        .map(|path| path.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    let has_claude_md = claude_md.exists();
    let has_claude_dir = claude_dir.exists();
    let has_claude_settings = !settings_paths.is_empty();

    if !has_claude_md && !has_claude_dir && !has_claude_settings {
        return None;
    }

    let project_path_string = project_path.to_string_lossy().to_string();
    let local_install = installs
        .iter()
        .filter(|install| install.repository_path.as_deref() == Some(project_path_string.as_str()))
        .min_by_key(|install| match install.host {
            HostKind::Claude => 0usize,
            HostKind::Codex => 1usize,
            HostKind::Unknown => 2usize,
        });

    let (effective_version, effective_source, local_install_path) =
        if let Some(local_install) = local_install {
            (
                local_install.local_version.clone(),
                "local_install".to_string(),
                Some(local_install.observed_path.clone()),
            )
        } else if let Some(global_install) = global_claude_install.or(global_codex_install) {
            (
                global_install.local_version.clone(),
                "global_install".to_string(),
                None,
            )
        } else {
            (None, "none".to_string(), None)
        };

    Some(DiscoveredProject {
        canonical_path: project_path_string,
        name: project_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("project")
            .to_string(),
        git_remote: git::remote_origin(project_path),
        has_claude_md,
        has_claude_dir,
        has_claude_settings,
        claude_settings_paths: settings_paths,
        gstack_install_observed_path: local_install_path,
        effective_gstack_version: effective_version,
        effective_gstack_source: effective_source,
    })
}

fn inspect_install(
    catalog: &Catalog,
    path: &Path,
    repository_path: Option<&Path>,
) -> Result<DiscoveredInstall> {
    let metadata = fs::symlink_metadata(path)?;
    let resolved_path = real_path_or_original(path);
    let has_git = path.join(".git").exists();
    let local_version = read_trimmed_file(&path.join("VERSION"));
    let browse_commit = read_trimmed_file(&path.join("browse").join("dist").join(".version"));
    let manifest = collect_local_manifest(path)?;
    let local_manifest_hash = if manifest.is_empty() {
        None
    } else {
        Some(manifest_hash(
            &manifest
                .iter()
                .map(|entry| {
                    (
                        entry.path.clone(),
                        entry.blob_sha.clone(),
                        entry.mode.clone(),
                    )
                })
                .collect::<Vec<_>>(),
        ))
    };
    let local_commit = if has_git { git::head(path) } else { None };
    let origin_url = if has_git {
        git::remote_origin(path)
    } else {
        None
    };
    let branch = if has_git {
        git::current_branch(path)
    } else {
        None
    };
    let dirty = if has_git { git::is_dirty(path) } else { false };
    let matched = catalog.match_upstream_commit(
        local_manifest_hash.as_deref(),
        local_commit.as_deref(),
        browse_commit.as_deref(),
        local_version.as_deref(),
    )?;
    let source = catalog.source_state()?;
    let exact_head_match = matched
        .as_ref()
        .and_then(|(sha, _)| source.head_commit_sha.as_ref().map(|head| sha == head))
        .unwrap_or(false);
    let is_outdated = if exact_head_match {
        Some(false)
    } else if let Some(ordering) =
        compare_versions(local_version.as_deref(), source.head_version.as_deref())
    {
        Some(ordering == std::cmp::Ordering::Less)
    } else if let (Some((sha, _)), Some(head)) = (matched.as_ref(), source.head_commit_sha.as_ref())
    {
        Some(sha != head)
    } else {
        None
    };

    Ok(DiscoveredInstall {
        observed_path: path.to_string_lossy().to_string(),
        resolved_path: resolved_path.to_string_lossy().to_string(),
        repository_path: repository_path.map(|value| value.to_string_lossy().to_string()),
        host: detect_host(path),
        install_type: classify_install_type(repository_path, has_git),
        is_symlink: metadata.file_type().is_symlink(),
        has_git,
        local_version,
        local_commit,
        browse_commit,
        manifest_hash: local_manifest_hash,
        origin_url,
        branch,
        dirty,
        matched_upstream_commit_sha: matched.as_ref().map(|(sha, _)| sha.clone()),
        matched_upstream_version: matched.and_then(|(_, version)| version),
        is_outdated,
    })
}

pub fn scan_local_installs(
    catalog: &Catalog,
    roots: &[PathBuf],
    max_depth: Option<usize>,
) -> Result<ScanResult> {
    let started_at = now_iso();
    let max_depth = max_depth.unwrap_or(DEFAULT_MAX_DEPTH);
    let roots = unique_existing_paths(roots);
    let project_roots = find_project_roots(&roots, max_depth);
    let repositories = project_roots
        .iter()
        .filter(|path| path.join(".git").exists())
        .map(|repo| inspect_repo(repo))
        .collect::<Vec<_>>();

    let mut install_paths = HashSet::new();
    for project_root in &project_roots {
        for relative in LOCAL_INSTALL_RELATIVE_PATHS {
            let candidate = project_root.join(relative);
            if candidate.exists() {
                install_paths.insert(candidate);
            }
        }
    }
    for candidate in known_install_locations() {
        if candidate.exists() {
            install_paths.insert(candidate);
        }
    }

    let mut installs = Vec::new();
    let mut paths = install_paths.into_iter().collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        let repository_path = infer_project_root(&path);
        installs.push(inspect_install(catalog, &path, repository_path.as_deref())?);
    }
    let global_claude_install = installs.iter().find(|install| {
        install.repository_path.is_none() && matches!(install.host, HostKind::Claude)
    });
    let global_codex_install = installs.iter().find(|install| {
        install.repository_path.is_none() && matches!(install.host, HostKind::Codex)
    });
    let projects = project_roots
        .iter()
        .filter_map(|project| {
            discover_project(
                project,
                &installs,
                global_claude_install,
                global_codex_install,
            )
        })
        .collect::<Vec<_>>();

    let source = catalog.source_state()?;
    Ok(ScanResult {
        started_at,
        finished_at: now_iso(),
        roots: roots
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect(),
        max_depth,
        source_head_sha: source.head_commit_sha,
        source_head_version: source.head_version,
        repositories,
        projects,
        installs,
    })
}

pub fn scan_specific_paths(catalog: &Catalog, install_paths: &[PathBuf]) -> Result<ScanResult> {
    let started_at = now_iso();
    let mut repositories = Vec::new();
    let mut project_roots = Vec::new();
    let mut seen_repos = HashSet::new();
    let mut seen_project_roots = HashSet::new();
    let mut installs = Vec::new();
    for path in unique_existing_paths(install_paths) {
        let repository_path = infer_project_root(&path);
        if let Some(project_path) = repository_path.as_ref() {
            if seen_project_roots.insert(project_path.clone()) {
                project_roots.push(project_path.clone());
            }
            if project_path.join(".git").exists() && seen_repos.insert(project_path.clone()) {
                repositories.push(inspect_repo(project_path));
            }
        }
        installs.push(inspect_install(catalog, &path, repository_path.as_deref())?);
    }
    let global_claude_install = installs.iter().find(|install| {
        install.repository_path.is_none() && matches!(install.host, HostKind::Claude)
    });
    let global_codex_install = installs.iter().find(|install| {
        install.repository_path.is_none() && matches!(install.host, HostKind::Codex)
    });
    let projects = project_roots
        .iter()
        .filter_map(|project| {
            discover_project(
                Path::new(project),
                &installs,
                global_claude_install,
                global_codex_install,
            )
        })
        .collect::<Vec<_>>();
    let source = catalog.source_state()?;
    Ok(ScanResult {
        started_at,
        finished_at: now_iso(),
        roots: install_paths
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect(),
        max_depth: 0,
        source_head_sha: source.head_commit_sha,
        source_head_version: source.head_version,
        repositories,
        projects,
        installs,
    })
}

pub fn sync_catalog(
    catalog: &Catalog,
    roots: &[PathBuf],
    max_depth: Option<usize>,
) -> Result<ScanResult> {
    ingest_upstream(catalog, None, None)?;
    let scan = scan_local_installs(catalog, roots, max_depth)?;
    catalog.record_scan(&scan)?;
    Ok(scan)
}
