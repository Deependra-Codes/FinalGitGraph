use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rusqlite::Connection;

use crate::adapters::db::{load_index_checkpoint, persist_planner_session, upsert_repository};
use crate::domain::models::{
    AccelerationReadiness, CheckpointAssessment, CheckpointRepositoryState, CheckpointTrustClass,
    DegradedModeDecision, DegradedModePolicy, IndexCheckpoint, IndexSessionPlan, IndexingMode,
    MaintenanceFreshness, PlannerEntrypoint, PlannerFeatureGate, PlannerHealth, PlannerRiskLevel,
    PlannerTraceEntry, RepositoryCapabilities, RepositoryIdentity, RepositoryRefStorage,
    RepositoryTopology,
};
use crate::support::clock::now_epoch_seconds;
use crate::support::error::{AppError, AppResult};

#[derive(Debug, Clone)]
pub(crate) struct CanonicalRepo {
    pub(crate) root: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct RepositoryContext {
    pub(crate) canonical_repo: CanonicalRepo,
    pub(crate) identity: RepositoryIdentity,
    pub(crate) capabilities: RepositoryCapabilities,
    pub(crate) maintenance_signals: RepositoryMaintenanceSignals,
    pub(crate) planner_trace: Vec<PlannerTraceEntry>,
}

#[derive(Debug, Clone)]
pub(crate) struct PlannedIndexSession {
    pub(crate) canonical_repo: CanonicalRepo,
    pub(crate) repo_id: i64,
    pub(crate) plan: IndexSessionPlan,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RepositoryMaintenanceSignals {
    pub(crate) gc_log_present: bool,
    pub(crate) commit_graph_behind_packfiles: bool,
    pub(crate) multi_pack_index_behind_packfiles: bool,
}

pub(crate) fn resolve_repository_context(path: &Path) -> AppResult<RepositoryContext> {
    if !path.exists() {
        return Err(AppError::PathNotFound(path.display().to_string()));
    }

    let canonical_input = fs::canonicalize(path)?;
    let repo = gix::discover(&canonical_input)
        .map_err(|_| AppError::NotGitRepository(path.display().to_string()))?;
    let is_bare = repo.is_bare();
    let root = if let Some(workdir) = repo.workdir() {
        fs::canonicalize(workdir)?
    } else {
        fs::canonicalize(repo.git_dir())?
    };
    let git_dir = fs::canonicalize(repo.git_dir())?;
    let common_dir = fs::canonicalize(repo.common_dir()).unwrap_or_else(|_| git_dir.clone());
    let object_dir = common_dir.join("objects");
    let is_shallow = common_dir.join("shallow").exists();
    let topology = match repo.kind() {
        gix::repository::Kind::Common => RepositoryTopology::Common,
        gix::repository::Kind::LinkedWorkTree => RepositoryTopology::LinkedWorkTree,
        gix::repository::Kind::Submodule => RepositoryTopology::Submodule,
    };
    let name = root
        .file_name()
        .map(|segment| segment.to_string_lossy().to_string())
        .unwrap_or_else(|| "repo".to_string());
    let object_hash = repo.object_hash().to_string();

    let canonical_repo = CanonicalRepo { root: root.clone() };

    let identity = RepositoryIdentity {
        path: root.display().to_string(),
        git_dir: git_dir.display().to_string(),
        git_common_dir: common_dir.display().to_string(),
        object_dir: object_dir.display().to_string(),
        name,
        topology,
        is_bare,
        is_shallow,
        superproject_root: None,
        identity_fingerprint: format!("{}::{}", common_dir.display(), root.display()),
    };

    let has_commit_graph = object_dir.join("info").join("commit-graph").exists()
        || object_dir
            .join("info")
            .join("commit-graphs")
            .join("commit-graph-chain")
            .exists();
    let has_changed_path_bloom_filters = detect_changed_path_bloom_filters(&object_dir);
    let has_multi_pack_index = object_dir.join("pack").join("multi-pack-index").exists();
    let has_sparse_checkout = common_dir.join("info").join("sparse-checkout").exists();
    let has_partial_clone = detect_partial_clone(&object_dir);
    let has_alternates = object_dir.join("info").join("alternates").exists();
    let has_split_index = detect_split_index(&git_dir);
    let has_worktree_config = git_dir.join("config.worktree").exists();
    let maintenance_signals = detect_repository_maintenance_signals(
        &common_dir,
        &object_dir,
        has_commit_graph,
        has_multi_pack_index,
    );
    let ref_storage = if common_dir.join("reftable").exists() {
        RepositoryRefStorage::Reftable
    } else {
        RepositoryRefStorage::Files
    };
    let capabilities = RepositoryCapabilities {
        object_hash,
        ref_storage,
        has_commit_graph,
        has_changed_path_bloom_filters,
        has_multi_pack_index,
        has_sparse_checkout,
        has_partial_clone,
        has_alternates,
        has_split_index,
        has_worktree_config,
        acceleration_readiness: resolve_acceleration_readiness(
            has_commit_graph,
            has_changed_path_bloom_filters,
            has_multi_pack_index,
        ),
    };

    let mut planner_trace = vec![trace(
        "repository_identity_resolved",
        format!(
            "resolved repository topology={} bare={} shallow={}",
            topology_label(topology),
            is_bare,
            is_shallow
        ),
    )];
    if capabilities.has_commit_graph {
        planner_trace.push(trace(
            "repository_commit_graph_detected",
            "commit-graph metadata is available".to_string(),
        ));
    }
    if capabilities.has_multi_pack_index {
        planner_trace.push(trace(
            "repository_multi_pack_index_detected",
            "multi-pack-index metadata is available".to_string(),
        ));
    }
    if capabilities.has_changed_path_bloom_filters {
        planner_trace.push(trace(
            "repository_changed_path_bloom_filters_detected",
            "commit-graph changed-path Bloom filters are available".to_string(),
        ));
    }
    if capabilities.has_sparse_checkout {
        planner_trace.push(trace(
            "repository_sparse_checkout_detected",
            "sparse-checkout metadata is active".to_string(),
        ));
    }
    if capabilities.has_partial_clone {
        planner_trace.push(trace(
            "repository_partial_clone_detected",
            "partial-clone promisor metadata is present".to_string(),
        ));
    }
    if capabilities.has_alternates {
        planner_trace.push(trace(
            "repository_alternates_detected",
            "alternate object-store configuration is present".to_string(),
        ));
    }
    if capabilities.has_split_index {
        planner_trace.push(trace(
            "repository_split_index_detected",
            "split-index metadata is present".to_string(),
        ));
    }
    if capabilities.has_worktree_config {
        planner_trace.push(trace(
            "repository_worktree_config_detected",
            "worktree-local Git config is present".to_string(),
        ));
    }
    if maintenance_signals.gc_log_present {
        planner_trace.push(trace(
            "repository_gc_log_detected",
            "gc.log is present; repository maintenance may be blocked or previously failed"
                .to_string(),
        ));
    }
    if maintenance_signals.commit_graph_behind_packfiles {
        planner_trace.push(trace(
            "repository_commit_graph_stale",
            "commit-graph metadata appears older than pack metadata".to_string(),
        ));
    }
    if maintenance_signals.multi_pack_index_behind_packfiles {
        planner_trace.push(trace(
            "repository_multi_pack_index_stale",
            "multi-pack-index appears older than pack metadata".to_string(),
        ));
    }

    Ok(RepositoryContext {
        canonical_repo,
        identity,
        capabilities,
        maintenance_signals,
        planner_trace,
    })
}

pub(crate) fn plan_scan_session(conn: &Connection, path: &Path) -> AppResult<PlannedIndexSession> {
    plan_repository_session(conn, path, IndexingMode::Full, PlannerEntrypoint::Scan)
}

pub(crate) fn plan_index_session(
    conn: &Connection,
    path: &Path,
    requested_mode: IndexingMode,
) -> AppResult<PlannedIndexSession> {
    plan_repository_session(conn, path, requested_mode, PlannerEntrypoint::Index)
}

fn plan_repository_session(
    conn: &Connection,
    path: &Path,
    requested_mode: IndexingMode,
    entrypoint: PlannerEntrypoint,
) -> AppResult<PlannedIndexSession> {
    let context = resolve_repository_context(path)?;
    let repo_id = upsert_repository(
        conn,
        context.identity.path.as_str(),
        context.identity.name.as_str(),
        context.identity.git_dir.as_str(),
    )?;
    let checkpoint = load_index_checkpoint(conn, repo_id)?;
    let (effective_mode, checkpoint_assessment, mut checkpoint_trace) = assess_checkpoint(
        requested_mode,
        &context.identity,
        &context.capabilities,
        checkpoint.as_ref(),
    );
    let degraded_mode = determine_degraded_mode(
        &context.capabilities,
        requested_mode,
        effective_mode,
        &checkpoint_assessment,
    );
    let health = determine_planner_health(
        &context.capabilities,
        &context.maintenance_signals,
        requested_mode,
        effective_mode,
        &checkpoint_assessment,
        &degraded_mode,
    );
    let mut planner_trace = context.planner_trace.clone();
    planner_trace.append(&mut checkpoint_trace);
    planner_trace.push(trace(
        "degraded_mode_selected",
        format!(
            "degraded_mode_policy={} reasons={} disabled_features={}",
            degraded_mode_label(degraded_mode.policy),
            degraded_mode.reasons.join(","),
            degraded_mode
                .disabled_features
                .iter()
                .map(|feature| feature.as_str())
                .collect::<Vec<_>>()
                .join(",")
        ),
    ));
    planner_trace.push(trace(
        "session_plan_selected",
        format!(
            "entrypoint={} requested_mode={} effective_mode={}",
            entrypoint.as_str(),
            mode_label(requested_mode),
            mode_label(effective_mode)
        ),
    ));
    planner_trace.push(trace(
        "planner_health_selected",
        format!(
            "risk_level={} maintenance_freshness={} findings={} recommendations={}",
            health.risk_level.as_str(),
            health.maintenance_freshness.as_str(),
            health.findings.join(","),
            health.recommended_actions.join(",")
        ),
    ));

    let mut plan = IndexSessionPlan {
        planner_session_id: None,
        entrypoint,
        repository: context.identity,
        capabilities: context.capabilities,
        requested_mode,
        effective_mode,
        checkpoint: checkpoint_assessment,
        degraded_mode,
        health,
        planner_trace,
    };
    let planner_session_id = persist_planner_session(conn, repo_id, &plan, now_epoch_seconds())?;
    plan.planner_session_id = Some(planner_session_id);

    Ok(PlannedIndexSession {
        canonical_repo: context.canonical_repo,
        repo_id,
        plan,
    })
}

fn assess_checkpoint(
    requested_mode: IndexingMode,
    repository_identity: &RepositoryIdentity,
    repository_capabilities: &RepositoryCapabilities,
    checkpoint: Option<&IndexCheckpoint>,
) -> (IndexingMode, CheckpointAssessment, Vec<PlannerTraceEntry>) {
    let (assessment, mut checkpoint_trace) =
        evaluate_checkpoint_assessment(repository_identity, repository_capabilities, checkpoint);
    match requested_mode {
        IndexingMode::Full => (
            IndexingMode::Full,
            CheckpointAssessment {
                resume_from_committer_time: None,
                hide_ancestor_sha: None,
                ..assessment
            },
            {
                checkpoint_trace.push(trace(
                    "full_index_requested",
                    "full indexing requested; checkpoint resume path disabled".to_string(),
                ));
                checkpoint_trace
            },
        ),
        IndexingMode::Incremental => match (checkpoint, assessment.trust_class) {
            (Some(checkpoint), CheckpointTrustClass::Trusted) => (
                IndexingMode::Incremental,
                CheckpointAssessment {
                    resume_from_committer_time: Some(checkpoint.last_committer_time),
                    hide_ancestor_sha: Some(checkpoint.last_commit_sha.clone()),
                    ..assessment
                },
                {
                    checkpoint_trace.push(trace(
                        "incremental_checkpoint_trusted",
                        format!(
                            "incremental indexing enabled from checkpoint sha={}",
                            checkpoint.last_commit_sha
                        ),
                    ));
                    checkpoint_trace
                },
            ),
            _ => {
                let fallback_reason = if assessment.invalidation_reasons.is_empty() {
                    "missing_checkpoint".to_string()
                } else {
                    assessment.invalidation_reasons.join(",")
                };
                checkpoint_trace.push(trace(
                    "incremental_fallback_full",
                    format!(
                        "incremental indexing requested but checkpoint trust was {}; falling back to full",
                        fallback_reason
                    ),
                ));
                (
                    IndexingMode::Full,
                    CheckpointAssessment {
                        resume_from_committer_time: None,
                        hide_ancestor_sha: None,
                        ..assessment
                    },
                    checkpoint_trace,
                )
            }
        },
    }
}

fn evaluate_checkpoint_assessment(
    repository_identity: &RepositoryIdentity,
    repository_capabilities: &RepositoryCapabilities,
    checkpoint: Option<&IndexCheckpoint>,
) -> (CheckpointAssessment, Vec<PlannerTraceEntry>) {
    let Some(checkpoint) = checkpoint else {
        return (
            CheckpointAssessment {
                present: false,
                trust_class: CheckpointTrustClass::Missing,
                invalidation_reasons: vec!["missing_checkpoint".to_string()],
                resume_from_committer_time: None,
                hide_ancestor_sha: None,
            },
            vec![trace(
                "checkpoint_missing",
                "no published checkpoint exists for this repository".to_string(),
            )],
        );
    };

    let Some(published_state) = checkpoint.repository_state.as_ref() else {
        return (
            CheckpointAssessment {
                present: true,
                trust_class: CheckpointTrustClass::Invalidated,
                invalidation_reasons: vec!["missing_repository_state_snapshot".to_string()],
                resume_from_committer_time: None,
                hide_ancestor_sha: None,
            },
            vec![trace(
                "checkpoint_state_snapshot_missing",
                "checkpoint exists but does not contain repository-state metadata".to_string(),
            )],
        );
    };

    let live_state = checkpoint_repository_state(repository_identity, repository_capabilities);
    let invalidation_reasons =
        collect_checkpoint_invalidation_reasons(&live_state, published_state);
    if invalidation_reasons.is_empty() {
        return (
            CheckpointAssessment {
                present: true,
                trust_class: CheckpointTrustClass::Trusted,
                invalidation_reasons,
                resume_from_committer_time: Some(checkpoint.last_committer_time),
                hide_ancestor_sha: Some(checkpoint.last_commit_sha.clone()),
            },
            vec![trace(
                "checkpoint_state_trusted",
                "checkpoint repository-state snapshot matches current repository state".to_string(),
            )],
        );
    }

    let mut trace_entries = Vec::with_capacity(invalidation_reasons.len());
    for reason in &invalidation_reasons {
        trace_entries.push(trace(
            "checkpoint_invalidated",
            format!("checkpoint invalidated due to {reason}"),
        ));
    }
    (
        CheckpointAssessment {
            present: true,
            trust_class: CheckpointTrustClass::Invalidated,
            invalidation_reasons,
            resume_from_committer_time: None,
            hide_ancestor_sha: None,
        },
        trace_entries,
    )
}

fn checkpoint_repository_state(
    repository_identity: &RepositoryIdentity,
    repository_capabilities: &RepositoryCapabilities,
) -> CheckpointRepositoryState {
    CheckpointRepositoryState {
        identity_fingerprint: repository_identity.identity_fingerprint.clone(),
        topology: repository_identity.topology,
        is_bare: repository_identity.is_bare,
        is_shallow: repository_identity.is_shallow,
        object_hash: repository_capabilities.object_hash.clone(),
        ref_storage: repository_capabilities.ref_storage,
        has_sparse_checkout: repository_capabilities.has_sparse_checkout,
        has_partial_clone: repository_capabilities.has_partial_clone,
    }
}

fn collect_checkpoint_invalidation_reasons(
    live_state: &CheckpointRepositoryState,
    published_state: &CheckpointRepositoryState,
) -> Vec<String> {
    let mut reasons = Vec::new();
    if live_state.identity_fingerprint != published_state.identity_fingerprint {
        reasons.push("repository_identity_changed".to_string());
    }
    if live_state.topology != published_state.topology {
        reasons.push("repository_topology_changed".to_string());
    }
    if live_state.is_bare != published_state.is_bare {
        reasons.push("repository_bare_state_changed".to_string());
    }
    if live_state.is_shallow != published_state.is_shallow {
        reasons.push("repository_shallow_state_changed".to_string());
    }
    if live_state.object_hash != published_state.object_hash {
        reasons.push("repository_object_hash_changed".to_string());
    }
    if live_state.ref_storage != published_state.ref_storage {
        reasons.push("repository_ref_storage_changed".to_string());
    }
    if live_state.has_sparse_checkout != published_state.has_sparse_checkout {
        reasons.push("repository_sparse_checkout_state_changed".to_string());
    }
    if live_state.has_partial_clone != published_state.has_partial_clone {
        reasons.push("repository_partial_clone_state_changed".to_string());
    }
    reasons
}

fn detect_partial_clone(object_dir: &Path) -> bool {
    let pack_dir = object_dir.join("pack");
    fs::read_dir(pack_dir)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .any(|entry| {
            entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| extension.eq_ignore_ascii_case("promisor"))
                .unwrap_or(false)
        })
}

fn detect_changed_path_bloom_filters(object_dir: &Path) -> bool {
    detect_commit_graph_chunk_markers(object_dir, &[b"BIDX", b"BDAT"])
}

fn detect_commit_graph_chunk_markers(object_dir: &Path, markers: &[&[u8; 4]]) -> bool {
    commit_graph_candidate_paths(object_dir)
        .into_iter()
        .filter_map(|path| fs::read(path).ok())
        .any(|bytes| {
            markers.iter().all(|marker| {
                bytes
                    .windows(marker.len())
                    .any(|window| window == marker.as_slice())
            })
        })
}

fn commit_graph_candidate_paths(object_dir: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let direct = object_dir.join("info").join("commit-graph");
    if direct.exists() {
        candidates.push(direct);
    }

    let layered_dir = object_dir.join("info").join("commit-graphs");
    if let Ok(entries) = fs::read_dir(layered_dir) {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| extension.eq_ignore_ascii_case("graph"))
                .unwrap_or(false)
            {
                candidates.push(path);
            }
        }
    }

    candidates
}

