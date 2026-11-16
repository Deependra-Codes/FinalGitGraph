# ADR 0020: Portable Automation with cargo xtask

Date: 2026-11-16
Status: Accepted (Phases 1 and 2 of 4)

## Context

Every quality gate ran through PowerShell, called as `powershell` from `package.json`. That command exists only on Windows. Several scripts required a D: drive, `dev:gate` ran in Docker by default, `scripts/preflight.ps1` checked only Windows tools, and CI had no macOS leg. No gate could run on a macOS or Linux workstation.

This is sub-project S2 of `docs/superpowers/specs/2026-11-16-cleanup-program-roadmap.md`. The full design is in `docs/superpowers/specs/2026-11-16-cargo-xtask-migration-design.md`.

## Decision

1. Replace the 13 PowerShell scripts and `package.json` with a Rust `xtask` workspace crate, reached through the cargo alias `xtask = "run --quiet --package xtask --"`. Prerequisites become Rust 1.94.0, a platform C toolchain and git 2.32 or later.
2. Delete instead of porting:
   - `package.json` and the Node/pnpm requirement;
   - the Docker gate lanes;
   - the D: drive rule (`doctor` warns about OneDrive target dirs on Windows instead);
   - `enter-vs-dev-shell.ps1`, `safe-install.ps1` and `faculty-server-sandbox.ps1`;
   - confirmed-dangling flags.
3. Port in four phases, one PR each. Run old and new side by side, and delete a script only after its xtask replacement matches it:

   | Phase | Scope |
   |---|---|
   | 1 | `doctor` and `gate`, plus a macOS CI leg |
   | 2 | guardrails, with a per-check mapping table in this ADR |
   | 3 | perf commands |
   | 4 | delete the scripts and switch AGENTS.md, docs and CI |
4. Keep every guardrail (AGENTS.md rule 6). Each of the 60 `Add-Failure` sites in `scripts/enforce-guardrails.ps1` is ported, re-targeted, or replaced by an equal or stronger check. The mapping table arrives in Phase 2.

## Phase 1 Results

- `cargo xtask gate [--fast]` runs the same four steps as `scripts/quality-gate.ps1`, in the same order:
  1. `cargo fmt --all -- --check`
  2. `cargo clippy --workspace --all-targets -- -D warnings`
  3. `cargo check --workspace --locked`
  4. `cargo test --workspace --locked`

  It stops at the first failure, and `--fast` skips the test step. It does not run guardrails yet, matching the old gate. Guardrails join in Phase 2.
- `cargo xtask doctor` replaces `scripts/preflight.ps1`. It checks rustc 1.94.0, git 2.32 or later, a working C compiler (vswhere on Windows) and `Cargo.lock`. It drops the MSVC `cl.exe`, WebView2 and `cargo-tauri` checks: the product has no Tauri dependency, and cc-rs finds MSVC through vswhere.

Gate parity was measured on 2026-11-16 on macOS 15.8.2 arm64, comparing `pwsh scripts/quality-gate.ps1 -SkipPreflight` with `cargo xtask gate`:

| Tree | Old exit (failing step) | New exit (failing step) |
|---|---|---|
| clean | 0 | 0 |
| rustfmt violation | 1 (rustfmt check) | 1 (fmt) |
| clippy `clone_on_copy` | 1 (clippy) | 1 (clippy) |
| failing integration test | 1 (cargo test) | 1 (test) |
| failing integration test, `-Fast` / `--fast` | 0 | 0 |

CI now runs `cargo xtask doctor` and `cargo xtask gate` on `ubuntu-latest`, `windows-latest` and `macos-latest`, alongside the old steps. The repository is public, so macOS runner minutes are free. Phase 1 CI run 43927526794 passed on all three operating systems.

## Phase 2 Results

All 59 failure sites of `scripts/enforce-guardrails.ps1` are ported to `cargo xtask guardrails [--repo <path>]` in `xtask/src/guardrails/`:

- the plain checks (`structure.rs`, `docs.rs`, `scripts.rs`, `tamper.rs`);
- one data table of 38 file contracts (`contracts.rs`).

Matching is per line and case-insensitive, because `Select-String` and `-match` default to that. `cargo xtask gate` now runs guardrails before the cargo steps, which is what `safe:fast` and `safe:full` did.

### Parity

`xtask/tests/guardrails.rs` works like this:

