# Phase 2 Kickoff (Slice A)

Date: 2026-03-07

## Goal

Start Phase 2 with production-oriented core behavior while keeping strict guardrails active.

## Implemented in This Kickoff

1. Typed command contracts and error codes:
   - Added request/response DTOs in `domain/models.rs`.
   - Added structured command errors (`code`, `message`) mapped from `AppError`.
2. Incremental indexing mode:
   - Added `IndexingMode` (`full`, `incremental`).
   - Added checkpoint storage via migration `0003_indexing_checkpoints.sql`.
   - Incremental mode replays from a short checkpoint lookback window and upserts safely.
3. Commit window API:
   - Added paginated commit-window retrieval with cursor (`committer_time`, `sha`).
   - Added CLI action `window` for local verification.
4. DB adapter hardening for Phase 2:
   - Added `clear_repository_index_data`, `synchronize_refs`, checkpoint load/upsert helpers.
5. Tests:
   - Added checkpoint roundtrip unit test.
   - Added integration test covering incremental indexing + commit window.

## Verification

Validated with repository guardrails and full Docker-backed quality gate:

1. `pnpm run safe:fast`
2. `pnpm run safe:full`

Both passed.

## What This Enables Next

1. File-change ingestion pipeline (`file_changes`) with bounded batch writes.
2. DAG traversal APIs (BFS/DFS/shortest-path).
3. Tauri command exposure over the new typed contracts.

## Phase 2 Slice B Candidate

1. Add `file_changes` ingestion in incremental path.
2. Add deterministic integration fixture with branching and merge commit set.
3. Add first DAG path query endpoint with response contract tests.

## Phase 2 Slice B Status (2026-03-07)

Completed:

1. `file_changes` ingestion from Git numstat output (full + incremental).
2. Incremental replay cleanup for file-change rows.
3. Added parser/unit coverage and integration validation.

## Phase 2 Slice C Status (2026-03-07)

Completed:

1. Added DAG traversal API surface (`bfs`, `dfs`, `topological`) over indexed commit graph.
2. Added shortest-path API with configurable graph direction (`parents`, `children`, `both`).
3. Added CLI actions for local verification:
   - `traverse`
   - `path`
4. Added integration test coverage for traversal and shortest-path behavior.

## Phase 2 Slice D Status (2026-03-07)

Completed:

1. Upgraded shortest-path query to bidirectional BFS for better large-graph behavior.
2. Added non-linear branch/merge integration fixture coverage for graph queries.
3. Validated directional reachability, traversal truncation signaling, and topological-direction guardrails.

## Phase 2 Slice E Status (2026-03-07)

Completed:

1. Extracted graph query logic into `application/graph_queries.rs` for stronger modularity and clearer orchestration boundaries.
2. Added batched graph-neighbor DB lookups (`load_neighbor_shas_batch`) using temporary-node joins to reduce per-node query overhead.
3. Upgraded BFS/topological and bidirectional shortest-path expansion to use batched frontier neighbor loading.
4. Added DB-level unit coverage for repo-scoped batched neighbor retrieval.
5. Hardened argument error semantics by returning `invalid_argument` for invalid graph query inputs (topological+both, missing indexed SHA).

## Phase 2 Slice F Status (2026-03-07)

Completed:

1. Switched graph query runtime identity from SHA strings to integer commit row IDs + compact node IDs.
2. Added rowid-based DB primitives:
   - commit sha <-> rowid resolution
   - single and batched rowid neighbor loading
3. Reworked traversal/path internals to dense integer state while preserving response DTOs.
4. Added ADR `0005-integer-graph-query-state.md`.
5. Added `docs/performance-engineering.md` and enforced it via guardrails.

## Phase 2 Slice G Status (2026-03-07)

Completed:

1. Added direction-optimizing BFS mode selection (`push`/`pull`) for traversal.
2. Added destination-driven pull frontier DB path with deterministic rowid ordering.
3. Added ignored release perf budget test (`perf_budget_direction_optimizing_bfs`).
4. Added `scripts/perf-budget.ps1` and `quality:perf` package script.
5. Wired Rust CI with `Performance Budget` step and updated guardrails to require perf gate artifacts.

## Phase 2 Slice H Status (2026-03-07)

Completed:

1. Added repository-wide automation guardrail contract doc (`docs/automation-guardrails.md`).
2. Added script platform metadata contract (`# platform: any|windows`) enforced in guardrails.
3. Added portable-script policy checks to block OS-specific assumptions in cross-platform scripts.
4. Added `Script Contract Probe` in Rust CI (`./scripts/perf-budget.ps1 -ProbeOnly`) for both runner OSes.
5. Hardened automation scripts for cross-platform home/path resolution.

## Phase 2 Slice I Status (2026-03-08)

Completed:

1. Fixed commit-window max-limit sentinel correctness (`limit=5000` still preserves `limit+1` fetch for `has_more`).
2. Added bounded retryable JIT enrichment lifecycle:
   - non-terminal failures stay `pending` with delayed queue retries
   - terminal transition to `failed` at max attempts
   - re-enqueue resets attempts and last_error for fresh indexing cycles
3. Hardened traversal budget behavior so BFS/DFS/topological truncation avoids unnecessary neighbor expansion when node budgets are exhausted.
4. Hardened file-change parallel extraction with:
   - dedicated bounded Rayon pool
   - adaptive incremental-friendly parallel threshold defaults
   - bounded wave/in-flight buffering with strict ordered emit
5. Added CI/perf coverage upgrades:
   - PR perf smoke report lane
   - nightly ingestion perf workflow (`cold` + `warm`, profile-selectable)
