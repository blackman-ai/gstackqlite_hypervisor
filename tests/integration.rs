use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use gstackqlite_hypervisor::db::Catalog;
use gstackqlite_hypervisor::ideas::build_ideas;
use gstackqlite_hypervisor::ingest::ingest_upstream;
use gstackqlite_hypervisor::scan::scan_local_installs;
use gstackqlite_hypervisor::upgrade::{apply_version_to_projects, materialize_targets};

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

fn init_fixture_repo(repo_dir: &Path) -> Result<(String, String)> {
    fs::create_dir_all(repo_dir)?;
    run(repo_dir, &["init"])?;
    run(repo_dir, &["config", "user.name", "Test User"])?;
    run(repo_dir, &["config", "user.email", "test@example.com"])?;
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
    let workspace = temp_dir("gstack-hypervisor-test");
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
    run(&project_repo, &["config", "user.name", "Test User"])?;
    run(&project_repo, &["config", "user.email", "test@example.com"])?;
    fs::create_dir_all(install_dir.join("docs"))?;
    fs::write(install_dir.join("VERSION"), "0.0.1.0\n")?;
    fs::write(install_dir.join("README.md"), "# fixture v1\n")?;
    fs::write(install_dir.join("docs").join("note.md"), "first\n")?;

    let scan = scan_local_installs(&catalog, std::slice::from_ref(&workspace), Some(4))?;
    catalog.record_scan(&scan)?;
    let installs = catalog.list_installs(false, None, None)?;
    assert_eq!(installs.len(), 1);
    assert_eq!(installs[0].is_outdated, Some(true));

    let ideas = build_ideas(&installs, &catalog.summary()?.source);
    assert!(
        ideas
            .iter()
            .any(|idea| idea.title.contains("older local gstack copy"))
    );

    let dry_run = materialize_targets(&catalog, None, None, &[], true, true, false)?;
    assert_eq!(dry_run.len(), 1);
    assert!(
        dry_run[0]
            .changes
            .updated
            .iter()
            .any(|path| path == "VERSION")
    );

    let applied = materialize_targets(&catalog, None, None, &[], true, false, false)?;
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
    let workspace = temp_dir("gstack-hypervisor-project-test");
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
    run(&project_repo, &["config", "user.name", "Test User"])?;
    run(&project_repo, &["config", "user.email", "test@example.com"])?;
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

    let scan = scan_local_installs(&catalog, std::slice::from_ref(&workspace), Some(4))?;
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
