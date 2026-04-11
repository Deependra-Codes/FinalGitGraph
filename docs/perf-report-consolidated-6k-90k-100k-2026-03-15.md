# Consolidated Perf Report (6k, 90k, 100k)

Date: 2026-03-15

## Related Architecture Proposal

- See [Architecture Overhaul Proposal (2026-03-15)](architecture-overhaul-proposal-2026-03-15.md) for the implementation plan targeting 9k+ throughput based on this baseline.

## Run Parameters

| Profile | Cycles | FeatureCommits | IncrementalCommits | QueryRuns | WindowLimit | MaxNodes | PreconditionCommitGraph | OutputPath |
|---|---:|---:|---:|---:|---:|---:|---|---|
| 6k | 600 | 6 | 600 | 20 | 300 | 6000 | true | tmp/perf-report-6k-full.txt |
| 90k | 9000 | 6 | 9000 | 20 | 1000 | 90000 | true | tmp/perf-report-90k-full.txt |
| 100k | 10000 | 6 | 10000 | 20 | 1200 | 100000 | true | tmp/perf-report-100k-full.txt |

## Key Metric Comparison

| Profile | core_end_to_end_ms | full_index_ms | incremental_index_ms | commit_throughput_per_sec | peak_observed_rss | ingest_file_diff_extract_ms | ingest_tx_commit_ms |
|---|---:|---:|---:|---:|---:|---:|---:|
| 6k | 13017.362 | 8586.699 | 501.309 | 628.996 | 16392192 | 265.908 | 90.080 |
| 90k | 196062.102 | 135916.019 | 6827.353 | 595.964 | 62930944 | 1858.442 | 1815.430 |
| 100k | 205731.797 | 149659.392 | 7775.853 | 601.372 | 67211264 | 3042.318 | 1667.192 |

## Full Raw Output: 6k

