# Performance Engineering Guardrails

Date: 2026-03-27

## Goal

Keep ingestion and query paths fast at repository scale while preserving deterministic behavior,
clear layering, and no-relational_query runtime architecture.

## Runtime Baseline

1. Runtime persistence is LMDB-only (`adapters/db.rs`).
2. Graph hot path uses persisted CSR snapshots (`parents/children` offsets + neighbors).
3. Query APIs run against in-memory graph structures hydrated from CSR snapshots plus bounded overlay adjacency state.
4. Ingestion writes are single-owner through `application/indexer.rs` -> `GraphStore`.
5. Delta events are append-only for mutation chronology and diagnostics.

## Non-Negotiable Performance Rules

1. Do not reintroduce relational_query runtime dependencies (`legacy_relational_driver`, relational_query migrations, relational_query query paths).
2. Keep traversal/path internal state integer-based (`NodeId`, compact indices).
3. Keep ingestion buffering bounded (`FILE_CHANGE_COMMIT_BATCH_SIZE`, `FILE_CHANGE_DB_BATCH_SIZE`).
4. Keep Git adapter compute parallelism bounded and deterministic.
5. Keep DB writes single-owner in indexer/db adapter flow.
6. Keep file-change hot path metadata-first; line stats remain selective/JIT.
7. Keep file-change state contract explicit (`pending|ready|failed`) with retryable queue semantics.
8. Preserve typed error contracts and deterministic traversal ordering.
9. Keep performance automation phase-aware (build vs execute) and memory-aware (RSS signals).

## Current Strategy

1. `adapters/git.rs` handles commit walk + diff extraction in-process via `gix`.
2. `application/indexer.rs` orchestrates streaming ingestion with bounded batches.
3. `adapters/db.rs` persists commits/refs/file-changes in LMDB named databases.
4. `adapters/db.rs` rebuilds and stores CSR snapshots after ingestion.
5. `application/indexer.rs` uses CSR snapshots plus bounded overlay parent/child rows for traversal and shortest-path operations.
6. Perf report emits phase timing, phase memory, ingestion-phase metrics, and file-change state metrics.
7. `adapters/db.rs` stores explicit graph snapshot metadata/versioning beside persisted CSR state.
8. Incremental indexing lands fresh commits in bounded LMDB overlay adjacency state first, then compacts into CSR snapshots by threshold.
9. LMDB open-time map sizing must not reopen smaller than the existing `data.mdb` reservation.

## Complexity Contracts

The repo cannot mechanically prove "global optimality" for every future algorithmic change.
Instead, it enforces the strongest practical asymptotic contracts for this workload.

1. Query hot path:
   - traversal and shortest-path execution must stay on integer-addressed CSR state
   - no query hot path may scan the append-only delta log
   - no query hot path may rebuild CSR in order to answer a read
   - bounded overlay hydration may extend the in-memory query graph, but it must not rewrite full CSR buffers per read
   - overlay child adjacency should be materialized on the write path, not reconstructed from scratch on every read
2. Commit-window hot path:
   - ordered commit-window reads must remain side-index based and range-seek based
   - load-all + global sort is recovery-only fallback, never the primary path
3. Enrichment-queue hot path:
   - pending work acquisition must stay due-index ordered with early stop
   - collect-all + full sort of pending candidates is forbidden in the primary path
4. Snapshot-build path:
   - full CSR rebuild must remain `O(V + E)` with flat contiguous buffers
   - nested per-node adjacency staging in rebuild logic is forbidden when a two-pass count/fill plan is available
5. Incremental-update path:
   - incremental index application must write bounded overlay state first and compact by threshold
   - incremental query freshness must come from `snapshot + bounded overlay`, never from raw delta-log replay
   - full rebuild remains a safety fallback, not the default incremental path
6. LMDB runtime path:
   - mmap/runtime flags must be centralized in one adapter path
   - map sizing must respect existing environment reservation on reopen
   - risky durability/performance flags (`WRITE_MAP`, `MAP_ASYNC`, `NO_MEM_INIT`) must remain explicit opt-in with validation
   - `NO_READAHEAD` remains opt-in and is intended only for random-read-heavy footprints larger than RAM
