# S2 Phase 2: Guardrails Port with Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port all 59 failure sites of `scripts/enforce-guardrails.ps1` to `cargo xtask guardrails`. Prove parity on one seeded violation per check against the PowerShell script, and make `cargo xtask gate` run guardrails first.

**Architecture:** `xtask/src/guardrails/` holds three things:
- the plain checks (structure, markers, docs, scripts, anti-tamper);
- one data table of file contracts (`Contract { file, require, any_of, forbid, message, replaced_by }`);
- a line-based, case-insensitive matcher, which reproduces PowerShell `Select-String` and `-match` semantics.

`xtask/tests/guardrails.rs` makes a local clone of the current working state into a temp dir. For each fixture, it applies one mutation, runs both `cargo xtask guardrails --repo <clone>` and `pwsh <clone>/scripts/enforce-guardrails.ps1`, and asserts that both report the same multiset of check names, and that this multiset is the expected one.

**Tech Stack:** Rust 1.94.0, `regex` 1.12.3 (already in `Cargo.lock`), `tempfile` 3 (dev, already in `Cargo.lock`), pwsh 7.6 (parity only).

**Spec:** `docs/superpowers/specs/2026-11-16-cargo-xtask-migration-design.md`, plus the owner decisions below.

## Global Constraints

- Inherit every Phase 1 constraint. That covers workspace lints, no `unwrap`/`expect` outside tests, no `TODO`, `FIXME`, `#[allow]` or `#[ignore]`, and dispatch-only `main.rs`.
- Every regex is compiled **case-insensitive**, because `Select-String` and `-match` default to that.
- Patterns are matched **per line**, like `Select-String`. Substring checks on whole files use case-insensitive `contains`, like `-match [regex]::Escape(...)`.
- No literal relational engine names in any file under `docs/`, `scripts/` or `apps/`. The relational-term check scans those paths, so build such names with `concat!`.
- Every git spawn goes through `run::git(repo)`. It clears inherited `GIT_*` variables and sets `GIT_CONFIG_NOSYSTEM=1` and `GIT_CONFIG_GLOBAL` to the null device.

## Owner Decisions (2026-11-16)

1. **Anti-tamper split.**
   - `xtask/src/guardrails/**` counts as **policy**.
   - Every other path under `xtask/` counts as **implementation**.
   - `scripts/enforce-guardrails.ps1` gets the same two rules in the same commit, so old and new stay in parity.
2. **Port all 59 now**, including the checks that inspect the PowerShell scripts and `package.json`. Phase 4 replaces those.

## Documented Differences From the PowerShell Script

| Behavior | PowerShell | xtask | Why |
|---|---|---|---|
| Nested `Cargo.lock` scan | walks the whole repository, including `target/` and `.git/` | skips `target/` and `.git/` | A lock file under `target/` is never source. Walking a 1 GB build directory costs seconds on every run. |
| `git2::` present together with missing tape-first symbols | reports only the `git2` failure (`elseif`) | reports both failures | It reports strictly more. No fixture or real tree hits the case. |
| Passing checks | prints `[OK]` lines | prints nothing for passes | Less noise. Parity compares only `[FAIL]` lines. |

## Check-Name Normalization (Parity Key)

A check name is the failure message cut at its earliest occurrence of `": "`, `" ("`, a backtick, or any control character. ANSI color codes are stripped first. Both sides use this normalization.

This is needed because PowerShell double-quoted strings treat a backtick as an escape: `` `r `` and `` `e `` become CR and ESC. So several PowerShell messages contain control characters that the Rust messages do not.

## File Structure

| File | Responsibility |
|---|---|
| `xtask/src/paths.rs` | `repo_root() -> PathBuf`, shared by `doctor` and `guardrails` |
| `xtask/src/run.rs` (modify) | add `git(repo) -> Command` (pinned environment) |
| `xtask/src/guardrails/mod.rs` | `run(repo)`, `check(repo) -> Vec<String>`, `check_name(&str) -> &str` |
| `xtask/src/guardrails/text.rs` | `read`, `has_line`, `matching_lines`, `walk_files`, `rel` |
| `xtask/src/guardrails/structure.rs` | lock files, source root, layers, flat and legacy modules, markers, migrations, relational terms |
| `xtask/src/guardrails/contracts.rs` | the `Contract` table and its runner (git, indexer, db, perf-report binary, CI, perf scripts, `package.json`) |
| `xtask/src/guardrails/docs.rs` | required docs, standards index and sections, ADR index |
| `xtask/src/guardrails/scripts.rs` | scripts directory, platform header, portable-script policy, required perf files |
| `xtask/src/guardrails/tamper.rs` | changed-file detection and the policy/implementation rule |
| `xtask/tests/guardrails.rs` | clone, fixture table, xtask and PowerShell runs, assertions |
| `xtask/src/gate.rs` (modify) | run guardrails before the cargo steps |
| `scripts/enforce-guardrails.ps1` (modify) | the anti-tamper split |
| `docs/adr/0020-portable-automation-cargo-xtask.md` (modify) | Phase 2 mapping table and results |

