use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use gstackqlite_hypervisor::db::Catalog;
use gstackqlite_hypervisor::ideas::build_ideas;
use gstackqlite_hypervisor::ingest::ingest_upstream;
use gstackqlite_hypervisor::scan::scan_local_installs;
use gstackqlite_hypervisor::upgrade::{
    apply_version_to_projects, materialize_targets, project_diff_preview, revert_projects,
};

fn temp_dir(prefix: &str) -> PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let path = std::env::temp_dir().join(format!("{prefix}-{}-{millis}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    path
}

fn run(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("failed to execute git {:?}", args))?;
    if !output.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn configure_test_repo(repo_dir: &Path) -> Result<()> {
    run(repo_dir, &["config", "user.name", "Test User"])?;
    run(repo_dir, &["config", "user.email", "test@example.com"])?;
    run(repo_dir, &["config", "commit.gpgsign", "false"])?;
    run(repo_dir, &["config", "tag.gpgsign", "false"])?;
    Ok(())
}

fn init_fixture_repo(repo_dir: &Path) -> Result<(String, String)> {
    fs::create_dir_all(repo_dir)?;
    run(repo_dir, &["init"])?;
    configure_test_repo(repo_dir)?;
    fs::create_dir_all(repo_dir.join("docs"))?;
    fs::write(repo_dir.join("VERSION"), "0.0.1.0\n")?;
    fs::write(repo_dir.join("README.md"), "# fixture v1\n")?;
    fs::write(repo_dir.join("docs").join("note.md"), "first\n")?;
    run(repo_dir, &["add", "."])?;
    run(repo_dir, &["commit", "-m", "first"])?;
    let old_sha = run(repo_dir, &["rev-parse", "HEAD"])?;

    fs::write(repo_dir.join("VERSION"), "0.0.2.0\n")?;
    fs::write(repo_dir.join("README.md"), "# fixture v2\n")?;
    fs::write(repo_dir.join("docs").join("note.md"), "second\n")?;
    fs::write(repo_dir.join("NEWFILE.md"), "new\n")?;
    run(repo_dir, &["add", "."])?;
    run(repo_dir, &["commit", "-m", "second"])?;
    let head_sha = run(repo_dir, &["rev-parse", "HEAD"])?;
    Ok((old_sha, head_sha))
}

#[test]
fn ingest_scan_and_upgrade_work_end_to_end() -> Result<()> {
    let workspace = temp_dir("gstackqlite-hypervisor-test");
    let upstream_repo = workspace.join("upstream");
    let project_repo = workspace.join("project");
    let install_dir = project_repo.join(".claude").join("skills").join("gstack");
    let db_path = workspace.join("catalog.sqlite");

    let (_old_sha, head_sha) = init_fixture_repo(&upstream_repo)?;
    let catalog = Catalog::new(&db_path)?;
    let summary = ingest_upstream(
        &catalog,
        Some(upstream_repo.to_str().unwrap()),
        Some("HEAD"),
    )?;
    assert_eq!(summary.commit_count, 2);
    assert_eq!(summary.head_sha, head_sha);

    fs::create_dir_all(&project_repo)?;
    run(&project_repo, &["init"])?;
    configure_test_repo(&project_repo)?;
    fs::create_dir_all(install_dir.join("docs"))?;
    fs::write(install_dir.join("VERSION"), "0.0.1.0\n")?;
    fs::write(install_dir.join("README.md"), "# fixture v1\n")?;
    fs::write(install_dir.join("docs").join("note.md"), "first\n")?;

    let scan = scan_local_installs(&catalog, std::slice::from_ref(&project_repo), Some(4))?;
    catalog.record_scan(&scan)?;
    let installs = catalog.list_installs(false, None, None)?;
    let canonical_install_dir = fs::canonicalize(&install_dir)?;
    let fixture_install = installs
        .iter()
        .find(|install| install.observed_path == canonical_install_dir.to_string_lossy())
        .expect("fixture install should be present in the catalog");
    assert_eq!(fixture_install.is_outdated, Some(true));

    let ideas = build_ideas(&installs, &catalog.summary()?.source);
    assert!(
        ideas
            .iter()
            .any(|idea| idea.title.contains("older local gstack copy"))
    );

    let dry_run = materialize_targets(
        &catalog,
        None,
        None,
        &[canonical_install_dir.to_string_lossy().to_string()],
        false,
        true,
        false,
    )?;
    assert_eq!(dry_run.len(), 1);
    assert!(
        dry_run[0]
            .changes
            .updated
            .iter()
            .any(|path| path == "VERSION")
    );

    let applied = materialize_targets(
        &catalog,
        None,
        None,
        &[canonical_install_dir.to_string_lossy().to_string()],
        false,
        false,
        false,
    )?;
    assert_eq!(applied.len(), 1);
    assert_eq!(
        fs::read_to_string(install_dir.join("VERSION"))?.trim(),
        "0.0.2.0"
    );
    assert_eq!(
        fs::read_to_string(install_dir.join("NEWFILE.md"))?.trim(),
        "new"
    );

    Ok(())
}

#[test]
fn project_catalog_and_merge_aware_apply_work_end_to_end() -> Result<()> {
    let workspace = temp_dir("gstackqlite-hypervisor-project-test");
    let upstream_repo = workspace.join("upstream");
    let project_repo = workspace.join("project");
    let install_dir = project_repo.join(".claude").join("skills").join("gstack");
    let db_path = workspace.join("catalog.sqlite");

    let (_old_sha, head_sha) = init_fixture_repo(&upstream_repo)?;
    let catalog = Catalog::new(&db_path)?;
    ingest_upstream(
        &catalog,
        Some(upstream_repo.to_str().unwrap()),
        Some("HEAD"),
    )?;

    fs::create_dir_all(&project_repo)?;
    run(&project_repo, &["init"])?;
    configure_test_repo(&project_repo)?;
    fs::write(
        project_repo.join("CLAUDE.md"),
        "## gstack\nUse local skills.\n",
    )?;
    fs::create_dir_all(project_repo.join(".claude"))?;
    fs::write(
        project_repo.join(".claude").join("settings.json"),
        "{ \"model\": \"sonnet\" }\n",
    )?;

    fs::create_dir_all(install_dir.join("docs"))?;
    fs::write(install_dir.join("VERSION"), "0.0.1.0\n")?;
    fs::write(install_dir.join("README.md"), "# locally customized v1\n")?;
    fs::write(install_dir.join("docs").join("note.md"), "first\n")?;
    fs::write(install_dir.join("CUSTOM.md"), "keep me\n")?;

    let scan = scan_local_installs(&catalog, std::slice::from_ref(&project_repo), Some(4))?;
    catalog.record_scan(&scan)?;

    let projects = catalog.list_projects()?;
    let canonical_install_dir = fs::canonicalize(&install_dir)?;
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].effective_gstack_source, "local_install");
    assert_eq!(
        projects[0].effective_gstack_version.as_deref(),
        Some("0.0.1.0")
    );
    assert_eq!(
        projects[0].gstack_install_observed_path.as_deref(),
        Some(canonical_install_dir.to_str().unwrap())
    );

    let versions = catalog.list_versions(None)?;
    assert!(versions.iter().any(|version| version.version == "0.0.1.0"));
    assert!(versions.iter().any(|version| version.version == "0.0.2.0"));

    let project_identifier = vec![projects[0].id.to_string()];
    let dry_run =
        apply_version_to_projects(&catalog, Some("0.0.2.0"), None, &project_identifier, true)?;
    assert_eq!(dry_run.len(), 1);
    assert!(
        dry_run[0]
            .applied_files
            .iter()
            .any(|path| path == "VERSION")
    );
    assert!(
        dry_run[0]
            .applied_files
            .iter()
            .any(|path| path == "NEWFILE.md")
    );
    assert!(
        dry_run[0]
            .merged_files
            .iter()
            .any(|path| path == "README.md")
    );
    assert!(
        dry_run[0]
            .preserved_local_files
            .iter()
            .any(|path| path == "CUSTOM.md")
    );

    let applied =
        apply_version_to_projects(&catalog, Some("0.0.2.0"), None, &project_identifier, false)?;
    assert_eq!(applied.len(), 1);
    let result = &applied[0];
    let backup_path = PathBuf::from(result.backup_path.as_ref().unwrap());
    assert!(backup_path.exists());
    assert_eq!(
        fs::read_to_string(install_dir.join("VERSION"))?.trim(),
        "0.0.2.0"
    );
    let readme = fs::read_to_string(install_dir.join("README.md"))?;
    assert!(readme.contains("<<<<<<< local customization"));
    assert!(readme.contains("# locally customized v1"));
    assert!(readme.contains("# fixture v2"));
    assert_eq!(
        fs::read_to_string(install_dir.join("docs").join("note.md"))?.trim(),
        "second"
    );
    assert_eq!(
        fs::read_to_string(install_dir.join("NEWFILE.md"))?.trim(),
        "new"
    );
    assert_eq!(
        fs::read_to_string(install_dir.join("CUSTOM.md"))?.trim(),
        "keep me"
    );
    assert_eq!(
        fs::read_to_string(backup_path.join("CUSTOM.md"))?.trim(),
        "keep me"
    );

    let refreshed_project = catalog
        .find_project(&projects[0].id.to_string())?
        .expect("project should still exist after apply");
    assert_eq!(
        refreshed_project.effective_gstack_version.as_deref(),
        Some("0.0.2.0")
    );

    let install_detail = catalog
        .install_detail(
            refreshed_project
                .gstack_install_observed_path
                .as_deref()
                .expect("project should still point at an install"),
        )?
        .expect("install detail should exist");
    assert!(!install_detail.sync_events.is_empty());
    assert_eq!(install_detail.sync_events[0].commit_sha, head_sha);

    Ok(())
}

