# Performance Engineering Guardrails

Date: 2026-03-07

## Goal

Keep graph-query code performant by default on medium/large repositories without weakening reliability or maintainability.

## Non-Negotiable Performance Rules

1. Do not use `String` as frontier identity in graph traversals.
2. Keep traversal/path internal state integer-based (`NodeId`, rowid, compact index).
3. Use batched neighbor loading for frontier expansion where possible.
4. Keep algorithm output contracts unchanged (`sha` remains external DTO identity).
5. Preserve deterministic traversal ordering and typed error semantics.
6. Keep indexing database writes single-writer and transaction-bounded.
7. If Rayon is used in ingestion, keep parallelism bounded/chunked and deterministic.
8. Do not modify docs/guardrails alone to satisfy quality gates; policy edits must be paired with implementation changes.
9. Performance automation must report build time and execution time as separate phases.
10. Performance automation must report RSS memory signals, including peak observed RSS, for repository-scale runs.
11. Repository-scale perf harness must include an explicit in-memory adjacency graph hydrated from SQLite persistence and report its setup/query costs.
12. Ingestion hot path must keep bounded rename detection and set-based incremental cleanup (no row-by-row delete loop).

## Current Strategy in This Repository

1. DB layer resolves commit identity to integer `rowid` for graph query internals.
2. Graph queries intern `rowid` values into compact in-memory IDs.
3. Visited sets use dense bit-like storage instead of `HashSet<String>`.
4. BFS uses direction-optimizing mode selection (`push` vs `pull`) by frontier/remaining ratio.
5. BFS/topological and bidirectional shortest-path expansion use batched neighbor queries with ordered vector outputs (`Vec<Vec<i64>>`) on hot paths.
6. SHA strings are only materialized at API boundaries (request/response) via ordered rowid lookups (no per-node `HashMap` lookups in traversal response assembly).
7. SQLite statement cache is enabled and hot graph queries use cached prepared statements.
8. Repository indexing uses libgit2 streaming adapters (`revwalk` + in-process diff callbacks), not full-buffer Git CLI log/show parsing.
9. Indexer memory is bounded by explicit commit/file-change batch sizes rather than repository-total history size.
10. File-change extraction uses lazy two-stage diff processing in `adapters/git.rs` (delta metadata first, selective `Patch::line_stats` only where required), while DB writes stay in the indexer's single transaction.
11. Git adapter keeps adaptive-but-bounded Rayon planning for chunk size and channel capacity to improve utilization without unbounded fan-out.
12. Graph hot-path edge storage uses integer commit IDs (`child_commit_id`/`parent_commit_id`) instead of SHA edge columns.
13. Performance report output includes phase-level timing (fixture generation, scan, full index, incremental prep/index, query benchmark) so setup cost is never mistaken for algorithm cost.
14. Performance report now includes an in-memory adjacency graph hydration phase sourced from SQLite plus comparative in-memory query latencies (BFS + shortest path).
15. Indexing stats now carry ingestion phase telemetry (`commit_walk`, `commit_upsert`, `edge_materialize`, `file_diff_extract`, `file_change_delete`, `file_change_upsert`, `tx_commit`).

## Why This Strategy

1. Integer node identities reduce hashing and allocation overhead vs repeated string cloning.
2. Dense frontier/visited state improves CPU-cache behavior compared to sparse string hash tables.
3. Batched DB neighbor retrieval reduces query round-trips and iterator overhead.
4. Ordered vector batch decoding removes repeated source-node hash lookups during frontier expansion.
5. Direction-optimizing BFS lowers duplicate expansion work when frontier becomes large.
6. Reusing prepared statements reduces parse/compile overhead in repeated query loops.
7. Parallel file-change compute improves CPU utilization while preserving write-side SQLite constraints.
8. Integer edge columns remove repeated SHA join and string comparison overhead in neighbor expansion paths.

## Guardrail Enforcement

`scripts/enforce-guardrails.ps1` enforces:

