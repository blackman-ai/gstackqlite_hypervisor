use std::fs;
use std::io::{self, BufRead, BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Map as JsonMap, Value as JsonValue, json};
use toml::Value as TomlValue;
use toml::map::Map as TomlMap;

use crate::config::{DEFAULT_MAX_DEPTH, default_scan_roots, home_dir};
use crate::db::Catalog;
use crate::ingest::ensure_catalog_has_upstream;
use crate::scan::sync_catalog;
use crate::upgrade::{apply_version_to_projects, project_diff_preview, revert_projects};
use crate::util::{ensure_dir, real_path_or_original};

pub const BINARY_NAME: &str = "gstackqlite-hypervisor";
pub const SERVER_NAME: &str = "gstackqlite-hypervisor";
const PROTOCOL_VERSION: &str = "2025-11-25";
const SERVER_ARGS: [&str; 2] = ["mcp", "serve"];

#[derive(Clone, Copy, Debug)]
pub enum McpAgent {
    Claude,
    Codex,
}

impl McpAgent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

#[derive(Clone, Debug)]
pub enum McpScope {
    Global,
    Project(PathBuf),
}

impl McpScope {
    fn label(&self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project(_) => "project",
        }
    }

    fn project_path(&self) -> Option<String> {
        match self {
            Self::Global => None,
            Self::Project(path) => Some(path.to_string_lossy().to_string()),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct McpConfigRecord {
    pub agent: String,
    pub scope: String,
    pub project_path: Option<String>,
    pub config_path: String,
    pub server_name: String,
    pub status: String,
}

struct ServerState {
    catalog: Catalog,
    initialized: bool,
}

fn success_response(id: JsonValue, result: JsonValue) -> JsonValue {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

fn error_response(id: Option<JsonValue>, code: i64, message: impl Into<String>) -> JsonValue {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(JsonValue::Null),
        "error": {
            "code": code,
            "message": message.into()
        }
    })
}

fn text_block(text: String) -> JsonValue {
    json!({
        "type": "text",
        "text": text
    })
}

fn tool_success<T: Serialize>(value: &T) -> JsonValue {
    let structured = serde_json::to_value(value).unwrap_or(JsonValue::Null);
    let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string());
    json!({
        "content": [text_block(text)],
        "structuredContent": structured,
        "isError": false
    })
}

fn tool_error(message: impl Into<String>) -> JsonValue {
    json!({
        "content": [text_block(message.into())],
        "isError": true
    })
}

fn as_object<'a>(value: &'a JsonValue, field: &str) -> Option<&'a JsonMap<String, JsonValue>> {
    value.get(field)?.as_object()
}

fn arg_string(arguments: &JsonMap<String, JsonValue>, key: &str) -> Option<String> {
    arguments.get(key)?.as_str().map(ToOwned::to_owned)
}

fn arg_usize(arguments: &JsonMap<String, JsonValue>, key: &str) -> Option<usize> {
    arguments
        .get(key)?
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
}

fn arg_i64(arguments: &JsonMap<String, JsonValue>, key: &str) -> Option<i64> {
    arguments.get(key)?.as_i64()
}

fn arg_bool(arguments: &JsonMap<String, JsonValue>, key: &str) -> Option<bool> {
    arguments.get(key)?.as_bool()
}

fn arg_string_list(arguments: &JsonMap<String, JsonValue>, key: &str) -> Option<Vec<String>> {
    arguments.get(key)?.as_array().map(|values| {
        values
            .iter()
            .filter_map(|value| value.as_str().map(ToOwned::to_owned))
            .collect()
    })
}

fn server_command_json() -> JsonValue {
    json!({
        "command": BINARY_NAME,
        "args": SERVER_ARGS,
    })
}

fn server_command_toml() -> TomlValue {
    let mut table = TomlMap::new();
    table.insert(
        "command".to_string(),
        TomlValue::String(BINARY_NAME.to_string()),
    );
    table.insert(
        "args".to_string(),
        TomlValue::Array(
            SERVER_ARGS
                .iter()
                .map(|value| TomlValue::String((*value).to_string()))
                .collect(),
        ),
    );
    TomlValue::Table(table)
}