1. It clones the working tree into a temporary directory and commits it as a base, then adds an empty commit so the anti-tamper fallback is deterministic.
2. It applies one planted violation per fixture.
3. It runs both `cargo xtask guardrails` and `pwsh scripts/enforce-guardrails.ps1`, and compares the multiset of check names. A check name is the message cut at its first `": "`, `" ("`, backtick or control character, because PowerShell turns backtick escapes into control characters.

On 2026-11-16 all 61 fixtures matched on both runners, and every check fired on its own fixture. The run took 47 s with 8 parallel clones. The one fixture without a PowerShell run is `scripts-dir`, because the script cannot run after its own directory is deleted. Phase 4 deletes the PowerShell half of the test, and the fixtures stay.

### Anti-tamper split (owner decision)

- `xtask/src/guardrails/**` is **policy**: editing guardrail rules alone is blocked, as editing `enforce-guardrails.ps1` alone was.
- Every other path under `xtask/` is **implementation**.

`enforce-guardrails.ps1` received the same two clauses in the same change. A root `Cargo.toml` edit still counts as implementation, as before.

### Documented differences

| Behavior | PowerShell | xtask |
|---|---|---|
| Nested `Cargo.lock` scan | whole repository, including `target/` and `.git/` | skips `target/` and `.git/`, where a lock file is never source and walking costs seconds |
| `git2::` present together with missing tape-first symbols | reports only the `git2` failure | reports both (strictly more) |
| Passing checks | prints `[OK]` lines | prints nothing |

### Mapping

"Owner" is the roadmap item whose behavior test or deletion replaces the name check. "S2 Phase 4" checks inspect the PowerShell scripts or `package.json`. Phase 4 deletes those files, and replaces the checks with:

- no `*.ps1`, `package.json` or `powershell`/`pwsh` invocation in the repository;
- `xtask/src/perf/` exists;
- the CI probe step calls `cargo xtask perf ... --probe`.

