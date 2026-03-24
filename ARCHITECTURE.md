# gstackqlite_hypervisor Architecture

The hypervisor is intentionally SQLite-first and Rust-native.

## Source of truth

Git is used only to pull upstream history into a temporary checkout. The durable model lives in SQLite:

- upstream commit metadata
- parent SHAs
- per-commit file manifests
- cached blob contents for materialization
- discovered Claude-enabled projects
- local install records
- scan history
- sync history

## Runtime layers

1. Ingestion
   - Clone upstream gstack to a temporary directory.
   - Walk every commit on the selected ref.
   - Store commit metadata and file manifests in SQLite.
   - Hydrate blob contents for the current head commit so upgrades can materialize directly from SQLite.

2. Discovery
   - Scan filesystem roots for project directories, Git repos, Claude settings, and known local/global install paths.
   - Compute local manifest hashes and version hints.
   - Match installs back to upstream commits using SQLite.
   - Record effective gstack version per project, including fallback to global installs when no repo-local install exists.

3. Reasoning
   - Build ideas from the SQLite catalog:
     - stale installs
     - fragmented versions
     - dirty installs
     - Git-backed installs that should move to SQLite-backed materialization

4. Materialization
   - Read a commit snapshot from SQLite.
   - Resolve a project target and its current install state.
   - Hydrate historical blobs on demand when an older version is requested.
   - Diff target, local state, and matched upstream base state.
   - Preserve unchanged local customizations, merge text conflicts, and back up the full pre-apply install.
   - Write the new snapshot from SQLite to disk and record the apply in SQLite.

5. Interface
   - Rust CLI for sync, project listing, version search, and apply operations.
   - Rust TUI for startup sync, project/version browsing, and per-project apply actions.
   - Optional generated lo-fi playback while the TUI is open.

## Tables

- `upstream_sources`
- `upstream_commits`
- `upstream_commit_files`
- `upstream_blobs`
- `repositories`
- `projects`
- `local_installs`
- `scan_runs`
- `install_observations`
- `project_observations`
- `sync_events`

## Current limits

- head-commit blobs are hydrated automatically; historical blobs are hydrated lazily when a target version is applied
- merge behavior is conservative: divergent text edits become conflict-marked files, and non-text conflicts preserve the local file with the incoming version written into the backup area
- there is not yet a first-class revert command; backups are retained on disk and recorded in apply history
- MCP is still a planned Rust follow-up, not part of this first TUI-centric slice
