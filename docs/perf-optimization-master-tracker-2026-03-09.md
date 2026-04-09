# Perf Optimization Master Tracker (P0-P5)

Date: 2026-03-09  
Last Updated: 2026-03-09  
Owner: @deepe (update if changed)  
Branch: `fix/pr-recovery-audit-and-current-changes`  
Baseline Reports:
1. `docs/perf-report-consolidated-500-6k-50k-100k-500k-2026-03-09.md`
2. `tmp/perf-consolidated/perf-report-500k.txt`
Run Environment: Docker-backed perf runs (`scripts/perf-report.ps1` host fallback mode)

## Global Status Board

| Phase | Objective | Status | % Complete | Baseline | Target | Current | Delta | Risk | Blocker | Evidence |
|---|---|---|---:|---|---|---|---|---|---|---|
| P0 | Baseline contract + measurement lock | In Progress | 60 | `full_index_ms=46019.157` | Baseline locked + tracker active | `full_index_ms=46019.157` | `0.000` | Low | None | `tmp/perf-consolidated/perf-report-500k.txt`, consolidated report |
| P1 | Ingestion DB write path acceleration | In Progress | 70 | `ingest_file_change_upsert_ms=15763.459` | `<=10500.000` | 500k Docker rerun complete (2-run avg captured) | `-300.861 ms` on `ingest_file_change_upsert_ms` (500k avg) | Medium | 500k traversal/query medians regressed, requires P3 fixes | `tmp/perf-consolidated-post-p1-docker/perf-report-500k.txt`, `tmp/perf-consolidated-post-p1-docker/perf-report-500k-r2.txt` |
| P2 | Git walk + diff extraction optimization | Not Started | 0 | `ingest_file_diff_extract_ms=12993.633` | `<=9000.000` | `12993.633` | `0.000` | Medium | None | `tmp/perf-consolidated/perf-report-500k.txt` |
| P3 | Query runtime + memory compaction | Not Started | 0 | `topological=126.444`, `bfs=57.492`, `rss=494.33 MiB` | `topological<=80`, `bfs<=40`, `rss<=370 MiB` | `topological=126.444`, `bfs=57.492`, `rss=494.33 MiB` | `0` | High | None | consolidated report + `tmp/perf-consolidated/perf-report-500k.txt` |
| P4 | Harness isolation (warm vs cold lanes) | Not Started | 0 | Fixture cost mixed in end-to-end | Warm/cold separated reports | Baseline only | N/A | Medium | None | consolidated report |
| P5 | CI perf guardrails + regression budgets | Not Started | 0 | Existing lanes (`25k`, `90k`) | Scale-aware guardrails (`50k`, `100k`, `500k`) | Baseline only | N/A | Medium | None | `docs/performance-engineering.md`, scripts/workflows |

## KPI Ledger (Headline Metrics)

| Metric | Baseline (500k) | Target | Latest | Delta vs Baseline | Status | Evidence |
|---|---:|---:|---:|---:|---|---|
| `full_index_ms` | 46019.157 | 34000.000 | 46432.884 (500k avg, r1+r2) | +413.727 | In Progress (near-baseline, target not met) | `tmp/perf-consolidated-post-p1-docker/perf-report-500k.txt`, `tmp/perf-consolidated-post-p1-docker/perf-report-500k-r2.txt` |
| `ingest_file_change_upsert_ms` | 15763.459 | 10500.000 | 15462.598 (500k avg, r1+r2) | -300.861 | In Progress (improved, target not met) | `tmp/perf-consolidated-post-p1-docker/perf-report-500k.txt`, `tmp/perf-consolidated-post-p1-docker/perf-report-500k-r2.txt` |
| `ingest_file_diff_extract_ms` | 12993.633 | 9000.000 | 13108.565 (500k avg, r1+r2) | +114.932 | In Progress (regressed) | `tmp/perf-consolidated-post-p1-docker/perf-report-500k.txt`, `tmp/perf-consolidated-post-p1-docker/perf-report-500k-r2.txt` |
| `topological_parents_median_ms` | 126.444 | 80.000 | 149.095 (500k avg, r1+r2) | +22.651 | In Progress (regressed) | `tmp/perf-consolidated-post-p1-docker/perf-report-500k.txt`, `tmp/perf-consolidated-post-p1-docker/perf-report-500k-r2.txt` |
| `bfs_parents_median_ms` | 57.492 | 40.000 | 70.829 (500k avg, r1+r2) | +13.337 | In Progress (regressed) | `tmp/perf-consolidated-post-p1-docker/perf-report-500k.txt`, `tmp/perf-consolidated-post-p1-docker/perf-report-500k-r2.txt` |
| `peak_rss_mib` | 494.330 | 370.000 | 494.707 (500k avg, r1+r2) | +0.377 | In Progress (effectively flat, target not met) | `tmp/perf-consolidated-post-p1-docker/perf-report-500k.txt`, `tmp/perf-consolidated-post-p1-docker/perf-report-500k-r2.txt` |