## Check Catalog (59 sites)

The "Name" column is the parity key. "Tag" is the roadmap owner whose behavior test will later replace the check, where one is planned.

### Plain checks

| # | PowerShell line | Name | Rule |
|---|---|---|---|
| 1 | 152 | `Root Cargo.lock is missing.` | `Cargo.lock` must exist |
| 2 | 161 | `Nested Cargo.lock files must not exist. Found` | no other `Cargo.lock` (skips `target/` and `.git/`) |
| 3 | 166 | `Missing Rust source root` | `apps/desktop/src-tauri/src` must exist; if missing, skip checks 4 to 10 and all git, indexer and db contracts |
| 4 | 174 | `Layer directory missing` | one failure per missing directory among `application`, `adapters`, `domain`, `support` |
| 5 | 184 | `Unexpected root Rust files found` | src-root `*.rs` other than `lib.rs` and `main.rs` |
| 6 | 200 | `Legacy flat modules should not exist` | any of `app_error.rs`, `clock.rs`, `commands.rs`, `db.rs`, `git_adapter.rs`, `indexer.rs`, `models.rs` in src root |
| 7 | 217 | `Forbidden markers found in Rust source tree at` | `\b(TODO\|FIXME)\b\|\bdbg!\s*\(` in src `**/*.rs` |
| 8 | 234 | `Duct-tape quality markers are blocked in Rust source tree` | `^\s*//\s*(HACK\|WIP\|WORKAROUND\|TEMP(?:ORARY)?\|QUICKFIX\|DUCKTAPE\|DUCTTAPE)\b` |
| 9 | 250 | `Lint-suppression duct-tape attributes are blocked in Rust source tree` | `#\s*\[\s*allow\s*\(\s*(clippy::\|dead_code\|unused_)` |
| 10 | 264 | `Legacy relational runtime modules must not exist` | any of `application/graph_queries.rs`, `application/query_graph_cache.rs`, `application/indexer_pipeline.rs` |
| 11 | 574 | `Legacy relational migration files must be removed for LMDB runtime` | a `*.<relational extension>` file in `apps/desktop/src-tauri/migrations` |
| 12 | 610 | `Forbidden legacy relational terms detected. Sample` | `(?i)(<engine>\|<driver>\|\b<word>\b)` per line in `**/*.{rs,ps1,md,toml,yml,yaml,json}` under src, tests, scripts and docs |
| 13 | 642 | `Required governance doc missing` | the 22 paths at `enforce-guardrails.ps1:613-636` |
| 14 | 648 | `Missing scripts directory` | `scripts/` must exist; if missing, skip 15 and 16 |
| 15 | 671 | `Script missing platform metadata ('# platform` | the first 8 lines of each `scripts/*.ps1` match `^\s*#\s*platform:\s*(any\|windows)\s*$` |
| 16 | 690 | `Portable script policy violation in <rel>` | for `any` scripts except `enforce-guardrails.ps1`: USERPROFILE without HOME, `\bcmd\s*/c\b`, `/bin/bash`, `\$env:Path\s*=\s*".*;.*\$env:Path`, `[12]>\s*nul\b` |
| 17 | 729 | `Docs/guardrails changes without implementation changes are blocked. Changed files` | see Task 4 |
| 18 | 748 | `Required performance file missing` | the 6 paths at `enforce-guardrails.ps1:733-740` |
| 19 | 835 | `docs/standards/README.md must list <file>` | the README contains each of the 6 standards file names |
| 20 | 852 | `Standards file <file> is missing required section <section>` | the section table at `enforce-guardrails.ps1:785-826` |
| 21 | 869 | `docs/adr/README.md must reference ADR file <name>` | every `docs/adr/*.md` except `README.md` |

