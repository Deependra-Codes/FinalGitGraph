# 1M Performance Final Result

Date: 2026-03-07

## Run Command

```powershell
powershell -ExecutionPolicy Bypass -File ./scripts/perf-report.ps1 -Cycles 100000 -FeatureCommits 6 -IncrementalCommits 100000 -QueryRuns 20 -WindowLimit 1000 -MaxNodes 1000000 -PreconditionCommitGraph -OutputPath tmp/perf-report-1m.txt
```

## Wrapper Timing (Build vs Execute)

1. Host build phase duration (s): `0.445`
2. Host execute phase duration (s): `1905.409`
3. Host total duration (s): `1905.854`

## Key Snapshot

1. commits_in_db: `1000001`
2. edges_in_db: `1100000`
3. file_changes_in_db: `1000002`
4. full_index_ms: `800770.763`
5. incremental_index_ms: `151697.967`
6. ingest_file_diff_extract_ms: `617772.496`
7. core_end_to_end_ms: `1851850.495`
8. peak_observed_rss_bytes: `210546688`

## Full 1M Report Output

```text
=== GitGraph Lab Performance Report ===
fixture_root: C:\Users\deepe\AppData\Local\Temp\gitgraph-lab-perf-1772884480415503000
config:
  cycles: 100000
  feature_commits_per_cycle: 6
  incremental_commits: 100000
  query_runs: 20
  window_limit: 1000
  traversal_max_nodes: 1000000
  workload_profile_hint: 90k

workload_shape:
  estimated_git_process_spawns: 16

phase_timing_ms:
  fixture_generation: 679864.135
  fixture_summary: 6143.077
  repository_preconditioning: 61370.123
  repository_scan: 73.515
  full_index: 800770.763
  incremental_fixture_append: 6416.529
  incremental_index: 151697.967
  query_benchmark: 142021.976
  in_memory_graph_hydration: 1006.695
  in_memory_query_benchmark: 990.002
  core_end_to_end: 1851850.495
  unattributed_core: 1495.712

phase_memory_bytes:
  fixture_generation_rss: 4415488
  fixture_summary_rss: 4423680
  repository_preconditioning_rss: 4423680
  repository_scan_rss: 7135232
  full_index_rss: 16347136
  incremental_fixture_append_rss: 18038784
  incremental_index_rss: 19787776
  query_benchmark_rss: 21360640
  in_memory_graph_hydration_rss: 208932864
  in_memory_query_benchmark_rss: 210546688
  final_rss: 210546688
  peak_observed_rss: 210546688

dataset_counts:
  commits_in_db: 1000001
  edges_in_db: 1100000
  file_changes_in_db: 1000002

full_index:
  duration_ms: 800770.763
  commits_indexed: 900001
  edges_indexed: 1000000
  file_changes_indexed: 900002
  commit_throughput_per_sec: 1123.918
  edge_throughput_per_sec: 1248.797
  file_change_throughput_per_sec: 1123.920

incremental_index:
  duration_ms: 151697.967
  commits_indexed: 100301
  edges_indexed: 100335
  file_changes_indexed: 100301
  commit_throughput_per_sec: 661.189
  edge_throughput_per_sec: 661.413
  file_change_throughput_per_sec: 661.189

ingestion_phase_timing_ms:
  full_commit_walk: 67873.152
  full_commit_upsert: 86597.809
  full_edge_materialize: 19103.393
  full_file_diff_extract: 551959.662
  full_file_change_delete: 0.000
  full_file_change_upsert: 46121.137
  full_tx_commit: 4043.065
  incremental_commit_walk: 36666.822
  incremental_commit_upsert: 10539.177
  incremental_edge_materialize: 1937.663
  incremental_file_diff_extract: 65812.833
  incremental_file_change_delete: 2421.907
  incremental_file_change_upsert: 5362.053
  incremental_tx_commit: 2655.564
  ingest_commit_walk_ms: 104539.974
  ingest_file_diff_extract_ms: 617772.496
  ingest_file_change_upsert_ms: 51483.190
  ingest_tx_commit_ms: 6698.629

query_latency_ms:
  commit_window: runs=20 min=3.913 median=4.325 p95=5.508 max=6.329 mean=4.548
  bfs_parents: runs=20 min=202.293 median=297.089 p95=326.377 max=348.411 mean=282.440
  dfs_parents: runs=20 min=73.365 median=104.240 p95=124.898 max=132.763 mean=101.780
  topological_parents: runs=20 min=111.316 median=162.018 p95=176.685 max=178.892 mean=156.553
  shortest_path_parents: runs=20 min=5093.730 median=5752.540 p95=7796.952 max=8733.933 mean=6008.454
  shortest_path_both: runs=20 min=133.994 median=136.045 p95=137.974 max=140.560 mean=136.085

in_memory_adjacency_graph:
  nodes: 1000001
  directed_edges: 1100000

in_memory_adjacency_query_latency_ms:
  bfs_parents: runs=20 min=20.369 median=21.497 p95=23.299 max=23.581 mean=21.710
  shortest_path_parents: runs=20 min=21.466 median=23.099 p95=24.680 max=27.143 mean=23.193
  shortest_path_both: runs=20 min=2.049 median=2.195 p95=2.660 max=2.756 mean=2.248

in_memory_vs_legacy_relational_engine_median_speedup:
  bfs_parents: 13.820x
  shortest_path_parents: 249.033x
  shortest_path_both: 61.976x
```

