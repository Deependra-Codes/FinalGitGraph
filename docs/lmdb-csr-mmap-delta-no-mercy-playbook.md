# LMDB -> CSR -> MMap -> Delta: No-Mercy Engineering Playbook

Date: 2026-03-27
Owner: Runtime Core
Status: Active
Superseding intent reference: ADR 0015

## 1) Mission

Build and maintain a no-SQL runtime with:

1. lowest practical time complexity on hot paths
2. bounded memory behavior at repository scale
3. explicit anti-pattern rejection
4. stateful execution notes that survive context compression

This document is the persistent execution brain for LMDB/CSR/delta work.

## 2) Runtime Topology (Current)

1. Persistence: LMDB named databases in `adapters/db.rs`
2. Graph substrate: persisted CSR snapshot (`parents_offsets/neighbors`, `children_offsets/neighbors`)
3. Query runtime: in-memory traversal over CSR in `application/indexer.rs`
4. Mutation trace: append-only delta events in LMDB `delta_log`
5. Ingestion: streaming git adapter -> bounded batches -> GraphStore writes

## 2.1) Primary-Source Research Verdict

Across dynamic-graph research and storage-engine literature, the consistent result is:

1. Static CSR is excellent for read-heavy analytics because it is contiguous, compact, and cache friendly.
2. Static CSR is poor as the primary mutable representation because point updates induce broad rewrites.
3. The winning architecture for mixed read/write workloads is therefore hybrid:
   - write-optimized buffer/log/delta structure for fresh updates
   - read-optimized CSR-like snapshot for analytics
   - periodic freeze/merge/compaction between them

That conclusion shows up repeatedly in:

1. LLAMA:
   - multiversioned read-optimized CSR store
   - separate write-optimized delta map
   - periodic freeze/merge of snapshots
2. GRAPHONE:
   - append-friendly edge log for fresh updates
   - adjacency-oriented read store for analytics
   - explicit archive/compaction phases
3. BACH:
   - layout transition from write-friendly adjacency structures to read-friendly CSR through compaction
4. Dynamic graph storage surveys:
   - CSR remains the best compact read format
   - update-friendly structures sacrifice locality/space unless hybridized

Repo-level implication:

1. We should not chase "fully mutable CSR everywhere" as the primary design goal.
2. We should keep LMDB as the durable write/delta/index substrate.
3. We should keep CSR as the read-optimized snapshot substrate.
4. We should make freshness a bounded overlay/compaction problem, not a query-time full-history replay problem.
5. We should centralize mmap/runtime flags because LMDB performance knobs have correctness and durability consequences.

## 2.2) Chosen Design Direction For This Repo

For this codebase, the best practical path is:

1. Base layer:
   - one persisted CSR snapshot per repository version boundary
   - integer-addressed parent/child arrays remain the query source of truth
2. Fresh-update layer:
   - append-only delta records for new commits/edges/file-change state transitions
   - bounded overlay structures keyed for targeted merge work
3. Query rule:
   - prefer snapshot-only reads when freshness allows
   - if overlay participation is needed, merge only bounded touched regions
   - never scan the raw delta log in query hot paths
4. Compaction rule:
   - periodically fold deltas into a new CSR snapshot
   - keep compaction cost explicit and measurable
5. LMDB rule:
   - large map sizes remain cheap on 64-bit
   - `NO_READAHEAD` is opt-in for random-read-heavy larger-than-RAM footprints
   - `WRITE_MAP`, `MAP_ASYNC`, and `NO_MEM_INIT` stay opt-in and validated because they trade safety/durability for speed

## 3) Deep Anti-Pattern Audit

### A) LMDB prefix hot paths

Observed anti-pattern (pre-fix):

1. full DB cursor scans with `starts_with(prefix)` and "reached_prefix" sentinel
2. cost: O(total_db_entries) even for a single repo slice when keyspace is multi-repo

Impact:

1. commit/file-change counters and queue scans degrade with total store size, not repo-local size
2. avoidable page walks increase cache pressure and latency jitter

### B) Enrichment pending load

Observed anti-pattern (pre-fix):

1. collect-all pending entries for repo
2. full sort all candidates
3. then take `limit`

Complexity:

1. time O(n log n)
2. memory O(n)

For large pending queues this is unnecessarily expensive when caller needs only top-k oldest.

### C) CSR rebuild heap shape

Observed anti-pattern (pre-fix):