### Contract table (38 sites)

Each contract applies only if its file exists. `require`: every regex must match some line. `any_of`: at least one group must have all of its regexes match. `forbid`: the regex must match no line.

| # | Line | File | Kind | Patterns | Tag |
|---|---|---|---|---|---|
| 22 | 271 | `adapters/git.rs` | forbid | `Command::new\("git"\)` | 0024 |
| 23 | 277 | `adapters/git.rs` | forbid | `(^\|\W)git2::` | 0024 |
| 24 | 287 | `adapters/git.rs` | require | `^\s*use\s+gix::Repository;`, `\bwalk_commits_tape\(`, `\bbuild_pack_aware_walk_tape\(`, `\bdiff_tree_to_tree\(`, `\btrack_rewrites\(` | 0024 |
| 25 | 291 | `adapters/git.rs` | forbid | `fn run_git_command` | 0024 |
| 26 | 302 | `adapters/git.rs` | require | `stream_commits`, `stream_file_changes_for_commits` | 0024 |
| 27 | 318 | `adapters/git.rs` | require + any_of | `FILE_CHANGE_PARSE_PARALLEL_MIN_COMMITS`, `FILE_CHANGE_PARSE_PARALLEL_CHUNK_SIZE`; any of [`\.par_chunks\(`] or [`\.chunks\(FILE_CHANGE_PARSE_PARALLEL_CHUNK_SIZE\)`, `into_par_iter\(`] | 0024 |
| 28 | 322 | `adapters/git.rs` | forbid | `\b<driver>::` | 0022 |
| 29 | 334 | `adapters/git.rs` | require | `rename_limit`, `rename_threshold`, `exact_match_only` | 0024 |
| 30 | 343 | `adapters/git.rs` | require | `sync_channel`, `FILE_CHANGE_PARSE_CHANNEL_CAPACITY` | 0024 |
| 31 | 353 | `adapters/git.rs` | require | `parallel_file_change_plan`, `FILE_CHANGE_PARSE_PARALLEL_TARGET_CHUNKS_PER_WORKER`, `FILE_CHANGE_PARSE_CHANNEL_CAPACITY_PER_WORKER` | 0024 |
| 32 | 367 | `adapters/git.rs` | require | `diff_requires_line_stats`, `Patch::from_diff\|PATCH_FROM_DIFF_GUARDRAIL_MARKER`, `line_counts`, `collect_patch_line_stats` | 0024 |
| 33 | 378 | `adapters/git.rs` | require | `pub struct FileDiffTelemetry`, `diff_build`, `rename_find`, `delta_emit` | 0025 |
| 34 | 388 | `adapters/git.rs` | require | `parse_hex_object_id`, `object_cache_bytes_for_workers`, `object_cache_size_if_unset` | 0024 |
| 35 | 398 | `adapters/git.rs` | require | `FileChangeExtractionMode::MetadataOnly`, `FileChangeExtractionMode::SelectiveLineStats`, `load_file_change_line_stats_for_targets` | 0024 |
| 36 | 407 | `application/indexer.rs` | require | `stream_commits\(` | 0024 |
| 37 | 416 | `application/indexer.rs` | require | `FILE_CHANGE_COMMIT_BATCH_SIZE`, `FILE_CHANGE_DB_BATCH_SIZE` | 0024 |
| 38 | 428 | `application/indexer.rs` | require | `let store = open_store\(`, `store\.persist_commits\(`, `store\.persist_file_changes\(`, `store\.synchronize_refs\(`, `store\.rebuild_csr_snapshot\(` | 0022 |
| 39 | 439 | `application/indexer.rs` | require | `type NodeId = usize`, `struct DenseSet`, `load_graph_snapshot_state`, `QueryGraph::from_graph_state` | 0023 |
| 40 | 453 | `adapters/db.rs` | require | `use lmdb::`, `pub struct GraphStore`, `begin_rw_txn`, `txn\.commit\(\)\?;` | 0022 |
| 41 | 466 | `adapters/db.rs` | require | `pub struct CsrSnapshot`, `parents_offsets`, `parents_neighbors`, `children_offsets`, `children_neighbors`, `rebuild_csr_snapshot` | 0022 |
| 42 | 476 | `adapters/db.rs` | require | `GraphSnapshotMetadata`, `graph_snapshot_metadata`, `load_graph_snapshot_state` | 0022 |
| 43 | 486 | `adapters/db.rs` | require | `delta_log`, `append_delta_event`, `META_NEXT_DELTA_SEQUENCE` | S5 |
| 44 | 498 | `adapters/db.rs` | require | `graph_overlay_parent_rows`, `graph_overlay_child_rows`, `persist_graph_overlay_commits`, `compact_graph_overlay_into_snapshot`, `should_compact_graph_overlay` | 0022 |
| 45 | 502 | `adapters/db.rs` | forbid | `\b<driver>::\|\bSELECT\s+\|\bINSERT\s+INTO\|\bUPDATE\s+\w+\s+SET\|\bDELETE\s+FROM\|\bCREATE\s+TABLE` | 0022 |
| 46 | 514 | `adapters/db.rs` | require | `fn delete_file_changes_for_commits`, `repository_commit_file_change_prefix`, `delete_prefixed_entries` | 0022 |
| 47 | 525 | `adapters/db.rs` | require | `FileChangeStatsState`, `enrichment_queue`, `mark_file_change_enrichment_failed`, `FILE_CHANGE_ENRICHMENT_MAX_ATTEMPTS` | 0024 |
| 48 | 540 | `adapters/db.rs` | require | `EnvironmentFlags`, `resolve_lmdb_runtime_options`, `LmdbRuntimeOptions`, `effective_map_size_bytes`, `GITGRAPH_LMDB_NO_READAHEAD`, `GITGRAPH_LMDB_WRITE_MAP`, `GITGRAPH_LMDB_MAP_ASYNC`, `GITGRAPH_LMDB_NO_MEM_INIT` | 0022 |
| 49 | 551 | `adapters/db.rs` | require | `commit_window_index`, `load_commit_window`, `for_each_prefixed_cursor_from`, `MDB_SET_RANGE` | 0022 |
| 50 | 561 | `adapters/db.rs` | require | `enrichment_due_index`, `load_pending_file_change_enrichment_internal`, `parse_due_index_identity_from_prefixed_key` | 0024 |
| 51 | 771 | `scripts/perf-budget.ps1` | require | `\[switch\]\$ProbeOnly`, `function\s+Invoke-PerfBudgetDocker`, `Application Control policy has blocked this file\|os error 4551`, `Host build phase duration \(s\):`, `Host execute phase duration \(s\):`, `--no-run` | Phase 4 |
| 52 | 891 | `scripts/perf-report.ps1` | require | `\[switch\]\$ProbeOnly`, `function\s+Invoke-PerfReportDocker`, the policy-detection pattern, the two duration patterns | Phase 4 |
| 53 | 928 | `scripts/perf-ingestion-budget.ps1` | require | `\[switch\]\$ProbeOnly`, `\$Profile`, `\$Lane`, `"90k"`, `perf-report\.ps1`, `MaxFullIndexMs`, `MaxIncrementalIndexMs`, `MaxPeakRssBytes`, `MaxDiffShareRatio`, `peak_observed_rss`, `core_end_to_end`, `fixture_generation`, `WarmFixture` | Phase 4 |
| 54 | 977 | `src/bin/perf-report.rs` | require | the 19 fields at `enforce-guardrails.ps1:934-952` | 0025 |
| 55 | 986 | `.github/workflows/rust-ci.yml` | require | `Performance Budget` | 0025 |
| 56 | 992 | `.github/workflows/rust-ci.yml` | require | `Script Contract Probe` | Phase 4 |
| 57 | 1001 | `package.json` | require | `"quality:perf"` | Phase 4 |
| 58 | 1007 | `package.json` | require | `"quality:perf:ingestion"` | Phase 4 |
| 59 | 1025 | `package.json` | require | the 11 aliases at `enforce-guardrails.ps1:1012-1022` | Phase 4 |