#[test]
fn diff_preview_and_targeted_revert_work_end_to_end() -> Result<()> {
    let workspace = temp_dir("gstackqlite-hypervisor-revert-test");
    let upstream_repo = workspace.join("upstream");
    let project_repo = workspace.join("project");
    let install_dir = project_repo.join(".claude").join("skills").join("gstack");
    let db_path = workspace.join("catalog.sqlite");

    init_fixture_repo(&upstream_repo)?;
    let catalog = Catalog::new(&db_path)?;
    ingest_upstream(
        &catalog,
        Some(upstream_repo.to_str().unwrap()),
        Some("HEAD"),
    )?;

    fs::create_dir_all(&project_repo)?;
    run(&project_repo, &["init"])?;
    configure_test_repo(&project_repo)?;
    fs::write(project_repo.join("CLAUDE.md"), "Use local skills.\n")?;
    fs::create_dir_all(project_repo.join(".claude"))?;
    fs::create_dir_all(install_dir.join("docs"))?;
    fs::write(install_dir.join("VERSION"), "0.0.1.0\n")?;
    fs::write(install_dir.join("README.md"), "# locally customized v1\n")?;
    fs::write(install_dir.join("docs").join("note.md"), "first\n")?;
    fs::write(install_dir.join("CUSTOM.md"), "keep me\n")?;

    let scan = scan_local_installs(&catalog, std::slice::from_ref(&project_repo), Some(4))?;
    catalog.record_scan(&scan)?;
    let mut projects = catalog.list_projects()?;
    let project = projects.remove(0);

    let preview = project_diff_preview(
        &catalog,
        &project.id.to_string(),
        Some("0.0.2.0"),
        None,
        8,
        12,
    )?;
    assert_eq!(preview.version.as_deref(), Some("0.0.2.0"));
    assert!(preview.total_changed_files >= 3);
    assert!(preview.files.iter().any(|file| file.path == "README.md"));
    assert!(preview.files.iter().any(|file| {
        file.path == "README.md"
            && file.preview_lines.iter().any(|line| {
                line.contains("# locally customized v1") || line.contains("# fixture v2")
            })
    }));

    let project_identifier = vec![project.id.to_string()];
    let applied =
        apply_version_to_projects(&catalog, Some("0.0.2.0"), None, &project_identifier, false)?;
    assert_eq!(applied.len(), 1);

    let history = catalog.project_backup_history(&project.id.to_string(), 10)?;
    assert_eq!(history.len(), 1);
    let event_id = history[0].id;

    let dry_run = revert_projects(&catalog, &project_identifier, Some(event_id), true)?;
    assert_eq!(dry_run.len(), 1);
    assert_eq!(dry_run[0].status, "dry_run");
    assert!(
        dry_run[0]
            .restored_files
            .iter()
            .any(|path| path == "README.md")
    );

    let reverted = revert_projects(&catalog, &project_identifier, Some(event_id), false)?;
    assert_eq!(reverted.len(), 1);
    assert_eq!(reverted[0].status, "reverted");
    assert_eq!(
        fs::read_to_string(install_dir.join("VERSION"))?.trim(),
        "0.0.1.0"
    );
    assert_eq!(
        fs::read_to_string(install_dir.join("README.md"))?.trim(),
        "# locally customized v1"
    );
    assert_eq!(
        fs::read_to_string(install_dir.join("CUSTOM.md"))?.trim(),
        "keep me"
    );

    Ok(())
}