1. build `Vec<Vec<u32>>` for parents and children adjacency staging
2. per-node Vec metadata + realloc churn + fragmentation

Complexity profile:

1. overhead much higher than flat CSR target structure
2. poor locality during construction

### D) Delta log growth

Observed anti-pattern (pre-fix):

1. strictly append-only sequence, no retention bound
2. unbounded growth with long-running repos/automation

Risk:

1. storage bloat
2. unnecessary read amplification for tooling that inspects delta history

## 4) Implemented Fixes (This Iteration)

### 4.1 Prefix-seek iteration (LMDB)

File: `apps/desktop/src-tauri/src/adapters/db.rs`

What changed:

1. introduced `for_each_prefixed_cursor(...)` using `MDB_SET_RANGE` + `MDB_NEXT`
2. replaced full-scan loops in:
   - pending enrichment load
   - commit load
   - file-change state counts
   - extraction queue metrics
   - generic prefixed delete/count helpers

Effect:

1. pre-fix: O(total_db_entries)
2. post-fix: O(entries_in_prefix_range)

### 4.2 Top-k pending selection (bounded memory)

File: `apps/desktop/src-tauri/src/adapters/db.rs`

What changed:

1. introduced `PendingEnrichmentCandidate` + bounded `BinaryHeap`
2. keep only earliest `k` pending entries while scanning
3. final sort on at most `k` entries

Effect:

1. pre-fix: O(n log n), memory O(n)
2. post-fix: O(n log k), memory O(k)

### 4.3 CSR rebuild without nested adjacency vectors

File: `apps/desktop/src-tauri/src/adapters/db.rs`

What changed:

1. switched from `Vec<Vec<u32>>` staging to two-pass count/fill:
   - pass 1: count parent/child degrees
   - build offsets via prefix sums
   - pass 2: fill flat neighbor arrays directly
2. added helpers:
   - `build_offsets_from_counts`
   - `offsets_to_write_positions`

Effect:

1. reduces allocator overhead and fragmentation
2. improves cache locality during rebuild
3. keeps final CSR contract unchanged

### 4.4 Delta retention cap

File: `apps/desktop/src-tauri/src/adapters/db.rs`

What changed:

1. added default retention cap: `DELTA_LOG_RETENTION_DEFAULT_EVENTS = 200_000`
2. env override: `GITGRAPH_DELTA_LOG_RETENTION_EVENTS`
3. prune oldest sequence key when cap exceeded

Effect:

1. bounds delta-log storage growth
2. keeps recent mutation chronology while avoiding unbounded bloat

## 5) Complexity Snapshot (After Changes)

1. Prefix scans: near O(prefix_range)
2. Pending queue top-k: O(n log k), O(k)
3. CSR build: O(V + E) with lower constant factors and lower overhead than nested vectors
4. Delta log growth: O(1) append + O(1) prune, bounded storage by policy

## 6) MMap Notes (LMDB-specific)

LMDB is memory-mapped by design; key operational implications:

1. map size is virtual reservation; backed pages grow with writes
2. random scans amplify page faults/cache misses
3. prefix-seek and range-local iteration materially improve mmap locality
4. map-size control remains via `GITGRAPH_LMDB_MAP_SIZE_BYTES`

Potential next hardening (not yet implemented):

1. MAP_FULL adaptive growth strategy with bounded step-up + telemetry
2. startup validation for map-size floor vs expected dataset size

## 7) Phase 1/2/3 Status (2026-03-27)

### Phase 1 (implemented)

1. Commit-window side index added: `commit_window_index`.
2. `load_commit_window` now serves ordered windows via index range scan + cursor.
3. Legacy load-all+sort path retained as migration fallback only.

Complexity impact:

1. pre-fix: O(repo_commits log repo_commits) + O(repo_commits) memory in hot read path
2. post-fix: O(window_limit) memory and near O(window_limit) time after range seek

### Phase 2 (implemented)

1. Due-time state index added: `enrichment_due_index`.
2. Pending enrichment fetch now scans due index in key order, early-stops after `now`.
3. Queue state transitions maintain due index atomically on enqueue/fail/success/delete.

Complexity impact:

1. pre-fix: collect/sort pending candidates
2. post-fix: ordered top-k acquisition by due-time index with bounded memory

### Phase 3 (implemented)