fn claude_candidates(scope: &McpScope) -> Vec<PathBuf> {
    match scope {
        McpScope::Global => vec![
            home_dir().join(".claude").join("settings.local.json"),
            home_dir().join(".claude").join("settings.json"),
        ],
        McpScope::Project(root) => vec![
            root.join(".claude").join("settings.local.json"),
            root.join(".claude").join("settings.json"),
        ],
    }
}

fn claude_target_path(scope: &McpScope) -> PathBuf {
    let candidates = claude_candidates(scope);
    match scope {
        McpScope::Global => candidates
            .iter()
            .find(|path| path.exists())
            .cloned()
            .unwrap_or_else(|| home_dir().join(".claude").join("settings.json")),
        McpScope::Project(root) => {
            let preferred = root.join(".claude").join("settings.local.json");
            if preferred.exists() {
                preferred
            } else {
                preferred
            }
        }
    }
}

fn codex_candidates(scope: &McpScope) -> Vec<PathBuf> {
    match scope {
        McpScope::Global => vec![
            home_dir().join(".codex").join("config.toml"),
            home_dir().join(".codex").join("settings.toml"),
        ],
        McpScope::Project(root) => vec![
            root.join(".codex").join("config.toml"),
            root.join(".codex").join("settings.toml"),
        ],
    }
}

fn codex_target_path(scope: &McpScope) -> PathBuf {
    let candidates = codex_candidates(scope);
    candidates
        .iter()
        .find(|path| path.exists())
        .cloned()
        .unwrap_or_else(|| match scope {
            McpScope::Global => home_dir().join(".codex").join("config.toml"),
            McpScope::Project(root) => root.join(".codex").join("config.toml"),
        })
}

fn load_json_root(path: &Path) -> Result<JsonValue> {
    if !path.exists() {
        return Ok(JsonValue::Object(JsonMap::new()));
    }
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let parsed: JsonValue = serde_json::from_str(&content)
        .with_context(|| format!("invalid JSON in {}", path.display()))?;
    if !parsed.is_object() {
        bail!("expected JSON object in {}", path.display());
    }
    Ok(parsed)
}

fn write_json_root(path: &Path, value: &JsonValue) -> Result<()> {
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    let serialized = serde_json::to_string_pretty(value)?;
    fs::write(path, format!("{serialized}\n"))
        .with_context(|| format!("failed to write {}", path.display()))
}

fn load_toml_root(path: &Path) -> Result<TomlValue> {
    if !path.exists() {
        return Ok(TomlValue::Table(TomlMap::new()));
    }
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let parsed: TomlValue =
        toml::from_str(&content).with_context(|| format!("invalid TOML in {}", path.display()))?;
    if !parsed.is_table() {
        bail!("expected TOML table in {}", path.display());
    }
    Ok(parsed)
}

fn write_toml_root(path: &Path, value: &TomlValue) -> Result<()> {
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    let serialized = toml::to_string_pretty(value)?;
    fs::write(path, format!("{serialized}\n"))
        .with_context(|| format!("failed to write {}", path.display()))
}

fn install_claude(scope: &McpScope) -> Result<McpConfigRecord> {
    let config_path = claude_target_path(scope);
    let mut root = load_json_root(&config_path)?;
    let object = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("expected JSON object in {}", config_path.display()))?;
    let servers = object
        .entry("mcpServers".to_string())
        .or_insert_with(|| JsonValue::Object(JsonMap::new()));
    let servers_object = servers.as_object_mut().ok_or_else(|| {
        anyhow::anyhow!("expected mcpServers object in {}", config_path.display())
    })?;
    let desired = server_command_json();
    let status = if servers_object.get(SERVER_NAME) == Some(&desired) {
        "already_installed"
    } else {
        servers_object.insert(SERVER_NAME.to_string(), desired);
        write_json_root(&config_path, &root)?;
        "installed"
    };
    Ok(McpConfigRecord {
        agent: McpAgent::Claude.as_str().to_string(),
        scope: scope.label().to_string(),
        project_path: scope.project_path(),
        config_path: config_path.to_string_lossy().to_string(),
        server_name: SERVER_NAME.to_string(),
        status: status.to_string(),
    })
}