fn detect_split_index(git_dir: &Path) -> bool {
    fs::read_dir(git_dir)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .any(|entry| {
            entry
                .file_name()
                .to_str()
                .map(|name| name.starts_with("sharedindex."))
                .unwrap_or(false)
        })
}

fn detect_repository_maintenance_signals(
    common_dir: &Path,
    object_dir: &Path,
    has_commit_graph: bool,
    has_multi_pack_index: bool,
) -> RepositoryMaintenanceSignals {
    let newest_pack_epoch = newest_pack_metadata_epoch(object_dir);
    let commit_graph_epoch = has_commit_graph
        .then(|| newest_commit_graph_epoch(object_dir))
        .flatten();
    let multi_pack_index_epoch = has_multi_pack_index
        .then(|| file_modified_epoch(&object_dir.join("pack").join("multi-pack-index")))
        .flatten();

    RepositoryMaintenanceSignals {
        gc_log_present: common_dir.join("gc.log").exists(),
        commit_graph_behind_packfiles: newest_pack_epoch
            .zip(commit_graph_epoch)
            .map(|(pack_epoch, graph_epoch)| pack_epoch > graph_epoch)
            .unwrap_or(false),
        multi_pack_index_behind_packfiles: newest_pack_epoch
            .zip(multi_pack_index_epoch)
            .map(|(pack_epoch, index_epoch)| pack_epoch > index_epoch)
            .unwrap_or(false),
    }
}

