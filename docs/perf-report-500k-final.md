# 500k Performance Final Result

Date: 2026-03-07

## Run Command

```powershell
powershell -ExecutionPolicy Bypass -File ./scripts/perf-report.ps1 -Cycles 50000 -FeatureCommits 6 -IncrementalCommits 50000 -QueryRuns 20 -WindowLimit 1000 -MaxNodes 500000 -PreconditionCommitGraph -OutputPath tmp/perf-report-500k.txt
```

## Wrapper Timing (Build vs Execute)

1. Host build phase duration (s): `0.639`
2. Host execute phase duration (s): `1043.666`
3. Host total duration (s): `1044.305`

## Key Snapshot

1. commits_in_db: `500001`
2. edges_in_db: `550000`
3. file_changes_in_db: `500002`
4. full_index_ms: `408038.288`
5. incremental_index_ms: `73074.797`
6. ingest_file_diff_extract_ms: `311380.379`
7. core_end_to_end_ms: `987893.098`
8. peak_observed_rss_bytes: `113831936`

## Full 500k Report Output

```text
=== GitGraph Lab Performance Report ===
fixture_root: C:\Users\deepe\AppData\Local\Temp\gitgraph-lab-perf-1772882794704047300
config:
  cycles: 50000
  feature_commits_per_cycle: 6
  incremental_commits: 50000
  query_runs: 20
  window_limit: 1000
  traversal_max_nodes: 500000
  workload_profile_hint: 90k

workload_shape:
  estimated_git_process_spawns: 16

phase_timing_ms:
  fixture_generation: 349781.127
  fixture_summary: 3815.294
  repository_preconditioning: 46315.502
  repository_scan: 105.832
  full_index: 408038.288
  incremental_fixture_append: 2979.285
  incremental_index: 73074.797
  query_benchmark: 102147.266
  in_memory_graph_hydration: 842.533
  in_memory_query_benchmark: 683.784
  core_end_to_end: 987893.098
  unattributed_core: 109.390

phase_memory_bytes:
  fixture_generation_rss: 2719744
  fixture_summary_rss: 2752512
  repository_preconditioning_rss: 2060288
  repository_scan_rss: 5599232
  full_index_rss: 14995456
  incremental_fixture_append_rss: 14905344
  incremental_index_rss: 17051648
  query_benchmark_rss: 18575360
  in_memory_graph_hydration_rss: 112717824
  in_memory_query_benchmark_rss: 113831936
  final_rss: 113831936
  peak_observed_rss: 113831936

dataset_counts:
  commits_in_db: 500001
  edges_in_db: 550000
  file_changes_in_db: 500002

full_index:
  duration_ms: 408038.288
  commits_indexed: 450001
  edges_indexed: 500000
  file_changes_indexed: 450002
  commit_throughput_per_sec: 1102.840
  edge_throughput_per_sec: 1225.375
  file_change_throughput_per_sec: 1102.843

incremental_index:
  duration_ms: 73074.797
  commits_indexed: 50301
  edges_indexed: 50335
  file_changes_indexed: 50301
  commit_throughput_per_sec: 688.350
  edge_throughput_per_sec: 688.815
  file_change_throughput_per_sec: 688.350

ingestion_phase_timing_ms:
  full_commit_walk: 31906.664
  full_commit_upsert: 39531.782
  full_edge_materialize: 8707.244
  full_file_diff_extract: 279365.030
  full_file_change_delete: 0.000
  full_file_change_upsert: 24160.435
  full_tx_commit: 2537.084
  incremental_commit_walk: 19304.264
  incremental_commit_upsert: 4731.024
  incremental_edge_materialize: 665.349
  incremental_file_diff_extract: 32015.349
  incremental_file_change_delete: 1045.952
  incremental_file_change_upsert: 2317.538
  incremental_tx_commit: 1699.649
  ingest_commit_walk_ms: 51210.929
  ingest_file_diff_extract_ms: 311380.379
  ingest_file_change_upsert_ms: 26477.973
  ingest_tx_commit_ms: 4236.733

query_latency_ms:
  commit_window: runs=20 min=4.503 median=4.893 p95=9.141 max=10.185 mean=5.646
  bfs_parents: runs=20 min=151.696 median=165.133 p95=238.579 max=248.881 mean=183.423
  dfs_parents: runs=20 min=74.399 median=88.598 p95=108.833 max=120.831 mean=89.692
  topological_parents: runs=20 min=99.429 median=109.710 p95=119.192 max=124.597 mean=110.590
  shortest_path_parents: runs=20 min=2665.761 median=3748.761 p95=6996.703 max=8064.595 mean=4394.059
  shortest_path_both: runs=20 min=117.942 median=136.954 p95=157.224 max=162.929 mean=135.646

in_memory_adjacency_graph:
  nodes: 500001
  directed_edges: 550000

in_memory_adjacency_query_latency_ms:
  bfs_parents: runs=20 min=13.310 median=14.816 p95=15.506 max=16.685 mean=14.701
  shortest_path_parents: runs=20 min=14.298 median=16.053 p95=16.532 max=16.558 mean=15.899
  shortest_path_both: runs=20 min=1.609 median=2.111 p95=2.386 max=2.587 mean=2.005

in_memory_vs_sqlite_median_speedup:
  bfs_parents: 11.145x
  shortest_path_parents: 233.525x
  shortest_path_both: 64.867x
```