1. Incremental CSR delta apply added for incremental indexing mode.
2. `indexer` first tries `apply_csr_delta_from_commit_shas`, falls back to full rebuild when needed.
3. Delta apply preserves persisted CSR snapshot + appends delta event contract.

Complexity impact:

1. pre-fix incremental: forced full graph rebuild work profile
2. post-fix incremental: workload proportional to delta commit edge set (plus compact remap work)

### Phase 4 (implemented)

1. Bounded overlay storage now persists explicit parent/child adjacency rows in LMDB.
2. Query hydration composes `snapshot + overlay adjacency` instead of rebuilding overlay child rows from full commit records.
3. LMDB map sizing now respects the existing `data.mdb` reservation on reopen so runtime options do not accidentally reopen smaller than the current environment.

Complexity impact:

1. pre-fix overlay read: bounded overlay load plus repeated child-adjacency reconstruction per hydration
2. post-fix overlay read: bounded overlay load in query-ready adjacency shape
3. mmap/runtime impact: map-size choice now has a safe lower bound from the current environment reservation

### Next candidates (not yet implemented)

1. Compact typed delta payload schema (smaller serialized events, versioned contract).
2. Optional neighbor compression for very large graphs.
3. Checksum + compaction policy lane for long-run snapshot/delta hygiene.
4. Dedicated read-only CSR snapshot artifact / mmap lane behind its own ADR and perf gate.

## 8) Stateful Execution Checklist (Use Every Session)

1. Run `pnpm run guardrails`
2. Run `pnpm run dev:gate:fast`
3. For runtime changes, run `pnpm run dev:gate`
4. For traversal perf-sensitive changes, run `pnpm run quality:perf`
5. Update this file:
   - what changed
   - complexity delta
   - risks introduced/retired

## 9) Session Log (2026-03-27)

Completed:

1. LMDB prefix-seek cursor iteration helper and migration of hot prefix scans.
2. Top-k bounded-memory pending enrichment selection.
3. CSR rebuild rewrite to two-pass count/fill flat arrays.
4. Delta retention cap with env override.
5. Phase 1 commit-window index migration (ordered index scan path).
6. Phase 2 due-time enrichment index migration (state-driven queue path).
7. Phase 3 incremental CSR delta-apply integration in indexer.
8. All gates revalidated after changes.

Validation executed:

1. `cargo check --package gitgraph-lab-core`
2. `cargo test --package gitgraph-lab-core`
3. `pnpm run guardrails`
4. `pnpm run dev:gate:fast`
5. `pnpm run dev:gate`
6. `pnpm run quality:perf`

Companion deep-research blueprint:

1. `docs/diff-extraction-io-no-shortcut-strategy-2026-03-28.md`

Known follow-up:

1. `perf:report:smoke` host run failed in this session (script execution phase), needs isolated diagnosis if required for report lanes.

## 9.1 Session Log (2026-03-28)

Completed:

1. Incremental freshness now lands in bounded overlay state before compaction.
2. Overlay persistence moved from whole commit rows to explicit parent/child adjacency rows in LMDB.
3. Query hydration now reads `snapshot + bounded overlay adjacency`.
4. LMDB map-size resolution now floors at the existing environment reservation.
5. ADR 0016 added for overlay adjacency and LMDB map flooring.

Validation executed:

1. `cargo test --package gitgraph-lab-core --locked`
2. `pnpm run guardrails`
3. `pnpm run dev:gate:fast`
4. `pnpm run dev:gate`
5. `powershell -ExecutionPolicy Bypass -File ./scripts/perf-budget.ps1 -CargoTargetRoot C:\\Users\\deepe\\.cargo-target\\gitgraph-lab-perf-overlay`

## 10) External References

1. LMDB cursor lower-bound semantics (`MDB_SET_RANGE`) in lmdb-rs cursor implementation:
   https://docs.rs/crate/lmdb/0.8.0/source/src/cursor.rs
2. LMDB official API docs (cursor operations):
   https://www.lmdb.tech/doc/group__mdb.html
3. Direction-optimizing BFS paper (hybrid push/pull heuristic):
   https://www.scottbeamer.net/pubs/beamer-sc2012.pdf
4. Dynamic Graph Storage: An Experimental Survey:
   https://ghlkm.github.io/assets/pdf/DynamicGraphStorage.pdf
5. LLAMA dissertation / multiversioned CSR + delta map:
   https://dash.harvard.edu/server/api/core/bitstreams/bd756690-87ae-4db2-ad93-f3fafc14212d/content