fn newest_commit_graph_epoch(object_dir: &Path) -> Option<i64> {
    commit_graph_candidate_paths(object_dir)
        .into_iter()
        .filter_map(|path| file_modified_epoch(&path))
        .max()
}

fn newest_pack_metadata_epoch(object_dir: &Path) -> Option<i64> {
    fs::read_dir(object_dir.join("pack"))
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .filter_map(|entry| {
            let path = entry.path();
            let extension = path.extension().and_then(|value| value.to_str())?;
            if extension.eq_ignore_ascii_case("pack") || extension.eq_ignore_ascii_case("idx") {
                file_modified_epoch(&path)
            } else {
                None
            }
        })
        .max()
}

fn file_modified_epoch(path: &Path) -> Option<i64> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let since_epoch = modified.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(since_epoch.as_secs()).ok()
}

fn resolve_acceleration_readiness(
    has_commit_graph: bool,
    has_changed_path_bloom_filters: bool,
    has_multi_pack_index: bool,
) -> AccelerationReadiness {
    match (
        has_commit_graph,
        has_changed_path_bloom_filters,
        has_multi_pack_index,
    ) {
        (true, true, true) => AccelerationReadiness::Ready,
        (true, _, _) | (_, _, true) => AccelerationReadiness::Partial,
        (false, false, false) => AccelerationReadiness::Unavailable,
        (false, true, false) => AccelerationReadiness::Partial,
    }
}

