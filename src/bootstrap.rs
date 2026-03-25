use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::config::home_dir;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentInstallChoice {
    None,
    Claude,
    Codex,
    Both,
}

impl AgentInstallChoice {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Both => "both",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ToolStatus {
    pub installed: bool,
    pub path: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct RuntimeStatus {
    pub bun: ToolStatus,
    pub claude: ToolStatus,
    pub codex: ToolStatus,
}

fn executable_candidates(name: &str) -> Vec<String> {
    #[cfg(windows)]
    {
        if Path::new(name).extension().is_some() {
            vec![name.to_string()]
        } else {
            vec![
                format!("{name}.exe"),
                format!("{name}.cmd"),
                format!("{name}.bat"),
                format!("{name}.ps1"),
                name.to_string(),
            ]
        }
    }
    #[cfg(not(windows))]
    {
        vec![name.to_string()]
    }
}

fn known_locations(name: &str) -> Vec<PathBuf> {
    let bun_bin = home_dir().join(".bun").join("bin");
    executable_candidates(name)
        .into_iter()
        .map(|candidate| bun_bin.join(candidate))
        .collect::<Vec<_>>()
}

fn resolve_executable(name: &str) -> Option<PathBuf> {
    let path_value = env::var_os("PATH").unwrap_or_default();
    for directory in env::split_paths(&path_value) {
        for candidate in executable_candidates(name) {
            let path = directory.join(&candidate);
            if path.is_file() {
                return Some(path);
            }
        }
    }

    known_locations(name)
        .into_iter()
        .find(|path| path.is_file())
}

fn prepend_bun_to_process_path() {
    let bun_bin = home_dir().join(".bun").join("bin");
    if !bun_bin.exists() {
        return;
    }
    let mut paths = vec![bun_bin];
    paths.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
    if let Ok(joined) = env::join_paths(paths) {
        // This only updates the current process so newly installed tools can be
        // discovered immediately by the running TUI.
        unsafe {
            env::set_var("PATH", joined);
        }
    }
}

fn tool_status(name: &str) -> ToolStatus {
    let path = resolve_executable(name);
    ToolStatus {
        installed: path.is_some(),
        path: path.map(|value| value.to_string_lossy().to_string()),
    }
}

pub fn detect_runtime_status() -> RuntimeStatus {
    prepend_bun_to_process_path();
    RuntimeStatus {
        bun: tool_status("bun"),
        claude: tool_status("claude"),
        codex: tool_status("codex"),
    }
}

fn collect_command_output(output: &std::process::Output) -> Result<Vec<String>> {
    let stdout =
        String::from_utf8(output.stdout.clone()).context("command stdout was not valid UTF-8")?;
    let stderr =
        String::from_utf8(output.stderr.clone()).context("command stderr was not valid UTF-8")?;
    let mut lines = stdout
        .lines()
        .chain(stderr.lines())
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        lines.push("(no command output)".to_string());
    }
    Ok(lines)
}

fn run_shell(script: &str) -> Result<Vec<String>> {
    let output = if cfg!(windows) {
        Command::new("powershell")
            .args(["-NoProfile", "-Command", script])
            .output()
            .with_context(|| format!("failed to execute PowerShell bootstrap: {script}"))?
    } else {
        Command::new("sh")
            .args(["-lc", script])
            .output()
            .with_context(|| format!("failed to execute shell bootstrap: {script}"))?
    };
    let lines = collect_command_output(&output)?;
    Ok(lines)
}

fn run_program(program: &Path, args: &[&str]) -> Result<Vec<String>> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("failed to execute {} {}", program.display(), args.join(" ")))?;
    let lines = collect_command_output(&output)?;
    if !output.status.success() {
        bail!(
            "{} {} failed:\n{}",
            program.display(),
            args.join(" "),
            lines.join("\n")
        );
    }
    Ok(lines)
}

pub fn ensure_bun_installed() -> Result<Vec<String>> {
    let status = detect_runtime_status();
    if status.bun.installed {
        return Ok(vec![format!(
            "Bun already installed at {}",
            status
                .bun
                .path
                .unwrap_or_else(|| "unknown path".to_string())
        )]);
    }

    let script = if cfg!(windows) {
        "irm bun.sh/install.ps1 | iex"
    } else {
        "if command -v curl >/dev/null 2>&1; then curl -fsSL https://bun.com/install | bash; elif command -v wget >/dev/null 2>&1; then wget -qO- https://bun.com/install | bash; else echo 'Missing curl or wget for Bun install' >&2; exit 1; fi"
    };
    let mut lines = run_shell(script)?;
    prepend_bun_to_process_path();
    let refreshed = detect_runtime_status();
    if !refreshed.bun.installed {
        bail!("Bun install completed but bun is still not available on PATH");
    }
    lines.push(format!(
        "Bun available at {}",
        refreshed
            .bun
            .path
            .unwrap_or_else(|| "unknown path".to_string())
    ));
    Ok(lines)
}

pub fn install_agents(choice: AgentInstallChoice) -> Result<Vec<String>> {
    if matches!(choice, AgentInstallChoice::None) {
        return Ok(vec!["Skipped Claude/Codex bootstrap".to_string()]);
    }

    let _ = ensure_bun_installed()?;
    prepend_bun_to_process_path();
    let bun_path = resolve_executable("bun").ok_or_else(|| {
        anyhow::anyhow!("bun was expected after install but could not be resolved on PATH")
    })?;

    let mut lines = Vec::new();
    let packages = match choice {
        AgentInstallChoice::Claude => vec![("@anthropic-ai/claude-code", "Claude Code")],
        AgentInstallChoice::Codex => vec![("@openai/codex", "Codex CLI")],
        AgentInstallChoice::Both => vec![
            ("@anthropic-ai/claude-code", "Claude Code"),
            ("@openai/codex", "Codex CLI"),
        ],
        AgentInstallChoice::None => Vec::new(),
    };

    for (package, label) in packages {
        lines.push(format!("Installing {label} with Bun..."));
        let mut command_lines = run_program(&bun_path, &["install", "--global", package])?;
        lines.append(&mut command_lines);
    }

    let refreshed = detect_runtime_status();
    match choice {
        AgentInstallChoice::Claude if !refreshed.claude.installed => {
            bail!("Claude Code install finished but `claude` is still unavailable")
        }
        AgentInstallChoice::Codex if !refreshed.codex.installed => {
            bail!("Codex install finished but `codex` is still unavailable")
        }
        AgentInstallChoice::Both if !refreshed.claude.installed || !refreshed.codex.installed => {
            bail!("Agent install finished but at least one CLI is still unavailable")
        }
        _ => {}
    }

    Ok(lines)
}