#[test]
fn version_metadata_files_are_overwritten_not_conflict_merged() -> Result<()> {
    let workspace = temp_dir("gstackqlite-hypervisor-version-metadata-test");
    let upstream_repo = workspace.join("upstream");
    let project_repo = workspace.join("project");
    let install_dir = project_repo.join(".claude").join("skills").join("gstack");
    let db_path = workspace.join("catalog.sqlite");

    init_fixture_repo(&upstream_repo)?;
    let catalog = Catalog::new(&db_path)?;
    ingest_upstream(
        &catalog,
        Some(upstream_repo.to_str().unwrap()),
        Some("HEAD"),
    )?;

    fs::create_dir_all(&project_repo)?;
    run(&project_repo, &["init"])?;
    configure_test_repo(&project_repo)?;
    fs::write(
        project_repo.join("CLAUDE.md"),
        "## gstack\nUse local skills.\n",
    )?;

    fs::create_dir_all(install_dir.join("docs"))?;
    fs::create_dir_all(install_dir.join("browse").join("dist"))?;
    fs::write(install_dir.join("VERSION"), "0.0.1.0\n")?;
    fs::write(
        install_dir.join("browse").join("dist").join(".version"),
        "oldcommit\n",
    )?;
    fs::write(install_dir.join("README.md"), "# fixture v1\n")?;
    fs::write(install_dir.join("docs").join("note.md"), "first\n")?;

    let initial_scan = scan_local_installs(&catalog, std::slice::from_ref(&project_repo), Some(4))?;
    catalog.record_scan(&initial_scan)?;

    fs::write(
        install_dir.join("VERSION"),
        "<<<<<<< local customization\n0.0.1.1\n=======\n0.0.2.0\n>>>>>>> gstack 0.0.2.0\n",
    )?;
    fs::write(
        install_dir.join("browse").join("dist").join(".version"),
        "<<<<<<< local customization\noldcommit\n=======\nnewcommit\n>>>>>>> gstack 0.0.2.0\n",
    )?;

    let modified_scan =
        scan_local_installs(&catalog, std::slice::from_ref(&project_repo), Some(4))?;
    catalog.record_scan(&modified_scan)?;

    let project = catalog
        .list_projects()?
        .into_iter()
        .next()
        .expect("project should be cataloged");

    let applied = apply_version_to_projects(
        &catalog,
        Some("0.0.2.0"),
        None,
        &[project.id.to_string()],
        false,
    )?;
    let result = &applied[0];
    assert!(result.applied_files.iter().any(|path| path == "VERSION"));
    assert!(!result.merged_files.iter().any(|path| path == "VERSION"));
    assert!(!result.conflict_files.iter().any(|path| path == "VERSION"));
    assert_eq!(
        fs::read_to_string(install_dir.join("VERSION"))?.trim(),
        "0.0.2.0"
    );

    Ok(())
}