fn determine_degraded_mode(
    repository_capabilities: &RepositoryCapabilities,
    requested_mode: IndexingMode,
    effective_mode: IndexingMode,
    checkpoint_assessment: &CheckpointAssessment,
) -> DegradedModeDecision {
    if requested_mode != effective_mode {
        return match (requested_mode, effective_mode) {
            (IndexingMode::Incremental, IndexingMode::Full) => DegradedModeDecision {
                policy: DegradedModePolicy::FallbackToFull,
                reasons: checkpoint_assessment.invalidation_reasons.clone(),
                disabled_features: vec![PlannerFeatureGate::IncrementalResume],
            },
            _ => DegradedModeDecision {
                policy: DegradedModePolicy::ScanOnly,
                reasons: checkpoint_assessment.invalidation_reasons.clone(),
                disabled_features: vec![PlannerFeatureGate::IncrementalResume],
            },
        };
    }

    let mut reasons = Vec::new();
    let mut disabled_features = Vec::new();
    if !repository_capabilities.has_commit_graph {
        reasons.push("commit_graph_unavailable".to_string());
        disabled_features.push(PlannerFeatureGate::CommitGraphAcceleration);
    }
    if !repository_capabilities.has_changed_path_bloom_filters {
        reasons.push("changed_path_bloom_filters_unavailable".to_string());
        disabled_features.push(PlannerFeatureGate::ChangedPathBloomFilters);
    }
    if !repository_capabilities.has_multi_pack_index {
        reasons.push("multi_pack_index_unavailable".to_string());
        disabled_features.push(PlannerFeatureGate::MultiPackIndexAcceleration);
    }
    if repository_capabilities.has_sparse_checkout {
        reasons.push("sparse_checkout_active".to_string());
        disabled_features.push(PlannerFeatureGate::OptimizedObjectStoreAccess);
    }
    if repository_capabilities.has_partial_clone {
        reasons.push("partial_clone_active".to_string());
        disabled_features.push(PlannerFeatureGate::OptimizedObjectStoreAccess);
    }
    if repository_capabilities.has_alternates {
        reasons.push("alternate_object_store_active".to_string());
        disabled_features.push(PlannerFeatureGate::OptimizedObjectStoreAccess);
    }
    if repository_capabilities.has_split_index {
        reasons.push("split_index_active".to_string());
        disabled_features.push(PlannerFeatureGate::OptimizedObjectStoreAccess);
    }

    dedupe_feature_gates(&mut disabled_features);

    if repository_capabilities.has_sparse_checkout
        || repository_capabilities.has_partial_clone
        || repository_capabilities.has_alternates
        || repository_capabilities.has_split_index
    {
        return DegradedModeDecision {
            policy: DegradedModePolicy::LimitedCapabilities,
            reasons,
            disabled_features,
        };
    }

    if !disabled_features.is_empty() {
        return DegradedModeDecision {
            policy: DegradedModePolicy::DisableAcceleration,
            reasons,
            disabled_features,
        };
    }

    DegradedModeDecision {
        policy: DegradedModePolicy::Nominal,
        reasons: Vec::new(),
        disabled_features: Vec::new(),
    }
}

