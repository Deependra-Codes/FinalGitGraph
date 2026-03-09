# Phase B Consolidated Perf + Test Report (6k / 25k / 90k)

Date: 2026-03-09
Owner: Desktop Core
Context: Post-Phase-B SQLite fallback staging + plan-guard integration

## 1. Commands Executed

### Quality and Test Gates

```powershell
pnpm run guardrails
pnpm run dev:gate:fast
pnpm run dev:gate
```

### Performance Reports

```powershell
powershell -ExecutionPolicy Bypass -File ./scripts/perf-report.ps1 -Cycles 600 -FeatureCommits 6 -IncrementalCommits 600 -QueryRuns 20 -WindowLimit 300 -MaxNodes 6000 -OutputPath tmp/perf-report-6k.txt
pnpm run perf:report:25k
pnpm run perf:report:90k
```

## 2. Gate/Test Results

1. `guardrails`: passed.
2. `dev:gate:fast` (Docker backend): passed.
3. `dev:gate` (Docker backend): passed.

`dev:gate` test summary:

1. Rust unit/integration/doc suite: `31 passed`, `0 failed`, `1 ignored`.
2. `indexer_smoke` integration suite: `11 passed`, `0 failed`, `0 ignored`.

`dev:gate` test names and outcomes:

```text
running 32 tests
test adapters::db::tests::enrichment_retry_cooldown_gates_pending_pickup ... ok
test adapters::db::tests::indexing_checkpoint_roundtrip ... ok
test adapters::git::tests::max_file_size_probe_is_skipped_for_large_delta_sets ... ok
test adapters::git::tests::object_cache_size_scales_and_stays_bounded ... ok
test adapters::git::tests::parallel_activation_supports_incremental_batches ... ok
test adapters::git::tests::parallel_file_change_plan_is_bounded ... ok
test adapters::git::tests::parallel_worker_count_respects_bounds_and_overrides ... ok
test adapters::git::tests::parse_hex_object_id_validates_input ... ok
test adapters::git::tests::rename_detection_bounds_are_configured ... ok
test adapters::git::tests::rename_detection_runs_only_when_diff_shape_requires_it ... ok
test adapters::db::tests::load_repository_id_by_path_roundtrip ... ok
test adapters::db::tests::commits_and_edges_have_repo_scoped_primary_keys ... ok
test adapters::git::tests::rename_threshold_is_adaptive_by_delta_count_and_file_size ... ok
test adapters::db::tests::delete_file_changes_for_commits_removes_requested_shas_only ... ok
test application::graph_queries::tests::bfs_mode_switches_to_pull_for_large_frontier ... ok
test application::graph_queries::tests::perf_budget_direction_optimizing_bfs ... ignored, performance budget gate runs this test in dedicated CI step
test adapters::db::tests::enqueue_resets_attempts_for_reindexed_rows ... ok
test adapters::db::tests::enrichment_failures_retry_until_terminal_failed ... ok
test application::indexer_pipeline::tests::pipeline_config_uses_bounded_worker_and_queue_defaults ... ok
test adapters::db::tests::neighbor_batch_lookup_returns_repo_scoped_neighbors ... ok
test adapters::git::tests::unknown_repo_returns_error ... ok
test adapters::db::tests::migrations_are_idempotent ... ok
test adapters::db::tests::sqlite_graph_query_plan_guard_accepts_indexed_staged_queries ... ok
test adapters::db::tests::enqueue_file_changes_handles_parameter_chunk_boundaries ... ok
test adapters::db::tests::upsert_file_changes_handles_parameter_chunk_boundaries ... ok
test application::indexer_pipeline::tests::pipeline_join_succeeds_without_in_flight_work ... ok
test application::indexer_pipeline::tests::pipeline_propagates_worker_failure_for_invalid_sha ... ok
test adapters::db::tests::neighbor_queries_handle_chunk_boundaries ... ok
test application::graph_queries::tests::bfs_truncation_respects_low_max_nodes_without_behavior_regression ... ok
test application::query_graph_cache::tests::cache_rehydrates_when_repository_indexed_at_changes ... ok
test application::query_graph_cache::tests::query_graph_hydration_serves_traversal_and_shortest_path ... ok
test application::indexer_pipeline::tests::pipeline_orders_batches_by_sequence_even_when_worker_completion_differs ... ok

test result: ok. 31 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out

running 11 tests
test scan_and_index_repository_smoke_test ... ok
test ingestion_diff_subphase_telemetry_is_consistent_for_zero_and_single_delta_commits ... ok
test rename_commit_is_recorded_as_rename_change ... ok
test file_change_stats_are_deferred_and_jit_enrichable ... ok
test graph_traversal_and_shortest_path_work ... ok
test branching_graph_queries_work ... ok
test jit_enrichment_is_commit_scoped ... ok
test commit_window_has_more_is_correct_at_max_limit_boundary ... ok
test incremental_indexing_and_commit_window_work ... ok
test full_index_results_are_stable_across_pipeline_worker_counts ... ok
test indexing_large_history_keeps_file_change_ingestion_correct ... ok

test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

## 3. Wrapper Timing (Build vs Execute)

| Profile | Build (s) | Execute (s) | Total (s) |
|---|---:|---:|---:|
| 6k | 0.961 | 8.062 | 9.024 |
| 25k | 0.672 | 25.535 | 26.207 |
| 90k | 0.499 | 113.814 | 114.312 |

## 4. Metric Definitions (Deep Dictionary)

General:

1. `fixture_root`: temporary repository root used by perf harness.
2. `config.cycles`: synthetic history cycles used to generate graph workload.
3. `config.feature_commits_per_cycle`: commits per feature branch cycle.
4. `config.incremental_commits`: commits appended before incremental indexing run.
5. `config.query_runs`: number of repeated samples per query family.
6. `config.window_limit`: commit-window page size.
7. `config.traversal_max_nodes`: max traversal cap used by graph queries.
8. `config.workload_profile_hint`: profile label emitted by perf binary.
9. `workload_shape.estimated_git_process_spawns`: expected spawned git process count in this scenario.

Phase timing:

1. `phase_timing_ms.fixture_generation`: time to generate synthetic git fixture history.
2. `phase_timing_ms.fixture_summary`: time to compute fixture summary stats.
3. `phase_timing_ms.repository_preconditioning`: optional preconditioning pass before scan/index.
4. `phase_timing_ms.repository_scan`: time for repo metadata scan command.
5. `phase_timing_ms.full_index`: full indexing duration.
6. `phase_timing_ms.incremental_fixture_append`: time adding incremental commits to fixture.
7. `phase_timing_ms.incremental_index`: incremental indexing duration.
8. `phase_timing_ms.query_benchmark`: SQLite-backed query benchmark phase duration.
9. `phase_timing_ms.jit_file_change_enrichment`: selective line-stat enrichment phase duration.
10. `phase_timing_ms.in_memory_graph_hydration`: time to load in-memory adjacency graph.
11. `phase_timing_ms.in_memory_query_benchmark`: in-memory graph query benchmark duration.
12. `phase_timing_ms.core_end_to_end`: total harness core duration.
13. `phase_timing_ms.unattributed_core`: remainder timing not attributed to named subphases.

Phase memory:

1. `phase_memory_bytes.*_rss`: RSS sample after each named phase.
2. `phase_memory_bytes.final_rss`: final sampled RSS at end of run.
3. `phase_memory_bytes.peak_observed_rss`: highest RSS observed during run.

Dataset and file-change state:

1. `dataset_counts.commits_in_db`: persisted commits count.
2. `dataset_counts.edges_in_db`: persisted edge count.
3. `dataset_counts.file_changes_in_db`: persisted file-change count.
4. `file_change_stats_state_counts.pending`: file changes pending line-stat enrichment.
5. `file_change_stats_state_counts.ready`: file changes with line stats available.
6. `file_change_stats_state_counts.failed`: terminal failed enrichment rows.

Index throughput:

1. `full_index.duration_ms`: full indexing elapsed time.
2. `full_index.commits_indexed`: commits written by full index phase.
3. `full_index.edges_indexed`: edges materialized in full index.
4. `full_index.file_changes_indexed`: file-change rows upserted in full index.
5. `full_index.commit_throughput_per_sec`: full-index commit rows per second.
6. `full_index.edge_throughput_per_sec`: full-index edge rows per second.
7. `full_index.file_change_throughput_per_sec`: full-index file-change rows per second.
8. `incremental_index.*`: same semantics as `full_index.*` but for incremental pass.

Ingestion subphase telemetry:

1. `ingestion_phase_timing_ms.full_commit_walk`: full-pass revision walk time.
2. `ingestion_phase_timing_ms.full_commit_upsert`: full-pass commit upsert time.
3. `ingestion_phase_timing_ms.full_edge_materialize`: full-pass edge materialization time.
4. `ingestion_phase_timing_ms.full_file_diff_extract`: full-pass diff extraction total time.
5. `ingestion_phase_timing_ms.full_file_diff_build`: full-pass diff build time.
6. `ingestion_phase_timing_ms.full_file_diff_rename_find`: full-pass rename detection time.
7. `ingestion_phase_timing_ms.full_file_diff_delta_emit`: full-pass delta emission time.
8. `ingestion_phase_timing_ms.full_extraction_dispatch_wait`: producer/consumer dispatch wait.
9. `ingestion_phase_timing_ms.full_extraction_worker_busy`: worker busy-time aggregate.
10. `ingestion_phase_timing_ms.full_extraction_reorder_buffer_max_depth`: max reorder buffer depth.
11. `ingestion_phase_timing_ms.full_extraction_worker_errors`: worker errors observed.
12. `ingestion_phase_timing_ms.full_extraction_worker_aborts`: worker abort count.
13. `ingestion_phase_timing_ms.full_file_change_delete`: full-pass delete phase time.
14. `ingestion_phase_timing_ms.full_file_change_upsert`: full-pass file-change upsert time.
15. `ingestion_phase_timing_ms.full_tx_commit`: full-pass transaction commit time.
16. `ingestion_phase_timing_ms.incremental_*`: same semantics for incremental pass.
17. `ingestion_phase_timing_ms.ingest_commit_walk_ms`: aggregate commit walk time across lanes.
18. `ingestion_phase_timing_ms.ingest_file_diff_extract_ms`: aggregate diff extract time.
19. `ingestion_phase_timing_ms.ingest_file_diff_build_ms`: aggregate diff build time.
20. `ingestion_phase_timing_ms.ingest_file_diff_rename_find_ms`: aggregate rename time.
21. `ingestion_phase_timing_ms.ingest_file_diff_delta_emit_ms`: aggregate delta emit time.
22. `ingestion_phase_timing_ms.ingest_extraction_dispatch_wait_ms`: aggregate dispatch wait.
23. `ingestion_phase_timing_ms.ingest_extraction_worker_busy_ms`: aggregate worker busy time.
24. `ingestion_phase_timing_ms.ingest_extraction_reorder_buffer_max_depth`: aggregate max reorder depth.
25. `ingestion_phase_timing_ms.ingest_extraction_worker_errors`: aggregate worker errors.
26. `ingestion_phase_timing_ms.ingest_extraction_worker_aborts`: aggregate worker aborts.
27. `ingestion_phase_timing_ms.ingest_file_change_upsert_ms`: aggregate file-change upsert time.
28. `ingestion_phase_timing_ms.ingest_tx_commit_ms`: aggregate transaction commit time.

Query latency:

1. `query_latency_ms.<query_name>` reports `runs`, `min`, `median`, `p95`, `max`, `mean`.
2. `commit_window`: paginated commit-window query latency.
3. `bfs_parents`: BFS traversal in parent direction.
4. `dfs_parents`: DFS traversal in parent direction.
5. `topological_parents`: topological traversal in parent direction.
6. `shortest_path_parents`: shortest path constrained to parent direction.
7. `shortest_path_both`: bidirectional shortest path mode.

In-memory adjacency:

1. `in_memory_adjacency_graph.nodes`: nodes loaded into memory graph cache.
2. `in_memory_adjacency_graph.directed_edges`: directed edges loaded into memory graph cache.
3. `in_memory_adjacency_query_latency_ms.*`: in-memory query latency stats with same tuple semantics.
4. `in_memory_vs_sqlite_median_speedup.*`: SQLite median divided by in-memory median.

## 5. Consolidated Metrics (6k / 25k / 90k)

### Core Profile Summary

| Metric | 6k | 25k | 90k |
|---|---:|---:|---:|
| commits_in_db | 6001 | 25001 | 90001 |
| edges_in_db | 6600 | 27500 | 99000 |
| file_changes_in_db | 6002 | 25002 | 90002 |
| full_index_ms | 1040.803 | 2812.669 | 17311.480 |
| incremental_index_ms | 547.294 | 1732.711 | 7891.085 |
| query_benchmark_ms | 1274.885 | 1870.977 | 3813.244 |
| jit_file_change_enrichment_ms | 122.252 | 164.663 | 420.855 |
| in_memory_graph_hydration_ms | 11.361 | 24.491 | 339.007 |
| peak_observed_rss_bytes | 16257024 | 27824128 | 62287872 |

### Query Medians (ms)

| Query | 6k | 25k | 90k |
|---|---:|---:|---:|
| commit_window | 8.656 | 9.864 | 9.237 |
| bfs_parents | 10.331 | 17.458 | 40.370 |
| dfs_parents | 9.973 | 17.634 | 41.250 |
| topological_parents | 11.498 | 18.070 | 47.339 |
| shortest_path_parents | 8.762 | 12.098 | 26.945 |
| shortest_path_both | 9.034 | 6.281 | 8.968 |

### In-Memory Medians (ms)

| Query | 6k | 25k | 90k |
|---|---:|---:|---:|
| bfs_parents | 0.076 | 0.207 | 4.969 |
| shortest_path_parents | 0.116 | 0.232 | 4.661 |
| shortest_path_both | 0.005 | 0.009 | 0.057 |

### In-Memory vs SQLite Median Speedup

| Query | 6k | 25k | 90k |
|---|---:|---:|---:|
| bfs_parents | 135.573x | 84.176x | 8.124x |
| shortest_path_parents | 75.469x | 52.238x | 5.780x |
| shortest_path_both | 1963.826x | 668.202x | 158.442x |

### Topological Share of Query-Mix Median

Mix definition: `commit_window + bfs_parents + dfs_parents + topological_parents + shortest_path_parents`

| Metric | 6k | 25k | 90k |
|---|---:|---:|---:|
| query_mix_median_sum_ms | 49.220 | 75.124 | 165.141 |
| topological_share_pct | 23.36% | 24.05% | 28.67% |

## 6. Comparison vs Previous Final Artifacts (2026-03-07)

Reference baselines:

1. `docs/perf-report-25k-final.md`
2. `docs/perf-report-90k-final.md`

| Metric | 25k old | 25k new | Delta | 90k old | 90k new | Delta |
|---|---:|---:|---:|---:|---:|---:|
| topological_parents median (ms) | 267.739 | 18.070 | -93.25% | 453.193 | 47.339 | -89.55% |
| query_benchmark phase (ms) | 25547.239 | 1870.977 | -92.68% | 42110.818 | 3813.244 | -90.94% |

## 7. Full Raw Outputs

### 6k Raw Output (`tmp/perf-report-6k.txt`)

```text
=== GitGraph Lab Performance Report ===
fixture_root: C:\Users\deepe\AppData\Local\Temp\gitgraph-lab-perf-1773049551372178300
config:
  cycles: 600
  feature_commits_per_cycle: 6
  incremental_commits: 600
  query_runs: 20
  window_limit: 300
  traversal_max_nodes: 6000
  workload_profile_hint: custom

