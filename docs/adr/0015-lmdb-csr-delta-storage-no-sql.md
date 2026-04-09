# ADR 0015: LMDB + CSR + Delta Storage (No SQL Runtime)

Date: 2026-03-27
Status: Accepted

## Context

The repository runtime previously depended on SQLite-centric persistence and query paths.
Current product requirements explicitly require:

1. no SQL runtime dependency
2. LMDB-backed storage for indexed graph data
3. CSR-based graph traversal substrate
4. append-only delta visibility for persistence mutations

This change crosses adapter + application boundaries and affects indexing, query, and performance
reporting contracts.

## Decision

Adopt an LMDB-native storage engine in `adapters/db.rs` with these contracts:

1. typed record persistence in named LMDB databases
2. append-only delta event log for repository/index/file-change mutations
3. persisted CSR snapshots (`parents` + `children` offsets/neighbors) per repository
4. application query traversal and shortest-path execution against CSR snapshots
5. no active SQL runtime code paths in indexing/query command flows

Operationally:

1. `application/indexer.rs` now orchestrates ingestion directly against `GraphStore`
2. graph read APIs (`traverse_graph`, `shortest_path`) hydrate from CSR snapshots
3. commit-window and enrichment queue operations are served from LMDB records
4. `perf-report` output preserves required telemetry keys for existing perf scripts

Level 3 wiring extension (feature-gated):

1. pack-aware ingestion backend selection is now explicit (`revision-walk` vs `pack-aware wired lane`)
2. changed-path Bloom diff gate has a dedicated integration point with correctness-preserving fallback
3. topology-aware extraction order can be enabled for parent-before-child processing waves
4. streaming CSR materialization can run during ingestion batches (with safe rebuild fallback)
5. JIT enrichment can be coscheduled with extraction waves for cache-locality optimization

## Consequences

Positive:

1. SQL dependency removed from active runtime paths.
2. Graph traversal now executes against compact CSR data, reducing query indirection.
3. Delta log introduces auditable mutation chronology at adapter boundary.

Tradeoffs / risks:

1. Query/read semantics now depend on CSR snapshot freshness after ingestion writes.
2. LMDB map-size and key-shape management become first-class operational concerns.
3. Level 3 feature flags increase execution-path count and require disciplined perf/soak validation before default-on rollout.

Migration impact:

1. Existing SQLite data files are not read by the LMDB runtime.
2. Re-indexing is required to populate LMDB datasets and CSR snapshots.
3. Teams should update guardrail policy docs/scripts to enforce LMDB + CSR + delta invariants.

## Alternatives Considered

1. Keep SQLite and optimize query plans.
Reason rejected: violates no-SQL runtime requirement.

2. Hybrid SQL + LMDB transition mode.
Reason rejected: still retains SQL dependency in runtime surface and increases system complexity.

3. In-memory-only graph runtime with periodic file dumps.
Reason rejected: weak durability model and less operationally predictable recovery behavior.

## References

1. `apps/desktop/src-tauri/src/adapters/db.rs`
2. `apps/desktop/src-tauri/src/application/indexer.rs`
3. `apps/desktop/src-tauri/src/bin/perf-report.rs`
4. `docs/engineering-system.md`
5. `docs/from-scratch-system.md`
6. `docs/lmdb-csr-mmap-delta-no-mercy-playbook.md`
