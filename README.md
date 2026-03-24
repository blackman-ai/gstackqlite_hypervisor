# gstackqlite_hypervisor

`gstackqlite_hypervisor` is a Rust, SQLite-first local supervisor for [gstack](https://github.com/garrytan/gstack), with a terminal UI and an installable CLI binary named `gstackqlite-hypervisor`.

It treats Git as an ingestion transport only:

- upstream gstack commit history and file manifests are stored in SQLite
- upstream version search is served from SQLite
- project and local install history are stored in SQLite
- upgrade ideas are generated from SQLite
- materialized local installs are updated from SQLite-backed snapshots
- repo-local customizations are preserved through a merge-aware apply flow with backups

## Current MVP

- Rust CLI and TUI
- SQLite catalog with:
  - upstream commit metadata
  - per-commit file manifests
  - cached blob contents for materializing snapshots
  - git, Claude, and Codex project records
  - local install records
  - scan history
  - apply and sync history
- local discovery for:
  - git repos, even before Claude/Codex setup
  - projects with `CLAUDE.md`
  - projects with `AGENTS.md`
  - projects with `.codex/config.toml`, `.codex/config.json`, `.codex/settings.toml`, `.codex/settings.json`, `.codex/settings.yaml`, or `.codex/settings.yml`
  - projects with `.claude/settings.json`, `.claude/settings.local.json`, `.claude/settings.yaml`, or `.claude/settings.yml`
  - `~/.claude/skills/gstack`
  - `~/.codex/skills/gstack`
  - `~/.gstack/repos/gstack`
  - repo-local `.claude/skills/gstack`
  - repo-local `.agents/skills/gstack`
- startup sync on TUI launch
- project-centric version browser and apply flow
- targeted revert flow from backup history
- pre-apply diff previews in both the TUI and CLI
- Rust stdio MCP server mode for external agents
- merge-aware apply with backup retention for local customizations
- optional generated lo-fi loop for the TUI, with startup-hub themed tracks and terminal palettes
- persisted TUI preferences for selected theme, track, and music on/off state

## Build

```bash
cargo build
```

Run the local dev binary:

```bash
cargo run
```

Install the CLI from the local checkout:

```bash
cargo install --path . --bin gstackqlite-hypervisor
```

## Distribution

For a public GitHub repo, the intended release flow is:

- push normal branches and PRs to run CI
- push a tag like `v0.0.1` to build release archives and publish a GitHub release
- let users install from the release page or via the installer script

Fastest Unix install command:

```bash
curl -fsSL https://raw.githubusercontent.com/blackman-ai/gstackqlite_hypervisor/main/scripts/install.sh | bash
```

The installer:

- detects macOS or Linux target architecture
- resolves `latest` to the newest GitHub release tag automatically
- downloads the matching release archive plus `SHA256SUMS`
- verifies the archive checksum before unpacking
- installs the binary to `~/.local/bin` by default
- updates `zsh`, `bash`, `fish`, or fallback profile config if that directory is not already on `PATH`
- can also be run directly from an extracted release archive with `./install.sh`

Pin a specific release instead of `latest`:

```bash
curl -fsSL https://raw.githubusercontent.com/blackman-ai/gstackqlite_hypervisor/main/scripts/install.sh | \
  GSTACKQLITE_HYPERVISOR_VERSION=v0.0.2 bash
```

Manual package install:

- macOS/Linux: download the release archive, extract it, and run `./install.sh`
- Windows: download the `.zip`, extract it, and run `.\install.ps1`

Release assets are built by [release.yml](/Users/michaelpoage/Work/gstackqlite_hypervisor/.github/workflows/release.yml). CI is in [ci.yml](/Users/michaelpoage/Work/gstackqlite_hypervisor/.github/workflows/ci.yml).

## Run

Open the TUI:

```bash
gstackqlite-hypervisor
```

Sync upstream history plus local project/install state:

```bash
gstackqlite-hypervisor sync --root ~/Work --root ~/src
```

List discovered git/Claude/Codex projects:

```bash
gstackqlite-hypervisor projects
```

Inspect one project:

```bash
gstackqlite-hypervisor project 12
```

List revertable backup events for one project:

```bash
gstackqlite-hypervisor history 12
```

Search historical upstream versions:

```bash
gstackqlite-hypervisor versions --search 0.11
```

Preview the diff between one project and a target version:

```bash
gstackqlite-hypervisor diff 12 --version 0.11.10.0
```

Dry-run a version apply against one project:

```bash
gstackqlite-hypervisor apply --project 12 --version 0.11.10.0 --dry-run
```

Apply a specific version to one or more projects:

```bash
gstackqlite-hypervisor apply --project 12 --project ~/Work/my-app --version 0.11.10.0
```

Dry-run a revert from the latest or selected backup event:

```bash
gstackqlite-hypervisor revert --project 12 --dry-run
gstackqlite-hypervisor revert --project 12 --event 44 --dry-run
```

Apply a targeted revert from backup history:

```bash
gstackqlite-hypervisor revert --project 12 --event 44
```

Raw upstream ingest:

```bash
gstackqlite-hypervisor ingest
```

Raw local scan:

```bash
gstackqlite-hypervisor scan --root ~/Work --root ~/src
```

List catalog installs:

```bash
gstackqlite-hypervisor list
```

Generate ideas:

```bash
gstackqlite-hypervisor ideas
```

Dry-run outdated upgrades:

```bash
gstackqlite-hypervisor upgrade --outdated --dry-run
```

Apply outdated upgrades:

```bash
gstackqlite-hypervisor upgrade --outdated
```

Run the MCP server over stdio:

```bash
gstackqlite-hypervisor mcp
```

Install the MCP server globally for both Claude and Codex:

```bash
gstackqlite-hypervisor mcp install --global
```

Install it for just one agent:

```bash
gstackqlite-hypervisor mcp install --agent claude
gstackqlite-hypervisor mcp install --agent codex
```

Install it into one project instead of your global agent config:

```bash
gstackqlite-hypervisor mcp install --project ~/Work/my-app
```

Inspect current MCP wiring:

```bash
gstackqlite-hypervisor mcp status --global
gstackqlite-hypervisor mcp status --project ~/Work/my-app
```

Remove the MCP wiring globally or for one project:

```bash
gstackqlite-hypervisor mcp uninstall --global
gstackqlite-hypervisor mcp uninstall --project ~/Work/my-app
```

## TUI keys

- `q`: quit
- `h` or `?`: open or close the in-app help modal
- `g`: sync upstream plus local project/install state
- `tab`: switch between the project list and version list
- `/`: start filtering the focused list
- `f`: clear the focused list filter
- `j` / `k`: move selection
- `left` / `right`: cycle file diff previews for the selected version
- `d`: dry-run apply of the selected version to the selected project
- `a`: apply the selected version to the selected project
- `b`: cycle backup-history entries for the selected project
- `z`: dry-run revert from the selected backup-history entry
- `x`: revert from the selected backup-history entry
- `m`: toggle the generated lo-fi loop
- `t`: cycle tracks (`Palo Alto Dawn`, `SoMa Afterhours`, `Shibuya Rain`, `Shenzhen Circuit`, `Seoul Rooftops`, `Flatiron Bebop`)
- `c`: cycle terminal themes (`Sand Hill Sandstone`, `Singapore Harbor`, `Bengaluru Garden`, `Shoreditch Neon`)
- `r`: refresh the catalog view

## Storage

- SQLite database: `~/.gstack/hypervisor/catalog.sqlite`
- Backups: `~/.gstack/hypervisor/backups/`

Override the database path with `--db /path/to/catalog.sqlite` or `GSTACKQLITE_HYPERVISOR_DB=/path/to/catalog.sqlite`.

## MCP Tools

The stdio MCP server exposes tool-style access to the same local catalog and actions:

- `sync_catalog`
- `list_projects`
- `project_detail`
- `project_history`
- `list_versions`
- `diff_preview`
- `apply_version`
- `revert_project`

`gstackqlite-hypervisor mcp install` edits Claude and Codex config files directly so users can turn the server on or off globally or per project without hand-editing JSON or TOML.

Use `gstackqlite-hypervisor mcp serve` when wiring it into another MCP client manually.
