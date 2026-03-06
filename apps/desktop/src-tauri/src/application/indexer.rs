use std::path::{Path, PathBuf};

use rusqlite::params;

use crate::adapters::db::{
    clear_repository_index_data, delete_file_changes_for_commits, load_commit_window,
    load_index_checkpoint, open_database, run_migrations, set_repository_indexed_at,
    synchronize_refs, upsert_file_changes, upsert_index_checkpoint, upsert_repository,
};
use crate::adapters::git::{
    load_commits, load_commits_since, load_file_changes_for_commits, load_refs, validate_repository,
};
use crate::domain::models::{
    CommitWindowCursor, CommitWindowResponse, IndexCheckpoint, IndexingMode, IndexingStats,
    RepositorySummary,
};
use crate::support::clock::now_epoch_seconds;
use crate::support::error::AppResult;

const DEFAULT_WINDOW_LIMIT: usize = 100;
const MAX_WINDOW_LIMIT: usize = 500;
const INCREMENTAL_LOOKBACK_SECONDS: i64 = 300;

pub struct Indexer {
    db_path: PathBuf,
}

impl Indexer {
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }

    pub fn default_db_path() -> PathBuf {
        PathBuf::from(".gitgraph-lab/gitgraph-lab.db")
    }

    pub fn scan_repository(&self, path: &Path) -> AppResult<RepositorySummary> {
        let canonical_repo = validate_repository(path)?;
        let mut conn = open_database(&self.db_path)?;
        run_migrations(&mut conn)?;
        upsert_repository(
            &conn,
            &canonical_repo.root.display().to_string(),
            &canonical_repo.name,
            &canonical_repo.git_dir.display().to_string(),
        )?;

        Ok(RepositorySummary {
            path: canonical_repo.root.display().to_string(),
            git_dir: canonical_repo.git_dir.display().to_string(),
            name: canonical_repo.name,
            is_valid: true,
        })
    }

    pub fn index_repository(&self, path: &Path, mode: IndexingMode) -> AppResult<IndexingStats> {
        let canonical_repo = validate_repository(path)?;
        let mut conn = open_database(&self.db_path)?;
        run_migrations(&mut conn)?;

        let repo_id = upsert_repository(
            &conn,
            &canonical_repo.root.display().to_string(),
            &canonical_repo.name,
            &canonical_repo.git_dir.display().to_string(),
        )?;

        let checkpoint = load_index_checkpoint(&conn, repo_id)?;
        let mut effective_mode = mode;
        let commits = match mode {
            IndexingMode::Full => load_commits(&canonical_repo.root)?,
            IndexingMode::Incremental => {
                if let Some(checkpoint) = &checkpoint {
                    let since = checkpoint
                        .last_committer_time
                        .saturating_sub(INCREMENTAL_LOOKBACK_SECONDS);
                    load_commits_since(&canonical_repo.root, since)?
                } else {
                    effective_mode = IndexingMode::Full;
                    load_commits(&canonical_repo.root)?
                }
            }
        };

        let commit_shas: Vec<String> = commits.iter().map(|commit| commit.sha.clone()).collect();
        let file_changes = load_file_changes_for_commits(&canonical_repo.root, &commit_shas)?;

        let refs = load_refs(&canonical_repo.root)?;
        let indexed_at_epoch = now_epoch_seconds();

        let tx = conn.transaction()?;
        if effective_mode == IndexingMode::Full {
            clear_repository_index_data(&tx, repo_id)?;
        } else {
            delete_file_changes_for_commits(&tx, repo_id, &commit_shas)?;
        }

        let mut commits_inserted = 0usize;
        let mut edges_inserted = 0usize;

        {
            let mut commit_stmt = tx.prepare(
                r#"
                INSERT INTO commits (
                  sha, repo_id, author_name, author_email, author_time, committer_time, subject, tree_sha
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(repo_id, sha) DO UPDATE SET
                  author_name = excluded.author_name,
                  author_email = excluded.author_email,
                  author_time = excluded.author_time,
                  committer_time = excluded.committer_time,
                  subject = excluded.subject,
                  tree_sha = excluded.tree_sha
            "#,
            )?;

            let mut edge_stmt = tx.prepare(
                r#"
                INSERT INTO commit_edges (child_sha, parent_sha, repo_id)
                VALUES (?1, ?2, ?3)
                ON CONFLICT(repo_id, child_sha, parent_sha) DO NOTHING
            "#,
            )?;

            for commit in &commits {
                commit_stmt.execute(params![
                    commit.sha.as_str(),
                    repo_id,
                    commit.author_name.as_str(),
                    commit.author_email.as_str(),
                    commit.author_time,
                    commit.committer_time,
                    commit.subject.as_str(),
                    commit.tree_sha.as_str()
                ])?;
                commits_inserted += 1;

                for parent in &commit.parents {
                    edge_stmt.execute(params![commit.sha.as_str(), parent.as_str(), repo_id])?;
                    edges_inserted += 1;
                }
            }
        }

        let file_changes_indexed = upsert_file_changes(&tx, repo_id, &file_changes)?;
        let refs_indexed = synchronize_refs(&tx, repo_id, &refs)?;
        set_repository_indexed_at(&tx, repo_id, indexed_at_epoch)?;

        if let Some(checkpoint) = build_checkpoint(&commits, indexed_at_epoch) {
            upsert_index_checkpoint(&tx, repo_id, &checkpoint)?;
        }

        tx.commit()?;

        Ok(IndexingStats {
            repo_id,
            mode: effective_mode,
            commits_indexed: commits_inserted,
            edges_indexed: edges_inserted,
            file_changes_indexed,
            refs_indexed,
            indexed_at_epoch,
        })
    }

    pub fn get_commit_window(
        &self,
        path: &Path,
        limit: Option<usize>,
        cursor: Option<CommitWindowCursor>,
    ) -> AppResult<CommitWindowResponse> {
        let canonical_repo = validate_repository(path)?;
        let mut conn = open_database(&self.db_path)?;
        run_migrations(&mut conn)?;

        let repo_id = upsert_repository(
            &conn,
            &canonical_repo.root.display().to_string(),
            &canonical_repo.name,
            &canonical_repo.git_dir.display().to_string(),
        )?;

        let requested_limit = limit.unwrap_or(DEFAULT_WINDOW_LIMIT);
        let bounded_limit = requested_limit.clamp(1, MAX_WINDOW_LIMIT);
        let fetch_limit = bounded_limit.saturating_add(1);

        let mut commits = load_commit_window(&conn, repo_id, fetch_limit, cursor.as_ref())?;
        let has_more = commits.len() > bounded_limit;
        if has_more {
            commits.truncate(bounded_limit);
        }

        let next_cursor = if has_more {
            commits.last().map(|commit| CommitWindowCursor {
                committer_time: commit.committer_time,
                sha: commit.sha.clone(),
            })
        } else {
            None
        };

        Ok(CommitWindowResponse {
            commits,
            next_cursor,
            has_more,
        })
    }
}

fn build_checkpoint(
    commits: &[crate::domain::models::ParsedCommit],
    indexed_at_epoch: i64,
) -> Option<IndexCheckpoint> {
    commits
        .iter()
        .max_by(|left, right| {
            left.committer_time
                .cmp(&right.committer_time)
                .then_with(|| left.sha.cmp(&right.sha))
        })
        .map(|commit| IndexCheckpoint {
            last_committer_time: commit.committer_time,
            last_commit_sha: commit.sha.clone(),
            indexed_at_epoch,
        })
}