workload_shape:
  estimated_git_process_spawns: 14

phase_timing_ms:
  fixture_generation: 3091.191
  fixture_summary: 248.473
  repository_preconditioning: 0.000
  repository_scan: 162.877
  full_index: 1040.803
  incremental_fixture_append: 413.797
  incremental_index: 547.294
  query_benchmark: 1274.885
  jit_file_change_enrichment: 122.252
  in_memory_graph_hydration: 11.361
  in_memory_query_benchmark: 4.578
  core_end_to_end: 6937.246
  unattributed_core: 19.735

phase_memory_bytes:
  fixture_generation_rss: 5214208
  fixture_summary_rss: 5226496
  repository_preconditioning_rss: 5226496
  repository_scan_rss: 7585792
  full_index_rss: 10088448
  incremental_fixture_append_rss: 10010624
  incremental_index_rss: 10018816
  query_benchmark_rss: 12828672
  jit_file_change_enrichment_rss: 14434304
  in_memory_graph_hydration_rss: 16203776
  in_memory_query_benchmark_rss: 16252928
  final_rss: 16257024
  peak_observed_rss: 16257024

dataset_counts:
  commits_in_db: 6001
  edges_in_db: 6600
  file_changes_in_db: 6002

file_change_stats_state_counts:
  pending: 5702
  ready: 300
  failed: 0

