# 90k Performance Final Result

Date: 2026-03-07

## Run Command

```powershell
pnpm run perf:report:90k
```

## Wrapper Timing (Build vs Execute)

1. Host build phase duration (s): `9.1`
2. Host execute phase duration (s): `287.099`
3. Host total duration (s): `296.199`

## Key Snapshot

1. workload_profile_hint: `90k`
2. commits_in_db: `90001`
3. edges_in_db: `99000`
4. file_changes_in_db: `90002`
5. full_index_ms: `129728.489`
6. incremental_index_ms: `37257.634`
7. ingest_file_diff_extract_ms: `119926.807`
8. peak_observed_rss_bytes: `32899072`

## Full 90k Report Output

```text
=== GitGraph Lab Performance Report ===
fixture_root: C:\Users\deepe\AppData\Local\Temp\gitgraph-lab-perf-1772881399526770600
config:
  cycles: 9000
  feature_commits_per_cycle: 6
  incremental_commits: 9000
  query_runs: 20
  window_limit: 1000
  traversal_max_nodes: 90000
  workload_profile_hint: 90k

workload_shape:
  estimated_git_process_spawns: 16

phase_timing_ms:
  fixture_generation: 56491.481
  fixture_summary: 957.541
  repository_preconditioning: 4008.575
  repository_scan: 37.561
  full_index: 129728.489
  incremental_fixture_append: 912.564
  incremental_index: 37257.634
  query_benchmark: 42110.818
  in_memory_graph_hydration: 142.045
  in_memory_query_benchmark: 114.355
  core_end_to_end: 271812.958
  unattributed_core: 51.895

phase_memory_bytes:
  fixture_generation_rss: 4595712
  fixture_summary_rss: 4620288
  repository_preconditioning_rss: 4620288
  repository_scan_rss: 7168000
  full_index_rss: 11083776
  incremental_fixture_append_rss: 12865536
  incremental_index_rss: 11997184
  query_benchmark_rss: 13778944
  in_memory_graph_hydration_rss: 31887360
  in_memory_query_benchmark_rss: 32899072
  final_rss: 32899072
  peak_observed_rss: 32899072

dataset_counts:
  commits_in_db: 90001
  edges_in_db: 99000
  file_changes_in_db: 90002

full_index:
  duration_ms: 129728.489
  commits_indexed: 81001
  edges_indexed: 90000
  file_changes_indexed: 81002
  commit_throughput_per_sec: 624.389
  edge_throughput_per_sec: 693.757
  file_change_throughput_per_sec: 624.396

incremental_index:
  duration_ms: 37257.634
  commits_indexed: 9301
  edges_indexed: 9335
  file_changes_indexed: 9301
  commit_throughput_per_sec: 249.640
  edge_throughput_per_sec: 250.553
  file_change_throughput_per_sec: 249.640

ingestion_phase_timing_ms:
  full_commit_walk: 4643.228
  full_commit_upsert: 8737.349
  full_edge_materialize: 3391.552
  full_file_diff_extract: 106167.733
  full_file_change_delete: 0.000
  full_file_change_upsert: 4356.367
  full_tx_commit: 1091.368
  incremental_commit_walk: 12987.328
  incremental_commit_upsert: 1159.157
  incremental_edge_materialize: 210.908
  incremental_file_diff_extract: 13759.073
  incremental_file_change_delete: 227.878
  incremental_file_change_upsert: 444.653
  incremental_tx_commit: 484.937
  ingest_commit_walk_ms: 17630.556
  ingest_file_diff_extract_ms: 119926.807
  ingest_file_change_upsert_ms: 4801.020
  ingest_tx_commit_ms: 1576.305

query_latency_ms:
  commit_window: runs=20 min=8.249 median=12.145 p95=14.540 max=14.733 mean=11.650
  bfs_parents: runs=20 min=223.647 median=255.652 p95=329.197 max=360.549 mean=265.954
  dfs_parents: runs=20 min=196.968 median=365.283 p95=501.139 max=510.374 mean=350.711
  topological_parents: runs=20 min=242.257 median=453.193 p95=606.529 max=628.005 mean=434.012
  shortest_path_parents: runs=20 min=635.504 median=772.671 p95=1428.458 max=1752.487 mean=916.547
  shortest_path_both: runs=20 min=22.313 median=33.243 p95=39.264 max=40.776 mean=33.692

in_memory_adjacency_graph:
  nodes: 90001
  directed_edges: 99000

in_memory_adjacency_query_latency_ms:
  bfs_parents: runs=20 min=1.926 median=2.746 p95=3.498 max=3.527 mean=2.727
  shortest_path_parents: runs=20 min=1.747 median=2.598 p95=3.496 max=3.512 mean=2.654
  shortest_path_both: runs=20 min=0.064 median=0.066 p95=0.069 max=0.087 mean=0.067

in_memory_vs_legacy_relational_engine_median_speedup:
  bfs_parents: 93.093x
  shortest_path_parents: 297.399x
  shortest_path_both: 504.452x
```

## Ingestion Budget Validation (90k Profile)

```powershell
pnpm run perf:ingestion:90k
```

1. max_full_index_ms: `180000`
2. max_incremental_index_ms: `90000`
3. max_peak_rss_bytes: `200000000`
4. observed_full_index_ms: `101143.923`
5. observed_incremental_index_ms: `24841.76`
6. observed_peak_rss_bytes: `31797248`
7. budget_result: `passed`

