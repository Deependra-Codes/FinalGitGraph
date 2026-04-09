# Diff Extraction + I/O No-Shortcut Strategy

Date: 2026-03-28  
Owner: Runtime Core  
Status: Active Research Blueprint  
Scope: Git ingestion hot path (`commit walk`, `diff extraction`, `LMDB write path`)

## 1) Performance Baseline (Current 25k Evidence)

Source baseline: `tmp/perf-report-25k.txt`.

Key observed timings:

1. `ingest_commit_walk_ms = 2288.240`
2. `ingest_file_diff_extract_ms = 1720.456`
3. `ingest_file_change_upsert_ms = 4024.248`
4. `peak_observed_rss = 48189440`
5. `commit_throughput_per_sec = 2719.221`

Primary interpretation:

1. The runtime is no longer a single bottleneck system.
2. Cost is split between traversal, diff work, and write amplification.
3. Further gains now require hardware-conscious design (cache, page cache, syscall, storage queue behavior), not only API-level tuning.

## 2) Non-Negotiable Optimization Contract

1. No false negatives in changed-path filtering.
2. No unbounded rename/copy exhaustive phase on large diffs.
3. No random object materialization before low-cost reject gates run.
4. No SQL sidecar or fallback runtime path.
5. Every optimization must have measurable phase impact (`commit_walk`, `diff_extract`, `file_change_upsert`, RSS, throughput).

## 3) Research-Backed Strategy Pillars

### 3.1 Commit-Graph First, Always

What to enforce:

1. Read commit-graph metadata first (`CDAT` lineage metadata + Bloom lookup path).
2. Run fast reject checks before `find_commit/tree/diff`.

Complexity effect:

1. Skips expensive object loads and tree diff setup for provable no-op commits.
2. Reduces random object access pressure and page-cache churn.

Source basis:

1. Commit-graph format and changed-path Bloom filter semantics.
2. `git commit-graph --changed-paths` behavior.

### 3.2 Reachability via Bitmaps for Large Walks

What to enforce:

1. Use bitmap-assisted traversal for large-history reachability when bitmap data is available.
2. Keep parent-walk as correctness fallback.

Complexity effect:

1. Shifts work from repeated graph traversal to bitmap operations over precomputed reachability sets.
2. Reduces walk overhead on huge histories.

Source basis:

1. Git bitmap format.
2. `git rev-list` object traversal model.

### 3.3 Keep Rename Detection Hard-Bounded

What to enforce:

1. Never allow unlimited exhaustive rename/copy search for large candidate sets.
2. Enforce strict limits (`renameLimit` equivalent policy) by diff size bins.

Complexity effect:

1. Prevents rename phase blowups that approach `O(n^2)`.

Source basis:

1. `git-diff` guidance around exhaustive rename/copy detection and `-l<num>` guard.

### 3.4 Algorithm Switching in Diff Core

What to enforce:

1. Use algorithm policy by workload shape, not one global setting.
2. Use Myers/minimal only where minimality gain is worth cost.
3. Use histogram/patience in high-churn code regions where practical runtime is better.

Complexity effect:

1. Better real-world latency while preserving diff quality where needed.

Source basis:

1. Myers O(ND) paper.
2. Git diff algorithm options.

### 3.5 I/O Hinting at OS Boundary

What to enforce:

1. Sequential hints for pack streaming workloads.
2. Random/no-readahead style behavior for LMDB random-access windows where appropriate.

Complexity effect:

1. Reduces page-cache thrash.
2. Lowers read amplification and unnecessary readahead work.

Source basis:

1. `posix_fadvise(2)`.
2. `madvise(2)`.
3. LMDB `MDB_NORDAHEAD`.

### 3.6 LMDB Write Path Tuning for Writer Pressure

What to enforce:

1. Use append/reserve-oriented write patterns where key-order semantics allow.
2. Evaluate env flags only under explicit durability/correctness policy.

Complexity effect:

1. Reduces write-side copy/zero/init overhead.
2. Limits B+tree churn and writer contention penalties.

Source basis:

1. LMDB single-writer model.
2. LMDB env flags (`MDB_WRITEMAP`, `MDB_MAPASYNC`, `MDB_NORDAHEAD`, `MDB_NOMEMINIT`).

### 3.7 Linux-Only High-Throughput Mode: io_uring Done Properly

What to enforce:

1. No superficial API swap.
2. Use batching plus registered files/buffers plus architecture-level integration.

Complexity effect:

1. Lowers syscall and context-switch overhead per I/O operation.

Source basis:

1. VLDB 2025 DBMS io_uring study.
2. io_uring man pages and execution model.

