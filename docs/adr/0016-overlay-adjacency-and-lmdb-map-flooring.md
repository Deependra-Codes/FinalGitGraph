# ADR 0016: Overlay Adjacency Rows and LMDB Map Flooring

Date: 2026-03-28
Status: Accepted

## Context

ADR 0015 established LMDB + CSR + delta as the runtime storage direction.
The first overlay implementation stored fresh graph deltas as whole commit records and rebuilt
overlay child adjacency during query hydration.

That was correct but not the best boundary for the workload:

1. query hydration paid repeated parent-to-child inversion work for bounded overlay state
2. write-path delta state was less explicit than the read-path adjacency it needed to serve
3. LMDB map sizing needed to respect existing environment size safely across reopen cycles
4. we wanted a stronger four-part contract across CSR, mmap, delta, and LMDB without introducing
   premature unsafe mmap artifact code

## Decision

Adopt these refinements:

1. bounded delta state in LMDB is stored as explicit overlay adjacency rows:
   - `graph_overlay_parent_rows`
   - `graph_overlay_child_rows`
2. `GraphSnapshotState` exposes:
   - persisted CSR snapshot
   - overlay parent rows
   - overlay child rows
3. query hydration in `application/indexer.rs` composes `snapshot + bounded overlay adjacency`
   directly instead of reconstructing overlay child rows from whole commit records on each read
4. compaction still folds bounded overlay state into the next CSR snapshot version
5. LMDB map sizing at open time uses an existing-environment floor:
   - configured map size remains the primary budget
   - existing `data.mdb` size prevents reopening smaller than the current environment

## Consequences

Positive:

1. delta storage is now closer to the actual query surface it must serve
2. query hydration avoids repeated child-adjacency reconstruction work for bounded overlays
3. LMDB open behavior is safer when environments are reopened after growth
4. this iteration advances all four storage/runtime themes together:
   - CSR snapshot
   - LMDB mmap sizing discipline
   - bounded delta overlay
   - LMDB keyspace layout

Tradeoffs / risks:

1. overlay writes now touch both parent-row and child-row keyspaces
2. bounded overlay duplication increases write amplification slightly
3. map-flooring is conservative; it prevents accidental shrink-on-open but does not yet
   implement live mapsize growth from true used-page telemetry

Migration impact:

1. existing LMDB environments will be rebuilt or compacted into the new overlay row layout
2. guardrails and performance docs must enforce the new adjacency-row contract

## Alternatives Considered

1. Keep overlay state as full commit rows only.
Reason rejected: query hydration would continue rebuilding child adjacency repeatedly.

2. Jump directly to a custom read-only CSR mmap artifact with new unsafe parsing logic.
Reason rejected: useful future direction, but too much boundary churn for this iteration and would
require a dedicated unsafe ADR.

3. Fully mutable CSR / PMA rewrite immediately.
Reason rejected: much larger implementation surface than this repo needs right now; bounded overlay
plus compaction is the more practical hybrid step.

## References

1. `apps/desktop/src-tauri/src/adapters/db.rs`
2. `apps/desktop/src-tauri/src/application/indexer.rs`
3. `docs/performance-engineering.md`
4. `docs/lmdb-csr-mmap-delta-no-mercy-playbook.md`
5. GraphOne (FAST 2019): https://www.usenix.org/system/files/fast19-kumar.pdf
6. Batch-Parallel CSR: https://brianwheatman.com/papers/batch_pcsr.pdf
7. PPCSR / PMA dynamic graph structure: https://people.csail.mit.edu/hjxu/papers/ppcsr-alenex.pdf
8. LMDB API: https://www.lmdb.tech/doc/group__mdb.html