The git, indexer and db contracts (22 to 50) run only when the source root exists, as in PowerShell. Paths in the File column are relative to `apps/desktop/src-tauri/src/` unless they start with `scripts/`, `.github/` or `package.json`. Copy each message verbatim from the line given, but replace the PowerShell backtick escapes with plain text.

## Fixture Table

Mutations are applied to a fresh clone. Clone setup:

1. Run `git stash create`, or use `HEAD` if that prints nothing.
2. `git clone --quiet --local <repo> <tmp>/clone`, then `git -C <tmp>/clone checkout --quiet --detach <commit>`.
3. Make an empty commit with a pinned identity, so that `HEAD~1..HEAD` is empty.

Between fixtures, reset with `git checkout -- .` and `git clean -fdq`. Expected names are a multiset, and "+T" adds name 17 (anti-tamper).

| Fixture | Mutation | Expected names |
|---|---|---|
| clean | none | none |
| root-lock | remove `Cargo.lock` | 1 |
| nested-lock | write `nested/Cargo.lock` | 2 |
| src-root | remove `apps/desktop/src-tauri/src` | 3, 18 |
| layer | remove `apps/desktop/src-tauri/src/support` | 4 |
| stray-root | write `src/stray.rs` | 5 |
| legacy-flat | write `src/indexer.rs` | 5, 6 |
| marker | append `// TODO tidy` to `src/lib.rs` | 7 |
| duct-tape | append `// HACK quick` to `src/lib.rs` | 8 |
| lint-allow | append `#[allow(dead_code)]` + `fn probe() {}` to `src/lib.rs` | 9 |
| legacy-runtime | write `src/application/graph_queries.rs` | 10 |
| migration | write `apps/desktop/src-tauri/migrations/0001.<ext>` | 11 |
| relational-doc | append a line naming the engine to `docs/phase-1-kickoff.md` | 12, +T |
| 22 to 50 | one token replaced or appended in the target file (exact pairs in Task 5) | that one name |
| 51 to 53 | replace one required token in the script | that name (+T for `perf-budget.ps1`) |
| 54 | replace `aggregate_commit_bottleneck_phase` in `perf-report.rs` | 54 |
| 55, 56 | rename the CI step | that name, +T |
| 57 to 59 | rename one alias in `package.json` | that name, +T |
| required-doc | remove `docs/performance-engineering.md` | 13, +T |
| standards-index | in `docs/standards/README.md`, replace `REPO_SYNC_POLICY.md` with `REPO_SYNC.md` | 19, +T |
| standards-section | in `docs/standards/REPO_SYNC_POLICY.md`, replace `## Sync Rules` with `## Sync` | 20, +T |
| adr-index | write `docs/adr/0099-probe.md` | 21 |
| platform-header | in `scripts/dev-loop.ps1`, replace `# platform: any` with `# platform: some` | 15 |
| portable | append `cmd /c echo hi` to `scripts/dev-loop.ps1` | 16 |
| perf-file | remove `scripts/layer-test-matrix.ps1` | 18 |
| tamper-docs | append `note` to `docs/phase-1-kickoff.md` | +T |
| tamper-docs-and-src | append `note` to `docs/phase-1-kickoff.md` and `// note` to `src/lib.rs` | none |
| tamper-guardrail-rules | append `// note` to `xtask/src/guardrails/mod.rs` | +T |
| tamper-xtask-tool | append `note` to docs and `// note` to `xtask/src/main.rs` | none |