#[test]
fn codex_project_markers_are_detected() -> Result<()> {
    let workspace = temp_dir("gstackqlite-hypervisor-codex-project-test");
    let upstream_repo = workspace.join("upstream");
    let project_repo = workspace.join("codex-project");
    let install_dir = project_repo.join(".codex").join("skills").join("gstack");
    let db_path = workspace.join("catalog.sqlite");

    init_fixture_repo(&upstream_repo)?;
    let catalog = Catalog::new(&db_path)?;
    ingest_upstream(
        &catalog,
        Some(upstream_repo.to_str().unwrap()),
        Some("HEAD"),
    )?;

    fs::create_dir_all(&project_repo)?;
    run(&project_repo, &["init"])?;
    configure_test_repo(&project_repo)?;
    fs::write(
        project_repo.join("AGENTS.md"),
        "# Agents\nUse Codex for local workflows.\n",
    )?;
    fs::create_dir_all(project_repo.join(".codex"))?;
    fs::write(
        project_repo.join(".codex").join("config.toml"),
        "model = \"gpt-5\"\n",
    )?;

    fs::create_dir_all(install_dir.join("docs"))?;
    fs::write(install_dir.join("VERSION"), "0.0.1.0\n")?;
    fs::write(install_dir.join("README.md"), "# codex local fixture v1\n")?;
    fs::write(install_dir.join("docs").join("note.md"), "first\n")?;

    let scan = scan_local_installs(&catalog, std::slice::from_ref(&project_repo), Some(4))?;
    catalog.record_scan(&scan)?;

    let projects = catalog.list_projects()?;
    assert_eq!(projects.len(), 1);
    assert!(!projects[0].has_claude_md);
    assert!(!projects[0].has_claude_dir);
    assert!(!projects[0].has_claude_settings);
    assert!(projects[0].has_agents_md);
    assert!(projects[0].has_codex_dir);
    assert!(!projects[0].has_agents_dir);
    assert!(projects[0].has_codex_settings);
    assert_eq!(projects[0].codex_settings_paths.len(), 1);
    assert!(
        projects[0].codex_settings_paths[0].ends_with("/codex-project/.codex/config.toml"),
        "unexpected codex settings path: {}",
        projects[0].codex_settings_paths[0]
    );
    assert_eq!(projects[0].effective_gstack_source, "local_install");
    assert_eq!(
        projects[0].effective_gstack_version.as_deref(),
        Some("0.0.1.0")
    );

    let installs = catalog.list_installs(false, None, None)?;
    let canonical_install_dir = fs::canonicalize(&install_dir)?;
    let fixture_install = installs
        .iter()
        .find(|install| install.observed_path == canonical_install_dir.to_string_lossy())
        .expect("fixture codex install should be present in the catalog");
    assert_eq!(fixture_install.host.as_str(), "codex");

    Ok(())
}