| # | PowerShell line | Check | Fixture | Owner |
|---|---|---|---|---|
| 1 | 152 | Root Cargo.lock is missing. | `root-lock` | — |
| 2 | 161 | Nested Cargo.lock files must not exist | `nested-lock` | — |
| 3 | 166 | Missing Rust source root | `src-root` | — |
| 4 | 174 | Layer directory missing | `layer` | — |
| 5 | 184 | Unexpected root Rust files found | `stray-root` | — |
| 6 | 200 | Legacy flat modules should not exist | `legacy-flat` | — |
| 7 | 217 | Forbidden markers found in Rust source tree | `marker` | — |
| 8 | 234 | Duct-tape quality markers are blocked | `duct-tape` | — |
| 9 | 250 | Lint-suppression duct-tape attributes are blocked | `lint-allow` | — |
| 10 | 264 | Legacy relational runtime modules must not exist | `legacy-runtime` | — |
| 11 | 574 | Legacy relational migration files must be removed | `migration` | — |
| 12 | 610 | Forbidden legacy relational terms detected | `relational-doc` | ADR 0022 |
| 13 | 642 | Required governance doc missing | `required-doc` | — |
| 14 | 648 | Missing scripts directory | `scripts-dir (xtask only)` | S2 Phase 4 |
| 15 | 671 | Script missing platform metadata | `platform-header` | S2 Phase 4 |
| 16 | 690 | Portable script policy violation | `portable` | S2 Phase 4 |
| 17 | 729 | Docs/guardrails changes without implementation changes are blocked | `tamper-*` | — |
| 18 | 748 | Required performance file missing | `perf-file` | S2 Phase 4 |
| 19 | 835 | docs/standards/README.md must list | `standards-index` | — |
| 20 | 852 | Standards file is missing required section | `standards-section` | — |
| 21 | 869 | docs/adr/README.md must reference ADR file | `adr-index` | — |
| 22 | 271 | Git adapter hot ingestion path must be in-process | `git-cli` | ADR 0024 |
| 23 | 277 | Git adapter must not reintroduce git2/libgit2 APIs | `git2` | ADR 0024 |
| 24 | 287 | Git adapter must keep tape-first gix ingestion path | `tape-first` | ADR 0024 |
| 25 | 291 | Legacy full-buffer git command helper must not be reintroduced | `git-helper` | ADR 0024 |
| 26 | 302 | Git adapter must expose streaming commit/file-change ingestion APIs | `streaming-api` | ADR 0024 |
| 27 | 318 | Git adapter must keep bounded Rayon parallel file-change compute path | `rayon` | ADR 0024 |
| 28 | 322 | Git adapter must remain DB-free | `git-db-free` | ADR 0022 |
| 29 | 334 | Git adapter must enforce bounded rename detection | `rename` | ADR 0024 |
| 30 | 343 | Git adapter must keep bounded producer/consumer channel | `channel` | ADR 0024 |
| 31 | 353 | Git adapter must keep adaptive-but-bounded Rayon planning | `rayon-plan` | ADR 0024 |
| 32 | 367 | Git adapter must keep two-stage selective line-count extraction | `line-stats` | ADR 0024 |
| 33 | 378 | Git adapter must keep diff sub-phase telemetry contract | `diff-telemetry` | ADR 0025 |
| 34 | 388 | Git adapter must keep SHA validation and bounded object-cache sizing guards | `object-cache` | ADR 0024 |
| 35 | 398 | Git adapter must enforce metadata-only hot ingestion | `metadata-only` | ADR 0024 |
| 36 | 407 | Indexer must use streaming commit ingestion | `indexer-stream` | ADR 0024 |
| 37 | 416 | Indexer must keep bounded file-change batch buffers | `indexer-batch` | ADR 0024 |
| 38 | 428 | Indexer must orchestrate ingestion writes through GraphStore | `indexer-store` | ADR 0022 |
| 39 | 439 | Indexer query hot path must keep integer-based CSR traversal state | `indexer-query` | ADR 0023 |
| 40 | 453 | DB adapter must keep LMDB GraphStore transaction contract | `db-txn` | ADR 0022 |
| 41 | 466 | DB adapter must keep persisted CSR snapshot contract | `db-csr` | ADR 0022 |
| 42 | 476 | DB adapter must keep explicit graph snapshot metadata/versioning contract | `db-metadata` | ADR 0022 |
| 43 | 486 | DB adapter must keep append-only delta mutation log contract | `db-delta` | S5 |
| 44 | 498 | DB adapter must keep bounded graph overlay adjacency plus compaction contract | `db-overlay` | ADR 0022 |
| 45 | 502 | DB adapter must remain LMDB-only | `db-statements` | ADR 0022 |
| 46 | 514 | DB adapter must keep prefix-set cleanup path | `db-prefix` | ADR 0022 |
| 47 | 525 | DB adapter must enforce stats-state + retryable enrichment queue contract | `db-enrichment` | ADR 0024 |
| 48 | 540 | DB adapter must keep centralized LMDB mmap/runtime flag parsing | `db-flags` | ADR 0022 |
| 49 | 551 | DB adapter must keep commit-window hot path on index/range-seek reads | `db-window` | ADR 0022 |
| 50 | 561 | DB adapter must keep due-index ordered enrichment acquisition path | `db-due-index` | ADR 0024 |
| 51 | 771 | Performance budget script must keep probe mode | `perf-budget-script` | S2 Phase 4 |
| 52 | 891 | Performance report script must keep probe mode | `perf-report-script` | S2 Phase 4 |
| 53 | 928 | Ingestion budget script must keep profile/lane/probe modes | `ingestion-script` | S2 Phase 4 |
| 54 | 977 | Performance report binary must emit phase timing + memory visibility contract | `perf-binary` | ADR 0025 |
| 55 | 986 | Rust CI workflow must include a Performance Budget step | `ci-budget` | ADR 0025 |
| 56 | 992 | Rust CI workflow must include a Script Contract Probe step | `ci-probe` | S2 Phase 4 |
| 57 | 1001 | package.json must include a quality:perf script | `package-perf` | S2 Phase 4 |
| 58 | 1007 | package.json must include a quality:perf:ingestion script | `package-ingestion` | S2 Phase 4 |
| 59 | 1025 | package.json must include perf:report ... and test:layers scripts | `package-tiers` | S2 Phase 4 |

## Consequences

- During Phases 1 to 3, CI runs both the old steps and the xtask steps, which costs some extra CI time. Phase 4 removes the old steps.
- `xtask` has no dependencies in Phase 1. Phase 2 adds `regex` 1.12.3, which is already in `Cargo.lock`.
- The guardrails do not yet know about `xtask/`. Phase 2 classifies `xtask/**` as policy in the anti-tamper rule.

## Alternatives Considered

1. **Keep PowerShell and make it portable** by switching to `pwsh` and dropping the D: drive rule. This is a smaller change, but it keeps three languages and untestable scripts. The owner chose xtask.
2. **A Docker dev container for every gate.** Perf numbers from a VM do not represent native macOS or Windows, and native Windows and macOS would stay untested.
3. **A big-bang replacement with no parity runs.** A lost guardrail would be invisible, which breaks AGENTS.md rule 6.
