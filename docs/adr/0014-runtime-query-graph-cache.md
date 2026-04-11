# ADR 0014: Runtime In-Memory Query Graph Cache

Date: 2026-03-09
Status: Superseded by ADR 0015 (`0015-lmdb-csr-delta-storage-no-relational_query.md`)

## Context

1. Repository-scale `query_benchmark` runtime is dominated by repeated legacy_relational_engine neighbor reads across BFS/DFS/topological traversal and shortest-path endpoints.
2. Existing query logic already uses integer identities and batched relational_query, but each API call still pays read-query cost and statement execution overhead.
3. The perf harness already proves the same graph operations are orders of magnitude faster when adjacency is hydrated in memory from legacy_relational_engine.
4. Runtime query APIs need this speedup without changing external DTO contracts or ingestion write semantics.

## Decision

1. Introduce an application-layer runtime cache that stores one hydrated in-memory adjacency graph per `(db_path, repo_id, repositories.indexed_at)`.
2. Route `Indexer::traverse_graph` and `Indexer::shortest_path` to the in-memory graph cache by default.
3. Keep existing legacy_relational_engine query implementation as a rollback path, controlled by `GITGRAPH_QUERY_RUNTIME_CACHE`:
   - enabled by default
   - disabled when env value is one of: `0`, `false`, `off`, `no`.
4. Invalidate cache after successful indexing transaction commit to prevent stale graph reuse.

## Consequences

1. `query_benchmark` shifts from repeated legacy_relational_engine read amplification to mostly in-memory traversal CPU time.
2. First query after index refresh pays hydration cost; subsequent queries reuse the graph.
3. Memory usage increases proportional to commit/edge volume while cache is resident.
4. Rollback is low-risk: disable cache via environment switch and use previous legacy_relational_engine query path.

## Alternatives Considered

1. Keep optimizing relational_query-only traversal.
   - rejected: still bounded by repeated database read path and cannot match in-memory adjacency throughput.
2. External graph service/process.
   - rejected: higher operational complexity and IPC overhead for current desktop-core architecture.
3. Persist precomputed transitive closure in legacy_relational_engine.
   - rejected: storage blow-up and expensive maintenance on incremental updates.

## References

1. `docs/performance-engineering.md`
2. `apps/desktop/src-tauri/src/bin/perf-report.rs`
3. `docs/adr/0009-in-memory-adjacency-graph-from-legacy_relational_engine-for-perf-harness.md`