full_index:
  duration_ms: 1040.803
  commits_indexed: 5401
  edges_indexed: 6000
  file_changes_indexed: 5402
  commit_throughput_per_sec: 5189.265
  edge_throughput_per_sec: 5764.782
  file_change_throughput_per_sec: 5190.225

incremental_index:
  duration_ms: 547.294
  commits_indexed: 600
  edges_indexed: 600
  file_changes_indexed: 600
  commit_throughput_per_sec: 1096.303
  edge_throughput_per_sec: 1096.303
  file_change_throughput_per_sec: 1096.303

ingestion_phase_timing_ms:
  full_commit_walk: 381.185
  full_commit_upsert: 90.452
  full_edge_materialize: 35.451
  full_file_diff_extract: 253.092
  full_file_diff_build: 246.770
  full_file_diff_rename_find: 0.536
  full_file_diff_delta_emit: 5.786
  full_extraction_dispatch_wait: 0.781
  full_extraction_worker_busy: 256.274
  full_extraction_reorder_buffer_max_depth: 3.000
  full_extraction_worker_errors: 0.000
  full_extraction_worker_aborts: 0.000
  full_file_change_delete: 0.000
  full_file_change_upsert: 156.457
  full_tx_commit: 139.315
  incremental_commit_walk: 265.679
  incremental_commit_upsert: 11.852
  incremental_edge_materialize: 3.799
  incremental_file_diff_extract: 43.281
  incremental_file_diff_build: 42.157
  incremental_file_diff_rename_find: 0.051
  incremental_file_diff_delta_emit: 1.073
  incremental_extraction_dispatch_wait: 0.051
  incremental_extraction_worker_busy: 43.603
  incremental_extraction_reorder_buffer_max_depth: 1.000
  incremental_extraction_worker_errors: 0.000
  incremental_extraction_worker_aborts: 0.000
  incremental_file_change_delete: 8.311
  incremental_file_change_upsert: 18.160
  incremental_tx_commit: 12.664
  ingest_commit_walk_ms: 646.864
  ingest_file_diff_extract_ms: 296.373
  ingest_file_diff_build_ms: 288.927
  ingest_file_diff_rename_find_ms: 0.588
  ingest_file_diff_delta_emit_ms: 6.859
  ingest_extraction_dispatch_wait_ms: 0.832
  ingest_extraction_worker_busy_ms: 299.877
  ingest_extraction_reorder_buffer_max_depth: 3.000
  ingest_extraction_worker_errors: 0.000
  ingest_extraction_worker_aborts: 0.000
  ingest_file_change_upsert_ms: 174.617
  ingest_tx_commit_ms: 151.979

