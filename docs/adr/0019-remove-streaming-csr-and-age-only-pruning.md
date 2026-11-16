# ADR 0019: Remove Streaming CSR and Age-Only Incremental Pruning

Date: 2026-11-16
Status: Accepted

## Context

Two defects made the stored commit graph disagree with git under default settings. Both were found by code reading in the 2026-11-01 blueprint and red-team reviews, and confirmed by failing tests in `apps/desktop/src-tauri/tests/graph_oracle.rs` before this change.

1. **Edges lost across write waves.** With `GITGRAPH_LEVEL3_STREAMING_CSR` on (the default), each commit write wave was applied to the CSR snapshot through `build_incremental_csr_snapshot`. That function drops a parent edge when the parent is not in the snapshot yet, and later waves copy old rows verbatim. The walk emits children before parents, so every edge whose parent arrived in a later wave was lost. A 20,000-commit full index lost exactly 2 edges: the linear edge at the 16,384-commit wave boundary, and a merge edge crossing it. An incremental run larger than one 8,192-commit wave lost edges the same way, and BFS from HEAD stopped early.
2. **Old-dated commits pruned by age alone.** Incremental walks pruned and skipped every commit older than the checkpoint time minus 300 s, whether or not it was indexed. Merging a branch whose commits predate the checkpoint (an old feature branch) dropped the whole branch. A single future-dated commit raised the checkpoint and blinded later incremental runs.

## Decision

1. Delete the per-wave streaming CSR apply, the full-mode empty-snapshot seed (`initialize_empty_csr_snapshot`), `GITGRAPH_LEVEL3_STREAMING_CSR` and `GITGRAPH_LEVEL3_STREAMING_CSR_MODE`.
   - A full index rebuilds the snapshot once after the walk.
   - An incremental index always goes through the overlay plus compaction. Compaction applies every overlay commit in one call, so no parent is missing when edges are built. `build_incremental_csr_snapshot` and `apply_csr_delta_from_commit_shas` stay for that path.
2. Replace the bare `since_epoch` walk input with `IncrementalCutoff { since_epoch, is_indexed }` in `adapters/git.rs`.
   - All three walkers (commit-graph breadth-first, commit-graph newest-first, pack tape) share one `cutoff_action`.
   - A commit older than the cutoff is pruned only when the index already holds it. Otherwise it is emitted and its parents are walked.
   - The indexer supplies `is_indexed` as a closure over `GraphStore::commit_exists`, so the git adapter stays DB-free.
3. Store index checkpoints under a versioned key (`CHECKPOINT_KEY_VERSION = 2`). Checkpoints written before this change are ignored on purpose. The planner then falls back to a full index once (`PlannerDegradedPolicy::FallbackToFull`), which rebuilds any index that lost edges. No storage format migration is needed.
4. Remove telemetry that only described streaming CSR: `incremental_csr_streaming_mode_effective`, `commit_csr_delta_apply_ms` and `ingest_commit_csr_delta_apply_ms`. No guardrail or script reads them.
5. Delete the unused `load_commits`, `load_commits_since` and `collect_commits`. Remove the streaming-CSR environment settings and ablation scenarios from `scripts/perf-report.ps1` and `scripts/perf-ablation.ps1`.

## Consequences

Verification on 2026-11-16 (macOS 15.8.2 arm64, Apple Git 2.39.5, rustc 1.94.0):

- The four tests written first (`full_index_keeps_every_edge_across_write_waves`, `incremental_index_larger_than_one_wave_keeps_history_connected`, `incremental_index_includes_an_old_dated_merged_branch`, `checkpoint_written_before_the_edge_loss_fix_is_ignored`) failed before the change and pass after it.
- Mutation check: disabling the `is_indexed` test in `cutoff_action` makes the old-dated-branch test fail again with 5 unreachable commits.
- `cargo fmt`, `clippy -D warnings`, `check`, the full test suite, `enforce-guardrails.ps1`, perf smoke and `perf-budget.ps1` pass.

Host 25k cold ingestion lane (`perf-ingestion-budget.ps1 -Profile 25k -Lane cold -PreconditionCommitGraph`, without `-FastMode` because that mode needs Docker), median of 3 runs per side with separate cargo target directories:

| Metric | Before | After |
|---|---|---|
| `full_index_ms` | 120.2 | 99.3 |
| `full_commit_throughput_per_sec` | 168,096 | 203,686 |
| `incremental_index_ms` | 46.9 | 48.3 |
| `peak_observed_rss_bytes` | 89.2 MB | 83.0 MB |
| `file_change_extraction_drain_ms` | 102.3 | 102.1 |

The incremental difference is inside the run-to-run spread (46.3–50.5 ms before, 47.7–50.9 ms after). This lane's 60 MB RSS budget fails on this machine both before and after the change. The CI lane runs with `-FastMode` in Docker, which has a 160 MB budget.

Costs and limits:

- A full index rebuilds the whole snapshot once instead of applying a delta per wave. The measurement above shows this is cheaper at 25k commits. Whole-blob rewrites remain until the layered storage in ADR 0022.
- An incremental walk does one LMDB lookup for each visited commit older than the cutoff. Usually that is only the boundary between new and indexed commits. After a future-dated commit, it covers every new commit.
- Every repository indexed before this change re-indexes in full once, on its next incremental run.
- Commit-graph `committer_time` and parent-first ordering are unchanged. Unshallow, replace refs and grafts still break the "parent indexed before child" assumption. They are covered by ADR 0023 and ADR 0024 in the cleanup roadmap.

This ADR supersedes the per-batch streaming CSR apply described in `0015-lmdb-csr-delta-storage.md` and the streaming-CSR note in `0018-commit-graph-fast-path-hardware-ingestion.md`. The rest of those ADRs stays in force.

## Alternatives Considered

1. **Re-link late parents inside `build_incremental_csr_snapshot`.** This would need an index of pending children per wave and a rewrite of old rows. It adds code to a path the program plans to replace, while the existing compaction path is already correct.
2. **Walk parents before children (parent-first IDs).** This is the long-term fix in ADR 0023, but it changes ID assignment and the query kernel, which is out of scope for a bug fix.
3. **Detect damaged indexes by counting edges on open.** Counting needs every commit loaded, which costs as much as a rebuild on every run. The versioned checkpoint key forces exactly one rebuild.
4. **Declare incremental mode unsound until the new core lands.** Rejected by the owner, who chose to fix the walk now.
