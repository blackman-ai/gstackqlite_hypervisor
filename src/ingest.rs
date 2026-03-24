use anyhow::{Context, Result};

use crate::db::Catalog;
use crate::git;
use crate::manifest::manifest_hash;
use crate::models::{IngestSummary, UpstreamCommitRecord};
use crate::util::TempWorkdir;

pub fn ensure_catalog_has_upstream(catalog: &Catalog) -> Result<()> {
    if catalog.source_state()?.head_commit_sha.is_some() {
        return Ok(());
    }
    ingest_upstream(catalog, None, None).map(|_| ())
}

pub fn ingest_upstream(
    catalog: &Catalog,
    repo_url: Option<&str>,
    reference: Option<&str>,
) -> Result<IngestSummary> {
    let source = catalog.source_state()?;
    let repo_url = repo_url.unwrap_or(source.repo_url.as_str()).to_string();
    let reference = reference.unwrap_or(source.default_ref.as_str()).to_string();
    let temp = TempWorkdir::new("gstackqlite-hypervisor")?;

    let result = (|| -> Result<IngestSummary> {
        git::clone_repo(&repo_url, temp.path())?;
        let head_sha = git::rev_parse(temp.path(), &reference)?;
        let commit_shas = git::rev_list(temp.path(), &reference)?;

        for sha in &commit_shas {
            let metadata = git::show_commit_metadata(temp.path(), sha)?;
            let version = git::show_file(temp.path(), sha, "VERSION")
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            let tree = git::list_tree(temp.path(), sha)?;
            let commit = UpstreamCommitRecord {
                sha: metadata.sha,
                source_id: source.id,
                parents: metadata.parents,
                author_name: metadata.author_name,
                author_email: metadata.author_email,
                authored_at: metadata.authored_at,
                committed_at: metadata.committed_at,
                subject: metadata.subject,
                body: metadata.body,
                version,
                manifest_hash: manifest_hash(
                    &tree
                        .iter()
                        .map(|entry| {
                            (
                                entry.path.clone(),
                                entry.blob_sha.clone(),
                                entry.mode.clone(),
                            )
                        })
                        .collect::<Vec<_>>(),
                ),
            };
            catalog.upsert_commit(&commit)?;
            catalog.replace_commit_files(sha, &tree)?;
        }

        let hydrated_blob_count = hydrate_commit_contents(catalog, temp.path(), &head_sha)?;
        let head_version = catalog
            .get_commit_by_sha(&head_sha)?
            .and_then(|(_, version)| version);
        catalog.update_source_state(
            &repo_url,
            &reference,
            Some(&head_sha),
            head_version.as_deref(),
            None,
        )?;

        Ok(IngestSummary {
            repo_url: repo_url.clone(),
            reference: reference.clone(),
            head_sha,
            head_version,
            commit_count: commit_shas.len(),
            hydrated_blob_count,
        })
    })();

    if let Err(error) = &result {
        let _ = catalog.update_source_state(
            &repo_url,
            &reference,
            source.head_commit_sha.as_deref(),
            source.head_version.as_deref(),
            Some(&error.to_string()),
        );
    }

    result.with_context(|| "failed to ingest upstream repo")
}

pub fn hydrate_commit_contents(
    catalog: &Catalog,
    repo_dir: &std::path::Path,
    commit_sha: &str,
) -> Result<usize> {
    let blob_shas = catalog.commit_blob_shas(commit_sha)?;
    let missing = catalog.missing_blob_shas(&blob_shas)?;
    let mut hydrated = 0usize;
    for blob_sha in missing {
        let content = git::cat_file(repo_dir, &blob_sha)?;
        catalog.upsert_blob(&blob_sha, content.len() as i64, &content)?;
        hydrated += 1;
    }
    Ok(hydrated)
}

pub fn hydrate_commit_by_sha(catalog: &Catalog, commit_sha: &str) -> Result<usize> {
    let source = catalog.source_state()?;
    let temp = TempWorkdir::new("gstackqlite-hypervisor-hydrate")?;
    git::clone_repo(&source.repo_url, temp.path())?;
    hydrate_commit_contents(catalog, temp.path(), commit_sha)
}
