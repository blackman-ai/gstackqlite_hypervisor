use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::config::{DEFAULT_MAX_DEPTH, default_scan_roots};
use crate::db::Catalog;
use crate::ideas::build_ideas;
use crate::lofi::LofiPlayer;
use crate::models::{CatalogInstall, CatalogProject, CatalogSummary, CatalogVersion, Idea};
use crate::scan::sync_catalog;
use crate::upgrade::apply_version_to_projects;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Projects,
    Versions,
}

struct App {
    catalog: Catalog,
    scan_roots: Vec<PathBuf>,
    summary: CatalogSummary,
    installs: Vec<CatalogInstall>,
    projects: Vec<CatalogProject>,
    versions: Vec<CatalogVersion>,
    ideas: Vec<Idea>,
    selected_project: usize,
    selected_version: usize,
    focus: Focus,
    lofi: Option<LofiPlayer>,
    music_started_at: Option<Instant>,
    status: String,
}

impl App {
    fn new(catalog: Catalog) -> Result<Self> {
        let scan_roots = default_scan_roots();
        let summary = catalog.summary()?;
        let mut app = Self {
            catalog,
            scan_roots,
            summary,
            installs: Vec::new(),
            projects: Vec::new(),
            versions: Vec::new(),
            ideas: Vec::new(),
            selected_project: 0,
            selected_version: 0,
            focus: Focus::Projects,
            lofi: None,
            music_started_at: None,
            status: "starting catalog bootstrap".to_string(),
        };
        app.refresh()?;
        app.sync_on_boot();
        app.start_lofi_default();
        Ok(app)
    }

    fn has_cached_catalog(&self) -> bool {
        self.summary.source.head_commit_sha.is_some()
            || !self.projects.is_empty()
            || !self.installs.is_empty()
    }

    fn sync_on_boot(&mut self) {
        match sync_catalog(&self.catalog, &self.scan_roots, Some(DEFAULT_MAX_DEPTH)) {
            Ok(scan) => {
                self.status = format!(
                    "startup sync: {} project(s), {} install(s), upstream {}",
                    scan.projects.len(),
                    scan.installs.len(),
                    scan.source_head_version
                        .unwrap_or_else(|| "unknown".to_string())
                );
            }
            Err(error) => {
                if self.has_cached_catalog() {
                    self.status = format!("startup sync failed, using cached catalog: {error}");
                } else {
                    self.status = format!("startup sync failed: {error}");
                }
            }
        }
        let _ = self.refresh();
    }

    fn refresh(&mut self) -> Result<()> {
        self.summary = self.catalog.summary()?;
        self.installs = self.catalog.list_installs(false, None, None)?;
        self.projects = self.catalog.list_projects()?;
        self.versions = self.catalog.list_versions(None).unwrap_or_default();
        self.ideas = build_ideas(&self.installs, &self.summary.source);
        if self.selected_project >= self.projects.len() {
            self.selected_project = self.projects.len().saturating_sub(1);
        }
        if self.selected_version >= self.versions.len() {
            self.selected_version = self.versions.len().saturating_sub(1);
        }
        Ok(())
    }

    fn sync_now(&mut self) {
        match sync_catalog(&self.catalog, &self.scan_roots, Some(DEFAULT_MAX_DEPTH)) {
            Ok(scan) => {
                self.status = format!(
                    "synced {} project(s), {} install(s), {} repo(s)",
                    scan.projects.len(),
                    scan.installs.len(),
                    scan.repositories.len()
                );
            }
            Err(error) => {
                self.status = format!("sync failed: {error}");
            }
        }
        let _ = self.refresh();
    }

