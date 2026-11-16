use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::bounded;

use crate::adapters::db::{
    open_store, should_compact_graph_overlay, CsrSnapshot, GraphOverlayParentRow,
    GraphSnapshotMetadata, GraphSnapshotState, GraphStore,
};
use crate::adapters::git::{
    hydrate_commit_metadata_for_shas, level3_runtime_options,
    load_file_change_line_stats_for_targets, load_refs, reorder_commits_topologically,
    reorder_commits_topologically_with_parents, stream_commit_batches_with_hide, stream_commits,
    stream_file_changes_for_commits, stream_file_changes_for_commits_drain, validate_repository,
    IncrementalCutoff, FILE_CHANGE_COMMIT_BATCH_SIZE,
};
use crate::application::index_planner::{plan_index_session, plan_scan_session};
use crate::application::limits::{normalize_graph_max_nodes, normalize_window_limit};
use crate::domain::models::{
    CommitMetadataState, CommitThroughputTelemetry, CommitWindowCursor, CommitWindowResponse,
    FileChangeExtractionDrainBatchStats, FileChangeExtractionDrainPhaseTimingMs,
    FileChangeExtractionDrainStats, FileChangeExtractionQueueMetrics, GraphDirection,
    GraphTraversalResponse, GraphTraversalStrategy, IndexCheckpoint, IndexingMode, IndexingStats,
    IngestionPhaseTimingMs, PendingFileChangeEnrichment, RepositorySummary, ShortestPathResponse,
};
use crate::support::clock::now_epoch_seconds;
use crate::support::error::{AppError, AppResult};

const INCREMENTAL_LOOKBACK_SECONDS: i64 = 300;
const FILE_CHANGE_DB_BATCH_SIZE: usize = 4_096;
const FILE_CHANGE_JIT_ENRICH_BATCH_SIZE: usize = 1_024;
const FILE_CHANGE_COMMIT_BATCH_SIZE_ENV: &str = "GITGRAPH_FILE_CHANGE_COMMIT_BATCH_SIZE";
const LEVEL3_COSCHEDULE_JIT_ENRICH_ENV: &str = "GITGRAPH_LEVEL3_COSCHEDULE_JIT_ENRICH";
const FILE_CHANGE_SYNC_STRICT_ENV: &str = "GITGRAPH_FILE_CHANGE_SYNC_STRICT";
const COMMIT_INGEST_PIPELINED_ENV: &str = "GITGRAPH_COMMIT_INGEST_PIPELINED";
const COMMIT_WRITE_WAVE_ADAPTIVE_ENV: &str = "GITGRAPH_COMMIT_WRITE_WAVE_ADAPTIVE";
const COMMIT_INGEST_QUEUE_CAPACITY_ENV: &str = "GITGRAPH_COMMIT_INGEST_QUEUE_CAPACITY";
const COMMIT_WINDOW_PREWARM_ENV: &str = "GITGRAPH_COMMIT_WINDOW_PREWARM";
const COMMIT_WINDOW_PREWARM_LIMIT_ENV: &str = "GITGRAPH_COMMIT_WINDOW_PREWARM_LIMIT";
const COMMIT_WRITE_WAVE_INITIAL_FULL: usize = 16_384;
const COMMIT_WRITE_WAVE_INITIAL_INCREMENTAL: usize = 8_192;
const COMMIT_WRITE_WAVE_MIN: usize = 2_048;
const COMMIT_WRITE_WAVE_MAX: usize = 32_768;
const COMMIT_INGEST_QUEUE_CAPACITY_DEFAULT: usize = 32_768;
const COMMIT_WINDOW_PREWARM_LIMIT_DEFAULT: usize = 1_024;

static RUNTIME_QUERY_GRAPH_CACHE: OnceLock<Mutex<Option<RuntimeQueryGraphCacheEntry>>> =
    OnceLock::new();

#[derive(Default)]
struct IngestionPhaseTiming {
    commit_walk: Duration,
    commit_walk_total: Duration,
    commit_callback: Duration,
    commit_walk_decode: Duration,
    commit_walk_parent_resolve: Duration,
    commit_walk_metadata_parse: Duration,
    commit_walk_tip_collection: Duration,
    commit_walk_hide_expansion: Duration,
    commit_walk_graph_lookup: Duration,
    commit_walk_ordering: Duration,
    commit_walk_emit: Duration,
    commit_walk_pack_inflate: Duration,
    commit_walk_pack_base_resolve: Duration,
    commit_walk_pack_parse: Duration,
    commit_walk_tape_build: Duration,
    commit_walk_graph_lookup_hit_count: usize,
    commit_walk_graph_lookup_miss_count: usize,
    commit_walk_graph_assist_hit_count: usize,
    commit_walk_graph_assist_miss_count: usize,
    commit_walk_nodes_seen_count: usize,
    commit_walk_nodes_pruned_count: usize,
    commit_walk_queue_max_depth: usize,
    commit_metadata_pending_count: usize,
    commit_walk_graph_missing_tip_count: usize,
    commit_walk_boundary_stop_count: usize,
    commit_walk_overlap_skip_count: usize,
    commit_walk_tape_node_count: usize,
    commit_walk_tape_parent_edges: usize,
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
    jit_enrich_coscheduled: Duration,
    commit_ingest_queue_wait: Duration,
    commit_ingest_queue_max_depth: usize,
    commit_write_wave_txn_count: usize,
    commit_write_wave_batch_median: f64,
    commit_csr_overlay_upsert: Duration,
    commit_csr_compaction: Duration,
    query_commit_window_prewarm_hydrated_rows: usize,
    tx_commit: Duration,
}

#[derive(Default)]
struct CommitWriteExecutionStats {
    commits_inserted: usize,
    commit_upsert: Duration,
    commit_write_wave_txn_count: usize,
    commit_write_wave_batch_sizes: Vec<usize>,
    commit_write_wave_batch_final: usize,
    queue_wait: Duration,
    queue_max_depth: usize,
}

struct CommitIngestWriterLoopOptions {
    repo_id: i64,
    queue_depth: Arc<AtomicUsize>,
    queue_max_depth: Arc<AtomicUsize>,
    initial_batch_size: usize,
    fixed_override: bool,
    adaptive: bool,
}

#[derive(Debug, Clone, Copy)]
struct CommitWriteWaveController {
    target_batch_size: usize,
    min_batch_size: usize,
    max_batch_size: usize,
    fixed_override: bool,
    adaptive: bool,
}

impl CommitWriteWaveController {
    fn new(initial_batch_size: usize, fixed_override: bool, adaptive: bool) -> Self {
        let clamped_initial =
            initial_batch_size.clamp(COMMIT_WRITE_WAVE_MIN, COMMIT_WRITE_WAVE_MAX);
        Self {
            target_batch_size: clamped_initial,
            min_batch_size: COMMIT_WRITE_WAVE_MIN,
            max_batch_size: COMMIT_WRITE_WAVE_MAX,
            fixed_override,
            adaptive,
        }
    }

    fn target_batch_size(&self) -> usize {
        self.target_batch_size
    }