query_latency_ms:
  commit_window: runs=20 min=5.611 median=8.656 p95=11.520 max=12.245 mean=8.702
  bfs_parents: runs=20 min=6.598 median=10.331 p95=14.155 max=17.329 mean=10.932
  dfs_parents: runs=20 min=6.481 median=9.973 p95=11.765 max=11.797 mean=9.318
  topological_parents: runs=20 min=7.794 median=11.498 p95=13.737 max=14.072 mean=11.469
  shortest_path_parents: runs=20 min=6.870 median=8.762 p95=12.009 max=12.408 mean=9.398
  shortest_path_both: runs=20 min=6.937 median=9.034 p95=14.518 max=14.584 mean=9.644

in_memory_adjacency_graph:
  nodes: 6001
  directed_edges: 6600

in_memory_adjacency_query_latency_ms:
  bfs_parents: runs=20 min=0.059 median=0.076 p95=0.086 max=0.092 mean=0.075
  shortest_path_parents: runs=20 min=0.079 median=0.116 p95=0.278 max=0.340 mean=0.132
  shortest_path_both: runs=20 min=0.004 median=0.005 p95=0.005 max=0.005 mean=0.005

in_memory_vs_sqlite_median_speedup:
  bfs_parents: 135.573x
  shortest_path_parents: 75.469x
  shortest_path_both: 1963.826x
