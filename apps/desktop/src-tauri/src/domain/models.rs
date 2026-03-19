use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IndexingMode {
    #[default]
    Full,
    Incremental,
}

impl IndexingMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Incremental => "incremental",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "full" => Some(Self::Full),
            "incremental" => Some(Self::Incremental),
            _ => None,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositorySummary {
    pub path: String,
    pub git_dir: String,
    pub name: String,
    pub is_valid: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScanRepositoryResponse {
    pub repository: RepositorySummary,
    pub session_plan: IndexSessionPlan,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryTopology {
    Common,
    LinkedWorkTree,
    Submodule,
}

impl RepositoryTopology {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Common => "common",
            Self::LinkedWorkTree => "linked_worktree",
            Self::Submodule => "submodule",
        }
    }

    pub fn parse_db(value: &str) -> Option<Self> {
        match value {
            "common" => Some(Self::Common),
            "linked_worktree" => Some(Self::LinkedWorkTree),
            "submodule" => Some(Self::Submodule),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryRefStorage {
    Files,
    Reftable,
    Unknown,
}

impl RepositoryRefStorage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Files => "files",
            Self::Reftable => "reftable",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse_db(value: &str) -> Option<Self> {
        match value {
            "files" => Some(Self::Files),
            "reftable" => Some(Self::Reftable),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccelerationReadiness {
    Unavailable,
    Partial,
    Ready,
}

impl AccelerationReadiness {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Partial => "partial",
            Self::Ready => "ready",
        }
    }

    pub fn parse_db(value: &str) -> Option<Self> {
        match value {
            "unavailable" => Some(Self::Unavailable),
            "partial" => Some(Self::Partial),
            "ready" => Some(Self::Ready),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointTrustClass {
    Missing,
    Trusted,
    Invalidated,
}

impl CheckpointTrustClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Trusted => "trusted",
            Self::Invalidated => "invalidated",
        }
    }

    pub fn parse_db(value: &str) -> Option<Self> {
        match value {
            "missing" => Some(Self::Missing),
            "trusted" => Some(Self::Trusted),
            "invalidated" => Some(Self::Invalidated),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceFreshness {
    Unknown,
    Fresh,
    Stale,
}

impl MaintenanceFreshness {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Fresh => "fresh",
            Self::Stale => "stale",
        }
    }

    pub fn parse_db(value: &str) -> Option<Self> {
        match value {
            "unknown" => Some(Self::Unknown),
            "fresh" => Some(Self::Fresh),
            "stale" => Some(Self::Stale),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlannerRiskLevel {
    Nominal,
    Advisory,
    Elevated,
    Critical,
}

impl PlannerRiskLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Nominal => "nominal",
            Self::Advisory => "advisory",
            Self::Elevated => "elevated",
            Self::Critical => "critical",
        }
    }

    pub fn parse_db(value: &str) -> Option<Self> {
        match value {
            "nominal" => Some(Self::Nominal),
            "advisory" => Some(Self::Advisory),
            "elevated" => Some(Self::Elevated),
            "critical" => Some(Self::Critical),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlannerTraceEntry {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryIdentity {
    pub path: String,
    pub git_dir: String,
    pub git_common_dir: String,
    pub object_dir: String,
    pub name: String,
    pub topology: RepositoryTopology,
    pub is_bare: bool,
    pub is_shallow: bool,
    pub superproject_root: Option<String>,
    pub identity_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositoryCapabilities {
    pub object_hash: String,
    pub ref_storage: RepositoryRefStorage,
    pub has_commit_graph: bool,
    pub has_changed_path_bloom_filters: bool,
    pub has_multi_pack_index: bool,
    pub has_sparse_checkout: bool,
    pub has_partial_clone: bool,
    pub has_alternates: bool,
    pub has_split_index: bool,
    pub has_worktree_config: bool,
    pub acceleration_readiness: AccelerationReadiness,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckpointAssessment {
    pub present: bool,
    pub trust_class: CheckpointTrustClass,
    pub invalidation_reasons: Vec<String>,
    pub resume_from_committer_time: Option<i64>,
    pub hide_ancestor_sha: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DegradedModePolicy {
    Nominal,
    FallbackToFull,
    DisableAcceleration,
    LimitedCapabilities,
    ScanOnly,
}

impl DegradedModePolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Nominal => "nominal",
            Self::FallbackToFull => "fallback_to_full",
            Self::DisableAcceleration => "disable_acceleration",
            Self::LimitedCapabilities => "limited_capabilities",
            Self::ScanOnly => "scan_only",
        }
    }

    pub fn parse_db(value: &str) -> Option<Self> {
        match value {
            "nominal" => Some(Self::Nominal),
            "fallback_to_full" => Some(Self::FallbackToFull),
            "disable_acceleration" => Some(Self::DisableAcceleration),
            "limited_capabilities" => Some(Self::LimitedCapabilities),
            "scan_only" => Some(Self::ScanOnly),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlannerFeatureGate {
    IncrementalResume,
    CommitGraphAcceleration,
    ChangedPathBloomFilters,
    MultiPackIndexAcceleration,
    OptimizedObjectStoreAccess,
}

impl PlannerFeatureGate {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::IncrementalResume => "incremental_resume",
            Self::CommitGraphAcceleration => "commit_graph_acceleration",
            Self::ChangedPathBloomFilters => "changed_path_bloom_filters",
            Self::MultiPackIndexAcceleration => "multi_pack_index_acceleration",
            Self::OptimizedObjectStoreAccess => "optimized_object_store_access",
        }
    }

    pub fn parse_db(value: &str) -> Option<Self> {
        match value {
            "incremental_resume" => Some(Self::IncrementalResume),
            "commit_graph_acceleration" => Some(Self::CommitGraphAcceleration),
            "changed_path_bloom_filters" => Some(Self::ChangedPathBloomFilters),
            "multi_pack_index_acceleration" => Some(Self::MultiPackIndexAcceleration),
            "optimized_object_store_access" => Some(Self::OptimizedObjectStoreAccess),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DegradedModeDecision {
    pub policy: DegradedModePolicy,
    pub reasons: Vec<String>,
    pub disabled_features: Vec<PlannerFeatureGate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlannerHealth {
    pub maintenance_freshness: MaintenanceFreshness,
    pub risk_level: PlannerRiskLevel,
    pub findings: Vec<String>,
    pub recommended_actions: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlannerEntrypoint {
    Scan,
    Index,
}

impl PlannerEntrypoint {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scan => "scan",
            Self::Index => "index",
        }
    }

    pub fn parse_db(value: &str) -> Option<Self> {
        match value {
            "scan" => Some(Self::Scan),
            "index" => Some(Self::Index),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexSessionPlan {
    pub planner_session_id: Option<i64>,
    pub entrypoint: PlannerEntrypoint,
    pub repository: RepositoryIdentity,
    pub capabilities: RepositoryCapabilities,
    pub requested_mode: IndexingMode,
    pub effective_mode: IndexingMode,
    pub checkpoint: CheckpointAssessment,
    pub degraded_mode: DegradedModeDecision,
    pub health: PlannerHealth,
    pub planner_trace: Vec<PlannerTraceEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlannerSessionRepositorySnapshot {
    pub path: String,
    pub git_dir: String,
    pub name: String,
    pub identity_fingerprint: String,
    pub topology: RepositoryTopology,
    pub is_bare: bool,
    pub is_shallow: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlannerCheckpointSnapshot {
    pub trust_class: CheckpointTrustClass,
    pub invalidation_reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlannerSessionRecord {
    pub session_id: i64,
    pub repo_id: i64,
    pub planned_at_epoch: i64,
    pub repository: PlannerSessionRepositorySnapshot,
    pub entrypoint: PlannerEntrypoint,
    pub requested_mode: IndexingMode,
    pub effective_mode: IndexingMode,
    pub capabilities: RepositoryCapabilities,
    pub checkpoint: PlannerCheckpointSnapshot,
    pub degraded_mode: DegradedModeDecision,
    pub health: PlannerHealth,
    pub plan_signature: String,
    pub planner_trace: Vec<PlannerTraceEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IndexingStats {
    pub repo_id: i64,
    pub mode: IndexingMode,
    pub commits_indexed: usize,
    pub edges_indexed: usize,
    pub file_changes_indexed: usize,
    pub refs_indexed: usize,
    pub indexed_at_epoch: i64,
    pub ingestion_phase_timing_ms: IngestionPhaseTimingMs,
    pub ingestion_geometry: IngestionGeometry,
    pub session_plan: IndexSessionPlan,
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

#[derive(Debug, Clone, Serialize, Default)]
pub struct FileChangeStatsStateCounts {
    pub pending: usize,
    pub ready: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct IngestionPhaseTimingMs {
    pub commit_walk: f64,
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
    pub tx_commit: f64,
    pub lsm_shadow_write_attempts: f64,
    pub lsm_shadow_write_success: f64,
    pub lsm_shadow_write_errors: f64,
    pub lsm_shadow_commit_batches: f64,
    pub lsm_shadow_file_change_batches: f64,
    pub lsm_shadow_disabled_batches: f64,
    pub lsm_shadow_bytes_written: f64,
    pub lsm_shadow_write_ms: f64,
    pub lsm_materialization_batches: f64,
    pub lsm_materialization_rows: f64,
    pub lsm_materialization_errors: f64,
    pub lsm_materialization_ms: f64,
    pub lsm_materialization_queue_fetch_ms: f64,
    pub lsm_materialization_decode_ms: f64,
    pub lsm_materialization_group_ms: f64,
    pub lsm_materialization_sqlite_write_ms: f64,
    pub lsm_materialization_queue_cleanup_ms: f64,
    pub lsm_materialization_checkpoint_update_ms: f64,
    pub lsm_materialization_empty_check_ms: f64,
    pub lsm_materialization_keys_scanned: f64,
    pub lsm_materialization_rows_decoded: f64,
    pub lsm_materialization_invalid_rows: f64,
    pub lsm_materialization_rows_applied: f64,
    pub lsm_materialization_rows_unresolved: f64,
    pub lsm_materialization_commit_edges_immediate: f64,
    pub lsm_materialization_commit_edges_resolved: f64,
    pub lsm_materialization_commit_edges_unresolved_staged: f64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct IngestionGeometry {
    pub file_change_pipeline_active: bool,
    pub file_change_pipeline_workers: usize,
    pub file_change_pipeline_commit_batch_size: usize,
    pub file_change_pipeline_result_channel_capacity: usize,
    pub file_change_fast_commit_batch_size: usize,
    pub file_change_drain_commit_batch_size: usize,
    pub file_change_db_batch_size: usize,
    pub file_change_diff_worker_threads: usize,
    pub file_change_drain_parallel_chunk_size: usize,
    pub file_change_drain_channel_capacity: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckpointRepositoryState {
    pub identity_fingerprint: String,
    pub topology: RepositoryTopology,
    pub is_bare: bool,
    pub is_shallow: bool,
    pub object_hash: String,
    pub ref_storage: RepositoryRefStorage,
    pub has_sparse_checkout: bool,
    pub has_partial_clone: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitWindowItem {
    pub sha: String,
    pub author_name: String,
    pub author_email: String,
    pub committer_time: i64,
    pub subject: String,
    pub tree_sha: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitWindowResponse {
    pub commits: Vec<CommitWindowItem>,
    pub next_cursor: Option<CommitWindowCursor>,
    pub has_more: bool,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexCheckpoint {
    pub last_committer_time: i64,
    pub last_commit_sha: String,
    pub indexed_at_epoch: i64,
    pub repository_state: Option<CheckpointRepositoryState>,
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
}

#[derive(Debug, Clone)]
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
