# Level 1 Optimization Research Tracker

Date: 2026-03-20  
Last Updated: 2026-03-20  
Scope: Level 1 only (`repository intake`, `repository identity`, `capability detection`, `index session planning`)  
Mode: Research complete, implementation checkpoint in progress

## Mission

Turn Level 1 from a path-validation prelude into a research-grade control plane that decides:

1. what repository we are really indexing
2. what capabilities and degraded states it has
3. what the effective indexing plan should be
4. why that plan was chosen

## Current Rule

Do not implement Level 1 changes until:

1. all five research iterations are complete
2. the target Level 1 architecture is synthesized
3. invariants, risks, and checkpoints are frozen

## Global Status Board

| Iteration | Focus | Status | % Complete | Main Question | Output |
|---|---|---:|---:|---|---|
| 1 | Git repository identity and capability signals | Complete | 100 | What is a repository, really, in Git internals? | Identity/capability findings and source pack |
| 2 | Session planning and control-plane patterns | Complete | 100 | How should Level 1 choose a physical plan from repo facts? | Planning-pattern evaluation |
| 3 | Failure semantics, checkpoints, resumability | Complete | 100 | How should Level 1 classify fallback and recovery behavior? | Failure-state and checkpoint model |
| 4 | High-performance local metadata strategies | Complete | 100 | What Level 1 metadata should be cached or materialized early? | Metadata/cache strategy evaluation |
| 5 | Final synthesis | Complete | 100 | What is the exact target architecture and readiness checklist? | Complete Level 1 design package |

## Checkpoint Ledger

| Checkpoint | Date | Status | Evidence | Notes |
|---|---|---|---|---|
| Tracker created | 2026-03-20 | Complete | This document | Research-only program for Level 1 initialized |
| Iteration 1 finished | 2026-03-20 | Complete | Research notes in session + source pack below | Repo identity must be capability-based, not path-based |
| Iteration 2 finished | 2026-03-20 | Complete | Research notes in session + source pack below | Level 1 should use logical intent + physical plan + explicit enforcers |
| Iteration 3 finished | 2026-03-20 | Complete | Research notes in session + source pack below | Checkpoints require trust classes and invalidation rules |
| Iteration 4 finished | 2026-03-20 | Complete | Research notes in session + source pack below | Level 1 should split eager facts from lazy facts and cache only derived metadata |
| Iteration 5 finished | 2026-03-20 | Complete | This document | Research package frozen; implementation may begin in a later turn |
| Implementation checkpoint 1 | 2026-03-20 | Complete | `guardrails` + `dev:gate:fast` green after planner integration | Level 1 planner wired into scan/index entry and branch compile drift repaired |
| Implementation checkpoint 2 | 2026-03-20 | Complete | `guardrails` + `dev:gate:fast` + `dev:gate` green after checkpoint-state hardening | Incremental resume is now invalidated when persisted repository state drifts |
| Implementation checkpoint 3 | 2026-03-20 | Complete | `guardrails` + `dev:gate:fast` + `dev:gate` green after degraded-mode promotion | Degraded execution is now an explicit planner decision carried in indexing results |
| Implementation checkpoint 4 | 2026-03-20 | Complete | `guardrails` + `dev:gate:fast` + `dev:gate` green after planner-session persistence and scan API upgrade | Planner traces are durable, capability detection is broader, and scan now returns the session plan directly |
| Implementation checkpoint 5 | 2026-03-20 | Complete | `guardrails` + `dev:gate:fast` + `dev:gate` green after planner-health + read-side hardening | Planner sessions now expose risk, maintenance freshness, durable history, and operator-facing diagnostics |

## Implementation Checkpoint 1

The first Level 1 implementation checkpoint is now complete.

Delivered in code:

1. `RepositoryIdentity`, `RepositoryCapabilities`, `CheckpointAssessment`, `IndexSessionPlan`, and `PlannerTraceEntry` were added as typed domain contracts.
2. `application/index_session_planner.rs` now resolves canonical repository context and selects the initial session plan.
3. `Indexer::scan_repository()` and `Indexer::index_repository()` now enter through the planner.
4. Bare-repository intake no longer collapses into the generic "not a repo" path.
5. Supporting compile-surface drift in ingest telemetry and perf-report contracts was repaired so the branch can move forward from a green fast gate again.

