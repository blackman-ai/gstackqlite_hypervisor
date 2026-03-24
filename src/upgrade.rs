use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::json;

use crate::config::backup_root;
use crate::db::Catalog;
use crate::ingest::hydrate_commit_by_sha;
use crate::manifest::{LocalManifestEntry, LocalManifestKind, collect_local_manifest};
use crate::models::{
    ApplyResult, CatalogInstall, CommitSnapshotFile, DiffPreviewFile, ProjectDiffPreview,
    RemoveResult, RevertResult, SyncChangeSet, SyncResult,
};
use crate::scan::scan_specific_paths;
use crate::util::{ensure_dir, timestamp_slug};

fn ensure_commit_files_available(
    catalog: &Catalog,
    commit_sha: &str,
) -> Result<Vec<CommitSnapshotFile>> {
    let mut files = catalog.commit_files(commit_sha)?;
    if files.iter().any(|file| file.content.is_none()) {
        hydrate_commit_by_sha(catalog, commit_sha)?;
        files = catalog.commit_files(commit_sha)?;
    }
    Ok(files)
}

fn read_local_entry_content(root: &Path, entry: &LocalManifestEntry) -> Result<Vec<u8>> {
    match &entry.kind {
        LocalManifestKind::File => Ok(fs::read(root.join(&entry.path))?),
        LocalManifestKind::Symlink(target) => Ok(target.as_bytes().to_vec()),
    }
}

fn change_set(
    source_files: &[CommitSnapshotFile],
    target_entries: &[LocalManifestEntry],
) -> SyncChangeSet {
    let source_map: HashMap<_, _> = source_files
        .iter()
        .map(|entry| (entry.path.clone(), entry))
        .collect();
    let target_map: HashMap<_, _> = target_entries
        .iter()
        .map(|entry| (entry.path.clone(), entry))
        .collect();
    let mut added = Vec::new();
    let mut updated = Vec::new();
    let mut removed = Vec::new();
    let mut unchanged_count = 0usize;

    for (path, source) in &source_map {
        match target_map.get(path) {
            None => added.push(path.clone()),
            Some(target) if target.blob_sha != source.blob_sha || target.mode != source.mode => {
                updated.push(path.clone())
            }
            Some(_) => unchanged_count += 1,
        }
    }

    for path in target_map.keys() {
        if !source_map.contains_key(path) {
            removed.push(path.clone());
        }
    }

    SyncChangeSet {
        added,
        updated,
        removed,
        unchanged_count,
    }
}

fn copy_manifest_to(root: &Path, entries: &[LocalManifestEntry], destination: &Path) -> Result<()> {
    ensure_dir(destination)?;
    for entry in entries {
        let src = root.join(&entry.path);
        let dst = destination.join(&entry.path);
        if let Some(parent) = dst.parent() {
            ensure_dir(parent)?;
        }
        match &entry.kind {
            LocalManifestKind::File => {
                fs::copy(&src, &dst)
                    .with_context(|| format!("failed to back up {}", src.display()))?;
            }
            LocalManifestKind::Symlink(target) => {
                #[cfg(unix)]
                {
                    std::os::unix::fs::symlink(target, &dst).with_context(|| {
                        format!("failed to create backup symlink {}", dst.display())
                    })?;
                }
                #[cfg(not(unix))]
                {
                    bail!("symlink backup is only implemented on unix targets");
                }
            }
        }
    }
    Ok(())
}

fn remove_path_if_exists(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn prune_empty_dirs(root: &Path) -> Result<()> {
    let mut dirs = Vec::new();
    for entry in walkdir::WalkDir::new(root).min_depth(1) {
        let entry = entry?;
        if entry.file_type().is_dir() {
            dirs.push(entry.into_path());
        }
    }
    dirs.sort_by(|left, right| right.components().count().cmp(&left.components().count()));
    for dir in dirs {
        if fs::read_dir(&dir)?.next().is_none() {
            let _ = fs::remove_dir(&dir);
        }
    }
    Ok(())
}

fn write_bytes_with_mode(destination: &Path, mode: &str, content: &[u8]) -> Result<()> {
    if let Some(parent) = destination.parent() {
        ensure_dir(parent)?;
    }
    let _ = remove_path_if_exists(destination);
    if mode == "120000" {
        let target = String::from_utf8_lossy(content).to_string();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, destination)?;
        }
        #[cfg(not(unix))]
        {
            bail!("symlink materialization is only implemented on unix targets");
        }
    } else {
        fs::write(destination, content)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = if mode == "100755" { 0o755 } else { 0o644 };
            fs::set_permissions(destination, fs::Permissions::from_mode(mode))?;
        }
    }
    Ok(())
}

