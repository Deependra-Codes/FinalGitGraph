# ADR 0017: Single Tape Traversal Engine With Commit-Graph Assist

Date: 2026-04-24
Status: Accepted

## Context

Commit traversal had drifted into mixed semantics with legacy backend routing, fallback counters, and mode flags that made behavior harder to reason about and performance harder to optimize. We needed one canonical execution path that preserves correctness when commit-graph metadata is stale or missing, while still using commit-graph/Bloom data as accelerators.

The prior contract exposed toggles and telemetry tied to backend/fallback vocabulary:

1. `GITGRAPH_LEVEL3_PACK_AWARE_INGEST`
2. `GITGRAPH_INDEX_FAST_MODE`
3. `GITGRAPH_COMMIT_WALK_BOUNDARY_V2`
4. backend/fallback telemetry fields in ingestion/perf reporting

This increased operational and testing complexity and encouraged half-migrated behavior.

## Decision

Adopt a single tape-first traversal architecture for ingestion and remove legacy traversal paths.

1. Canonical traversal engine is always tape (`commit_walk_engine=tape`).
2. Commit-graph is assist-only:
   - accelerates lookups/pruning/changed-path decisions when available
   - never becomes final emit authority
3. Correctness fallback remains inside the tape engine only:
   - pack decode path may fall back to repository object lookup
   - no routing to revision-walk backend
4. Incremental hide boundary semantics are fixed to checkpoint-stop.
5. Remove traversal mode flags:
   - `GITGRAPH_LEVEL3_PACK_AWARE_INGEST`
   - `GITGRAPH_INDEX_FAST_MODE`
   - `GITGRAPH_COMMIT_WALK_BOUNDARY_V2`
6. Keep canonical knobs:
   - `GITGRAPH_COMMIT_WALK_SCOPE`
   - `GITGRAPH_COMMIT_WALK_SORT`
7. Replace backend/fallback telemetry with single-engine semantics:
   - `commit_walk_engine`
   - `commit_walk_boundary_mode`
   - `commit_walk_graph_assist_hit_count`
   - `commit_walk_graph_assist_miss_count`
   - `commit_walk_graph_missing_tip_count`

## Consequences

1. Breaking telemetry/environment contract change is intentional and immediate.
2. Ingestion and perf-report parsing become simpler with one engine vocabulary.
3. Rollback surface is reduced because traversal behavior no longer branches by backend mode.
4. Stale or absent commit-graph data no longer threatens correctness; it only impacts acceleration hit rates.
5. Existing external tooling that consumed removed keys must migrate to new telemetry names.

## Alternatives Considered

1. Keep dual-backend support with phased deprecation:
   - rejected due ongoing complexity and ambiguous correctness ownership.
2. Preserve old env/telemetry keys as compatibility shim:
   - rejected to avoid prolonged contract debt and mixed semantics.
3. Make commit-graph primary emit source with tape fallback:
   - rejected because stale graph snapshots create brittle edge cases for authoritative emission.

## References

1. `apps/desktop/src-tauri/src/adapters/git.rs`
2. `apps/desktop/src-tauri/src/application/indexer.rs`
3. `apps/desktop/src-tauri/src/bin/perf-report.rs`
4. `scripts/perf-report.ps1`
5. `scripts/perf-ingestion-budget.ps1`
6. `scripts/enforce-guardrails.ps1`
