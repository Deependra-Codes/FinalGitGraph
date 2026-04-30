use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IndexingMode {
    #[default]
    Full,
    Incremental,
}

impl IndexingMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "full" => Some(Self::Full),
            "incremental" => Some(Self::Incremental),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Incremental => "incremental",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlannerRiskLevel {
    #[default]
    Low,
    Medium,
    High,
}

impl PlannerRiskLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlannerDegradedPolicy {
    #[default]
    None,
    FallbackToFull,
}

impl PlannerDegradedPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::FallbackToFull => "fallback_to_full",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GraphTraversalStrategy {
    #[default]
    Bfs,
    Dfs,
    Topological,
}

impl GraphTraversalStrategy {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "bfs" => Some(Self::Bfs),
            "dfs" => Some(Self::Dfs),
            "topological" => Some(Self::Topological),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GraphDirection {
    #[default]
    Parents,
    Children,
    Both,
}

impl GraphDirection {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "parents" => Some(Self::Parents),
            "children" => Some(Self::Children),
            "both" => Some(Self::Both),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanRepositoryRequest {
    pub path: String,
    pub db_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexRepositoryRequest {
    pub path: String,
    pub db_path: Option<String>,
    pub mode: IndexingMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitWindowCursor {
    pub committer_time: i64,
    pub sha: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitWindowRequest {
    pub path: String,
    pub db_path: Option<String>,
    pub limit: usize,
    pub cursor: Option<CommitWindowCursor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphTraversalRequest {
    pub path: String,
    pub db_path: Option<String>,
    pub start_sha: String,
    pub strategy: GraphTraversalStrategy,
    pub direction: GraphDirection,
    pub max_nodes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortestPathRequest {
    pub path: String,
    pub db_path: Option<String>,
    pub from_sha: String,
    pub to_sha: String,
    pub direction: GraphDirection,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommandError {
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CommandError {}

#[derive(Debug, Clone, Serialize)]
pub struct RepositorySummary {
    pub path: String,
    pub git_dir: String,
    pub name: String,
    pub is_valid: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct IndexingStats {
    pub repo_id: i64,
    pub mode: IndexingMode,
    pub planner_session_id: u64,
    pub planner_requested_mode: IndexingMode,
    pub planner_effective_mode: IndexingMode,
    pub planner_checkpoint_trusted: bool,
    pub planner_risk_level: PlannerRiskLevel,
    pub planner_degraded_policy: PlannerDegradedPolicy,
    pub planner_trace_count: usize,
    pub lmdb_effective_map_size_bytes: usize,
    pub lmdb_no_readahead: bool,
    pub lmdb_write_map: bool,
    pub lmdb_map_async: bool,
    pub lmdb_no_mem_init: bool,
    pub commits_indexed: usize,
    pub edges_indexed: usize,
    pub file_changes_indexed: usize,
    pub file_change_commits_enqueued: usize,
    pub file_change_queue_depth_after_index: usize,
    pub file_change_queue_oldest_age_seconds: i64,
    pub refs_indexed: usize,
    pub indexed_at_epoch: i64,
    pub commit_walk_total_ms: f64,
    pub commit_callback_ms: f64,
    pub commit_walk_decode_ms: f64,
    pub commit_walk_parent_resolve_ms: f64,
    pub commit_walk_metadata_parse_ms: f64,
    pub commit_walk_tip_collection_ms: f64,
    pub commit_walk_hide_expansion_ms: f64,
    pub commit_walk_graph_lookup_ms: f64,
    pub commit_walk_ordering_ms: f64,
    pub commit_walk_emit_ms: f64,
    pub commit_walk_pack_inflate_ms: f64,
    pub commit_walk_pack_base_resolve_ms: f64,
    pub commit_walk_pack_parse_ms: f64,
    pub commit_walk_tape_build_ms: f64,
    pub commit_walk_graph_lookup_hit_count: usize,
    pub commit_walk_graph_lookup_miss_count: usize,
    pub commit_walk_graph_assist_hit_count: usize,
    pub commit_walk_graph_assist_miss_count: usize,
    pub commit_walk_nodes_seen_count: usize,
    pub commit_walk_nodes_pruned_count: usize,
    pub commit_walk_queue_max_depth: usize,
    pub commit_metadata_pending_count: usize,
    pub commit_walk_boundary_mode: String,
    pub commit_walk_graph_missing_tip_count: usize,
    pub commit_walk_boundary_stop_count: usize,
    pub commit_walk_overlap_skip_count: usize,
    pub commit_walk_tape_node_count: usize,
    pub commit_walk_tape_parent_edges: usize,
    pub commit_window_jit_hydrated_rows: usize,
    pub query_commit_window_prewarm_hydrated_rows: usize,
    pub incremental_csr_streaming_mode_effective: String,
    pub commit_write_wave_txn_count: usize,
    pub commit_write_wave_batch_median: f64,
    pub commit_ingest_queue_max_depth: usize,
    pub commit_ingest_queue_wait_ms: f64,
    pub commit_csr_delta_apply_ms: f64,
    pub commit_csr_overlay_upsert_ms: f64,
    pub commit_csr_compaction_ms: f64,
    pub ingestion_phase_timing_ms: IngestionPhaseTimingMs,
    pub commit_throughput: CommitThroughputTelemetry,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct FileChangeExtractionQueueMetrics {
    pub depth: usize,
    pub oldest_queue_age_seconds: i64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct FileChangeExtractionDrainStats {
    pub drained_commits: usize,
    pub failed_commits: usize,
    pub file_changes_indexed: usize,
    pub queue: FileChangeExtractionQueueMetrics,
    pub phase_timing_ms: FileChangeExtractionDrainPhaseTimingMs,
    pub batch_stats: FileChangeExtractionDrainBatchStats,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct FileChangeExtractionDrainPhaseTimingMs {
    pub queue_fetch: f64,
    pub diff_extract: f64,
    pub stage_insert: f64,
    pub delete_old_rows: f64,
    pub file_change_merge_upsert: f64,
    pub enrichment_enqueue: f64,
    pub queue_clear: f64,
    pub tx_commit: f64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct FileChangeExtractionDrainBatchStats {
    pub batches: usize,
    pub commits_per_batch_mean: f64,
    pub file_changes_per_batch_mean: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeStatsState {
    Pending,
    Ready,
    Failed,
}

impl FileChangeStatsState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }

    pub fn parse_db(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "ready" => Some(Self::Ready),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommitMetadataState {
    Pending,
    Ready,
    Failed,
}

impl CommitMetadataState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }

    pub fn parse_db(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "ready" => Some(Self::Ready),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct FileChangeStatsStateCounts {
    pub pending: usize,
    pub ready: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct IngestionPhaseTimingMs {
    pub commit_walk: f64,
    pub commit_walk_total: f64,
    pub commit_callback: f64,
    pub commit_walk_decode: f64,
    pub commit_walk_parent_resolve: f64,
    pub commit_walk_metadata_parse: f64,
    pub commit_walk_tip_collection: f64,
    pub commit_walk_hide_expansion: f64,
    pub commit_walk_graph_lookup: f64,
    pub commit_walk_ordering: f64,
    pub commit_walk_emit: f64,
    pub commit_walk_pack_inflate: f64,
    pub commit_walk_pack_base_resolve: f64,
    pub commit_walk_pack_parse: f64,
    pub commit_walk_tape_build: f64,
    pub commit_walk_graph_lookup_hit_count: f64,
    pub commit_walk_graph_lookup_miss_count: f64,
    pub commit_walk_graph_assist_hit_count: f64,
    pub commit_walk_graph_assist_miss_count: f64,
    pub commit_walk_nodes_seen_count: f64,
    pub commit_walk_nodes_pruned_count: f64,
    pub commit_walk_queue_max_depth: f64,
    pub commit_metadata_pending_count: f64,
    pub commit_walk_graph_missing_tip_count: f64,
    pub commit_walk_boundary_stop_count: f64,
    pub commit_walk_overlap_skip_count: f64,
    pub commit_walk_tape_node_count: f64,
    pub commit_walk_tape_parent_edges: f64,
    pub commit_upsert: f64,
    pub edge_materialize: f64,
    pub file_diff_extract: f64,
    pub file_diff_build: f64,
    pub file_diff_rename_find: f64,
    pub file_diff_delta_emit: f64,
    pub extraction_dispatch_wait: f64,
    pub extraction_worker_busy: f64,
    pub extraction_reorder_buffer_max_depth: f64,
    pub extraction_worker_errors: f64,
    pub extraction_worker_aborts: f64,
    pub file_change_delete: f64,
    pub file_change_upsert: f64,
    pub jit_enrich_coscheduled: f64,
    pub commit_ingest_queue_wait: f64,
    pub commit_ingest_queue_max_depth: f64,
    pub commit_write_wave_txn_count: f64,
    pub commit_write_wave_batch_median: f64,
    pub commit_csr_delta_apply: f64,
    pub commit_csr_overlay_upsert: f64,
    pub commit_csr_compaction: f64,
    pub query_commit_window_prewarm_hydrated_rows: f64,
    pub tx_commit: f64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct CommitThroughputTelemetry {
    pub planner_session_id: u64,
    pub planner_requested_mode: IndexingMode,
    pub planner_effective_mode: IndexingMode,
    pub planner_checkpoint_trusted: bool,
    pub planner_risk_level: PlannerRiskLevel,
    pub planner_degraded_policy: PlannerDegradedPolicy,
    pub planner_trace_count: usize,
    pub commits_seen: usize,
    pub commits_indexed: usize,
    pub end_to_end_ms: f64,
    pub commits_per_sec_seen: f64,
    pub commits_per_sec_indexed: f64,
    pub ms_per_commit_seen: f64,
    pub ms_per_commit_indexed: f64,
    pub commit_walk_scope: String,
    pub commit_walk_engine: String,
    pub commit_walk_sort_effective: String,
    pub commit_walk_boundary_mode: String,
    pub commit_walk_tip_count: usize,
    pub commit_walk_pruned_parent_expansions: usize,
    pub commit_walk_total_ms: f64,
    pub commit_callback_ms: f64,
    pub commit_walk_decode_ms: f64,
    pub commit_walk_parent_resolve_ms: f64,
    pub commit_walk_metadata_parse_ms: f64,
    pub commit_walk_tip_collection_ms: f64,
    pub commit_walk_hide_expansion_ms: f64,
    pub commit_walk_graph_lookup_ms: f64,
    pub commit_walk_ordering_ms: f64,
    pub commit_walk_emit_ms: f64,
    pub commit_walk_pack_inflate_ms: f64,
    pub commit_walk_pack_base_resolve_ms: f64,
    pub commit_walk_pack_parse_ms: f64,
    pub commit_walk_tape_build_ms: f64,
    pub commit_walk_graph_lookup_hit_count: usize,
    pub commit_walk_graph_lookup_miss_count: usize,
    pub commit_walk_graph_assist_hit_count: usize,
    pub commit_walk_graph_assist_miss_count: usize,
    pub commit_walk_nodes_seen_count: usize,
    pub commit_walk_nodes_pruned_count: usize,
    pub commit_walk_queue_max_depth: usize,
    pub commit_metadata_pending_count: usize,
    pub commit_walk_graph_missing_tip_count: usize,
    pub commit_walk_boundary_stop_count: usize,
    pub commit_walk_overlap_skip_count: usize,
    pub commit_walk_tape_node_count: usize,
    pub commit_walk_tape_parent_edges: usize,
    pub commit_window_jit_hydrated_rows: usize,
    pub query_commit_window_prewarm_hydrated_rows: usize,
    pub incremental_csr_streaming_mode_effective: String,
    pub commit_walk_ms: f64,
    pub commit_walk_commits_per_sec_seen: f64,
    pub commit_upsert_ms: f64,
    pub commit_upsert_commits_per_sec_seen: f64,
    pub commit_write_wave_txn_count: usize,
    pub commit_write_wave_batch_median: f64,
    pub commit_ingest_queue_max_depth: usize,
    pub commit_ingest_queue_wait_ms: f64,
    pub commit_csr_delta_apply_ms: f64,
    pub commit_csr_overlay_upsert_ms: f64,
    pub commit_csr_compaction_ms: f64,
    pub file_diff_extract_ms: f64,
    pub file_diff_extract_commits_per_sec_seen: f64,
    pub file_change_upsert_ms: f64,
    pub file_change_upsert_commits_per_sec_seen: f64,
    pub tx_commit_ms: f64,
    pub tx_commit_commits_per_sec_seen: f64,
    pub bottleneck_phase: String,
    pub bottleneck_phase_ms: f64,
    pub bottleneck_phase_share_of_tracked_time: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitWindowItem {
    pub sha: String,
    pub author_name: String,
    pub author_email: String,
    pub committer_time: i64,
    pub subject: String,
    pub tree_sha: String,
    pub metadata_state: CommitMetadataState,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitWindowResponse {
    pub commits: Vec<CommitWindowItem>,
    pub next_cursor: Option<CommitWindowCursor>,
    pub has_more: bool,
    pub jit_hydrated_rows: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphNodeVisit {
    pub sha: String,
    pub depth: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphTraversalResponse {
    pub strategy: GraphTraversalStrategy,
    pub direction: GraphDirection,
    pub nodes: Vec<GraphNodeVisit>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShortestPathResponse {
    pub from_sha: String,
    pub to_sha: String,
    pub direction: GraphDirection,
    pub found: bool,
    pub path: Vec<String>,
    pub visited_nodes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexCheckpoint {
    pub last_committer_time: i64,
    pub last_commit_sha: String,
    pub indexed_at_epoch: i64,
}

#[derive(Debug, Clone)]
pub struct ParsedCommit {
    pub sha: String,
    pub parents: Vec<String>,
    pub tree_sha: String,
    pub author_name: String,
    pub author_email: String,
    pub author_time: i64,
    pub committer_time: i64,
    pub subject: String,
    pub metadata_state: CommitMetadataState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedRef {
    pub ref_name: String,
    pub target_sha: String,
    pub ref_type: String,
}

#[derive(Debug, Clone)]
pub struct ParsedFileChange {
    pub commit_sha: String,
    pub file_path: String,
    pub additions: Option<i64>,
    pub deletions: Option<i64>,
    pub change_type: String,
    pub stats_state: FileChangeStatsState,
    pub requires_line_stats: bool,
}

#[derive(Debug, Clone)]
pub struct PendingFileChangeEnrichment {
    pub commit_sha: String,
    pub file_path: String,
}

#[derive(Debug, Clone)]
pub struct FileChangeLineStatsUpdate {
    pub commit_sha: String,
    pub file_path: String,
    pub additions: i64,
    pub deletions: i64,
}
