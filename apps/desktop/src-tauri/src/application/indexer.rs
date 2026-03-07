use std::path::{Path, PathBuf};

use rusqlite::{params, Transaction};

use crate::adapters::db::{
    clear_repository_index_data, delete_file_changes_for_commits, load_commit_rowid,
    load_commit_window, load_index_checkpoint, open_database, run_migrations,
    set_repository_indexed_at, synchronize_refs, upsert_file_changes, upsert_index_checkpoint,
    upsert_repository,
};
use crate::adapters::git::{
    load_refs, stream_commits, stream_file_changes_for_commits, validate_repository,
    FILE_CHANGE_COMMIT_BATCH_SIZE,
};
use crate::application::graph_queries;
use crate::domain::models::{
    CommitWindowCursor, CommitWindowResponse, GraphDirection, GraphTraversalResponse,
    GraphTraversalStrategy, IndexCheckpoint, IndexingMode, IndexingStats, ParsedFileChange,
    RepositorySummary, ShortestPathResponse,
};
use crate::support::clock::now_epoch_seconds;
use crate::support::error::{AppError, AppResult};

const DEFAULT_WINDOW_LIMIT: usize = 100;
const MAX_WINDOW_LIMIT: usize = 500;
const INCREMENTAL_LOOKBACK_SECONDS: i64 = 300;
const FILE_CHANGE_DB_BATCH_SIZE: usize = 4_096;

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
        let since_epoch = match mode {
            IndexingMode::Full => None,
            IndexingMode::Incremental => {
                if let Some(checkpoint) = &checkpoint {
                    Some(
                        checkpoint
                            .last_committer_time
                            .saturating_sub(INCREMENTAL_LOOKBACK_SECONDS),
                    )
                } else {
                    effective_mode = IndexingMode::Full;
                    None
                }
            }
        };

        let refs = load_refs(&canonical_repo.root)?;
        let indexed_at_epoch = now_epoch_seconds();

        let tx = conn.transaction()?;
        if effective_mode == IndexingMode::Full {
            clear_repository_index_data(&tx, repo_id)?;
        }

        let mut commits_inserted = 0usize;
        let mut edges_inserted = 0usize;
        let mut file_changes_indexed = 0usize;
        let mut checkpoint_candidate: Option<(i64, String)> = None;
        let mut pending_commit_shas = Vec::with_capacity(FILE_CHANGE_COMMIT_BATCH_SIZE);
        let mut pending_file_changes = Vec::with_capacity(FILE_CHANGE_DB_BATCH_SIZE);

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

            stream_commits(&canonical_repo.root, since_epoch, |commit| {
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

                if pending_commit_shas.len() >= FILE_CHANGE_COMMIT_BATCH_SIZE {
                    flush_pending_commit_file_changes(
                        &tx,
                        &canonical_repo.root,
                        repo_id,
                        effective_mode,
                        &mut pending_commit_shas,
                        &mut pending_file_changes,
                        &mut file_changes_indexed,
                    )?;
                }
                pending_commit_shas.push(commit.sha.clone());
                update_checkpoint_candidate(
                    &mut checkpoint_candidate,
                    commit.committer_time,
                    &commit.sha,
                );

                Ok(())
            })?;
        }

        flush_pending_commit_file_changes(
            &tx,
            &canonical_repo.root,
            repo_id,
            effective_mode,
            &mut pending_commit_shas,
            &mut pending_file_changes,
            &mut file_changes_indexed,
        )?;
        if !pending_file_changes.is_empty() {
            file_changes_indexed = file_changes_indexed.saturating_add(upsert_file_changes(
                &tx,
                repo_id,
                &pending_file_changes,
            )?);
            pending_file_changes.clear();
        }

        let refs_indexed = synchronize_refs(&tx, repo_id, &refs)?;
        set_repository_indexed_at(&tx, repo_id, indexed_at_epoch)?;

        if let Some(checkpoint) = build_checkpoint(checkpoint_candidate, indexed_at_epoch) {
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

    pub fn traverse_graph(
        &self,
        path: &Path,
        start_sha: &str,
        strategy: GraphTraversalStrategy,
        direction: GraphDirection,
        max_nodes: usize,
    ) -> AppResult<GraphTraversalResponse> {
        let canonical_repo = validate_repository(path)?;
        let mut conn = open_database(&self.db_path)?;
        run_migrations(&mut conn)?;

        let repo_id = upsert_repository(
            &conn,
            &canonical_repo.root.display().to_string(),
            &canonical_repo.name,
            &canonical_repo.git_dir.display().to_string(),
        )?;

        let Some(start_rowid) = load_commit_rowid(&conn, repo_id, start_sha)? else {
            return Err(AppError::InvalidArgument(format!(
                "start commit not indexed for repository: {start_sha}"
            )));
        };

        graph_queries::traverse_graph(&conn, repo_id, start_rowid, strategy, direction, max_nodes)
    }

    pub fn shortest_path(
        &self,
        path: &Path,
        from_sha: &str,
        to_sha: &str,
        direction: GraphDirection,
    ) -> AppResult<ShortestPathResponse> {
        let canonical_repo = validate_repository(path)?;
        let mut conn = open_database(&self.db_path)?;
        run_migrations(&mut conn)?;

        let repo_id = upsert_repository(
            &conn,
            &canonical_repo.root.display().to_string(),
            &canonical_repo.name,
            &canonical_repo.git_dir.display().to_string(),
        )?;

        let Some(from_rowid) = load_commit_rowid(&conn, repo_id, from_sha)? else {
            return Err(AppError::InvalidArgument(format!(
                "source commit not indexed for repository: {from_sha}"
            )));
        };
        let Some(to_rowid) = load_commit_rowid(&conn, repo_id, to_sha)? else {
            return Err(AppError::InvalidArgument(format!(
                "target commit not indexed for repository: {to_sha}"
            )));
        };

        graph_queries::shortest_path(
            &conn, repo_id, from_sha, to_sha, direction, from_rowid, to_rowid,
        )
    }
}

