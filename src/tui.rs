use std::collections::HashMap;
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::config::{DEFAULT_MAX_DEPTH, default_scan_roots};
use crate::db::Catalog;
use crate::lofi::{LofiPlayer, TrackKind};
use crate::models::{
    CatalogCommitNote, CatalogInstall, CatalogProject, CatalogSummary, CatalogSyncEvent,
    CatalogVersion, CatalogVersionContext, DiffPreviewFile, ProjectDiffPreview,
};
use crate::scan::sync_catalog;
use crate::upgrade::{apply_version_to_projects, project_diff_preview, revert_projects};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Projects,
    Versions,
}

impl Focus {
    fn label(self) -> &'static str {
        match self {
            Self::Projects => "projects",
            Self::Versions => "versions",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InputMode {
    Normal,
    Filtering(Focus),
}

#[derive(Clone, Copy)]
struct Theme {
    id: &'static str,
    name: &'static str,
    accent: Color,
    accent_soft: Color,
    text: Color,
    muted: Color,
}

const THEMES: [Theme; 4] = [
    Theme {
        id: "sandhill_sandstone",
        name: "Sand Hill Sandstone",
        accent: Color::Yellow,
        accent_soft: Color::LightYellow,
        text: Color::White,
        muted: Color::DarkGray,
    },
    Theme {
        id: "singapore_harbor",
        name: "Singapore Harbor",
        accent: Color::Cyan,
        accent_soft: Color::LightCyan,
        text: Color::White,
        muted: Color::Blue,
    },
    Theme {
        id: "bengaluru_garden",
        name: "Bengaluru Garden",
        accent: Color::Green,
        accent_soft: Color::LightGreen,
        text: Color::White,
        muted: Color::DarkGray,
    },
    Theme {
        id: "shoreditch_neon",
        name: "Shoreditch Neon",
        accent: Color::Magenta,
        accent_soft: Color::LightMagenta,
        text: Color::White,
        muted: Color::DarkGray,
    },
];

const UI_THEME_SETTING_KEY: &str = "tui.theme_id";
const UI_TRACK_SETTING_KEY: &str = "tui.track_key";
const UI_MUSIC_SETTING_KEY: &str = "tui.music_enabled";
const DIFF_PREVIEW_DEBOUNCE: Duration = Duration::from_millis(150);

fn theme_index_by_id(theme_id: &str) -> Option<usize> {
    THEMES.iter().position(|theme| theme.id == theme_id)
}

fn track_index_by_key(track_key: &str) -> Option<usize> {
    TrackKind::all()
        .iter()
        .position(|track| track.storage_key() == track_key)
}

fn parse_setting_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

struct App {
    catalog: Catalog,
    scan_roots: Vec<PathBuf>,
    summary: CatalogSummary,
    installs: Vec<CatalogInstall>,
    all_projects: Vec<CatalogProject>,
    projects: Vec<CatalogProject>,
    all_versions: Vec<CatalogVersion>,
    versions: Vec<CatalogVersion>,
    version_context: Option<CatalogVersionContext>,
    diff_preview: Option<ProjectDiffPreview>,
    backup_history: Vec<CatalogSyncEvent>,
    version_context_cache: HashMap<(String, String), CatalogVersionContext>,
    diff_preview_cache: HashMap<(i64, String), ProjectDiffPreview>,
    backup_history_cache: HashMap<i64, Vec<CatalogSyncEvent>>,
    selected_project: usize,
    selected_version: usize,
    selected_diff_file: usize,
    selected_backup: usize,
    diff_preview_pending: bool,
    diff_preview_requested_at: Option<Instant>,
    focus: Focus,
    input_mode: InputMode,
    project_filter: String,
    version_filter: String,
    theme_index: usize,
    track_index: usize,
    music_enabled: bool,
    show_help: bool,
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
            all_projects: Vec::new(),
            projects: Vec::new(),
            all_versions: Vec::new(),
            versions: Vec::new(),
            version_context: None,
            diff_preview: None,
            backup_history: Vec::new(),
            version_context_cache: HashMap::new(),
            diff_preview_cache: HashMap::new(),
            backup_history_cache: HashMap::new(),
            selected_project: 0,
            selected_version: 0,
            selected_diff_file: 0,
            selected_backup: 0,
            diff_preview_pending: false,
            diff_preview_requested_at: None,
            focus: Focus::Projects,
            input_mode: InputMode::Normal,
            project_filter: String::new(),
            version_filter: String::new(),
            theme_index: 0,
            track_index: 0,
            music_enabled: true,
            show_help: false,
            lofi: None,
            music_started_at: None,
            status: "starting catalog bootstrap".to_string(),
        };
        app.restore_ui_preferences()?;
        app.refresh()?;
        app.sync_on_boot();
        app.start_lofi_default();
        Ok(app)
    }

    fn current_theme(&self) -> Theme {
        THEMES[self.theme_index % THEMES.len()]
    }

    fn current_track(&self) -> TrackKind {
        TrackKind::all()[self.track_index % TrackKind::all().len()]
    }

    fn restore_ui_preferences(&mut self) -> Result<()> {
        if let Some(theme_id) = self.catalog.app_setting(UI_THEME_SETTING_KEY)? {
            if let Some(index) = theme_index_by_id(&theme_id) {
                self.theme_index = index;
            }
        }
        if let Some(track_key) = self.catalog.app_setting(UI_TRACK_SETTING_KEY)? {
            if let Some(index) = track_index_by_key(&track_key) {
                self.track_index = index;
            }
        }
        if let Some(value) = self.catalog.app_setting(UI_MUSIC_SETTING_KEY)? {
            self.music_enabled = parse_setting_bool(&value).unwrap_or(true);
        }
        Ok(())
    }

