use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rusqlite::{params, Transaction};

use crate::adapters::db::{
    apply_file_change_line_stats_updates, clear_repository_index_data,
    delete_file_changes_for_commits, enqueue_file_changes_for_enrichment, load_commit_rowid,
    load_commit_window, load_index_checkpoint, load_pending_file_change_enrichment,
    load_pending_file_change_enrichment_for_commits, mark_file_change_enrichment_failed,
    open_database, run_migrations, set_repository_indexed_at, synchronize_refs,
    upsert_file_changes, upsert_index_checkpoint, upsert_repository,
};
use crate::adapters::git::{
    load_file_change_line_stats_for_targets, load_refs, stream_commits, stream_commits_with_hide,
    stream_file_changes_for_commits, validate_repository, FILE_CHANGE_COMMIT_BATCH_SIZE,
};
use crate::application::graph_queries;
use crate::domain::models::{
    CommitWindowCursor, CommitWindowResponse, GraphDirection, GraphTraversalResponse,
    GraphTraversalStrategy, IndexCheckpoint, IndexingMode, IndexingStats, IngestionPhaseTimingMs,
    ParsedFileChange, PendingFileChangeEnrichment, RepositorySummary, ShortestPathResponse,
};
use crate::support::clock::now_epoch_seconds;
use crate::support::error::{AppError, AppResult};

const DEFAULT_WINDOW_LIMIT: usize = 100;
const MAX_WINDOW_LIMIT: usize = 5_000;
const INCREMENTAL_LOOKBACK_SECONDS: i64 = 300;
const FILE_CHANGE_DB_BATCH_SIZE: usize = 4_096;
const FILE_CHANGE_JIT_ENRICH_BATCH_SIZE: usize = 1_024;

#[derive(Default)]
struct IngestionPhaseTiming {
    commit_walk: Duration,
    commit_upsert: Duration,
    edge_materialize: Duration,
    file_diff_extract: Duration,
    file_change_delete: Duration,
    file_change_upsert: Duration,
    tx_commit: Duration,
}

impl IngestionPhaseTiming {
    fn into_ms(self) -> IngestionPhaseTimingMs {
        IngestionPhaseTimingMs {
            commit_walk: duration_ms(self.commit_walk),
            commit_upsert: duration_ms(self.commit_upsert),
            edge_materialize: duration_ms(self.edge_materialize),
            file_diff_extract: duration_ms(self.file_diff_extract),
            file_change_delete: duration_ms(self.file_change_delete),
            file_change_upsert: duration_ms(self.file_change_upsert),
            tx_commit: duration_ms(self.tx_commit),
        }
    }
}