fn uninstall_claude(scope: &McpScope) -> Result<Vec<McpConfigRecord>> {
    let candidates = claude_candidates(scope);
    let mut results = Vec::new();
    let mut any_existing = false;

    for path in candidates.iter().filter(|path| path.exists()) {
        any_existing = true;
        let mut root = load_json_root(path)?;
        let object = root
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("expected JSON object in {}", path.display()))?;
        let status = if let Some(servers) = object
            .get_mut("mcpServers")
            .and_then(JsonValue::as_object_mut)
        {
            if servers.remove(SERVER_NAME).is_some() {
                if servers.is_empty() {
                    object.remove("mcpServers");
                }
                write_json_root(path, &root)?;
                "removed"
            } else {
                "absent"
            }
        } else {
            "absent"
        };
        results.push(McpConfigRecord {
            agent: McpAgent::Claude.as_str().to_string(),
            scope: scope.label().to_string(),
            project_path: scope.project_path(),
            config_path: path.to_string_lossy().to_string(),
            server_name: SERVER_NAME.to_string(),
            status: status.to_string(),
        });
    }

    if !any_existing {
        let path = claude_target_path(scope);
        results.push(McpConfigRecord {
            agent: McpAgent::Claude.as_str().to_string(),
            scope: scope.label().to_string(),
            project_path: scope.project_path(),
            config_path: path.to_string_lossy().to_string(),
            server_name: SERVER_NAME.to_string(),
            status: "absent".to_string(),
        });
    }

    Ok(results)
}

fn inspect_claude(scope: &McpScope) -> Result<Vec<McpConfigRecord>> {
    let candidates = claude_candidates(scope);
    let mut results = Vec::new();
    let mut any_existing = false;

    for path in candidates.iter().filter(|path| path.exists()) {
        any_existing = true;
        let root = load_json_root(path)?;
        let status = root
            .get("mcpServers")
            .and_then(JsonValue::as_object)
            .and_then(|servers| servers.get(SERVER_NAME))
            .map(|_| "installed")
            .unwrap_or("absent");
        results.push(McpConfigRecord {
            agent: McpAgent::Claude.as_str().to_string(),
            scope: scope.label().to_string(),
            project_path: scope.project_path(),
            config_path: path.to_string_lossy().to_string(),
            server_name: SERVER_NAME.to_string(),
            status: status.to_string(),
        });
    }

    if !any_existing {
        let path = claude_target_path(scope);
        results.push(McpConfigRecord {
            agent: McpAgent::Claude.as_str().to_string(),
            scope: scope.label().to_string(),
            project_path: scope.project_path(),
            config_path: path.to_string_lossy().to_string(),
            server_name: SERVER_NAME.to_string(),
            status: "absent".to_string(),
        });
    }

    Ok(results)
}

fn install_codex(scope: &McpScope) -> Result<McpConfigRecord> {
    let config_path = codex_target_path(scope);
    let mut root = load_toml_root(&config_path)?;
    let table = root
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("expected TOML table in {}", config_path.display()))?;
    let servers = table
        .entry("mcp_servers".to_string())
        .or_insert_with(|| TomlValue::Table(TomlMap::new()));
    let servers_table = servers.as_table_mut().ok_or_else(|| {
        anyhow::anyhow!("expected mcp_servers table in {}", config_path.display())
    })?;
    let desired = server_command_toml();
    let status = if servers_table.get(SERVER_NAME) == Some(&desired) {
        "already_installed"
    } else {
        servers_table.insert(SERVER_NAME.to_string(), desired);
        write_toml_root(&config_path, &root)?;
        "installed"
    };
    Ok(McpConfigRecord {
        agent: McpAgent::Codex.as_str().to_string(),
        scope: scope.label().to_string(),
        project_path: scope.project_path(),
        config_path: config_path.to_string_lossy().to_string(),
        server_name: SERVER_NAME.to_string(),
        status: status.to_string(),
    })
}

