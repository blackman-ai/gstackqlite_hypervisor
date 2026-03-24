# gstackqlite_hypervisor

`gstackqlite_hypervisor` is a Rust, SQLite-first local supervisor for [gstack](https://github.com/garrytan/gstack), with a terminal UI as the primary interface.

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
  - Claude-enabled project records
  - local install records
  - scan history
  - apply and sync history
- local discovery for:
  - projects with `CLAUDE.md`
  - projects with `.claude/settings.json`, `.claude/settings.local.json`, `.claude/settings.yaml`, or `.claude/settings.yml`
  - `~/.claude/skills/gstack`
  - `~/.codex/skills/gstack`
  - `~/.gstack/repos/gstack`
  - repo-local `.claude/skills/gstack`
  - repo-local `.agents/skills/gstack`
- startup sync on TUI launch
- project-centric version browser and apply flow
- merge-aware apply with backup retention for local customizations
- optional generated lo-fi loop for the TUI, with startup-hub themed tracks and terminal palettes
- persisted TUI preferences for selected theme, track, and music on/off state

## Build

```bash
cargo build
```

## Distribution

For a public GitHub repo, the intended release flow is:

- push normal branches and PRs to run CI
- push a tag like `v0.1.0` to build release archives and publish a GitHub release
- let users install from the release page or via the installer script

Unix install command:

```bash
curl -fsSL https://raw.githubusercontent.com/YOUR_ORG/YOUR_REPO/main/scripts/install.sh | \
  GSTACK_HYPERVISOR_REPO=YOUR_ORG/YOUR_REPO bash
```

The installer:

- detects macOS or Linux target architecture
- downloads the matching release archive plus `SHA256SUMS`
- verifies the archive checksum before unpacking
- installs the binary to `~/.local/bin` by default
- updates `zsh`, `bash`, `fish`, or fallback profile config if that directory is not already on `PATH`

Release assets are built by [release.yml](/Users/michaelpoage/Work/gstackqlite_hypervisor/.github/workflows/release.yml). CI is in [ci.yml](/Users/michaelpoage/Work/gstackqlite_hypervisor/.github/workflows/ci.yml).

## Run

Open the TUI:

```bash
cargo run
```

Sync upstream history plus local project/install state:

```bash
cargo run -- sync --root ~/Work --root ~/src
```

List Claude-enabled projects:

```bash
cargo run -- projects
```

Inspect one project:

```bash
cargo run -- project 12
```

Search historical upstream versions:

```bash
cargo run -- versions --search 0.11
```

Dry-run a version apply against one project:

```bash
cargo run -- apply --project 12 --version 0.11.10.0 --dry-run
```

Apply a specific version to one or more projects:

```bash
cargo run -- apply --project 12 --project ~/Work/my-app --version 0.11.10.0
```

Raw upstream ingest:

```bash
cargo run -- ingest
```

Raw local scan:

```bash
cargo run -- scan --root ~/Work --root ~/src
```

List catalog installs:

```bash
cargo run -- list
```

Generate ideas:

```bash
cargo run -- ideas
```

Dry-run outdated upgrades:

```bash
cargo run -- upgrade --outdated --dry-run
```

Apply outdated upgrades:

```bash
cargo run -- upgrade --outdated
```

## TUI keys

- `q`: quit
- `h` or `?`: open or close the in-app help modal
- `g`: sync upstream plus local project/install state
- `tab`: switch between the project list and version list
- `/`: start filtering the focused list
- `f`: clear the focused list filter
- `j` / `k`: move selection
- `d`: dry-run apply of the selected version to the selected project
- `a`: apply the selected version to the selected project
- `m`: toggle the generated lo-fi loop
- `t`: cycle tracks (`Palo Alto Dawn`, `SoMa Afterhours`, `Shibuya Rain`, `Shenzhen Circuit`, `Seoul Rooftops`, `Flatiron Bebop`)
- `c`: cycle terminal themes (`Sandhill Sandstone`, `Singapore Harbor`, `Bengaluru Garden`, `Shoreditch Neon`)
- `r`: refresh the catalog view

## Storage

- SQLite database: `~/.gstack/hypervisor/catalog.sqlite`
- Backups: `~/.gstack/hypervisor/backups/`

Override the database path with `--db /path/to/catalog.sqlite` or `GSTACK_HYPERVISOR_DB=/path/to/catalog.sqlite`.

## Next steps

- richer revert flows from backup history
- diff views inside the TUI before apply
- MCP server in Rust