fn determine_planner_health(
    repository_capabilities: &RepositoryCapabilities,
    maintenance_signals: &RepositoryMaintenanceSignals,
    requested_mode: IndexingMode,
    effective_mode: IndexingMode,
    checkpoint_assessment: &CheckpointAssessment,
    degraded_mode: &DegradedModeDecision,
) -> PlannerHealth {
    let mut findings = Vec::new();
    let mut recommended_actions = Vec::new();
    let mut maintenance_freshness = MaintenanceFreshness::Unknown;

    if maintenance_signals.gc_log_present {
        findings.push("gc_log_present".to_string());
        recommended_actions.push(
            "inspect gc.log and run `git maintenance run --task=gc` once repository maintenance is healthy"
                .to_string(),
        );
        maintenance_freshness = MaintenanceFreshness::Stale;
    }

    if maintenance_signals.commit_graph_behind_packfiles {
        findings.push("commit_graph_behind_packfiles".to_string());
        recommended_actions.push(
            "refresh commit-graph metadata with `git maintenance run --task=commit-graph`"
                .to_string(),
        );
        maintenance_freshness = MaintenanceFreshness::Stale;
    }

    if maintenance_signals.multi_pack_index_behind_packfiles {
        findings.push("multi_pack_index_behind_packfiles".to_string());
        recommended_actions.push(
            "refresh multi-pack-index metadata with `git multi-pack-index write` or `git maintenance run --task=incremental-repack`"
                .to_string(),
        );
        maintenance_freshness = MaintenanceFreshness::Stale;
    }

    if maintenance_freshness == MaintenanceFreshness::Unknown
        && (repository_capabilities.has_commit_graph
            || repository_capabilities.has_multi_pack_index)
    {
        maintenance_freshness = MaintenanceFreshness::Fresh;
    }

    if requested_mode == IndexingMode::Incremental
        && effective_mode == IndexingMode::Full
        && !checkpoint_assessment.invalidation_reasons.is_empty()
    {
        findings.extend(checkpoint_assessment.invalidation_reasons.clone());
        recommended_actions.push(
            "treat the next run as a full publish barrier before trusting incremental resume again"
                .to_string(),
        );
    }

    if repository_capabilities.has_partial_clone {
        findings.push("partial_clone_limits_object_completeness".to_string());
        recommended_actions.push(
            "prefetch promisor objects before large indexing sessions when possible".to_string(),
        );
    }
    if repository_capabilities.has_sparse_checkout {
        findings.push("sparse_checkout_limits_workspace_visibility".to_string());
        recommended_actions.push(
            "verify sparse-checkout patterns before relying on repo-wide planner assumptions"
                .to_string(),
        );
    }
    if !repository_capabilities.has_commit_graph {
        recommended_actions
            .push("enable commit-graph maintenance to unlock faster commit planning".to_string());
    }
    if !repository_capabilities.has_changed_path_bloom_filters {
        recommended_actions.push(
            "write commit-graph with changed-path Bloom filters to improve path-history planning"
                .to_string(),
        );
    }
    if !repository_capabilities.has_multi_pack_index {
        recommended_actions.push(
            "consider multi-pack-index maintenance when pack fragmentation starts to grow"
                .to_string(),
        );
    }

    dedupe_strings(&mut findings);
    dedupe_strings(&mut recommended_actions);

    let risk_level = match degraded_mode.policy {
        DegradedModePolicy::FallbackToFull | DegradedModePolicy::ScanOnly => {
            PlannerRiskLevel::Critical
        }
        DegradedModePolicy::LimitedCapabilities => PlannerRiskLevel::Elevated,
        DegradedModePolicy::DisableAcceleration => {
            if maintenance_freshness == MaintenanceFreshness::Stale {
                PlannerRiskLevel::Elevated
            } else {
                PlannerRiskLevel::Advisory
            }
        }
        DegradedModePolicy::Nominal => {
            if maintenance_freshness == MaintenanceFreshness::Stale {
                PlannerRiskLevel::Advisory
            } else {
                PlannerRiskLevel::Nominal
            }
        }
    };

    PlannerHealth {
        maintenance_freshness,
        risk_level,
        findings,
        recommended_actions,
    }
}