Still intentionally deferred:

1. loading/querying persisted planner sessions through a first-class read API
2. finer-grained scan-only or limited-write policy beyond current fallback/limited-capability decisions
3. optional persistence of richer capability snapshots into checkpoint trust state beyond the current correctness-critical fields

## Implementation Checkpoint 2

The second Level 1 implementation checkpoint is now complete.

Delivered in code:

1. checkpoint rows now persist a repository-state snapshot at publish time
2. the planner compares live repo state against the last published snapshot before allowing incremental resume
3. incremental mode now falls back to full when identity, topology, bare/shallow state, object hash, ref storage, sparse-checkout state, or partial-clone state drifted
4. legacy checkpoints without repository-state metadata are treated as invalid for incremental resume
5. DB and planner tests cover roundtrip, legacy-row handling, and state-drift invalidation

## Implementation Checkpoint 3

The third Level 1 implementation checkpoint is now complete.

Delivered in code:

1. degraded execution is now represented by an explicit planner contract instead of being inferred from checkpoint invalidation strings alone
2. `IndexSessionPlan` now carries a first-class degraded-mode decision
3. indexing results now include the selected session plan so the planner outcome is visible outside internal orchestration
4. planner tests now cover nominal vs fallback-to-full degraded-mode behavior

## Implementation Checkpoint 4

The fourth Level 1 implementation checkpoint is now complete.

Delivered in code:

1. planner sessions are now persisted durably in SQLite with normalized trace, reason, and disabled-feature tables
2. scan now returns a dedicated response that includes the selected session plan directly
3. planner sessions now carry a durable session id and entrypoint (`scan` or `index`)
4. capability detection now includes changed-path Bloom filters, alternates, split-index, and worktree-config signals in addition to the earlier commit-graph/MIDX/sparse/promisor checks
5. degraded-mode policy now supports `disable_acceleration` and `limited_capabilities`, not only `fallback_to_full`
6. DB and integration tests cover planner-session persistence and the upgraded scan/index contract

## Implementation Checkpoint 5

The fifth Level 1 implementation checkpoint is now complete.

Delivered in code:

1. `IndexSessionPlan` now carries first-class planner health with `risk_level`, `maintenance_freshness`, findings, and recommended actions.
2. Level 1 now classifies battle-hardening signals beyond simple capability presence:
   - `gc.log` present
   - commit-graph older than pack metadata
   - multi-pack-index older than pack metadata
3. Planner-session persistence now stores planner health durably through migration `0009_planner_session_health.sql`.
4. SQLite now exposes read-side planner history APIs:
   - latest planner session
   - recent planner sessions
   - plan signature reconstruction for quick diffing/debugging
5. Application and CLI surfaces now expose Level 1 diagnostics without requiring a new scan/index run:
   - `planner-latest`
   - `planner-history`
6. DB, planner, and integration tests now cover planner health classification plus persisted planner-session history roundtrip.

## Ten-Iteration Hardening Sweep

This hardening batch was executed as a ten-iteration upgrade of Level 1, combining research-backed decisions with implementation.

| Iteration | Type | Outcome |
|---|---|---|
| 1 | Existing foundation | Canonical planner boundary already in place |
| 2 | Existing foundation | Checkpoint trust hardened against repo-state drift |
| 3 | Existing foundation | Degraded-mode policy promoted to first-class contract |
| 4 | Existing foundation | Durable planner-session persistence and scan-plan surfacing |
| 5 | Existing foundation | Broader capability detection (Bloom filters, alternates, split-index, worktree-config) |
| 6 | New hardening | Planner health contract added (`risk_level`, `maintenance_freshness`, findings, recommendations) |
| 7 | New hardening | Maintenance freshness signals added (`gc.log`, stale commit-graph, stale MIDX) |
| 8 | New hardening | Planner-session schema extended to persist health durably |
| 9 | New hardening | Read-side history APIs added for latest and recent planner sessions |
| 10 | New hardening | CLI, test, and gate coverage added so diagnostics survive real operator use |

