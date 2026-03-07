# ADR 0010: Bounded Rename Detection and Ingestion-Phase Telemetry

Date: 2026-03-07
Status: Accepted

## Context

Repository-scale indexing showed ingestion as a primary bottleneck for large histories:

1. file-change extraction could spend disproportionate time in rename detection.
2. incremental cleanup used per-commit delete loops for file changes.
3. indexing metrics exposed totals but did not isolate commit walk/upsert/diff/delete/upsert/commit phases.
4. ingestion parse and DB write stages needed bounded overlap without violating SQLite single-writer ownership.

## Decision

Adopt ingestion architecture and telemetry contracts:

1. enforce bounded rename detection in `adapters/git.rs`:
   - explicit `rename_limit`
   - explicit `rename_threshold`
   - adaptive `exact_match_only` for large delta sets
2. replace row-by-row incremental file-change delete loops with set-based temp-table delete.
3. expose indexed ingestion phase timing in `IndexingStats`:
   - `commit_walk`
   - `commit_upsert`
   - `edge_materialize`
   - `file_diff_extract`
   - `file_change_delete`
   - `file_change_upsert`
   - `tx_commit`
4. adopt lazy two-stage file diff extraction in `adapters/git.rs`:
   - stage-1: collect delta metadata and classify targets
   - stage-2: run selective `Patch::from_diff(...).line_stats()` only for deltas that require line counts
5. keep parse parallelism and DB single-writer ownership with bounded producer/consumer channel between parse and callback/write path.
6. keep adaptive-but-bounded Rayon planning for file-change extraction (`parallel_file_change_plan`) so chunk/channel sizing scales with worker count without unbounded fan-out.
7. require perf report contract keys for ingestion (`ingest_*_ms`) and enforce through guardrails.
8. add dedicated ingestion budget lane (`perf:ingestion:25k`) with full/incremental and peak RSS thresholds.

## Consequences

Positive:

1. ingestion regressions become diagnosable by phase instead of a single total.
2. rename-heavy commit batches are bounded and less likely to trigger superlinear cliffs.
3. incremental delete path reduces SQLite statement churn.
4. line-level diff work is loaded only where required, reducing avoidable extraction overhead.
5. adaptive chunk/channel planning improves CPU utilization while keeping deterministic bounded flow.
6. CI/local budget lane can fail ingestion and memory regressions early.

Tradeoffs:

1. telemetry/report surface grows and must be maintained.
2. bounded rename strategy may trade some rename fidelity for predictable performance on large deltas.
3. producer/consumer coordination adds moderate complexity in adapter code.
4. two-stage extraction introduces additional decision logic for line-stat eligibility.

## Alternatives Considered

1. keep rename detection fully unbounded.
   - Rejected: high variance and worst-case runtime spikes on large diffs.
2. remove rename detection entirely.
   - Rejected: loses useful change classification semantics.
3. keep per-commit delete loop.
   - Rejected: unnecessary statement overhead in hot incremental path.
4. skip phase-level telemetry and depend on wall-clock totals.
   - Rejected: not sufficient for root-cause isolation.

## References

1. Git diff rename limit behavior (`-l`, rename/copy detection): https://git-scm.com/docs/git-diff/2.37.2.html
2. `git2` DiffFindOptions API (`rename_limit`, `rename_threshold`, `exact_match_only`): https://docs.rs/git2/latest/git2/struct.DiffFindOptions.html
3. libgit2 diff find options: https://libgit2.org/docs/reference/main/diff/git_diff_find_options.html
4. SQLite transaction batching guidance: https://sqlite.org/faq.html
5. SQLite WAL model: https://sqlite.org/wal.html
6. SQLite pragma guidance: https://www.sqlite.org/pragma.html
