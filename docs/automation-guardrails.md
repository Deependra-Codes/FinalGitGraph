# Automation Guardrails

Date: 2026-03-07

## Goal

Prevent CI/runtime script failures caused by hidden platform assumptions while keeping delivery speed and performance work unblocked.

## Design Principles

1. Platform intent must be explicit in every script.
2. Portable scripts must avoid OS-specific command/runtime assumptions.
3. CI must probe automation contracts before expensive build/test stages.
4. Guardrails should target reliability classes, not one-off incidents.

## Script Platform Contract

Every script in `scripts/*.ps1` must declare one metadata line near the top:

1. `# platform: any` for cross-platform scripts.
2. `# platform: windows` for Windows-only scripts.

`scripts/enforce-guardrails.ps1` enforces this contract.

## Portable Script Policy (`# platform: any`)

Portable scripts are rejected if they use brittle OS-specific primitives:

1. direct `$env:USERPROFILE` dependence
2. direct `cmd /c`
3. hardcoded `/bin/bash`
4. hardcoded `;` PATH separator writes
5. Windows-only `nul` redirection patterns

These are class-level checks, not tied to a single historical failure.

## CI Automation Guardrails

`Rust CI` includes:

1. `Guardrail Policy Check` as an early hard gate.
2. `Script Contract Probe` via `./scripts/perf-budget.ps1 -ProbeOnly` on both OS runners.
3. `Performance Budget` execution on Ubuntu (release perf gate).
4. `Perf Smoke (PR)` report lane on pull requests for fast regression signal.

This ordering fails fast on automation regressions before expensive lanes.

Nightly scale enforcement:

1. `.github/workflows/nightly-ingestion-perf.yml` runs ingestion budget checks on `cold` and `warm` lanes.
2. Nightly/default profile is `25k`; manual dispatch may select `90k`.
3. The workflow runs `scripts/perf-ingestion-budget.ps1` with profile/lane inputs and preserves existing ingestion telemetry contracts.

## Performance Script Resilience Contract

`scripts/perf-budget.ps1` must keep three properties:

1. `-ProbeOnly` mode for zero-cost contract validation in CI.
2. host execution path for normal fast environments.
3. resilient fallback path when host policy blocks test binaries (for example Windows Application Control), with the same perf test executed in Docker.

`scripts/enforce-guardrails.ps1` verifies this contract so reliability does not depend on local machine policy details.

## Performance Timing Transparency Contract

Performance automation must not hide where time is spent. Required properties:

1. `scripts/perf-budget.ps1` prints separate build-phase and execute-phase durations.
2. `scripts/perf-report.ps1` prints separate build-phase and execute-phase durations.
3. `apps/desktop/src-tauri/src/bin/perf-report.rs` emits phase-level timing in report output (`phase_timing_ms`), including fixture generation and indexing/query phases.
4. `apps/desktop/src-tauri/src/bin/perf-report.rs` emits phase memory output (`phase_memory_bytes`) including peak observed RSS.
5. `apps/desktop/src-tauri/src/bin/perf-report.rs` emits in-memory adjacency hydration and query sections sourced from LMDB-backed persisted state.
6. `scripts/layer-test-matrix.ps1` must exist and remain runnable through `pnpm run test:layers`.
7. `package.json` must expose `perf:report:25k` for very-large soak profiling.
8. CI/local operators can distinguish compile regressions from runtime regressions without manual instrumentation.
9. `scripts/perf-ingestion-budget.ps1` must enforce full-index/incremental-index + peak RSS thresholds for the 25k ingestion lane.
10. `scripts/perf-ingestion-budget.ps1` profile mode must support larger parameter sets (for example `-Profile 90k`) with parameter-appropriate default thresholds.
11. Optional commit-graph preconditioning lane (`perf:report:25k:precondition`) must continue to run through `scripts/perf-report.ps1`.
12. `apps/desktop/src-tauri/src/bin/perf-report.rs` must emit file-change stats-state counts (`pending|ready|failed`) and JIT enrichment timing phase.
13. `scripts/perf-ingestion-budget.ps1` must support warm/cold lanes and enforce diff-share regression checks (`ingest_file_diff_extract_ms / core_end_to_end`).
14. Warm ingestion lanes must be exposed in `package.json` (`perf:ingestion:25k:warm`, `perf:ingestion:90k:warm`).
15. `apps/desktop/src-tauri/src/bin/perf-report.rs` must emit commit throughput telemetry by lane/phase (`full|incremental|aggregate`) with bottleneck phase attribution.

