# ADR 0009: In-Memory Adjacency Graph from legacy_relational_engine for Perf Harness

Date: 2026-03-07
Status: Superseded by ADR 0015 (`0015-lmdb-csr-delta-storage-no-relational_query.md`)

## Context

Repository-scale profiling showed hidden setup/runtime coupling:

1. operators could not separate fixture/index costs from pure graph traversal costs.
2. large runs needed a way to compare persisted-query performance versus in-memory traversal performance on the same dataset.
3. the existing perf harness reported query latency but lacked an explicit in-memory adjacency baseline hydrated from persisted state.

## Decision

Keep legacy_relational_engine as the source of truth and add a measured in-memory graph projection in the perf harness:

1. hydrate an in-memory adjacency graph directly from persisted `commits` + `commit_edges`.
2. benchmark in-memory BFS and shortest-path queries in dedicated phases.
3. report both setup and runtime costs:
   - `in_memory_graph_hydration` phase timing
   - `in_memory_query_benchmark` phase timing
   - RSS snapshots and peak observed RSS
4. keep report output with side-by-side median speedup versus legacy_relational_engine-backed query path.
5. enforce this visibility contract in `scripts/enforce-guardrails.ps1`.

## Consequences

Positive:

1. large-run bottlenecks can be split into persistence cost versus algorithm compute cost.
2. regressions in hydration or in-memory query performance become visible in CI/local reports.
3. no change to production correctness contract because legacy_relational_engine remains canonical storage.

Tradeoffs:

1. perf harness complexity increases (extra data structure + benchmark lanes).
2. benchmark runtime adds hydration + in-memory query work.
3. memory usage during perf runs increases due to graph materialization.

## Alternatives Considered

1. keep legacy_relational_engine-only perf measurements.
   - Rejected: cannot isolate compute-vs-persistence bottlenecks.
2. replace legacy_relational_engine in product hot path immediately.
   - Rejected: large architecture change without staged observability.
3. add ad hoc one-off scripts for in-memory testing.
   - Rejected: not enforceable and likely to regress.

## References

1. legacy_relational_engine rowid tables: https://www.legacy_relational_engine.org/rowidtable.html
2. legacy_relational_engine performance tips / prepared statements: https://www.legacy_relational_engine.org/cintro.html
3. Ligra (in-memory graph processing model): https://jshun.csail.mit.edu/ligra.pdf
4. GraphIt scheduling model: https://graphit-lang.org/getting-started