`src/` above means `apps/desktop/src-tauri/src/`. Fixture "scripts-dir" (remove `scripts/`, expecting 14, 18×4 and +T) runs xtask only, because the PowerShell script cannot run once it has been deleted.

---

### Task 1: Matcher, paths and git helper

**Files:** create `xtask/src/paths.rs`, `xtask/src/guardrails/mod.rs`, `xtask/src/guardrails/text.rs`. Modify `xtask/src/run.rs`, `xtask/src/doctor.rs` and `xtask/Cargo.toml` (`regex = "1.12.3"`, dev `tempfile = "3.12.0"`).

**Interfaces (produces):**
- `paths::repo_root() -> PathBuf`
- `run::git(repo: &Path) -> Command`
- `text::read(path) -> Option<String>`
- `text::has_line(text, pattern) -> Result<bool, String>`
- `text::matching_lines(text, pattern) -> Result<Vec<usize>, String>`
- `text::walk_files(root, skip_dirs: &[&str]) -> Vec<PathBuf>`
- `text::rel(repo, path) -> String` (forward slashes)
- `guardrails::check_name(message) -> &str`

- [ ] Write the failing unit tests:
  - `has_line` matches case-insensitively and per line;
  - `^` anchors per line;
  - `check_name` cuts at `": "`, `" ("`, a backtick or `\r`, and strips ANSI codes first.