fn write_snapshot_file(destination: &Path, source_file: &CommitSnapshotFile) -> Result<()> {
    let Some(content) = source_file.content.as_ref() else {
        bail!("missing blob content for {}", source_file.path);
    };
    write_bytes_with_mode(destination, &source_file.mode, content)
}

fn apply_snapshot(
    target_path: &Path,
    source_files: &[CommitSnapshotFile],
    changes: &SyncChangeSet,
) -> Result<()> {
    let files_to_write = changes
        .added
        .iter()
        .chain(&changes.updated)
        .cloned()
        .collect::<BTreeSet<_>>();

    for relative in &changes.removed {
        remove_path_if_exists(&target_path.join(relative))?;
    }

    for source_file in source_files {
        if !files_to_write.contains(&source_file.path) {
            continue;
        }
        let destination = target_path.join(&source_file.path);
        write_snapshot_file(&destination, source_file)?;
    }

    prune_empty_dirs(target_path)?;
    Ok(())
}

pub fn materialize_targets(
    catalog: &Catalog,
    commit_sha: Option<&str>,
    version: Option<&str>,
    target_identifiers: &[String],
    outdated_only: bool,
    dry_run: bool,
    allow_git_targets: bool,
) -> Result<Vec<SyncResult>> {
    let Some((commit_sha, version)) = catalog.resolve_commit_ref(commit_sha, version)? else {
        bail!("no upstream commit is available in the catalog");
    };

    let source_files = ensure_commit_files_available(catalog, &commit_sha)?;

    let installs = catalog.list_installs(false, None, None)?;
    let targets = if !target_identifiers.is_empty() {
        installs
            .into_iter()
            .filter(|install| {
                target_identifiers.iter().any(|identifier| {
                    identifier == &install.id.to_string()
                        || identifier == &install.observed_path
                        || identifier == &install.resolved_path
                })
            })
            .collect::<Vec<_>>()
    } else if outdated_only {
        installs
            .into_iter()
            .filter(|install| install.is_outdated == Some(true))
            .collect::<Vec<_>>()
    } else {
        installs
    };

    let mut results = Vec::new();
    ensure_dir(&backup_root())?;

    for target in targets {
        if target.has_git && !allow_git_targets {
            continue;
        }
        let target_path = PathBuf::from(&target.observed_path);
        let target_entries = collect_local_manifest(&target_path)?;
        let changes = change_set(&source_files, &target_entries);
        let changed_files = changes
            .added
            .iter()
            .chain(&changes.updated)
            .chain(&changes.removed)
            .cloned()
            .collect::<Vec<_>>();
        let backup_path = if dry_run || changed_files.is_empty() {
            None
        } else {
            let path =
                backup_root().join(format!("{}-{}-{}", timestamp_slug(), target.id, "gstack"));
            copy_manifest_to(&target_path, &target_entries, &path)?;
            apply_snapshot(&target_path, &source_files, &changes)?;
            Some(path.to_string_lossy().to_string())
        };

        let status = if dry_run { "dry_run" } else { "applied" };
        catalog.record_sync_event(
            target.id,
            &commit_sha,
            version.as_deref(),
            dry_run,
            &changed_files,
            backup_path.as_deref(),
            status,
            &json!({
                "added": changes.added.len(),
                "updated": changes.updated.len(),
                "removed": changes.removed.len(),
                "unchanged": changes.unchanged_count
            }),
        )?;
        results.push(SyncResult {
            target,
            commit_sha: commit_sha.clone(),
            version: version.clone(),
            dry_run,
            changes,
            backup_path,
            status: status.to_string(),
        });
    }

    if !dry_run && !results.is_empty() {
        let paths = results
            .iter()
            .map(|result| PathBuf::from(&result.target.observed_path))
            .collect::<Vec<_>>();
        let scan = scan_specific_paths(catalog, &paths)?;
        catalog.record_scan(&scan)?;
    }

    Ok(results)
}

fn local_matches_commit(local: &LocalManifestEntry, commit: &CommitSnapshotFile) -> bool {
    local.blob_sha == commit.blob_sha && local.mode == commit.mode
}

fn can_merge_text(local: &LocalManifestEntry, target: &CommitSnapshotFile) -> bool {
    matches!(local.kind, LocalManifestKind::File) && target.mode != "120000"
}

fn merge_text_conflict(local: &[u8], target: &[u8], target_label: &str) -> Option<Vec<u8>> {
    let local_text = String::from_utf8(local.to_vec()).ok()?;
    let target_text = String::from_utf8(target.to_vec()).ok()?;
    Some(
        format!(
            "<<<<<<< local customization\n{local_text}\n=======\n{target_text}\n>>>>>>> {target_label}\n"
        )
        .into_bytes(),
    )
}

