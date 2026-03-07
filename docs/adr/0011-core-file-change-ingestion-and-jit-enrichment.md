# ADR 0011: Core File-Change Ingestion and JIT Enrichment

Date: 2026-03-07

## Context

1. Repository-scale runs showed `file_diff_extract` dominating ingestion wall time.
2. Hot-path indexing should remain single-writer and transaction-bounded for SQLite reliability.
3. Expensive patch line-stat extraction is not required for every graph/query operation.
4. We need predictable cold-vs-warm performance lanes and enforceable anti-regression guardrails.

## Decision

1. Split file-change ingestion into:
   - core metadata ingestion (`commit_sha`, `file_path`, `change_type`) in the hot path
   - deferred line-stat enrichment (`additions`, `deletions`) via queue/JIT path.
2. Store `file_changes.additions` and `file_changes.deletions` as nullable values.
3. Add `file_changes.stats_state` (`pending|ready|failed`) as explicit state contract.
4. Add `file_change_enrichment_queue` to persist pending enrichment work.
5. Hot indexing path uses metadata-only extraction and enqueues only rows requiring line stats.
6. Enrichment path computes selective line stats only for queued/targeted rows and writes results through the DB single-writer path.
7. Query-time JIT is supported through commit-scoped enrichment (`ensure_file_change_stats_for_commits`).
8. Keep bounded rename detection and make thresholds adaptive by delta count and file-size bins.
9. Add warm fixture reuse mode for perf lanes (`--fixture-root` + `--reuse-fixture`) so repeated runs do not re-pay fixture generation.
10. Extend ingestion budget guardrails with warm/cold lane support and diff-share regression checks.

## Consequences

1. Full-index hot path avoids eager patch line-stat extraction and reduces ingestion variance.
2. Enrichment work becomes visible, controllable, and safely retryable through explicit queue/state.
3. API paths that need line stats can request targeted enrichment without forcing global eager extraction.
4. Perf analysis can distinguish fixture cost, core indexing cost, and JIT enrichment cost.
5. Schema complexity increases slightly due nullable stats + queue state management.

## Alternatives Considered

1. Keep eager line-stats extraction in hot ingestion.
   - rejected: remains dominant bottleneck at large scale.
2. Remove line stats completely.
   - rejected: loses functionality for consumers that require change magnitude signals.
3. Move DB writes into parallel workers.
   - rejected: conflicts with SQLite single-writer design and increases lock contention risk.

## References

1. Git diff rename detection behavior/limits: https://git-scm.com/docs/git-diff/2.37.2.html
2. git2 rename controls: https://docs.rs/git2/latest/git2/struct.DiffFindOptions.html
3. libgit2 diff find options: https://libgit2.org/docs/reference/main/diff/git_diff_find_options.html
4. SQLite WAL single-writer model: https://sqlite.org/wal.html
5. SQLite transaction batching guidance: https://sqlite.org/faq.html