```

### 25k Raw Output (`tmp/perf-report-25k.txt`)

```text
=== GitGraph Lab Performance Report ===
fixture_root: C:\Users\deepe\AppData\Local\Temp\gitgraph-lab-perf-1773049568011736600
config:
  cycles: 2500
  feature_commits_per_cycle: 6
  incremental_commits: 2500
  query_runs: 20
  window_limit: 500
  traversal_max_nodes: 25000
  workload_profile_hint: 25k

workload_shape:
  estimated_git_process_spawns: 14

phase_timing_ms:
  fixture_generation: 15371.602
  fixture_summary: 263.405
  repository_preconditioning: 0.000
  repository_scan: 142.578
  full_index: 2812.669
  incremental_fixture_append: 393.241
  incremental_index: 1732.711
  query_benchmark: 1870.977
  jit_file_change_enrichment: 164.663
  in_memory_graph_hydration: 24.491
  in_memory_query_benchmark: 10.749
  core_end_to_end: 22825.895
  unattributed_core: 38.809

phase_memory_bytes:
  fixture_generation_rss: 5218304
  fixture_summary_rss: 5230592
  repository_preconditioning_rss: 5230592
  repository_scan_rss: 7536640
  full_index_rss: 11051008
  incremental_fixture_append_rss: 10997760
  incremental_index_rss: 11948032
  query_benchmark_rss: 23011328
  jit_file_change_enrichment_rss: 21950464
  in_memory_graph_hydration_rss: 27672576
  in_memory_query_benchmark_rss: 27820032
  final_rss: 27824128
  peak_observed_rss: 27824128