fn uninstall_codex(scope: &McpScope) -> Result<Vec<McpConfigRecord>> {
    let candidates = codex_candidates(scope);
    let mut results = Vec::new();
    let mut any_existing = false;

    for path in candidates.iter().filter(|path| path.exists()) {
        any_existing = true;
        let mut root = load_toml_root(path)?;
        let table = root
            .as_table_mut()
            .ok_or_else(|| anyhow::anyhow!("expected TOML table in {}", path.display()))?;
        let status = if let Some(servers) = table
            .get_mut("mcp_servers")
            .and_then(TomlValue::as_table_mut)
        {
            if servers.remove(SERVER_NAME).is_some() {
                if servers.is_empty() {
                    table.remove("mcp_servers");
                }
                write_toml_root(path, &root)?;
                "removed"
            } else {
                "absent"
            }
        } else {
            "absent"
        };
        results.push(McpConfigRecord {
            agent: McpAgent::Codex.as_str().to_string(),
            scope: scope.label().to_string(),
            project_path: scope.project_path(),
            config_path: path.to_string_lossy().to_string(),
            server_name: SERVER_NAME.to_string(),
            status: status.to_string(),
        });
    }

    if !any_existing {
        let path = codex_target_path(scope);
        results.push(McpConfigRecord {
            agent: McpAgent::Codex.as_str().to_string(),
            scope: scope.label().to_string(),
            project_path: scope.project_path(),
            config_path: path.to_string_lossy().to_string(),
            server_name: SERVER_NAME.to_string(),
            status: "absent".to_string(),
        });
    }

    Ok(results)
}

fn inspect_codex(scope: &McpScope) -> Result<Vec<McpConfigRecord>> {
    let candidates = codex_candidates(scope);
    let mut results = Vec::new();
    let mut any_existing = false;

    for path in candidates.iter().filter(|path| path.exists()) {
        any_existing = true;
        let root = load_toml_root(path)?;
        let status = root
            .get("mcp_servers")
            .and_then(TomlValue::as_table)
            .and_then(|servers| servers.get(SERVER_NAME))
            .map(|_| "installed")
            .unwrap_or("absent");
        results.push(McpConfigRecord {
            agent: McpAgent::Codex.as_str().to_string(),
            scope: scope.label().to_string(),
            project_path: scope.project_path(),
            config_path: path.to_string_lossy().to_string(),
            server_name: SERVER_NAME.to_string(),
            status: status.to_string(),
        });
    }

    if !any_existing {
        let path = codex_target_path(scope);
        results.push(McpConfigRecord {
            agent: McpAgent::Codex.as_str().to_string(),
            scope: scope.label().to_string(),
            project_path: scope.project_path(),
            config_path: path.to_string_lossy().to_string(),
            server_name: SERVER_NAME.to_string(),
            status: "absent".to_string(),
        });
    }

    Ok(results)
}

pub fn resolve_project_scope(catalog: &Catalog, identifier: &str) -> Result<McpScope> {
    let path = PathBuf::from(identifier);
    if path.exists() {
        return Ok(McpScope::Project(real_path_or_original(&path)));
    }

    let Some(project) = catalog.find_project(identifier)? else {
        bail!("project not found: {identifier}");
    };
    Ok(McpScope::Project(PathBuf::from(project.canonical_path)))
}

pub fn install_config(scope: &McpScope, agents: &[McpAgent]) -> Result<Vec<McpConfigRecord>> {
    let mut results = Vec::new();
    for agent in agents {
        match agent {
            McpAgent::Claude => results.push(install_claude(scope)?),
            McpAgent::Codex => results.push(install_codex(scope)?),
        }
    }
    Ok(results)
}

pub fn uninstall_config(scope: &McpScope, agents: &[McpAgent]) -> Result<Vec<McpConfigRecord>> {
    let mut results = Vec::new();
    for agent in agents {
        match agent {
            McpAgent::Claude => results.extend(uninstall_claude(scope)?),
            McpAgent::Codex => results.extend(uninstall_codex(scope)?),
        }
    }
    Ok(results)
}