```text
=== GitGraph Lab Performance Report ===
fixture_root: C:\Users\deepe\AppData\Local\Temp\gitgraph-lab-perf-1773589056922536200
config:
  cycles: 600
  feature_commits_per_cycle: 6
  incremental_commits: 600
  query_runs: 20
  window_limit: 300
  traversal_max_nodes: 6000
  workload_profile_hint: custom

workload_shape:
  estimated_git_process_spawns: 16

phase_timing_ms:
  fixture_generation: 1745.558
  fixture_summary: 119.306
  repository_preconditioning: 147.669
  repository_scan: 82.536
  full_index: 8586.699
  incremental_fixture_append: 277.114
  incremental_index: 501.309
  file_change_extraction_drain: 0.000
  query_benchmark: 1361.825
  jit_file_change_enrichment: 142.158
  in_memory_graph_hydration: 12.897
  in_memory_query_benchmark: 3.855
  core_end_to_end: 13017.362
  unattributed_core: 36.436

phase_memory_bytes:
  fixture_generation_rss: 5251072
  fixture_summary_rss: 5263360
  repository_preconditioning_rss: 5263360
  repository_scan_rss: 7544832
  full_index_rss: 10104832
  incremental_fixture_append_rss: 10174464
  incremental_index_rss: 10620928
  file_change_extraction_drain_rss: 10620928
  query_benchmark_rss: 12845056
  jit_file_change_enrichment_rss: 14753792
  in_memory_graph_hydration_rss: 16384000
  in_memory_query_benchmark_rss: 16392192
  final_rss: 16392192
  peak_observed_rss: 16392192

dataset_counts:
  commits_in_db: 6001
  edges_in_db: 6600
  file_changes_in_db: 6002

file_change_stats_state_counts:
  pending: 5702
  ready: 300
  failed: 0

file_change_extraction:
  file_change_extraction_drain_ms: 0.000
  file_change_extraction_queue_depth: 0
  oldest_file_change_extraction_queue_age_seconds: 0
  file_change_extraction_drain_phase_timing_ms.queue_fetch: 0.000
  file_change_extraction_drain_phase_timing_ms.diff_extract: 0.000
  file_change_extraction_drain_phase_timing_ms.stage_insert: 0.000
  file_change_extraction_drain_phase_timing_ms.delete_old_rows: 0.000
  file_change_extraction_drain_phase_timing_ms.file_change_merge_upsert: 0.000
  file_change_extraction_drain_phase_timing_ms.enrichment_enqueue: 0.000
  file_change_extraction_drain_phase_timing_ms.queue_clear: 0.000
  file_change_extraction_drain_phase_timing_ms.tx_commit: 0.000
  file_change_extraction_drain_batches: 0
  file_change_extraction_drain_commits_per_batch_mean: 0.000
  file_change_extraction_drain_file_changes_per_batch_mean: 0.000

full_index:
  duration_ms: 8586.699
  commits_indexed: 5401
  edges_indexed: 6000
  file_changes_indexed: 5402
  commit_throughput_per_sec: 628.996
  edge_throughput_per_sec: 698.755
  file_change_throughput_per_sec: 629.113

incremental_index:
  duration_ms: 501.309
  commits_indexed: 600
  edges_indexed: 600
  file_changes_indexed: 600
  commit_throughput_per_sec: 1196.868
  edge_throughput_per_sec: 1196.868
  file_change_throughput_per_sec: 1196.868

ingestion_phase_timing_ms:
  full_commit_walk: 278.202
  full_commit_upsert: 55.014
  full_edge_materialize: 29.041
  full_file_diff_extract: 202.519
  full_file_diff_build: 197.596
  full_file_diff_rename_find: 0.359
  full_file_diff_delta_emit: 4.564
  full_extraction_dispatch_wait: 0.478
  full_extraction_worker_busy: 204.589
  full_extraction_reorder_buffer_max_depth: 2.000
  full_extraction_worker_errors: 0.000
  full_extraction_worker_aborts: 0.000
  full_file_change_delete: 0.000
  full_file_change_upsert: 84.797
  full_tx_commit: 77.014
  incremental_commit_walk: 231.264
  incremental_commit_upsert: 16.614
  incremental_edge_materialize: 4.051
  incremental_file_diff_extract: 63.389
  incremental_file_diff_build: 62.012
  incremental_file_diff_rename_find: 0.055
  incremental_file_diff_delta_emit: 1.322
  incremental_extraction_dispatch_wait: 0.190
  incremental_extraction_worker_busy: 63.698
  incremental_extraction_reorder_buffer_max_depth: 2.000
  incremental_extraction_worker_errors: 0.000
  incremental_extraction_worker_aborts: 0.000
  incremental_file_change_delete: 5.880
  incremental_file_change_upsert: 24.636
  incremental_tx_commit: 13.067
  ingest_commit_walk_ms: 509.465
  ingest_file_diff_extract_ms: 265.908
  ingest_file_diff_build_ms: 259.608
  ingest_file_diff_rename_find_ms: 0.414
  ingest_file_diff_delta_emit_ms: 5.886
  ingest_extraction_dispatch_wait_ms: 0.668
  ingest_extraction_worker_busy_ms: 268.287
  ingest_extraction_reorder_buffer_max_depth: 2.000
  ingest_extraction_worker_errors: 0.000
  ingest_extraction_worker_aborts: 0.000
  ingest_file_change_upsert_ms: 109.433
  ingest_tx_commit_ms: 90.080

query_latency_ms:
  commit_window: runs=20 min=8.378 median=9.672 p95=10.670 max=11.066 mean=9.644
  bfs_parents: runs=20 min=8.293 median=11.868 p95=13.700 max=13.926 mean=11.712
  dfs_parents: runs=20 min=5.501 median=11.863 p95=13.038 max=13.079 mean=11.108
  topological_parents: runs=20 min=6.795 median=13.712 p95=16.788 max=17.815 mean=13.112
  shortest_path_parents: runs=20 min=7.848 median=11.166 p95=12.337 max=12.556 mean=10.789
  shortest_path_both: runs=20 min=4.238 median=8.409 p95=10.406 max=11.151 mean=7.993

in_memory_adjacency_graph:
  nodes: 6001
  directed_edges: 6600

in_memory_adjacency_query_latency_ms:
  bfs_parents: runs=20 min=0.074 median=0.092 p95=0.097 max=0.119 mean=0.093
  shortest_path_parents: runs=20 min=0.080 median=0.082 p95=0.083 max=0.095 mean=0.082
  shortest_path_both: runs=20 min=0.005 median=0.005 p95=0.005 max=0.006 mean=0.005

in_memory_vs_legacy_relational_engine_median_speedup:
  bfs_parents: 128.725x
  shortest_path_parents: 137.012x
  shortest_path_both: 1648.765x

commit_throughput_diagnostics:
  combined_commits_indexed: 6001
  combined_index_duration_ms: 9088.008
  combined_commit_throughput_per_sec: 660.321
  effective_commit_throughput_with_drain_per_sec: 660.321
  end_to_end_commit_throughput_per_sec: 461.000
```

## Full Raw Output: 90k