## P0 Baseline Contract

### Goal
Lock baseline numbers, environment, and reporting contract so all optimization deltas are trustworthy.

### Hotspots attacked
1. Measurement inconsistency (mixed fixture/product timing).
2. Missing single-source progress visibility.

### Planned tasks
1. Freeze baseline metrics from current consolidated + raw 500k report.
2. Add and maintain this master tracker as canonical status source.
3. Ensure each future phase update records evidence and gate status.

### Done tasks
- [x] Baseline metrics captured from `tmp/perf-consolidated/perf-report-500k.txt`.
- [x] Canonical master tracker created.
- [ ] Add warm/cold marker fields in report output.
- [ ] Add phase-share percentages directly in report output.

### Acceptance metrics
1. Baseline values in tracker exactly match source reports.
2. Global table and phase sections stay 1:1 mapped (`P0..P5`).

### Actual results
1. Baseline values populated and cross-linked.
2. Tracker bootstrapped with phase-level status fields.

### Rollback/fallback toggles
1. N/A (documentation and process contract only).

### Evidence links
1. `docs/perf-report-consolidated-500-6k-50k-100k-500k-2026-03-09.md`
2. `tmp/perf-consolidated/perf-report-500k.txt`

## P1 Ingestion DB Write Path

### Goal
Reduce ingest write overhead, especially upsert and commit-cost hotspots.

### Hotspots attacked
1. `ingest_file_change_upsert_ms`
2. `full_commit_upsert`
3. `ingest_tx_commit_ms`

### Planned tasks
1. Stage/merge upsert strategy for large file-change and commit batches.
2. Transaction chunking for large full-ingest lanes.
3. Throughput profile tuning and validation.

### Done tasks
- [x] Slice A implemented: cached SQLite max bind-parameter lookup, cached bulk SQL templates, and reduced per-chunk allocation churn in DB upsert/enqueue hot paths.
- [x] Slice B implemented: full-mode commit/file-change writes now use insert-optimized DB paths (`ON CONFLICT DO NOTHING`) instead of conflict-update upsert in full indexing flow.
- [x] Slice C implemented: incremental hot path now also uses insert-only commit/file-change persistence where identity is immutable or rows are deleted-before-insert.
- [x] Added chunk-boundary unit coverage for full-mode insert paths:
  - `insert_file_changes_batch_handles_parameter_chunk_boundaries`
  - `insert_commits_batch_handles_parameter_chunk_boundaries`
- [x] Required quality gates passed for this slice (`guardrails`, `dev:gate:fast`, `dev:gate`, `quality:perf`, `quality:perf:ingestion`).
- [x] 500k Docker rerun performed with repeat sample (`r1`, `r2`) and KPI averages recorded.
- [ ] Design finalized and documented for remaining P1 slices (stage/merge and tx chunking).
- [ ] 500k traversal/query regressions addressed (handoff to P3).

### Acceptance metrics
1. `ingest_file_change_upsert_ms <= 10500.000`
2. `full_index_ms` trend improves with no correctness regression.

### Actual results
1. Implementation landed in `apps/desktop/src-tauri/src/adapters/db.rs` for batch-upsert/enqueue hot paths.
2. Full-index orchestration now routes full-mode persistence through insert-optimized APIs in:
   - `apps/desktop/src-tauri/src/application/indexer.rs`
   - `apps/desktop/src-tauri/src/application/indexer_pipeline.rs`