## Non-Goals

1. No requirement to make Windows-only scripts cross-platform.
2. No slowdown of traversal/runtime algorithm paths due to automation checks.
3. No policy that blocks performance-oriented refactors by default.

## Code Hygiene Guardrails

To block AI/human duct-tape regressions, `scripts/enforce-guardrails.ps1` rejects:

1. Rust comment markers that indicate temporary shortcuts (`// HACK`, `// WIP`, `// WORKAROUND`, `// TEMP`, `// QUICKFIX`, `// DUCKTAPE`).
2. Rust lint-suppression shortcuts in source (`#[allow(clippy::...)]`, `#[allow(dead_code)]`, `#[allow(unused_...)]`) that bypass normal quality gates.

## Ingestion Guardrails

Repository-scale indexing guardrails now also enforce:

1. no process-spawned Git CLI hot path in `adapters/git.rs`
2. streaming ingest APIs for commits and file changes
3. bounded buffering in `application/indexer.rs` for commit/file-change batches
4. bounded Rayon chunk parallelism for file-change compute in `adapters/git.rs`
5. no database dependency in `adapters/git.rs` so DB writes stay single-owner in indexer/db adapters
6. explicit single-writer ingestion orchestration in `application/indexer.rs` via `GraphStore`
7. LMDB-backed DB adapter contract in `adapters/db.rs` (rw txn + commit semantics)
8. persisted CSR graph hot-path contract in `adapters/db.rs` (`parents/children` offsets + neighbors)
9. bounded rename detection in `adapters/git.rs` (`rename_limit`, `rename_threshold`, `exact_match_only`)
10. prefix-set incremental file-change cleanup in `adapters/db.rs` (no relational_query temp-table/delete-loop path)
11. ingestion phase telemetry contract in perf report (`ingest_commit_walk_ms`, `ingest_file_diff_extract_ms`, `ingest_file_change_upsert_ms`, `ingest_tx_commit_ms`)
12. lazy two-stage diff extraction in `adapters/git.rs` (`diff_requires_line_stats` + selective `Patch::from_diff` line stats)
13. adaptive-but-bounded Rayon planning in `adapters/git.rs` for chunk/channel sizing (`parallel_file_change_plan`)
14. metadata-only file-change hot ingestion mode in `adapters/git.rs` with selective JIT line-stats enrichment path
15. file-change stats-state + retryable enrichment-queue persistence contract in LMDB (`stats_state`, `enrichment_queue`)
16. retryable enrichment scheduling in `adapters/db.rs` (`attempts`, `last_error`, delayed `enqueued_at_epoch`, terminal `failed` at max attempts)
17. dedicated bounded Rayon pool + adaptive incremental-friendly parallel threshold in `adapters/git.rs` (`GITGRAPH_FILE_CHANGE_PARALLEL_THREADS`, `GITGRAPH_FILE_CHANGE_PARALLEL_MIN_COMMITS`)
18. no legacy relational_query runtime modules in `application/*` (`graph_queries.rs`, `query_graph_cache.rs`, `indexer_pipeline.rs`)
19. no relational_query migration files under `apps/desktop/src-tauri/migrations`

This protects against memory-regression classes on large histories while preserving domain/API contracts.

## Policy and Sync Guardrails

To prevent process drift in AI-assisted development, guardrails also enforce:

1. required standards docs under `docs/standards`
2. required section structure in each standards doc
3. `docs/standards/README.md` index sync with standards files
4. `docs/adr/README.md` index sync with all existing ADR files
5. presence of `docs/ai-change-survival-playbook.md` referenced by `AGENTS.md`

## References

1. GitHub workflow shell behavior (`cmd` default on Windows, different defaults elsewhere): https://docs.github.com/en/actions/writing-workflows/workflow-syntax-for-github-actions
2. GitHub-hosted runner path/env guidance (`HOME`, `GITHUB_WORKSPACE`): https://docs.github.com/en/actions/reference/github-hosted-runners-reference
3. Building and testing PowerShell in GitHub Actions (`pwsh`): https://docs.github.com/actions/automating-builds-and-tests/building-and-testing-powershell
4. PowerShell automatic variables (`$IsWindows`): https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_automatic_variables
5. PowerShell environment variables across platforms: https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_environment_variables
6. PSScriptAnalyzer GitHub Action reference: https://github.com/microsoft/psscriptanalyzer-action