#[derive(Default)]
struct FileChangeFlushStats {
    file_changes_indexed: usize,
    file_diff_extract: Duration,
    file_change_delete: Duration,
    file_change_upsert: Duration,
}

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
        let mut ingestion_timing = IngestionPhaseTiming::default();
        let mut checkpoint_candidate: Option<(i64, String)> = None;
        let mut pending_commit_shas = Vec::with_capacity(FILE_CHANGE_COMMIT_BATCH_SIZE);
        let mut pending_file_changes = Vec::with_capacity(FILE_CHANGE_DB_BATCH_SIZE);

        {
            tx.execute_batch(
                r#"
                CREATE TEMP TABLE IF NOT EXISTS current_pending_commit_edges (
                  child_sha TEXT NOT NULL,
                  parent_sha TEXT NOT NULL
                );
                DELETE FROM current_pending_commit_edges;
            "#,
            )?;

            let mut pending_edge_stmt = tx.prepare_cached(
                r#"
                INSERT INTO current_pending_commit_edges (child_sha, parent_sha)
                VALUES (?1, ?2)
            "#,
            )?;

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

            let commit_walk_started = Instant::now();
            let mut commit_callback_elapsed = Duration::default();
            let hide_ancestor_sha = checkpoint
                .as_ref()
                .filter(|_| effective_mode == IndexingMode::Incremental)
                .map(|checkpoint| checkpoint.last_commit_sha.as_str());
            let stream_result = if hide_ancestor_sha.is_some() {
                stream_commits_with_hide(
                    &canonical_repo.root,
                    since_epoch,
                    hide_ancestor_sha,
                    |commit| {
                        let callback_started = Instant::now();
                        let commit_upsert_started = Instant::now();
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
                            pending_edge_stmt
                                .execute(params![commit.sha.as_str(), parent.as_str()])?;
                            edges_inserted += 1;
                        }
                        ingestion_timing.commit_upsert += commit_upsert_started.elapsed();

                        if pending_commit_shas.len() >= FILE_CHANGE_COMMIT_BATCH_SIZE {
                            let flush_stats = flush_pending_commit_file_changes(
                                &tx,
                                &canonical_repo.root,
                                repo_id,
                                effective_mode,
                                &mut pending_commit_shas,
                                &mut pending_file_changes,
                            )?;
                            file_changes_indexed = file_changes_indexed
                                .saturating_add(flush_stats.file_changes_indexed);
                            ingestion_timing.file_diff_extract += flush_stats.file_diff_extract;
                            ingestion_timing.file_change_delete += flush_stats.file_change_delete;
                            ingestion_timing.file_change_upsert += flush_stats.file_change_upsert;
                        }
                        pending_commit_shas.push(commit.sha.clone());
                        update_checkpoint_candidate(
                            &mut checkpoint_candidate,
                            commit.committer_time,
                            &commit.sha,
                        );
                        commit_callback_elapsed += callback_started.elapsed();

                        Ok(())
                    },
                )
            } else {
                stream_commits(&canonical_repo.root, since_epoch, |commit| {
                    let callback_started = Instant::now();
                    let commit_upsert_started = Instant::now();
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
                        pending_edge_stmt.execute(params![commit.sha.as_str(), parent.as_str()])?;
                        edges_inserted += 1;
                    }
                    ingestion_timing.commit_upsert += commit_upsert_started.elapsed();

                    if pending_commit_shas.len() >= FILE_CHANGE_COMMIT_BATCH_SIZE {
                        let flush_stats = flush_pending_commit_file_changes(
                            &tx,
                            &canonical_repo.root,
                            repo_id,
                            effective_mode,
                            &mut pending_commit_shas,
                            &mut pending_file_changes,
                        )?;
                        file_changes_indexed =
                            file_changes_indexed.saturating_add(flush_stats.file_changes_indexed);
                        ingestion_timing.file_diff_extract += flush_stats.file_diff_extract;
                        ingestion_timing.file_change_delete += flush_stats.file_change_delete;
                        ingestion_timing.file_change_upsert += flush_stats.file_change_upsert;
                    }
                    pending_commit_shas.push(commit.sha.clone());
                    update_checkpoint_candidate(
                        &mut checkpoint_candidate,
                        commit.committer_time,
                        &commit.sha,
                    );
                    commit_callback_elapsed += callback_started.elapsed();

                    Ok(())
                })
            };
            stream_result?;
            ingestion_timing.commit_walk += commit_walk_started
                .elapsed()
                .saturating_sub(commit_callback_elapsed);

            let edge_materialize_started = Instant::now();
            tx.execute(
                r#"
                INSERT INTO commit_edges (repo_id, child_commit_id, parent_commit_id)
                SELECT
                  ?1 AS repo_id,
                  child.commit_id AS child_commit_id,
                  parent.commit_id AS parent_commit_id
                FROM current_pending_commit_edges edge
                JOIN commits child
                  ON child.repo_id = ?1
                 AND child.sha = edge.child_sha
                JOIN commits parent
                  ON parent.repo_id = ?1
                 AND parent.sha = edge.parent_sha
                ON CONFLICT(repo_id, child_commit_id, parent_commit_id) DO NOTHING
            "#,
                params![repo_id],
            )?;
            ingestion_timing.edge_materialize += edge_materialize_started.elapsed();
        }

        let flush_stats = flush_pending_commit_file_changes(
            &tx,
            &canonical_repo.root,
            repo_id,
            effective_mode,
            &mut pending_commit_shas,
            &mut pending_file_changes,
        )?;
        file_changes_indexed =
            file_changes_indexed.saturating_add(flush_stats.file_changes_indexed);
        ingestion_timing.file_diff_extract += flush_stats.file_diff_extract;
        ingestion_timing.file_change_delete += flush_stats.file_change_delete;
        ingestion_timing.file_change_upsert += flush_stats.file_change_upsert;
        if !pending_file_changes.is_empty() {
            let file_change_upsert_started = Instant::now();
            file_changes_indexed = file_changes_indexed.saturating_add(upsert_file_changes(
                &tx,
                repo_id,
                &pending_file_changes,
            )?);
            enqueue_file_changes_for_enrichment(&tx, repo_id, &pending_file_changes)?;
            ingestion_timing.file_change_upsert += file_change_upsert_started.elapsed();
            pending_file_changes.clear();
        }

        let refs_indexed = synchronize_refs(&tx, repo_id, &refs)?;
        set_repository_indexed_at(&tx, repo_id, indexed_at_epoch)?;

        if let Some(checkpoint) = build_checkpoint(checkpoint_candidate, indexed_at_epoch) {
            upsert_index_checkpoint(&tx, repo_id, &checkpoint)?;
        }

        let tx_commit_started = Instant::now();
        tx.commit()?;
        ingestion_timing.tx_commit += tx_commit_started.elapsed();

        Ok(IndexingStats {
            repo_id,
            mode: effective_mode,
            commits_indexed: commits_inserted,
            edges_indexed: edges_inserted,
            file_changes_indexed,
            refs_indexed,
            indexed_at_epoch,
            ingestion_phase_timing_ms: ingestion_timing.into_ms(),
        })
    }

    pub fn enrich_pending_file_change_stats(
        &self,
        path: &Path,
        max_items: Option<usize>,
    ) -> AppResult<usize> {
        let canonical_repo = validate_repository(path)?;
        let mut conn = open_database(&self.db_path)?;
        run_migrations(&mut conn)?;

        let repo_id = upsert_repository(
            &conn,
            &canonical_repo.root.display().to_string(),
            &canonical_repo.name,
            &canonical_repo.git_dir.display().to_string(),
        )?;
        let batch_size = max_items
            .unwrap_or(FILE_CHANGE_JIT_ENRICH_BATCH_SIZE)
            .max(1);
        let pending = load_pending_file_change_enrichment(&conn, repo_id, batch_size)?;
        enrich_file_change_stats_batch(&canonical_repo.root, &mut conn, repo_id, &pending)
    }

    pub fn ensure_file_change_stats_for_commits(
        &self,
        path: &Path,
        commit_shas: &[String],
        max_items: Option<usize>,
    ) -> AppResult<usize> {
        if commit_shas.is_empty() {
            return Ok(0);
        }

        let canonical_repo = validate_repository(path)?;
        let mut conn = open_database(&self.db_path)?;
        run_migrations(&mut conn)?;

        let repo_id = upsert_repository(
            &conn,
            &canonical_repo.root.display().to_string(),
            &canonical_repo.name,
            &canonical_repo.git_dir.display().to_string(),
        )?;
        let batch_size = max_items
            .unwrap_or(FILE_CHANGE_JIT_ENRICH_BATCH_SIZE)
            .max(1);
        let pending = load_pending_file_change_enrichment_for_commits(
            &conn,
            repo_id,
            commit_shas,
            batch_size,
        )?;
        enrich_file_change_stats_batch(&canonical_repo.root, &mut conn, repo_id, &pending)
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
) -> AppResult<FileChangeFlushStats> {
    let mut stats = FileChangeFlushStats::default();
    if pending_commit_shas.is_empty() {
        return Ok(stats);
    }

    if mode == IndexingMode::Incremental {
        let delete_started = Instant::now();
        delete_file_changes_for_commits(tx, repo_id, pending_commit_shas)?;
        stats.file_change_delete += delete_started.elapsed();
    }

    let extract_started = Instant::now();
    let mut upsert_duration = Duration::default();
    stream_file_changes_for_commits(repo_root, pending_commit_shas, |change| {
        pending_file_changes.push(change);
        if pending_file_changes.len() >= FILE_CHANGE_DB_BATCH_SIZE {
            let upsert_started = Instant::now();
            stats.file_changes_indexed = stats
                .file_changes_indexed
                .saturating_add(upsert_file_changes(tx, repo_id, pending_file_changes)?);
            enqueue_file_changes_for_enrichment(tx, repo_id, pending_file_changes)?;
            upsert_duration += upsert_started.elapsed();
            pending_file_changes.clear();
        }
        Ok(())
    })?;
    let extract_elapsed = extract_started.elapsed();
    stats.file_change_upsert += upsert_duration;
    stats.file_diff_extract += extract_elapsed.saturating_sub(upsert_duration);

    pending_commit_shas.clear();
    Ok(stats)
}

fn enrich_file_change_stats_batch(
    repo_root: &Path,
    conn: &mut rusqlite::Connection,
    repo_id: i64,
    pending: &[PendingFileChangeEnrichment],
) -> AppResult<usize> {
    if pending.is_empty() {
        return Ok(0);
    }

    let updates = match load_file_change_line_stats_for_targets(repo_root, pending) {
        Ok(updates) => updates,
        Err(error) => {
            let tx = conn.transaction()?;
            mark_file_change_enrichment_failed(&tx, repo_id, pending, error.to_string().as_str())?;
            tx.commit()?;
            return Err(error);
        }
    };

    let tx = conn.transaction()?;
    let applied = apply_file_change_line_stats_updates(&tx, repo_id, &updates)?;
    tx.commit()?;
    Ok(applied)
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

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}