fn preview_text(bytes: &[u8]) -> Option<String> {
    String::from_utf8(bytes.to_vec()).ok()
}

fn compress_diff_ops(ops: Vec<(char, String)>) -> Vec<String> {
    let mut output = Vec::new();
    let mut index = 0usize;
    while index < ops.len() {
        let prefix = ops[index].0;
        let mut end = index + 1;
        while end < ops.len() && ops[end].0 == prefix {
            end += 1;
        }
        let run = &ops[index..end];
        if prefix == ' ' && run.len() > 2 {
            output.push(format!("  {}", run[0].1));
            output.push(format!("  ... {} unchanged line(s) ...", run.len() - 2));
            output.push(format!("  {}", run[run.len() - 1].1));
        } else {
            for (_, line) in run {
                output.push(format!("{prefix} {line}"));
            }
        }
        index = end;
    }
    output
}

fn text_diff_preview(old_text: &str, new_text: &str, max_lines: usize) -> (Vec<String>, bool) {
    let old_lines = old_text.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    let new_lines = new_text.lines().map(ToOwned::to_owned).collect::<Vec<_>>();

    if old_lines.len().saturating_mul(new_lines.len()) > 40_000 {
        let mut lines = vec![
            format!("- {}", old_lines.first().cloned().unwrap_or_default()),
            format!("+ {}", new_lines.first().cloned().unwrap_or_default()),
        ];
        let truncated = lines.len() > max_lines;
        lines.truncate(max_lines);
        return (lines, truncated);
    }

    let old_len = old_lines.len();
    let new_len = new_lines.len();
    let mut lcs = vec![vec![0usize; new_len + 1]; old_len + 1];
    for old_index in (0..old_len).rev() {
        for new_index in (0..new_len).rev() {
            lcs[old_index][new_index] = if old_lines[old_index] == new_lines[new_index] {
                lcs[old_index + 1][new_index + 1] + 1
            } else {
                lcs[old_index + 1][new_index].max(lcs[old_index][new_index + 1])
            };
        }
    }

    let mut old_index = 0usize;
    let mut new_index = 0usize;
    let mut ops = Vec::new();
    while old_index < old_len && new_index < new_len {
        if old_lines[old_index] == new_lines[new_index] {
            ops.push((' ', old_lines[old_index].clone()));
            old_index += 1;
            new_index += 1;
        } else if lcs[old_index + 1][new_index] >= lcs[old_index][new_index + 1] {
            ops.push(('-', old_lines[old_index].clone()));
            old_index += 1;
        } else {
            ops.push(('+', new_lines[new_index].clone()));
            new_index += 1;
        }
    }
    while old_index < old_len {
        ops.push(('-', old_lines[old_index].clone()));
        old_index += 1;
    }
    while new_index < new_len {
        ops.push(('+', new_lines[new_index].clone()));
        new_index += 1;
    }

    let mut preview = compress_diff_ops(ops);
    let truncated = preview.len() > max_lines;
    preview.truncate(max_lines);
    (preview, truncated)
}

fn build_diff_preview_file(
    path: &str,
    change_type: &str,
    old_label: &str,
    new_label: &str,
    old_bytes: Option<&[u8]>,
    new_bytes: Option<&[u8]>,
    max_lines: usize,
) -> DiffPreviewFile {
    let mut preview_lines = vec![format!("--- {old_label}"), format!("+++ {new_label}")];

    let (mut body, truncated) = match (old_bytes, new_bytes) {
        (Some(old_bytes), Some(new_bytes)) => {
            match (preview_text(old_bytes), preview_text(new_bytes)) {
                (Some(old_text), Some(new_text)) => {
                    text_diff_preview(&old_text, &new_text, max_lines.saturating_sub(2))
                }
                _ => (
                    vec!["(binary or symlink content omitted)".to_string()],
                    false,
                ),
            }
        }
        (None, Some(new_bytes)) => match preview_text(new_bytes) {
            Some(new_text) => {
                let mut lines = new_text
                    .lines()
                    .map(|line| format!("+ {line}"))
                    .collect::<Vec<_>>();
                let truncated = lines.len() > max_lines.saturating_sub(2);
                lines.truncate(max_lines.saturating_sub(2));
                (lines, truncated)
            }
            None => (
                vec!["(binary or symlink content omitted)".to_string()],
                false,
            ),
        },
        (Some(old_bytes), None) => match preview_text(old_bytes) {
            Some(old_text) => {
                let mut lines = old_text
                    .lines()
                    .map(|line| format!("- {line}"))
                    .collect::<Vec<_>>();
                let truncated = lines.len() > max_lines.saturating_sub(2);
                lines.truncate(max_lines.saturating_sub(2));
                (lines, truncated)
            }
            None => (
                vec!["(binary or symlink content omitted)".to_string()],
                false,
            ),
        },
        (None, None) => (vec!["(no content available)".to_string()], false),
    };
    preview_lines.append(&mut body);

    DiffPreviewFile {
        path: path.to_string(),
        change_type: change_type.to_string(),
        old_label: old_label.to_string(),
        new_label: new_label.to_string(),
        preview_lines,
        truncated,
    }
}