## Research Notes For Checkpoint 5

The new hardening work follows these source-backed patterns:

1. Git maintenance tasks are explicit and separable (`commit-graph`, `gc`, `incremental-repack`), so Level 1 should recommend concrete repair actions instead of vague "metadata stale" messages.
2. Git GC behavior records state in `gc.log`, which is a useful early warning signal that repo maintenance is blocked or failed.
3. PostgreSQL `auto_explain` demonstrates the value of persisting plan diagnostics after the fact instead of limiting them to transient logs.
4. SQLite WAL keeps write publication and checkpointing conceptually separate, which reinforces our Level 1 design of persisting planner sessions as an auditable control-plane artifact.

## Level 1 Problem Statement

Current Level 1 behavior is spread across:

1. [apps/desktop/src-tauri/src/adapters/git.rs](/d:/dev/finalProject/apps/desktop/src-tauri/src/adapters/git.rs)
2. [apps/desktop/src-tauri/src/adapters/db.rs](/d:/dev/finalProject/apps/desktop/src-tauri/src/adapters/db.rs)
3. [apps/desktop/src-tauri/src/application/indexer.rs](/d:/dev/finalProject/apps/desktop/src-tauri/src/application/indexer.rs)

This means repository intake, DB state lookup, and indexing-mode decisions are not yet represented as one explicit planning stage.

## Target Deliverable After Research

Research should end with a frozen design for:

1. `RepositoryIdentity`
2. `RepositoryCapabilities`
3. `RepositoryHealth`
4. `IndexIntent`
5. `IndexSessionPlan`
6. planner trace and degraded-reason ledger

## Iteration 1 Summary

### Question

What is the minimum correct model of repository identity and capability detection for Level 1?

### Key Findings

1. Git repository identity is not equivalent to the user-supplied path.
2. Linked worktrees, `.git` indirection, `commondir`, alternates, submodules, and bare repos must be classified at intake time.
3. `git rev-parse` exposes critical signals for Level 1 planning, including canonical git dir, common dir, shallow status, worktree status, superproject root, and object format.
4. Git acceleration metadata already exists and should be treated as Level 1 planning inputs:
   - commit-graph
   - generation numbers
   - changed-path Bloom filters
   - multi-pack-index
   - maintenance state
5. Sparse checkout, sparse index, shallow clone, and partial clone are not late-stage details; they are Level 1 capability or degradation facts.
6. Level 1 should output one immutable planning object before Level 2 commit ingestion starts.

### Main Decisions Emerging

1. Level 1 must become a control plane, not a helper function chain.
2. Repository identity must be canonical and topology-aware.
3. Capability detection should be captured once per session, then passed down as a snapshot.
4. All fallback behavior should be explicit and machine-readable.

### Open Questions Carried Into Later Iterations

1. How should the planner choose between full, incremental, degraded incremental, and forced full?
2. Which repo facts belong in durable cache vs session-only snapshot?
3. How should checkpoint validity interact with shallow/partial/sparse states?
4. What is the minimum planner trace needed for observability and perf diagnosis?

## Iteration 2 Summary

### Question

How should Level 1 transform repository facts and user intent into a stable, explainable execution plan?

### Key Findings

1. Volcano separates logical and physical algebras and explicitly models physical properties, applicability functions, cost, and enforcers. This is a strong pattern match for Level 1 planning.
2. Cascades extends this with rule guidance, tracing support, enforcers, and grouped search. This suggests our Level 1 planner should support ordered rule evaluation plus planner trace output.
3. The planning-based optimizer paper argues for goal-directed planning that intermingles planning with execution knowledge. For Level 1, this means planning should react to discovered repo facts, not just static config.
4. Sourcegraph uses a pragmatic common-case optimization strategy:
   - indexed default branch for the common case
   - fast non-indexed path for everything else
   This is evidence that Level 1 should be allowed to choose a common-case fast path plus an explicit fallback path.