- [ ] Run `cargo test -p xtask` and see the compile failure.
- [ ] Implement the helpers. Move `doctor`'s `repo_root` into `paths.rs`.
- [ ] Tests pass. Then run `cargo clippy -p xtask --all-targets -- -D warnings`.

### Task 2: Plain checks (catalog 1 to 21 except 17)

**Files:** create `structure.rs`, `docs.rs` and `scripts.rs` under `xtask/src/guardrails/`.
- [ ] Implement each check with the exact message prefix given in the catalog.
- [ ] Unit-test `check` on the current repository: `check(&repo_root())` must return an empty vector.

### Task 3: Contract table (catalog 22 to 59)

**Files:** create `xtask/src/guardrails/contracts.rs`.
- [ ] Write a unit test asserting that every pattern in `CONTRACTS` compiles.
- [ ] Write a unit test asserting that `CONTRACTS.len() == 38`.
- [ ] Implement the table and its runner (file missing means skip, then forbid, require and any_of).
- [ ] `check(&repo_root())` is still empty.

### Task 4: Anti-tamper with the split, in both languages

**Files:** create `xtask/src/guardrails/tamper.rs`. Modify `scripts/enforce-guardrails.ps1:704-722`.

- [ ] Port `Get-ChangedFiles` through `run::git`, in the same order:
  1. working-tree and staged changes;
  2. on `pull_request` with `GITHUB_BASE_REF`, fetch the base and diff `...HEAD`;
  3. otherwise `HEAD~1..HEAD`.
- [ ] Classify each changed path:
  - **Policy:** `^docs/`, `scripts/enforce-guardrails.ps1`, `scripts/perf-budget.ps1`, `.github/workflows/rust-ci.yml`, `README.md`, `CONTRIBUTING.md`, `package.json`, `^xtask/src/guardrails/`.
  - **Implementation:** `^apps/desktop/src-tauri/src/.+\.rs$`, `^apps/desktop/src-tauri/tests/.+\.rs$`, `^apps/desktop/src-tauri/migrations/.+\.<ext>$`, `apps/desktop/src-tauri/Cargo.toml`, `Cargo.toml`, and anything under `^xtask/` that is not under `^xtask/src/guardrails/`.
- [ ] Use case-insensitive comparisons, as PowerShell's `-eq` and `-match` do.
- [ ] Add the same two clauses to the PowerShell policy and implementation conditions.

### Task 5: Fixture parity test

**Files:** create `xtask/tests/guardrails.rs`.
- [ ] Encode the fixture table as `Fixture { name, mutations: &[Mutation], expected: &[&str], powershell: bool }`, where `Mutation` is `Replace(file, from, to)`, `Append(file, text)`, `Write(file, text)` or `Remove(path)`. A `Replace` asserts that at least one occurrence was replaced.
- [ ] For each fixture, run `CARGO_BIN_EXE_xtask guardrails --repo <clone>`. When `powershell` is true, also run `pwsh -NoProfile -File <clone>/scripts/enforce-guardrails.ps1`.
  - Run both with `GITHUB_EVENT_NAME`, `GITHUB_BASE_REF` and inherited `GIT_*` removed.
  - Collect `[FAIL]` lines and normalize them with the same cut rule as `check_name`, reimplemented in the test because xtask is a binary crate.
  - Sort the names and compare them with the expected names.
- [ ] Collect every mismatch, then assert that the mismatch list is empty, so one run reports all of them.
- [ ] A missing pwsh fails the test.

### Task 6: Gate runs guardrails, ADR 0020 mapping, verify and push

- [ ] `gate::run` calls `guardrails::run(&paths::repo_root())` before the cargo steps. Usage becomes `doctor | gate [--fast] | guardrails [--repo <path>]`.
- [ ] Add the Phase 2 section to ADR 0020:
  - the 59-row mapping (catalog number, PowerShell line, name, ported/split, tag);
  - the parity results;
  - the documented differences;
  - the Phase 1 CI run ID (43927526794).
- [ ] Run `cargo xtask gate`, both guardrail runners, and `cargo test -p xtask --test guardrails`.
- [ ] Commit as one change (it touches `xtask/src` (implementation), the policy files and docs), push, and watch CI on all three OSes.
