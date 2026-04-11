use std::path::Path;

use crate::adapters::db::{GraphStore, PlannerSessionWriteInput};
use crate::adapters::git::{validate_repository, CanonicalRepo};
use crate::domain::models::{
    IndexCheckpoint, IndexingMode, PlannerDegradedPolicy, PlannerRiskLevel,
};
use crate::support::clock::now_epoch_seconds;
use crate::support::error::AppResult;

#[derive(Debug, Clone)]
pub struct PlannerDecision {
    pub repo_id: i64,
    pub planner_session_id: u64,
    pub requested_mode: IndexingMode,
    pub effective_mode: IndexingMode,
    pub checkpoint_trusted: bool,
    pub resume_from_committer_time: Option<i64>,
    pub hide_ancestor_sha: Option<String>,
    pub risk_level: PlannerRiskLevel,
    pub degraded_policy: PlannerDegradedPolicy,
    pub trace_count: usize,
}

#[derive(Debug, Clone)]
pub struct PlannedRepositorySession {
    pub canonical_repo: CanonicalRepo,
    pub decision: PlannerDecision,
}

pub fn plan_scan_session(store: &GraphStore, path: &Path) -> AppResult<PlannedRepositorySession> {
    plan_session(store, path, IndexingMode::Full)
}

pub fn plan_index_session(
    store: &GraphStore,
    path: &Path,
    requested_mode: IndexingMode,
) -> AppResult<PlannedRepositorySession> {
    plan_session(store, path, requested_mode)
}

fn plan_session(
    store: &GraphStore,
    path: &Path,
    requested_mode: IndexingMode,
) -> AppResult<PlannedRepositorySession> {
    let canonical_repo = validate_repository(path)?;
    let repo_id = store.upsert_repository(
        &canonical_repo.root.display().to_string(),
        &canonical_repo.name,
        &canonical_repo.git_dir.display().to_string(),
    )?;
    let checkpoint = store.load_index_checkpoint(repo_id)?;
    let outcome = evaluate_outcome(requested_mode, checkpoint.as_ref());
    let planned_at_epoch = now_epoch_seconds();
    let planner_session_id = store.persist_planner_session(PlannerSessionWriteInput {
        repo_id,
        requested_mode,
        effective_mode: outcome.effective_mode,
        checkpoint_trusted: outcome.checkpoint_trusted,
        degraded_policy: outcome.degraded_policy,
        risk_level: outcome.risk_level,
        trace_count: outcome.trace_count,
        planned_at_epoch,
    })?;

    Ok(PlannedRepositorySession {
        canonical_repo,
        decision: PlannerDecision {
            repo_id,
            planner_session_id,
            requested_mode,
            effective_mode: outcome.effective_mode,
            checkpoint_trusted: outcome.checkpoint_trusted,
            resume_from_committer_time: outcome.resume_from_committer_time,
            hide_ancestor_sha: outcome.hide_ancestor_sha,
            risk_level: outcome.risk_level,
            degraded_policy: outcome.degraded_policy,
            trace_count: outcome.trace_count,
        },
    })
}

#[derive(Debug, Clone)]
struct PlannerOutcome {
    effective_mode: IndexingMode,
    checkpoint_trusted: bool,
    resume_from_committer_time: Option<i64>,
    hide_ancestor_sha: Option<String>,
    risk_level: PlannerRiskLevel,
    degraded_policy: PlannerDegradedPolicy,
    trace_count: usize,
}

fn evaluate_outcome(
    requested_mode: IndexingMode,
    checkpoint: Option<&IndexCheckpoint>,
) -> PlannerOutcome {
    let mut trace_count = 1usize; // base planner invocation event
    match requested_mode {
        IndexingMode::Full => PlannerOutcome {
            effective_mode: IndexingMode::Full,
            checkpoint_trusted: false,
            resume_from_committer_time: None,
            hide_ancestor_sha: None,
            risk_level: PlannerRiskLevel::Low,
            degraded_policy: PlannerDegradedPolicy::None,
            trace_count,
        },
        IndexingMode::Incremental => {
            if let Some(value) = checkpoint {
                trace_count = trace_count.saturating_add(1);
                PlannerOutcome {
                    effective_mode: IndexingMode::Incremental,
                    checkpoint_trusted: true,
                    resume_from_committer_time: Some(value.last_committer_time),
                    hide_ancestor_sha: Some(value.last_commit_sha.clone()),
                    risk_level: PlannerRiskLevel::Low,
                    degraded_policy: PlannerDegradedPolicy::None,
                    trace_count,
                }
            } else {
                trace_count = trace_count.saturating_add(1);
                PlannerOutcome {
                    effective_mode: IndexingMode::Full,
                    checkpoint_trusted: false,
                    resume_from_committer_time: None,
                    hide_ancestor_sha: None,
                    risk_level: PlannerRiskLevel::Medium,
                    degraded_policy: PlannerDegradedPolicy::FallbackToFull,
                    trace_count,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::evaluate_outcome;
    use crate::domain::models::{
        IndexCheckpoint, IndexingMode, PlannerDegradedPolicy, PlannerRiskLevel,
    };

    #[test]
    fn full_mode_remains_full_without_checkpoint_resume() {
        let outcome = evaluate_outcome(IndexingMode::Full, None);
        assert_eq!(outcome.effective_mode, IndexingMode::Full);
        assert!(!outcome.checkpoint_trusted);
        assert_eq!(outcome.resume_from_committer_time, None);
        assert_eq!(outcome.hide_ancestor_sha, None);
        assert_eq!(outcome.risk_level, PlannerRiskLevel::Low);
        assert_eq!(outcome.degraded_policy, PlannerDegradedPolicy::None);
    }

    #[test]
    fn incremental_mode_uses_trusted_checkpoint_when_available() {
        let checkpoint = IndexCheckpoint {
            last_committer_time: 1234,
            last_commit_sha: "abc".to_string(),
            indexed_at_epoch: 5678,
        };
        let outcome = evaluate_outcome(IndexingMode::Incremental, Some(&checkpoint));
        assert_eq!(outcome.effective_mode, IndexingMode::Incremental);
        assert!(outcome.checkpoint_trusted);
        assert_eq!(outcome.resume_from_committer_time, Some(1234));
        assert_eq!(outcome.hide_ancestor_sha.as_deref(), Some("abc"));
        assert_eq!(outcome.risk_level, PlannerRiskLevel::Low);
        assert_eq!(outcome.degraded_policy, PlannerDegradedPolicy::None);
    }

    #[test]
    fn incremental_mode_falls_back_to_full_without_checkpoint() {
        let outcome = evaluate_outcome(IndexingMode::Incremental, None);
        assert_eq!(outcome.effective_mode, IndexingMode::Full);
        assert!(!outcome.checkpoint_trusted);
        assert_eq!(outcome.resume_from_committer_time, None);
        assert_eq!(outcome.hide_ancestor_sha, None);
        assert_eq!(outcome.risk_level, PlannerRiskLevel::Medium);
        assert_eq!(
            outcome.degraded_policy,
            PlannerDegradedPolicy::FallbackToFull
        );
    }
}