5. Sourcegraph treats repository metadata as a persistent cache while keeping the code host as source of truth. For us, the Git repository is source of truth; DB-held repository facts are a cache and must be treated that way.
6. GitHub Blackbird treats commit-level consistency as a design property, not a best effort. This strongly supports making Level 1 produce a commit-consistent session barrier before any later stage starts.
7. Blackbird also optimizes ingest order using similarity and delta encoding. Inference: future Level 1 planning can legitimately choose order-aware or topology-aware execution strategies instead of naive traversal defaults.

### Main Decisions Emerging

1. Level 1 should take `IndexIntent` as logical input and produce `IndexSessionPlan` as physical output.
2. The plan should include a physical-property vector, such as:
   - repo topology
   - acceleration availability
   - checkpoint trust
   - degraded states
   - consistency barrier
   - selected execution policy
3. Level 1 should support enforcer-style fallbacks, such as:
   - `force_full_reindex`
   - `disable_incremental_hide_boundary`
   - `disable_acceleration_path`
   - `read_only_scan_only`
4. Planner output should be traced and memoized:
   - positive facts
   - negative facts
   - forced fallbacks
   - final selected plan
5. Repository cache state in SQLite should be explicitly labeled as derived metadata, not treated as canonical truth.

### New Patterns Worth Evaluating For This Repo

1. `PhysicalPropertyVector` for Level 1 planning
2. `PlanEnforcer` rules for forced fallback behavior
3. `PlannerTrace` for every session
4. `InterestingFactsMemo`:
   - facts worth remembering because they prune future plan work
   - failures worth remembering because they prevent repeated bad choices
5. `CommonCasePlan + FallbackPlan` pairing instead of one monolithic policy branch

### Open Questions Carried Forward

1. How should Level 1 represent plan cost: numeric score, rule priority, or ordered policy lattice?
2. Which planner facts should persist between sessions and which should be recomputed every run?
3. How should commit-consistent planning interact with incremental checkpoints and worktree churn?
4. How should fallback rules interact with user-requested mode vs effective mode?

## Iteration 3 Summary

### Question

How should Level 1 decide whether an existing checkpoint is trustworthy enough to resume incrementally?

### Key Findings

1. ARIES shows that recovery correctness depends on more than "there is a checkpoint"; checkpoints must identify active work, dirty state, and a safe restart point.
2. SQLite WAL provides snapshot isolation for readers, single-writer semantics, append-first commit, and checkpointing as a separate concern. This suggests Level 1 should separate:
   - plan barrier
   - write barrier
   - checkpoint publication
3. Git maintenance explicitly protects against race conditions in object-database maintenance through locking and incremental-safe update procedures. This supports using repo-state trust rules, not blind optimism.
4. Blackbird treats commit-level consistency as a core property. This reinforces that Level 1 should publish one consistent session boundary for later stages.
5. RocksDB recovery material shows the practical lesson that "persisted enough" is subtle. Unsynced metadata, WAL state, and storage-directory durability matter. Inference: Level 1 should classify checkpoint trust conservatively.

### Main Decisions Emerging

1. Replace boolean checkpoint validity with `CheckpointTrustClass`, for example:
   - `Trusted`
   - `NeedsVerification`
   - `Degraded`
   - `Unsafe`
2. Level 1 should keep a `CheckpointInvalidationReason` ledger, for example:
   - repo identity changed
   - worktree topology changed
   - shallow/partial state changed
   - object format/ref storage changed
   - prior index ended with uncertain publication
   - missing required capability for requested mode
3. The planner should distinguish:
   - `resume_from_checkpoint`
   - `resume_after_verification`
   - `force_full_reindex`
   - `scan_only_do_not_index`
4. Checkpoint publication should happen only after the session reaches a commit-consistent publish point.
5. Level 1 should model restart as "repeat from last safe published boundary", not "continue from wherever we think we stopped".

### New Patterns Worth Evaluating For This Repo