    fn persist_ui_preferences(&mut self) {
        if let Err(error) = self
            .catalog
            .set_app_setting(UI_THEME_SETTING_KEY, Some(self.current_theme().id))
        {
            self.status = format!("failed to save theme preference: {error}");
            return;
        }
        if let Err(error) = self.catalog.set_app_setting(
            UI_TRACK_SETTING_KEY,
            Some(self.current_track().storage_key()),
        ) {
            self.status = format!("failed to save track preference: {error}");
            return;
        }
        if let Err(error) = self.catalog.set_app_setting(
            UI_MUSIC_SETTING_KEY,
            Some(if self.music_enabled { "true" } else { "false" }),
        ) {
            self.status = format!("failed to save music preference: {error}");
        }
    }

    fn current_filter(&self, focus: Focus) -> &str {
        match focus {
            Focus::Projects => &self.project_filter,
            Focus::Versions => &self.version_filter,
        }
    }

    fn current_filter_mut(&mut self, focus: Focus) -> &mut String {
        match focus {
            Focus::Projects => &mut self.project_filter,
            Focus::Versions => &mut self.version_filter,
        }
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
        self.all_projects = self.catalog.list_projects()?;
        self.all_versions = self.catalog.list_versions(None).unwrap_or_default();
        self.version_context_cache.clear();
        self.diff_preview_cache.clear();
        self.backup_history_cache.clear();
        self.apply_filters()?;
        self.refresh_pending_diff_preview(true)
    }

    fn apply_filters(&mut self) -> Result<()> {
        let selected_project_id = self.selected_project().map(|project| project.id);
        let selected_version_sha = self
            .selected_version()
            .map(|version| version.commit_sha.clone());
        let previous_project_index = self.selected_project;
        let previous_version_index = self.selected_version;

        self.projects = filter_projects(&self.all_projects, &self.project_filter);
        self.versions = filter_versions(&self.all_versions, &self.version_filter);

        self.selected_project =
            restore_project_selection(&self.projects, selected_project_id, previous_project_index);
        self.selected_version = restore_version_selection(
            &self.versions,
            selected_version_sha.as_deref(),
            previous_version_index,
        );
        self.refresh_selection_views()
    }

    fn refresh_selection_views(&mut self) -> Result<()> {
        self.refresh_version_context()?;
        self.refresh_backup_history()?;
        self.queue_diff_preview_refresh();
        Ok(())
    }

    fn refresh_version_context(&mut self) -> Result<()> {
        let current_commit_sha = self
            .install_for_selected_project()
            .and_then(|install| install.matched_upstream_commit_sha.clone());
        let selected_commit_sha = self
            .selected_version()
            .map(|version| version.commit_sha.clone());
        self.version_context = if let Some(selected_commit_sha) = selected_commit_sha {
            let cache_key = (
                current_commit_sha.clone().unwrap_or_default(),
                selected_commit_sha.clone(),
            );
            if let Some(context) = self.version_context_cache.get(&cache_key) {
                Some(context.clone())
            } else {
                let context = self.catalog.version_context(
                    current_commit_sha.as_deref(),
                    &selected_commit_sha,
                    12,
                )?;
                if let Some(context) = context.as_ref() {
                    self.version_context_cache
                        .insert(cache_key, context.clone());
                }
                context
            }
        } else {
            None
        };
        Ok(())
    }

    fn current_diff_preview_key(&self) -> Option<(i64, String)> {
        let project = self.selected_project()?;
        let version = self.selected_version()?;
        Some((project.id, version.commit_sha.clone()))
    }

    fn queue_diff_preview_refresh(&mut self) {
        let current_key = self.current_diff_preview_key();
        let active_key = self
            .diff_preview
            .as_ref()
            .map(|preview| (preview.project.id, preview.commit_sha.clone()));

        match current_key {
            Some(key) if active_key.as_ref() == Some(&key) && !self.diff_preview_pending => {}
            Some(key) => {
                self.selected_diff_file = 0;
                if let Some(cached) = self.diff_preview_cache.get(&key) {
                    self.diff_preview = Some(cached.clone());
                    self.diff_preview_pending = false;
                    self.diff_preview_requested_at = None;
                } else {
                    self.diff_preview = None;
                    self.diff_preview_pending = true;
                    self.diff_preview_requested_at = Some(Instant::now());
                }
            }
            None => {
                self.diff_preview = None;
                self.diff_preview_pending = false;
                self.diff_preview_requested_at = None;
                self.selected_diff_file = 0;
            }
        }
    }

    fn refresh_pending_diff_preview(&mut self, force: bool) -> Result<()> {
        if !self.diff_preview_pending {
            return Ok(());
        }

        if !force
            && self
                .diff_preview_requested_at
                .map(|started_at| started_at.elapsed() < DIFF_PREVIEW_DEBOUNCE)
                .unwrap_or(false)
        {
            return Ok(());
        }

        let Some((project_id, commit_sha)) = self.current_diff_preview_key() else {
            self.diff_preview = None;
            self.diff_preview_pending = false;
            self.diff_preview_requested_at = None;
            return Ok(());
        };

        let preview = project_diff_preview(
            &self.catalog,
            &project_id.to_string(),
            None,
            Some(&commit_sha),
            8,
            10,
        )?;
        self.selected_diff_file = self
            .selected_diff_file
            .min(preview.files.len().saturating_sub(1));
        self.diff_preview_cache
            .insert((project_id, commit_sha), preview.clone());
        self.diff_preview = Some(preview);
        self.diff_preview_pending = false;
        self.diff_preview_requested_at = None;
        Ok(())
    }

