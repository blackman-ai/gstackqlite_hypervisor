use crate::models::{CatalogInstall, Idea, SourceState};

pub fn build_ideas(installs: &[CatalogInstall], source: &SourceState) -> Vec<Idea> {
    let mut ideas = Vec::new();
    let outdated: Vec<_> = installs
        .iter()
        .filter(|install| install.is_outdated == Some(true))
        .collect();
    let git_backed: Vec<_> = installs.iter().filter(|install| install.has_git).collect();
    let dirty: Vec<_> = installs.iter().filter(|install| install.dirty).collect();
    let unknown_mapping: Vec<_> = installs
        .iter()
        .filter(|install| install.matched_upstream_commit_sha.is_none() && !install.has_git)
        .collect();

    let mut version_counts = std::collections::BTreeMap::<String, i64>::new();
    for install in installs {
        let key = install
            .local_version
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        *version_counts.entry(key).or_default() += 1;
    }

    if !outdated.is_empty() {
        ideas.push(Idea {
            severity: "high".to_string(),
            title: "Upgrade stale materialized installs".to_string(),
            rationale: match &source.head_version {
                Some(version) => format!(
                    "{} install(s) are behind upstream v{version}.",
                    outdated.len()
                ),
                None => format!(
                    "{} install(s) appear to be behind the upstream head snapshot.",
                    outdated.len()
                ),
            },
            action:
                "Run the upgrade flow against outdated installs or target individual install ids."
                    .to_string(),
            install_ids: outdated.iter().map(|install| install.id).collect(),
            paths: outdated
                .iter()
                .map(|install| install.observed_path.clone())
                .collect(),
        });
    }

    if !git_backed.is_empty() {
        ideas.push(Idea {
            severity: "medium".to_string(),
            title: "Migrate Git-backed installs toward SQLite-managed copies".to_string(),
            rationale: format!(
                "{} install(s) still depend on `.git` state instead of pure SQLite-backed materialization.",
                git_backed.len()
            ),
            action: "Use Git-backed installs as discovery inputs, then replace them with materialized snapshots managed from SQLite.".to_string(),
            install_ids: git_backed.iter().map(|install| install.id).collect(),
            paths: git_backed.iter().map(|install| install.observed_path.clone()).collect(),
        });
    }

    let non_unknown_versions = version_counts
        .iter()
        .filter(|(version, _)| version.as_str() != "unknown")
        .map(|(version, count)| format!("{version} ({count})"))
        .collect::<Vec<_>>();
    if non_unknown_versions.len() > 1 {
        ideas.push(Idea {
            severity: "medium".to_string(),
            title: "Standardize fragmented local versions".to_string(),
            rationale: format!("Local gstack versions are fragmented: {}.", non_unknown_versions.join(", ")),
            action: "Choose a single SQLite-backed target version and roll installs forward consistently.".to_string(),
            install_ids: installs.iter().map(|install| install.id).collect(),
            paths: installs.iter().map(|install| install.observed_path.clone()).collect(),
        });
    }

    if !dirty.is_empty() {
        ideas.push(Idea {
            severity: "info".to_string(),
            title: "Review dirty installs before replacing them".to_string(),
            rationale: format!("{} install(s) have local modifications.", dirty.len()),
            action:
                "Back up or explicitly record those changes before materializing a new snapshot."
                    .to_string(),
            install_ids: dirty.iter().map(|install| install.id).collect(),
            paths: dirty
                .iter()
                .map(|install| install.observed_path.clone())
                .collect(),
        });
    }

    if !unknown_mapping.is_empty() {
        ideas.push(Idea {
            severity: "info".to_string(),
            title: "Investigate installs that do not map cleanly to upstream history".to_string(),
            rationale: format!(
                "{} install(s) could not be matched to an upstream commit via manifest hash, commit hints, or version.",
                unknown_mapping.len()
            ),
            action: "Inspect those installs for local patches and decide whether to preserve or replace them.".to_string(),
            install_ids: unknown_mapping.iter().map(|install| install.id).collect(),
            paths: unknown_mapping.iter().map(|install| install.observed_path.clone()).collect(),
        });
    }

    ideas
}