fn dedupe_feature_gates(disabled_features: &mut Vec<PlannerFeatureGate>) {
    let mut deduped = Vec::with_capacity(disabled_features.len());
    for feature in disabled_features.iter().copied() {
        if !deduped.contains(&feature) {
            deduped.push(feature);
        }
    }
    *disabled_features = deduped;
}

fn dedupe_strings(values: &mut Vec<String>) {
    let mut deduped = Vec::with_capacity(values.len());
    for value in values.iter() {
        if !deduped.contains(value) {
            deduped.push(value.clone());
        }
    }
    *values = deduped;
}

fn trace(code: &str, message: String) -> PlannerTraceEntry {
    PlannerTraceEntry {
        code: code.to_string(),
        message,
    }
}

fn topology_label(topology: RepositoryTopology) -> &'static str {
    topology.as_str()
}

fn mode_label(mode: IndexingMode) -> &'static str {
    match mode {
        IndexingMode::Full => "full",
        IndexingMode::Incremental => "incremental",
    }
}

fn degraded_mode_label(policy: DegradedModePolicy) -> &'static str {
    policy.as_str()
}

#[cfg(test)]
mod tests {
    use crate::domain::models::CheckpointTrustClass;

    use super::assess_checkpoint;
    use super::determine_degraded_mode;
    use super::determine_planner_health;
    use super::resolve_acceleration_readiness;
    use super::RepositoryMaintenanceSignals;
    use crate::domain::models::{
        AccelerationReadiness, CheckpointAssessment, CheckpointRepositoryState, DegradedModePolicy,
        IndexCheckpoint, IndexingMode, MaintenanceFreshness, PlannerFeatureGate, PlannerRiskLevel,
        RepositoryCapabilities, RepositoryIdentity, RepositoryRefStorage, RepositoryTopology,
    };