    fn refresh_backup_history(&mut self) -> Result<()> {
        self.backup_history = if let Some(project) = self.selected_project() {
            if let Some(history) = self.backup_history_cache.get(&project.id) {
                history.clone()
            } else {
                let history = self
                    .catalog
                    .project_backup_history(&project.id.to_string(), 20)?;
                self.backup_history_cache
                    .insert(project.id, history.clone());
                history
            }
        } else {
            Vec::new()
        };
        self.selected_backup = self
            .selected_backup
            .min(self.backup_history.len().saturating_sub(1));
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

    fn begin_filter(&mut self) {
        self.input_mode = InputMode::Filtering(self.focus);
        self.status = format!(
            "filtering {}: type to search, enter or esc to finish, ctrl+u to clear",
            self.focus.label()
        );
    }

    fn finish_filter(&mut self, focus: Focus) {
        self.input_mode = InputMode::Normal;
        let filter = self.current_filter(focus).trim();
        self.status = if filter.is_empty() {
            format!("{} filter cleared", focus.label())
        } else {
            format!("{} filter active: {}", focus.label(), filter)
        };
    }

    fn clear_filter(&mut self, focus: Focus) {
        self.current_filter_mut(focus).clear();
        if let Err(error) = self.apply_filters() {
            self.status = format!("failed to clear {} filter: {error}", focus.label());
            return;
        }
        self.status = format!("{} filter cleared", focus.label());
    }

    fn push_filter_char(&mut self, focus: Focus, ch: char) {
        self.current_filter_mut(focus).push(ch);
        if let Err(error) = self.apply_filters() {
            self.status = format!("failed to update {} filter: {error}", focus.label());
            return;
        }
        let filter = self.current_filter(focus).trim();
        self.status = format!("{} filter: {}", focus.label(), filter);
    }

    fn pop_filter_char(&mut self, focus: Focus) {
        self.current_filter_mut(focus).pop();
        if let Err(error) = self.apply_filters() {
            self.status = format!("failed to update {} filter: {error}", focus.label());
            return;
        }
        let filter = self.current_filter(focus).trim();
        self.status = if filter.is_empty() {
            format!("{} filter cleared", focus.label())
        } else {
            format!("{} filter: {}", focus.label(), filter)
        };
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

    fn revert_selected(&mut self, dry_run: bool) {
        let Some(project) = self.selected_project().cloned() else {
            self.status = "no project selected".to_string();
            return;
        };
        let event_id = self.selected_backup_event().map(|event| event.id);
        match revert_projects(&self.catalog, &[project.id.to_string()], event_id, dry_run) {
            Ok(results) => {
                if let Some(result) = results.first() {
                    self.status = format!(
                        "{} restored={} removed={} source={}",
                        if dry_run {
                            "revert dry-run"
                        } else {
                            "reverted"
                        },
                        result.restored_files.len(),
                        result.removed_files.len(),
                        result
                            .restored_from_backup_path
                            .clone()
                            .unwrap_or_else(|| "-".to_string())
                    );
                } else {
                    self.status = "no matching revert result".to_string();
                }
            }
            Err(error) => {
                self.status = format!("revert failed: {error}");
            }
        }
        let _ = self.refresh();
    }

    fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
        self.status = if self.show_help {
            "help open: press h, ?, enter, or esc to close".to_string()
        } else {
            "help closed".to_string()
        };
    }

    fn start_track(&mut self, track: TrackKind, user_facing_message: &str) {
        match LofiPlayer::start(track) {
            Ok(player) => {
                self.lofi = Some(player);
                self.music_started_at = Some(Instant::now());
                self.status = format!("{user_facing_message}: {}", track.name());
            }
            Err(error) => {
                self.lofi = None;
                self.music_started_at = None;
                self.status = format!("music unavailable: {error}");
            }
        }
    }

    fn start_lofi_default(&mut self) {
        if self.music_enabled {
            self.start_track(self.current_track(), "music on");
        }
    }

    fn toggle_lofi(&mut self) {
        if self.lofi.take().is_some() {
            self.music_enabled = false;
            self.music_started_at = None;
            self.status = "music off".to_string();
        } else {
            self.music_enabled = true;
            self.start_track(self.current_track(), "music on");
        }
        self.persist_ui_preferences();
    }

    fn cycle_track(&mut self) {
        self.track_index = (self.track_index + 1) % TrackKind::all().len();
        let track = self.current_track();
        if self.lofi.is_some() {
            self.lofi = None;
            self.start_track(track, "switched track");
        } else {
            self.status = format!("selected track: {}", track.name());
        }
        self.persist_ui_preferences();
    }

    fn cycle_theme(&mut self) {
        self.theme_index = (self.theme_index + 1) % THEMES.len();
        self.status = format!("theme: {}", self.current_theme().name);
        self.persist_ui_preferences();
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

    fn selected_diff_file(&self) -> Option<&DiffPreviewFile> {
        self.diff_preview
            .as_ref()
            .and_then(|preview| preview.files.get(self.selected_diff_file))
    }

    fn selected_backup_event(&self) -> Option<&CatalogSyncEvent> {
        self.backup_history.get(self.selected_backup)
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
        let _ = self.refresh_selection_views();
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
        let _ = self.refresh_selection_views();
    }

    fn next_diff_file(&mut self) {
        if self.diff_preview_pending {
            self.status = "diff preview updating...".to_string();
            return;
        }
        let Some(preview) = self.diff_preview.as_ref() else {
            self.status = "no changed files to preview".to_string();
            return;
        };
        if preview.files.is_empty() {
            self.status = "no changed files to preview".to_string();
            return;
        }
        self.selected_diff_file = (self.selected_diff_file + 1) % preview.files.len();
        if let Some(file) = self.selected_diff_file() {
            self.status = format!(
                "diff preview {}/{}: {}",
                self.selected_diff_file + 1,
                preview.files.len(),
                file.path
            );
        }
    }

    fn previous_diff_file(&mut self) {
        if self.diff_preview_pending {
            self.status = "diff preview updating...".to_string();
            return;
        }
        let Some(preview) = self.diff_preview.as_ref() else {
            self.status = "no changed files to preview".to_string();
            return;
        };
        if preview.files.is_empty() {
            self.status = "no changed files to preview".to_string();
            return;
        }
        self.selected_diff_file = if self.selected_diff_file == 0 {
            preview.files.len() - 1
        } else {
            self.selected_diff_file - 1
        };
        if let Some(file) = self.selected_diff_file() {
            self.status = format!(
                "diff preview {}/{}: {}",
                self.selected_diff_file + 1,
                preview.files.len(),
                file.path
            );
        }
    }

    fn cycle_backup_event(&mut self) {
        if self.backup_history.is_empty() {
            self.status = "no backup history for selected project".to_string();
            return;
        }
        self.selected_backup = (self.selected_backup + 1) % self.backup_history.len();
        if let Some(event) = self.selected_backup_event() {
            self.status = format!(
                "backup {}/{}: event #{} {}",
                self.selected_backup + 1,
                self.backup_history.len(),
                event.id,
                event.created_at
            );
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

fn block(title: &str, theme: Theme, focused: bool) -> Block<'static> {
    let border = if focused { theme.accent } else { theme.muted };
    Block::default()
        .title(Line::from(Span::styled(
            title.to_string(),
            Style::default().fg(theme.accent_soft),
        )))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border))
}

fn render_list<'a>(
    title: &'a str,
    items: Vec<ListItem<'a>>,
    selected: Option<usize>,
    focused: bool,
    theme: Theme,
) -> (List<'a>, ListState) {
    let list = List::new(items)
        .block(block(title, theme, focused))
        .highlight_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = ListState::default();
    state.select(selected);
    (list, state)
}

fn trim_query(query: &str) -> &str {
    query.trim()
}

fn matches_query(fields: &[String], query: &str) -> bool {
    let terms = query
        .split_whitespace()
        .map(|term| term.to_ascii_lowercase())
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>();
    if terms.is_empty() {
        return true;
    }
    let haystack = fields.join("\n").to_ascii_lowercase();
    terms.iter().all(|term| haystack.contains(term))
}

fn project_state_key(project: &CatalogProject) -> &'static str {
    match project.effective_gstack_source.as_str() {
        "local_install" => "local_install",
        "global_install" => "global_install",
        "none" if project.has_git_repo => "ready_to_install",
        "none" => "configured_without_install",
        _ => "unknown",
    }
}

