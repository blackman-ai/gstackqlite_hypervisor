use std::env;
use std::path::PathBuf;

pub const DEFAULT_SOURCE_NAME: &str = "gstack";
pub const DEFAULT_UPSTREAM_URL: &str = "https://github.com/garrytan/gstack.git";
pub const DEFAULT_UPSTREAM_REF: &str = "main";
pub const DEFAULT_MAX_DEPTH: usize = 5;

pub const LOCAL_INSTALL_RELATIVE_PATHS: [&str; 3] = [
    ".claude/skills/gstack",
    ".agents/skills/gstack",
    ".codex/skills/gstack",
];
pub const LOCAL_MANIFEST_EXCLUDES: [&str; 4] = [".git", "node_modules", "browse/dist", ".DS_Store"];
pub const REPO_SCAN_SKIP_DIRS: [&str; 9] = [
    ".git",
    "node_modules",
    "dist",
    "build",
    ".next",
    "coverage",
    "target",
    "vendor",
    ".turbo",
];

pub fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn app_root() -> PathBuf {
    home_dir().join(".gstack").join("hypervisor")
}

pub fn default_database_path() -> PathBuf {
    app_root().join("catalog.sqlite")
}

pub fn backup_root() -> PathBuf {
    app_root().join("backups")
}

pub fn known_install_locations() -> Vec<PathBuf> {
    vec![
        home_dir().join(".claude").join("skills").join("gstack"),
        home_dir().join(".codex").join("skills").join("gstack"),
        home_dir().join(".gstack").join("repos").join("gstack"),
    ]
}

pub fn default_scan_roots() -> Vec<PathBuf> {
    let home = home_dir();
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let candidates = [
        cwd,
        home.join("Work"),
        home.join("Code"),
        home.join("code"),
        home.join("src"),
        home.join("projects"),
        home.join("Developer"),
    ];

    candidates
        .into_iter()
        .filter(|candidate| candidate.exists())
        .collect()
}