pub fn inspect_config(scope: &McpScope, agents: &[McpAgent]) -> Result<Vec<McpConfigRecord>> {
    let mut results = Vec::new();
    for agent in agents {
        match agent {
            McpAgent::Claude => results.extend(inspect_claude(scope)?),
            McpAgent::Codex => results.extend(inspect_codex(scope)?),
        }
    }
    Ok(results)
}

fn tool_definitions() -> Vec<JsonValue> {
    vec![
        json!({
            "name": "sync_catalog",
            "title": "Sync Catalog",
            "description": "Fetch upstream gstack history and rescan local projects.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "roots": {"type": "array", "items": {"type": "string"}},
                    "maxDepth": {"type": "integer", "minimum": 1}
                }
            },
            "annotations": {
                "readOnlyHint": false,
                "idempotentHint": true,
                "destructiveHint": false
            }
        }),
        json!({
            "name": "list_projects",
            "title": "List Projects",
            "description": "List discovered git, Claude, and Codex projects from SQLite.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            },
            "annotations": {
                "readOnlyHint": true,
                "idempotentHint": true
            }
        }),
        json!({
            "name": "project_detail",
            "title": "Project Detail",
            "description": "Inspect one cataloged project by id, path, or name.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "identifier": {"type": "string"}
                },
                "required": ["identifier"]
            },
            "annotations": {
                "readOnlyHint": true,
                "idempotentHint": true
            }
        }),
        json!({
            "name": "project_history",
            "title": "Project History",
            "description": "List revertable backup events for a project.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "identifier": {"type": "string"},
                    "limit": {"type": "integer", "minimum": 1}
                },
                "required": ["identifier"]
            },
            "annotations": {
                "readOnlyHint": true,
                "idempotentHint": true
            }
        }),
        json!({
            "name": "list_versions",
            "title": "List Versions",
            "description": "List ingested upstream gstack versions, optionally filtered by search text.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "search": {"type": "string"}
                }
            },
            "annotations": {
                "readOnlyHint": true,
                "idempotentHint": true
            }
        }),
        json!({
            "name": "diff_preview",
            "title": "Diff Preview",
            "description": "Preview file-level and content-level changes between a project's current state and a target gstack version.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "identifier": {"type": "string"},
                    "version": {"type": "string"},
                    "commit": {"type": "string"},
                    "maxFiles": {"type": "integer", "minimum": 1},
                    "maxLines": {"type": "integer", "minimum": 1}
                },
                "required": ["identifier"]
            },
            "annotations": {
                "readOnlyHint": true,
                "idempotentHint": true
            }
        }),
        json!({
            "name": "apply_version",
            "title": "Apply Version",
            "description": "Apply or dry-run a target gstack version against one or more projects.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "projects": {"type": "array", "items": {"type": "string"}},
                    "version": {"type": "string"},
                    "commit": {"type": "string"},
                    "dryRun": {"type": "boolean"}
                },
                "required": ["projects"]
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": true,
                "idempotentHint": false
            }
        }),
        json!({
            "name": "revert_project",
            "title": "Revert Project",
            "description": "Restore a project from backup history, optionally targeting a specific backup event.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "projects": {"type": "array", "items": {"type": "string"}},
                    "eventId": {"type": "integer"},
                    "dryRun": {"type": "boolean"}
                },
                "required": ["projects"]
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": true,
                "idempotentHint": false
            }
        }),
    ]
}

