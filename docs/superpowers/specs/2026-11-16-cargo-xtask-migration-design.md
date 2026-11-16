# Cargo xtask migration: design

Date: 2026-11-16
Status: approved by the owner on 2026-11-16
Sub-project: S2 of the cleanup program. Read `2026-11-16-cleanup-program-roadmap.md` first. S0 (toolchain) and S1 (bug fix, ADR-0019) land before this one.

## Context

The owner wants a line-by-line cleanup and optimization of the whole repository. That work cannot start yet, because nothing can be verified on macOS or Linux workstations:

- `package.json` launches every gate through `powershell`, which exists only on Windows. That is 30 aliases, plus 2 direct calls in `scripts/perf-ablation.ps1:325,335`.
- `rust-toolchain.toml` pins only the `x86_64-pc-windows-msvc` target.
- `scripts/preflight.ps1` checks MSVC `cl.exe`, WebView2 and `cargo-tauri`. The product has no Tauri dependency.
- Seven scripts throw on a Windows machine without a D: drive.
- `dev:gate` runs in Docker by default (`scripts/dev-loop.ps1:4`).
- CI (`.github/workflows/rust-ci.yml`) runs Ubuntu and Windows only. It has no macOS leg.

This sub-project replaces all automation with a Rust `cargo xtask` crate. The setup then needs only Rust, a platform C toolchain and git ≥2.32 on macOS, Linux and Windows. The order of the other sub-projects, and every carried-forward research decision, is in the roadmap.

## Goals