### 3.8 Silicon-Level Profiling Loop (Mandatory)

What to enforce:

1. Before each optimization wave, classify bottleneck type: Front-End bound, Back-End bound, branch/speculation bound, syscall/I/O bound.
2. Only ship changes that move the right bound.

Complexity effect:

1. Prevents spending engineering effort on non-dominant costs.

Source basis:

1. Intel Top-Down method.
2. Kernel/perf analysis workflow.

## 4) Max-Speedup Fused Pre-Diff Gate (Inference)

Proposal:

1. Path-ID dictionary for target paths.
2. Prehashed Bloom probe tokens per path-ID.
3. Commit-graph parent-tree fast reject (`CDAT`) before object load.
4. Bloom probe over prehashed tokens.
5. Materialize commit/tree/diff only when gate returns maybe-changed.

Expected behavior:

1. Hot path shifts toward contiguous memory reads and branch-light integer operations.
2. Reduces blob/tree materialization frequency.

Expected complexity shape:

1. Gate check near `O(log N + k*H)` per commit (where `N` is indexed commits, `k` target paths, `H` hash rounds).
2. Materialization cost paid only for probable positives.

Max-speedup implementation profile:

1. Build and cache a stable `path -> path_id` dictionary once per run.
2. Precompute Bloom hash rounds per `path_id` once, not per commit.
3. For each commit, run `CDAT` first-parent tree-equality reject first.
4. Run prehashed Bloom probe second.
5. Call `find_commit/tree/diff` only on `maybe-changed`.
6. Emit gate telemetry for every phase:
`cdat_fast_reject_count`, `bloom_fast_reject_count`, `materialized_commit_count`, `gate_false_positive_rate`.

Target speedup envelope (hypothesis, validate in perf lane):

1. `ingest_file_diff_extract_ms`: target 20% to 45% reduction on large histories with high no-op ratio.
2. `ingest_commit_walk_ms + ingest_file_diff_extract_ms`: target 15% to 35% reduction when commit-graph and Bloom are warm.
3. `commit_throughput_per_sec`: target 1.2x to 1.6x in metadata-heavy indexing waves.

Acceptance criteria for this gate:

1. Zero false negatives versus exact fallback.
2. No regression in `file_change_stats_state_counts` correctness.
3. No increase in peak RSS beyond 10% versus baseline profile.
4. Stable wins across cold and warm lanes.

Inference note:

1. This design is an engineering synthesis from commit-graph/Bloom semantics plus current ingestion structure, not a directly copied upstream Git algorithm.

## 5) Execution Order (Impact-First)

1. Max-speedup fused pre-diff gate (`Path-ID + prehash + CDAT + Bloom`) with telemetry.
2. Commit-graph-first pre-diff gating completeness.
3. Bitmap-assisted large-history traversal.
4. Rename detection hard bounds and policy validation.
5. Diff algorithm switching policy by workload shape.
6. OS I/O hint integration and validation.
7. LMDB writer-path tuning under durability policy.
8. Optional Linux io_uring lane.
9. Continuous silicon-level profiling loop across all phases.

## 6) Anti-Patterns Explicitly Rejected

1. Unbounded rename/copy exhaustive passes.
2. Always-materialize commit/tree before low-cost reject tests.
3. Bloom mode that silently degrades to permissive always-true without telemetry.
4. Any optimization with no measurable phase-level evidence.
5. SQL-era fallback paths for ingestion/query runtime.

## 7) Source List

1. https://git-scm.com/docs/commit-graph-format
2. https://git-scm.com/docs/git-commit-graph
3. https://git-scm.com/docs/gitformat-pack
4. https://git-scm.com/docs/bitmap-format
5. https://git-scm.com/docs/git-rev-list
6. https://git-scm.com/docs/git-diff
7. https://neil.fraser.name/writing/diff/myers.pdf
8. https://man7.org/linux/man-pages/man2/posix_fadvise.2.html
9. https://man7.org/linux/man-pages/man2/madvise.2.html
10. https://manpages.ubuntu.com/manpages/questing/man3/mdb_env.3.html
11. https://web.stanford.edu/class/cs276a/projects/docs/lmdb/intro.html
12. https://docs.rs/gix-commitgraph/latest/gix_commitgraph/struct.Graph.html
13. https://www.informatik.tu-darmstadt.de/media/systems/pdf_publications/iouring_vldb.pdf
14. https://www.kernel.org/doc/html/latest/admin-guide/mm/transhuge.html
15. https://www.intel.com/content/www/us/en/docs/vtune-profiler/cookbook/2023-1/top-down-microarchitecture-analysis-method.html
