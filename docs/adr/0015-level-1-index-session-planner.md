# ADR 0015: Level 1 Index Session Planner

Date: 2026-03-20
Status: Accepted

## Context

1. Repository intake, checkpoint lookup, and indexing-mode selection were previously spread across `adapters/git.rs`, `adapters/db.rs`, and `application/indexer.rs`.
2. That scattered design made it easy to silently fall back from incremental to full indexing, hard to reason about repository identity, and difficult to explain degraded behavior.
3. Level 1 research concluded that this stage should be a control plane that resolves repository identity, detects repository capabilities, assesses checkpoint trust, and emits one explicit execution plan before Level 2 ingest begins.

## Decision

1. Introduce an application-layer Level 1 planner in `application/index_session_planner.rs`.
2. Represent Level 1 outputs with typed domain contracts:
   - `RepositoryIdentity`
   - `RepositoryCapabilities`
   - `CheckpointAssessment`
   - `DegradedModeDecision`
   - `IndexSessionPlan`
   - `PlannerTraceEntry`
3. Route `Indexer::scan_repository()` and `Indexer::index_repository()` through Level 1 planning instead of recomputing intake logic ad hoc.
4. Keep Git repository discovery in `adapters` and keep execution-policy selection in `application`.
5. Make requested mode and effective mode explicit so incremental fallback behavior is traceable instead of silent.
6. Treat degraded execution as a first-class planner outcome, not an implicit side effect of checkpoint invalidation.
7. Return the selected `IndexSessionPlan` with indexing results so downstream callers can observe the planner decision directly.
8. Persist planner sessions durably in legacy_relational_engine, including reason ledgers, disabled feature gates, and ordered trace entries.
9. Return planner-backed scan responses so repository intake and indexing expose the same Level 1 planning surface.
10. Classify planner health as part of Level 1, including:
   - planner risk level
   - maintenance freshness
   - operator-facing findings
   - recommended remediation actions
11. Expose read-side planner inspection APIs so the latest or recent Level 1 decisions can be queried without rerunning the planner.

## Consequences

1. Repository intake now has one explicit boundary and one main planning output.
2. Later ingest stages can consume stable session facts instead of rediscovering topology or checkpoint state on demand.
3. Checkpoint invalidation reasons remain useful diagnostics, but they are now paired with an explicit degraded-mode policy (`nominal`, `fallback_to_full`, `disable_acceleration`, `limited_capabilities`, future `scan_only`).
4. Planner decisions are now auditable after the fact through durable planner-session tables instead of only being visible during the current process lifetime.
5. The current planner is intentionally conservative:
   - checkpoint trust is stronger than before but still limited to currently persisted repo-state signals
   - capability detection now includes commit-graph Bloom filters, alternates, split-index, and worktree-config signals, but can still expand further
   - degraded-state policy will continue to expand in follow-up work
6. Level 1 now has a battle-hardened operator story:
   - stale maintenance metadata is surfaced explicitly
   - recovery hints are concrete instead of generic
   - planner history can be inspected post-run
7. Architecture complexity increases slightly in exchange for a much clearer and more observable Level 1 contract.

## Alternatives Considered

1. Keep planning logic inside `Indexer`.
   - rejected: continues the current coupling and hides fallback behavior.
2. Push planning entirely into adapters.
   - rejected: execution-policy selection belongs in `application`, not in Git or DB adapters.
3. Add a generic abstraction layer before defining Level 1 contracts.
   - rejected: this would create indirection before the real stage boundary was stabilized.

## References

1. [level-1-optimization-research-tracker-2026-03-20.md](/d:/dev/finalProject/docs/level-1-optimization-research-tracker-2026-03-20.md)
2. https://git-scm.com/docs/gitrepository-layout/2.49.0.html
3. https://git-scm.com/docs/git-worktree.html
4. https://git-scm.com/docs/git-commit-graph/2.46.0.html
5. https://git-scm.com/docs/git-gc
6. https://git-scm.com/docs/git-maintenance
7. https://www.postgres.org/docs/current/auto-explain.html