fn project_state_rank(project: &CatalogProject) -> usize {
    match project_state_key(project) {
        "local_install" => 0,
        "global_install" => 1,
        "ready_to_install" => 2,
        "configured_without_install" => 3,
        _ => 4,
    }
}

fn project_status_label(project: &CatalogProject) -> String {
    match project_state_key(project) {
        "local_install" => project
            .effective_gstack_version
            .as_ref()
            .map(|version| format!("local {version}"))
            .unwrap_or_else(|| "local".to_string()),
        "global_install" => project
            .effective_gstack_version
            .as_ref()
            .map(|version| format!("global {version}"))
            .unwrap_or_else(|| "global".to_string()),
        "ready_to_install" => "ready".to_string(),
        "configured_without_install" => "configured".to_string(),
        _ => "unknown".to_string(),
    }
}

fn project_current_gstack_line(project: &CatalogProject) -> String {
    match project_state_key(project) {
        "local_install" => format!(
            "Current gstack: repo-local {} install={}",
            project
                .effective_gstack_version
                .clone()
                .unwrap_or_else(|| "unmapped".to_string()),
            project
                .gstack_install_observed_path
                .clone()
                .unwrap_or_else(|| "-".to_string())
        ),
        "global_install" => format!(
            "Current gstack: inherited global {}",
            project
                .effective_gstack_version
                .clone()
                .unwrap_or_else(|| "unmapped".to_string())
        ),
        "ready_to_install" => {
            "Current gstack: no install yet; this repo is ready for a local gstack install"
                .to_string()
        }
        "configured_without_install" => {
            "Current gstack: Claude/Codex settings detected, but no gstack install is cataloged yet"
                .to_string()
        }
        _ => format!(
            "Current gstack: source={} version={} install={}",
            project.effective_gstack_source,
            project
                .effective_gstack_version
                .clone()
                .unwrap_or_else(|| "-".to_string()),
            project
                .gstack_install_observed_path
                .clone()
                .unwrap_or_else(|| "-".to_string())
        ),
    }
}

fn filter_projects(projects: &[CatalogProject], query: &str) -> Vec<CatalogProject> {
    let query = trim_query(query);
    let mut filtered = projects
        .iter()
        .filter(|project| {
            matches_query(
                &[
                    project.id.to_string(),
                    project.name.clone(),
                    project.canonical_path.clone(),
                    project
                        .effective_gstack_version
                        .clone()
                        .unwrap_or_else(|| "not_installed".to_string()),
                    project_status_label(project),
                    project_state_key(project).to_string(),
                    project.effective_gstack_source.clone(),
                    if project.has_git_repo {
                        "git_repo".to_string()
                    } else {
                        "marker_only".to_string()
                    },
                    project
                        .gstack_install_observed_path
                        .clone()
                        .unwrap_or_default(),
                    project.claude_settings_paths.join(" "),
                    project.codex_settings_paths.join(" "),
                    project.git_remote.clone().unwrap_or_default(),
                ],
                query,
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    filtered.sort_by_key(|project| {
        (
            project_state_rank(project),
            project.name.to_ascii_lowercase(),
            project.canonical_path.to_ascii_lowercase(),
        )
    });
    filtered
}

fn filter_versions(versions: &[CatalogVersion], query: &str) -> Vec<CatalogVersion> {
    let query = trim_query(query);
    if query.is_empty() {
        return versions.to_vec();
    }
    versions
        .iter()
        .filter(|version| {
            matches_query(
                &[
                    version.version.clone(),
                    version.commit_sha.clone(),
                    version.committed_at.clone(),
                    version.subject.clone(),
                    version.body.clone(),
                ],
                query,
            )
        })
        .cloned()
        .collect()
}

fn restore_project_selection(
    projects: &[CatalogProject],
    selected_project_id: Option<i64>,
    previous_index: usize,
) -> usize {
    if projects.is_empty() {
        return 0;
    }
    selected_project_id
        .and_then(|project_id| projects.iter().position(|project| project.id == project_id))
        .unwrap_or_else(|| previous_index.min(projects.len().saturating_sub(1)))
}

fn restore_version_selection(
    versions: &[CatalogVersion],
    selected_commit_sha: Option<&str>,
    previous_index: usize,
) -> usize {
    if versions.is_empty() {
        return 0;
    }
    selected_commit_sha
        .and_then(|commit_sha| {
            versions
                .iter()
                .position(|version| version.commit_sha == commit_sha)
        })
        .unwrap_or_else(|| previous_index.min(versions.len().saturating_sub(1)))
}

fn truncate_inline(value: &str, max_chars: usize) -> String {
    let trimmed = value.trim();
    let count = trimmed.chars().count();
    if count <= max_chars {
        return trimmed.to_string();
    }
    let mut truncated = trimmed
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

fn display_filter(filter: &str) -> String {
    let filter = trim_query(filter);
    if filter.is_empty() {
        "-".to_string()
    } else {
        truncate_inline(filter, 20)
    }
}

fn pane_title(base: &str, visible: usize, total: usize, filter: &str, editing: bool) -> String {
    let mut title = format!("{base} {visible}/{total}");
    let filter = trim_query(filter);
    if !filter.is_empty() {
        title.push_str(" | /");
        title.push_str(&truncate_inline(filter, 18));
    }
    if editing {
        title.push_str(" *");
    }
    title
}

fn centered_area(
    area: ratatui::layout::Rect,
    horizontal_percent: u16,
    vertical_percent: u16,
) -> ratatui::layout::Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - vertical_percent) / 2),
            Constraint::Percentage(vertical_percent),
            Constraint::Percentage((100 - vertical_percent) / 2),
        ])
        .split(area);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - horizontal_percent) / 2),
            Constraint::Percentage(horizontal_percent),
            Constraint::Percentage((100 - horizontal_percent) / 2),
        ])
        .split(vertical[1]);
    horizontal[1]
}