    fn sample_identity() -> RepositoryIdentity {
        RepositoryIdentity {
            path: "/tmp/repo".to_string(),
            git_dir: "/tmp/repo/.git".to_string(),
            git_common_dir: "/tmp/repo/.git".to_string(),
            object_dir: "/tmp/repo/.git/objects".to_string(),
            name: "repo".to_string(),
            topology: RepositoryTopology::Common,
            is_bare: false,
            is_shallow: false,
            superproject_root: None,
            identity_fingerprint: "/tmp/repo/.git::/tmp/repo".to_string(),
        }
    }

    fn sample_capabilities() -> RepositoryCapabilities {
        RepositoryCapabilities {
            object_hash: "sha1".to_string(),
            ref_storage: RepositoryRefStorage::Files,
            has_commit_graph: true,
            has_changed_path_bloom_filters: true,
            has_multi_pack_index: true,
            has_sparse_checkout: false,
            has_partial_clone: false,
            has_alternates: false,
            has_split_index: false,
            has_worktree_config: false,
            acceleration_readiness: AccelerationReadiness::Ready,
        }
    }

    fn sample_checkpoint_state() -> CheckpointRepositoryState {
        CheckpointRepositoryState {
            identity_fingerprint: "/tmp/repo/.git::/tmp/repo".to_string(),
            topology: RepositoryTopology::Common,
            is_bare: false,
            is_shallow: false,
            object_hash: "sha1".to_string(),
            ref_storage: RepositoryRefStorage::Files,
            has_sparse_checkout: false,
            has_partial_clone: false,
        }
    }

    #[test]
    fn incremental_without_checkpoint_falls_back_to_full() {
        let identity = sample_identity();
        let capabilities = sample_capabilities();
        let (effective_mode, checkpoint, trace) =
            assess_checkpoint(IndexingMode::Incremental, &identity, &capabilities, None);

        assert_eq!(effective_mode, IndexingMode::Full);
        assert_eq!(checkpoint.trust_class, CheckpointTrustClass::Missing);
        assert_eq!(checkpoint.invalidation_reasons, vec!["missing_checkpoint"]);
        assert_eq!(trace.len(), 2);
    }

    #[test]
    fn incremental_with_checkpoint_stays_incremental() {
        let identity = sample_identity();
        let capabilities = sample_capabilities();
        let checkpoint = IndexCheckpoint {
            last_committer_time: 42,
            last_commit_sha: "abc123".to_string(),
            indexed_at_epoch: 100,
            repository_state: Some(sample_checkpoint_state()),
        };

        let (effective_mode, assessment, _) = assess_checkpoint(
            IndexingMode::Incremental,
            &identity,
            &capabilities,
            Some(&checkpoint),
        );

        assert_eq!(effective_mode, IndexingMode::Incremental);
        assert_eq!(assessment.trust_class, CheckpointTrustClass::Trusted);
        assert_eq!(assessment.resume_from_committer_time, Some(42));
        assert_eq!(assessment.hide_ancestor_sha.as_deref(), Some("abc123"));
    }

    #[test]
    fn incremental_with_repository_state_drift_falls_back_to_full() {
        let mut identity = sample_identity();
        identity.is_shallow = true;
        let capabilities = sample_capabilities();
        let checkpoint = IndexCheckpoint {
            last_committer_time: 42,
            last_commit_sha: "abc123".to_string(),
            indexed_at_epoch: 100,
            repository_state: Some(sample_checkpoint_state()),
        };

        let (effective_mode, assessment, _) = assess_checkpoint(
            IndexingMode::Incremental,
            &identity,
            &capabilities,
            Some(&checkpoint),
        );

        assert_eq!(effective_mode, IndexingMode::Full);
        assert_eq!(assessment.trust_class, CheckpointTrustClass::Invalidated);
        assert_eq!(
            assessment.invalidation_reasons,
            vec!["repository_shallow_state_changed"]
        );
    }

    #[test]
    fn incremental_with_checkpoint_missing_state_snapshot_falls_back_to_full() {
        let identity = sample_identity();
        let capabilities = sample_capabilities();
        let checkpoint = IndexCheckpoint {
            last_committer_time: 42,
            last_commit_sha: "abc123".to_string(),
            indexed_at_epoch: 100,
            repository_state: None,
        };

        let (effective_mode, assessment, _) = assess_checkpoint(
            IndexingMode::Incremental,
            &identity,
            &capabilities,
            Some(&checkpoint),
        );

        assert_eq!(effective_mode, IndexingMode::Full);
        assert_eq!(assessment.trust_class, CheckpointTrustClass::Invalidated);
        assert_eq!(
            assessment.invalidation_reasons,
            vec!["missing_repository_state_snapshot"]
        );
    }

    #[test]
    fn acceleration_readiness_tracks_available_metadata() {
        assert_eq!(
            resolve_acceleration_readiness(false, false, false),
            AccelerationReadiness::Unavailable
        );
        assert_eq!(
            resolve_acceleration_readiness(true, false, false),
            AccelerationReadiness::Partial
        );
        assert_eq!(
            resolve_acceleration_readiness(true, true, true),
            AccelerationReadiness::Ready
        );
    }

