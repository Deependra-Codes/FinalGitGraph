use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rusqlite::{params, params_from_iter, types::Value};

use crate::adapters::db::{
    apply_file_change_line_stats_updates, clear_file_change_extraction_queue_for_commits,
    clear_repository_index_data, delete_file_changes_for_commits,
    enqueue_commits_for_file_change_extraction, enqueue_file_changes_for_enrichment,
    enqueue_file_changes_for_enrichment_from_drain_stage, load_commit_rowid, load_commit_window,
    load_file_change_extraction_queue_metrics, load_latest_planner_session,
    load_pending_file_change_enrichment, load_pending_file_change_enrichment_for_commits,
    load_pending_file_change_extraction_commits, load_recent_planner_sessions,
    load_repository_id_by_path, mark_file_change_enrichment_failed,
    mark_file_change_extraction_failed, merge_file_changes_from_drain_stage, open_database,
    prepare_file_change_drain_stage, run_migrations, set_repository_indexed_at,
    stage_file_change_for_drain, synchronize_refs, upsert_index_checkpoint, upsert_repository,
};
use crate::adapters::git::{
    configured_file_change_drain_channel_capacity,
    configured_file_change_drain_parallel_chunk_size, configured_file_change_parallel_worker_count,
    load_file_change_line_stats_for_targets, load_refs, stream_commits, stream_commits_with_hide,
    stream_file_changes_for_commits_drain, validate_repository, FILE_CHANGE_COMMIT_BATCH_SIZE,
};
use crate::application::graph_queries;
use crate::application::index_session_planner::{
    plan_index_session, plan_scan_session, resolve_repository_context,
};
use crate::application::indexer_pipeline::{
    dispatch_pending_commit_batch, drain_all_file_change_batches, drain_ready_file_change_batches,
    resolve_file_change_pipeline_config, FileChangeExtractionPipeline, FileChangeFlushStats,
};
use crate::application::ingest_store::{
    ensure_lsm_materializer_started, lsm_shadow_metrics_snapshot, reset_lsm_shadow_metrics,
    should_inline_enqueue_file_change_enrichment, IngestStore, SqliteIngestStore,
};
use crate::application::limits::normalize_window_limit;
use crate::application::query_graph_cache::{
    invalidate_query_graph_cache, load_or_hydrate_query_graph,
};
use crate::domain::models::{
    CheckpointRepositoryState, CommitWindowCursor, CommitWindowResponse,
    FileChangeExtractionDrainBatchStats, FileChangeExtractionDrainPhaseTimingMs,
    FileChangeExtractionDrainStats, GraphDirection, GraphTraversalResponse, GraphTraversalStrategy,
    IndexCheckpoint, IndexingMode, IndexingStats, IngestionGeometry, IngestionPhaseTimingMs,
    ParsedCommit, ParsedFileChange, PendingFileChangeEnrichment, PlannerSessionRecord,
    RepositoryCapabilities, RepositoryIdentity, RepositorySummary, ScanRepositoryResponse,
    ShortestPathResponse,
};
use crate::support::clock::now_epoch_seconds;
use crate::support::error::{AppError, AppResult};

const INCREMENTAL_LOOKBACK_SECONDS: i64 = 300;
const FILE_CHANGE_DB_BATCH_SIZE: usize = 4_096;
const FILE_CHANGE_JIT_ENRICH_BATCH_SIZE: usize = 1_024;
const FILE_CHANGE_COMMIT_BATCH_SIZE_ENV: &str = "GITGRAPH_FILE_CHANGE_COMMIT_BATCH_SIZE";
const FILE_CHANGE_EXTRACTION_QUEUE_BATCH_SIZE_FAST_DEFAULT: usize = 512;
const FILE_CHANGE_EXTRACTION_QUEUE_BATCH_SIZE_FAST_ENV: &str =
    "GITGRAPH_FILE_CHANGE_PIPELINE_COMMIT_BATCH_FULL";
const FILE_CHANGE_EXTRACTION_QUEUE_BATCH_SIZE_DRAIN_DEFAULT: usize = 1_000;
const FILE_CHANGE_EXTRACTION_QUEUE_BATCH_SIZE_DRAIN_ENV: &str =
    "GITGRAPH_FILE_CHANGE_PIPELINE_COMMIT_BATCH_DRAIN";
const INDEX_FAST_MODE_ENV: &str = "GITGRAPH_INDEX_FAST_MODE";
const SQLITE_MAX_BIND_PARAMETERS_FALLBACK: usize = 999;
const PENDING_EDGE_STAGE_COLUMNS: usize = 2;