fn handle_tool_call(
    state: &mut ServerState,
    name: &str,
    arguments: &JsonMap<String, JsonValue>,
) -> JsonValue {
    match name {
        "sync_catalog" => {
            let roots = arg_string_list(arguments, "roots").unwrap_or_else(|| {
                default_scan_roots()
                    .iter()
                    .map(|path| path.to_string_lossy().to_string())
                    .collect()
            });
            let max_depth = arg_usize(arguments, "maxDepth").unwrap_or(DEFAULT_MAX_DEPTH);
            let root_paths = roots.into_iter().map(PathBuf::from).collect::<Vec<_>>();
            match sync_catalog(&state.catalog, &root_paths, Some(max_depth)) {
                Ok(scan) => tool_success(&scan),
                Err(error) => tool_error(error.to_string()),
            }
        }
        "list_projects" => match state.catalog.list_projects() {
            Ok(projects) => tool_success(&projects),
            Err(error) => tool_error(error.to_string()),
        },
        "project_detail" => {
            let Some(identifier) = arg_string(arguments, "identifier") else {
                return tool_error("missing required argument: identifier");
            };
            match state.catalog.project_detail(&identifier) {
                Ok(Some(detail)) => tool_success(&detail),
                Ok(None) => tool_error(format!("project not found: {identifier}")),
                Err(error) => tool_error(error.to_string()),
            }
        }
        "project_history" => {
            let Some(identifier) = arg_string(arguments, "identifier") else {
                return tool_error("missing required argument: identifier");
            };
            let limit = arg_usize(arguments, "limit").unwrap_or(10);
            match state.catalog.project_backup_history(&identifier, limit) {
                Ok(history) => tool_success(&history),
                Err(error) => tool_error(error.to_string()),
            }
        }
        "list_versions" => {
            if let Err(error) = ensure_catalog_has_upstream(&state.catalog) {
                return tool_error(error.to_string());
            }
            match state
                .catalog
                .list_versions(arg_string(arguments, "search").as_deref())
            {
                Ok(versions) => tool_success(&versions),
                Err(error) => tool_error(error.to_string()),
            }
        }
        "diff_preview" => {
            let Some(identifier) = arg_string(arguments, "identifier") else {
                return tool_error("missing required argument: identifier");
            };
            match project_diff_preview(
                &state.catalog,
                &identifier,
                arg_string(arguments, "version").as_deref(),
                arg_string(arguments, "commit").as_deref(),
                arg_usize(arguments, "maxFiles").unwrap_or(6),
                arg_usize(arguments, "maxLines").unwrap_or(14),
            ) {
                Ok(preview) => tool_success(&preview),
                Err(error) => tool_error(error.to_string()),
            }
        }
        "apply_version" => {
            let Some(projects) = arg_string_list(arguments, "projects") else {
                return tool_error("missing required argument: projects");
            };
            match apply_version_to_projects(
                &state.catalog,
                arg_string(arguments, "version").as_deref(),
                arg_string(arguments, "commit").as_deref(),
                &projects,
                arg_bool(arguments, "dryRun").unwrap_or(true),
            ) {
                Ok(results) => tool_success(&results),
                Err(error) => tool_error(error.to_string()),
            }
        }
        "revert_project" => {
            let Some(projects) = arg_string_list(arguments, "projects") else {
                return tool_error("missing required argument: projects");
            };
            match revert_projects(
                &state.catalog,
                &projects,
                arg_i64(arguments, "eventId"),
                arg_bool(arguments, "dryRun").unwrap_or(true),
            ) {
                Ok(results) => tool_success(&results),
                Err(error) => tool_error(error.to_string()),
            }
        }
        _ => tool_error(format!("unknown tool: {name}")),
    }
}

fn handle_request(state: &mut ServerState, message: &JsonValue) -> Option<JsonValue> {
    let object = message.as_object()?;
    let method = object.get("method")?.as_str()?;
    let id = object.get("id").cloned();

    match method {
        "initialize" => Some(success_response(
            id.unwrap_or(JsonValue::Null),
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {
                    "tools": {
                        "listChanged": false
                    }
                },
                "serverInfo": {
                    "name": SERVER_NAME,
                    "title": SERVER_NAME,
                    "version": env!("CARGO_PKG_VERSION"),
                    "description": "SQLite-backed local supervisor for gstack."
                },
                "instructions": "Use the exposed tools to inspect, diff, apply, and revert gstack versions across local projects."
            }),
        )),
        "notifications/initialized" => {
            state.initialized = true;
            None
        }
        "ping" => id.map(|request_id| success_response(request_id, json!({}))),
        "tools/list" => {
            if !state.initialized {
                return id.map(|request_id| {
                    error_response(Some(request_id), -32002, "server has not been initialized")
                });
            }
            id.map(|request_id| {
                success_response(
                    request_id,
                    json!({
                        "tools": tool_definitions()
                    }),
                )
            })
        }
        "tools/call" => {
            if !state.initialized {
                return id.map(|request_id| {
                    error_response(Some(request_id), -32002, "server has not been initialized")
                });
            }
            let Some(params) = as_object(message, "params") else {
                return id.map(|request_id| {
                    error_response(Some(request_id), -32602, "missing request params")
                });
            };
            let Some(name) = params.get("name").and_then(JsonValue::as_str) else {
                return id.map(|request_id| {
                    error_response(Some(request_id), -32602, "missing tool name")
                });
            };
            let arguments = params
                .get("arguments")
                .and_then(JsonValue::as_object)
                .cloned()
                .unwrap_or_default();
            id.map(|request_id| {
                success_response(request_id, handle_tool_call(state, name, &arguments))
            })
        }
        _ => id.map(|request_id| {
            error_response(
                Some(request_id),
                -32601,
                format!("method not found: {method}"),
            )
        }),
    }
}