fn render(app: &mut App, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    terminal.draw(|frame| {
        let theme = app.current_theme();
        let areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(8),
                Constraint::Min(12),
                Constraint::Length(11),
            ])
            .split(frame.area());

        let summary_areas = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(78), Constraint::Percentage(22)])
            .split(areas[0]);

        let summary_text = vec![
            Line::from(vec![
                Span::styled(
                    "Source ",
                    Style::default()
                        .fg(theme.accent_soft)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    app.summary
                        .source
                        .head_version
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string()),
                    Style::default().fg(theme.text),
                ),
                Span::raw("  "),
                Span::styled(
                    "Head ",
                    Style::default()
                        .fg(theme.accent_soft)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    app.summary
                        .source
                        .head_commit_sha
                        .clone()
                        .map(|sha| sha.chars().take(12).collect::<String>())
                        .unwrap_or_else(|| "none".to_string()),
                    Style::default().fg(theme.text),
                ),
            ]),
            Line::from(format!(
                "Projects: {}  With local gstack: {}  Installs: {}  Outdated: {}  Last scan: {}",
                app.summary.total_projects,
                app.summary.projects_with_local_gstack,
                app.summary.total_installs,
                app.summary.outdated_installs,
                app.summary
                    .last_scan_at
                    .clone()
                    .unwrap_or_else(|| "never".to_string()),
            )),
            Line::from(format!(
                "Theme: {}  Track: {}  Music: {}",
                theme.name,
                app.current_track().name(),
                if app.lofi.is_some() { "on" } else { "off" }
            )),
            Line::from(
                format!(
                    "Search: projects=/{}, versions=/{}{}",
                    display_filter(&app.project_filter),
                    display_filter(&app.version_filter),
                    match app.input_mode {
                        InputMode::Filtering(focus) => {
                            format!("  editing {}", focus.label())
                        }
                        InputMode::Normal => String::new(),
                    }
                ),
            ),
            Line::from(
                "Keys: q quit | h help | g sync | / filter | f clear | m music | t track | c theme | tab switch | j/k move | ←/→ diff | d dry-run | a apply | b backup | z revert dry-run | x revert | r refresh",
            ),
            Line::from(Span::styled(
                app.status.clone(),
                Style::default().fg(theme.accent),
            )),
        ];
        let summary = Paragraph::new(summary_text)
            .block(block("Summary", theme, false))
            .wrap(Wrap { trim: true });
        frame.render_widget(summary, summary_areas[0]);

        let visualizer = Paragraph::new(build_visualizer_lines(app, theme))
            .block(block("Visualizer", theme, false))
            .wrap(Wrap { trim: true });
        frame.render_widget(visualizer, summary_areas[1]);

        let middle = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(41),
                Constraint::Percentage(33),
                Constraint::Percentage(26),
            ])
            .split(areas[1]);

        let project_items = if app.projects.is_empty() {
            vec![ListItem::new("No git/Claude/Codex projects in catalog.")]
        } else {
            app.projects
                .iter()
                .map(|project| {
                    ListItem::new(format!(
                        "{} [{}] {}",
                        project.name,
                        project_status_label(project),
                        project.canonical_path
                    ))
                })
                .collect::<Vec<_>>()
        };
        let projects_title = pane_title(
            "Projects",
            app.projects.len(),
            app.all_projects.len(),
            &app.project_filter,
            app.input_mode == InputMode::Filtering(Focus::Projects),
        );
        let (projects_list, mut projects_state) = render_list(
            &projects_title,
            project_items,
            (!app.projects.is_empty()).then_some(app.selected_project),
            app.focus == Focus::Projects,
            theme,
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
                        truncate_inline(&version.subject, 44)
                    ))
                })
                .collect::<Vec<_>>()
        };
        let versions_title = pane_title(
            "Versions",
            app.versions.len(),
            app.all_versions.len(),
            &app.version_filter,
            app.input_mode == InputMode::Filtering(Focus::Versions),
        );
        let (versions_list, mut versions_state) = render_list(
            &versions_title,
            version_items,
            (!app.versions.is_empty()).then_some(app.selected_version),
            app.focus == Focus::Versions,
            theme,
        );
        frame.render_stateful_widget(versions_list, middle[1], &mut versions_state);

        let notes = Paragraph::new(build_version_notes(app, theme))
            .block(block("Selected Version", theme, false))
            .wrap(Wrap { trim: true });
        frame.render_widget(notes, middle[2]);

        let detail = Paragraph::new(build_project_detail(app, theme))
            .block(block("Project Detail", theme, false))
            .wrap(Wrap { trim: true });
        frame.render_widget(detail, areas[2]);

        if app.show_help {
            let help_area = centered_area(frame.area(), 62, 68);
            let help = Paragraph::new(build_help_lines(app, theme))
                .block(block("Help", theme, true))
                .wrap(Wrap { trim: true });
            frame.render_widget(Clear, help_area);
            frame.render_widget(help, help_area);
        }
    })?;
    Ok(())
}