    fn observe_flush(&mut self, commits_in_flush: usize, flush_duration: Duration) {
        if !self.adaptive || self.fixed_override || commits_in_flush == 0 {
            return;
        }
        let ms_per_commit = duration_ms(flush_duration) / commits_in_flush as f64;
        if ms_per_commit <= 0.12 {
            self.target_batch_size = (self.target_batch_size.saturating_mul(2))
                .clamp(self.min_batch_size, self.max_batch_size);
        } else if ms_per_commit >= 0.40 {
            self.target_batch_size = (self.target_batch_size / 2)
                .max(self.min_batch_size)
                .min(self.max_batch_size);
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PlannerTelemetryContext {
    session_id: u64,
    requested_mode: IndexingMode,
    effective_mode: IndexingMode,
    checkpoint_trusted: bool,
    risk_level: crate::domain::models::PlannerRiskLevel,
    degraded_policy: crate::domain::models::PlannerDegradedPolicy,
    trace_count: usize,
}

#[derive(Debug, Clone, Copy)]
struct CommitWalkTelemetryContext<'a> {
    scope: &'a str,
    engine: &'a str,
    sort_effective: &'a str,
    boundary_mode: &'a str,
    tip_count: usize,
    pruned_parent_expansions: usize,
    decode: Duration,
    parent_resolve: Duration,
    metadata_parse: Duration,
    tip_collection: Duration,
    hide_expansion: Duration,
    graph_lookup: Duration,
    ordering: Duration,
    emit: Duration,
    pack_inflate: Duration,
    pack_base_resolve: Duration,
    pack_parse: Duration,
    tape_build: Duration,
    graph_lookup_hit_count: usize,
    graph_lookup_miss_count: usize,
    graph_assist_hit_count: usize,
    graph_assist_miss_count: usize,
    nodes_seen_count: usize,
    nodes_pruned_count: usize,
    queue_max_depth: usize,
    metadata_pending_count: usize,
    graph_missing_tip_count: usize,
    boundary_stop_count: usize,
    overlap_skip_count: usize,
    tape_node_count: usize,
    tape_parent_edges: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct InMemoryAdjacencyQueryLatencyMs {
    pub bfs_parents: f64,
    pub shortest_path_parents: f64,
    pub shortest_path_both: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct InMemoryQueryBenchmarkStats {
    pub hydration: Duration,
    pub query_benchmark: Duration,
    pub adjacency_latency_ms: InMemoryAdjacencyQueryLatencyMs,
}

impl IngestionPhaseTiming {
    fn into_ms(self) -> IngestionPhaseTimingMs {
        IngestionPhaseTimingMs {
            commit_walk: duration_ms(self.commit_walk),
            commit_walk_total: duration_ms(self.commit_walk_total),
            commit_callback: duration_ms(self.commit_callback),
            commit_walk_decode: duration_ms(self.commit_walk_decode),
            commit_walk_parent_resolve: duration_ms(self.commit_walk_parent_resolve),
            commit_walk_metadata_parse: duration_ms(self.commit_walk_metadata_parse),
            commit_walk_tip_collection: duration_ms(self.commit_walk_tip_collection),
            commit_walk_hide_expansion: duration_ms(self.commit_walk_hide_expansion),
            commit_walk_graph_lookup: duration_ms(self.commit_walk_graph_lookup),
            commit_walk_ordering: duration_ms(self.commit_walk_ordering),
            commit_walk_emit: duration_ms(self.commit_walk_emit),
            commit_walk_pack_inflate: duration_ms(self.commit_walk_pack_inflate),
            commit_walk_pack_base_resolve: duration_ms(self.commit_walk_pack_base_resolve),
            commit_walk_pack_parse: duration_ms(self.commit_walk_pack_parse),
            commit_walk_tape_build: duration_ms(self.commit_walk_tape_build),
            commit_walk_graph_lookup_hit_count: self.commit_walk_graph_lookup_hit_count as f64,
            commit_walk_graph_lookup_miss_count: self.commit_walk_graph_lookup_miss_count as f64,
            commit_walk_graph_assist_hit_count: self.commit_walk_graph_assist_hit_count as f64,
            commit_walk_graph_assist_miss_count: self.commit_walk_graph_assist_miss_count as f64,
            commit_walk_nodes_seen_count: self.commit_walk_nodes_seen_count as f64,
            commit_walk_nodes_pruned_count: self.commit_walk_nodes_pruned_count as f64,
            commit_walk_queue_max_depth: self.commit_walk_queue_max_depth as f64,
            commit_metadata_pending_count: self.commit_metadata_pending_count as f64,
            commit_walk_graph_missing_tip_count: self.commit_walk_graph_missing_tip_count as f64,
            commit_walk_boundary_stop_count: self.commit_walk_boundary_stop_count as f64,
            commit_walk_overlap_skip_count: self.commit_walk_overlap_skip_count as f64,
            commit_walk_tape_node_count: self.commit_walk_tape_node_count as f64,
            commit_walk_tape_parent_edges: self.commit_walk_tape_parent_edges as f64,
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
            jit_enrich_coscheduled: duration_ms(self.jit_enrich_coscheduled),
            commit_ingest_queue_wait: duration_ms(self.commit_ingest_queue_wait),
            commit_ingest_queue_max_depth: self.commit_ingest_queue_max_depth as f64,
            commit_write_wave_txn_count: self.commit_write_wave_txn_count as f64,
            commit_write_wave_batch_median: self.commit_write_wave_batch_median,
            commit_csr_overlay_upsert: duration_ms(self.commit_csr_overlay_upsert),
            commit_csr_compaction: duration_ms(self.commit_csr_compaction),
            query_commit_window_prewarm_hydrated_rows: self
                .query_commit_window_prewarm_hydrated_rows
                as f64,
            tx_commit: duration_ms(self.tx_commit),
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
        PathBuf::from(".gitgraph-lab/gitgraph-lab.lmdb")
    }

    pub fn scan_repository(&self, path: &Path) -> AppResult<RepositorySummary> {
        let store = open_store(&self.db_path)?;
        let planned = plan_scan_session(&store, path)?;
        let canonical_repo = planned.canonical_repo;

        Ok(RepositorySummary {
            path: canonical_repo.root.display().to_string(),
            git_dir: canonical_repo.git_dir.display().to_string(),
            name: canonical_repo.name,
            is_valid: true,
        })
    }

    pub fn index_repository(&self, path: &Path, mode: IndexingMode) -> AppResult<IndexingStats> {
        let index_started = Instant::now();
        let store = open_store(&self.db_path)?;
        let planned = plan_index_session(&store, path, mode)?;
        let canonical_repo = planned.canonical_repo;
        let decision = planned.decision;
        let repo_id = decision.repo_id;
        let effective_mode = decision.effective_mode;
        let planner_session_id = decision.planner_session_id;
        let planner_requested_mode = decision.requested_mode;
        let planner_effective_mode = decision.effective_mode;
        let planner_checkpoint_trusted = decision.checkpoint_trusted;
        let planner_risk_level = decision.risk_level;
        let planner_degraded_policy = decision.degraded_policy;
        let planner_trace_count = decision.trace_count;
        let planner_telemetry = PlannerTelemetryContext {
            session_id: planner_session_id,
            requested_mode: planner_requested_mode,
            effective_mode: planner_effective_mode,
            checkpoint_trusted: planner_checkpoint_trusted,
            risk_level: planner_risk_level,
            degraded_policy: planner_degraded_policy,
            trace_count: planner_trace_count,
        };
        let since_epoch = decision
            .resume_from_committer_time
            .map(|value| value.saturating_sub(INCREMENTAL_LOOKBACK_SECONDS));
        let hide_ancestor_sha = decision.hide_ancestor_sha;
        let is_indexed = |sha: &str| store.commit_exists(repo_id, sha);
        let incremental_cutoff = since_epoch.map(|since_epoch| IncrementalCutoff {
            since_epoch,
            is_indexed: &is_indexed,
        });
        let level3_options = level3_runtime_options();
        let coschedule_jit_enrichment_enabled =
            parse_env_bool_enabled(LEVEL3_COSCHEDULE_JIT_ENRICH_ENV);
        let sync_file_change_extraction = parse_env_bool_enabled(FILE_CHANGE_SYNC_STRICT_ENV);
        let commit_window_prewarm_enabled = parse_env_bool_enabled(COMMIT_WINDOW_PREWARM_ENV);
        let commit_window_prewarm_limit = parse_positive_env_usize(COMMIT_WINDOW_PREWARM_LIMIT_ENV)
            .unwrap_or(COMMIT_WINDOW_PREWARM_LIMIT_DEFAULT)
            .max(1);
        let commit_batch_override = parse_positive_env_usize(FILE_CHANGE_COMMIT_BATCH_SIZE_ENV)
            .map(|value| value.clamp(COMMIT_WRITE_WAVE_MIN, COMMIT_WRITE_WAVE_MAX));
        let commit_batch_size =
            resolve_initial_commit_batch_size(effective_mode, commit_batch_override);
        let pipelined_commit_ingest =
            parse_env_bool_with_default(COMMIT_INGEST_PIPELINED_ENV, true);
        let adaptive_commit_waves =
            parse_env_bool_with_default(COMMIT_WRITE_WAVE_ADAPTIVE_ENV, true);
        let commit_ingest_queue_capacity = resolve_commit_ingest_queue_capacity(commit_batch_size);

        if effective_mode == IndexingMode::Full {
            store.clear_repository_index_data(repo_id)?;
        }

        let refs = load_refs(&canonical_repo.root)?;
        let indexed_at_epoch = now_epoch_seconds();
        let mut ingestion_timing = IngestionPhaseTiming::default();
        let mut checkpoint_candidate: Option<(i64, String)> = None;

        let mut edges_inserted = 0usize;
        let mut streamed_commit_shas = Vec::new();
        let mut streamed_commit_parents: HashMap<String, Vec<String>> = HashMap::new();
        let capture_streamed_commit_parents =
            sync_file_change_extraction && level3_options.topology_aware_extraction;
        let mut commit_callback_duration = Duration::ZERO;
        let mut commit_queue_wait_duration = Duration::ZERO;
        let mut commit_writer_stats = CommitWriteExecutionStats::default();

        let commit_walk_started = Instant::now();
        let commit_walk_stats = if pipelined_commit_ingest {
            let (commit_sender, commit_receiver) =
                bounded::<Vec<crate::domain::models::ParsedCommit>>(
                    resolve_commit_ingest_batch_queue_capacity(
                        commit_ingest_queue_capacity,
                        commit_batch_size,
                    ),
                );
            let queue_depth = Arc::new(AtomicUsize::new(0));
            let queue_max_depth = Arc::new(AtomicUsize::new(0));
            let queue_depth_for_writer = Arc::clone(&queue_depth);
            let queue_max_depth_for_writer = Arc::clone(&queue_max_depth);
            let writer_options = CommitIngestWriterLoopOptions {
                repo_id,
                queue_depth: queue_depth_for_writer,
                queue_max_depth: queue_max_depth_for_writer,
                initial_batch_size: commit_batch_size,
                fixed_override: commit_batch_override.is_some(),
                adaptive: adaptive_commit_waves,
            };
            let store_for_writer = &store;
            let (commit_walk_stats, writer_stats) = thread::scope(|scope| -> AppResult<_> {
                let writer_handle = scope.spawn(move || -> AppResult<CommitWriteExecutionStats> {
                    run_commit_ingest_writer_loop(store_for_writer, commit_receiver, writer_options)
                });

                let mut on_streamed_commit_batch =
                    |commit_batch: Vec<crate::domain::models::ParsedCommit>| -> AppResult<()> {
                        let callback_started = Instant::now();
                        let commit_count = commit_batch.len();
                        for commit in &commit_batch {
                            edges_inserted = edges_inserted.saturating_add(commit.parents.len());
                            update_checkpoint_candidate(
                                &mut checkpoint_candidate,
                                commit.committer_time,
                                &commit.sha,
                            );
                            if capture_streamed_commit_parents {
                                streamed_commit_parents
                                    .insert(commit.sha.clone(), commit.parents.clone());
                            }
                            streamed_commit_shas.push(commit.sha.clone());
                        }

                        let depth_after_enqueue = queue_depth
                            .fetch_add(commit_count, Ordering::Relaxed)
                            .saturating_add(commit_count);
                        update_max_atomic(&queue_max_depth, depth_after_enqueue);
                        let queue_wait_started = Instant::now();
                        if let Err(error) = commit_sender.send(commit_batch) {
                            let _ = queue_depth.fetch_sub(commit_count, Ordering::Relaxed);
                            return Err(AppError::Parse(format!(
                                "commit ingest queue send failed: {error}"
                            )));
                        }
                        let queue_wait = queue_wait_started.elapsed();
                        commit_queue_wait_duration =
                            commit_queue_wait_duration.saturating_add(queue_wait);

                        commit_callback_duration =
                            commit_callback_duration.saturating_add(callback_started.elapsed());
                        Ok(())
                    };

                let walk_result = if effective_mode == IndexingMode::Incremental {
                    let hide_ancestor_sha = hide_ancestor_sha.as_deref();
                    stream_commit_batches_with_hide(
                        &canonical_repo.root,
                        incremental_cutoff,
                        hide_ancestor_sha,
                        commit_batch_size,
                        &mut on_streamed_commit_batch,
                    )
                } else {
                    stream_commits(
                        &canonical_repo.root,
                        commit_batch_size,
                        &mut on_streamed_commit_batch,
                    )
                };

                drop(commit_sender);
                let writer_result = writer_handle.join().map_err(|_| {
                    AppError::Parse("commit ingest writer thread panicked".to_string())
                })?;
                let commit_walk_stats = walk_result?;
                let mut writer_stats = writer_result?;
                writer_stats.queue_wait = commit_queue_wait_duration;
                Ok((commit_walk_stats, writer_stats))
            })?;
            commit_writer_stats = writer_stats;
            commit_walk_stats
        } else {
            let mut commit_batch = Vec::with_capacity(commit_batch_size);
            let mut controller = CommitWriteWaveController::new(
                commit_batch_size,
                commit_batch_override.is_some(),
                adaptive_commit_waves,
            );

            let mut on_streamed_commit_batch =
                |streamed_batch: Vec<crate::domain::models::ParsedCommit>| -> AppResult<()> {
                    let callback_started = Instant::now();
                    for commit in streamed_batch {
                        edges_inserted = edges_inserted.saturating_add(commit.parents.len());
                        update_checkpoint_candidate(
                            &mut checkpoint_candidate,
                            commit.committer_time,
                            &commit.sha,
                        );
                        if capture_streamed_commit_parents {
                            streamed_commit_parents
                                .insert(commit.sha.clone(), commit.parents.clone());
                        }
                        streamed_commit_shas.push(commit.sha.clone());
                        commit_batch.push(commit);
                        if commit_batch.len() >= controller.target_batch_size() {
                            flush_commit_write_wave(
                                &store,
                                repo_id,
                                &mut commit_batch,
                                &mut controller,
                                &mut commit_writer_stats,
                            )?;
                        }
                    }
                    commit_callback_duration =
                        commit_callback_duration.saturating_add(callback_started.elapsed());
                    Ok(())
                };

            let commit_walk_stats = if effective_mode == IndexingMode::Incremental {
                let hide_ancestor_sha = hide_ancestor_sha.as_deref();
                stream_commit_batches_with_hide(
                    &canonical_repo.root,
                    incremental_cutoff,
                    hide_ancestor_sha,
                    commit_batch_size,
                    &mut on_streamed_commit_batch,
                )?
            } else {
                stream_commits(
                    &canonical_repo.root,
                    commit_batch_size,
                    &mut on_streamed_commit_batch,
                )?
            };

            if !commit_batch.is_empty() {
                flush_commit_write_wave(
                    &store,
                    repo_id,
                    &mut commit_batch,
                    &mut controller,
                    &mut commit_writer_stats,
                )?;
            }

            commit_walk_stats
        };

        ingestion_timing.commit_walk_total = commit_walk_started.elapsed();
        ingestion_timing.commit_callback = commit_callback_duration;
        ingestion_timing.commit_walk = ingestion_timing
            .commit_walk_total
            .saturating_sub(ingestion_timing.commit_callback);
        ingestion_timing.commit_walk_decode = commit_walk_stats.decode;
        ingestion_timing.commit_walk_parent_resolve = commit_walk_stats.parent_resolve;
        ingestion_timing.commit_walk_metadata_parse = commit_walk_stats.metadata_parse;
        ingestion_timing.commit_walk_tip_collection = commit_walk_stats.tip_collection;
        ingestion_timing.commit_walk_hide_expansion = commit_walk_stats.hide_expansion;
        ingestion_timing.commit_walk_graph_lookup = commit_walk_stats.graph_lookup;
        ingestion_timing.commit_walk_ordering = commit_walk_stats.ordering;
        ingestion_timing.commit_walk_emit = commit_walk_stats.emit;
        ingestion_timing.commit_walk_pack_inflate = commit_walk_stats.pack_inflate;
        ingestion_timing.commit_walk_pack_base_resolve = commit_walk_stats.pack_base_resolve;
        ingestion_timing.commit_walk_pack_parse = commit_walk_stats.pack_parse;
        ingestion_timing.commit_walk_tape_build = commit_walk_stats.tape_build;
        ingestion_timing.commit_walk_graph_lookup_hit_count =
            commit_walk_stats.graph_lookup_hit_count;
        ingestion_timing.commit_walk_graph_lookup_miss_count =
            commit_walk_stats.graph_lookup_miss_count;
        ingestion_timing.commit_walk_graph_assist_hit_count =
            commit_walk_stats.graph_assist_hit_count;
        ingestion_timing.commit_walk_graph_assist_miss_count =
            commit_walk_stats.graph_assist_miss_count;
        ingestion_timing.commit_walk_nodes_seen_count = commit_walk_stats.nodes_seen_count;
        ingestion_timing.commit_walk_nodes_pruned_count = commit_walk_stats.nodes_pruned_count;
        ingestion_timing.commit_walk_queue_max_depth = commit_walk_stats.queue_max_depth;
        ingestion_timing.commit_metadata_pending_count = commit_walk_stats.metadata_pending_count;
        ingestion_timing.commit_walk_graph_missing_tip_count =
            commit_walk_stats.graph_missing_tip_count;
        ingestion_timing.commit_walk_boundary_stop_count = commit_walk_stats.boundary_stop_count;
        ingestion_timing.commit_walk_overlap_skip_count = commit_walk_stats.overlap_skip_count;
        ingestion_timing.commit_walk_tape_node_count = commit_walk_stats.tape_node_count;
        ingestion_timing.commit_walk_tape_parent_edges = commit_walk_stats.tape_parent_edges;
        ingestion_timing.commit_upsert = ingestion_timing
            .commit_upsert
            .saturating_add(commit_writer_stats.commit_upsert);
        ingestion_timing.commit_ingest_queue_wait = ingestion_timing
            .commit_ingest_queue_wait
            .saturating_add(commit_writer_stats.queue_wait);
        ingestion_timing.commit_ingest_queue_max_depth = commit_writer_stats.queue_max_depth;
        ingestion_timing.commit_write_wave_txn_count =
            commit_writer_stats.commit_write_wave_txn_count;
        ingestion_timing.commit_write_wave_batch_median =
            median_usize(&commit_writer_stats.commit_write_wave_batch_sizes);
        let commits_inserted = commit_writer_stats.commits_inserted;

        let mut file_changes_indexed = 0usize;
        let mut file_change_commits_enqueued = 0usize;
        if sync_file_change_extraction {
            let mut pending_file_changes = Vec::with_capacity(FILE_CHANGE_DB_BATCH_SIZE);
            for commit_chunk in streamed_commit_shas.chunks(commit_batch_size.max(1)) {
                let extraction_chunk = if level3_options.topology_aware_extraction {
                    reorder_commits_topologically_with_parents(
                        commit_chunk,
                        &streamed_commit_parents,
                    )
                } else {
                    commit_chunk.to_vec()
                };
                if effective_mode == IndexingMode::Incremental {
                    let delete_started = Instant::now();
                    store.delete_file_changes_for_commits(repo_id, commit_chunk)?;
                    ingestion_timing.file_change_delete = ingestion_timing
                        .file_change_delete
                        .saturating_add(delete_started.elapsed());
                }

                let diff_extract_started = Instant::now();
                let telemetry = stream_file_changes_for_commits(
                    &canonical_repo.root,
                    &extraction_chunk,
                    |change| {
                        pending_file_changes.push(change);
                        Ok(())
                    },
                )?;
                ingestion_timing.file_diff_extract = ingestion_timing
                    .file_diff_extract
                    .saturating_add(diff_extract_started.elapsed());
                ingestion_timing.file_diff_build = ingestion_timing
                    .file_diff_build
                    .saturating_add(telemetry.diff_build);
                ingestion_timing.file_diff_rename_find = ingestion_timing
                    .file_diff_rename_find
                    .saturating_add(telemetry.rename_find);
                ingestion_timing.file_diff_delta_emit = ingestion_timing
                    .file_diff_delta_emit
                    .saturating_add(telemetry.delta_emit);

                if pending_file_changes.is_empty() {
                    continue;
                }

                let upsert_started = Instant::now();
                let persisted = store.persist_file_changes(repo_id, &pending_file_changes)?;
                file_changes_indexed = file_changes_indexed.saturating_add(persisted);
                ingestion_timing.file_change_upsert = ingestion_timing
                    .file_change_upsert
                    .saturating_add(upsert_started.elapsed());
                if coschedule_jit_enrichment_enabled {
                    let jit_started = Instant::now();
                    let pending = store.load_pending_file_change_enrichment_for_commits(
                        repo_id,
                        &extraction_chunk,
                        FILE_CHANGE_JIT_ENRICH_BATCH_SIZE,
                    )?;
                    let _ = enrich_file_change_stats_batch(
                        &canonical_repo.root,
                        &store,
                        repo_id,
                        &pending,
                    )?;
                    ingestion_timing.jit_enrich_coscheduled = ingestion_timing
                        .jit_enrich_coscheduled
                        .saturating_add(jit_started.elapsed());
                }
                pending_file_changes.clear();
            }
        } else if !streamed_commit_shas.is_empty() {
            let enqueue_started = Instant::now();
            file_change_commits_enqueued = if effective_mode == IndexingMode::Full {
                store.enqueue_commits_for_file_change_extraction_after_clear(
                    repo_id,
                    &streamed_commit_shas,
                )?
            } else {
                store.enqueue_commits_for_file_change_extraction(repo_id, &streamed_commit_shas)?
            };
            ingestion_timing.extraction_dispatch_wait = ingestion_timing
                .extraction_dispatch_wait
                .saturating_add(enqueue_started.elapsed());
        }

        let refs_indexed = store.synchronize_refs(repo_id, &refs)?;
        store.set_repository_indexed_at(repo_id, indexed_at_epoch)?;
        if let Some(checkpoint) = build_checkpoint(checkpoint_candidate, indexed_at_epoch) {
            store.upsert_index_checkpoint(repo_id, &checkpoint)?;
        }

        if effective_mode == IndexingMode::Incremental {
            if !streamed_commit_shas.is_empty() {
                let snapshot_started = Instant::now();
                let overlay_metadata = if store.load_graph_snapshot_metadata(repo_id)?.is_some() {
                    let overlay_started = Instant::now();
                    let metadata =
                        store.persist_graph_overlay_commits(repo_id, &streamed_commit_shas)?;
                    ingestion_timing.commit_csr_overlay_upsert = ingestion_timing
                        .commit_csr_overlay_upsert
                        .saturating_add(overlay_started.elapsed());
                    metadata
                } else {
                    let rebuild_started = Instant::now();
                    let _ = store.rebuild_csr_snapshot(repo_id)?;
                    ingestion_timing.commit_csr_compaction = ingestion_timing
                        .commit_csr_compaction
                        .saturating_add(rebuild_started.elapsed());
                    store
                        .load_graph_snapshot_metadata(repo_id)?
                        .ok_or_else(|| {
                            AppError::Parse(
                                "graph snapshot metadata missing after incremental fallback rebuild"
                                    .to_string(),
                            )
                        })?
                };

                // Compaction applies every overlay commit in one call, so no parent is missing yet.
                if should_compact_graph_overlay(&overlay_metadata) {
                    let compact_started = Instant::now();
                    let compacted = store.compact_graph_overlay_into_snapshot(repo_id)?;
                    if compacted.is_none() {
                        let _ = store.rebuild_csr_snapshot(repo_id)?;
                    }
                    ingestion_timing.commit_csr_compaction = ingestion_timing
                        .commit_csr_compaction
                        .saturating_add(compact_started.elapsed());
                }

                ingestion_timing.edge_materialize = ingestion_timing
                    .edge_materialize
                    .saturating_add(snapshot_started.elapsed());
            }
        } else {
            // One rebuild after the whole walk: per-wave snapshot deltas dropped edges whose
            // parent arrived in a later wave (ADR-0019).
            let snapshot_started = Instant::now();
            let _ = store.rebuild_csr_snapshot(repo_id)?;
            ingestion_timing.commit_csr_compaction = ingestion_timing
                .commit_csr_compaction
                .saturating_add(snapshot_started.elapsed());
            ingestion_timing.edge_materialize = ingestion_timing
                .edge_materialize
                .saturating_add(snapshot_started.elapsed());
        }

        let mut query_commit_window_prewarm_hydrated_rows = 0usize;
        if commit_window_prewarm_enabled && !streamed_commit_shas.is_empty() {
            let prewarm_shas: Vec<String> = streamed_commit_shas
                .iter()
                .take(commit_window_prewarm_limit)
                .cloned()
                .collect();
            let (hydrated, failed) =
                hydrate_commit_metadata_for_shas(&canonical_repo.root, prewarm_shas.as_slice())?;
            if !hydrated.is_empty() || !failed.is_empty() {
                let _ = store.persist_commit_metadata_updates(repo_id, &hydrated, &failed)?;
            }
            query_commit_window_prewarm_hydrated_rows = hydrated.len();
            ingestion_timing.query_commit_window_prewarm_hydrated_rows =
                query_commit_window_prewarm_hydrated_rows;
        }

        let commit_walk_telemetry = CommitWalkTelemetryContext {
            scope: commit_walk_stats.scope.as_str(),
            engine: commit_walk_stats.engine.as_str(),
            sort_effective: commit_walk_stats.sort_effective.as_str(),
            boundary_mode: commit_walk_stats.boundary_mode.as_str(),
            tip_count: commit_walk_stats.tip_count,
            pruned_parent_expansions: commit_walk_stats.pruned_parent_expansions,
            decode: commit_walk_stats.decode,
            parent_resolve: commit_walk_stats.parent_resolve,
            metadata_parse: commit_walk_stats.metadata_parse,
            tip_collection: commit_walk_stats.tip_collection,
            hide_expansion: commit_walk_stats.hide_expansion,
            graph_lookup: commit_walk_stats.graph_lookup,
            ordering: commit_walk_stats.ordering,
            emit: commit_walk_stats.emit,
            pack_inflate: commit_walk_stats.pack_inflate,
            pack_base_resolve: commit_walk_stats.pack_base_resolve,
            pack_parse: commit_walk_stats.pack_parse,
            tape_build: commit_walk_stats.tape_build,
            graph_lookup_hit_count: commit_walk_stats.graph_lookup_hit_count,
            graph_lookup_miss_count: commit_walk_stats.graph_lookup_miss_count,
            graph_assist_hit_count: commit_walk_stats.graph_assist_hit_count,
            graph_assist_miss_count: commit_walk_stats.graph_assist_miss_count,
            nodes_seen_count: commit_walk_stats.nodes_seen_count,
            nodes_pruned_count: commit_walk_stats.nodes_pruned_count,
            queue_max_depth: commit_walk_stats.queue_max_depth,
            metadata_pending_count: commit_walk_stats.metadata_pending_count,
            graph_missing_tip_count: commit_walk_stats.graph_missing_tip_count,
            boundary_stop_count: commit_walk_stats.boundary_stop_count,
            overlap_skip_count: commit_walk_stats.overlap_skip_count,
            tape_node_count: commit_walk_stats.tape_node_count,
            tape_parent_edges: commit_walk_stats.tape_parent_edges,
        };
        let commit_throughput = build_commit_throughput_telemetry(
            &planner_telemetry,
            &commit_walk_telemetry,
            streamed_commit_shas.len(),
            commits_inserted,
            index_started.elapsed(),
            &ingestion_timing,
        );
        let lmdb_runtime_contract = store.lmdb_runtime_contract();
        let queue_after_index = if effective_mode == IndexingMode::Full {
            FileChangeExtractionQueueMetrics {
                depth: file_change_commits_enqueued,
                oldest_queue_age_seconds: 0,
            }
        } else {
            store.load_file_change_extraction_queue_metrics(repo_id)?
        };

        Ok(IndexingStats {
            repo_id,
            mode: effective_mode,
            planner_session_id,
            planner_requested_mode,
            planner_effective_mode,
            planner_checkpoint_trusted,
            planner_risk_level,
            planner_degraded_policy,
            planner_trace_count,
            lmdb_effective_map_size_bytes: lmdb_runtime_contract.effective_map_size_bytes,
            lmdb_no_readahead: lmdb_runtime_contract.no_readahead,
            lmdb_write_map: lmdb_runtime_contract.write_map,
            lmdb_map_async: lmdb_runtime_contract.map_async,
            lmdb_no_mem_init: lmdb_runtime_contract.no_mem_init,
            commits_indexed: commits_inserted,
            edges_indexed: edges_inserted,
            file_changes_indexed,
            file_change_commits_enqueued,
            file_change_queue_depth_after_index: queue_after_index.depth,
            file_change_queue_oldest_age_seconds: queue_after_index.oldest_queue_age_seconds,
            refs_indexed,
            indexed_at_epoch,
            commit_walk_total_ms: duration_ms(ingestion_timing.commit_walk_total),
            commit_callback_ms: duration_ms(ingestion_timing.commit_callback),
            commit_walk_decode_ms: duration_ms(ingestion_timing.commit_walk_decode),
            commit_walk_parent_resolve_ms: duration_ms(ingestion_timing.commit_walk_parent_resolve),
            commit_walk_metadata_parse_ms: duration_ms(ingestion_timing.commit_walk_metadata_parse),
            commit_walk_tip_collection_ms: duration_ms(ingestion_timing.commit_walk_tip_collection),
            commit_walk_hide_expansion_ms: duration_ms(ingestion_timing.commit_walk_hide_expansion),
            commit_walk_graph_lookup_ms: duration_ms(ingestion_timing.commit_walk_graph_lookup),
            commit_walk_ordering_ms: duration_ms(ingestion_timing.commit_walk_ordering),
            commit_walk_emit_ms: duration_ms(ingestion_timing.commit_walk_emit),
            commit_walk_pack_inflate_ms: duration_ms(ingestion_timing.commit_walk_pack_inflate),
            commit_walk_pack_base_resolve_ms: duration_ms(
                ingestion_timing.commit_walk_pack_base_resolve,
            ),
            commit_walk_pack_parse_ms: duration_ms(ingestion_timing.commit_walk_pack_parse),
            commit_walk_tape_build_ms: duration_ms(ingestion_timing.commit_walk_tape_build),
            commit_walk_graph_lookup_hit_count: ingestion_timing.commit_walk_graph_lookup_hit_count,
            commit_walk_graph_lookup_miss_count: ingestion_timing
                .commit_walk_graph_lookup_miss_count,
            commit_walk_graph_assist_hit_count: ingestion_timing.commit_walk_graph_assist_hit_count,
            commit_walk_graph_assist_miss_count: ingestion_timing
                .commit_walk_graph_assist_miss_count,
            commit_walk_nodes_seen_count: ingestion_timing.commit_walk_nodes_seen_count,
            commit_walk_nodes_pruned_count: ingestion_timing.commit_walk_nodes_pruned_count,
            commit_walk_queue_max_depth: ingestion_timing.commit_walk_queue_max_depth,
            commit_metadata_pending_count: ingestion_timing.commit_metadata_pending_count,
            commit_walk_boundary_mode: commit_walk_telemetry.boundary_mode.to_string(),
            commit_walk_graph_missing_tip_count: ingestion_timing
                .commit_walk_graph_missing_tip_count,
            commit_walk_boundary_stop_count: ingestion_timing.commit_walk_boundary_stop_count,
            commit_walk_overlap_skip_count: ingestion_timing.commit_walk_overlap_skip_count,
            commit_walk_tape_node_count: ingestion_timing.commit_walk_tape_node_count,
            commit_walk_tape_parent_edges: ingestion_timing.commit_walk_tape_parent_edges,
            commit_window_jit_hydrated_rows: 0,
            query_commit_window_prewarm_hydrated_rows,
            commit_write_wave_txn_count: ingestion_timing.commit_write_wave_txn_count,
            commit_write_wave_batch_median: ingestion_timing.commit_write_wave_batch_median,
            commit_ingest_queue_max_depth: ingestion_timing.commit_ingest_queue_max_depth,
            commit_ingest_queue_wait_ms: duration_ms(ingestion_timing.commit_ingest_queue_wait),
            commit_csr_overlay_upsert_ms: duration_ms(ingestion_timing.commit_csr_overlay_upsert),
            commit_csr_compaction_ms: duration_ms(ingestion_timing.commit_csr_compaction),
            ingestion_phase_timing_ms: ingestion_timing.into_ms(),
            commit_throughput,
        })
    }

    pub fn drain_file_change_extraction_queue(
        &self,
        path: &Path,
        max_commits: Option<usize>,
    ) -> AppResult<FileChangeExtractionDrainStats> {
        let canonical_repo = validate_repository(path)?;
        let store = open_store(&self.db_path)?;
        let repo_id = resolve_repository_id_for_read(
            &store,
            &canonical_repo.root.display().to_string(),
            &canonical_repo.name,
            &canonical_repo.git_dir.display().to_string(),
        )?;
        let mut phase_timing = FileChangeExtractionDrainPhaseTimingMs::default();
        let mut batch_stats = FileChangeExtractionDrainBatchStats::default();
        let level3_options = level3_runtime_options();

        let queue_fetch_started = Instant::now();
        let _queue_before = store.load_file_change_extraction_queue_metrics(repo_id)?;
        phase_timing.queue_fetch = duration_ms(queue_fetch_started.elapsed());

        let mut remaining = max_commits
            .unwrap_or(FILE_CHANGE_JIT_ENRICH_BATCH_SIZE)
            .max(1);
        let extraction_commit_batch_size =
            parse_positive_env_usize(FILE_CHANGE_COMMIT_BATCH_SIZE_ENV)
                .unwrap_or(FILE_CHANGE_COMMIT_BATCH_SIZE)
                .clamp(1, COMMIT_WRITE_WAVE_MAX);
        let mut drained_commits_set: HashSet<String> = HashSet::new();
        let mut failed_commits = 0usize;
        let mut file_changes_indexed = 0usize;
        let mut total_file_changes_processed = 0usize;
        let mut batches = 0usize;

        while remaining > 0 {
            let batch_limit = remaining.clamp(1, extraction_commit_batch_size);
            let fetch_started = Instant::now();
            let pending_commits =
                store.load_pending_file_change_extraction_commits(repo_id, batch_limit)?;
            phase_timing.queue_fetch += duration_ms(fetch_started.elapsed());
            if pending_commits.is_empty() {
                break;
            }

            let extraction_chunk = if level3_options.topology_aware_extraction {
                reorder_commits_topologically(&canonical_repo.root, &pending_commits)
                    .unwrap_or_else(|_| pending_commits.clone())
            } else {
                pending_commits.clone()
            };
            let mut pending_file_changes = Vec::with_capacity(FILE_CHANGE_DB_BATCH_SIZE);

            let delete_started = Instant::now();
            store.delete_file_changes_for_commits(repo_id, &extraction_chunk)?;
            phase_timing.delete_old_rows += duration_ms(delete_started.elapsed());

            let diff_extract_started = Instant::now();
            let extraction = stream_file_changes_for_commits_drain(
                &canonical_repo.root,
                &extraction_chunk,
                |change| {
                    pending_file_changes.push(change);
                    Ok(())
                },
            );
            phase_timing.diff_extract += duration_ms(diff_extract_started.elapsed());

            let telemetry = match extraction {
                Ok(telemetry) => telemetry,
                Err(error) => {
                    let error_message = error.to_string();
                    let failure_started = Instant::now();
                    let _ = store.mark_file_change_extraction_commits_failed(
                        repo_id,
                        &pending_commits,
                        &error_message,
                    )?;
                    phase_timing.queue_clear += duration_ms(failure_started.elapsed());
                    failed_commits = failed_commits.saturating_add(pending_commits.len());
                    remaining = remaining.saturating_sub(pending_commits.len());
                    continue;
                }
            };
            phase_timing.stage_insert += duration_ms(telemetry.total());

            let merge_upsert_started = Instant::now();
            let persisted = store.persist_file_changes(repo_id, &pending_file_changes)?;
            let merge_upsert_elapsed = merge_upsert_started.elapsed();
            phase_timing.file_change_merge_upsert += duration_ms(merge_upsert_elapsed);
            phase_timing.enrichment_enqueue += duration_ms(merge_upsert_elapsed);

            let queue_clear_started = Instant::now();
            let _ = store.remove_file_change_extraction_commits(repo_id, &pending_commits)?;
            phase_timing.queue_clear += duration_ms(queue_clear_started.elapsed());

            for commit_sha in &pending_commits {
                drained_commits_set.insert(commit_sha.clone());
            }

            batches = batches.saturating_add(1);
            total_file_changes_processed =
                total_file_changes_processed.saturating_add(pending_file_changes.len());
            file_changes_indexed = file_changes_indexed.saturating_add(persisted);
            remaining = remaining.saturating_sub(pending_commits.len());
        }

        if batches > 0 {
            batch_stats.batches = batches;
            batch_stats.commits_per_batch_mean = drained_commits_set.len() as f64 / batches as f64;
            batch_stats.file_changes_per_batch_mean =
                total_file_changes_processed as f64 / batches as f64;
        }

        let queue_after_started = Instant::now();
        let queue = store.load_file_change_extraction_queue_metrics(repo_id)?;
        phase_timing.queue_clear += duration_ms(queue_after_started.elapsed());

        Ok(FileChangeExtractionDrainStats {
            drained_commits: drained_commits_set.len(),
            failed_commits,
            file_changes_indexed,
            queue,
            phase_timing_ms: phase_timing,
            batch_stats,
        })
    }

    pub fn enrich_pending_file_change_stats(
        &self,
        path: &Path,
        max_items: Option<usize>,
    ) -> AppResult<usize> {
        let canonical_repo = validate_repository(path)?;
        let store = open_store(&self.db_path)?;
        let repo_id = resolve_repository_id_for_read(
            &store,
            &canonical_repo.root.display().to_string(),
            &canonical_repo.name,
            &canonical_repo.git_dir.display().to_string(),
        )?;
        let batch_size = max_items
            .unwrap_or(FILE_CHANGE_JIT_ENRICH_BATCH_SIZE)
            .max(1);
        let pending = store.load_pending_file_change_enrichment(repo_id, batch_size)?;
        enrich_file_change_stats_batch(&canonical_repo.root, &store, repo_id, &pending)
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
        let store = open_store(&self.db_path)?;
        let repo_id = resolve_repository_id_for_read(
            &store,
            &canonical_repo.root.display().to_string(),
            &canonical_repo.name,
            &canonical_repo.git_dir.display().to_string(),
        )?;
        let batch_size = max_items
            .unwrap_or(FILE_CHANGE_JIT_ENRICH_BATCH_SIZE)
            .max(1);
        let pending = store.load_pending_file_change_enrichment_for_commits(
            repo_id,
            commit_shas,
            batch_size,
        )?;
        enrich_file_change_stats_batch(&canonical_repo.root, &store, repo_id, &pending)
    }

    pub fn get_commit_window(
        &self,
        path: &Path,
        limit: Option<usize>,
        cursor: Option<CommitWindowCursor>,
    ) -> AppResult<CommitWindowResponse> {
        let canonical_repo = validate_repository(path)?;
        let store = open_store(&self.db_path)?;

        let repo_id = resolve_repository_id_for_read(
            &store,
            &canonical_repo.root.display().to_string(),
            &canonical_repo.name,
            &canonical_repo.git_dir.display().to_string(),
        )?;

        let bounded_limit = normalize_window_limit(limit);
        let fetch_limit = bounded_limit.saturating_add(1);
        let mut commits = store.load_commit_window(repo_id, fetch_limit, cursor.as_ref())?;
        let pending_metadata_shas: Vec<String> = commits
            .iter()
            .filter(|commit| {
                matches!(
                    commit.metadata_state,
                    CommitMetadataState::Pending | CommitMetadataState::Failed
                )
            })
            .map(|commit| commit.sha.clone())
            .collect();
        let mut jit_hydrated_rows = 0usize;
        if !pending_metadata_shas.is_empty() {
            let (hydrated, failed) =
                hydrate_commit_metadata_for_shas(&canonical_repo.root, &pending_metadata_shas)?;
            if !hydrated.is_empty() || !failed.is_empty() {
                let _ = store.persist_commit_metadata_updates(repo_id, &hydrated, &failed)?;
                jit_hydrated_rows = hydrated.len();
                commits = store.load_commit_window(repo_id, fetch_limit, cursor.as_ref())?;
            }
        }

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
            jit_hydrated_rows,
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
        let store = open_store(&self.db_path)?;
        let repo_id = resolve_repository_id_for_read(
            &store,
            &canonical_repo.root.display().to_string(),
            &canonical_repo.name,
            &canonical_repo.git_dir.display().to_string(),
        )?;

        let query_graph = load_runtime_query_graph(&store, repo_id)?;
        query_graph.traverse(start_sha, strategy, direction, max_nodes)
    }

    pub fn shortest_path(
        &self,
        path: &Path,
        from_sha: &str,
        to_sha: &str,
        direction: GraphDirection,
    ) -> AppResult<ShortestPathResponse> {
        let canonical_repo = validate_repository(path)?;
        let store = open_store(&self.db_path)?;
        let repo_id = resolve_repository_id_for_read(
            &store,
            &canonical_repo.root.display().to_string(),
            &canonical_repo.name,
            &canonical_repo.git_dir.display().to_string(),
        )?;

        let query_graph = load_runtime_query_graph(&store, repo_id)?;
        query_graph.shortest_path(from_sha, to_sha, direction)
    }

    pub fn benchmark_in_memory_queries(
        &self,
        path: &Path,
        start_sha: &str,
        root_sha: &str,
        pre_merge_main_sha: &str,
        query_runs: usize,
        max_nodes: usize,
    ) -> AppResult<InMemoryQueryBenchmarkStats> {
        let canonical_repo = validate_repository(path)?;
        let store = open_store(&self.db_path)?;
        let repo_id = resolve_repository_id_for_read(
            &store,
            &canonical_repo.root.display().to_string(),
            &canonical_repo.name,
            &canonical_repo.git_dir.display().to_string(),
        )?;

        let hydration_started = Instant::now();
        let graph_state = store.load_graph_snapshot_state(repo_id)?.ok_or_else(|| {
            AppError::InvalidArgument("repository has no indexed CSR snapshot".to_string())
        })?;
        let query_graph = QueryGraph::from_graph_state(graph_state)?;
        let hydration = hydration_started.elapsed();

        let runs = query_runs.max(1);
        let mut bfs_parents_total = Duration::default();
        let mut shortest_path_parents_total = Duration::default();
        let mut shortest_path_both_total = Duration::default();
        let query_started = Instant::now();
        for _ in 0..runs {
            let bfs_started = Instant::now();
            let _ = query_graph.traverse(
                start_sha,
                GraphTraversalStrategy::Bfs,
                GraphDirection::Parents,
                max_nodes,
            )?;
            bfs_parents_total = bfs_parents_total.saturating_add(bfs_started.elapsed());

            let shortest_parents_started = Instant::now();
            let _ = query_graph.shortest_path(start_sha, root_sha, GraphDirection::Parents)?;
            shortest_path_parents_total =
                shortest_path_parents_total.saturating_add(shortest_parents_started.elapsed());

            let shortest_both_started = Instant::now();
            let _ =
                query_graph.shortest_path(start_sha, pre_merge_main_sha, GraphDirection::Both)?;
            shortest_path_both_total =
                shortest_path_both_total.saturating_add(shortest_both_started.elapsed());
        }

        Ok(InMemoryQueryBenchmarkStats {
            hydration,
            query_benchmark: query_started.elapsed(),
            adjacency_latency_ms: InMemoryAdjacencyQueryLatencyMs {
                bfs_parents: duration_ms(bfs_parents_total) / runs as f64,
                shortest_path_parents: duration_ms(shortest_path_parents_total) / runs as f64,
                shortest_path_both: duration_ms(shortest_path_both_total) / runs as f64,
            },
        })
    }
}

fn resolve_repository_id_for_read(
    store: &GraphStore,
    repo_path: &str,
    repo_name: &str,
    repo_git_dir: &str,
) -> AppResult<i64> {
    if let Some(repo_id) = store.load_repository_id_by_path(repo_path)? {
        return Ok(repo_id);
    }
    store.upsert_repository(repo_path, repo_name, repo_git_dir)
}

#[derive(Clone, PartialEq, Eq)]
struct RuntimeQueryGraphCacheKey {
    store_dir: PathBuf,
    repo_id: i64,
    snapshot_version: u64,
    generated_at_epoch: i64,
    node_count: usize,
    directed_edges: usize,
    pending_overlay_commits: usize,
    pending_overlay_edges: usize,
}

struct RuntimeQueryGraphCacheEntry {
    key: RuntimeQueryGraphCacheKey,
    graph: Arc<QueryGraph>,
}

fn runtime_query_graph_cache_key(
    store: &GraphStore,
    repo_id: i64,
    metadata: &GraphSnapshotMetadata,
) -> RuntimeQueryGraphCacheKey {
    RuntimeQueryGraphCacheKey {
        store_dir: store.store_directory().to_path_buf(),
        repo_id,
        snapshot_version: metadata.snapshot_version,
        generated_at_epoch: metadata.generated_at_epoch,
        node_count: metadata.node_count,
        directed_edges: metadata.directed_edges,
        pending_overlay_commits: metadata.pending_overlay_commits,
        pending_overlay_edges: metadata.pending_overlay_edges,
    }
}

fn cached_runtime_query_graph(key: &RuntimeQueryGraphCacheKey) -> Option<Arc<QueryGraph>> {
    let cache = RUNTIME_QUERY_GRAPH_CACHE.get_or_init(|| Mutex::new(None));
    let Ok(guard) = cache.lock() else {
        return None;
    };
    guard
        .as_ref()
        .filter(|entry| entry.key == *key)
        .map(|entry| Arc::clone(&entry.graph))
}

fn store_runtime_query_graph(key: RuntimeQueryGraphCacheKey, graph: Arc<QueryGraph>) {
    let cache = RUNTIME_QUERY_GRAPH_CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = cache.lock() {
        *guard = Some(RuntimeQueryGraphCacheEntry { key, graph });
    }
}

fn load_runtime_query_graph(store: &GraphStore, repo_id: i64) -> AppResult<Arc<QueryGraph>> {
    if let Some(metadata) = store.load_graph_snapshot_metadata(repo_id)? {
        let key = runtime_query_graph_cache_key(store, repo_id, &metadata);
        if let Some(graph) = cached_runtime_query_graph(&key) {
            return Ok(graph);
        }
    }

    let graph_state = store.load_graph_snapshot_state(repo_id)?.ok_or_else(|| {
        AppError::InvalidArgument("repository has no indexed CSR snapshot".to_string())
    })?;
    let key = runtime_query_graph_cache_key(store, repo_id, &graph_state.metadata);
    let graph = Arc::new(QueryGraph::from_graph_state(graph_state)?);
    store_runtime_query_graph(key, Arc::clone(&graph));
    Ok(graph)
}

fn enrich_file_change_stats_batch(
    repo_root: &Path,
    store: &GraphStore,
    repo_id: i64,
    pending: &[PendingFileChangeEnrichment],
) -> AppResult<usize> {
    if pending.is_empty() {
        return Ok(0);
    }

    let updates = match load_file_change_line_stats_for_targets(repo_root, pending) {
        Ok(updates) => updates,
        Err(error) => {
            store.mark_file_change_enrichment_failed(
                repo_id,
                pending,
                error.to_string().as_str(),
            )?;
            return Err(error);
        }
    };

    store.apply_file_change_line_stats_updates(repo_id, &updates)
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

fn parse_env_bool_enabled(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn parse_env_bool_with_default(key: &str, default_value: bool) -> bool {
    std::env::var(key)
        .ok()
        .and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
        .unwrap_or(default_value)
}

fn parse_positive_env_usize(key: &str) -> Option<usize> {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
}

fn resolve_initial_commit_batch_size(mode: IndexingMode, override_value: Option<usize>) -> usize {
    if let Some(value) = override_value {
        return value.clamp(COMMIT_WRITE_WAVE_MIN, COMMIT_WRITE_WAVE_MAX);
    }

    match mode {
        IndexingMode::Full => COMMIT_WRITE_WAVE_INITIAL_FULL,
        IndexingMode::Incremental => COMMIT_WRITE_WAVE_INITIAL_INCREMENTAL,
    }
}

fn resolve_commit_ingest_queue_capacity(batch_size: usize) -> usize {
    parse_positive_env_usize(COMMIT_INGEST_QUEUE_CAPACITY_ENV)
        .unwrap_or(COMMIT_INGEST_QUEUE_CAPACITY_DEFAULT)
        .max(1)
        .max(batch_size / 2)
}

fn resolve_commit_ingest_batch_queue_capacity(commit_capacity: usize, _batch_size: usize) -> usize {
    commit_capacity.max(1)
}

fn update_max_atomic(target: &AtomicUsize, candidate: usize) {
    let mut observed = target.load(Ordering::Relaxed);
    while candidate > observed {
        match target.compare_exchange(observed, candidate, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(next) => observed = next,
        }
    }
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn throughput_per_sec(commits: usize, duration: Duration) -> f64 {
    if commits == 0 {
        return 0.0;
    }
    let seconds = duration.as_secs_f64();
    if seconds <= 0.0 {
        return 0.0;
    }
    commits as f64 / seconds
}

fn ms_per_commit(commits: usize, duration: Duration) -> f64 {
    if commits == 0 {
        return 0.0;
    }
    duration_ms(duration) / commits as f64
}

fn median_usize(values: &[usize]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        sorted[middle] as f64
    } else {
        (sorted[middle - 1] as f64 + sorted[middle] as f64) / 2.0
    }
}

fn flush_commit_write_wave(
    store: &GraphStore,
    repo_id: i64,
    pending_commits: &mut Vec<crate::domain::models::ParsedCommit>,
    controller: &mut CommitWriteWaveController,
    stats: &mut CommitWriteExecutionStats,
) -> AppResult<()> {
    if pending_commits.is_empty() {
        return Ok(());
    }

    let upsert_started = Instant::now();
    let persisted = store.persist_commits(repo_id, pending_commits)?;
    let upsert_elapsed = upsert_started.elapsed();
    stats.commits_inserted = stats.commits_inserted.saturating_add(persisted);
    stats.commit_upsert = stats.commit_upsert.saturating_add(upsert_elapsed);

    stats.commit_write_wave_txn_count = stats.commit_write_wave_txn_count.saturating_add(1);
    stats
        .commit_write_wave_batch_sizes
        .push(pending_commits.len());
    controller.observe_flush(pending_commits.len(), upsert_elapsed);
    stats.commit_write_wave_batch_final = controller.target_batch_size();
    pending_commits.clear();
    Ok(())
}

fn run_commit_ingest_writer_loop(
    store: &GraphStore,
    commit_receiver: crossbeam_channel::Receiver<Vec<crate::domain::models::ParsedCommit>>,
    options: CommitIngestWriterLoopOptions,
) -> AppResult<CommitWriteExecutionStats> {
    let mut controller = CommitWriteWaveController::new(
        options.initial_batch_size,
        options.fixed_override,
        options.adaptive,
    );
    let mut pending_commits = Vec::with_capacity(controller.target_batch_size());
    let mut stats = CommitWriteExecutionStats {
        commit_write_wave_batch_final: controller.target_batch_size(),
        ..CommitWriteExecutionStats::default()
    };
    while let Ok(mut commits) = commit_receiver.recv() {
        options
            .queue_depth
            .fetch_sub(commits.len(), Ordering::Relaxed);
        pending_commits.append(&mut commits);
        if pending_commits.len() >= controller.target_batch_size() {
            flush_commit_write_wave(
                store,
                options.repo_id,
                &mut pending_commits,
                &mut controller,
                &mut stats,
            )?;
        }
    }
    if !pending_commits.is_empty() {
        flush_commit_write_wave(
            store,
            options.repo_id,
            &mut pending_commits,
            &mut controller,
            &mut stats,
        )?;
    }
    stats.queue_max_depth = options.queue_max_depth.load(Ordering::Relaxed);
    Ok(stats)
}

fn build_commit_throughput_telemetry(
    planner: &PlannerTelemetryContext,
    commit_walk: &CommitWalkTelemetryContext<'_>,
    commits_seen: usize,
    commits_indexed: usize,
    end_to_end: Duration,
    ingestion_timing: &IngestionPhaseTiming,
) -> CommitThroughputTelemetry {
    let tracked_phases: [(&str, Duration); 8] = [
        ("commit_walk", ingestion_timing.commit_walk),
        ("commit_upsert", ingestion_timing.commit_upsert),
        ("edge_materialize", ingestion_timing.edge_materialize),
        ("file_diff_extract", ingestion_timing.file_diff_extract),
        ("file_change_delete", ingestion_timing.file_change_delete),
        ("file_change_upsert", ingestion_timing.file_change_upsert),
        (
            "jit_enrich_coscheduled",
            ingestion_timing.jit_enrich_coscheduled,
        ),
        ("tx_commit", ingestion_timing.tx_commit),
    ];

    let mut tracked_total = Duration::ZERO;
    let mut bottleneck_phase = "none";
    let mut bottleneck_duration = Duration::ZERO;
    for (phase_name, phase_duration) in tracked_phases {
        tracked_total = tracked_total.saturating_add(phase_duration);
        if phase_duration > bottleneck_duration {
            bottleneck_phase = phase_name;
            bottleneck_duration = phase_duration;
        }
    }

    let bottleneck_share_of_tracked_time = if tracked_total.as_secs_f64() <= 0.0 {
        0.0
    } else {
        bottleneck_duration.as_secs_f64() / tracked_total.as_secs_f64()
    };

    CommitThroughputTelemetry {
        planner_session_id: planner.session_id,
        planner_requested_mode: planner.requested_mode,
        planner_effective_mode: planner.effective_mode,
        planner_checkpoint_trusted: planner.checkpoint_trusted,
        planner_risk_level: planner.risk_level,
        planner_degraded_policy: planner.degraded_policy,
        planner_trace_count: planner.trace_count,
        commits_seen,
        commits_indexed,
        end_to_end_ms: duration_ms(end_to_end),
        commits_per_sec_seen: throughput_per_sec(commits_seen, end_to_end),
        commits_per_sec_indexed: throughput_per_sec(commits_indexed, end_to_end),
        ms_per_commit_seen: ms_per_commit(commits_seen, end_to_end),
        ms_per_commit_indexed: ms_per_commit(commits_indexed, end_to_end),
        commit_walk_scope: commit_walk.scope.to_string(),
        commit_walk_engine: commit_walk.engine.to_string(),
        commit_walk_sort_effective: commit_walk.sort_effective.to_string(),
        commit_walk_boundary_mode: commit_walk.boundary_mode.to_string(),
        commit_walk_tip_count: commit_walk.tip_count,
        commit_walk_pruned_parent_expansions: commit_walk.pruned_parent_expansions,
        commit_walk_total_ms: duration_ms(ingestion_timing.commit_walk_total),
        commit_callback_ms: duration_ms(ingestion_timing.commit_callback),
        commit_walk_decode_ms: duration_ms(commit_walk.decode),
        commit_walk_parent_resolve_ms: duration_ms(commit_walk.parent_resolve),
        commit_walk_metadata_parse_ms: duration_ms(commit_walk.metadata_parse),
        commit_walk_tip_collection_ms: duration_ms(commit_walk.tip_collection),
        commit_walk_hide_expansion_ms: duration_ms(commit_walk.hide_expansion),
        commit_walk_graph_lookup_ms: duration_ms(commit_walk.graph_lookup),
        commit_walk_ordering_ms: duration_ms(commit_walk.ordering),
        commit_walk_emit_ms: duration_ms(commit_walk.emit),
        commit_walk_pack_inflate_ms: duration_ms(commit_walk.pack_inflate),
        commit_walk_pack_base_resolve_ms: duration_ms(commit_walk.pack_base_resolve),
        commit_walk_pack_parse_ms: duration_ms(commit_walk.pack_parse),
        commit_walk_tape_build_ms: duration_ms(commit_walk.tape_build),
        commit_walk_graph_lookup_hit_count: commit_walk.graph_lookup_hit_count,
        commit_walk_graph_lookup_miss_count: commit_walk.graph_lookup_miss_count,
        commit_walk_graph_assist_hit_count: commit_walk.graph_assist_hit_count,
        commit_walk_graph_assist_miss_count: commit_walk.graph_assist_miss_count,
        commit_walk_nodes_seen_count: commit_walk.nodes_seen_count,
        commit_walk_nodes_pruned_count: commit_walk.nodes_pruned_count,
        commit_walk_queue_max_depth: commit_walk.queue_max_depth,
        commit_metadata_pending_count: commit_walk.metadata_pending_count,
        commit_walk_graph_missing_tip_count: commit_walk.graph_missing_tip_count,
        commit_walk_boundary_stop_count: commit_walk.boundary_stop_count,
        commit_walk_overlap_skip_count: commit_walk.overlap_skip_count,
        commit_walk_tape_node_count: commit_walk.tape_node_count,
        commit_walk_tape_parent_edges: commit_walk.tape_parent_edges,
        commit_window_jit_hydrated_rows: 0,
        query_commit_window_prewarm_hydrated_rows: ingestion_timing
            .query_commit_window_prewarm_hydrated_rows,
        commit_walk_ms: duration_ms(ingestion_timing.commit_walk),
        commit_walk_commits_per_sec_seen: throughput_per_sec(
            commits_seen,
            ingestion_timing.commit_walk,
        ),
        commit_upsert_ms: duration_ms(ingestion_timing.commit_upsert),
        commit_upsert_commits_per_sec_seen: throughput_per_sec(
            commits_seen,
            ingestion_timing.commit_upsert,
        ),
        commit_write_wave_txn_count: ingestion_timing.commit_write_wave_txn_count,
        commit_write_wave_batch_median: ingestion_timing.commit_write_wave_batch_median,
        commit_ingest_queue_max_depth: ingestion_timing.commit_ingest_queue_max_depth,
        commit_ingest_queue_wait_ms: duration_ms(ingestion_timing.commit_ingest_queue_wait),
        commit_csr_overlay_upsert_ms: duration_ms(ingestion_timing.commit_csr_overlay_upsert),
        commit_csr_compaction_ms: duration_ms(ingestion_timing.commit_csr_compaction),
        file_diff_extract_ms: duration_ms(ingestion_timing.file_diff_extract),
        file_diff_extract_commits_per_sec_seen: throughput_per_sec(
            commits_seen,
            ingestion_timing.file_diff_extract,
        ),
        file_change_upsert_ms: duration_ms(ingestion_timing.file_change_upsert),
        file_change_upsert_commits_per_sec_seen: throughput_per_sec(
            commits_seen,
            ingestion_timing.file_change_upsert,
        ),
        tx_commit_ms: duration_ms(ingestion_timing.tx_commit),
        tx_commit_commits_per_sec_seen: throughput_per_sec(
            commits_seen,
            ingestion_timing.tx_commit,
        ),
        bottleneck_phase: bottleneck_phase.to_string(),
        bottleneck_phase_ms: duration_ms(bottleneck_duration),
        bottleneck_phase_share_of_tracked_time: bottleneck_share_of_tracked_time,
    }
}

type NodeId = usize;
const BFS_PULL_MIN_FRONTIER: usize = 256;
const BFS_PULL_MIN_REMAINING: usize = 1_024;
const BFS_PULL_FRONTIER_TO_REMAINING_RATIO: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BfsMode {
    Push,
    Pull,
}

#[derive(Default)]
struct DenseSet {
    words: Vec<u64>,
    count: usize,
}

impl DenseSet {
    fn insert(&mut self, id: NodeId) -> bool {
        let word_index = id / u64::BITS as usize;
        let bit_mask = 1u64 << (id % u64::BITS as usize);

        if word_index >= self.words.len() {
            self.words.resize(word_index + 1, 0);
        }

        if self.words[word_index] & bit_mask == 0 {
            self.words[word_index] |= bit_mask;
            self.count = self.count.saturating_add(1);
            true
        } else {
            false
        }
    }

    fn contains(&self, id: NodeId) -> bool {
        let word_index = id / u64::BITS as usize;
        let bit_mask = 1u64 << (id % u64::BITS as usize);
        self.words
            .get(word_index)
            .map(|word| *word & bit_mask != 0)
            .unwrap_or(false)
    }

    fn count(&self) -> usize {
        self.count
    }

    fn union_count(&self, other: &Self) -> usize {
        let max_len = self.words.len().max(other.words.len());
        let mut total = 0usize;
        for index in 0..max_len {
            let left = self.words.get(index).copied().unwrap_or_default();
            let right = other.words.get(index).copied().unwrap_or_default();
            total = total.saturating_add((left | right).count_ones() as usize);
        }
        total
    }
}

struct SearchSide {
    frontier: VecDeque<NodeId>,
    visited: DenseSet,
    predecessors: Vec<Option<NodeId>>,
}

impl SearchSide {
    fn new(start: NodeId) -> Self {
        let mut side = Self {
            frontier: VecDeque::from([start]),
            visited: DenseSet::default(),
            predecessors: Vec::new(),
        };
        side.visited.insert(start);
        ensure_slot(&mut side.predecessors, start);
        side
    }
}

struct QueryGraph {
    sha_to_id: HashMap<String, NodeId>,
    id_to_sha: Vec<String>,
    base_node_count: usize,
    parents_offsets: Vec<u32>,
    parents_neighbors: Vec<u32>,
    children_offsets: Vec<u32>,
    children_neighbors: Vec<u32>,
    overlay_parent_neighbors: Vec<Vec<u32>>,
    overlay_child_neighbors: HashMap<NodeId, Vec<u32>>,
}

impl QueryGraph {
    fn from_graph_state(state: GraphSnapshotState) -> AppResult<Self> {
        let GraphSnapshotState {
            snapshot,
            overlay_parent_rows,
            overlay_child_rows,
            ..
        } = state;
        let mut graph = Self::from_snapshot(snapshot)?;
        graph.attach_overlay(overlay_parent_rows, overlay_child_rows)?;
        Ok(graph)
    }

    fn from_snapshot(snapshot: CsrSnapshot) -> AppResult<Self> {
        if snapshot.parents_offsets.len() != snapshot.shas.len().saturating_add(1) {
            return Err(AppError::Parse(
                "csr snapshot parent offsets length does not match node count".to_string(),
            ));
        }
        if snapshot.children_offsets.len() != snapshot.shas.len().saturating_add(1) {
            return Err(AppError::Parse(
                "csr snapshot child offsets length does not match node count".to_string(),
            ));
        }

        let mut sha_to_id = HashMap::with_capacity(snapshot.shas.len());
        for (index, sha) in snapshot.shas.iter().enumerate() {
            sha_to_id.insert(sha.clone(), index);
        }

        Ok(Self {
            sha_to_id,
            base_node_count: snapshot.shas.len(),
            id_to_sha: snapshot.shas,
            parents_offsets: snapshot.parents_offsets,
            parents_neighbors: snapshot.parents_neighbors,
            children_offsets: snapshot.children_offsets,
            children_neighbors: snapshot.children_neighbors,
            overlay_parent_neighbors: Vec::new(),
            overlay_child_neighbors: HashMap::new(),
        })
    }

    fn attach_overlay(
        &mut self,
        mut overlay_parent_rows: Vec<GraphOverlayParentRow>,
        overlay_child_rows: HashMap<String, Vec<String>>,
    ) -> AppResult<()> {
        if overlay_parent_rows.is_empty() && overlay_child_rows.is_empty() {
            return Ok(());
        }

        overlay_parent_rows.sort_unstable_by(|left, right| {
            right
                .committer_time
                .cmp(&left.committer_time)
                .then_with(|| right.child_sha.cmp(&left.child_sha))
        });

        let mut unique_overlay = Vec::with_capacity(overlay_parent_rows.len());
        for row in overlay_parent_rows {
            if self.sha_to_id.contains_key(row.child_sha.as_str()) {
                continue;
            }
            let node_id = self.id_to_sha.len();
            self.sha_to_id.insert(row.child_sha.clone(), node_id);
            self.id_to_sha.push(row.child_sha.clone());
            unique_overlay.push(row);
        }

        if unique_overlay.is_empty() {
            return Ok(());
        }

        self.overlay_parent_neighbors
            .resize_with(unique_overlay.len(), Vec::new);

        for row in unique_overlay {
            let Some(&node_id) = self.sha_to_id.get(row.child_sha.as_str()) else {
                continue;
            };
            let overlay_index = node_id.saturating_sub(self.base_node_count);

            let mut parent_ids = Vec::with_capacity(row.parent_shas.len());
            for parent_sha in &row.parent_shas {
                let Some(&parent_id) = self.sha_to_id.get(parent_sha.as_str()) else {
                    continue;
                };
                let parent_id_u32 = u32::try_from(parent_id).map_err(|_| {
                    AppError::Parse(
                        "graph overlay parent index exceeded u32 capacity during query hydration"
                            .to_string(),
                    )
                })?;
                parent_ids.push(parent_id_u32);
            }
            parent_ids.sort_unstable_by(|left, right| right.cmp(left));
            parent_ids.dedup();

            self.overlay_parent_neighbors[overlay_index] = parent_ids.clone();
        }

        for (parent_sha, child_shas) in overlay_child_rows {
            let Some(&parent_index) = self.sha_to_id.get(parent_sha.as_str()) else {
                continue;
            };
            let mut child_ids = Vec::with_capacity(child_shas.len());
            for child_sha in child_shas {
                let Some(&child_index) = self.sha_to_id.get(child_sha.as_str()) else {
                    continue;
                };
                let child_id = u32::try_from(child_index).map_err(|_| {
                    AppError::Parse(
                        "graph overlay child index exceeded u32 capacity during query hydration"
                            .to_string(),
                    )
                })?;
                child_ids.push(child_id);
            }

            child_ids.sort_unstable_by(|left, right| right.cmp(left));
            child_ids.dedup();
            if !child_ids.is_empty() {
                self.overlay_child_neighbors.insert(parent_index, child_ids);
            }
        }

        Ok(())
    }

    fn node_count(&self) -> usize {
        self.id_to_sha.len()
    }

    fn traverse(
        &self,
        start_sha: &str,
        strategy: GraphTraversalStrategy,
        direction: GraphDirection,
        max_nodes: usize,
    ) -> AppResult<GraphTraversalResponse> {
        let bounded_max_nodes = normalize_graph_max_nodes(max_nodes);
        let Some(start_id) = self.sha_to_id.get(start_sha).copied() else {
            return Err(AppError::InvalidArgument(format!(
                "start commit not indexed for repository: {start_sha}"
            )));
        };

        let (visits, truncated) = match strategy {
            GraphTraversalStrategy::Bfs => {
                self.traverse_bfs(start_id, direction, bounded_max_nodes)?
            }
            GraphTraversalStrategy::Dfs => {
                self.traverse_dfs(start_id, direction, bounded_max_nodes)?
            }
            GraphTraversalStrategy::Topological => {
                if direction == GraphDirection::Both {
                    return Err(AppError::InvalidArgument(
                        "topological traversal requires direction=parents or direction=children"
                            .to_string(),
                    ));
                }
                self.traverse_topological(start_id, direction, bounded_max_nodes)?
            }
        };

        let nodes = visits
            .into_iter()
            .map(|(node_id, depth)| crate::domain::models::GraphNodeVisit {
                sha: self.id_to_sha[node_id].clone(),
                depth,
            })
            .collect();

        Ok(GraphTraversalResponse {
            strategy,
            direction,
            nodes,
            truncated,
        })
    }

    fn shortest_path(
        &self,
        from_sha: &str,
        to_sha: &str,
        direction: GraphDirection,
    ) -> AppResult<ShortestPathResponse> {
        let Some(from_id) = self.sha_to_id.get(from_sha).copied() else {
            return Err(AppError::InvalidArgument(format!(
                "source commit not indexed for repository: {from_sha}"
            )));
        };
        let Some(to_id) = self.sha_to_id.get(to_sha).copied() else {
            return Err(AppError::InvalidArgument(format!(
                "target commit not indexed for repository: {to_sha}"
            )));
        };

        if from_id == to_id {
            return Ok(ShortestPathResponse {
                from_sha: from_sha.to_string(),
                to_sha: to_sha.to_string(),
                direction,
                found: true,
                path: vec![from_sha.to_string()],
                visited_nodes: 1,
            });
        }

        let reverse_direction = reverse_direction(direction);
        let mut forward = SearchSide::new(from_id);
        let mut backward = SearchSide::new(to_id);
        let mut meeting: Option<NodeId> = None;

        while !forward.frontier.is_empty() && !backward.frontier.is_empty() && meeting.is_none() {
            let expand_forward = forward.frontier.len() <= backward.frontier.len();
            if expand_forward {
                meeting = self.expand_frontier(&mut forward, &backward.visited, direction)?;
            } else {
                meeting =
                    self.expand_frontier(&mut backward, &forward.visited, reverse_direction)?;
            }
        }

        let mut path = Vec::new();
        if let Some(meeting_id) = meeting {
            let id_path = reconstruct_bidirectional_path(
                from_id,
                to_id,
                meeting_id,
                &forward.predecessors,
                &backward.predecessors,
            )?;
            path = id_path
                .into_iter()
                .map(|node_id| self.id_to_sha[node_id].clone())
                .collect();
        }

        Ok(ShortestPathResponse {
            from_sha: from_sha.to_string(),
            to_sha: to_sha.to_string(),
            direction,
            found: meeting.is_some(),
            path,
            visited_nodes: forward.visited.union_count(&backward.visited),
        })
    }

    fn traverse_bfs(
        &self,
        start_id: NodeId,
        direction: GraphDirection,
        max_nodes: usize,
    ) -> AppResult<(Vec<(NodeId, usize)>, bool)> {
        let total_nodes = self.node_count();
        let mut visits = Vec::new();
        let mut visited = DenseSet::default();
        visited.insert(start_id);
        let mut depth = 0usize;
        let mut frontier = vec![start_id];

        while !frontier.is_empty() {
            let remaining_budget = max_nodes.saturating_sub(visits.len());
            if remaining_budget == 0 {
                return Ok((visits, true));
            }

            if frontier.len() > remaining_budget {
                for node_id in frontier.iter().take(remaining_budget) {
                    visits.push((*node_id, depth));
                }
                return Ok((visits, true));
            }

            for node_id in &frontier {
                visits.push((*node_id, depth));
            }

            let remaining_nodes = total_nodes.saturating_sub(visited.count());
            frontier = match select_bfs_mode(frontier.len(), remaining_nodes) {
                BfsMode::Push => self.expand_bfs_frontier_push(&frontier, direction, &mut visited),
                BfsMode::Pull => self.expand_bfs_frontier_pull(&frontier, direction, &mut visited),
            };
            depth = depth.saturating_add(1);
        }

        Ok((visits, false))
    }

    fn expand_bfs_frontier_push(
        &self,
        frontier: &[NodeId],
        direction: GraphDirection,
        visited: &mut DenseSet,
    ) -> Vec<NodeId> {
        let mut next_frontier = Vec::new();
        for source in frontier {
            self.for_each_neighbor(*source, direction, |neighbor| {
                if visited.insert(neighbor) {
                    next_frontier.push(neighbor);
                }
            });
        }
        next_frontier
    }

    fn expand_bfs_frontier_pull(
        &self,
        frontier: &[NodeId],
        direction: GraphDirection,
        visited: &mut DenseSet,
    ) -> Vec<NodeId> {
        let mut discovered = Vec::new();
        for source in frontier {
            self.for_each_neighbor(*source, direction, |neighbor| discovered.push(neighbor));
        }
        discovered.sort_unstable_by(|left, right| right.cmp(left));
        discovered.dedup();

        let mut next_frontier = Vec::with_capacity(discovered.len());
        for node_id in discovered {
            if visited.insert(node_id) {
                next_frontier.push(node_id);
            }
        }
        next_frontier
    }

    fn traverse_dfs(
        &self,
        start_id: NodeId,
        direction: GraphDirection,
        max_nodes: usize,
    ) -> AppResult<(Vec<(NodeId, usize)>, bool)> {
        let mut visits = Vec::new();
        let mut visited = DenseSet::default();
        visited.insert(start_id);
        let mut stack = vec![(start_id, 0usize)];

        while let Some((node_id, depth)) = stack.pop() {
            visits.push((node_id, depth));
            if visits.len() >= max_nodes {
                let has_more =
                    !stack.is_empty() || self.has_unvisited_neighbors(node_id, direction, &visited);
                return Ok((visits, has_more));
            }

            let mut neighbors = Vec::new();
            self.for_each_neighbor(node_id, direction, |neighbor| neighbors.push(neighbor));
            for neighbor in neighbors.into_iter().rev() {
                if visited.insert(neighbor) {
                    stack.push((neighbor, depth.saturating_add(1)));
                }
            }
        }

        Ok((visits, false))
    }

    fn traverse_topological(
        &self,
        start_id: NodeId,
        direction: GraphDirection,
        max_nodes: usize,
    ) -> AppResult<(Vec<(NodeId, usize)>, bool)> {
        let (reachable, reachable_depths, truncated_reachable) =
            self.collect_reachable_bfs(start_id, direction, max_nodes)?;
        let reachable_count = reachable.len();
        let mut local_index_by_node = vec![usize::MAX; self.node_count()];
        for (local_index, node_id) in reachable.iter().copied().enumerate() {
            local_index_by_node[node_id] = local_index;
        }

        let mut indegree = vec![0usize; reachable_count];
        let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); reachable_count];

        for (source_local_index, source_id) in reachable.iter().copied().enumerate() {
            self.for_each_neighbor(source_id, direction, |neighbor_id| {
                if let Some(&neighbor_local_index) = local_index_by_node.get(neighbor_id) {
                    if neighbor_local_index != usize::MAX {
                        adjacency[source_local_index].push(neighbor_local_index);
                    }
                }
            });

            adjacency[source_local_index].sort_unstable();
            adjacency[source_local_index].dedup();
            for &neighbor_local_index in &adjacency[source_local_index] {
                indegree[neighbor_local_index] = indegree[neighbor_local_index].saturating_add(1);
            }
        }

        let max_depth = reachable_depths.iter().copied().max().unwrap_or_default();
        let mut ready_depth_buckets: Vec<VecDeque<usize>> = std::iter::repeat_with(VecDeque::new)
            .take(max_depth.saturating_add(1))
            .collect();
        let mut next_ready_depth = ready_depth_buckets.len();
        for local_index in 0..reachable_count {
            if indegree[local_index] == 0 {
                let depth = reachable_depths[local_index];
                ready_depth_buckets[depth].push_back(local_index);
                next_ready_depth = next_ready_depth.min(depth);
            }
        }

        let mut ordered = Vec::with_capacity(reachable_count);
        let mut emitted_local = vec![false; reachable_count];
        while next_ready_depth < ready_depth_buckets.len() {
            while next_ready_depth < ready_depth_buckets.len()
                && ready_depth_buckets[next_ready_depth].is_empty()
            {
                next_ready_depth = next_ready_depth.saturating_add(1);
            }
            if next_ready_depth >= ready_depth_buckets.len() {
                break;
            }

            let Some(source_local_index) = ready_depth_buckets[next_ready_depth].pop_front() else {
                continue;
            };
            emitted_local[source_local_index] = true;
            ordered.push((
                reachable[source_local_index],
                reachable_depths[source_local_index],
            ));

            for &neighbor_local_index in &adjacency[source_local_index] {
                if indegree[neighbor_local_index] == 0 {
                    continue;
                }
                indegree[neighbor_local_index] -= 1;
                if indegree[neighbor_local_index] == 0 {
                    let neighbor_depth = reachable_depths[neighbor_local_index];
                    ready_depth_buckets[neighbor_depth].push_back(neighbor_local_index);
                    if neighbor_depth < next_ready_depth {
                        next_ready_depth = neighbor_depth;
                    }
                }
            }
        }

        let mut truncated = truncated_reachable || ordered.len() < reachable_count;
        if ordered.len() < reachable_count {
            for (local_index, was_emitted) in emitted_local.into_iter().enumerate() {
                if !was_emitted {
                    ordered.push((reachable[local_index], reachable_depths[local_index]));
                }
            }
            truncated = true;
        }

        Ok((ordered, truncated))
    }

    fn collect_reachable_bfs(
        &self,
        start_id: NodeId,
        direction: GraphDirection,
        max_nodes: usize,
    ) -> AppResult<(Vec<NodeId>, Vec<usize>, bool)> {
        let mut order = Vec::new();
        let mut reachable_depths = Vec::new();
        let mut visited = DenseSet::default();
        visited.insert(start_id);

        let mut queue = VecDeque::from([(start_id, 0usize)]);
        while !queue.is_empty() {
            let breadth = queue.len();
            let mut level_nodes = Vec::with_capacity(breadth);
            for _ in 0..breadth {
                if let Some((node_id, depth)) = queue.pop_front() {
                    level_nodes.push((node_id, depth));
                }
            }

            let remaining_budget = max_nodes.saturating_sub(order.len());
            if remaining_budget == 0 {
                return Ok((order, reachable_depths, true));
            }

            if level_nodes.len() > remaining_budget {
                for (node_id, depth) in level_nodes.iter().take(remaining_budget) {
                    order.push(*node_id);
                    reachable_depths.push(*depth);
                }
                return Ok((order, reachable_depths, true));
            }

            for (node_id, depth) in &level_nodes {
                order.push(*node_id);
                reachable_depths.push(*depth);
            }

            for (node_id, depth) in level_nodes {
                self.for_each_neighbor(node_id, direction, |neighbor| {
                    if visited.insert(neighbor) {
                        queue.push_back((neighbor, depth.saturating_add(1)));
                    }
                });
            }
        }

        Ok((order, reachable_depths, false))
    }

    fn has_unvisited_neighbors(
        &self,
        source: NodeId,
        direction: GraphDirection,
        visited: &DenseSet,
    ) -> bool {
        let mut has_more = false;
        self.for_each_neighbor(source, direction, |neighbor| {
            if !visited.contains(neighbor) {
                has_more = true;
            }
        });
        has_more
    }

    fn expand_frontier(
        &self,
        current_side: &mut SearchSide,
        visited_other_side: &DenseSet,
        direction: GraphDirection,
    ) -> AppResult<Option<NodeId>> {
        let breadth = current_side.frontier.len();
        let mut level_nodes = Vec::with_capacity(breadth);
        for _ in 0..breadth {
            if let Some(node_id) = current_side.frontier.pop_front() {
                level_nodes.push(node_id);
            }
        }

        for source_id in level_nodes {
            let mut neighbors = Vec::new();
            self.for_each_neighbor(source_id, direction, |neighbor| neighbors.push(neighbor));
            for neighbor_id in neighbors {
                if current_side.visited.insert(neighbor_id) {
                    ensure_slot(&mut current_side.predecessors, neighbor_id);
                    current_side.predecessors[neighbor_id] = Some(source_id);

                    if visited_other_side.contains(neighbor_id) {
                        return Ok(Some(neighbor_id));
                    }

                    current_side.frontier.push_back(neighbor_id);
                }
            }
        }

        Ok(None)
    }

    fn for_each_neighbor(
        &self,
        source: NodeId,
        direction: GraphDirection,
        mut visit: impl FnMut(NodeId),
    ) {
        if source >= self.node_count() {
            return;
        }

        match direction {
            GraphDirection::Parents => {
                self.for_each_parent_neighbor(source, &mut visit);
            }
            GraphDirection::Children => {
                self.for_each_child_neighbor(source, &mut visit);
            }
            GraphDirection::Both => {
                self.for_each_parent_neighbor(source, &mut visit);
                self.for_each_child_neighbor(source, &mut visit);
            }
        }
    }

    fn for_each_parent_neighbor(&self, source: NodeId, visit: &mut impl FnMut(NodeId)) {
        let neighbors = if source < self.base_node_count {
            csr_neighbor_slice(&self.parents_offsets, &self.parents_neighbors, source)
        } else {
            let overlay_index = source.saturating_sub(self.base_node_count);
            self.overlay_parent_neighbors
                .get(overlay_index)
                .map(Vec::as_slice)
                .unwrap_or(&[])
        };

        for &neighbor in neighbors {
            if let Ok(id) = usize::try_from(neighbor) {
                visit(id);
            }
        }
    }

    fn for_each_child_neighbor(&self, source: NodeId, visit: &mut impl FnMut(NodeId)) {
        if source < self.base_node_count {
            for &neighbor in
                csr_neighbor_slice(&self.children_offsets, &self.children_neighbors, source)
            {
                if let Ok(id) = usize::try_from(neighbor) {
                    visit(id);
                }
            }
        }

        if let Some(overlay_neighbors) = self.overlay_child_neighbors.get(&source) {
            for &neighbor in overlay_neighbors {
                if let Ok(id) = usize::try_from(neighbor) {
                    visit(id);
                }
            }
        }
    }
}

fn csr_neighbor_slice<'a>(offsets: &[u32], neighbors: &'a [u32], node_id: usize) -> &'a [u32] {
    let Some(&start) = offsets.get(node_id) else {
        return &[];
    };
    let Some(&end) = offsets.get(node_id.saturating_add(1)) else {
        return &[];
    };
    let Ok(start_index) = usize::try_from(start) else {
        return &[];
    };
    let Ok(end_index) = usize::try_from(end) else {
        return &[];
    };
    if start_index > end_index || end_index > neighbors.len() {
        return &[];
    }
    &neighbors[start_index..end_index]
}

fn select_bfs_mode(frontier_len: usize, remaining_nodes: usize) -> BfsMode {
    if frontier_len < BFS_PULL_MIN_FRONTIER
        || remaining_nodes < BFS_PULL_MIN_REMAINING
        || remaining_nodes == 0
    {
        return BfsMode::Push;
    }

    if frontier_len.saturating_mul(BFS_PULL_FRONTIER_TO_REMAINING_RATIO) >= remaining_nodes {
        BfsMode::Pull
    } else {
        BfsMode::Push
    }
}

fn reverse_direction(direction: GraphDirection) -> GraphDirection {
    match direction {
        GraphDirection::Parents => GraphDirection::Children,
        GraphDirection::Children => GraphDirection::Parents,
        GraphDirection::Both => GraphDirection::Both,
    }
}

fn reconstruct_bidirectional_path(
    from_id: NodeId,
    to_id: NodeId,
    meeting_id: NodeId,
    forward_prev: &[Option<NodeId>],
    backward_prev: &[Option<NodeId>],
) -> AppResult<Vec<NodeId>> {
    let mut left = vec![meeting_id];
    let mut cursor = meeting_id;
    while cursor != from_id {
        let Some(prev) = forward_prev.get(cursor).copied().flatten() else {
            return Err(AppError::Parse(
                "path reconstruction failed from source side".to_string(),
            ));
        };
        cursor = prev;
        left.push(cursor);
    }
    left.reverse();

    let mut right = Vec::new();
    let mut cursor = meeting_id;
    while cursor != to_id {
        let Some(next) = backward_prev.get(cursor).copied().flatten() else {
            return Err(AppError::Parse(
                "path reconstruction failed from target side".to_string(),
            ));
        };
        cursor = next;
        right.push(cursor);
    }

    left.extend(right);
    Ok(left)
}

fn ensure_slot<T>(slots: &mut Vec<Option<T>>, index: usize) {
    if index >= slots.len() {
        slots.resize_with(index + 1, || None);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        median_usize, resolve_initial_commit_batch_size, CommitWriteWaveController, IndexingMode,
        COMMIT_WRITE_WAVE_INITIAL_FULL, COMMIT_WRITE_WAVE_INITIAL_INCREMENTAL,
        COMMIT_WRITE_WAVE_MAX, COMMIT_WRITE_WAVE_MIN,
    };
    use std::time::Duration;

    #[test]
    fn commit_wave_initial_batch_defaults_follow_mode() {
        assert_eq!(
            resolve_initial_commit_batch_size(IndexingMode::Full, None),
            COMMIT_WRITE_WAVE_INITIAL_FULL
        );
        assert_eq!(
            resolve_initial_commit_batch_size(IndexingMode::Incremental, None),
            COMMIT_WRITE_WAVE_INITIAL_INCREMENTAL
        );
    }

    #[test]
    fn commit_wave_controller_adapts_and_clamps() {
        let mut controller = CommitWriteWaveController::new(8_192, false, true);
        controller.observe_flush(8_192, Duration::from_millis(600));
        assert_eq!(controller.target_batch_size(), 16_384);
        controller.observe_flush(16_384, Duration::from_millis(7_000));
        assert_eq!(controller.target_batch_size(), 8_192);

        let mut low = CommitWriteWaveController::new(COMMIT_WRITE_WAVE_MIN, false, true);
        low.observe_flush(COMMIT_WRITE_WAVE_MIN, Duration::from_millis(20_000));
        assert_eq!(low.target_batch_size(), COMMIT_WRITE_WAVE_MIN);

        let mut high = CommitWriteWaveController::new(COMMIT_WRITE_WAVE_MAX, false, true);
        high.observe_flush(COMMIT_WRITE_WAVE_MAX, Duration::from_millis(100));
        assert_eq!(high.target_batch_size(), COMMIT_WRITE_WAVE_MAX);
    }

    #[test]
    fn commit_walk_deconvolution_keeps_pure_walk_non_negative() {
        let walk_total = Duration::from_millis(50);
        let callback = Duration::from_millis(80);
        let pure_walk = walk_total.saturating_sub(callback);
        assert_eq!(pure_walk, Duration::from_millis(0));
        assert_eq!(median_usize(&[1024, 8192, 16384]), 8192.0);
        assert_eq!(median_usize(&[2048, 4096]), 3072.0);
    }
}
