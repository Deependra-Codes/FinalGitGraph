# Consolidated Perf Report (500, 6k, 50k, 100k, 500k)

Date: 2026-03-09

Execution Tracker: `docs/perf-optimization-master-tracker-2026-03-09.md`
Master Consolidated Program Report: `docs/perf-program-consolidated-results-2026-03-09.md`

## Run Setup

All five reports were generated with the same workload shape:

1. `feature_commits_per_cycle=6`
2. `incremental_commits=cycles`
3. `query_runs=20`
4. `window_limit=1000` (except `500` run: `window_limit=500`)
5. `traversal_max_nodes=<target scale>`

Execution command pattern:

```powershell
powershell -ExecutionPolicy Bypass -File ./scripts/perf-report.ps1 `
  -Cycles <N> -FeatureCommits 6 -IncrementalCommits <N> `
  -QueryRuns 20 -WindowLimit <W> -MaxNodes <M> `
  -OutputPath tmp/perf-consolidated/perf-report-<scale>.txt
```

Note: host execution auto-fell back to Docker due local host policy, so these are Docker-backed perf runs.

## Consolidated Results

| Scale | Commits in DB | Full Index ms | Full Commit/s | Full File Changes Indexed | Incremental ms | Incremental Commit/s | Core End-to-End ms | Peak RSS (MiB) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 500 | 501 | 70.951 | 6,356.459 | 452 | 38.145 | 1,310.799 | 628.846 | 11.31 |
| 6k | 6,001 | 199.394 | 27,087.080 | 5,402 | 100.302 | 5,981.921 | 2,145.359 | 19.95 |
| 50k | 50,001 | 2,493.140 | 18,049.928 | 45,002 | 655.871 | 7,623.453 | 12,496.534 | 69.53 |
| 100k | 100,001 | 4,651.544 | 19,348.631 | 90,002 | 1,578.116 | 6,336.670 | 20,215.390 | 116.42 |
| 500k | 500,001 | 46,019.157 | 9,778.558 | 450,002 | 10,597.427 | 4,718.126 | 121,318.997 | 494.33 |

## Query Summary (Median)

| Scale | BFS ms | Topological ms | Shortest Path Both ms | In-Memory BFS Speedup |
|---|---:|---:|---:|---:|
| 500 | 0.841 | 0.956 | 0.653 | 153.859x |
| 6k | 0.874 | 1.537 | 0.440 | 24.498x |
| 50k | 6.171 | 12.055 | 0.449 | 18.232x |
| 100k | 12.347 | 28.192 | 1.738 | 6.426x |
| 500k | 57.492 | 126.444 | 0.421 | 2.910x |

## Bottleneck Analysis (Who Choked the System)

### 1) Harness Bottleneck: Fixture Generation

Fixture generation is the largest non-product cost at most scales.

1. `500`: `199.055 ms` (~31.7% of `core_end_to_end`)
2. `6k`: `1472.966 ms` (~68.7%)
3. `50k`: `7376.652 ms` (~59.0%)
4. `100k`: `9295.548 ms` (~46.0%)
5. `500k`: `45316.150 ms` (~37.4%)

Interpretation: benchmark runtime is heavily influenced by synthetic history creation; this can hide real indexing/query regressions unless warm/reuse lanes are tracked separately.

### 2) Product Bottleneck at Scale: Full Index Pipeline

At `500k`, `full_index` becomes the dominant product-path choke.

1. `full_index`: `46019.157 ms` (~37.9% of core)
2. Biggest full-ingest sub-phases:
   - `full_file_change_upsert`: `13906.494 ms`
   - `full_commit_upsert`: `13558.864 ms`
   - `full_file_diff_extract`: `11648.127 ms`
   - `full_tx_commit`: `6566.272 ms`
   - `full_commit_walk`: `6242.749 ms`

Interpretation: the choke is a combined DB write + diff extraction + commit walk cost, not a single micro-step.

### 3) Query Bottleneck at Scale: Graph Traversals

Query latency growth is concentrated in traversal-heavy queries.

1. `500k` medians:
   - `topological_parents`: `126.444 ms` (worst)
   - `bfs_parents`: `57.492 ms`
   - `shortest_path_parents`: `47.899 ms`
2. In-memory advantage drops at `500k`:
   - BFS speedup falls to `2.910x` (from `18.232x` at `50k`)

Interpretation: traversal/query working-set pressure rises sharply at `500k`; topology-style ordering is the most stressed path.

### 4) Memory Pressure Breakpoint

Peak RSS rises steeply with scale.

1. `100k`: `116.42 MiB`
2. `500k`: `494.33 MiB`

Interpretation: this jump aligns with throughput drop (`full commit/s` from `19,348.631` at `100k` to `9,778.558` at `500k`) and traversal slowdown.

## Recommended Optimal Test System

Use a tiered lane model so local/CI signal stays fast while still catching scale regressions:

1. PR fast lane: `500` + `6k`
2. Pre-merge quality lane: `50k`
3. Nightly lane: `100k`
4. Weekly soak lane: `500k`

Why this is optimal:

1. `500` and `6k` give very fast feedback and still exercise full indexing + diff extraction.
2. `50k` catches realistic performance drift with modest runtime/memory cost.
3. `100k` is a strong nightly regression guard before large-scale soak.
4. `500k` validates heavy-history behavior without slowing every PR.

## Source Reports

1. `tmp/perf-consolidated/perf-report-500.txt`
2. `tmp/perf-consolidated/perf-report-6k.txt`
3. `tmp/perf-consolidated/perf-report-50k.txt`
4. `tmp/perf-consolidated/perf-report-100k.txt`
5. `tmp/perf-consolidated/perf-report-500k.txt`