fn resolve_projects(
    catalog: &Catalog,
    project_identifiers: &[String],
) -> Result<Vec<crate::models::CatalogProject>> {
    if project_identifiers.is_empty() {
        return catalog.list_projects();
    }

    let mut resolved = Vec::new();
    for identifier in project_identifiers {
        if let Some(project) = catalog.find_project(identifier)? {
            resolved.push(project);
        }
    }
    Ok(resolved)
}

fn build_install_map(catalog: &Catalog) -> Result<HashMap<i64, CatalogInstall>> {
    Ok(catalog
        .list_installs(false, None, None)?
        .into_iter()
        .map(|install| (install.id, install))
        .collect::<HashMap<_, _>>())
}

fn default_project_install_path(project: &crate::models::CatalogProject) -> PathBuf {
    let base = PathBuf::from(&project.canonical_path);
    if project.has_claude_dir || project.has_claude_md || project.has_claude_settings {
        return base.join(".claude").join("skills").join("gstack");
    }
    if project.has_codex_dir || project.has_codex_settings {
        return base.join(".codex").join("skills").join("gstack");
    }
    if project.has_agents_md || project.has_agents_dir {
        return base.join(".agents").join("skills").join("gstack");
    }
    base.join(".claude").join("skills").join("gstack")
}

fn resolve_project_install(
    catalog: &Catalog,
    project: &crate::models::CatalogProject,
    install_by_id: &HashMap<i64, CatalogInstall>,
) -> Result<(Option<CatalogInstall>, PathBuf, Option<i64>)> {
    if let Some(install) = project
        .gstack_install_id
        .and_then(|id| install_by_id.get(&id).cloned())
    {
        return Ok((
            Some(install.clone()),
            PathBuf::from(&install.observed_path),
            Some(install.id),
        ));
    }

    if let Some(path) = project.gstack_install_observed_path.as_ref() {
        return Ok((None, PathBuf::from(path), None));
    }

    if let Some((install_id, path)) = catalog.latest_project_install_history(project.id)? {
        let install = install_by_id.get(&install_id).cloned();
        return Ok((install, PathBuf::from(path), Some(install_id)));
    }

    Ok((None, default_project_install_path(project), None))
}

fn backup_manifest_snapshot(
    install_path: &Path,
    local_entries: &[LocalManifestEntry],
    label: &str,
) -> Result<Option<PathBuf>> {
    if local_entries.is_empty() {
        return Ok(None);
    }
    let backup = backup_root().join(format!("{}-{}", timestamp_slug(), label));
    copy_manifest_to(install_path, local_entries, &backup)?;
    Ok(Some(backup))
}

fn restore_manifest_snapshot(
    source_root: &Path,
    source_entries: &[LocalManifestEntry],
    target_root: &Path,
) -> Result<()> {
    let existing_entries = collect_local_manifest(target_root)?;
    let source_paths = source_entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect::<BTreeSet<_>>();

    for entry in existing_entries {
        if !source_paths.contains(&entry.path) {
            remove_path_if_exists(&target_root.join(&entry.path))?;
        }
    }

    if !source_entries.is_empty() {
        ensure_dir(target_root)?;
    }
    for entry in source_entries {
        let bytes = read_local_entry_content(source_root, entry)?;
        write_bytes_with_mode(&target_root.join(&entry.path), &entry.mode, &bytes)?;
    }

    if target_root.exists() {
        prune_empty_dirs(target_root)?;
    }
    Ok(())
}

fn record_project_scan(
    catalog: &Catalog,
    project_paths: &[String],
) -> Result<HashMap<String, CatalogInstall>> {
    let roots = project_paths.iter().map(PathBuf::from).collect::<Vec<_>>();
    let scan = crate::scan::scan_local_installs(catalog, &roots, Some(1))?;
    catalog.record_scan(&scan)?;
    Ok(catalog
        .list_installs(false, None, None)?
        .into_iter()
        .map(|install| (install.observed_path.clone(), install))
        .collect::<HashMap<_, _>>())
}