    fn apply_selected(&mut self, dry_run: bool) {
        let Some(project) = self.selected_project().cloned() else {
            self.status = "no project selected".to_string();
            return;
        };
        let selected_version = self
            .selected_version()
            .map(|version| version.version.clone());
        match apply_version_to_projects(
            &self.catalog,
            selected_version.as_deref(),
            None,
            &[project.id.to_string()],
            dry_run,
        ) {
            Ok(results) => {
                if let Some(result) = results.first() {
                    self.status = format!(
                        "{} {} applied={} merged={} conflicts={} preserved={} removed={}",
                        if dry_run { "dry-run" } else { "applied" },
                        result.version.clone().unwrap_or_else(|| result
                            .commit_sha
                            .chars()
                            .take(12)
                            .collect()),
                        result.applied_files.len(),
                        result.merged_files.len(),
                        result.conflict_files.len(),
                        result.preserved_local_files.len(),
                        result.removed_files.len(),
                    );
                } else {
                    self.status = "no matching project result".to_string();
                }
            }
            Err(error) => {
                self.status = format!("apply failed: {error}");
            }
        }
        let _ = self.refresh();
    }

    fn toggle_lofi(&mut self) {
        if self.lofi.take().is_some() {
            self.music_started_at = None;
            self.status = "lofi loop stopped".to_string();
            return;
        }

        match LofiPlayer::start() {
            Ok(player) => {
                self.lofi = Some(player);
                self.music_started_at = Some(Instant::now());
                self.status = "lofi loop started".to_string();
            }
            Err(error) => {
                self.status = format!("lofi unavailable: {error}");
            }
        }
    }

    fn start_lofi_default(&mut self) {
        match LofiPlayer::start() {
            Ok(player) => {
                self.lofi = Some(player);
                self.music_started_at = Some(Instant::now());
                self.status = format!("{} | lofi on", self.status);
            }
            Err(error) => {
                self.status = format!("{} | lofi unavailable: {error}", self.status);
            }
        }
    }

    fn selected_project(&self) -> Option<&CatalogProject> {
        self.projects.get(self.selected_project)
    }

    fn selected_version(&self) -> Option<&CatalogVersion> {
        self.versions.get(self.selected_version)
    }

    fn install_for_selected_project(&self) -> Option<&CatalogInstall> {
        let project = self.selected_project()?;
        let install_id = project.gstack_install_id?;
        self.installs
            .iter()
            .find(|install| install.id == install_id)
    }

    fn next(&mut self) {
        match self.focus {
            Focus::Projects => {
                if !self.projects.is_empty() {
                    self.selected_project = (self.selected_project + 1) % self.projects.len();
                }
            }
            Focus::Versions => {
                if !self.versions.is_empty() {
                    self.selected_version = (self.selected_version + 1) % self.versions.len();
                }
            }
        }
    }

    fn previous(&mut self) {
        match self.focus {
            Focus::Projects => {
                if !self.projects.is_empty() {
                    self.selected_project = if self.selected_project == 0 {
                        self.projects.len() - 1
                    } else {
                        self.selected_project - 1
                    };
                }
            }
            Focus::Versions => {
                if !self.versions.is_empty() {
                    self.selected_version = if self.selected_version == 0 {
                        self.versions.len() - 1
                    } else {
                        self.selected_version - 1
                    };
                }
            }
        }
    }

    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Projects => Focus::Versions,
            Focus::Versions => Focus::Projects,
        };
    }
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    Terminal::new(CrosstermBackend::new(stdout)).map_err(Into::into)
}

fn restore_terminal(mut terminal: Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn render_list<'a>(
    title: &'a str,
    items: Vec<ListItem<'a>>,
    selected: Option<usize>,
    focused: bool,
) -> (List<'a>, ListState) {
    let block_style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let list = List::new(items)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(block_style),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = ListState::default();
    state.select(selected);
    (list, state)
}