dataset_counts:
  commits_in_db: 25001
  edges_in_db: 27500
  file_changes_in_db: 25002

file_change_stats_state_counts:
  pending: 24502
  ready: 500
  failed: 0

full_index:
  duration_ms: 2812.669
  commits_indexed: 22501
  edges_indexed: 25000
  file_changes_indexed: 22502
  commit_throughput_per_sec: 7999.874
  edge_throughput_per_sec: 8888.354
  file_change_throughput_per_sec: 8000.229

incremental_index:
  duration_ms: 1732.711
  commits_indexed: 2500
  edges_indexed: 2500
  file_changes_indexed: 2500
  commit_throughput_per_sec: 1442.826
  edge_throughput_per_sec: 1442.826
  file_change_throughput_per_sec: 1442.826

ingestion_phase_timing_ms:
  full_commit_walk: 832.641
  full_commit_upsert: 421.423
  full_edge_materialize: 217.514
  full_file_diff_extract: 549.787
  full_file_diff_build: 535.317
  full_file_diff_rename_find: 1.465
  full_file_diff_delta_emit: 13.005
  full_extraction_dispatch_wait: 2.355
  full_extraction_worker_busy: 557.630
  full_extraction_reorder_buffer_max_depth: 2.000
  full_extraction_worker_errors: 0.000
  full_extraction_worker_aborts: 0.000
  full_file_change_delete: 0.000
  full_file_change_upsert: 486.837
  full_tx_commit: 290.404
  incremental_commit_walk: 783.771
  incremental_commit_upsert: 96.499
  incremental_edge_materialize: 22.486
  incremental_file_diff_extract: 95.449
  incremental_file_diff_build: 93.563
  incremental_file_diff_rename_find: 0.143
  incremental_file_diff_delta_emit: 1.742
  incremental_extraction_dispatch_wait: 0.170
  incremental_extraction_worker_busy: 96.271
  incremental_extraction_reorder_buffer_max_depth: 1.000
  incremental_extraction_worker_errors: 0.000
  incremental_extraction_worker_aborts: 0.000
  incremental_file_change_delete: 70.328
  incremental_file_change_upsert: 165.101
  incremental_tx_commit: 194.114
  ingest_commit_walk_ms: 1616.412
  ingest_file_diff_extract_ms: 645.236
  ingest_file_diff_build_ms: 628.880
  ingest_file_diff_rename_find_ms: 1.609
  ingest_file_diff_delta_emit_ms: 14.748
  ingest_extraction_dispatch_wait_ms: 2.525
  ingest_extraction_worker_busy_ms: 653.901
  ingest_extraction_reorder_buffer_max_depth: 2.000
  ingest_extraction_worker_errors: 0.000
  ingest_extraction_worker_aborts: 0.000
  ingest_file_change_upsert_ms: 651.938
  ingest_tx_commit_ms: 484.518