2. Gate status:
   - `pnpm run guardrails`: pass
   - `pnpm run dev:gate:fast`: pass
   - `pnpm run dev:gate`: pass
   - `pnpm run quality:perf`: pass
   - `pnpm run quality:perf:ingestion`: pass
3. Latest ingestion budget (25k cold lane) observed after slices A+B+C:
   - `full_index_ms=4293.991`
   - `incremental_index_ms=2190.406`
   - `peak_observed_rss=28860416`
   - `diff_share_ratio=0.0329`
4. 500k KPI deltas now captured from Docker `r1+r2` average:
   - `full_index_ms`: `+0.90%` (near baseline)
   - `ingest_file_change_upsert_ms`: `-1.91%` (improved)
   - `ingest_file_diff_extract_ms`: `+0.88%` (regressed)
   - `ingest_tx_commit_ms`: `+2.30%` (regressed)
5. Query medians at 500k remain regressed and are now explicit blocker for next phase:
   - `bfs_parents median`: `57.492 -> 70.829` (avg, `r1+r2`)
   - `topological_parents median`: `126.444 -> 149.095` (avg, `r1+r2`)

### Rollback/fallback toggles
1. Keep feature flags/env toggles for write-path strategy and chunk size.

### Evidence links
1. `apps/desktop/src-tauri/src/adapters/db.rs` (implemented slices A+B)
2. `apps/desktop/src-tauri/src/application/indexer.rs` (full-mode persistence routing)
3. `apps/desktop/src-tauri/src/application/indexer_pipeline.rs` (full-mode file-change flush routing)
4. `tmp/perf-consolidated/perf-report-500k.txt` (500k baseline)
5. `tmp/perf-consolidated-post-p1-docker/perf-report-500k.txt` (500k rerun r1)
6. `tmp/perf-consolidated-post-p1-docker/perf-report-500k-r2.txt` (500k rerun r2)
7. `tmp/perf-report-ingestion-budget.txt` (post-change 25k ingestion gate output)

## P2 Git Walk + Diff Extraction

### Goal
Cut commit walk and diff extraction runtime while preserving ingestion guardrails.

### Hotspots attacked
1. `ingest_commit_walk_ms`
2. `ingest_file_diff_extract_ms`
3. Extraction worker utilization overhead

### Planned tasks
1. Improve walk path using commit-graph-aware strategy where available.
2. Tighten bounded parallel extraction/backpressure behavior.
3. Increase object/tree reuse to avoid redundant diff cost.

### Done tasks
- [ ] Design finalized and documented.
- [ ] Implementation merged.
- [ ] A/B benchmark and regression checks completed.

### Acceptance metrics
1. `ingest_file_diff_extract_ms <= 9000.000`
2. `ingest_commit_walk_ms` decreases from baseline with parity preserved.

### Actual results
1. Pending.

### Rollback/fallback toggles
1. Keep fallback to existing extraction/walk behavior via runtime flags.

### Evidence links
1. `tmp/perf-consolidated/perf-report-500k.txt` (baseline)
2. Add PR and CI links when phase starts.

## P3 Query Runtime + Memory

### Goal
Reduce traversal latency and memory pressure at high scale (`500k`).

### Hotspots attacked
1. `topological_parents` median latency
2. `bfs_parents` median latency
3. `peak_rss_mib`

### Planned tasks
1. Compact indexed adjacency representation with reusable buffers.
2. Topological traversal optimization on indexed graph structures.
3. Direction-optimizing BFS tuning for large frontier regimes.

### Done tasks
- [ ] Design finalized and documented.
- [ ] Implementation merged.
- [ ] A/B benchmark and regression checks completed.

### Acceptance metrics
1. `topological_parents_median_ms <= 80.000`
2. `bfs_parents_median_ms <= 40.000`
3. `peak_rss_mib <= 370.000`

### Actual results
1. Pending.

### Rollback/fallback toggles
1. Keep query-engine fallback path and runtime cache disable toggle.