fn build_visualizer_lines(app: &App, theme: Theme) -> Vec<Line<'static>> {
    let track_label = app.current_track().name().to_string();
    if app.lofi.is_none() {
        return vec![
            Line::from(vec![
                Span::styled("track ", Style::default().fg(theme.muted)),
                Span::styled(track_label, Style::default().fg(theme.accent_soft)),
            ]),
            Line::from(Span::styled("music off", Style::default().fg(theme.muted))),
            Line::from(vec![
                Span::styled("[  ]", Style::default().fg(theme.muted)),
                Span::styled("[  ]", Style::default().fg(theme.muted)),
                Span::styled("[  ]", Style::default().fg(theme.muted)),
                Span::styled("[  ]", Style::default().fg(theme.muted)),
            ]),
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
    lines.push(Line::from(vec![
        Span::styled("track ", Style::default().fg(theme.muted)),
        Span::styled(track_label, Style::default().fg(theme.accent_soft)),
    ]));
    for row in (1..=height).rev() {
        let mut spans = Vec::new();
        for level in &levels {
            let filled = *level >= row;
            spans.push(Span::styled(
                if filled { "[##]" } else { "[  ]" },
                Style::default().fg(if filled { theme.accent } else { theme.muted }),
            ));
        }
        lines.push(Line::from(spans));
    }
    lines
}

fn build_help_lines(app: &App, theme: Theme) -> Vec<Line<'static>> {
    let focus_label = app.focus.label();
    let project_filter = display_filter(&app.project_filter);
    let version_filter = display_filter(&app.version_filter);

    vec![
        Line::from(Span::styled(
            "Workspace navigator for local gstack installs and upstream versions.",
            Style::default()
                .fg(theme.accent_soft)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("Navigation"),
        Line::from(format!(
            "  tab switches focus between Projects and Versions. Current focus: {focus_label}."
        )),
        Line::from("  j / k or arrow keys move the current selection."),
        Line::from("  left / right cycles the selected file diff preview."),
        Line::from("  h, ?, enter, or esc closes this help modal."),
        Line::from(""),
        Line::from("Filters"),
        Line::from(format!(
            "  / starts filtering the focused pane. Active filters: projects=/{project_filter}, versions=/{version_filter}."
        )),
        Line::from(
            "  Type while filtering for live search across names, paths, versions, SHAs, and commit text.",
        ),
        Line::from(
            "  f clears the focused pane filter. ctrl+u clears while you are actively typing a filter.",
        ),
        Line::from(""),
        Line::from("Actions"),
        Line::from("  g syncs upstream gstack history and rescans local projects."),
        Line::from("  d dry-runs the selected version against the selected project."),
        Line::from("  a applies the selected version with merge-aware updates and backups."),
        Line::from("  b cycles backup-history entries for the selected project."),
        Line::from("  z dry-runs a revert from the selected backup entry."),
        Line::from("  x restores the selected backup entry and creates a new backup first."),
        Line::from("  r refreshes the catalog view from SQLite."),
        Line::from(""),
        Line::from("Audio and Theme"),
        Line::from(format!(
            "  m toggles music. Current track: {}. Current theme: {}.",
            app.current_track().name(),
            app.current_theme().name
        )),
        Line::from("  t cycles tracks. c cycles terminal color palettes."),
        Line::from("  Theme, track, and music on/off now persist across sessions."),
        Line::from(""),
        Line::from("Panes"),
        Line::from(
            "  Projects lists repos sorted by install state, then name. Row labels are local, global, ready, or configured.",
        ),
        Line::from(
            "  Versions lists upstream gstack versions from SQLite; the right pane shows commit context and a real file diff preview.",
        ),
        Line::from(
            "  Project Detail shows install state, selected backup history, and the current revert/apply target.",
        ),
    ]
}

fn excerpt_body(body: &str, max_lines: usize) -> Vec<String> {
    body.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(max_lines)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>()
}

fn short_commit_line(commit: &CatalogCommitNote) -> String {
    match &commit.version {
        Some(version) => format!("{version} {}", commit.subject),
        None => format!(
            "{} {}",
            commit.commit_sha.chars().take(8).collect::<String>(),
            commit.subject
        ),
    }
}

fn build_version_notes(app: &App, theme: Theme) -> Vec<Line<'static>> {
    let Some(selected_version) = app.selected_version() else {
        return vec![Line::from("Select an upstream version to inspect it.")];
    };
    let Some(context) = app.version_context.as_ref() else {
        return vec![Line::from("No version context available yet.")];
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Version ", Style::default().fg(theme.muted)),
            Span::styled(
                selected_version.version.clone(),
                Style::default()
                    .fg(theme.accent_soft)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(format!(
            "{}  {}",
            context
                .selected
                .commit_sha
                .chars()
                .take(12)
                .collect::<String>(),
            context.selected.committed_at
        )),
        Line::from(context.selected.subject.clone()),
    ];

    for line in excerpt_body(&context.selected.body, 2) {
        lines.push(Line::from(Span::styled(
            line,
            Style::default().fg(theme.text),
        )));
    }

    lines.push(Line::from(""));
    let path_label = match context.direction.as_str() {
        "upgrade" => format!("Upgrade path: {} commit(s)", context.path_commits.len()),
        "downgrade" => format!("Rollback path: {} commit(s)", context.path_commits.len()),
        "current" => "Already on this upstream commit".to_string(),
        "preview" => "No matched upstream commit for this project yet".to_string(),
        _ => "Selected commit is not on the same first-parent path".to_string(),
    };
    lines.push(Line::from(Span::styled(
        path_label,
        Style::default().fg(theme.accent),
    )));

    let path_commits = if context.direction == "downgrade" {
        context
            .path_commits
            .iter()
            .rev()
            .take(4)
            .cloned()
            .collect::<Vec<_>>()
    } else {
        context
            .path_commits
            .iter()
            .take(4)
            .cloned()
            .collect::<Vec<_>>()
    };
    for commit in path_commits {
        lines.push(Line::from(format!("• {}", short_commit_line(&commit))));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!(
            "File delta: +{}  ~{}  -{}",
            context.diff.added_paths.len(),
            context.diff.updated_paths.len(),
            context.diff.removed_paths.len()
        ),
        Style::default().fg(theme.accent),
    )));

    if app.diff_preview_pending {
        lines.push(Line::from(Span::styled(
            "Diff preview updating...",
            Style::default().fg(theme.muted),
        )));
        return lines;
    }

    let Some(diff_preview) = app.diff_preview.as_ref() else {
        lines.push(Line::from("Diff preview unavailable."));
        return lines;
    };
    if diff_preview.files.is_empty() {
        lines.push(Line::from("No file content changes for this target."));
        return lines;
    }

    let Some(file) = app.selected_diff_file() else {
        lines.push(Line::from("No diff file selected."));
        return lines;
    };
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!(
            "Preview file {}/{} [{}] {}",
            app.selected_diff_file + 1,
            diff_preview.files.len(),
            file.change_type,
            file.path
        ),
        Style::default().fg(theme.accent_soft),
    )));
    for preview_line in &file.preview_lines {
        let fg = if preview_line.starts_with("+ ") || preview_line.starts_with("+++") {
            theme.accent
        } else if preview_line.starts_with("- ") || preview_line.starts_with("---") {
            Color::LightRed
        } else if preview_line.contains("unchanged line(s)") {
            theme.muted
        } else {
            theme.text
        };
        lines.push(Line::from(Span::styled(
            preview_line.clone(),
            Style::default().fg(fg),
        )));
    }
    if file.truncated {
        lines.push(Line::from(Span::styled(
            "... diff preview truncated ...",
            Style::default().fg(theme.muted),
        )));
    }

    lines
}