fn fallback_commit_ref(catalog: &Catalog) -> Result<(String, Option<String>)> {
    catalog
        .resolve_commit_ref(None, None)?
        .ok_or_else(|| anyhow!("no upstream commit is available in the catalog"))
}

pub fn project_diff_preview(
    catalog: &Catalog,
    project_identifier: &str,
    version: Option<&str>,
    commit_sha: Option<&str>,
    max_files: usize,
    max_lines: usize,
) -> Result<ProjectDiffPreview> {
    let Some((target_commit_sha, target_version)) =
        catalog.resolve_commit_ref(commit_sha, version)?
    else {
        bail!("no upstream commit is available in the catalog");
    };

    let target_files = ensure_commit_files_available(catalog, &target_commit_sha)?;
    let target_map = target_files
        .iter()
        .map(|file| (file.path.clone(), file.clone()))
        .collect::<HashMap<_, _>>();

    let projects = resolve_projects(catalog, &[project_identifier.to_string()])?;
    let Some(project) = projects.into_iter().next() else {
        bail!("project not found: {project_identifier}");
    };
    let install_by_id = build_install_map(catalog)?;
    let (_current_install, install_path, _) =
        resolve_project_install(catalog, &project, &install_by_id)?;
    let local_entries = collect_local_manifest(&install_path)?;
    let local_map = local_entries
        .iter()
        .map(|entry| (entry.path.clone(), entry.clone()))
        .collect::<HashMap<_, _>>();

    let mut all_paths = BTreeSet::new();
    for path in target_map.keys() {
        all_paths.insert(path.clone());
    }
    for path in local_map.keys() {
        all_paths.insert(path.clone());
    }

    let mut files = Vec::new();
    let mut added_count = 0usize;
    let mut updated_count = 0usize;
    let mut removed_count = 0usize;
    let target_label = target_version
        .clone()
        .unwrap_or_else(|| target_commit_sha.chars().take(12).collect::<String>());

    for path in all_paths {
        let local = local_map.get(&path);
        let target = target_map.get(&path);
        match (local, target) {
            (None, Some(target)) => {
                added_count += 1;
                if files.len() < max_files {
                    files.push(build_diff_preview_file(
                        &path,
                        "added",
                        "/dev/null",
                        &format!("gstack {target_label}"),
                        None,
                        target.content.as_deref(),
                        max_lines,
                    ));
                }
            }
            (Some(local), None) => {
                removed_count += 1;
                if files.len() < max_files {
                    let local_bytes = read_local_entry_content(&install_path, local)?;
                    files.push(build_diff_preview_file(
                        &path,
                        "removed",
                        "local",
                        "/dev/null",
                        Some(local_bytes.as_slice()),
                        None,
                        max_lines,
                    ));
                }
            }
            (Some(local), Some(target))
                if local.blob_sha != target.blob_sha || local.mode != target.mode =>
            {
                updated_count += 1;
                if files.len() < max_files {
                    let local_bytes = read_local_entry_content(&install_path, local)?;
                    files.push(build_diff_preview_file(
                        &path,
                        "updated",
                        "local",
                        &format!("gstack {target_label}"),
                        Some(local_bytes.as_slice()),
                        target.content.as_deref(),
                        max_lines,
                    ));
                }
            }
            _ => {}
        }
    }

    Ok(ProjectDiffPreview {
        project,
        install_path: install_path.to_string_lossy().to_string(),
        commit_sha: target_commit_sha,
        version: target_version,
        total_changed_files: added_count + updated_count + removed_count,
        added_count,
        updated_count,
        removed_count,
        files,
    })
}