query_latency_ms:
  commit_window: runs=20 min=7.646 median=9.864 p95=13.938 max=14.964 mean=10.042
  bfs_parents: runs=20 min=9.475 median=17.458 p95=24.402 max=25.192 mean=17.700
  dfs_parents: runs=20 min=11.535 median=17.634 p95=23.527 max=24.280 mean=17.995
  topological_parents: runs=20 min=15.377 median=18.070 p95=27.864 max=28.127 mean=19.713
  shortest_path_parents: runs=20 min=9.008 median=12.098 p95=17.638 max=21.466 mean=12.199
  shortest_path_both: runs=20 min=4.744 median=6.281 p95=7.863 max=7.982 mean=6.235

in_memory_adjacency_graph:
  nodes: 25001
  directed_edges: 27500

in_memory_adjacency_query_latency_ms:
  bfs_parents: runs=20 min=0.200 median=0.207 p95=0.314 max=0.479 mean=0.231
  shortest_path_parents: runs=20 min=0.227 median=0.232 p95=0.259 max=0.278 mean=0.237
  shortest_path_both: runs=20 min=0.009 median=0.009 p95=0.009 max=0.009 mean=0.009

in_memory_vs_sqlite_median_speedup:
  bfs_parents: 84.176x
  shortest_path_parents: 52.238x
  shortest_path_both: 668.202x
```

### 90k Raw Output (`tmp/perf-report-90k.txt`)

```text
=== GitGraph Lab Performance Report ===
fixture_root: C:\Users\deepe\AppData\Local\Temp\gitgraph-lab-perf-1773049600193558400
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
  fixture_generation: 66163.650
  fixture_summary: 571.211
  repository_preconditioning: 3001.575
  repository_scan: 133.798
  full_index: 17311.480
  incremental_fixture_append: 685.405
  incremental_index: 7891.085
  query_benchmark: 3813.244
  jit_file_change_enrichment: 420.855
  in_memory_graph_hydration: 339.007
  in_memory_query_benchmark: 211.030
  core_end_to_end: 100719.671
  unattributed_core: 177.331

phase_memory_bytes:
  fixture_generation_rss: 5173248
  fixture_summary_rss: 5181440
  repository_preconditioning_rss: 5181440
  repository_scan_rss: 7512064
  full_index_rss: 12230656
  incremental_fixture_append_rss: 12238848
  incremental_index_rss: 12423168
  query_benchmark_rss: 50655232
  jit_file_change_enrichment_rss: 50319360
  in_memory_graph_hydration_rss: 61014016
  in_memory_query_benchmark_rss: 62283776
  final_rss: 62287872
  peak_observed_rss: 62287872

dataset_counts:
  commits_in_db: 90001
  edges_in_db: 99000
  file_changes_in_db: 90002

