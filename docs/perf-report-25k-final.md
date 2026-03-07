# 25k Performance Final Result

Date: 2026-03-07

## Run Command

```powershell
pnpm run perf:report:25k
```

## Wrapper Timing (Build vs Execute)

1. Host build phase duration (s): `6.36`
2. Host execute phase duration (s): `104.185`
3. Host total duration (s): `110.545`

## Full 25k Report Output

```text
=== GitGraph Lab Performance Report ===
fixture_root: C:\Users\deepe\AppData\Local\Temp\gitgraph-lab-perf-1772875042754135700
config:
  cycles: 2500
  feature_commits_per_cycle: 6
  incremental_commits: 2500
  query_runs: 20
  window_limit: 500
  traversal_max_nodes: 25000

workload_shape:
  estimated_git_process_spawns: 14

phase_timing_ms:
  fixture_generation: 30409.586
  fixture_summary: 685.364
  repository_scan: 116.629
  full_index: 27064.861
  incremental_fixture_append: 530.396
  incremental_index: 7804.446
  query_benchmark: 25547.239
  in_memory_graph_hydration: 57.328
  in_memory_query_benchmark: 24.032
  core_end_to_end: 92262.391
  unattributed_core: 22.510

phase_memory_bytes:
  fixture_generation_rss: 4943872
  fixture_summary_rss: 4800512
  repository_scan_rss: 7385088
  full_index_rss: 12353536
  incremental_fixture_append_rss: 12189696
  incremental_index_rss: 13438976
  query_benchmark_rss: 13578240
  in_memory_graph_hydration_rss: 19693568
  in_memory_query_benchmark_rss: 20283392
  final_rss: 20287488
  peak_observed_rss: 20287488

dataset_counts:
  commits_in_db: 25001
  edges_in_db: 27500
  file_changes_in_db: 25002

full_index:
  duration_ms: 27064.861
  commits_indexed: 22501
  edges_indexed: 25000
  file_changes_indexed: 22502
  commit_throughput_per_sec: 831.373
  edge_throughput_per_sec: 923.707
  file_change_throughput_per_sec: 831.410

incremental_index:
  duration_ms: 7804.446
  commits_indexed: 2801
  edges_indexed: 2835
  file_changes_indexed: 2801
  commit_throughput_per_sec: 358.898
  edge_throughput_per_sec: 363.254
  file_change_throughput_per_sec: 358.898

query_latency_ms:
  commit_window: runs=20 min=12.992 median=16.859 p95=19.315 max=22.221 mean=16.780
  bfs_parents: runs=20 min=194.888 median=216.943 p95=270.120 max=275.880 mean=225.203
  dfs_parents: runs=20 min=219.962 median=308.316 p95=426.048 max=468.895 mean=314.886
  topological_parents: runs=20 min=228.271 median=267.739 p95=315.983 max=349.310 mean=267.894
  shortest_path_parents: runs=20 min=299.206 median=350.182 p95=457.631 max=513.067 mean=363.736
  shortest_path_both: runs=20 min=15.674 median=19.258 p95=26.749 max=27.321 mean=20.259

in_memory_adjacency_graph:
  nodes: 25001
  directed_edges: 27500

in_memory_adjacency_query_latency_ms:
  bfs_parents: runs=20 min=0.267 median=0.571 p95=0.730 max=0.730 mean=0.506
  shortest_path_parents: runs=20 min=0.305 median=0.615 p95=0.950 max=0.966 mean=0.592
  shortest_path_both: runs=20 min=0.013 median=0.023 p95=0.025 max=0.026 mean=0.020

in_memory_vs_sqlite_median_speedup:
  bfs_parents: 379.935x
  shortest_path_parents: 569.586x
  shortest_path_both: 848.361x
```
