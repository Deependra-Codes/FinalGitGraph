# Perf Program Consolidated Results (Master)

Date: 2026-03-09

Primary tracker: `docs/perf-optimization-master-tracker-2026-03-09.md`  
Primary scale baseline: `docs/perf-report-consolidated-500-6k-50k-100k-500k-2026-03-09.md`

## 1) Repository-Scale Baseline (500, 6k, 50k, 100k, 500k)

| Scale | Commits in DB | Full Index ms | Full Commit/s | Full File Changes Indexed | Incremental ms | Incremental Commit/s | Core End-to-End ms | Peak RSS (MiB) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 500 | 501 | 70.951 | 6,356.459 | 452 | 38.145 | 1,310.799 | 628.846 | 11.31 |
| 6k | 6,001 | 199.394 | 27,087.080 | 5,402 | 100.302 | 5,981.921 | 2,145.359 | 19.95 |
| 50k | 50,001 | 2,493.140 | 18,049.928 | 45,002 | 655.871 | 7,623.453 | 12,496.534 | 69.53 |
| 100k | 100,001 | 4,651.544 | 19,348.631 | 90,002 | 1,578.116 | 6,336.670 | 20,215.390 | 116.42 |
| 500k | 500,001 | 46,019.157 | 9,778.558 | 450,002 | 10,597.427 | 4,718.126 | 121,318.997 | 494.33 |

## 2) Query Baseline (Median)

| Scale | BFS ms | Topological ms | Shortest Path Both ms | In-Memory BFS Speedup |
|---|---:|---:|---:|---:|
| 500 | 0.841 | 0.956 | 0.653 | 153.859x |
| 6k | 0.874 | 1.537 | 0.440 | 24.498x |
| 50k | 6.171 | 12.055 | 0.449 | 18.232x |
| 100k | 12.347 | 28.192 | 1.738 | 6.426x |
| 500k | 57.492 | 126.444 | 0.421 | 2.910x |

## 3) Bottleneck Snapshot (500k Baseline)

1. Full index is dominant product choke: `46019.157 ms` (~37.9% of core).
2. Biggest ingest sub-phases:
   - `full_file_change_upsert=13906.494 ms`
   - `full_commit_upsert=13558.864 ms`
   - `full_file_diff_extract=11648.127 ms`
   - `full_tx_commit=6566.272 ms`
   - `full_commit_walk=6242.749 ms`
3. Query choke:
   - `topological_parents median=126.444 ms` (worst)
   - `bfs_parents median=57.492 ms`
4. Memory breakpoint:
   - `peak_rss=494.33 MiB` at 500k.

## 4) P1 Slice-A Patch (Implemented)

Patch scope in `apps/desktop/src-tauri/src/adapters/db.rs`:

1. Cached `PRAGMA max_variable_number` lookup.
2. Cached generated bulk relational_query templates by row count:
   - file-change upsert relational_query
   - commit upsert relational_query
   - enrichment enqueue relational_query
3. Reused chunk bind vectors (`clear()` reuse) to cut hot-loop allocations.

## 4.1) P1 Slice-B Patch (Implemented)

1. Added full-mode insert-optimized DB APIs:
   - `insert_commits_batch`
   - `insert_file_changes_batch`
2. Routed full indexing to use insert paths (instead of conflict-update upsert) in:
   - `application/indexer.rs`
   - `application/indexer_pipeline.rs`
3. Added chunk-boundary tests for both insert APIs in `adapters/db.rs`.

## 4.2) P1 Slice-C Patch (Implemented)

1. Extended insert-only persistence strategy to incremental indexing path in:
   - `application/indexer.rs`
   - `application/indexer_pipeline.rs`
2. Commit persistence now uses insert-only in indexing flow (commit identity is immutable by SHA).
3. File-change persistence now uses insert-only in indexing flow where incremental already deletes touched commit rows before write.

## 5) Patch-Only A/B (25k Cold Lane, 2 Runs Each)

Command used for both baseline and patched trees:

```powershell
powershell -ExecutionPolicy Bypass -File ./scripts/perf-ingestion-budget.ps1 -OutputPath <path>
```

Artifacts:

1. `tmp/perf-ab/perf-ab-baseline-25k-r1.txt`
2. `tmp/perf-ab/perf-ab-baseline-25k-r2.txt`
3. `tmp/perf-ab/perf-ab-patched-25k-r1.txt`
4. `tmp/perf-ab/perf-ab-patched-25k-r2.txt`

### 5.1 A/B Summary (average of 2 runs)

| Metric | Baseline Avg | Patched Avg | Delta | Delta % |
|---|---:|---:|---:|---:|
| `full_index_ms` | 3100.018 | 2494.146 | -605.872 | -19.54% |
| `commit_throughput_per_sec` | 7355.993 | 9394.028 | +2038.035 | +27.71% |
| `ingest_file_change_upsert_ms` | 635.594 | 561.329 | -74.266 | -11.68% |
| `ingest_file_diff_extract_ms` | 805.992 | 679.986 | -126.006 | -15.63% |
| `ingest_commit_walk_ms` | 1622.668 | 1558.341 | -64.327 | -3.96% |
| `incremental_index_ms` | 1523.615 | 1937.623 | +414.008 | +27.17% |
| `ingest_tx_commit_ms` | 417.336 | 551.745 | +134.409 | +32.21% |
| `peak_observed_rss_bytes` | 29177856 | 29069312 | -108544 | -0.37% |
| `core_end_to_end_ms` | 24665.313 | 20896.285 | -3769.028 | -15.28% |

