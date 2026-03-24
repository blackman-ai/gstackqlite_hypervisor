use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand, ValueEnum};

use gstackqlite_hypervisor::config::{DEFAULT_MAX_DEPTH, default_scan_roots};
use gstackqlite_hypervisor::db::Catalog;
use gstackqlite_hypervisor::ideas::build_ideas;
use gstackqlite_hypervisor::ingest::{ensure_catalog_has_upstream, ingest_upstream};
use gstackqlite_hypervisor::mcp;
use gstackqlite_hypervisor::scan::{scan_local_installs, sync_catalog};
use gstackqlite_hypervisor::tui;
use gstackqlite_hypervisor::upgrade::{
    apply_version_to_projects, materialize_targets, project_diff_preview, remove_projects,
    revert_projects,
};
use gstackqlite_hypervisor::util::default_db_path;

#[derive(Parser)]
#[command(name = "gstackqlite-hypervisor")]
#[command(
    about = "SQLite-first Rust CLI and TUI for tracking, previewing, and applying local gstack installs."
)]
struct Cli {
    #[arg(long)]
    db: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    Sync {
        #[arg(long = "root")]
        roots: Vec<PathBuf>,
        #[arg(long, default_value_t = DEFAULT_MAX_DEPTH)]
        max_depth: usize,
        #[arg(long)]
        json: bool,
    },
    Ingest {
        #[arg(long)]
        repo_url: Option<String>,
        #[arg(long)]
        reference: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Scan {
        #[arg(long = "root")]
        roots: Vec<PathBuf>,
        #[arg(long, default_value_t = DEFAULT_MAX_DEPTH)]
        max_depth: usize,
        #[arg(long)]
        json: bool,
    },
    Projects {
        #[arg(long)]
        json: bool,
    },
    Project {
        identifier: String,
    },
    History {
        identifier: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    Versions {
        #[arg(long)]
        search: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Diff {
        identifier: String,
        #[arg(long)]
        commit: Option<String>,
        #[arg(long)]
        version: Option<String>,
        #[arg(long, default_value_t = 6)]
        files: usize,
        #[arg(long, default_value_t = 14)]
        lines: usize,
        #[arg(long)]
        json: bool,
    },
    Apply {
        #[arg(long)]
        commit: Option<String>,
        #[arg(long)]
        version: Option<String>,
        #[arg(long = "project")]
        projects: Vec<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    Remove {
        #[arg(long = "project")]
        projects: Vec<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    Revert {
        #[arg(long = "project")]
        projects: Vec<String>,
        #[arg(long)]
        event: Option<i64>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    List {
        #[arg(long)]
        outdated: bool,
        #[arg(long)]
        json: bool,
    },
    Ideas {
        #[arg(long)]
        json: bool,
    },
    Inspect {
        identifier: String,
    },
    Upgrade {
        #[arg(long)]
        commit: Option<String>,
        #[arg(long)]
        version: Option<String>,
        #[arg(long = "target")]
        targets: Vec<String>,
        #[arg(long)]
        outdated: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        allow_git_targets: bool,
        #[arg(long)]
        json: bool,
    },
    Mcp {
        #[command(subcommand)]
        command: Option<McpCommand>,
    },
    Tui,
}

#[derive(Subcommand)]
enum McpCommand {
    Serve,
    Install {
        #[arg(long, conflicts_with = "project")]
        global: bool,
        #[arg(long)]
        project: Option<String>,
        #[arg(long, value_enum, default_value_t = AgentSelection::All)]
        agent: AgentSelection,
        #[arg(long)]
        json: bool,
    },
    Uninstall {
        #[arg(long, conflicts_with = "project")]
        global: bool,
        #[arg(long)]
        project: Option<String>,
        #[arg(long, value_enum, default_value_t = AgentSelection::All)]
        agent: AgentSelection,
        #[arg(long)]
        json: bool,
    },
    Status {
        #[arg(long, conflicts_with = "project")]
        global: bool,
        #[arg(long)]
        project: Option<String>,
        #[arg(long, value_enum, default_value_t = AgentSelection::All)]
        agent: AgentSelection,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum AgentSelection {
    All,
    Claude,
    Codex,
}

fn print_json<T: serde::Serialize>(value: &T) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string())
    );
}

fn resolve_roots(roots: Vec<PathBuf>) -> Vec<PathBuf> {
    if roots.is_empty() {
        default_scan_roots()
    } else {
        roots
    }
}

fn selected_agents(selection: AgentSelection) -> Vec<mcp::McpAgent> {
    match selection {
        AgentSelection::All => vec![mcp::McpAgent::Claude, mcp::McpAgent::Codex],
        AgentSelection::Claude => vec![mcp::McpAgent::Claude],
        AgentSelection::Codex => vec![mcp::McpAgent::Codex],
    }
}

fn resolve_mcp_scope(
    catalog: &Catalog,
    global: bool,
    project: Option<String>,
) -> Result<mcp::McpScope> {
    match (global, project) {
        (true, None) | (false, None) => Ok(mcp::McpScope::Global),
        (_, Some(identifier)) => mcp::resolve_project_scope(catalog, &identifier),
    }
}

fn cli_project_status(project: &gstackqlite_hypervisor::models::CatalogProject) -> String {
    match project.effective_gstack_source.as_str() {
        "local_install" => project
            .effective_gstack_version
            .as_ref()
            .map(|version| format!("local:{version}"))
            .unwrap_or_else(|| "local".to_string()),
        "global_install" => project
            .effective_gstack_version
            .as_ref()
            .map(|version| format!("global:{version}"))
            .unwrap_or_else(|| "global".to_string()),
        "none" if project.has_git_repo => "ready".to_string(),
        "none" => "configured".to_string(),
        other => other.to_string(),
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let db_path = cli.db.unwrap_or_else(default_db_path);
    let catalog = Catalog::new(&db_path)?;

    match cli.command {
        Some(Command::Sync {
            roots,
            max_depth,
            json,
        }) => {
            let scan_roots = resolve_roots(roots);
            let scan = sync_catalog(&catalog, &scan_roots, Some(max_depth))?;
            if json {
                print_json(&scan);
            } else {
                println!(
                    "Synced upstream {} ({}) and scanned {} project(s), {} install(s), {} repo(s).",
                    scan.source_head_version
                        .unwrap_or_else(|| "unknown".to_string()),
                    scan.source_head_sha
                        .map(|sha| sha.chars().take(12).collect::<String>())
                        .unwrap_or_else(|| "unknown".to_string()),
                    scan.projects.len(),
                    scan.installs.len(),
                    scan.repositories.len(),
                );
            }
        }
        Some(Command::Ingest {
            repo_url,
            reference,
            json,
        }) => {
            let summary = ingest_upstream(&catalog, repo_url.as_deref(), reference.as_deref())?;
            if json {
                print_json(&summary);
            } else {
                println!(
                    "Ingested {} commits into {}. Head: {} ({})",
                    summary.commit_count,
                    catalog.path.display(),
                    summary.head_sha,
                    summary
                        .head_version
                        .unwrap_or_else(|| "unknown".to_string())
                );
            }
        }
        Some(Command::Scan {
            roots,
            max_depth,
            json,
        }) => {
            ensure_catalog_has_upstream(&catalog)?;
            let scan_roots = resolve_roots(roots);
            let scan = scan_local_installs(&catalog, &scan_roots, Some(max_depth))?;
            catalog.record_scan(&scan)?;
            if json {
                print_json(&scan);
            } else {
                println!(
                    "Scanned {} project(s), {} install(s), {} repo(s). Upstream head: {} ({})",
                    scan.projects.len(),
                    scan.installs.len(),
                    scan.repositories.len(),
                    scan.source_head_sha
                        .unwrap_or_else(|| "unknown".to_string()),
                    scan.source_head_version
                        .unwrap_or_else(|| "unknown".to_string())
                );
            }
        }
        Some(Command::Projects { json }) => {
            let projects = catalog.list_projects()?;
            if json {
                print_json(&projects);
            } else {
                for project in projects {
                    println!(
                        "#{} {} status={} path={} install={}",
                        project.id,
                        project.name,
                        cli_project_status(&project),
                        project.canonical_path,
                        project
                            .gstack_install_observed_path
                            .unwrap_or_else(|| "-".to_string())
                    );
                }
            }
        }
        Some(Command::Project { identifier }) => {
            let Some(detail) = catalog.project_detail(&identifier)? else {
                bail!("project not found: {identifier}");
            };
            print_json(&detail);
        }
        Some(Command::History {
            identifier,
            limit,
            json,
        }) => {
            let history = catalog.project_backup_history(&identifier, limit)?;
            if json {
                print_json(&history);
            } else {
                for event in history {
                    println!(
                        "#{} {} version={} status={} files={} backup={}",
                        event.id,
                        event.created_at,
                        event.version.unwrap_or_else(|| {
                            event.commit_sha.chars().take(12).collect::<String>()
                        }),
                        event.status,
                        event.changed_files.len(),
                        event.backup_path.unwrap_or_else(|| "-".to_string())
                    );
                }
            }
        }
        Some(Command::Versions { search, json }) => {
            ensure_catalog_has_upstream(&catalog)?;
            let versions = catalog.list_versions(search.as_deref())?;
            if json {
                print_json(&versions);
            } else {
                for version in versions {
                    println!(
                        "{} {} {} {}",
                        version.version,
                        version.commit_sha.chars().take(12).collect::<String>(),
                        version.committed_at,
                        version.subject
                    );
                }
            }
        }
        Some(Command::Diff {
            identifier,
            commit,
            version,
            files,
            lines,
            json,
        }) => {
            let preview = project_diff_preview(
                &catalog,
                &identifier,
                version.as_deref(),
                commit.as_deref(),
                files,
                lines,
            )?;
            if json {
                print_json(&preview);
            } else {
                println!(
                    "project={} target={} changed={} (+{} ~{} -{}) install={}",
                    preview.project.name,
                    preview.version.clone().unwrap_or_else(|| preview
                        .commit_sha
                        .chars()
                        .take(12)
                        .collect()),
                    preview.total_changed_files,
                    preview.added_count,
                    preview.updated_count,
                    preview.removed_count,
                    preview.install_path
                );
                for file in preview.files {
                    println!();
                    println!("[{}] {}", file.change_type, file.path);
                    for line in file.preview_lines {
                        println!("{line}");
                    }
                    if file.truncated {
                        println!("... preview truncated ...");
                    }
                }
            }
        }
        Some(Command::Apply {
            commit,
            version,
            projects,
            dry_run,
            json,
        }) => {
            let results = apply_version_to_projects(
                &catalog,
                version.as_deref(),
                commit.as_deref(),
                &projects,
                dry_run,
            )?;
            if json {
                print_json(&results);
            } else {
                for result in results {
                    println!(
                        "{} project={} version={} applied={} merged={} conflicts={} preserved={} removed={} backup={}",
                        result.status,
                        result.project.name,
                        result.version.unwrap_or_else(|| result
                            .commit_sha
                            .chars()
                            .take(12)
                            .collect()),
                        result.applied_files.len(),
                        result.merged_files.len(),
                        result.conflict_files.len(),
                        result.preserved_local_files.len(),
                        result.removed_files.len(),
                        result.backup_path.unwrap_or_else(|| "-".to_string())
                    );
                }
            }
        }
        Some(Command::Remove {
            projects,
            dry_run,
            json,
        }) => {
            let results = remove_projects(&catalog, &projects, dry_run)?;
            if json {
                print_json(&results);
            } else {
                for result in results {
                    println!(
                        "{} project={} removed={} backup={}",
                        result.status,
                        result.project.name,
                        result.removed_files.len(),
                        result.backup_path.unwrap_or_else(|| "-".to_string())
                    );
                }
            }
        }
        Some(Command::Revert {
            projects,
            event,
            dry_run,
            json,
        }) => {
            let results = revert_projects(&catalog, &projects, event, dry_run)?;
            if json {
                print_json(&results);
            } else {
                for result in results {
                    println!(
                        "{} project={} restored={} removed={} source_backup={} backup={}",
                        result.status,
                        result.project.name,
                        result.restored_files.len(),
                        result.removed_files.len(),
                        result
                            .restored_from_backup_path
                            .unwrap_or_else(|| "-".to_string()),
                        result.backup_path.unwrap_or_else(|| "-".to_string())
                    );
                }
            }
        }
        Some(Command::List { outdated, json }) => {
            let installs = catalog.list_installs(outdated, None, None)?;
            if json {
                print_json(&installs);
            } else {
                for install in installs {
                    println!(
                        "#{} {} {} version={} outdated={} path={}",
                        install.id,
                        install.host,
                        install.install_type,
                        install
                            .local_version
                            .unwrap_or_else(|| "unknown".to_string()),
                        install
                            .is_outdated
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "unknown".to_string()),
                        install.observed_path
                    );
                }
            }
        }
        Some(Command::Ideas { json }) => {
            let summary = catalog.summary()?;
            let ideas = build_ideas(&catalog.list_installs(false, None, None)?, &summary.source);
            if json {
                print_json(&ideas);
            } else {
                for idea in ideas {
                    println!("[{}] {}", idea.severity, idea.title);
                    println!("  {}", idea.rationale);
                    println!("  Action: {}", idea.action);
                }
            }
        }
        Some(Command::Inspect { identifier }) => {
            let Some(detail) = catalog.install_detail(&identifier)? else {
                bail!("install not found: {identifier}");
            };
            print_json(&detail);
        }
        Some(Command::Upgrade {
            commit,
            version,
            targets,
            outdated,
            dry_run,
            allow_git_targets,
            json,
        }) => {
            let results = materialize_targets(
                &catalog,
                commit.as_deref(),
                version.as_deref(),
                &targets,
                outdated,
                dry_run,
                allow_git_targets,
            )?;
            if json {
                print_json(&results);
            } else {
                for result in results {
                    println!(
                        "{} #{} -> {} ({}) added={} updated={} removed={}",
                        result.status,
                        result.target.id,
                        result.commit_sha,
                        result.version.unwrap_or_else(|| "unknown".to_string()),
                        result.changes.added.len(),
                        result.changes.updated.len(),
                        result.changes.removed.len(),
                    );
                }
            }
        }
        Some(Command::Mcp { command }) => match command {
            None | Some(McpCommand::Serve) => {
                mcp::run_stdio_server(catalog)?;
            }
            Some(McpCommand::Install {
                global,
                project,
                agent,
                json,
            }) => {
                let scope = resolve_mcp_scope(&catalog, global, project)?;
                let results = mcp::install_config(&scope, &selected_agents(agent))?;
                if json {
                    print_json(&results);
                } else {
                    for result in results {
                        println!(
                            "{} {} scope={} config={}",
                            result.status, result.agent, result.scope, result.config_path
                        );
                    }
                }
            }
            Some(McpCommand::Uninstall {
                global,
                project,
                agent,
                json,
            }) => {
                let scope = resolve_mcp_scope(&catalog, global, project)?;
                let results = mcp::uninstall_config(&scope, &selected_agents(agent))?;
                if json {
                    print_json(&results);
                } else {
                    for result in results {
                        println!(
                            "{} {} scope={} config={}",
                            result.status, result.agent, result.scope, result.config_path
                        );
                    }
                }
            }
            Some(McpCommand::Status {
                global,
                project,
                agent,
                json,
            }) => {
                let scope = resolve_mcp_scope(&catalog, global, project)?;
                let results = mcp::inspect_config(&scope, &selected_agents(agent))?;
                if json {
                    print_json(&results);
                } else {
                    for result in results {
                        println!(
                            "{} {} scope={} config={}",
                            result.status, result.agent, result.scope, result.config_path
                        );
                    }
                }
            }
        },
        Some(Command::Tui) | None => {
            tui::run(catalog)?;
        }
    }

    Ok(())
}
