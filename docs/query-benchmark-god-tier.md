# Query Benchmark God-Tier Plan

Date: 2026-03-09
Owner: Desktop Core

## Problem Statement

On the 90k workload profile, `query_benchmark` dominates end-to-end runtime while ingestion phases are already below query cost.

Baseline snapshot (`tmp/perf-report-90k.txt`):

1. `phase_timing_ms.query_benchmark = 133723.388`
2. `phase_timing_ms.full_index = 20335.529`
3. `phase_timing_ms.incremental_index = 7290.967`
4. `in_memory_vs_sqlite_median_speedup`
   - `bfs_parents = 485.273x`
   - `shortest_path_parents = 396.421x`
   - `shortest_path_both = 1233.180x`

Interpretation: query latency is dominated by SQLite read-path repetition, not graph algorithm complexity.

## Hypothesis

If runtime query endpoints use the same in-memory adjacency model already proven in the perf harness, `query_benchmark` should collapse by one to two orders of magnitude after first hydration.

## Design

1. Hydrate a repository-scoped in-memory graph from SQLite (`commits`, `commit_edges`).
2. Cache key: `(db_path, repo_id, repositories.indexed_at)`.
3. Route `traverse_graph` and `shortest_path` to cached graph by default.
4. Keep SQL traversal path behind rollback switch:
   - `GITGRAPH_QUERY_RUNTIME_CACHE=0|false|off|no`.
5. Invalidate cache after indexing transaction commit.

## Expected Outcome

1. `query_benchmark` becomes bounded mainly by in-memory traversal + response serialization.
2. First query after reindex includes one-time hydration; subsequent query runs amortize that cost.
3. Throughput/latency should move toward the perf harness in-memory envelope for BFS/shortest-path operations.

## Risks and Controls

1. Risk: stale cache after index update.
   - control: invalidate after successful `tx.commit()` and include `indexed_at` in cache key.
2. Risk: memory growth on very large repositories.
   - control: single-entry cache for active `(db_path, repo_id)` in current process.
3. Risk: rollback need after field feedback.
   - control: env-gated fallback to legacy SQL path without code rollback.

## Verification Protocol

1. Run `pnpm run guardrails`.
2. Run `pnpm run dev:gate:fast`.
3. Run `pnpm run dev:gate`.
4. Run `pnpm run perf:report:90k`.
5. Compare before/after:
   - `phase_timing_ms.query_benchmark`
   - `query_latency_ms.*` medians
   - `phase_memory_bytes.*` deltas.