fn build_project_detail(app: &App, theme: Theme) -> Vec<Line<'static>> {
    let Some(project) = app.selected_project() else {
        return vec![Line::from("No project selected.")];
    };
    let install = app.install_for_selected_project();
    let target = app.selected_version();
    let context = app.version_context.as_ref();
    let mut lines = vec![
        Line::from(format!("Project: {}", project.name)),
        Line::from(format!("Path: {}", project.canonical_path)),
        Line::from(format!(
            "Repository: git_repo={} remote={}",
            project.has_git_repo,
            project
                .git_remote
                .clone()
                .unwrap_or_else(|| "-".to_string())
        )),
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
            "Codex markers: AGENTS.md={} .codex={} .agents={} settings={} paths={}",
            project.has_agents_md,
            project.has_codex_dir,
            project.has_agents_dir,
            project.has_codex_settings,
            if project.codex_settings_paths.is_empty() {
                "-".to_string()
            } else {
                project.codex_settings_paths.join(", ")
            }
        )),
        Line::from(project_current_gstack_line(project)),
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
                    app.summary
                        .source
                        .head_commit_sha
                        .as_ref()
                        .map(|sha| sha.chars().take(12).collect::<String>())
                })
                .unwrap_or_else(|| "none".to_string())
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
            None => {
                "Install state: no repo-local install cataloged; apply will materialize one if needed"
                    .to_string()
            }
        }),
    ];

    lines.push(Line::from(match app.selected_backup_event() {
        Some(event) => format!(
            "Backup history: {}/{} selected event #{} {} version={} files={}",
            app.selected_backup + 1,
            app.backup_history.len(),
            event.id,
            event.created_at,
            event.version.clone().unwrap_or_else(|| event
                .commit_sha
                .chars()
                .take(12)
                .collect::<String>()),
            event.changed_files.len()
        ),
        None => "Backup history: no revertable backups cataloged yet".to_string(),
    }));
    lines.push(Line::from(match app.selected_backup_event() {
        Some(event) => format!(
            "Revert controls: b cycle history | z dry-run revert | x revert from {}",
            event.backup_path.clone().unwrap_or_else(|| "-".to_string())
        ),
        None => "Revert controls: b cycle history once backups exist".to_string(),
    }));
    lines.push(Line::from(match app.selected_diff_file() {
        Some(file) => format!(
            "Diff controls: left/right preview files ({}/{}) current={}",
            app.selected_diff_file + 1,
            app.diff_preview
                .as_ref()
                .map(|preview| preview.files.len())
                .unwrap_or(0),
            file.path
        ),
        None if app.diff_preview_pending => {
            "Diff controls: preview is updating for the current selection".to_string()
        }
        None => "Diff controls: no changed files in the current preview".to_string(),
    }));
    lines.push(Line::from(match context.map(|ctx| ctx.direction.as_str()) {
        Some("upgrade") => Span::styled(
            "Change view: showing commits you would gain plus file-level delta to the selected version",
            Style::default().fg(theme.accent),
        ),
        Some("downgrade") => Span::styled(
            "Change view: showing commits you would roll back plus file-level delta to the selected version",
            Style::default().fg(theme.accent),
        ),
        Some("current") => Span::styled(
            "Change view: selected version already matches the project's mapped upstream commit",
            Style::default().fg(theme.muted),
        ),
        Some("preview") => Span::styled(
            "Change view: this project has no mapped upstream base, so the diff is a new-install preview",
            Style::default().fg(theme.muted),
        ),
        _ => Span::styled(
            "Change view: the selected commit is outside this project's known first-parent path",
            Style::default().fg(theme.muted),
        ),
    }));

    lines
}