1. `application/graph_queries.rs` must not contain:
   - `HashSet<String>`
   - `HashMap<String, String>`
   - `VecDeque<String>`
2. Required governance/performance docs must exist, including this file.
3. Performance CI artifacts must exist:
   - `scripts/perf-budget.ps1`
   - `.github/workflows/rust-ci.yml` contains `Performance Budget` step
   - `package.json` contains `quality:perf` script
4. Anti-tamper rule:
   - if docs/guardrail/policy files are changed, implementation files must also change in the same diff.
5. Automation guardrails and script-platform policy are defined in `docs/automation-guardrails.md`.
6. Git adapter indexing hot path must avoid process-spawned Git CLI and expose streaming APIs.
7. Indexer must use streaming commit ingestion with bounded file-change batch buffers.
8. Git adapter must keep bounded Rayon chunk parallelism for file-change compute.
9. Git adapter must keep adaptive-but-bounded Rayon chunk/channel planning for file-change extraction.
10. Git adapter must remain DB-free (`rusqlite` must not be imported in `adapters/git.rs`).
11. Git adapter must keep two-stage selective line-count extraction (`diff_requires_line_stats` + `Patch::from_diff` path).
12. Indexer must keep explicit single-writer transaction boundary for ingestion writes.
13. DB adapter graph hot-path queries must use integer edge columns (no SHA edge columns).
14. `scripts/perf-budget.ps1` must print separate build/execute phase durations.
15. `scripts/perf-report.ps1` must print separate build/execute phase durations.
16. `apps/desktop/src-tauri/src/bin/perf-report.rs` must emit phase timing in report output.
17. `apps/desktop/src-tauri/src/bin/perf-report.rs` must emit phase memory output (`phase_memory_bytes`, `peak_observed_rss`).
18. `apps/desktop/src-tauri/src/bin/perf-report.rs` must emit in-memory adjacency hydration/query sections.
19. `package.json` must include `perf:report:25k` for large-soak repository-scale benchmarking.
20. `apps/desktop/src-tauri/src/bin/perf-report.rs` must emit ingestion contract fields (`ingest_commit_walk_ms`, `ingest_file_diff_extract_ms`, `ingest_file_change_upsert_ms`, `ingest_tx_commit_ms`).
21. `adapters/git.rs` must enforce bounded rename detection settings (`rename_limit`, `rename_threshold`, `exact_match_only`).
22. `adapters/db.rs` must use set-based temp-table delete for incremental file-change cleanup.
23. `package.json` must expose `perf:ingestion:25k` and `quality:perf:ingestion`.

## Ingestion Performance Policy

1. Do not reintroduce full-buffer hot-path ingestion (`Command::output`-based `git log`/`git show`) in adapter code.
2. Keep commit ingestion streaming.
3. Keep file-change extraction and database writes bounded by explicit batch sizes.
4. Keep index transaction semantics and idempotent upsert behavior unchanged.
5. Keep parse/compute parallelism in Git adapter separated from DB write ownership.
6. Keep integer edge storage contract for graph-query hot paths (`child_commit_id`/`parent_commit_id`).

## Direction-Optimizing BFS Policy

1. Runtime heuristic:
   - switch to `pull` only when frontier is large enough and frontier-to-remaining ratio crosses threshold.
2. Pull expansion contract:
   - database query returns deduplicated candidate rowids in deterministic order.
   - Rust-side visited set remains source of truth for first-visit semantics.
3. Strategy safety:
   - output contract (`GraphTraversalResponse`) and traversal semantics remain unchanged.

## CI Performance Budget

1. Perf budget test name:
   - `perf_budget_direction_optimizing_bfs`
2. It is intentionally `#[ignore]` in normal test loops and runs only in dedicated CI/perf command.
3. Gate command:
   - `pnpm run quality:perf`
4. Current threshold:
   - adaptive BFS median runtime must be no more than 50% slower than push-only baseline on the fixed overlap-heavy fixture.
   - this is a regression guardrail, not a synthetic peak-throughput claim.