```text
=== GitGraph Lab Performance Report ===
fixture_root: C:\Users\deepe\AppData\Local\Temp\gitgraph-lab-perf-1773589072323351700
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
  fixture_generation: 45323.824
  fixture_summary: 406.795
  repository_preconditioning: 1719.250
  repository_scan: 107.008
  full_index: 135916.019
  incremental_fixture_append: 636.914
  incremental_index: 6827.353
  file_change_extraction_drain: 0.000
  query_benchmark: 4289.003
  jit_file_change_enrichment: 398.926
  in_memory_graph_hydration: 161.305
  in_memory_query_benchmark: 134.913
  core_end_to_end: 196062.102
  unattributed_core: 140.792

phase_memory_bytes:
  fixture_generation_rss: 5185536
  fixture_summary_rss: 5206016
  repository_preconditioning_rss: 5197824
  repository_scan_rss: 7491584
  full_index_rss: 13545472
  incremental_fixture_append_rss: 14692352
  incremental_index_rss: 13889536
  file_change_extraction_drain_rss: 13889536
  query_benchmark_rss: 52088832
  jit_file_change_enrichment_rss: 50839552
  in_memory_graph_hydration_rss: 61681664
  in_memory_query_benchmark_rss: 62930944
  final_rss: 62930944
  peak_observed_rss: 62930944

dataset_counts:
  commits_in_db: 90001
  edges_in_db: 99000
  file_changes_in_db: 90002

file_change_stats_state_counts:
  pending: 89002
  ready: 1000
  failed: 0

file_change_extraction:
  file_change_extraction_drain_ms: 0.000
  file_change_extraction_queue_depth: 0
  oldest_file_change_extraction_queue_age_seconds: 0
  file_change_extraction_drain_phase_timing_ms.queue_fetch: 0.000
  file_change_extraction_drain_phase_timing_ms.diff_extract: 0.000
  file_change_extraction_drain_phase_timing_ms.stage_insert: 0.000
  file_change_extraction_drain_phase_timing_ms.delete_old_rows: 0.000
  file_change_extraction_drain_phase_timing_ms.file_change_merge_upsert: 0.000
  file_change_extraction_drain_phase_timing_ms.enrichment_enqueue: 0.000
  file_change_extraction_drain_phase_timing_ms.queue_clear: 0.000
  file_change_extraction_drain_phase_timing_ms.tx_commit: 0.000
  file_change_extraction_drain_batches: 0
  file_change_extraction_drain_commits_per_batch_mean: 0.000
  file_change_extraction_drain_file_changes_per_batch_mean: 0.000

full_index:
  duration_ms: 135916.019
  commits_indexed: 81001
  edges_indexed: 90000
  file_changes_indexed: 81002
  commit_throughput_per_sec: 595.964
  edge_throughput_per_sec: 662.174
  file_change_throughput_per_sec: 595.971

incremental_index:
  duration_ms: 6827.353
  commits_indexed: 9000
  edges_indexed: 9000
  file_changes_indexed: 9000
  commit_throughput_per_sec: 1318.227
  edge_throughput_per_sec: 1318.227
  file_change_throughput_per_sec: 1318.227

ingestion_phase_timing_ms:
  full_commit_walk: 1250.280
  full_commit_upsert: 2549.417
  full_edge_materialize: 1848.515
  full_file_diff_extract: 1418.043
  full_file_diff_build: 1384.451
  full_file_diff_rename_find: 3.729
  full_file_diff_delta_emit: 29.862
  full_extraction_dispatch_wait: 5.731
  full_extraction_worker_busy: 1438.068
  full_extraction_reorder_buffer_max_depth: 4.000
  full_extraction_worker_errors: 0.000
  full_extraction_worker_aborts: 0.000
  full_file_change_delete: 0.000
  full_file_change_upsert: 2617.829
  full_tx_commit: 1172.210
  incremental_commit_walk: 2371.778
  incremental_commit_upsert: 879.082
  incremental_edge_materialize: 186.017
  incremental_file_diff_extract: 440.399
  incremental_file_diff_build: 432.693
  incremental_file_diff_rename_find: 0.852
  incremental_file_diff_delta_emit: 6.854
  incremental_extraction_dispatch_wait: 1.004
  incremental_extraction_worker_busy: 444.445
  incremental_extraction_reorder_buffer_max_depth: 1.000
  incremental_extraction_worker_errors: 0.000
  incremental_extraction_worker_aborts: 0.000
  incremental_file_change_delete: 362.115
  incremental_file_change_upsert: 965.173
  incremental_tx_commit: 643.219
  ingest_commit_walk_ms: 3622.057
  ingest_file_diff_extract_ms: 1858.442
  ingest_file_diff_build_ms: 1817.144
  ingest_file_diff_rename_find_ms: 4.581
  ingest_file_diff_delta_emit_ms: 36.716
  ingest_extraction_dispatch_wait_ms: 6.735
  ingest_extraction_worker_busy_ms: 1882.514
  ingest_extraction_reorder_buffer_max_depth: 4.000
  ingest_extraction_worker_errors: 0.000
  ingest_extraction_worker_aborts: 0.000
  ingest_file_change_upsert_ms: 3583.002
  ingest_tx_commit_ms: 1815.430

query_latency_ms:
  commit_window: runs=20 min=6.960 median=10.411 p95=13.339 max=14.528 mean=9.741
  bfs_parents: runs=20 min=25.865 median=34.416 p95=37.948 max=39.161 mean=34.469
  dfs_parents: runs=20 min=23.284 median=42.183 p95=51.548 max=52.320 mean=40.392
  topological_parents: runs=20 min=47.358 median=68.012 p95=76.064 max=76.392 mean=66.003
  shortest_path_parents: runs=20 min=21.469 median=36.925 p95=42.773 max=45.987 mean=34.536
  shortest_path_both: runs=20 min=7.449 median=9.361 p95=10.086 max=10.423 mean=9.024

in_memory_adjacency_graph:
  nodes: 90001
  directed_edges: 99000

in_memory_adjacency_query_latency_ms:
  bfs_parents: runs=20 min=2.473 median=3.441 p95=4.008 max=4.250 mean=3.388
  shortest_path_parents: runs=20 min=2.309 median=2.974 p95=3.669 max=3.833 mean=2.979
  shortest_path_both: runs=20 min=0.062 median=0.066 p95=0.067 max=0.080 mean=0.065

in_memory_vs_legacy_relational_engine_median_speedup:
  bfs_parents: 10.003x
  shortest_path_parents: 12.416x
  shortest_path_both: 142.263x

commit_throughput_diagnostics:
  combined_commits_indexed: 90001
  combined_index_duration_ms: 142743.372
  combined_commit_throughput_per_sec: 630.509
  effective_commit_throughput_with_drain_per_sec: 630.509
  end_to_end_commit_throughput_per_sec: 459.043
```