#[derive(Default)]
struct IngestionPhaseTiming {
    commit_walk: Duration,
    commit_upsert: Duration,
    edge_materialize: Duration,
    file_diff_extract: Duration,
    file_diff_build: Duration,
    file_diff_rename_find: Duration,
    file_diff_delta_emit: Duration,
    extraction_dispatch_wait: Duration,
    extraction_worker_busy: Duration,
    extraction_reorder_buffer_max_depth: usize,
    extraction_worker_errors: usize,
    extraction_worker_aborts: usize,
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
            file_diff_build: duration_ms(self.file_diff_build),
            file_diff_rename_find: duration_ms(self.file_diff_rename_find),
            file_diff_delta_emit: duration_ms(self.file_diff_delta_emit),
            extraction_dispatch_wait: duration_ms(self.extraction_dispatch_wait),
            extraction_worker_busy: duration_ms(self.extraction_worker_busy),
            extraction_reorder_buffer_max_depth: self.extraction_reorder_buffer_max_depth as f64,
            extraction_worker_errors: self.extraction_worker_errors as f64,
            extraction_worker_aborts: self.extraction_worker_aborts as f64,
            file_change_delete: duration_ms(self.file_change_delete),
            file_change_upsert: duration_ms(self.file_change_upsert),
            tx_commit: duration_ms(self.tx_commit),
            lsm_shadow_write_attempts: 0.0,
            lsm_shadow_write_success: 0.0,
            lsm_shadow_write_errors: 0.0,
            lsm_shadow_commit_batches: 0.0,
            lsm_shadow_file_change_batches: 0.0,
            lsm_shadow_disabled_batches: 0.0,
            lsm_shadow_bytes_written: 0.0,
            lsm_shadow_write_ms: 0.0,
            lsm_materialization_batches: 0.0,
            lsm_materialization_rows: 0.0,
            lsm_materialization_errors: 0.0,
            lsm_materialization_ms: 0.0,
            lsm_materialization_queue_fetch_ms: 0.0,
            lsm_materialization_decode_ms: 0.0,
            lsm_materialization_group_ms: 0.0,
            lsm_materialization_sqlite_write_ms: 0.0,
            lsm_materialization_queue_cleanup_ms: 0.0,
            lsm_materialization_checkpoint_update_ms: 0.0,
            lsm_materialization_empty_check_ms: 0.0,
            lsm_materialization_keys_scanned: 0.0,
            lsm_materialization_rows_decoded: 0.0,
            lsm_materialization_invalid_rows: 0.0,
            lsm_materialization_rows_applied: 0.0,
            lsm_materialization_rows_unresolved: 0.0,
            lsm_materialization_commit_edges_immediate: 0.0,
            lsm_materialization_commit_edges_resolved: 0.0,
            lsm_materialization_commit_edges_unresolved_staged: 0.0,
        }
    }
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

    pub fn scan_repository(&self, path: &Path) -> AppResult<ScanRepositoryResponse> {
        let mut conn = open_database(&self.db_path)?;
        run_migrations(&mut conn)?;
        let planned_session = plan_scan_session(&conn, path)?;
        let session_plan = planned_session.plan;
        let repository = RepositorySummary {
            path: session_plan.repository.path.clone(),
            git_dir: session_plan.repository.git_dir.clone(),
            name: session_plan.repository.name.clone(),
            is_valid: true,
        };

        Ok(ScanRepositoryResponse {
            repository,
            session_plan,
        })
    }

    pub fn index_repository(&self, path: &Path, mode: IndexingMode) -> AppResult<IndexingStats> {
        reset_lsm_shadow_metrics();
        let mut conn = open_database(&self.db_path)?;
        run_migrations(&mut conn)?;
        let planned_session = plan_index_session(&conn, path, mode)?;
        let repo_id = planned_session.repo_id;
        let canonical_repo = planned_session.canonical_repo;
        let session_plan = planned_session.plan.clone();
        let repository_identity = session_plan.repository.clone();
        let repository_capabilities = session_plan.capabilities.clone();
        let effective_mode = session_plan.effective_mode;
        let checkpoint_assessment = session_plan.checkpoint.clone();
        let since_epoch = checkpoint_assessment
            .resume_from_committer_time
            .map(|committer_time| committer_time.saturating_sub(INCREMENTAL_LOOKBACK_SECONDS));

        let refs = load_refs(&canonical_repo.root)?;
        let indexed_at_epoch = now_epoch_seconds();
        let fast_mode = index_fast_mode_enabled();

        let tx = conn.transaction()?;
        if effective_mode == IndexingMode::Full {
            clear_repository_index_data(&tx, repo_id)?;
        }

        let mut commits_inserted = 0usize;
        let mut edges_inserted = 0usize;
        let mut file_changes_indexed = 0usize;
        let mut ingestion_timing = IngestionPhaseTiming::default();
        let mut checkpoint_candidate: Option<(i64, String)> = None;
        let resolved_file_change_commit_batch_size = resolve_file_change_commit_batch_size();
        let pipeline_config = (!fast_mode).then(|| {
            resolve_file_change_pipeline_config(
                effective_mode,
                resolved_file_change_commit_batch_size,
            )
        });
        let file_change_commit_batch_size = pipeline_config
            .as_ref()
            .map(|config| config.commit_batch_size)
            .unwrap_or(resolved_file_change_commit_batch_size)
            .max(1);
        let fast_mode_commit_batch_size = resolve_fast_mode_commit_batch_size();
        let commit_upsert_buffer_size = if fast_mode {
            fast_mode_commit_batch_size
        } else {
            file_change_commit_batch_size
        };
        let mut pending_commits = Vec::with_capacity(commit_upsert_buffer_size);
        let mut pending_commit_shas = Vec::with_capacity(file_change_commit_batch_size);
        let mut pending_extraction_queue_shas = Vec::with_capacity(fast_mode_commit_batch_size);
        let mut pending_file_changes = Vec::with_capacity(FILE_CHANGE_DB_BATCH_SIZE);
        let mut file_change_pipeline = pipeline_config
            .as_ref()
            .map(|config| FileChangeExtractionPipeline::spawn(&canonical_repo.root, config));

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

            let edge_stage_batch_size = resolve_pending_edge_stage_batch_size(&tx);
            let mut pending_edges =
                Vec::with_capacity(edge_stage_batch_size.saturating_mul(2).max(1));

            let commit_walk_started = Instant::now();
            let mut commit_callback_elapsed = Duration::default();
            let hide_ancestor_sha = checkpoint_assessment
                .hide_ancestor_sha
                .as_deref()
                .filter(|_| effective_mode == IndexingMode::Incremental);
            let mut on_streamed_commit = |commit: ParsedCommit| -> AppResult<()> {
                let callback_started = Instant::now();

                for parent in &commit.parents {
                    pending_edges.push((commit.sha.clone(), parent.clone()));
                    if pending_edges.len() >= edge_stage_batch_size {
                        stage_pending_commit_edges(&tx, &mut pending_edges, edge_stage_batch_size)?;
                    }
                    edges_inserted += 1;
                }
                if fast_mode {
                    pending_extraction_queue_shas.push(commit.sha.clone());
                    if pending_extraction_queue_shas.len() >= fast_mode_commit_batch_size {
                        enqueue_commits_for_file_change_extraction(
                            &tx,
                            repo_id,
                            &pending_extraction_queue_shas,
                        )?;
                        pending_extraction_queue_shas.clear();
                    }
                } else {
                    if pending_commit_shas.len() >= file_change_commit_batch_size {
                        let pipeline = file_change_pipeline.as_mut().ok_or_else(|| {
                            AppError::Parse(
                                "file-change extraction pipeline missing in normal index mode"
                                    .to_string(),
                            )
                        })?;
                        let dispatch_wait =
                            dispatch_pending_commit_batch(pipeline, &mut pending_commit_shas)?;
                        ingestion_timing.extraction_dispatch_wait = ingestion_timing
                            .extraction_dispatch_wait
                            .saturating_add(dispatch_wait);
                        let flush_stats = drain_ready_file_change_batches(
                            &tx,
                            repo_id,
                            effective_mode,
                            pipeline,
                            &mut pending_file_changes,
                            FILE_CHANGE_DB_BATCH_SIZE,
                        )?;
                        accumulate_flush_stats(
                            &mut file_changes_indexed,
                            &mut ingestion_timing,
                            flush_stats,
                        );
                    }
                    pending_commit_shas.push(commit.sha.clone());
                }

                update_checkpoint_candidate(
                    &mut checkpoint_candidate,
                    commit.committer_time,
                    &commit.sha,
                );
                pending_commits.push(commit);
                if pending_commits.len() >= commit_upsert_buffer_size {
                    let commit_upsert_started = Instant::now();
                    commits_inserted = commits_inserted.saturating_add(persist_commits_batch(
                        &tx,
                        repo_id,
                        effective_mode,
                        &pending_commits,
                    )?);
                    pending_commits.clear();
                    ingestion_timing.commit_upsert += commit_upsert_started.elapsed();
                }
                commit_callback_elapsed += callback_started.elapsed();

                Ok(())
            };
            let stream_result = if let Some(hide_ancestor_sha) = hide_ancestor_sha {
                stream_commits_with_hide(
                    &canonical_repo.root,
                    since_epoch,
                    Some(hide_ancestor_sha),
                    &mut on_streamed_commit,
                )
            } else {
                stream_commits(&canonical_repo.root, since_epoch, &mut on_streamed_commit)
            };
            stream_result?;
            ingestion_timing.commit_walk += commit_walk_started
                .elapsed()
                .saturating_sub(commit_callback_elapsed);
            if !pending_commits.is_empty() {
                let commit_upsert_started = Instant::now();
                commits_inserted = commits_inserted.saturating_add(persist_commits_batch(
                    &tx,
                    repo_id,
                    effective_mode,
                    &pending_commits,
                )?);
                pending_commits.clear();
                ingestion_timing.commit_upsert += commit_upsert_started.elapsed();
            }
            if fast_mode && !pending_extraction_queue_shas.is_empty() {
                enqueue_commits_for_file_change_extraction(
                    &tx,
                    repo_id,
                    &pending_extraction_queue_shas,
                )?;
                pending_extraction_queue_shas.clear();
            }
            if !pending_edges.is_empty() {
                stage_pending_commit_edges(&tx, &mut pending_edges, edge_stage_batch_size)?;
            }

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

        if !fast_mode {
            let pipeline = file_change_pipeline.as_mut().ok_or_else(|| {
                AppError::Parse(
                    "file-change extraction pipeline missing in normal index mode".to_string(),
                )
            })?;
            let dispatch_wait = dispatch_pending_commit_batch(pipeline, &mut pending_commit_shas)?;
            ingestion_timing.extraction_dispatch_wait = ingestion_timing
                .extraction_dispatch_wait
                .saturating_add(dispatch_wait);
            pipeline.close_input();
            let flush_stats = drain_all_file_change_batches(
                &tx,
                repo_id,
                effective_mode,
                pipeline,
                &mut pending_file_changes,
                FILE_CHANGE_DB_BATCH_SIZE,
            )?;
            accumulate_flush_stats(
                &mut file_changes_indexed,
                &mut ingestion_timing,
                flush_stats,
            );
            pipeline.join_worker()?;
            let pipeline_metrics = pipeline.metrics();
            ingestion_timing.extraction_reorder_buffer_max_depth = ingestion_timing
                .extraction_reorder_buffer_max_depth
                .max(pipeline_metrics.reorder_buffer_max_depth);
            ingestion_timing.extraction_worker_errors = ingestion_timing
                .extraction_worker_errors
                .saturating_add(pipeline_metrics.worker_errors);
            ingestion_timing.extraction_worker_aborts = ingestion_timing
                .extraction_worker_aborts
                .saturating_add(pipeline_metrics.worker_aborts);
            if !pending_file_changes.is_empty() {
                let file_change_upsert_started = Instant::now();
                file_changes_indexed =
                    file_changes_indexed.saturating_add(persist_file_changes_batch(
                        &tx,
                        repo_id,
                        effective_mode,
                        &pending_file_changes,
                    )?);
                if should_inline_enqueue_file_change_enrichment() {
                    enqueue_file_changes_for_enrichment(&tx, repo_id, &pending_file_changes)?;
                }
                ingestion_timing.file_change_upsert += file_change_upsert_started.elapsed();
                pending_file_changes.clear();
            }
        }

        let refs_indexed = synchronize_refs(&tx, repo_id, &refs)?;
        set_repository_indexed_at(&tx, repo_id, indexed_at_epoch)?;

        if let Some(checkpoint) = build_checkpoint(
            checkpoint_candidate,
            indexed_at_epoch,
            &repository_identity,
            &repository_capabilities,
        ) {
            upsert_index_checkpoint(&tx, repo_id, &checkpoint)?;
        }

        let tx_commit_started = Instant::now();
        tx.commit()?;
        ingestion_timing.tx_commit += tx_commit_started.elapsed();
        ensure_lsm_materializer_started(&self.db_path);
        invalidate_query_graph_cache(&self.db_path);

        let shadow = lsm_shadow_metrics_snapshot();
        let mut ingestion_phase_timing_ms = ingestion_timing.into_ms();
        ingestion_phase_timing_ms.lsm_shadow_write_attempts = shadow.write_attempts as f64;
        ingestion_phase_timing_ms.lsm_shadow_write_success = shadow.write_success as f64;
        ingestion_phase_timing_ms.lsm_shadow_write_errors = shadow.write_errors as f64;
        ingestion_phase_timing_ms.lsm_shadow_commit_batches = shadow.commit_batches as f64;
        ingestion_phase_timing_ms.lsm_shadow_file_change_batches =
            shadow.file_change_batches as f64;
        ingestion_phase_timing_ms.lsm_shadow_disabled_batches = shadow.disabled_batches as f64;
        ingestion_phase_timing_ms.lsm_shadow_bytes_written = shadow.bytes_written as f64;
        ingestion_phase_timing_ms.lsm_shadow_write_ms = shadow.write_micros as f64 / 1_000.0;
        ingestion_phase_timing_ms.lsm_materialization_batches =
            shadow.materialization_batches as f64;
        ingestion_phase_timing_ms.lsm_materialization_rows = shadow.materialization_rows as f64;
        ingestion_phase_timing_ms.lsm_materialization_errors = shadow.materialization_errors as f64;
        ingestion_phase_timing_ms.lsm_materialization_ms =
            shadow.materialization_micros as f64 / 1_000.0;
        ingestion_phase_timing_ms.lsm_materialization_queue_fetch_ms =
            shadow.materialization_queue_fetch_micros as f64 / 1_000.0;
        ingestion_phase_timing_ms.lsm_materialization_decode_ms =
            shadow.materialization_decode_micros as f64 / 1_000.0;
        ingestion_phase_timing_ms.lsm_materialization_group_ms =
            shadow.materialization_group_micros as f64 / 1_000.0;
        ingestion_phase_timing_ms.lsm_materialization_sqlite_write_ms =
            shadow.materialization_sqlite_write_micros as f64 / 1_000.0;
        ingestion_phase_timing_ms.lsm_materialization_queue_cleanup_ms =
            shadow.materialization_queue_cleanup_micros as f64 / 1_000.0;
        ingestion_phase_timing_ms.lsm_materialization_checkpoint_update_ms =
            shadow.materialization_checkpoint_update_micros as f64 / 1_000.0;
        ingestion_phase_timing_ms.lsm_materialization_empty_check_ms =
            shadow.materialization_empty_check_micros as f64 / 1_000.0;
        ingestion_phase_timing_ms.lsm_materialization_keys_scanned =
            shadow.materialization_keys_scanned as f64;
        ingestion_phase_timing_ms.lsm_materialization_rows_decoded =
            shadow.materialization_rows_decoded as f64;
        ingestion_phase_timing_ms.lsm_materialization_invalid_rows =
            shadow.materialization_invalid_rows as f64;
        ingestion_phase_timing_ms.lsm_materialization_rows_applied =
            shadow.materialization_rows_applied as f64;
        ingestion_phase_timing_ms.lsm_materialization_rows_unresolved =
            shadow.materialization_rows_unresolved as f64;
        ingestion_phase_timing_ms.lsm_materialization_commit_edges_immediate =
            shadow.materialization_commit_edges_immediate as f64;
        ingestion_phase_timing_ms.lsm_materialization_commit_edges_resolved =
            shadow.materialization_commit_edges_resolved as f64;
        ingestion_phase_timing_ms.lsm_materialization_commit_edges_unresolved_staged =
            shadow.materialization_commit_edges_unresolved_staged as f64;
        let ingestion_geometry = build_ingestion_geometry(
            fast_mode,
            pipeline_config.as_ref(),
            fast_mode_commit_batch_size,
            resolve_drain_commit_batch_size(),
        );

        Ok(IndexingStats {
            repo_id,
            mode: effective_mode,
            commits_indexed: commits_inserted,
            edges_indexed: edges_inserted,
            file_changes_indexed,
            refs_indexed,
            indexed_at_epoch,
            ingestion_phase_timing_ms,
            ingestion_geometry,
            session_plan,
        })
    }

    pub fn latest_planner_session(&self, path: &Path) -> AppResult<Option<PlannerSessionRecord>> {
        let mut conn = open_database(&self.db_path)?;
        run_migrations(&mut conn)?;
        let Some(repo_id) = load_planner_repo_id(&conn, path)? else {
            return Ok(None);
        };
        load_latest_planner_session(&conn, repo_id)
    }

    pub fn recent_planner_sessions(
        &self,
        path: &Path,
        limit: Option<usize>,
    ) -> AppResult<Vec<PlannerSessionRecord>> {
        let mut conn = open_database(&self.db_path)?;
        run_migrations(&mut conn)?;
        let Some(repo_id) = load_planner_repo_id(&conn, path)? else {
            return Ok(Vec::new());
        };
        load_recent_planner_sessions(&conn, repo_id, limit.unwrap_or(10))
    }

    pub fn drain_file_change_extraction_queue(
        &self,
        path: &Path,
        max_commits: Option<usize>,
    ) -> AppResult<FileChangeExtractionDrainStats> {
        let canonical_repo = validate_repository(path)?;
        let mut conn = open_database(&self.db_path)?;
        run_migrations(&mut conn)?;

        let repo_path = canonical_repo.root.display().to_string();
        let git_dir = canonical_repo.git_dir.display().to_string();
        let repo_id = resolve_repository_id_for_read(
            &conn,
            repo_path.as_str(),
            canonical_repo.name.as_str(),
            git_dir.as_str(),
        )?;
        let mut remaining_budget = max_commits.unwrap_or(usize::MAX).max(1);
        let drain_batch_size = resolve_drain_commit_batch_size();
        let mut drained_commits = 0usize;
        let mut failed_commits = 0usize;
        let mut file_changes_indexed = 0usize;
        let mut batches = 0usize;
        let mut total_commits_queried = 0usize;

        let mut queue_fetch = Duration::default();
        let mut diff_extract = Duration::default();
        let mut stage_insert = Duration::default();
        let mut delete_old_rows = Duration::default();
        let mut file_change_merge_upsert = Duration::default();
        let mut enrichment_enqueue = Duration::default();
        let mut queue_clear = Duration::default();
        let mut tx_commit = Duration::default();

        loop {
            if remaining_budget == 0 {
                break;
            }
            let batch_limit = remaining_budget.clamp(1, drain_batch_size);
            let queue_fetch_started = Instant::now();
            let pending_commits =
                load_pending_file_change_extraction_commits(&conn, repo_id, batch_limit)?;
            queue_fetch = queue_fetch.saturating_add(queue_fetch_started.elapsed());
            if pending_commits.is_empty() {
                break;
            }
            batches = batches.saturating_add(1);
            total_commits_queried = total_commits_queried.saturating_add(pending_commits.len());

            let tx = conn.transaction()?;
            prepare_file_change_drain_stage(&tx)?;

            let mut batch_stage_insert = Duration::default();
            let diff_extract_started = Instant::now();
            let extraction_result = stream_file_changes_for_commits_drain(
                &canonical_repo.root,
                &pending_commits,
                |change| {
                    let stage_insert_started = Instant::now();
                    stage_file_change_for_drain(&tx, &change)?;
                    batch_stage_insert =
                        batch_stage_insert.saturating_add(stage_insert_started.elapsed());
                    Ok(())
                },
            );
            let batch_diff_extract_elapsed = diff_extract_started.elapsed();
            stage_insert = stage_insert.saturating_add(batch_stage_insert);
            diff_extract = diff_extract
                .saturating_add(batch_diff_extract_elapsed.saturating_sub(batch_stage_insert));

            match extraction_result {
                Ok(_) => {
                    let delete_started = Instant::now();
                    delete_file_changes_for_commits(&tx, repo_id, &pending_commits)?;
                    delete_old_rows = delete_old_rows.saturating_add(delete_started.elapsed());

                    let merge_started = Instant::now();
                    let merged = merge_file_changes_from_drain_stage(&tx, repo_id)?;
                    file_change_merge_upsert =
                        file_change_merge_upsert.saturating_add(merge_started.elapsed());
                    file_changes_indexed = file_changes_indexed.saturating_add(merged);

                    let enqueue_started = Instant::now();
                    let _ = enqueue_file_changes_for_enrichment_from_drain_stage(&tx, repo_id)?;
                    enrichment_enqueue =
                        enrichment_enqueue.saturating_add(enqueue_started.elapsed());

                    let queue_clear_started = Instant::now();
                    clear_file_change_extraction_queue_for_commits(&tx, repo_id, &pending_commits)?;
                    queue_clear = queue_clear.saturating_add(queue_clear_started.elapsed());

                    let tx_commit_started = Instant::now();
                    tx.commit()?;
                    tx_commit = tx_commit.saturating_add(tx_commit_started.elapsed());
                    drained_commits = drained_commits.saturating_add(pending_commits.len());
                }
                Err(error) => {
                    drop(tx);
                    let tx = conn.transaction()?;
                    mark_file_change_extraction_failed(
                        &tx,
                        repo_id,
                        &pending_commits,
                        error.to_string().as_str(),
                    )?;
                    tx.commit()?;
                    failed_commits = failed_commits.saturating_add(pending_commits.len());
                }
            }

            if max_commits.is_some() {
                remaining_budget = remaining_budget.saturating_sub(pending_commits.len());
            }
        }

        let queue = load_file_change_extraction_queue_metrics(&conn, repo_id)?;
        Ok(FileChangeExtractionDrainStats {
            drained_commits,
            failed_commits,
            file_changes_indexed,
            queue,
            phase_timing_ms: FileChangeExtractionDrainPhaseTimingMs {
                queue_fetch: duration_ms(queue_fetch),
                diff_extract: duration_ms(diff_extract),
                stage_insert: duration_ms(stage_insert),
                delete_old_rows: duration_ms(delete_old_rows),
                file_change_merge_upsert: duration_ms(file_change_merge_upsert),
                enrichment_enqueue: duration_ms(enrichment_enqueue),
                queue_clear: duration_ms(queue_clear),
                tx_commit: duration_ms(tx_commit),
            },
            batch_stats: FileChangeExtractionDrainBatchStats {
                batches,
                commits_per_batch_mean: if batches == 0 {
                    0.0
                } else {
                    total_commits_queried as f64 / batches as f64
                },
                file_changes_per_batch_mean: if batches == 0 {
                    0.0
                } else {
                    file_changes_indexed as f64 / batches as f64
                },
            },
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

        let repo_path = canonical_repo.root.display().to_string();
        let git_dir = canonical_repo.git_dir.display().to_string();
        let repo_id = resolve_repository_id_for_read(
            &conn,
            repo_path.as_str(),
            canonical_repo.name.as_str(),
            git_dir.as_str(),
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

        let repo_path = canonical_repo.root.display().to_string();
        let git_dir = canonical_repo.git_dir.display().to_string();
        let repo_id = resolve_repository_id_for_read(
            &conn,
            repo_path.as_str(),
            canonical_repo.name.as_str(),
            git_dir.as_str(),
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

        let repo_path = canonical_repo.root.display().to_string();
        let git_dir = canonical_repo.git_dir.display().to_string();
        let repo_id = resolve_repository_id_for_read(
            &conn,
            repo_path.as_str(),
            canonical_repo.name.as_str(),
            git_dir.as_str(),
        )?;

        let bounded_limit = normalize_window_limit(limit);
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

        let repo_path = canonical_repo.root.display().to_string();
        let git_dir = canonical_repo.git_dir.display().to_string();
        let repo_id = resolve_repository_id_for_read(
            &conn,
            repo_path.as_str(),
            canonical_repo.name.as_str(),
            git_dir.as_str(),
        )?;
        if query_runtime_cache_enabled() {
            let graph = load_or_hydrate_query_graph(&conn, &self.db_path, repo_id)?;
            return graph.traverse(start_sha, strategy, direction, max_nodes);
        }

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

        let repo_path = canonical_repo.root.display().to_string();
        let git_dir = canonical_repo.git_dir.display().to_string();
        let repo_id = resolve_repository_id_for_read(
            &conn,
            repo_path.as_str(),
            canonical_repo.name.as_str(),
            git_dir.as_str(),
        )?;
        if query_runtime_cache_enabled() {
            let graph = load_or_hydrate_query_graph(&conn, &self.db_path, repo_id)?;
            return graph.shortest_path(from_sha, to_sha, direction);
        }

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

fn parse_env_bool_enabled(key: &str) -> bool {
    std::env::var(key)
        .map(|raw| {
            let normalized = raw.trim().to_ascii_lowercase();
            matches!(normalized.as_str(), "1" | "true" | "on" | "yes")
        })
        .unwrap_or(false)
}

fn parse_positive_env_usize(key: &str) -> Option<usize> {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
}

fn index_fast_mode_enabled() -> bool {
    parse_env_bool_enabled(INDEX_FAST_MODE_ENV)
}

fn resolve_file_change_commit_batch_size() -> usize {
    parse_positive_env_usize(FILE_CHANGE_COMMIT_BATCH_SIZE_ENV)
        .unwrap_or(FILE_CHANGE_COMMIT_BATCH_SIZE)
        .max(1)
}

fn resolve_fast_mode_commit_batch_size() -> usize {
    parse_positive_env_usize(FILE_CHANGE_EXTRACTION_QUEUE_BATCH_SIZE_FAST_ENV)
        .unwrap_or(FILE_CHANGE_EXTRACTION_QUEUE_BATCH_SIZE_FAST_DEFAULT)
        .max(1)
}

fn resolve_drain_commit_batch_size() -> usize {
    parse_positive_env_usize(FILE_CHANGE_EXTRACTION_QUEUE_BATCH_SIZE_DRAIN_ENV)
        .unwrap_or(FILE_CHANGE_EXTRACTION_QUEUE_BATCH_SIZE_DRAIN_DEFAULT)
        .max(1)
}

fn build_ingestion_geometry(
    fast_mode: bool,
    pipeline_config: Option<&crate::application::indexer_pipeline::FileChangePipelineConfig>,
    fast_mode_commit_batch_size: usize,
    drain_commit_batch_size: usize,
) -> IngestionGeometry {
    IngestionGeometry {
        file_change_pipeline_active: !fast_mode,
        file_change_pipeline_workers: pipeline_config
            .map(|config| config.worker_count)
            .unwrap_or(0),
        file_change_pipeline_commit_batch_size: pipeline_config
            .map(|config| config.commit_batch_size)
            .unwrap_or(0),
        file_change_pipeline_result_channel_capacity: pipeline_config
            .map(|config| config.result_channel_capacity)
            .unwrap_or(0),
        file_change_fast_commit_batch_size: fast_mode_commit_batch_size,
        file_change_drain_commit_batch_size: drain_commit_batch_size,
        file_change_db_batch_size: FILE_CHANGE_DB_BATCH_SIZE,
        file_change_diff_worker_threads: configured_file_change_parallel_worker_count(),
        file_change_drain_parallel_chunk_size: configured_file_change_drain_parallel_chunk_size(),
        file_change_drain_channel_capacity: configured_file_change_drain_channel_capacity(),
    }
}

fn query_runtime_cache_enabled() -> bool {
    std::env::var("GITGRAPH_QUERY_RUNTIME_CACHE")
        .map(|raw| {
            let normalized = raw.trim().to_ascii_lowercase();
            !matches!(normalized.as_str(), "0" | "false" | "off" | "no")
        })
        .unwrap_or(true)
}

fn resolve_repository_id_for_read(
    conn: &rusqlite::Connection,
    repo_path: &str,
    repo_name: &str,
    repo_git_dir: &str,
) -> AppResult<i64> {
    if let Some(repo_id) = load_repository_id_by_path(conn, repo_path)? {
        return Ok(repo_id);
    }

    upsert_repository(conn, repo_path, repo_name, repo_git_dir)
}

fn load_planner_repo_id(conn: &rusqlite::Connection, path: &Path) -> AppResult<Option<i64>> {
    let context = resolve_repository_context(path)?;
    load_repository_id_by_path(conn, context.identity.path.as_str())
}

fn persist_commits_batch(
    tx: &rusqlite::Connection,
    repo_id: i64,
    mode: IndexingMode,
    commits: &[ParsedCommit],
) -> AppResult<usize> {
    let store = SqliteIngestStore;
    store.persist_commits(tx, repo_id, mode, commits)
}

fn persist_file_changes_batch(
    tx: &rusqlite::Connection,
    repo_id: i64,
    mode: IndexingMode,
    file_changes: &[ParsedFileChange],
) -> AppResult<usize> {
    let store = SqliteIngestStore;
    store.persist_file_changes(tx, repo_id, mode, file_changes)
}

fn accumulate_flush_stats(
    file_changes_indexed: &mut usize,
    ingestion_timing: &mut IngestionPhaseTiming,
    stats: FileChangeFlushStats,
) {
    *file_changes_indexed = file_changes_indexed.saturating_add(stats.file_changes_indexed);
    ingestion_timing.file_diff_extract = ingestion_timing
        .file_diff_extract
        .saturating_add(stats.file_diff_extract);
    ingestion_timing.file_diff_build = ingestion_timing
        .file_diff_build
        .saturating_add(stats.file_diff_build);
    ingestion_timing.file_diff_rename_find = ingestion_timing
        .file_diff_rename_find
        .saturating_add(stats.file_diff_rename_find);
    ingestion_timing.file_diff_delta_emit = ingestion_timing
        .file_diff_delta_emit
        .saturating_add(stats.file_diff_delta_emit);
    ingestion_timing.extraction_worker_busy = ingestion_timing
        .extraction_worker_busy
        .saturating_add(stats.extraction_worker_busy);
    ingestion_timing.file_change_delete = ingestion_timing
        .file_change_delete
        .saturating_add(stats.file_change_delete);
    ingestion_timing.file_change_upsert = ingestion_timing
        .file_change_upsert
        .saturating_add(stats.file_change_upsert);
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
    repository_identity: &RepositoryIdentity,
    repository_capabilities: &RepositoryCapabilities,
) -> Option<IndexCheckpoint> {
    checkpoint_candidate.map(|(last_committer_time, last_commit_sha)| IndexCheckpoint {
        last_committer_time,
        last_commit_sha,
        indexed_at_epoch,
        repository_state: Some(CheckpointRepositoryState {
            identity_fingerprint: repository_identity.identity_fingerprint.clone(),
            topology: repository_identity.topology,
            is_bare: repository_identity.is_bare,
            is_shallow: repository_identity.is_shallow,
            object_hash: repository_capabilities.object_hash.clone(),
            ref_storage: repository_capabilities.ref_storage,
            has_sparse_checkout: repository_capabilities.has_sparse_checkout,
            has_partial_clone: repository_capabilities.has_partial_clone,
        }),
    })
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn resolve_pending_edge_stage_batch_size(conn: &rusqlite::Connection) -> usize {
    let max_bind_parameters = conn
        .query_row("PRAGMA max_variable_number", [], |row| row.get::<_, i64>(0))
        .ok()
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value > 0)
        .unwrap_or(SQLITE_MAX_BIND_PARAMETERS_FALLBACK);

    max_bind_parameters
        .checked_div(PENDING_EDGE_STAGE_COLUMNS)
        .unwrap_or(1)
        .max(1)
}

fn stage_pending_commit_edges(
    tx: &rusqlite::Connection,
    pending_edges: &mut Vec<(String, String)>,
    batch_size: usize,
) -> AppResult<()> {
    if pending_edges.is_empty() {
        return Ok(());
    }

    let effective_batch_size = batch_size.max(1);
    let mut chunks = pending_edges.chunks_exact(effective_batch_size);
    if pending_edges.len() >= effective_batch_size {
        let full_sql = build_pending_edge_stage_insert_sql(effective_batch_size);
        let mut full_stmt = tx.prepare_cached(&full_sql)?;
        for chunk in &mut chunks {
            let mut bind_values = Vec::with_capacity(chunk.len() * PENDING_EDGE_STAGE_COLUMNS);
            for (child_sha, parent_sha) in chunk {
                bind_values.push(Value::Text(child_sha.clone()));
                bind_values.push(Value::Text(parent_sha.clone()));
            }
            full_stmt.execute(params_from_iter(bind_values.iter()))?;
        }
    }

    let remainder = chunks.remainder();
    if !remainder.is_empty() {
        let remainder_sql = build_pending_edge_stage_insert_sql(remainder.len());
        let mut remainder_stmt = tx.prepare_cached(&remainder_sql)?;
        let mut bind_values = Vec::with_capacity(remainder.len() * PENDING_EDGE_STAGE_COLUMNS);
        for (child_sha, parent_sha) in remainder {
            bind_values.push(Value::Text(child_sha.clone()));
            bind_values.push(Value::Text(parent_sha.clone()));
        }
        remainder_stmt.execute(params_from_iter(bind_values.iter()))?;
    }

    pending_edges.clear();
    Ok(())
}

fn build_pending_edge_stage_insert_sql(row_count: usize) -> String {
    let bounded_row_count = row_count.max(1);
    let mut sql = String::with_capacity(96 + bounded_row_count * 8);
    sql.push_str("INSERT INTO current_pending_commit_edges (child_sha, parent_sha) VALUES ");
    for index in 0..bounded_row_count {
        if index > 0 {
            sql.push_str(", ");
        }
        sql.push_str("(?, ?)");
    }
    sql
}