fn render(app: &mut App, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    terminal.draw(|frame| {
        let areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(6),
                Constraint::Min(12),
                Constraint::Length(10),
            ])
            .split(frame.area());

        let summary_areas = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(78), Constraint::Percentage(22)])
            .split(areas[0]);

        let summary_text = vec![
            Line::from(vec![
                Span::styled("Source ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(
                    app.summary
                        .source
                        .head_version
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string()),
                ),
                Span::raw("  "),
                Span::styled("Head ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(
                    app.summary
                        .source
                        .head_commit_sha
                        .clone()
                        .map(|sha| sha.chars().take(12).collect::<String>())
                        .unwrap_or_else(|| "none".to_string()),
                ),
            ]),
            Line::from(format!(
                "Projects: {}  With local gstack: {}  Installs: {}  Outdated: {}  Git-backed: {}",
                app.summary.total_projects,
                app.summary.projects_with_local_gstack,
                app.summary.total_installs,
                app.summary.outdated_installs,
                app.summary.git_backed_installs,
            )),
            Line::from(format!(
                "Last scan: {}  Music: {}  Keys: q quit | g sync | m music | tab switch | j/k move | d dry-run | a apply | r refresh",
                app.summary
                    .last_scan_at
                    .clone()
                    .unwrap_or_else(|| "never".to_string()),
                if app.lofi.is_some() { "on" } else { "off" }
            )),
            Line::from(app.status.clone()),
        ];
        let summary = Paragraph::new(summary_text)
            .block(Block::default().title("Summary").borders(Borders::ALL))
            .wrap(Wrap { trim: true });
        frame.render_widget(summary, summary_areas[0]);

        let visualizer = Paragraph::new(build_visualizer_lines(app))
            .block(Block::default().title("Visualizer").borders(Borders::ALL))
            .wrap(Wrap { trim: true });
        frame.render_widget(visualizer, summary_areas[1]);

        let middle = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(45),
                Constraint::Percentage(30),
                Constraint::Percentage(25),
            ])
            .split(areas[1]);

        let project_items = if app.projects.is_empty() {
            vec![ListItem::new("No Claude-enabled projects in catalog.")]
        } else {
            app.projects
                .iter()
                .map(|project| {
                    ListItem::new(format!(
                        "#{} {} [{}] v{} {}",
                        project.id,
                        project.name,
                        project.effective_gstack_source,
                        project
                            .effective_gstack_version
                            .clone()
                            .unwrap_or_else(|| "unknown".to_string()),
                        project.canonical_path
                    ))
                })
                .collect::<Vec<_>>()
        };
        let (projects_list, mut projects_state) = render_list(
            "Projects",
            project_items,
            (!app.projects.is_empty()).then_some(app.selected_project),
            app.focus == Focus::Projects,
        );
        frame.render_stateful_widget(projects_list, middle[0], &mut projects_state);

        let version_items = if app.versions.is_empty() {
            vec![ListItem::new("No upstream versions in catalog yet.")]
        } else {
            app.versions
                .iter()
                .map(|version| {
                    ListItem::new(format!(
                        "{} {} {}",
                        version.version,
                        version.commit_sha.chars().take(12).collect::<String>(),
                        version.committed_at
                    ))
                })
                .collect::<Vec<_>>()
        };
        let (versions_list, mut versions_state) = render_list(
            "Versions",
            version_items,
            (!app.versions.is_empty()).then_some(app.selected_version),
            app.focus == Focus::Versions,
        );
        frame.render_stateful_widget(versions_list, middle[1], &mut versions_state);

        let idea_items = if app.ideas.is_empty() {
            vec![ListItem::new("No catalog recommendations yet.")]
        } else {
            app.ideas
                .iter()
                .map(|idea| ListItem::new(format!("[{}] {}", idea.severity, idea.title)))
                .collect::<Vec<_>>()
        };
        let ideas = List::new(idea_items)
            .block(Block::default().title("Recommendations").borders(Borders::ALL));
        frame.render_widget(ideas, middle[2]);

        let detail_text = if let Some(project) = app.selected_project() {
            let target = app.selected_version();
            let install = app.install_for_selected_project();
            vec![
                Line::from(format!("Project: #{} {}", project.id, project.name)),
                Line::from(format!("Path: {}", project.canonical_path)),
                Line::from(format!(
                    "Claude markers: CLAUDE.md={} .claude={} settings={} paths={}",
                    project.has_claude_md,
                    project.has_claude_dir,
                    project.has_claude_settings,
                    if project.claude_settings_paths.is_empty() {
                        "-".to_string()
                    } else {
                        project.claude_settings_paths.join(", ")
                    }
                )),
                Line::from(format!(
                    "Current gstack: source={} version={} install={}",
                    project.effective_gstack_source,
                    project
                        .effective_gstack_version
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string()),
                    project
                        .gstack_install_observed_path
                        .clone()
                        .unwrap_or_else(|| "-".to_string())
                )),
                Line::from(format!(
                    "Selected target: {} ({})",
                    target
                        .map(|version| version.version.clone())
                        .unwrap_or_else(|| {
                            app.summary
                                .source
                                .head_version
                                .clone()
                                .unwrap_or_else(|| "head".to_string())
                        }),
                    target
                        .map(|version| version.commit_sha.chars().take(12).collect::<String>())
                        .or_else(|| {
                            app.summary.source.head_commit_sha.as_ref().map(|sha| {
                                sha.chars().take(12).collect::<String>()
                            })
                        })
                        .unwrap_or_else(|| "none".to_string())
                )),
                Line::from(format!(
                    "Target subject: {}",
                    target
                        .map(|version| version.subject.clone())
                        .unwrap_or_else(|| "Recommendations on the right are catalog suggestions, not commit messages.".to_string())
                )),
                Line::from(match install {
                    Some(install) => format!(
                        "Install state: host={} type={} dirty={} outdated={} matched={}",
                        install.host,
                        install.install_type,
                        install.dirty,
                        install
                            .is_outdated
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "unknown".to_string()),
                        install
                            .matched_upstream_commit_sha
                            .clone()
                            .map(|sha| sha.chars().take(12).collect::<String>())
                            .unwrap_or_else(|| "none".to_string())
                    ),
                    None => "Install state: no repo-local install cataloged; apply will materialize one if needed".to_string(),
                }),
            ]
        } else {
            vec![Line::from("No project selected.")]
        };
        let detail = Paragraph::new(detail_text)
            .block(Block::default().title("Project Detail").borders(Borders::ALL))
            .wrap(Wrap { trim: true });
        frame.render_widget(detail, areas[2]);
    })?;
    Ok(())
}