1. `CheckpointTrustClass`
2. `CheckpointInvalidationReason`
3. `PublishBarrier`
4. `ResumeToken`
5. `SafeRestartBoundary`
6. `SessionOutcome`:
   - published
   - aborted
   - degraded
   - unknown

### Open Questions Carried Forward

1. What exact repo facts should be hashed into checkpoint identity?
2. Which failures should downgrade to verification and which should force full?
3. Should Level 1 persist planner trace and session outcome separately from the main checkpoint?
4. How much checkpoint metadata belongs in SQLite vs a sidecar session journal?

## Iteration 4 Summary

### Question

What repository metadata should Level 1 compute eagerly, what should it cache, and what should it leave lazy?

### Key Findings

1. Git already ships multiple metadata acceleration structures:
   - commit-graph
   - changed-path Bloom filters
   - multi-pack-index
   - sparse index
   - split index
   - untracked cache
   These should be treated as capability signals, not recreated as bespoke metadata.
2. Sparse-index docs show Git uses guarded rollout with `command_requires_full_index` and carefully expands the sparse index for unsupported commands. This is a strong pattern for us: acceleration should be enabled only when the command path is proven safe.
3. Multi-pack-index chain design shows append-friendly metadata layering can reduce refresh cost without invalidating prior layers. This is a useful pattern for future Level 1 capability caches and repo-state journals.
4. Sourcegraph explicitly treats repository metadata as a persistent cache, not truth. This aligns with our Level 1 architecture: Git is source of truth; SQLite-held repo facts are derived and refreshable.
5. Sourcegraph also uses a smart heuristic based on past commit frequency to schedule update work. Inference: capability refresh policy does not need to be constant-frequency; it can be freshness-sensitive.
6. Blackbird optimizes metadata and ingest order using delta encoding and similarity. Inference: repo metadata can be valuable if it materially changes future work choice; otherwise it is just overhead.
7. Git sparse-checkout and sparse-index docs repeatedly show that some expensive or advanced metadata should be lazily activated only for supported operations. This supports a minimal eager-facts model.

### Main Decisions Emerging

1. Level 1 metadata should be divided into:
   - `EagerFacts`
   - `LazyFacts`
   - `DerivedCacheFacts`
2. `EagerFacts` should be cheap, deterministic, and always needed:
   - canonical repo identity
   - repo topology class
   - shallow/bare/worktree/submodule status
   - object format and ref-storage basics
   - checkpoint presence
3. `LazyFacts` should be computed only if a candidate plan needs them:
   - commit-graph verification detail
   - Bloom-filter version/usefulness
   - MIDX chain details
   - advanced maintenance freshness
   - optional worktree/sparse-index nuances
4. `DerivedCacheFacts` may persist in SQLite, but must always be versioned and invalidatable:
   - last capability snapshot
   - last planner trace summary
   - last successful publish boundary
   - capability refresh timestamps
5. Level 1 should use `AccelerationReadiness` instead of assuming acceleration is binary available/unavailable.

### New Patterns Worth Evaluating For This Repo

1. `EagerFacts`
2. `LazyFacts`
3. `DerivedCacheFacts`
4. `AccelerationReadiness`
5. `CapabilityRefreshPolicy`
6. `NeedsFact`:
   - planner requests additional facts only if a candidate plan depends on them
7. `GuardedAcceleration`:
   - enabled only when path safety is proven

### Open Questions Carried Forward

1. Which lazy facts are worth persisting after first discovery?
2. How should capability refresh TTL vary by repository churn?
3. Which metadata belongs in planner trace vs capability snapshot?
4. How should Level 1 expose readiness to Level 2 without leaking Git-internal detail into later stages?

## Decision Ledger

| Topic | Provisional Direction | Confidence | Locked |
|---|---|---:|---|
| Path-only repo identity | Reject | High | No |
| Capability snapshot at session start | Accept | High | No |
| Planner/executor split for Level 1 | Accept | High | No |
| Explicit degraded-reason ledger | Accept | High | No |
| Logical intent -> physical plan model | Accept | High | No |
| Enforcer-style fallback rules | Accept | Medium-High | No |
| Planner trace and interesting-facts memo | Accept | Medium-High | No |
| Checkpoint trust classes instead of boolean validity | Accept | High | No |
| Publish barrier before checkpoint advance | Accept | High | No |
| Eager facts vs lazy facts split | Accept | High | No |
| Guarded acceleration instead of global enablement | Accept | High | No |
| Implementation before all research rounds finish | Reject | High | Yes |