6. GRAPHONE hybrid graph store:
   https://www.usenix.org/system/files/fast19-kumar.pdf
7. BACH: Bridging adjacency list and CSR using LSM-style compaction:
   https://www.vldb.org/pvldb/vol18/p1509-miao.pdf

## 11) Level 3 Research + Wiring (2026-03-27)

### 11.1 Source-backed intent

1. Pack-aware ingestion:
   - Git pack files are explicitly structured for sequential object-entry traversal (`PACK` header, object entries, trailer).
   - Source: `gitformat-pack`.
2. Bloom-accelerated diffing:
   - Commit-graph format includes changed-path Bloom filter chunks (`BIDX`, `BDAT`).
   - `git commit-graph write --changed-paths` computes/writes Bloom filters.
   - Sources: `gitformat-commit-graph`, `git-commit-graph`.
3. Topology-aware extraction:
   - `--topo-order` semantics preserve ancestor-before-descendant visibility constraints.
   - Source: `git-rev-list`.
4. Streaming CSR materialization:
   - Dynamic CSR research emphasizes avoiding full vertex-array rebuilds after every update and preserving fast contiguous traversal.
   - Source: BP-CSR paper (`batch_pcsr.pdf`).
5. JIT enrichment coscheduling:
   - Linux page cache retains recently read file data and serves subsequent reads without storage round-trips.
   - Source: Linux kernel memory-management concepts (page cache section).

### 11.2 Implemented wiring in this repo

1. Level 3 runtime flags (Git adapter):
   - `GITGRAPH_LEVEL3_PACK_AWARE_INGEST`
   - `GITGRAPH_LEVEL3_BLOOM_DIFF_SKIP`
   - `GITGRAPH_LEVEL3_TOPO_EXTRACTION_ORDER`
   - Implemented in `adapters/git.rs` via `level3_runtime_options()`.
2. Pack-aware ingestion lane:
   - `walk_commits()` now selects backend (`RevisionWalk` vs `PackAwareWired`).
   - `PackAwareWired` is behavior-preserving and currently routes through revision-walk while the dedicated pack traversal backend is integrated.
3. Bloom diff gate insertion point:
   - Added `filter_commits_for_line_stat_targets(...)` and `commit_might_touch_target_paths(...)`.
   - Current behavior remains preserving correctness (no false negatives): commits are retained until Bloom lookup backend is wired.
4. Topology-aware extraction:
   - Added `reorder_commits_topologically(...)` (Kahn-style parent-before-child ordering with stable tie-break by original chunk order).
   - Indexer uses it when `GITGRAPH_LEVEL3_TOPO_EXTRACTION_ORDER=1`.
5. Streaming CSR materialization:
   - Added `GraphStore::initialize_empty_csr_snapshot(repo_id)` for full-index seed.
   - Indexer can apply CSR deltas per persisted commit batch when `GITGRAPH_LEVEL3_STREAMING_CSR=1`.
   - Fallback rebuild remains for safety when no streaming apply occurs.
6. JIT enrichment coscheduling:
   - Indexer can run enrichment immediately after enqueue on the same extraction wave with
     `GITGRAPH_LEVEL3_COSCHEDULE_JIT_ENRICH=1`.
   - Added ingestion timing metric: `jit_enrich_coscheduled`.

### 11.3 Current delivery state

1. Wiring-first status: complete for all five Level 3 items.
2. Default behavior: unchanged unless Level 3 env flags are enabled.
3. Validation: core check/test passed after wiring changes; full repo gates are run at session end.

### 11.4 Next implementation hardening order

1. Pack-aware backend:
   - Replace `PackAwareWired` fallback with true pack-index driven commit object enumeration.
2. Bloom backend:
   - Implement changed-path Bloom lookups against commit-graph chunks.
   - Maintain strict no-false-negative contract before enabling by default.
3. Topology cache reuse:
   - Add per-wave parent tree/diff cache keyed by commit id to reduce repeated blob/tree lookups.
4. Streaming CSR complexity reduction:
   - Move from repeated full-buffer rewrites toward delta-friendly append/rebalance strategy.
5. Coscheduled enrichment policy:
   - Add adaptive budget controls (batch size / latency ceiling) to avoid tail-latency spikes.

### 11.5 Level 3 references

1. Git pack format:
   https://git-scm.com/docs/gitformat-pack