### Evidence links
1. `docs/perf-report-consolidated-500-6k-50k-100k-500k-2026-03-09.md` (baseline)
2. `tmp/perf-consolidated/perf-report-500k.txt` (baseline)
3. Add PR and CI links when phase starts.

## P4 Harness Isolation

### Goal
Separate fixture-generation cost from product-path runtime to prevent misattribution.

### Hotspots attacked
1. End-to-end timing inflation from fixture generation.
2. Ambiguous interpretation between cold and warm runs.

### Planned tasks
1. Standardize warm/cold lanes in perf reporting workflow.
2. Add explicit report sections/markers for warm vs cold.
3. Document interpretation rules and dashboard mapping.

### Done tasks
- [ ] Design finalized and documented.
- [ ] Implementation merged.
- [ ] A/B benchmark and regression checks completed.

### Acceptance metrics
1. Warm reports exclude fixture-generation from product KPI comparison.
2. Cold reports remain available for full pipeline health.

### Actual results
1. Pending.

### Rollback/fallback toggles
1. Keep existing report mode as fallback while warm/cold split stabilizes.

### Evidence links
1. `docs/perf-report-consolidated-500-6k-50k-100k-500k-2026-03-09.md`
2. Add PR and CI links when phase starts.

## P5 CI Perf Guardrails

### Goal
Enforce regression budgets in CI across representative scales.

### Hotspots attacked
1. Silent perf regressions at large scales.
2. Missing budget enforcement for target lanes.

### Planned tasks
1. Add/extend CI lanes for `50k`, `100k`, `500k` budget checks.
2. Define strict fail thresholds for headline KPIs.
3. Publish artifacts for trend visibility and auditability.

### Done tasks
- [ ] Design finalized and documented.
- [ ] Implementation merged.
- [ ] CI budget gating active.

### Acceptance metrics
1. CI fails when KPI thresholds regress beyond defined budget.
2. Every perf gate run emits report artifacts with comparable fields.

### Actual results
1. Pending.

### Rollback/fallback toggles
1. Keep existing perf gates active while adding stricter scale lanes.

### Evidence links
1. `docs/performance-engineering.md`
2. Add workflow run URLs and PR links when phase starts.

## Dry Update Entry (P0 Process Validation)

- Date/Time: 2026-03-09 (dry run entry)
- Phase: `P0`
- Commands run:
  1. `Get-Content tmp/perf-consolidated/perf-report-500k.txt -TotalCount 260`
  2. `git branch --show-current`
- Report path:
  1. `tmp/perf-consolidated/perf-report-500k.txt`
- Gate status:
  1. `pnpm run guardrails`: `not-run (dry entry)`
  2. `pnpm run dev:gate:fast`: `not-run (dry entry)`
  3. `pnpm run dev:gate`: `not-run (dry entry)`
  4. `pnpm run quality:perf`: `not-run (dry entry)`
- Notes:
  1. This validates update format only; no phase completion claim.

## Update Entry (P1 Slice A)

- Date/Time: 2026-03-09
- Phase: `P1`
- Commands run:
  1. `pnpm run guardrails`
  2. `pnpm run dev:gate:fast`
  3. `pnpm run dev:gate`
  4. `pnpm run quality:perf`
  5. `pnpm run quality:perf:ingestion`
- Report path:
  1. `tmp/perf-report-ingestion-budget.txt`
  2. `tmp/perf-consolidated/perf-report-500k.txt` (baseline reference)
- Gate status:
  1. `pnpm run guardrails`: `pass`
  2. `pnpm run dev:gate:fast`: `pass`
  3. `pnpm run dev:gate`: `pass`
  4. `pnpm run quality:perf`: `pass`
  5. `pnpm run quality:perf:ingestion`: `pass`
- Notes:
  1. Local host direct test binary execution is policy-blocked (`os error 4551`); repository Docker-backed gates provided full verification.
  2. P1 remains `In Progress` until 500k A/B rerun updates KPI deltas.

## Update Entry (P1 Slice B)