fn build_visualizer_lines(app: &App) -> Vec<Line<'static>> {
    if app.lofi.is_none() {
        return vec![
            Line::from("music off"),
            Line::from(""),
            Line::from("[  ][  ][  ][  ]"),
        ];
    }

    let elapsed = app
        .music_started_at
        .map(|started_at| started_at.elapsed().as_secs_f32())
        .unwrap_or_default();
    let band_count = 5usize;
    let height = 3usize;
    let levels = (0..band_count)
        .map(|index| {
            let slow = ((elapsed * (0.8 + index as f32 * 0.09)).sin() * 0.5) + 0.5;
            let fast = ((elapsed * (1.7 + index as f32 * 0.18) + 0.7).sin() * 0.5) + 0.5;
            let level = ((slow * 0.4 + fast * 0.6) * height as f32).ceil() as usize;
            level.clamp(1, height)
        })
        .collect::<Vec<_>>();

    let mut lines = Vec::new();
    lines.push(Line::from("lofi on"));
    for row in (1..=height).rev() {
        let mut line = String::new();
        for level in &levels {
            if *level >= row {
                line.push_str("[##]");
            } else {
                line.push_str("[  ]");
            }
        }
        lines.push(Line::from(line));
    }
    lines
}

fn run_loop(app: &mut App, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    loop {
        render(app, terminal)?;
        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char('g') => app.sync_now(),
                    KeyCode::Char('m') => app.toggle_lofi(),
                    KeyCode::Char('a') => app.apply_selected(false),
                    KeyCode::Char('d') => app.apply_selected(true),
                    KeyCode::Char('r') => {
                        if let Err(error) = app.refresh() {
                            app.status = format!("refresh failed: {error}");
                        }
                    }
                    KeyCode::Tab => app.toggle_focus(),
                    KeyCode::Down | KeyCode::Char('j') => app.next(),
                    KeyCode::Up | KeyCode::Char('k') => app.previous(),
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

pub fn run(catalog: Catalog) -> Result<()> {
    let mut app = App::new(catalog)?;
    let mut terminal = setup_terminal()?;
    let loop_result = run_loop(&mut app, &mut terminal);
    let restore_result = restore_terminal(terminal);
    loop_result.and(restore_result)
}