5. Reliability contract:
   - when host execution is blocked by endpoint policy (for example Windows application control), `scripts/perf-budget.ps1` retries in Docker and still enforces the same test.
6. Visibility contract:
   - build and execute durations must be printed separately for each run.

## Tiered Perf Testing Pattern

1. Fast local lane:
   - `pnpm run perf:report:smoke`
2. Regression gate lane:
   - `pnpm run quality:perf`
3. Ingestion regression gate lane:
   - `pnpm run quality:perf:ingestion`
4. Large soak lane:
   - `pnpm run perf:report:large`
5. Very large soak lane (~25k commits):
   - `pnpm run perf:report:25k`
6. Balanced default report lane:
   - `pnpm run perf:report`
7. Optional preconditioned soak lane:
   - `pnpm run perf:report:25k:precondition`
8. Interpretation rule:
   - inspect `phase_timing_ms` first; if fixture generation dominates, optimize fixture strategy before changing traversal algorithms.
9. In-memory comparison rule:
   - inspect `in_memory_adjacency_query_latency_ms` and `in_memory_vs_sqlite_median_speedup` to separate persistence costs from pure graph compute.
10. Ingestion diagnosis rule:
   - inspect `ingestion_phase_timing_ms` and aggregate ingestion contract keys before changing batch sizes.
11. Layered throttling isolation lane:
   - `pnpm run test:layers` (runs section-wise test lanes + perf smoke phase metrics including RSS).

## Change Checklist for Graph Query Work

1. Verify algorithm state uses integer node IDs.
2. Verify batch DB path is used for BFS-like expansion.
3. Verify BFS mode-switch heuristic still satisfies deterministic output and perf budget test.
4. Add or update unit/integration tests covering correctness and error contracts.
5. Verify file-change extraction parallel path remains bounded and deterministic.
6. Run:
   - `pnpm run guardrails`
   - `pnpm run dev:gate:fast`
   - `pnpm run dev:gate`
   - `pnpm run quality:perf`
   - `pnpm run quality:perf:ingestion`
   - `pnpm run perf:report:smoke`
   - `pnpm run perf:report:25k` (for repository-scale bottleneck validation)
   - `pnpm run test:layers`

## References

1. SQLite rowid tables: https://www.sqlite.org/rowidtable.html
2. SQLite prepared statement lifecycle/performance guidance: https://www.sqlite.org/cintro.html
3. SQLite `SQLITE_PREPARE_PERSISTENT` guidance (reused statements): https://www.sqlite.org/capi3ref.html#prepare_flags
4. Direction-Optimizing BFS (Beamer et al., SC 2012): https://www.scottbeamer.net/pubs/beamer-sc2012.pdf
5. Ligra (shared-memory graph processing with sparse/dense push-pull): https://jshun.csail.mit.edu/ligra.pdf
6. GraphIt push/pull scheduling and frontier representation options: https://graphit-lang.org/getting-started
7. GAP Benchmark Suite (direction-optimizing BFS in a reference benchmark): https://arxiv.org/abs/1508.03619
8. Rust process output collection semantics: https://doc.rust-lang.org/std/process/struct.Command.html
9. git2 `Revwalk` API: https://docs.rs/git2/latest/git2/struct.Revwalk.html
10. libgit2 diff callback API: https://libgit2.org/docs/reference/main/diff/git_diff_foreach.html
11. Git commit-graph command: https://git-scm.com/docs/git-commit-graph
12. Git maintenance tasks (`commit-graph`): https://git-scm.com/docs/git-maintenance
13. Rayon parallel iterator guide: https://docs.rs/rayon/latest/rayon/iter/index.html
14. Git commit-graph format: https://git-scm.com/docs/gitformat-commit-graph
15. Git diff rename detection and limits: https://git-scm.com/docs/git-diff/2.37.2.html
16. git2 diff find options: https://docs.rs/git2/latest/git2/struct.DiffFindOptions.html
17. SQLite FAQ (batching inserts/transactions): https://sqlite.org/faq.html
18. SQLite WAL: https://sqlite.org/wal.html