    #[test]
    fn degraded_mode_is_nominal_when_requested_and_effective_modes_match() {
        let decision = determine_degraded_mode(
            &sample_capabilities(),
            IndexingMode::Full,
            IndexingMode::Full,
            &CheckpointAssessment {
                present: false,
                trust_class: CheckpointTrustClass::Missing,
                invalidation_reasons: Vec::new(),
                resume_from_committer_time: None,
                hide_ancestor_sha: None,
            },
        );

        assert_eq!(decision.policy, DegradedModePolicy::Nominal);
        assert!(decision.reasons.is_empty());
        assert!(decision.disabled_features.is_empty());
    }

    #[test]
    fn degraded_mode_fallback_to_full_preserves_reasons() {
        let decision = determine_degraded_mode(
            &sample_capabilities(),
            IndexingMode::Incremental,
            IndexingMode::Full,
            &CheckpointAssessment {
                present: true,
                trust_class: CheckpointTrustClass::Invalidated,
                invalidation_reasons: vec!["repository_shallow_state_changed".to_string()],
                resume_from_committer_time: None,
                hide_ancestor_sha: None,
            },
        );

        assert_eq!(decision.policy, DegradedModePolicy::FallbackToFull);
        assert_eq!(decision.reasons, vec!["repository_shallow_state_changed"]);
        assert_eq!(
            decision.disabled_features,
            vec![PlannerFeatureGate::IncrementalResume]
        );
    }

    #[test]
    fn degraded_mode_can_disable_acceleration_without_fallback() {
        let mut capabilities = sample_capabilities();
        capabilities.has_multi_pack_index = false;
        capabilities.acceleration_readiness = AccelerationReadiness::Partial;

        let decision = determine_degraded_mode(
            &capabilities,
            IndexingMode::Full,
            IndexingMode::Full,
            &CheckpointAssessment {
                present: false,
                trust_class: CheckpointTrustClass::Missing,
                invalidation_reasons: Vec::new(),
                resume_from_committer_time: None,
                hide_ancestor_sha: None,
            },
        );

        assert_eq!(decision.policy, DegradedModePolicy::DisableAcceleration);
        assert_eq!(decision.reasons, vec!["multi_pack_index_unavailable"]);
        assert_eq!(
            decision.disabled_features,
            vec![PlannerFeatureGate::MultiPackIndexAcceleration]
        );
    }

    #[test]
    fn degraded_mode_can_mark_limited_capabilities() {
        let mut capabilities = sample_capabilities();
        capabilities.has_sparse_checkout = true;

        let decision = determine_degraded_mode(
            &capabilities,
            IndexingMode::Full,
            IndexingMode::Full,
            &CheckpointAssessment {
                present: false,
                trust_class: CheckpointTrustClass::Missing,
                invalidation_reasons: Vec::new(),
                resume_from_committer_time: None,
                hide_ancestor_sha: None,
            },
        );

        assert_eq!(decision.policy, DegradedModePolicy::LimitedCapabilities);
        assert_eq!(decision.reasons, vec!["sparse_checkout_active"]);
        assert_eq!(
            decision.disabled_features,
            vec![PlannerFeatureGate::OptimizedObjectStoreAccess]
        );
    }

    #[test]
    fn planner_health_marks_fallback_runs_critical() {
        let checkpoint = CheckpointAssessment {
            present: false,
            trust_class: CheckpointTrustClass::Missing,
            invalidation_reasons: vec!["missing_checkpoint".to_string()],
            resume_from_committer_time: None,
            hide_ancestor_sha: None,
        };
        let degraded = determine_degraded_mode(
            &sample_capabilities(),
            IndexingMode::Incremental,
            IndexingMode::Full,
            &checkpoint,
        );
        let health = determine_planner_health(
            &sample_capabilities(),
            &RepositoryMaintenanceSignals::default(),
            IndexingMode::Incremental,
            IndexingMode::Full,
            &checkpoint,
            &degraded,
        );

        assert_eq!(health.risk_level, PlannerRiskLevel::Critical);
        assert!(health.findings.contains(&"missing_checkpoint".to_string()));
        assert!(health
            .recommended_actions
            .iter()
            .any(|value| value.contains("full publish barrier")));
    }

    #[test]
    fn planner_health_marks_stale_metadata_advisory() {
        let checkpoint = CheckpointAssessment {
            present: true,
            trust_class: CheckpointTrustClass::Trusted,
            invalidation_reasons: Vec::new(),
            resume_from_committer_time: None,
            hide_ancestor_sha: None,
        };
        let degraded = determine_degraded_mode(
            &sample_capabilities(),
            IndexingMode::Full,
            IndexingMode::Full,
            &checkpoint,
        );
        let health = determine_planner_health(
            &sample_capabilities(),
            &RepositoryMaintenanceSignals {
                gc_log_present: true,
                commit_graph_behind_packfiles: true,
                multi_pack_index_behind_packfiles: false,
            },
            IndexingMode::Full,
            IndexingMode::Full,
            &checkpoint,
            &degraded,
        );

        assert_eq!(health.maintenance_freshness, MaintenanceFreshness::Stale);
        assert_eq!(health.risk_level, PlannerRiskLevel::Advisory);
        assert!(health.findings.contains(&"gc_log_present".to_string()));
        assert!(health
            .recommended_actions
            .iter()
            .any(|value| value.contains("commit-graph")));
    }
}