pub fn apply_version_to_projects(
    catalog: &Catalog,
    version: Option<&str>,
    commit_sha: Option<&str>,
    project_identifiers: &[String],
    dry_run: bool,
) -> Result<Vec<ApplyResult>> {
    let Some((target_commit_sha, target_version)) =
        catalog.resolve_commit_ref(commit_sha, version)?
    else {
        bail!("no upstream commit is available in the catalog");
    };
    let target_files = ensure_commit_files_available(catalog, &target_commit_sha)?;
    let target_map = target_files
        .iter()
        .map(|file| (file.path.clone(), file.clone()))
        .collect::<HashMap<_, _>>();

    let projects = resolve_projects(catalog, project_identifiers)?;
    let install_by_id = build_install_map(catalog)?;
    let mut results = Vec::new();
    ensure_dir(&backup_root())?;

    for project in projects {
        let (current_install, install_path, _) =
            resolve_project_install(catalog, &project, &install_by_id)?;
        let base_files = if let Some(base_sha) = current_install
            .as_ref()
            .and_then(|install| install.matched_upstream_commit_sha.clone())
        {
            ensure_commit_files_available(catalog, &base_sha)?
                .into_iter()
                .map(|file| (file.path.clone(), file))
                .collect::<HashMap<_, _>>()
        } else {
            HashMap::new()
        };
        let local_entries = collect_local_manifest(&install_path)?;
        let local_map = local_entries
            .iter()
            .map(|entry| (entry.path.clone(), entry.clone()))
            .collect::<HashMap<_, _>>();
        if !dry_run && !target_files.is_empty() {
            ensure_dir(&install_path)?;
        }

        let mut all_paths = BTreeSet::new();
        for path in base_files.keys() {
            all_paths.insert(path.clone());
        }
        for path in target_map.keys() {
            all_paths.insert(path.clone());
        }
        for path in local_map.keys() {
            all_paths.insert(path.clone());
        }

        let mut applied_files = Vec::new();
        let mut preserved_local_files = Vec::new();
        let mut merged_files = Vec::new();
        let mut conflict_files = Vec::new();
        let mut removed_files = Vec::new();

        let backup_path = if dry_run {
            None
        } else {
            let backup = backup_root().join(format!("{}-project-{}", timestamp_slug(), project.id));
            copy_manifest_to(&install_path, &local_entries, &backup)?;
            Some(backup)
        };

        for path in all_paths {
            let local = local_map.get(&path);
            let base = base_files.get(&path);
            let target = target_map.get(&path);
            match (local, base, target) {
                (None, _, Some(target)) => {
                    applied_files.push(path.clone());
                    if !dry_run {
                        write_snapshot_file(&install_path.join(&path), target)?;
                    }
                }
                (Some(local), Some(base), None) => {
                    if local_matches_commit(local, base) {
                        removed_files.push(path.clone());
                        if !dry_run {
                            remove_path_if_exists(&install_path.join(&path))?;
                        }
                    } else {
                        preserved_local_files.push(path.clone());
                    }
                }
                (Some(_local), None, None) => {
                    preserved_local_files.push(path.clone());
                }
                (Some(local), Some(base), Some(target)) => {
                    if local_matches_commit(local, target) {
                        continue;
                    }
                    if local_matches_commit(local, base) {
                        applied_files.push(path.clone());
                        if !dry_run {
                            write_snapshot_file(&install_path.join(&path), target)?;
                        }
                    } else if base.blob_sha == target.blob_sha && base.mode == target.mode {
                        preserved_local_files.push(path.clone());
                    } else {
                        let local_bytes = read_local_entry_content(&install_path, local)?;
                        let target_bytes = target.content.as_deref().unwrap_or_default();
                        if can_merge_text(local, target) {
                            if let Some(merged) = merge_text_conflict(
                                &local_bytes,
                                target_bytes,
                                &format!(
                                    "gstack {}",
                                    target_version
                                        .clone()
                                        .unwrap_or_else(|| target_commit_sha.clone())
                                ),
                            ) {
                                merged_files.push(path.clone());
                                if !dry_run {
                                    write_bytes_with_mode(
                                        &install_path.join(&path),
                                        &target.mode,
                                        &merged,
                                    )?;
                                }
                                continue;
                            }
                        }
                        preserved_local_files.push(path.clone());
                        conflict_files.push(path.clone());
                        if !dry_run {
                            if let Some(backup) = backup_path.as_ref() {
                                let incoming =
                                    backup.join("incoming").join(format!("{path}.incoming"));
                                write_bytes_with_mode(&incoming, &target.mode, target_bytes)?;
                            }
                        }
                    }
                }
                (Some(local), None, Some(target)) => {
                    if local_matches_commit(local, target) {
                        continue;
                    }
                    let local_bytes = read_local_entry_content(&install_path, local)?;
                    let target_bytes = target.content.as_deref().unwrap_or_default();
                    if can_merge_text(local, target) {
                        if let Some(merged) = merge_text_conflict(
                            &local_bytes,
                            target_bytes,
                            &format!(
                                "gstack {}",
                                target_version
                                    .clone()
                                    .unwrap_or_else(|| target_commit_sha.clone())
                            ),
                        ) {
                            merged_files.push(path.clone());
                            if !dry_run {
                                write_bytes_with_mode(
                                    &install_path.join(&path),
                                    &target.mode,
                                    &merged,
                                )?;
                            }
                            continue;
                        }
                    }
                    preserved_local_files.push(path.clone());
                    conflict_files.push(path.clone());
                    if !dry_run {
                        if let Some(backup) = backup_path.as_ref() {
                            let incoming = backup.join("incoming").join(format!("{path}.incoming"));
                            write_bytes_with_mode(&incoming, &target.mode, target_bytes)?;
                        }
                    }
                }
                (None, Some(_), None) | (None, None, None) => {}
            }
        }

        if !dry_run && install_path.exists() {
            prune_empty_dirs(&install_path)?;
        }

        results.push(ApplyResult {
            project: project.clone(),
            install_path: install_path.to_string_lossy().to_string(),
            commit_sha: target_commit_sha.clone(),
            version: target_version.clone(),
            dry_run,
            applied_files,
            preserved_local_files,
            merged_files,
            conflict_files,
            removed_files,
            backup_path: backup_path.map(|path| path.to_string_lossy().to_string()),
            status: if dry_run {
                "dry_run".to_string()
            } else {
                "applied".to_string()
            },
        });
    }

    let install_id_by_path = if !dry_run && !results.is_empty() {
        record_project_scan(
            catalog,
            &results
                .iter()
                .map(|result| result.project.canonical_path.clone())
                .collect::<Vec<_>>(),
        )?
        .into_values()
        .map(|install| (install.observed_path.clone(), install.id))
        .collect::<HashMap<_, _>>()
    } else {
        install_by_id
            .values()
            .map(|install| (install.observed_path.clone(), install.id))
            .collect::<HashMap<_, _>>()
    };

    for result in &results {
        let Some(install_id) = install_id_by_path.get(&result.install_path).copied() else {
            continue;
        };
        let changed_files = result
            .applied_files
            .iter()
            .chain(&result.preserved_local_files)
            .chain(&result.merged_files)
            .chain(&result.conflict_files)
            .chain(&result.removed_files)
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        catalog.record_sync_event(
            install_id,
            &result.commit_sha,
            result.version.as_deref(),
            result.dry_run,
            &changed_files,
            result.backup_path.as_deref(),
            &result.status,
            &json!({
                "project_id": result.project.id,
                "project_path": result.project.canonical_path,
                "install_path": result.install_path,
                "applied_files": result.applied_files,
                "preserved_local_files": result.preserved_local_files,
                "merged_files": result.merged_files,
                "conflict_files": result.conflict_files,
                "removed_files": result.removed_files,
            }),
        )?;
    }

    Ok(results)
}