## Full Raw Output: 100k

```text
=== GitGraph Lab Performance Report ===
fixture_root: C:\Users\deepe\AppData\Local\Temp\gitgraph-lab-perf-1773589281819037600
config:
  cycles: 10000
  feature_commits_per_cycle: 6
  incremental_commits: 10000
  query_runs: 20
  window_limit: 1200
  traversal_max_nodes: 100000
  workload_profile_hint: 90k

workload_shape:
  estimated_git_process_spawns: 16

phase_timing_ms:
  fixture_generation: 39107.615
  fixture_summary: 489.251
  repository_preconditioning: 2474.755
  repository_scan: 143.384
  full_index: 149659.392
  incremental_fixture_append: 629.292
  incremental_index: 7775.853
  file_change_extraction_drain: 0.000
  query_benchmark: 4661.392
  jit_file_change_enrichment: 389.455
  in_memory_graph_hydration: 160.981
  in_memory_query_benchmark: 107.366
  core_end_to_end: 205731.797
  unattributed_core: 133.061

phase_memory_bytes:
  fixture_generation_rss: 5193728
  fixture_summary_rss: 5201920
  repository_preconditioning_rss: 5206016
  repository_scan_rss: 7471104
  full_index_rss: 12386304
  incremental_fixture_append_rss: 12398592
  incremental_index_rss: 13344768
  file_change_extraction_drain_rss: 13344768
  query_benchmark_rss: 54980608
  jit_file_change_enrichment_rss: 53440512
  in_memory_graph_hydration_rss: 65884160
  in_memory_query_benchmark_rss: 67211264
  final_rss: 67211264
  peak_observed_rss: 67211264

dataset_counts:
  commits_in_db: 100001
  edges_in_db: 110000
  file_changes_in_db: 100002

file_change_stats_state_counts:
  pending: 98802
  ready: 1200
  failed: 0

file_change_extraction:
  file_change_extraction_drain_ms: 0.000
  file_change_extraction_queue_depth: 0
  oldest_file_change_extraction_queue_age_seconds: 0
  file_change_extraction_drain_phase_timing_ms.queue_fetch: 0.000
  file_change_extraction_drain_phase_timing_ms.diff_extract: 0.000
  file_change_extraction_drain_phase_timing_ms.stage_insert: 0.000
  file_change_extraction_drain_phase_timing_ms.delete_old_rows: 0.000
  file_change_extraction_drain_phase_timing_ms.file_change_merge_upsert: 0.000
  file_change_extraction_drain_phase_timing_ms.enrichment_enqueue: 0.000
  file_change_extraction_drain_phase_timing_ms.queue_clear: 0.000
  file_change_extraction_drain_phase_timing_ms.tx_commit: 0.000
  file_change_extraction_drain_batches: 0
  file_change_extraction_drain_commits_per_batch_mean: 0.000
  file_change_extraction_drain_file_changes_per_batch_mean: 0.000

full_index:
  duration_ms: 149659.392
  commits_indexed: 90001
  edges_indexed: 100000
  file_changes_indexed: 90002
  commit_throughput_per_sec: 601.372
  edge_throughput_per_sec: 668.184
  file_change_throughput_per_sec: 601.379

incremental_index:
  duration_ms: 7775.853
  commits_indexed: 10000
  edges_indexed: 10000
  file_changes_indexed: 10000
  commit_throughput_per_sec: 1286.033
  edge_throughput_per_sec: 1286.033
  file_change_throughput_per_sec: 1286.033

ingestion_phase_timing_ms:
  full_commit_walk: 3836.441
  full_commit_upsert: 5408.710
  full_edge_materialize: 1832.783
  full_file_diff_extract: 2560.447
  full_file_diff_build: 2498.636
  full_file_diff_rename_find: 6.780
  full_file_diff_delta_emit: 55.031
  full_extraction_dispatch_wait: 11.885
  full_extraction_worker_busy: 2597.984
  full_extraction_reorder_buffer_max_depth: 5.000
  full_extraction_worker_errors: 0.000
  full_extraction_worker_aborts: 0.000
  full_file_change_delete: 0.000
  full_file_change_upsert: 5411.414
  full_tx_commit: 1097.087
  incremental_commit_walk: 2877.638
  incremental_commit_upsert: 1006.613
  incremental_edge_materialize: 164.217
  incremental_file_diff_extract: 481.870
  incremental_file_diff_build: 473.425
  incremental_file_diff_rename_find: 0.819
  incremental_file_diff_delta_emit: 7.626
  incremental_extraction_dispatch_wait: 1.727
  incremental_extraction_worker_busy: 486.292
  incremental_extraction_reorder_buffer_max_depth: 1.000
  incremental_extraction_worker_errors: 0.000
  incremental_extraction_worker_aborts: 0.000
  incremental_file_change_delete: 377.812
  incremental_file_change_upsert: 1016.062
  incremental_tx_commit: 570.105
  ingest_commit_walk_ms: 6714.079
  ingest_file_diff_extract_ms: 3042.318
  ingest_file_diff_build_ms: 2972.062
  ingest_file_diff_rename_find_ms: 7.599
  ingest_file_diff_delta_emit_ms: 62.657
  ingest_extraction_dispatch_wait_ms: 13.612
  ingest_extraction_worker_busy_ms: 3084.276
  ingest_extraction_reorder_buffer_max_depth: 5.000
  ingest_extraction_worker_errors: 0.000
  ingest_extraction_worker_aborts: 0.000
  ingest_file_change_upsert_ms: 6427.476
  ingest_tx_commit_ms: 1667.192

query_latency_ms:
  commit_window: runs=20 min=8.434 median=11.710 p95=12.758 max=13.060 mean=11.446
  bfs_parents: runs=20 min=23.159 median=40.735 p95=44.962 max=45.260 mean=38.134
  dfs_parents: runs=20 min=38.042 median=47.582 p95=52.880 max=53.198 mean=46.329
  topological_parents: runs=20 min=56.844 median=66.453 p95=70.877 max=71.422 mean=65.197
  shortest_path_parents: runs=20 min=25.852 median=40.945 p95=42.351 max=43.181 mean=39.365
  shortest_path_both: runs=20 min=7.579 median=8.700 p95=9.712 max=10.083 mean=8.744

in_memory_adjacency_graph:
  nodes: 100001
  directed_edges: 110000

in_memory_adjacency_query_latency_ms:
  bfs_parents: runs=20 min=1.625 median=2.637 p95=3.665 max=3.688 mean=2.577
  shortest_path_parents: runs=20 min=1.396 median=2.644 p95=3.721 max=3.903 mean=2.439
  shortest_path_both: runs=20 min=0.077 median=0.077 p95=0.078 max=0.098 mean=0.078

in_memory_vs_legacy_relational_engine_median_speedup:
  bfs_parents: 15.447x
  shortest_path_parents: 15.488x
  shortest_path_both: 112.547x

commit_throughput_diagnostics:
  combined_commits_indexed: 100001
  combined_index_duration_ms: 157435.245
  combined_commit_throughput_per_sec: 635.188
  effective_commit_throughput_with_drain_per_sec: 635.188
  end_to_end_commit_throughput_per_sec: 486.075
```