1. Every gate runs natively on macOS, Linux and Windows through `cargo xtask`.
2. No guardrail check is weakened (AGENTS.md rule 6). Every one of the 60 `Add-Failure` sites in `scripts/enforce-guardrails.ps1` is ported, re-targeted, or replaced by an equal or stronger check. ADR-0020 records which. (ADR-0019 is reserved for S1, following the blueprint's numbering.)
3. Each piece of automation logic exists once and is reused.
4. Dangling code is removed rather than ported.

## Non-goals

- No change to product code under `apps/desktop/src-tauri/src`, except what a failing baseline forces us to report. Fixes belong to later sub-projects.
- No change to the perf telemetry contract of `src/bin/perf-report.rs`.
- No new perf budgets or thresholds. The existing values are ported unchanged.

## Decisions

| Topic | Decision |
|---|---|
| Automation language | Rust `xtask` crate. PowerShell is retired. |
| Migration proof | Parity port. Old and new run side by side until outputs match, then the `.ps1` is deleted. |
| pnpm and Node | `package.json` is deleted. Node and pnpm are no longer required. |
| Docker gate lanes | Deleted. Gates run natively. |
| D: drive rule | Deleted. `doctor` warns on Windows when the target dir is inside OneDrive. |
| Windows shell helpers | `enter-vs-dev-shell.ps1` and `preflight.ps1` are deleted and replaced by `cargo xtask doctor`. |
| `safe-install.ps1`, `faculty-server-sandbox.ps1` | Deleted. Nothing references the sandbox script. |
| Dangling flags | Removed when confirmed dangling (see "Cut rule"). |

## Architecture

### Crate

- `xtask/` at the repo root. It is a workspace member but not in `default-members`.
- `[lints] workspace = true`, so `unsafe_code = "forbid"`, `unwrap_used` and `expect_used` stay denied.
- `.cargo/config.toml` gets the alias `xtask = "run --package xtask --"`.
- Its only dependency is `regex`, which is already in `Cargo.lock`. Arguments are parsed by hand with std. Processes run through `std::process::Command`.
- xtask is tooling, so the product's application/adapters/domain/support layering does not apply. Each command gets its own module, and no logic goes in `main.rs` beyond dispatch.

```
xtask/src/
  main.rs          dispatch only
  run.rs           spawn, time, App Control detection
  paths.rs         repo root, target dir (honors CARGO_TARGET_DIR)
  report.rs        perf-report output parsing
  doctor.rs
  gate.rs
  guardrails/      check table + runner
  perf/            report.rs, budget.rs, ingestion.rs, ablation.rs, tiers.rs
  test_layers.rs
```

### Command mapping

| Old | New |
|---|---|
| `pnpm run guardrails` | `cargo xtask guardrails` |
| `safe:fast`, `dev:gate:fast`, `dev:gate:host:fast`, `quality:rust:fast` | `cargo xtask gate --fast`, which runs guardrails first |
| `safe:full`, `dev:gate`, `dev:gate:host`, `quality:rust` | `cargo xtask gate` |
| `quality:rust:docker`, `quality:rust:docker:fast` | removed |
| `preflight` | `cargo xtask doctor` |
| `quality:perf` | `cargo xtask perf budget` |
| `perf:report`, `perf:report:smoke`, `:large`, `:25k`, `:25k:precondition`, `:90k`, `:100k:hardware` | `cargo xtask perf report --tier default\|smoke\|large\|25k\|25k-pre\|90k\|100k-hw` |
| `quality:perf:ingestion`, `perf:ingestion:*` | `cargo xtask perf ingestion --profile 25k\|90k\|100k [--lane warm] [--fast] [--hardware-max]` |
| `perf:ablation:25k` | `cargo xtask perf ablation` |
| `test:layers` | `cargo xtask test-layers` |

`gate` runs, in order:

1. guardrails;
2. `cargo fmt --all -- --check`;
3. `cargo clippy --workspace --all-targets -- -D warnings`;
4. `cargo check --workspace --locked`;
5. `cargo test --workspace --locked`.

`--fast` keeps the subset that `quality-gate.ps1 -Fast` runs today.

`doctor` checks:

- that rustc reports 1.94.0;
- that git is present at version 2.32 or later (the first with `GIT_CONFIG_GLOBAL`);
- that a C compiler works, since `lmdb-sys` and `libmimalloc-sys` compile C;
- that the workspace `Cargo.lock` exists;
- on Windows only, whether the target dir is inside OneDrive, as a warning rather than a failure.

### Reuse

| Module | Replaces |
|---|---|
| `run.rs` | `Invoke-Step`×2, `Invoke-ExternalPhase`×2, `Format-ElapsedSeconds`×2, `Test-IsPolicyBlocked`×2. It also holds the single git spawn helper. Every git call goes through it, with `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL` set to the null device, inherited `GIT_*` variables cleared, and `GIT_CEILING_DIRECTORIES` set for any fixture or worktree path (RT, ADR-0020 and 0021 rows). |
| `report.rs` | `Get-ReportValue`×3, `Parse-DoubleOrThrow` |
| `paths.rs` | `Normalize-AbsolutePath`×4, and 31 D: drive occurrences in 10 scripts |
| `guardrails` check table | about 25 hand-written code-contract blocks in `enforce-guardrails.ps1:265-561`, replaced by one `Contract { id, file, must_contain, message }` table and one loop |
| `perf/tiers.rs` | parameter blocks repeated across `perf-report.ps1`, `perf-ingestion-budget.ps1` and `perf-ablation.ps1` |

Deleted rather than reused: `Test-DockerReady`×2, `Invoke-DockerPhase`×2, `Get-DockerRunArgs`×2, `Assert-OnDDrive`×2 and `Assert-WindowsPathOnDDrive`×2.

### Cut rule

A flag, branch or file is removed only when all three hold:

1. Nothing in CI, docs, `package.json` or other code references it.
2. No guardrail contract requires it.
3. Its effect is dead, or the owner approved the cut.

Confirmed cuts:

- `perf-report.ps1` `-EnableLsmRocksdb` and `-LibclangPath` (lines 15, 20, 350-474). RocksDB is not a dependency.
- `perf-ablation.ps1`'s 14 manual flags, none referenced outside the script: `-Resume`, `-OnlyScenario`, `-RequiredScenario`, `-MinAggregateCps`, `-MinFullCps`, `-RequireQueryPositive`, `-MinCpsGainVsControlPercent`, `-CommitWalkResearch90k`, `-CooldownSeconds`, `-CooldownBetweenRunsSeconds`, `-CooldownCpuIdleThreshold`, `-CooldownProcPerfMinPercent`, `-CooldownMaxWaitSeconds` and `-NoCooldown`. The default 25k ablation run stays, including its fixed cooldown sleeps (10 s per scenario, 5 s per run, `perf-ablation.ps1:36-54`).
- `perf-ablation.ps1`'s CPU-counter cooldown polling (`:115-175`). It reads Windows-only `Get-Counter`. On Linux and macOS the read fails and returns -1, so the loop always waits the full 180 s. Only the fixed sleeps are ported. Approved by the owner on 2026-11-16.
- Docker lanes, the D: drive rule, the VS dev shell, `safe-install.ps1` and `faculty-server-sandbox.ps1`, all approved.

Kept although unreferenced, because a guardrail contract requires them: ingestion probe/profile/lane modes and the diff-share budget (`enforce-guardrails.ps1:928`), and perf-report probe mode (`:891`).

## Guardrail mapping

The 60 `Add-Failure` sites fall into six groups. The exact per-site table is the first deliverable of the plan and lives in ADR-0020.

| Group | Approx. sites | Content | Treatment |
|---|---|---|---|
| A. Repo structure | 6 | root `Cargo.lock`, no nested locks, layer dirs, no flat or legacy modules | port 1:1 |
| B. Source markers | 5 | `TODO`/`FIXME`/`dbg!`, duct-tape markers, lint-suppression attributes, relational terms | port 1:1 |
| C. Code contracts | 25 | required symbols in `adapters/git.rs`, `application/indexer.rs`, `adapters/db.rs` | port 1:1, data-driven. Each row is tagged with the ADR whose behavior test will replace it (for example, LMDB contracts are tagged 0022 and `delta_log` contracts S5). |
| D. Governance docs | 4 | standards sections, standards README, ADR README references | port 1:1 |
| E. Script meta-checks | 12 | platform header, portable-script policy, required perf files, probe/fallback contracts, `package.json` aliases, CI step names | re-target (below) |
| F. Anti-tamper | 1 | policy change without implementation change | port, with paths re-pointed |

Group E re-targets:

- `package.json` aliases become a test that every required `cargo xtask` subcommand exists.
- "Required perf files" becomes a check that the `xtask/src/perf/` modules exist.
- Probe mode becomes a `--probe` flag on perf commands. The CI "Script Contract Probe" step stays and calls it.
- The Docker "resilient fallback" contract is removed with Docker. App Control detection (`os error 4551`) stays in `run.rs`.
- The platform header and portable-script policy are replaced by a stronger check: no `*.ps1`, no `package.json`, and no `powershell` or `pwsh` invocation in the repo.
- The perf-report binary telemetry contract (`:977`) and CI "Performance Budget" step (`:986`) are ported unchanged.

Anti-tamper paths:

- **Policy:** `docs/**`, `xtask/**`, `.github/workflows/rust-ci.yml`, `README.md`, `CONTRIBUTING.md`.
- **Implementation:** product `src/**/*.rs`, product `tests/**/*.rs`, product `Cargo.toml`, root `Cargo.toml`.

The existing loophole, where a root `Cargo.toml` edit counts as implementation, is kept as it is and documented in ADR-0020.

## Parity

| Area | Method | Pass condition |
|---|---|---|
| Guardrails | `xtask/tests/guardrail_parity.rs` creates a `git worktree` under `target/` through the `run.rs` git helper, with `core.ignorecase=false`. For each check it plants one violation, then runs `pwsh scripts/enforce-guardrails.ps1` and `cargo xtask guardrails --repo <worktree>`. | Both fail with the same check ID per violation. Both pass on a clean tree. A missing pwsh fails the test rather than skipping it. |
| Gate | Run old and new on 4 trees: clean, a fmt violation, a clippy warning, and a failing test. | Identical exit codes. |
| Perf | Invocation parity: the same `perf-report` binary args and env vars for every tier and profile. Verdict parity: the same pass or fail on recorded reports (one passing, one over budget, one missing a field). | Identical args, env and verdicts. Timings are never compared. |

The parity test is deleted in Phase 4. The 60 planted-violation fixtures stay as permanent xtask tests, so every check keeps proof that it fires.

## Phases

Each phase is one PR. Each must pass guardrails (old and new while both exist) and the gate.

| Phase | Work | Exit criteria |
|---|---|---|
| 0. Toolchain | Done in roadmap S0, before S1. This sub-project starts from that baseline, with pwsh still installed for parity. | S0 and S1 complete. |
| 1. Skeleton | `xtask` crate with `run.rs`, `paths.rs`, `report.rs`, `doctor` and `gate`. Gate parity. CI: add `macos-latest` to the matrix, and run `cargo xtask gate` alongside the old steps. Create ADR-0020 and reference it in `docs/adr/README.md`. Commit this spec. | Gate parity green on 3 OSes in CI. |
| 2. Guardrails | Data-driven check table, 60 planted-violation fixtures, parity test. CI runs both. | Parity green. Every site mapped in ADR-0020. |
| 3. Perf | `perf report`, `budget`, `ingestion`, `test-layers` and `ablation`, with the 14 flags and the counter polling cut. The streaming-CSR scenarios are already gone after S1, so they are not ported. Invocation and verdict parity. | Parity green. |
| 4. Delete and switch | Delete the 13 `.ps1`, `package.json` and the Docker lanes. Activate the group E re-targets and the new no-PowerShell check. Update AGENTS.md, `docs/engineering-system.md`, `docs/from-scratch-system.md`, `docs/automation-guardrails.md` and the standards references to `cargo xtask`. Drop pwsh steps from CI. Delete the parity test. | `cargo xtask gate` green on 3 OSes. No `.ps1` or `package.json` left. ADR-0020 complete. |

## Rollback

- Phases 1 to 3: the old scripts keep running. Rollback means removing `xtask/` and its workspace entry.
- Phase 4: reverting the phase commit restores the scripts, `package.json` and the docs.

## Risks and open items

- **macOS CI cost.** On a private repo, macOS runner minutes cost 10x Linux minutes.
- **Canned reports for verdict parity.** The perf scripts may not accept a pre-recorded report. If one cannot be fed in without editing the `.ps1`, verdict parity falls back to comparing two real smoke runs on the same recorded output files. The plan resolves this per script.
- **Case-insensitive filesystems.** macOS and Windows set `core.ignorecase=true` on `git init` (see `reports/FinalGitGraph blueprint adversarial review.md`). Any fixture repo built during parity or perf runs must be created with `-c core.ignorecase=false -c core.precomposeunicode=false`.
- **macOS perf numbers.** Perf budgets stay gated on Ubuntu only, as today. macOS numbers are informational.
- **Line count.** No line-count or speed claim is made in advance (AGENTS.md rule 8). Results are measured after Phase 4.