2. Commit-graph format (Bloom chunks):
   https://git-scm.com/docs/gitformat-commit-graph
3. `git commit-graph` (`--changed-paths`):
   https://git-scm.com/docs/git-commit-graph
4. Topological revision ordering:
   https://git-scm.com/docs/git-rev-list
5. gix pack index APIs:
   https://docs.rs/gix-pack/latest/gix_pack/index/struct.File.html
6. gix commit-graph APIs:
   https://docs.rs/gix-commitgraph/latest/gix_commitgraph/struct.Graph.html
7. Batch-Parallel CSR paper:
   https://brianwheatman.com/papers/batch_pcsr.pdf
8. Linux page cache concept:
   https://docs.kernel.org/5.15/admin-guide/mm/concepts.html

## 12) Level 3 Setup Research Contract (Pack + Bloom + Topology)

Date: 2026-03-28
Status: Active and enforced in code

### 12.1 Non-negotiable optimization contract

1. No full-history random object-lookup walk when pack-index sequential traversal is available.
2. No full tree-diff for line-stat target commits when changed-path filter or exact path-entry check can prove skip.
3. No topology reordering path that reloads commit objects when parent lists are already in memory.
4. No feature may trade correctness for speed. False negatives in changed-path filtering are forbidden.
5. Every optimization must be measurable through existing guardrails and perf lanes.

### 12.2 Algorithm choices and complexity targets

1. Pack-aware ingestion:
   - Strategy: `.idx` sorted offsets -> sequential `.pack` decode -> parsed commit cache -> reachable traversal.
   - Target complexity: O(P) decode for P pack entries + O(V + E) reachable traversal.
   - Memory target: O(C) commit metadata cache for C parsed commits (no full-object materialization).
2. Bloom-accelerated changed-path filtering:
   - Strategy: parse commit-graph `OIDF/OIDL/BIDX/BDAT`; membership test via Murmur3 double hashing.
   - Target complexity per commit/path check: O(log N + H), where N is commits in graph layer and H is Bloom hash rounds.
   - Safety fallback: exact tree-entry comparison on Bloom-negative path to avoid false negatives.
3. Topology-aware extraction:
   - Strategy: Kahn ordering from in-memory `sha -> parents` map captured during stream ingest.
   - Target complexity: O(V + E_chunk) per extraction chunk.
   - Memory target: O(V + E_chunk) graph bookkeeping only (no extra repository object fetch pass).

### 12.3 Anti-patterns explicitly banned

1. Per-commit repository reload during topology reorder when parent map is available.
2. “Bloom enabled” mode that silently returns permissive `true` for every commit.
3. Commit-graph parse failure that aborts indexing (must fail open to exact check path).
4. Any approach that requires SQL sidecar state for these three features.

### 12.4 Source-backed primitives

1. PACK format and offset-indexed access:
   https://git-scm.com/docs/gitformat-pack
2. Commit-graph chunk format and Bloom chunks (`BIDX/BDAT`):
   https://git-scm.com/docs/gitformat-commit-graph
3. Changed-path generation (`git commit-graph --changed-paths`):
   https://git-scm.com/docs/git-commit-graph
4. Topological ordering semantics:
   https://git-scm.com/docs/git-rev-list

### 12.5 Implementation state (2026-03-28)

1. Pack-aware ingestion:
   - Active implementation in `adapters/git.rs` using `gix-pack` index/data decode.
2. Bloom filtering:
   - Active commit-graph parser and Bloom membership check in `adapters/git.rs`.
   - Exact path-entry fallback integrated for correctness.
   - Applied directly in selective line-stats extraction hot path (not flag-gated, evaluated before commit object load).
   - Applied in metadata ingestion hot path as a pre-`find_commit` gate:
     - commit-graph first-parent tree equality short-circuit
     - zeroed changed-path Bloom short-circuit
   - Effect: no-op commits are rejected before commit/tree object resolution and before diff setup.
3. Topology-aware extraction:
   - Active parent-map reorder path in `indexer.rs` + `adapters/git.rs`.
   - Avoids extra `find_commit` loop during extraction ordering.

### 12.6 Validation lane (must pass before merge)

1. `cargo check --package gitgraph-lab-core`
2. `cargo test --package gitgraph-lab-core`
3. `pnpm run guardrails`
4. `pnpm run dev:gate:fast`
5. `pnpm run dev:gate`
6. `pnpm run quality:perf`