#[test]
fn plain_git_repos_are_cataloged_and_ready_for_install() -> Result<()> {
    let workspace = temp_dir("gstackqlite-hypervisor-git-project-test");
    let upstream_repo = workspace.join("upstream");
    let project_repo = workspace.join("git-only-project");
    let db_path = workspace.join("catalog.sqlite");

    init_fixture_repo(&upstream_repo)?;
    let catalog = Catalog::new(&db_path)?;
    ingest_upstream(
        &catalog,
        Some(upstream_repo.to_str().unwrap()),
        Some("HEAD"),
    )?;

    fs::create_dir_all(&project_repo)?;
    run(&project_repo, &["init"])?;
    configure_test_repo(&project_repo)?;
    fs::write(project_repo.join("README.md"), "# plain repo\n")?;
    run(&project_repo, &["add", "README.md"])?;
    run(&project_repo, &["commit", "-m", "plain repo init"])?;

    let scan = scan_local_installs(&catalog, std::slice::from_ref(&project_repo), Some(4))?;
    catalog.record_scan(&scan)?;

    let projects = catalog.list_projects()?;
    assert_eq!(projects.len(), 1);
    assert!(projects[0].has_git_repo);
    assert!(!projects[0].has_claude_md);
    assert!(!projects[0].has_claude_dir);
    assert!(!projects[0].has_claude_settings);
    assert!(!projects[0].has_agents_md);
    assert!(!projects[0].has_agents_dir);
    assert!(!projects[0].has_codex_dir);
    assert!(!projects[0].has_codex_settings);
    assert_eq!(projects[0].effective_gstack_source, "none");
    assert_eq!(projects[0].effective_gstack_version, None);

    let project_identifier = vec![projects[0].id.to_string()];
    let dry_run =
        apply_version_to_projects(&catalog, Some("0.0.2.0"), None, &project_identifier, true)?;
    assert_eq!(dry_run.len(), 1);
    let canonical_project_repo = fs::canonicalize(&project_repo)?;
    assert_eq!(
        dry_run[0].install_path,
        canonical_project_repo
            .join(".claude")
            .join("skills")
            .join("gstack")
            .to_string_lossy()
    );
    assert!(
        dry_run[0]
            .applied_files
            .iter()
            .any(|path| path == "VERSION")
    );

    Ok(())
}