file_change_stats_state_counts:
  pending: 89002
  ready: 1000
  failed: 0

full_index:
  duration_ms: 17311.480
  commits_indexed: 81001
  edges_indexed: 90000
  file_changes_indexed: 81002
  commit_throughput_per_sec: 4679.034
  edge_throughput_per_sec: 5198.862
  file_change_throughput_per_sec: 4679.092

incremental_index:
  duration_ms: 7891.085
  commits_indexed: 9000
  edges_indexed: 9000
  file_changes_indexed: 9000
  commit_throughput_per_sec: 1140.528
  edge_throughput_per_sec: 1140.528
  file_change_throughput_per_sec: 1140.528

ingestion_phase_timing_ms:
  full_commit_walk: 3494.246
  full_commit_upsert: 4513.734
  full_edge_materialize: 1644.532
  full_file_diff_extract: 2234.860
  full_file_diff_build: 2177.956
  full_file_diff_rename_find: 5.714
  full_file_diff_delta_emit: 51.190
  full_extraction_dispatch_wait: 9.664
  full_extraction_worker_busy: 2266.066
  full_extraction_reorder_buffer_max_depth: 1.000
  full_extraction_worker_errors: 0.000
  full_extraction_worker_aborts: 0.000
  full_file_change_delete: 0.000
  full_file_change_upsert: 4540.003
  full_tx_commit: 1118.258
  incremental_commit_walk: 3116.890
  incremental_commit_upsert: 675.293
  incremental_edge_materialize: 186.502
  incremental_file_diff_extract: 328.385
  incremental_file_diff_build: 322.421
  incremental_file_diff_rename_find: 0.569
  incremental_file_diff_delta_emit: 5.394
  incremental_extraction_dispatch_wait: 0.760
  incremental_extraction_worker_busy: 331.401
  incremental_extraction_reorder_buffer_max_depth: 1.000
  incremental_extraction_worker_errors: 0.000
  incremental_extraction_worker_aborts: 0.000
  incremental_file_change_delete: 290.936
  incremental_file_change_upsert: 949.189
  incremental_tx_commit: 421.741
  ingest_commit_walk_ms: 6611.136
  ingest_file_diff_extract_ms: 2563.245
  ingest_file_diff_build_ms: 2500.377
  ingest_file_diff_rename_find_ms: 6.283
  ingest_file_diff_delta_emit_ms: 56.584
  ingest_extraction_dispatch_wait_ms: 10.424
  ingest_extraction_worker_busy_ms: 2597.467
  ingest_extraction_reorder_buffer_max_depth: 1.000
  ingest_extraction_worker_errors: 0.000
  ingest_extraction_worker_aborts: 0.000
  ingest_file_change_upsert_ms: 5489.193
  ingest_tx_commit_ms: 1539.999

query_latency_ms:
  commit_window: runs=20 min=5.721 median=9.237 p95=14.193 max=17.431 mean=9.751
  bfs_parents: runs=20 min=21.391 median=40.370 p95=47.479 max=47.747 mean=37.060
  dfs_parents: runs=20 min=25.563 median=41.250 p95=51.200 max=51.522 mean=39.170
  topological_parents: runs=20 min=36.135 median=47.339 p95=60.627 max=73.892 mean=47.649
  shortest_path_parents: runs=20 min=20.281 median=26.945 p95=40.298 max=48.994 mean=29.104
  shortest_path_both: runs=20 min=5.170 median=8.968 p95=12.598 max=15.374 mean=8.929

in_memory_adjacency_graph:
  nodes: 90001
  directed_edges: 99000

in_memory_adjacency_query_latency_ms:
  bfs_parents: runs=20 min=2.931 median=4.969 p95=6.829 max=7.118 mean=5.089
  shortest_path_parents: runs=20 min=3.040 median=4.661 p95=6.463 max=6.707 mean=4.720
  shortest_path_both: runs=20 min=0.050 median=0.057 p95=0.140 max=0.143 mean=0.075

in_memory_vs_sqlite_median_speedup:
  bfs_parents: 8.124x
  shortest_path_parents: 5.780x
  shortest_path_both: 158.442x
```