pub fn remove_projects(
    catalog: &Catalog,
    project_identifiers: &[String],
    dry_run: bool,
) -> Result<Vec<RemoveResult>> {
    let projects = resolve_projects(catalog, project_identifiers)?;
    let install_by_id = build_install_map(catalog)?;
    let mut results = Vec::new();
    ensure_dir(&backup_root())?;

    for project in projects {
        let (current_install, install_path, historical_install_id) =
            resolve_project_install(catalog, &project, &install_by_id)?;
        let local_entries = collect_local_manifest(&install_path)?;
        let removed_files = local_entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        let backup_path = if dry_run {
            None
        } else {
            backup_manifest_snapshot(
                &install_path,
                &local_entries,
                &format!("project-{}-remove", project.id),
            )?
        };

        if !dry_run && install_path.exists() {
            remove_path_if_exists(&install_path)?;
        }

        let status = if local_entries.is_empty() {
            "no_install"
        } else if dry_run {
            "dry_run"
        } else {
            "removed"
        };

        let install_id = current_install
            .as_ref()
            .map(|install| install.id)
            .or(historical_install_id);
        if let Some(install_id) = install_id {
            let (commit_sha, version) = current_install
                .as_ref()
                .and_then(|install| {
                    install
                        .matched_upstream_commit_sha
                        .clone()
                        .map(|sha| (sha, install.matched_upstream_version.clone()))
                })
                .map(Ok)
                .unwrap_or_else(|| fallback_commit_ref(catalog))?;
            let backup_string = backup_path
                .as_ref()
                .map(|path| path.to_string_lossy().to_string());
            catalog.record_sync_event(
                install_id,
                &commit_sha,
                version.as_deref(),
                dry_run,
                &removed_files,
                backup_string.as_deref(),
                status,
                &json!({
                    "project_id": project.id,
                    "project_path": project.canonical_path,
                    "install_path": install_path.to_string_lossy().to_string(),
                    "removed_files": removed_files,
                }),
            )?;
        }

        results.push(RemoveResult {
            project,
            install_path: install_path.to_string_lossy().to_string(),
            dry_run,
            removed_files,
            backup_path: backup_path.map(|path| path.to_string_lossy().to_string()),
            status: status.to_string(),
        });
    }

    if !dry_run && !results.is_empty() {
        let _ = record_project_scan(
            catalog,
            &results
                .iter()
                .map(|result| result.project.canonical_path.clone())
                .collect::<Vec<_>>(),
        )?;
    }

    Ok(results)
}