### 5.2 Query Medians on A/B (average of 2 runs)

| Metric | Baseline Avg | Patched Avg | Delta % |
|---|---:|---:|---:|
| `bfs_parents_median_ms` | 10.380 | 12.395 | +19.42% |
| `topological_parents_median_ms` | 19.870 | 16.141 | -18.76% |
| `shortest_path_parents_median_ms` | 12.604 | 11.443 | -9.21% |
| `shortest_path_both_median_ms` | 5.107 | 5.519 | +8.08% |

Interpretation:

1. Patch gives clear full-ingest wins.
2. Incremental + tx-commit path regressed and is next fix target.
3. Query medians are mixed across runs and need dedicated query-focused follow-up before concluding query trend.

## 6) Quality/Gate Status for Implemented Slice

Passed:

1. `pnpm run guardrails`
2. `pnpm run dev:gate:fast`
3. `pnpm run dev:gate`
4. `pnpm run quality:perf`
5. `pnpm run quality:perf:ingestion`

Latest 25k ingestion gate observation after slices A+B+C:

1. `full_index_ms=4293.991`
2. `incremental_index_ms=2190.406`
3. `peak_observed_rss_bytes=28860416`
4. `diff_share_ratio=0.0329`

Note: direct host binary test execution is endpoint-policy blocked (`os error 4551`), so Docker-backed repo gates were used for reliable validation.

## 7) Phase Snapshot (from Master Tracker)

1. `P0`: In Progress
2. `P1`: In Progress (Slices A+B+C done, 500k rerun pending)
3. `P2`: Not Started
4. `P3`: Not Started
5. `P4`: Not Started
6. `P5`: Not Started

## 8) Deep Docker Suite (Post P1 Slices A+B+C)

Environment was forced to Docker for apples-to-apples comparison with baseline:

```powershell
$env:GITGRAPH_PERF_REPORT_DOCKER_ONLY='1'
```

Artifacts:

1. `tmp/perf-consolidated-post-p1-docker/perf-report-500.txt`
2. `tmp/perf-consolidated-post-p1-docker/perf-report-6k.txt`
3. `tmp/perf-consolidated-post-p1-docker/perf-report-50k.txt`
4. `tmp/perf-consolidated-post-p1-docker/perf-report-100k.txt`
5. `tmp/perf-consolidated-post-p1-docker/perf-report-500k.txt`
6. `tmp/perf-consolidated-post-p1-docker/perf-report-500k-r2.txt` (stability repeat)

### 8.1 Post-P1 Scale Results

| Scale | Commits in DB | Full Index ms | Full Commit/s | Full File Changes Indexed | Incremental ms | Incremental Commit/s | Core End-to-End ms | Peak RSS (MiB) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 500 | 501 | 76.170 | 5,920.947 | 452 | 38.382 | 1,302.690 | 754.508 | 11.11 |
| 6k | 6,001 | 240.429 | 22,463.984 | 5,402 | 136.143 | 4,407.136 | 2,908.129 | 20.09 |
| 50k | 50,001 | 2,032.826 | 22,137.160 | 45,002 | 651.193 | 7,678.218 | 13,644.620 | 69.52 |
| 100k | 100,001 | 4,486.796 | 20,059.079 | 90,002 | 1,493.555 | 6,695.435 | 19,524.281 | 107.95 |
| 500k (avg r1+r2) | 500,001 | 46,432.884 | 9,919.300 | 450,002 | 11,317.895 | 4,439.601 | 126,474.188 | 494.71 |

### 8.2 Query Summary (Median, Post-P1)

| Scale | BFS ms | Topological ms | Shortest Path Both ms | In-Memory BFS Speedup |
|---|---:|---:|---:|---:|
| 500 | 0.931 | 0.982 | 0.814 | 122.651x |
| 6k | 1.354 | 2.753 | 1.058 | 24.058x |
| 50k | 5.244 | 10.461 | 0.398 | 17.986x |
| 100k | 11.454 | 21.581 | 0.393 | 12.667x |
| 500k (avg r1+r2) | 70.829 | 149.095 | 0.433 | 9.202x |

### 8.3 Delta vs Baseline (Key)

| Scale | Full Index | Incremental Index | Core End-to-End | BFS Median | Topological Median |
|---|---:|---:|---:|---:|---:|
| 500 | +7.36% | +0.62% | +19.98% | +10.70% | +2.72% |
| 6k | +20.58% | +35.73% | +35.55% | +54.92% | +79.12% |
| 50k | -18.46% | -0.71% | +9.19% | -15.02% | -13.22% |
| 100k | -3.54% | -5.36% | -3.42% | -7.23% | -23.45% |
| 500k (avg r1+r2) | +0.90% | +6.80% | +4.25% | +23.20% | +17.91% |

Interpretation:

1. Mid/large scales (`50k`, `100k`) show net ingest improvements.
2. Small scales (`500`, `6k`) are slower end-to-end; this is acceptable for now but should be monitored.
3. `500k` ingest is close to baseline, but traversal medians regressed, so query/runtime optimization is now the dominant next work item.

## 9) Next Required Work

1. Move priority to `P3 Query Runtime + Memory` to recover `500k` traversal medians (`bfs`, `topological`).
2. Keep `P2` focused on `ingest_file_diff_extract_ms` and `ingest_tx_commit_ms` since both remain off target at 500k.
3. Re-run the same Docker deep suite after P2/P3 and update section `8.3` in-place for trend continuity.

