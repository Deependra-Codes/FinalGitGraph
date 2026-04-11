# Topological Query God-Tier Research Plan (2026-03-09)

Owner: Desktop Core  
Scope: `topological_parents` hot path (`query_benchmark`) with legacy_relational_engine fallback and in-memory runtime path

## 1. What the current data says

Using current local perf artifacts:

1. [tmp/perf-report-25k.txt](../tmp/perf-report-25k.txt)
2. [tmp/perf-report-50k.txt](../tmp/perf-report-50k.txt)
3. [tmp/perf-report-90k.txt](../tmp/perf-report-90k.txt)
4. [tmp/perf-report-250k.txt](../tmp/perf-report-250k.txt)

Observed `topological_parents` medians:

1. 25k: `16.880 ms`
2. 50k: `39.125 ms`
3. 90k: `163.844 ms`
4. 250k: `260.532 ms`

Share of median query-mix time (commit-window + bfs + dfs + topological + shortest-path):

1. 25k: `58.30%`
2. 50k: `58.73%`
3. 90k: `49.60%`
4. 250k: `73.78%`

Conclusion: topological traversal is now a primary query bottleneck at scale.

## 2. Architecture reality check in this repo

Topological has two execution lanes:

1. Runtime cache lane (default enabled) in [apps/desktop/src-tauri/src/application/indexer.rs](../apps/desktop/src-tauri/src/application/indexer.rs:480)
2. legacy_relational_engine lane (fallback) in [apps/desktop/src-tauri/src/application/graph_queries.rs](../apps/desktop/src-tauri/src/application/graph_queries.rs:462)

The in-memory lane also uses a Kahn-style topo phase with hash maps + BTreeSet in [apps/desktop/src-tauri/src/application/query_graph_cache.rs](../apps/desktop/src-tauri/src/application/query_graph_cache.rs:428).

Important: default behavior is cache-enabled (`GITGRAPH_QUERY_RUNTIME_CACHE` defaults to true) at [apps/desktop/src-tauri/src/application/indexer.rs](../apps/desktop/src-tauri/src/application/indexer.rs:535).

## 3. Research-backed principles

1. Kahn-style topological ordering is still the baseline DAG method; cost is linear in graph size plus ordering data-structure overhead.
   - Source: https://cacm.acm.org/research/topological-sorting-of-large-networks/
2. Recursive relational_query speed improves when selection/projection/dedup are pushed as early as possible.
   - Source: https://www2.cs.uh.edu/~ordonez/pdfwww/w-2010-TKDE-lrq.pdf
3. Recursive graph queries in RDBMS benefit strongly from physical layout + pushdown choices (row vs column/array behavior differs by workload).
   - Source: https://www2.cs.uh.edu/~ordonez/pdfwww/f-2017-IS-Graph-comparison.pdf
4. Modern recursive engines gain from robust parallel recursion strategies (source/destination/hybrid) under skew.
   - Source: https://www.vldb.org/pvldb/vol18/p4465-chakraborty.pdf
5. Seeding transitive structures with selective anti-projection can cut navigational query search space significantly.
   - Source: https://link.springer.com/article/10.1007/s00778-024-00892-7
6. Differential maintenance of recursive graph state is viable for dynamic updates, reducing repeated full recomputation.
   - Source: https://vldb.org/pvldb/vol15/p3186-ammar.pdf
7. legacy_relational_engine-specific constraints and levers:
   - recursive CTE semantics, materialization hints, and DFS/BFS order controls: https://www.legacy_relational_engine.org/lang_with.html
   - query plan verification (`SCAN` vs `SEARCH`, temp b-tree detection): https://www.legacy_relational_engine.org/eqp.html
   - planner/index behavior: https://www.legacy_relational_engine.org/queryplanner.html and https://www.legacy_relational_engine.org/optoverview.html
   - host parameter limits and compile-time tuning: https://www.legacy_relational_engine.org/limits.html and https://www.legacy_relational_engine.org/compile.html
   - maintenance guidance (`PRAGMA optimize`): https://www.legacy_relational_engine.org/pragma.html#pragma_optimize

## 4. God-tier execution plan for this codebase

### Phase A (highest ROI, no new infra): optimize in-memory topological path

Target files:

1. [apps/desktop/src-tauri/src/application/query_graph_cache.rs](../apps/desktop/src-tauri/src/application/query_graph_cache.rs)
2. [apps/desktop/src-tauri/src/application/graph_queries.rs](../apps/desktop/src-tauri/src/application/graph_queries.rs)

Actions:

1. Replace `HashMap<NodeId, usize>` indegree with `Vec<usize>` indexed by compact `NodeId`.
2. Replace `HashMap<NodeId, Vec<NodeId>>` adjacency with `Vec<Vec<NodeId>>`.
3. Replace `BTreeSet<(depth, node)>` ready-set with depth buckets + FIFO queues to remove `O(log n)` tree churn.
4. Fuse reachable-collection and indegree/adjoining edge capture in one pass to avoid redundant neighbor iteration.

Why this is valid:

1. Current graph is already compact-ID based.
2. This directly cuts allocator pressure and branchy map/tree overhead.

### Phase B (legacy_relational_engine fallback hardening): reduce relational_query path overhead

Target files:

1. [apps/desktop/src-tauri/src/application/graph_queries.rs](../apps/desktop/src-tauri/src/application/graph_queries.rs)
2. [apps/desktop/src-tauri/src/adapters/db.rs](../apps/desktop/src-tauri/src/adapters/db.rs)

Actions:

1. For topological fallback, move from repeated `VALUES` chunk joins to temp-table frontier/reachable staging for large frontiers.
2. Add an EXPLAIN-plan guard in perf/dev tooling to fail if fallback query regresses to broad `SCAN` or temp b-tree sorts.
3. Re-check neighbor indexes for ordered scans in both directions and add composite indexes only if EXPLAIN proves benefit.

### Phase C (research-grade): precomputed topological rank for hot direction

Goal: runtime query does not execute full topo sort each call.

Actions:

1. Persist per-commit rank metadata during indexing (or right after index commit) for parent-direction DAG traversal.
2. Query path becomes bounded range/filter/order over rank for common start anchors.
3. Keep exact fallback topo recompute path for correctness validation and edge cases.

This maps to recursive-query seeding and differential-maintenance results from VLDB literature.

## 5. Acceptance gates

For `250k` profile:

1. Reduce `topological_parents` median by at least 40%.
2. Reduce `query_benchmark` by at least 25%.
3. Keep `full_index` and `incremental_index` within plus/minus 10% of baseline.
4. No regressions in traversal correctness tests and truncation semantics.

## 6. Non-goals

1. No Windows workflow breakage.
2. No mandatory Linux-only benchmarking requirement for local dev.
3. No unsafe Rust introduction.



