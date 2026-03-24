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

    let install_count_label = |count: usize| {
        if count == 1 {
            "1 local gstack copy".to_string()
        } else {
            format!("{count} local gstack copies")
        }
    };

    if !outdated.is_empty() {
        let noun = if outdated.len() == 1 {
            "local gstack copy is"
        } else {
            "local gstack copies are"
        };
        ideas.push(Idea {
            severity: "high".to_string(),
            title: if outdated.len() == 1 {
                "A project is using an older local gstack copy".to_string()
            } else {
                "Some projects are using older local gstack copies".to_string()
            },
            rationale: match &source.head_version {
                Some(version) => format!(
                    "{} {} behind upstream v{version}.",
                    outdated.len(),
                    noun
                ),
                None => format!(
                    "{} {} behind the current upstream snapshot.",
                    outdated.len(),
                    noun
                ),
            },
            action: "Select the affected project and apply the target version if you want to bring it forward."
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
            title: "Some gstack copies are still managed directly from Git".to_string(),
            rationale: format!(
                "{} still depend on `.git` state instead of the SQLite catalog.",
                install_count_label(git_backed.len())
            ),
            action: "If you want centralized control, replace those Git-managed copies with catalog-managed project installs.".to_string(),
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
            title: "Projects are split across different gstack versions".to_string(),
            rationale: format!(
                "Workspace versions are fragmented: {}.",
                non_unknown_versions.join(", ")
            ),
            action: "Pick a target version and roll projects toward it intentionally instead of letting versions drift."
                .to_string(),
            install_ids: installs.iter().map(|install| install.id).collect(),
            paths: installs.iter().map(|install| install.observed_path.clone()).collect(),
        });
    }

    if !dirty.is_empty() {
        ideas.push(Idea {
            severity: "info".to_string(),
            title: "Some local gstack copies have custom edits".to_string(),
            rationale: format!(
                "{} differ from a clean upstream snapshot.",
                install_count_label(dirty.len())
            ),
            action: "Review those customizations before replacing them; apply will preserve or merge them, but you should still know they exist.".to_string(),
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
            title: "Some local gstack copies do not map cleanly to upstream history".to_string(),
            rationale: format!(
                "{} could not be matched to upstream by manifest, commit hint, or version.",
                install_count_label(unknown_mapping.len())
            ),
            action: "Inspect those projects before changing them; they may contain local patches or manually copied files."
                .to_string(),
            install_ids: unknown_mapping.iter().map(|install| install.id).collect(),
            paths: unknown_mapping.iter().map(|install| install.observed_path.clone()).collect(),
        });
    }

    ideas
}
