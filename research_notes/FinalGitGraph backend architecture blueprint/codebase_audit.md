# Codebase audit (2026-11-01)

Source: whole-repo audit of FinalGitGraph by code reading. No code was run (no cargo or pwsh on the audit machine). Every finding has file:line evidence.

What this repo does: A Rust library and CLI that reads a local git repository, copies its commits into an LMDB database, and keeps a compact graph of parent/child links (CSR: compressed sparse row, meaning two flat arrays of offsets and neighbors). It answers commit-list, traversal (BFS/DFS/topological) and shortest-path queries from that graph, and extracts per-commit file changes in the background.

Load assumed: one developer on a desktop, repositories up to about 1M commits (the size of the repo's largest perf lane). A Tauri UI that sends several commands at once is the stated next step, so findings that break under that are called out.

## Must fix

1. **Graph loses parent links between write batches** (`apps/desktop/src-tauri/src/adapters/db.rs:2657-2659`, `apps/desktop/src-tauri/src/application/indexer.rs:1595-1606`, `apps/desktop/src-tauri/src/adapters/git.rs:1363`)
   - What this is: During indexing, commits are written in batches ("waves") of 16,384. After each batch, the graph is extended with the new commits.
   - Problem: The walk sends children before parents. When a child's parent is in a later batch, `build_incremental_csr_snapshot` skips that parent (`let Some(..) = sha_to_index.get(parent_sha) else { continue; }`). When the parent arrives later, nothing adds the link back. Case: full index of a 20,000-commit linear history with default settings. The oldest commit of batch 1 loses its link to the newest commit of batch 2, so BFS over parents from HEAD stops after about 16,384 commits, and `shortest_path(HEAD, root)` returns `found=false`.
   - Fix: Stop updating the graph per batch. Build it once after the walk with the existing `rebuild_csr_snapshot` (the path at `indexer.rs:797-806`, which the hardware lane already uses per ADR 0018). For incremental runs, use the existing overlay path (`indexer.rs:742-780`); its compaction passes all overlay commits in one call, so links resolve. This deletes the streaming code instead of adding any.
   - If we skip it: Every repo bigger than one batch gives wrong traversal and path answers, with no error.

2. **Every `pnpm run` gate fails on macOS and Linux** (`package.json`, all scripts)
   - What this is: All gates (`guardrails`, `dev:gate`, `quality:perf`, ...) run `powershell -ExecutionPolicy Bypass -File ...`.
   - Problem: `powershell` is the Windows-only PowerShell 5.1 command. PowerShell 7 on macOS/Linux installs as `pwsh`. Case: on a Mac with PowerShell 7 installed, `pnpm run guardrails` fails with "command not found". CI passes only because the workflow calls `pwsh` directly (`.github/workflows/rust-ci.yml`), so local and CI behavior differ.
   - Fix: Short term, change `powershell` to `pwsh` in `package.json`. Long term, replace the PowerShell scripts with a Rust task runner (`cargo xtask`), covered in the architecture report.
   - If we skip it: The required gates in AGENTS.md cannot run on two of the three operating systems.

## Should fix

3. **The "Performance Budget" gate measures no performance** (`scripts/perf-budget.ps1:237-247`, `.github/workflows/rust-ci.yml` step "Performance Budget")
   - What this is: `pnpm run quality:perf` and the CI step meant to guard traversal speed.
   - Problem: It runs one functional test (`lmdb_index_and_graph_queries_work_end_to_end`) on a 3-commit repo and checks only that it passed. Nothing in it times anything. Case: make BFS 100 times slower and the gate stays green.
   - Fix: Delete the gate, or make it run a fixed-size fixture and fail on a stored time or instruction-count threshold.
   - If we skip it: AGENTS.md and the docs claim a perf guard that does not exist.

4. **No test checks the graph against git** (`apps/desktop/src-tauri/tests/indexer_smoke.rs:85`, `apps/desktop/src-tauri/src/bin/perf-report.rs:266-301`)
   - What this is: The only edge assertion and the perf harness queries.
   - Problem: The test compares `snapshot_metadata.directed_edges` with `counts.1`, but `counts.1` is read from the same metadata (`db.rs:1884-1890`), so it always matches. All smoke repos have 3 commits, which is below the smallest batch (2,048). The perf harness throws away every traversal and path result with `let _ =`. This is why finding 1 was not caught.
   - Fix: Add one test: build more than 2,048 linear commits with `GITGRAPH_FILE_CHANGE_COMMIT_BATCH_SIZE=2048`, index, and assert that `shortest_path(HEAD, root).found` is true and the stored edge count equals the parent count from `git rev-list --all --parents`.
   - If we skip it: Graph correctness can break again with every gate green.

5. **Background extraction deletes old rows before it knows it can replace them** (`apps/desktop/src-tauri/src/application/indexer.rs:1002-1031`, `apps/desktop/src-tauri/src/adapters/db.rs:984`)
   - What this is: The `drain` job that fills in per-commit file changes.
   - Problem: It deletes a commit's file-change rows in one transaction, extracts in a second step, and saves in a third. If extraction fails, the rows are already gone. After 5 failures the commit is dropped from the queue for good. Case: a commit whose tree object is missing (shallow or partial clone) ends with no file changes and no visible error.
   - Fix: Delete the old rows inside the same transaction that writes the new ones, and only when extraction succeeded.
   - If we skip it: File history quietly goes missing for some commits.

6. **The "bounded" commit queue has no real bound** (`apps/desktop/src-tauri/src/application/indexer.rs:426-432`, `:1516-1525`)
   - What this is: The channel between the commit walker and the database writer, meant to stop the walker from racing ahead.
   - Problem: The capacity is computed in commits (32,768) but used as a count of messages, and each message is a batch of 16,384 commits. So the channel holds up to about 536 million commits and never pushes back. Case: on a 1M-commit repo with a slow disk, the whole history piles up in memory waiting for the writer.
   - Fix: Divide the commit capacity by the batch size (minimum 1), or use a small fixed message count such as 2–4. Delete the unused `_batch_size` parameter.
   - If we skip it: Memory grows with repo size, which the guardrails say must not happen.

7. **Drain rescans the whole retry index for every batch** (`apps/desktop/src-tauri/src/adapters/db.rs:2527-2557`, called at `:656`)
   - What this is: Cleanup of the "due index", which orders file changes waiting for line counts.
   - Problem: `delete_enrichment_due_entries_for_commits` walks every due entry of the repo each time file changes for a batch of commits are deleted. Drain calls this once per batch of 1,024 commits. Case: 100k commits means about 100 full scans of a list that can hold millions of entries, so the cost grows roughly with the square of repo size.
   - Fix: For each enrichment-queue row of the commit, compute the exact due key from its stored `enqueued_at_epoch` and delete that key. The same pattern already exists at `db.rs:717-728`.
   - If we skip it: Drain slows sharply on big repos.

8. **Perf lanes set 8 settings the code never reads** (`scripts/perf-report.ps1:456-470` and other scripts)
   - What this is: Environment variables that scripts set to configure runs.
   - Problem: These are set by scripts but read nowhere in the Rust code: `GITGRAPH_INGEST_WRITE_PROFILE`, `GITGRAPH_INGEST_PRIMARY` (set to `rocksdb`, which is not a dependency), `GITGRAPH_QUERY_RUNTIME_CACHE`, `GITGRAPH_PERF_REPORT_DOCKER_ONLY`, `GITGRAPH_FILE_CHANGE_PIPELINE_COMMIT_BATCH_DRAIN`, `GITGRAPH_FILE_CHANGE_PIPELINE_COMMIT_BATCH_FULL`, `GITGRAPH_WINDOWS_DATA_ROOT`, `GITGRAPH_FACULTY_SANDBOX_ROOT`. Case: ADR 0018 describes a "throughput" write profile for the 100k lane, but setting it changes nothing.
   - Fix: Delete them from the scripts and fix the ADR text.
   - If we skip it: Perf numbers are credited to settings that do nothing.

9. **Some timing numbers are always zero or counted twice** (`apps/desktop/src-tauri/src/application/indexer.rs:1033`, `:1039`, `:1683`)
   - What this is: Phase timings shown in reports and used to name the "bottleneck".
   - Problem: `tx_commit` is never assigned anywhere, so `ingest_tx_commit_ms` is always 0, although guardrails require it. `enrichment_enqueue` copies the `file_change_merge_upsert` time, so it is counted twice. `stage_insert` holds the diff time.
   - Fix: Delete `tx_commit` and `enrichment_enqueue`, rename `stage_insert` to what it measures, and update the guardrail list.
   - If we skip it: Performance decisions rest on numbers that are wrong.

10. **Tests change environment variables while other tests read them** (`apps/desktop/src-tauri/tests/indexer_smoke.rs:20`, `:89`, `:222`, `:705`)
    - What this is: Tests switch behavior with `std::env::set_var` behind a mutex.
    - Problem: Three tests do not take the mutex, yet the indexer reads about 30 environment variables on every run. Cargo runs tests in parallel threads. Case: `strict_sync_mode_materializes_file_changes_inline` sets strict mode while `lmdb_index_and_graph_queries_work_end_to_end` indexes, so the second test sometimes runs in the wrong mode. On Linux (glibc), setting and reading the environment from different threads at once is a data race that can crash. Rust 2024 marks `set_var` `unsafe` for this reason.
    - Fix: Pass settings as a typed config value into `Indexer` instead of reading the environment deep inside the code. Tests then build their own config.
    - If we skip it: Flaky tests, and a hard blocker for moving to Rust 2024.

11. **Test and perf files are left behind** (`apps/desktop/src-tauri/src/bin/perf-report.rs:161-164`, `:551-556`, `scripts/perf-report.ps1:318`, `apps/desktop/src-tauri/src/application/indexer.rs:327`)
    - What this is: Where fixtures, databases and reports are written.
    - Problem: Fixture folders `gitgraph-lab-perf-<millis>` are deleted only on a normal return. On a panic or Ctrl-C they stay, and a failed delete is ignored (`let _ = fs::remove_dir_all`). On Windows they go to a hardcoded `D:\temp\gitgraph-lab` when a `D:` drive exists. Warm lanes write large fixture repos to `tmp/perf-fixtures/` inside the repo, with no cleanup command. The CLI's default database is `.gitgraph-lab/` in whatever folder you run it from.
    - Fix: Use `tempfile::TempDir` for the fixture root, which deletes itself even on panic. Put kept fixtures under `target/` so `cargo clean` removes them. Put the default database in the OS data folder.
    - If we skip it: Disk fills with hidden copies, and results from one run can leak into the next.

12. **The LMDB size limit can stop indexing halfway** (`apps/desktop/src-tauri/src/adapters/db.rs:22`, `:302`, `apps/desktop/src-tauri/src/application/indexer.rs:404`)
    - What this is: LMDB needs a maximum file size set up front (the "map size"); the default here is 4 GiB.
    - Problem: Nothing handles the "map full" error. A full index first deletes the old data, then writes. Case: a multi-million-commit repo with file changes passes 4 GiB, and the run fails after the old index is gone. The user must know to set `GITGRAPH_LMDB_MAP_SIZE_BYTES`.
    - Fix: Catch the map-full error, grow the map, and retry the transaction. Or choose a storage design without a fixed limit (see the architecture report).
    - If we skip it: Large repos fail with an index left half-built.

13. **The database is opened again on every command** (every `Indexer` method in `apps/desktop/src-tauri/src/application/indexer.rs`, via `open_store`)
    - What this is: Each call such as `traverse_graph` or `get_commit_window` opens a new LMDB environment.
    - Problem: LMDB's own documentation says not to open the same environment twice in one process at the same time, because closing one breaks the other's file locks. Fine for a one-shot CLI. Case: in the planned Tauri app, indexing runs while the UI asks for a commit window, and two environments are open on the same files in one process.
    - Fix: Open the store once per process and share it (for example, hold it in `Indexer`).
    - If we skip it: Rare corruption or lock errors once the UI arrives.

14. **The guardrails check names, not behavior, and code exists only to satisfy them** (`scripts/enforce-guardrails.ps1:287`, `:360`, `:407-439`, `apps/desktop/src-tauri/src/adapters/git.rs:67`, `:4037`)
    - What this is: A 1,038-line PowerShell script that searches the source for exact strings.
    - Problem: It requires names such as `walk_commits_tape` and `rebuild_csr_snapshot` to exist, but never checks what they do. The constant `PATCH_FROM_DIFF_GUARDRAIL_MARKER` and the line `let _ = PATCH_FROM_DIFF_GUARDRAIL_MARKER;` have no purpose except to match a search. Case: finding 1 passes every guardrail, while renaming a correct function fails them.
    - Fix: Delete the marker. Replace name searches with real checks: crate boundaries, `clippy.toml` disallowed methods, and the correctness test in finding 4.
    - If we skip it: The rules block safe refactors and let real bugs through.

15. **The same telemetry fields are copied by hand in seven places** (`apps/desktop/src-tauri/src/adapters/git.rs:809-886`, `apps/desktop/src-tauri/src/application/indexer.rs:593-642`, `:825-857`, `:877-947`, `:1703-1790`, `apps/desktop/src-tauri/src/domain/models.rs:172-461`)
    - What this is: Timing and counter structs returned from indexing.
    - Problem: `IndexingStats` (62 fields), `IngestionPhaseTimingMs` (51) and `CommitThroughputTelemetry` (70) repeat the same ~25 commit-walk fields. Each field is copied across `CommitWalkPhaseTiming`, `CommitWalkExecutionStats`, `CommitWalkTelemetryContext`, `IngestionPhaseTiming` and the three DTOs, and counters are turned into `f64`. The report formatter is 1,553 lines (`perf-report.rs:1106-2659`). Case: adding one counter means editing about 8 places, and one missed copy shows a silent 0.
    - Fix: Keep one `CommitWalkStats` struct and embed it where needed instead of flattening it. Drop the duplicate flattened fields from `IndexingStats`.
    - If we skip it: Hundreds of lines to maintain, and the wrong-number class from finding 9 keeps coming back.

16. **Two tables are written on every change but never read** (`apps/desktop/src-tauri/src/adapters/db.rs:2838-2872`, `:1298-1355`, `apps/desktop/src-tauri/src/application/index_planner.rs`)
    - What this is: `delta_log` (an event log) and `planner_sessions` (a record of each index decision).
    - Problem: `append_delta_event` runs in nearly every write transaction, but nothing reads `delta_log`. `planner_sessions` is read only by its own unit test. The planner also produces `risk_level`, `degraded_policy` and `trace_count` (always 1 or 2) for what is a two-way choice: checkpoint present or not.
    - Fix: Delete both tables and their writes. Keep the planner as the small checkpoint check.
    - If we skip it: Extra writes on every transaction and code with no user.

17. **Dead code** (various)
    - What this is: Code with no callers anywhere in source or tests (checked with grep).
    - Problem: 12 public functions are unused: `children_for`, `parents_for`, `configured_file_change_drain_channel_capacity`, `configured_file_change_drain_parallel_chunk_size`, `configured_file_change_parallel_worker_count`, `enqueue_file_changes_for_enrichment` (also a near-copy of `db.rs:716-756`), `index_commits`, `load_commits`, `load_commits_since`, `load_file_changes_for_commits`, `load_repository_indexed_at`, and both `parse_db` methods. The sort mode `CommitTimeCutoff` behaves exactly like `CommitTimeNewest` (`git.rs:98`, `:654`).
    - Fix: Delete them.
    - If we skip it: More code to read and keep compiling, with nothing gained.

18. **The same helper is written two or three times** (`indexer.rs:1430-1503`, `db.rs:2213`, `git.rs:2162-2196`; `indexer.rs:2503` vs `db.rs:2820`; `indexer.rs:1805`, `git.rs:573`, `git.rs:605`; `commands.rs:98,130`, `main.rs:190,239`, `limits.rs`)
    - What this is: Small utilities.
    - Problem: `parse_env_bool_enabled` exists 3 times, `parse_env_bool_with_default` and `parse_positive_env_usize` twice each, and the CSR slice function twice. There are three bit-set types (`DenseSet`, `PositionBitmap`, `DenseBitmap`). The default window limit (100) and node limit (2,000) are written in three places. Case: someone changes the default in `limits.rs` and the CLI still uses its own number.
    - Fix: Keep one of each in `support/`, and use the `limits.rs` constants everywhere.
    - If we skip it: The copies drift apart.

19. **32 runtime switches nobody tests together** (`apps/desktop/src-tauri/src/application/indexer.rs:36-45`, `apps/desktop/src-tauri/src/adapters/git.rs:41-65`, `apps/desktop/src-tauri/src/adapters/db.rs:25-30`)
    - What this is: Environment variables that turn pipeline features on and off.
    - Problem: 32 distinct `GITGRAPH_*` variables are read at runtime, mostly leftovers from perf experiments. Only a few combinations are tested. Finding 1 lives in the default combination.
    - Fix: Delete switches whose default won. Keep the rest in one typed config struct passed in (as in finding 10).
    - If we skip it: Every bug report starts with "which variables were set?"

## Nice to have

20. **Each query does whole-graph work even for tiny answers** (`apps/desktop/src-tauri/src/application/indexer.rs:2254`, `:2581-2584`, `:1873`)
    - What this is: Memory use inside traversal queries.
    - Problem: Topological traversal allocates an array the size of the whole graph on every call, even with `max_nodes=10`. Shortest path grows its predecessor array to the highest node id it touches. SHA lookup uses a `HashMap<String, usize>`, roughly 100 MB or more at 1M commits.
    - Fix: Use a small hash map sized to the visit budget, and a sorted binary SHA array with binary search (the way git looks up object ids).
    - If we skip it: Slower small queries and higher memory on big repos.

Left out 4 smaller findings: docs bloat (54 docs, 7,767 lines, several superseded ADRs and perf logs); the incremental 300-second lookback re-saves already-filled commits as "metadata pending"; read commands create a repository row when the repo is unknown; `edges_indexed` counts parent links seen, not links stored (which hid finding 1).

Verdict: Fix 1 first (wrong answers), with the test from 4. Then fix 2 so the gates run on every OS.

Lean: -1,500 lines possible (estimate; Rust only, not counting the 4,200 lines of PowerShell that a Rust task runner would replace), -0 dependencies.

Not checked: `git.rs` diff, rename and Bloom internals (about lines 2593–4490) were skimmed, not read line by line, except the murmur3 v1 hash. It copies git's signed-`char` sign-extension quirk, which matches v1 filters written on signed-`char` platforms only. A v1 filter written on an unsigned-`char` platform (for example aarch64 Linux) will not match for non-ASCII paths, and a reader cannot tell which one it has (see git_ingestion_and_derived_data.md). The queue retry code in `db.rs` (lines 911–1270) and the bodies of `perf-ablation.ps1`, `safe-install.ps1` and `faculty-server-sandbox.ps1` were not read. Nothing was compiled or run, so every finding comes from reading the code.