pub fn revert_projects(
    catalog: &Catalog,
    project_identifiers: &[String],
    event_id: Option<i64>,
    dry_run: bool,
) -> Result<Vec<RevertResult>> {
    if event_id.is_some() && project_identifiers.len() > 1 {
        bail!("specific backup event reverts only support one project at a time");
    }
    let projects = resolve_projects(catalog, project_identifiers)?;
    let install_by_id = build_install_map(catalog)?;
    let mut results = Vec::new();
    ensure_dir(&backup_root())?;

    for project in projects {
        let (_current_install, install_path, historical_install_id) =
            resolve_project_install(catalog, &project, &install_by_id)?;
        let install_id = project.gstack_install_id.or(historical_install_id);
        let Some(install_id) = install_id else {
            results.push(RevertResult {
                project,
                install_path: install_path.to_string_lossy().to_string(),
                restored_from_backup_path: None,
                dry_run,
                restored_files: Vec::new(),
                removed_files: Vec::new(),
                backup_path: None,
                status: "no_history".to_string(),
            });
            continue;
        };

        let Some(detail) = catalog.install_detail(&install_id.to_string())? else {
            results.push(RevertResult {
                project,
                install_path: install_path.to_string_lossy().to_string(),
                restored_from_backup_path: None,
                dry_run,
                restored_files: Vec::new(),
                removed_files: Vec::new(),
                backup_path: None,
                status: "no_history".to_string(),
            });
            continue;
        };
        let Some(source_event) = detail
            .sync_events
            .iter()
            .find(|event| event.backup_path.is_some() && event_id.is_none_or(|id| event.id == id))
            .cloned()
        else {
            results.push(RevertResult {
                project,
                install_path: install_path.to_string_lossy().to_string(),
                restored_from_backup_path: None,
                dry_run,
                restored_files: Vec::new(),
                removed_files: Vec::new(),
                backup_path: None,
                status: if event_id.is_some() {
                    "event_not_found".to_string()
                } else {
                    "no_backup".to_string()
                },
            });
            continue;
        };

        let source_backup_path = PathBuf::from(
            source_event
                .backup_path
                .clone()
                .expect("selected sync event must have a backup path"),
        );
        let source_entries = collect_local_manifest(&source_backup_path)?;
        let current_entries = collect_local_manifest(&install_path)?;
        let restored_files = source_entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        let source_paths = restored_files.iter().cloned().collect::<BTreeSet<_>>();
        let removed_files = current_entries
            .iter()
            .filter(|entry| !source_paths.contains(&entry.path))
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        let backup_path = if dry_run {
            None
        } else {
            backup_manifest_snapshot(
                &install_path,
                &current_entries,
                &format!("project-{}-revert", project.id),
            )?
        };

        if !dry_run {
            restore_manifest_snapshot(&source_backup_path, &source_entries, &install_path)?;
        }

        results.push(RevertResult {
            project,
            install_path: install_path.to_string_lossy().to_string(),
            restored_from_backup_path: Some(source_backup_path.to_string_lossy().to_string()),
            dry_run,
            restored_files,
            removed_files,
            backup_path: backup_path.map(|path| path.to_string_lossy().to_string()),
            status: if dry_run {
                "dry_run".to_string()
            } else {
                "reverted".to_string()
            },
        });
    }

    let install_by_path = if !dry_run && !results.is_empty() {
        record_project_scan(
            catalog,
            &results
                .iter()
                .map(|result| result.project.canonical_path.clone())
                .collect::<Vec<_>>(),
        )?
    } else {
        install_by_id
            .values()
            .map(|install| (install.observed_path.clone(), install.clone()))
            .collect::<HashMap<_, _>>()
    };

    for result in &results {
        let Some(install) = install_by_path.get(&result.install_path) else {
            continue;
        };
        let (commit_sha, version) = install
            .matched_upstream_commit_sha
            .clone()
            .map(|sha| (sha, install.matched_upstream_version.clone()))
            .map(Ok)
            .unwrap_or_else(|| fallback_commit_ref(catalog))?;
        let changed_files = result
            .restored_files
            .iter()
            .chain(&result.removed_files)
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        catalog.record_sync_event(
            install.id,
            &commit_sha,
            version.as_deref(),
            result.dry_run,
            &changed_files,
            result.backup_path.as_deref(),
            &result.status,
            &json!({
                "project_id": result.project.id,
                "project_path": result.project.canonical_path,
                "install_path": result.install_path,
                "restored_from_backup_path": result.restored_from_backup_path,
                "restored_files": result.restored_files,
                "removed_files": result.removed_files,
            }),
        )?;
    }

    Ok(results)
}