- Date/Time: 2026-03-09
- Phase: `P1`
- Commands run:
  1. `cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml`
  2. `pnpm run guardrails`
  3. `pnpm run dev:gate:fast`
  4. `pnpm run dev:gate`
  5. `pnpm run quality:perf`
  6. `pnpm run quality:perf:ingestion`
- Report path:
  1. `tmp/perf-report-ingestion-budget.txt`
  2. `tmp/perf-consolidated/perf-report-500k.txt` (baseline reference)
- Gate status:
  1. `pnpm run guardrails`: `pass`
  2. `pnpm run dev:gate:fast`: `pass`
  3. `pnpm run dev:gate`: `pass`
  4. `pnpm run quality:perf`: `pass`
  5. `pnpm run quality:perf:ingestion`: `pass`
- Notes:
  1. This slice targets full-mode persistence path to reduce upsert/update overhead during full indexing.
  2. P1 still remains `In Progress` until 500k A/B rerun updates KPI deltas.

## Update Entry (P1 Slice C)

- Date/Time: 2026-03-09
- Phase: `P1`
- Commands run:
  1. `cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml`
  2. `pnpm run guardrails`
  3. `pnpm run dev:gate:fast`
  4. `pnpm run dev:gate`
  5. `pnpm run quality:perf:ingestion`
  6. `pnpm run quality:perf`
- Report path:
  1. `tmp/perf-report-ingestion-budget.txt`
  2. `tmp/perf-consolidated/perf-report-500k.txt` (baseline reference)
- Gate status:
  1. `pnpm run guardrails`: `pass`
  2. `pnpm run dev:gate:fast`: `pass`
  3. `pnpm run dev:gate`: `pass`
  4. `pnpm run quality:perf`: `pass`
  5. `pnpm run quality:perf:ingestion`: `pass`
- Notes:
  1. This slice targets incremental write amplification by avoiding conflict-update upsert on immutable commit identity and delete-before-insert file-change rows.
  2. P1 remains `In Progress` with 500k KPI deltas captured; next blocker is traversal/query regression at 500k.

## Update Entry (P1 Deep Docker Validation)

- Date/Time: 2026-03-09
- Phase: `P1`
- Commands run:
  1. Full Docker-forced perf suite for `500`, `6k`, `50k`, `100k`, `500k`
  2. Additional 500k stability rerun (`r2`)
- Report path:
  1. `tmp/perf-consolidated-post-p1-docker/perf-report-500.txt`
  2. `tmp/perf-consolidated-post-p1-docker/perf-report-6k.txt`
  3. `tmp/perf-consolidated-post-p1-docker/perf-report-50k.txt`
  4. `tmp/perf-consolidated-post-p1-docker/perf-report-100k.txt`
  5. `tmp/perf-consolidated-post-p1-docker/perf-report-500k.txt`
  6. `tmp/perf-consolidated-post-p1-docker/perf-report-500k-r2.txt`
- Gate status:
  1. `pnpm run guardrails`: `pass`
  2. `pnpm run dev:gate:fast`: `pass`
  3. `pnpm run dev:gate`: `pass`
  4. `pnpm run quality:perf`: `pass`
  5. `pnpm run quality:perf:ingestion`: `pass`
- Notes:
  1. Environment consistency was enforced with `GITGRAPH_PERF_REPORT_DOCKER_ONLY=1` for apples-to-apples comparison against baseline.
  2. 500k KPI averages (`r1+r2`) are now recorded in the ledger.

## Update Protocol (Mandatory)

Every phase update must include all fields below:

1. Date/time.
2. Phase ID (`P0..P5`).
3. Command(s) run.
4. Report file path(s).
5. Gate outcomes:
   - `pnpm run guardrails` (`pass|fail`)
   - `pnpm run dev:gate:fast` (`pass|fail`)
   - `pnpm run dev:gate` (`pass|fail`)
   - `pnpm run quality:perf` (`pass|fail`)
6. KPI delta update in `KPI Ledger`.
7. Evidence links (report, PR, CI run).

Rules:

1. No phase may be marked `Done` without metric delta and at least one evidence link.
2. If gates are skipped, status must remain `In Progress` or `Blocked` with explicit reason.
3. If metric regresses, update `Risk` and `Blocker` in Global Status Board before closing update.