## Risks Under Study

1. Worktree and submodule identity collisions
2. Silent incremental fallback behavior
3. Commit-graph or Bloom-filter capability assumptions that are not always valid
4. Partial-clone or shallow-clone correctness drift
5. Overengineering the planner before contracts are minimal and clear

## Evidence Pack

### Git internals and capability sources

1. Git repository layout: https://git-scm.com/docs/gitrepository-layout/2.49.0.html
2. Git worktree docs: https://git-scm.com/docs/git-worktree.html
3. Git rev-parse docs: https://git-scm.com/docs/git-rev-parse/2.45.0.html
4. Git config docs: https://git-scm.com/docs/git-config.html
5. Git sparse-checkout docs: https://git-scm.com/docs/git-sparse-checkout.html
6. Git commit-graph docs: https://git-scm.com/docs/git-commit-graph/2.46.0.html
7. Commit-graph file format: https://git-scm.com/docs/commit-graph-format/2.23.0.html
8. Git maintenance docs: https://git-scm.com/docs/git-maintenance
9. Partial clone docs: https://git-scm.com/docs/partial-clone.html
10. `gix-commitgraph` docs: https://docs.rs/gix-commitgraph/latest/gix_commitgraph/

### Planning and control-plane references queued for later iterations

1. Cascades optimizer framework: https://www.sigmod.org/publications/dblp/db/journals/debu/Graefe95a.html
2. Sourcegraph architecture: https://sourcegraph.com/docs/admin/architecture
3. GitHub Blackbird architecture: https://github.blog/engineering/the-technology-behind-githubs-new-code-search/
4. Volcano optimizer generator: https://www.cs.cornell.edu/~mshong/DDG_Fall05/Volcano.pdf
5. Planning-based optimizer control: https://sigmod.org/publications/dblp/db/conf/vldb/MitchellDZ93.html
6. SQLite WAL: https://www.sqlite.org/wal.html
7. SQLite transactions: https://www.sqlite.org/lang_transaction.html
8. ARIES: https://web.stanford.edu/class/cs345d-01/rl/aries.pdf
9. RocksDB recovery notes: https://rocksdb.org/blog/2022/10/05/lost-buffered-write-recovery.html
10. RocksDB WAL notes: https://rocksdb.org/blog/2014/03/27/how-to-persist-in-memory-rocksdb-database.html
11. Multi-pack-index docs: https://git-scm.com/docs/multi-pack-index
12. Sparse-index docs: https://git-scm.com/docs/sparse-index
13. Git index format docs: https://git-scm.com/docs/gitformat-index.html
14. Sourcegraph repo update policy: https://sourcegraph.com/docs/admin/repo/update-frequency
15. Git GC docs: https://git-scm.com/docs/git-gc
16. PostgreSQL `auto_explain`: https://www.postgresql.org/docs/current/auto-explain.html

## Iteration 5 Summary

### Final Verdict

Level 1 should become a dedicated control-plane stage with one job:

1. resolve canonical repository identity
2. collect a bounded capability snapshot
3. classify checkpoint trust
4. choose an explainable physical indexing plan
5. publish a session boundary that later stages can trust

### Frozen Level 1 Target Architecture

#### Input

1. user path
2. db path
3. requested mode
4. runtime flags/env

#### Stage A: Repository Identity Resolution

Output: `RepositoryIdentity`

Required fields:

1. `worktree_root`
2. `git_dir`
3. `git_common_dir`
4. `object_dir`
5. `superproject_root`
6. `is_bare`
7. `is_worktree`
8. `is_submodule`
9. `is_shallow`
10. `object_format`
11. `ref_storage`
12. `identity_fingerprint`

#### Stage B: Capability Snapshot