fn write_message(writer: &mut BufWriter<io::StdoutLock<'_>>, message: &JsonValue) -> Result<()> {
    serde_json::to_writer(&mut *writer, message)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

pub fn run_stdio_server(catalog: Catalog) -> Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut writer = BufWriter::new(stdout.lock());
    let mut state = ServerState {
        catalog,
        initialized: false,
    };

    for line in stdin.lock().lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let message: JsonValue = match serde_json::from_str(trimmed) {
            Ok(message) => message,
            Err(error) => {
                write_message(
                    &mut writer,
                    &error_response(None, -32700, error.to_string()),
                )?;
                continue;
            }
        };

        if let Some(batch) = message.as_array() {
            for item in batch {
                if let Some(response) = handle_request(&mut state, item) {
                    write_message(&mut writer, &response)?;
                }
            }
            continue;
        }

        if let Some(response) = handle_request(&mut state, &message) {
            write_message(&mut writer, &response)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        BINARY_NAME, McpAgent, McpScope, SERVER_NAME, inspect_config, install_config,
        uninstall_config,
    };
    use crate::util::TempWorkdir;

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).expect("config should be readable")
    }

    #[test]
    fn claude_project_install_and_uninstall_round_trip() {
        let temp = TempWorkdir::new("gstackqlite-hypervisor-mcp-claude-test").expect("temp dir");
        let scope = McpScope::Project(temp.path().to_path_buf());

        let install = install_config(&scope, &[McpAgent::Claude]).expect("install should work");
        assert_eq!(install.len(), 1);
        assert_eq!(install[0].status, "installed");

        let config_path = temp.path().join(".claude").join("settings.local.json");
        let content = read(&config_path);
        assert!(content.contains(SERVER_NAME));
        assert!(content.contains(BINARY_NAME));

        let status = inspect_config(&scope, &[McpAgent::Claude]).expect("inspect should work");
        assert_eq!(status[0].status, "installed");

        let uninstall =
            uninstall_config(&scope, &[McpAgent::Claude]).expect("uninstall should work");
        assert!(uninstall.iter().any(|record| record.status == "removed"));
        let content = read(&config_path);
        assert!(!content.contains(SERVER_NAME));
    }

    #[test]
    fn codex_project_install_and_uninstall_round_trip() {
        let temp = TempWorkdir::new("gstackqlite-hypervisor-mcp-codex-test").expect("temp dir");
        let scope = McpScope::Project(temp.path().to_path_buf());

        let install = install_config(&scope, &[McpAgent::Codex]).expect("install should work");
        assert_eq!(install.len(), 1);
        assert_eq!(install[0].status, "installed");

        let config_path = temp.path().join(".codex").join("config.toml");
        let content = read(&config_path);
        assert!(content.contains(SERVER_NAME));
        assert!(content.contains(BINARY_NAME));

        let uninstall =
            uninstall_config(&scope, &[McpAgent::Codex]).expect("uninstall should work");
        assert!(uninstall.iter().any(|record| record.status == "removed"));
        let content = read(&config_path);
        assert!(!content.contains(SERVER_NAME));
    }
}