fn flush_pending_commit_file_changes(
    tx: &Transaction<'_>,
    repo_root: &Path,
    repo_id: i64,
    mode: IndexingMode,
    pending_commit_shas: &mut Vec<String>,
    pending_file_changes: &mut Vec<ParsedFileChange>,
    file_changes_indexed: &mut usize,
) -> AppResult<()> {
    if pending_commit_shas.is_empty() {
        return Ok(());
    }

    if mode == IndexingMode::Incremental {
        delete_file_changes_for_commits(tx, repo_id, pending_commit_shas)?;
    }

    stream_file_changes_for_commits(repo_root, pending_commit_shas, |change| {
        pending_file_changes.push(change);
        if pending_file_changes.len() >= FILE_CHANGE_DB_BATCH_SIZE {
            *file_changes_indexed = file_changes_indexed.saturating_add(upsert_file_changes(
                tx,
                repo_id,
                pending_file_changes,
            )?);
            pending_file_changes.clear();
        }
        Ok(())
    })?;

    pending_commit_shas.clear();
    Ok(())
}

fn update_checkpoint_candidate(
    checkpoint_candidate: &mut Option<(i64, String)>,
    committer_time: i64,
    sha: &str,
) {
    match checkpoint_candidate {
        None => {
            *checkpoint_candidate = Some((committer_time, sha.to_string()));
        }
        Some((best_time, best_sha)) => {
            if committer_time > *best_time
                || (committer_time == *best_time && sha > best_sha.as_str())
            {
                *best_time = committer_time;
                *best_sha = sha.to_string();
            }
        }
    }
}

fn build_checkpoint(
    checkpoint_candidate: Option<(i64, String)>,
    indexed_at_epoch: i64,
) -> Option<IndexCheckpoint> {
    checkpoint_candidate.map(|(last_committer_time, last_commit_sha)| IndexCheckpoint {
        last_committer_time,
        last_commit_sha,
        indexed_at_epoch,
    })
}