fn run_loop(app: &mut App, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    loop {
        if let Err(error) = app.refresh_pending_diff_preview(false) {
            app.status = format!("diff preview refresh failed: {error}");
        }
        render(app, terminal)?;
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if app.show_help {
                    match key.code {
                        KeyCode::Char('h') | KeyCode::Char('?') | KeyCode::Esc | KeyCode::Enter => {
                            app.toggle_help()
                        }
                        _ => {}
                    }
                    continue;
                }

                match app.input_mode {
                    InputMode::Normal => match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Char('h') | KeyCode::Char('?') => app.toggle_help(),
                        KeyCode::Char('g') => app.sync_now(),
                        KeyCode::Char('/') => app.begin_filter(),
                        KeyCode::Char('f') => app.clear_filter(app.focus),
                        KeyCode::Char('m') => app.toggle_lofi(),
                        KeyCode::Char('t') => app.cycle_track(),
                        KeyCode::Char('c') => app.cycle_theme(),
                        KeyCode::Char('a') => app.apply_selected(false),
                        KeyCode::Char('d') => app.apply_selected(true),
                        KeyCode::Char('b') => app.cycle_backup_event(),
                        KeyCode::Char('z') => app.revert_selected(true),
                        KeyCode::Char('x') => app.revert_selected(false),
                        KeyCode::Char('r') => {
                            if let Err(error) = app.refresh() {
                                app.status = format!("refresh failed: {error}");
                            }
                        }
                        KeyCode::Tab => app.toggle_focus(),
                        KeyCode::Right => app.next_diff_file(),
                        KeyCode::Left => app.previous_diff_file(),
                        KeyCode::Down | KeyCode::Char('j') => app.next(),
                        KeyCode::Up | KeyCode::Char('k') => app.previous(),
                        _ => {}
                    },
                    InputMode::Filtering(focus) => match key.code {
                        KeyCode::Enter | KeyCode::Esc => app.finish_filter(focus),
                        KeyCode::Backspace => app.pop_filter_char(focus),
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.clear_filter(focus)
                        }
                        KeyCode::Char(ch)
                            if !key.modifiers.contains(KeyModifiers::CONTROL)
                                && !key.modifiers.contains(KeyModifiers::ALT) =>
                        {
                            app.push_filter_char(focus, ch)
                        }
                        _ => {}
                    },
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

#[cfg(test)]
mod tests {
    use super::{filter_projects, filter_versions, project_status_label};
    use crate::models::{CatalogProject, CatalogVersion};

    fn sample_project(
        id: i64,
        name: &str,
        path: &str,
        version: Option<&str>,
        source: &str,
    ) -> CatalogProject {
        CatalogProject {
            id,
            canonical_path: path.to_string(),
            name: name.to_string(),
            git_remote: None,
            has_git_repo: true,
            has_claude_md: false,
            has_claude_dir: true,
            has_claude_settings: true,
            claude_settings_paths: vec![format!("{path}/.claude/settings.local.json")],
            has_agents_md: false,
            has_agents_dir: false,
            has_codex_dir: false,
            has_codex_settings: false,
            codex_settings_paths: Vec::new(),
            gstack_install_id: None,
            gstack_install_observed_path: Some(format!("{path}/.claude/skills/gstack")),
            effective_gstack_version: version.map(ToOwned::to_owned),
            effective_gstack_source: source.to_string(),
            first_seen_at: "2026-03-23T00:00:00Z".to_string(),
            last_seen_at: "2026-03-23T00:00:00Z".to_string(),
        }
    }

    fn sample_version(version: &str, sha: &str, subject: &str, body: &str) -> CatalogVersion {
        CatalogVersion {
            version: version.to_string(),
            commit_sha: sha.to_string(),
            committed_at: "2026-03-23T00:00:00Z".to_string(),
            subject: subject.to_string(),
            body: body.to_string(),
        }
    }

    #[test]
    fn project_filter_matches_name_path_and_version() {
        let projects = vec![
            sample_project(
                1,
                "jenkins-chat",
                "/Users/example/Work/jenkins-chat",
                Some("0.8.6"),
                "local_install",
            ),
            sample_project(
                2,
                "startup_world",
                "/Users/example/Work/startup_world",
                Some("0.11.10.0"),
                "none",
            ),
        ];

        assert_eq!(filter_projects(&projects, "jenkins").len(), 1);
        assert_eq!(filter_projects(&projects, "startup 0.11.10").len(), 1);
        assert_eq!(filter_projects(&projects, "Work local_install").len(), 1);
        assert!(filter_projects(&projects, "missing value").is_empty());
    }

    #[test]
    fn projects_are_sorted_by_state_then_name() {
        let local = sample_project(
            1,
            "jenkins-chat",
            "/Users/example/Work/jenkins-chat",
            Some("0.8.6"),
            "local_install",
        );
        let ready = sample_project(
            2,
            "startup_world",
            "/Users/example/Work/startup_world",
            None,
            "none",
        );
        let mut configured =
            sample_project(3, "advisory", "/Users/example/Work/advisory", None, "none");
        configured.has_git_repo = false;

        let sorted = filter_projects(&[ready.clone(), configured.clone(), local.clone()], "");
        let names = sorted
            .iter()
            .map(|project| project.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["jenkins-chat", "startup_world", "advisory"]);
        assert_eq!(project_status_label(&ready), "ready");
        assert_eq!(project_status_label(&configured), "configured");
        assert_eq!(project_status_label(&local), "local 0.8.6");
    }

    #[test]
    fn version_filter_matches_version_sha_subject_and_body() {
        let versions = vec![
            sample_version(
                "0.11.10.0",
                "f4bbfaa5bdfd1234",
                "feat: CI evals on Ubicloud",
                "Adds 12 parallel runners and a docker image.",
            ),
            sample_version(
                "0.11.9.0",
                "ffd9ab29b9321234",
                "fix: tighten terminal refresh",
                "Cleans up rendering and search state.",
            ),
        ];

        assert_eq!(filter_versions(&versions, "0.11.10").len(), 1);
        assert_eq!(filter_versions(&versions, "f4bbfaa5 feat").len(), 1);
        assert_eq!(filter_versions(&versions, "docker runners").len(), 1);
        assert!(filter_versions(&versions, "rollback only").is_empty());
    }
}