7. Snapshot-state path:
   - persisted CSR snapshots must carry explicit versioned metadata in LMDB
   - later delta-overlay/compaction work must advance snapshot versions deterministically

## Research-Backed Storage Direction

Primary-source research points to the same practical design:

1. Static CSR is the right read layout for graph traversal because it is compact and contiguous.
2. Pure CSR is the wrong primary write layout because point updates force wide rewrites.
3. High-performance dynamic graph systems therefore use a hybrid model:
   - a write-optimized delta/log/adjacency buffer for fresh updates
   - a read-optimized CSR-style snapshot for analytics
   - periodic freeze/merge/compaction between them
4. For this repo, that means:
   - LMDB remains the durable write-optimized substrate and index/keyspace manager
   - CSR remains the read-optimized graph substrate
   - append-only delta state exists to absorb change chronology and bounded freshness gaps
   - bounded overlay adjacency rows should be stored in LMDB close to the read shape they serve
   - future CSR-delta work should prefer bounded overlay + compaction over "fully mutable CSR everywhere"

## Guardrail Enforcement

`scripts/enforce-guardrails.ps1` enforces:

1. no process-spawned Git CLI hot path in `adapters/git.rs`
2. streaming ingestion APIs in Git adapter/indexer
3. bounded ingestion batching and bounded adapter parallelism
4. no DB dependency in `adapters/git.rs`
5. LMDB GraphStore transaction contract in `adapters/db.rs`
6. persisted CSR snapshot contract in `adapters/db.rs`
7. append-only delta log contract in `adapters/db.rs`
8. no relational_query runtime artifacts in active Rust paths
9. no legacy relational_query runtime modules in `application/*`
10. no relational_query migration files under `apps/desktop/src-tauri/migrations`
11. perf script and perf report telemetry contracts
12. anti-tamper policy: policy-only changes without implementation are blocked
13. centralized LMDB mmap/runtime flag parsing and validation in `adapters/db.rs`
14. commit-window reads remain index/range-seek based in `adapters/db.rs`
15. due-time enrichment reads remain ordered/index based in `adapters/db.rs`
16. query hot path remains integer-based and overlay-aware in `application/indexer.rs`
17. CSR persistence keeps explicit graph snapshot metadata/versioning in `adapters/db.rs`
18. LMDB graph overlay adjacency storage and compaction thresholds remain explicit in `adapters/db.rs`
19. LMDB map-size flooring remains explicit in `adapters/db.rs`

## Ingestion Policy

1. Keep commit ingestion streaming and memory-bounded.
2. Keep file-change extraction adaptive/bounded with deterministic emit order.
3. Keep incremental cleanup prefix-set based in LMDB keyspace.
4. Keep enrichment retries bounded by attempts and backoff.
5. Keep index checkpoint and commit-window contracts stable.

## Perf Workflow

1. `pnpm run guardrails`
2. `pnpm run dev:gate:fast`
3. `pnpm run dev:gate`
4. `pnpm run quality:perf`
5. `pnpm run quality:perf:ingestion`
6. `pnpm run perf:report:smoke`
7. `pnpm run perf:report:25k`
8. `pnpm run test:layers`

## References

1. `docs/automation-guardrails.md`
2. `docs/adr/0015-lmdb-csr-delta-storage.md`
3. `docs/adr/0016-overlay-adjacency-and-lmdb-map-flooring.md`
4. `apps/desktop/src-tauri/src/adapters/db.rs`
5. `apps/desktop/src-tauri/src/adapters/git.rs`
6. `apps/desktop/src-tauri/src/application/indexer.rs`
7. Dynamic Graph Storage: An Experimental Survey (VLDB 2024)
8. LLAMA dissertation / multiversioned CSR + delta map architecture
9. GRAPHONE (FAST 2019) hybrid edge-log + adjacency store
10. BACH (PVLDB 2025) adjacency/CSR layout transition via LSM-style compaction
11. Beamer et al., direction-optimizing BFS
12. LMDB official/runtime docs (`mdb_env_open`, `MDB_NORDAHEAD`, single-writer MVCC)