Output: `RepositoryCapabilities`

Required fields:

1. `has_commit_graph`
2. `commit_graph_generation_version`
3. `changed_paths_bloom_state`
4. `has_multi_pack_index`
5. `partial_clone_state`
6. `sparse_checkout_state`
7. `sparse_index_state`
8. `split_index_state`
9. `fsmonitor_state`
10. `maintenance_state`
11. `acceleration_readiness`

#### Stage C: Checkpoint Evaluation

Output: `CheckpointAssessment`

Required fields:

1. `checkpoint_present`
2. `checkpoint_trust_class`
3. `checkpoint_identity_match`
4. `checkpoint_invalidation_reasons`
5. `safe_restart_boundary`
6. `resume_allowed`

#### Stage D: Plan Selection

Output: `IndexSessionPlan`

Required fields:

1. `logical_intent`
2. `effective_mode`
3. `consistency_barrier`
4. `selected_acceleration_path`
5. `selected_checkpoint_strategy`
6. `selected_fallbacks`
7. `eager_facts_used`
8. `lazy_facts_used`
9. `planner_trace`
10. `session_plan_version`

#### Stage E: Publishable Session Start

Output: `IndexSessionStart`

Required properties:

1. later stages receive immutable planning output
2. checkpoint does not advance until publish barrier is reached
3. later stages do not reinterpret repo facts on their own

### Frozen Design Principles

1. Git is the source of truth.
2. SQLite-held repository facts are a cache, not truth.
3. Planner output must be deterministic for the same repo state and requested mode.
4. Incremental mode is earned, not assumed.
5. Acceleration is guarded, not globally enabled.
6. Every fallback must have a machine-readable reason.
7. Every session must have a traceable plan.

## Core Invariants

1. One repository state maps to one canonical `RepositoryIdentity`.
2. `effective_mode=incremental` is illegal unless checkpoint trust and repo state both allow it.
3. No later pipeline stage may override Level 1’s repo classification silently.
4. A checkpoint is only advanced after a publish barrier.
5. Degraded repository states must be visible in planner output.
6. Capability facts must be versioned and invalidatable.
7. Derived cache facts must never be treated as authoritative over live Git facts.

## Acceptance Metrics

### Correctness

1. Same repository resolved through different user paths produces the same identity fingerprint.
2. Worktree, submodule, bare, shallow, and partial-clone classification are all test-covered.
3. Incremental fallback to full always emits at least one invalidation reason.
4. Checkpoint trust transitions are deterministic and test-covered.

### Performance

1. Level 1 eager path should stay bounded and cheap relative to full indexing.
2. No expensive lazy fact should be computed unless a candidate plan actually needs it.
3. Repeat session planning on unchanged repo state should reuse cached derived facts when valid.

### Observability

1. Every session has a `planner_trace`.
2. Every degraded plan has explicit reasons.
3. Effective mode, acceleration path, and checkpoint strategy are externally inspectable.

## Risk Register

1. Overengineering the planner before implementation slices are small enough
2. Leaking Git-internal complexity into later pipeline layers
3. Treating cached metadata as canonical truth
4. Allowing too many implicit fallback branches
5. Under-specifying checkpoint invalidation and later paying correctness debt

## Implementation Readiness Checklist

Implementation may begin only if the first slice stays within Level 1 and follows this order:

1. Define domain contracts:
   - `RepositoryIdentity`
   - `RepositoryCapabilities`
   - `CheckpointAssessment`
   - `IndexSessionPlan`
2. Add one planner entrypoint:
   - `plan_index_session(...)`
3. Move current scattered Level 1 logic behind the planner without changing later-stage behavior yet.
4. Add planner trace and degraded-reason output before optimization work.
5. Add tests for identity normalization and mode selection before any advanced acceleration logic.

## Research Program Status

The five-iteration Level 1 research program is complete.

Implementation status:

1. not started
2. intentionally blocked until a later turn

Recommended next move in the next implementation phase:

1. create Level 1 domain types
2. create planner entrypoint
3. route `scan_repository` and `index_repository` through that planner
