# GitGraph Lab - Industry Approach and Intent Check

Date: 2026-03-07

## Phase 0 Addendum (Engineering System)

Before feature-heavy Phase 1 work, the project now also enforces a Phase 0 baseline:

1. Workspace-level lint governance and root lockfile policy
2. Deterministic quality gate script (`scripts/quality-gate.ps1`)
3. ADR process for architectural decisions (`docs/adr`)

Reference: `docs/engineering-system.md`
From-scratch reset execution: `docs/from-scratch-system.md`

## Intent Check

Yes, using Rust is intentional, not accidental.

1. The SDD explicitly sets Rust as the core engine (`tokio`, `rayon`, `petgraph`) for heavy graph/index/simulation workloads.
2. This is the right industry choice for your product goals: deterministic performance, memory safety, and strong concurrency on 8 GB systems.
3. React/Tauri remains the UX layer, while Rust owns performance-critical logic.

## Industry-Grade Engineering Approach

1. Layered architecture:
   - `adapters`: git commands, SQLite, external APIs
   - `domain`: DAG, merge simulation, scoring engines
   - `application`: command handlers, jobs, orchestration
2. Schema discipline:
   - versioned migrations
   - backward-compatible evolution
   - repo-scoped keys to prevent cross-repo corruption
3. Reliability by default:
   - idempotent indexing
   - transactional writes
   - resumable jobs and checkpoints
4. Security controls:
   - in-process repository access via libgit2 APIs in indexing hot paths
   - path canonicalization and repo boundary checks
   - secret storage only in OS keychain
5. Observability:
   - structured logs with correlation IDs
   - job timing and failure reasons
   - p50/p95 latency and throughput counters
6. Test strategy:
   - unit tests for algorithms and parsers
   - integration tests with golden repositories
   - differential parity checks vs Git plumbing

## Phase 1 (Completed Kickoff Scope)

1. Rust backend scaffold in `apps/desktop/src-tauri`.
2. SQLite migrations pipeline with WAL profile.
3. Repository validation + commit/edge/ref indexing foundation.
4. Industry hardening added:
   - migration system supports multi-version upgrades
   - repo-scoped commit/edge keys (migration v2)
   - safer empty-repo handling for git plumbing commands

## Phase 2 (Added)

Phase 2 target: make the core operationally production-ready before advanced features.

1. Command surface and contracts
   - expose stable Tauri commands for `scan_repository`, `index_commits`, `get_commit_window`
   - add typed DTOs and consistent error codes
2. Incremental indexing engine
   - full vs incremental mode using last indexed checkpoint
   - batch insert pipeline with bounded memory
3. File change ingestion
   - populate `file_changes` from streaming libgit2 diff pipeline
   - support rename/change type classification
4. Job orchestration
   - priority queues (`interactive`, `background`, `maintenance`)
   - cancellation and timeout controls
5. Baseline DAG query APIs
   - BFS/DFS/topological traversal
   - shortest path between commits
6. Quality gates
   - unit tests for parser/indexer/query logic
   - integration test for sample branching repo
   - performance baseline reports (index speed, query p95)

### Phase 2 Kickoff Status (2026-03-07)

Implemented Slice A:

1. typed command contracts and error codes
2. incremental indexing with checkpoint persistence
3. commit-window cursor API
4. checkpoint + incremental integration tests

Implemented Slice B:

1. file-change ingestion into `file_changes`
2. incremental replay cleanup for file-level rows
3. parser + integration validation for file change ingestion

Implemented Slice C:

1. DAG traversal APIs (`bfs`, `dfs`, `topological`)
2. shortest-path API across `parents`/`children`/`both` directions
3. integration validation for traversal and shortest-path contracts

Implemented Slice D:

1. bidirectional BFS shortest-path engine for better scaling behavior
2. non-linear branch/merge fixture coverage for graph-query correctness
3. directional guardrails validation (`both` + topological rejection, traversal truncation signaling)

Implemented Slice E:

1. modular graph-query extraction into dedicated application module (`graph_queries`)
2. batched frontier neighbor retrieval in DB adapter to reduce graph-query I/O round-trips
3. batched BFS/topological and bidirectional-path expansion over indexed commit graph
4. stronger repo-scoped DB unit coverage for neighbor-batch semantics
5. typed `invalid_argument` error semantics for invalid graph-query inputs

Implemented Slice F:

1. integer graph-query identity path (`rowid` -> compact `NodeId`) replacing SHA-string hot-path state
2. rowid-based adapter primitives for commit lookup and neighbor expansion (single + batch)
3. dense integer traversal/path state with SHA materialization only at API boundary
4. ADR-backed performance decision (`0005`) and explicit performance guardrail doc
5. guardrail enforcement against String-heavy frontier state regression

Implemented Slice G:

1. direction-optimizing BFS (`push`/`pull`) with deterministic mode-switch heuristic
2. destination-driven pull expansion query path with deduplicated rowid frontier generation
3. release-mode perf budget test for BFS hot path (`perf_budget_direction_optimizing_bfs`)
4. CI performance budget gate (`scripts/perf-budget.ps1` + Rust CI workflow step)
5. guardrail checks requiring perf gate artifacts and script contract

Implemented Slice H:

1. cross-platform automation guardrail policy (`docs/automation-guardrails.md`)
2. script-level platform metadata contract (`# platform: any|windows`)
3. static portable-script checks against OS-specific shell/env assumptions
4. CI script contract probe lane before heavy build/test stages
5. script runtime hardening for home/path resolution across Windows and Linux runners

## Phase 2 Exit Criteria

1. Deterministic re-index with no data drift for same repo state.
2. Incremental indexing updates only new commits and refs.
3. Commit window and path query APIs return stable, typed payloads.
4. Test suite covers critical indexing/query paths.
5. Observability logs can explain failures without debugging locally.
