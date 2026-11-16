# Cleanup and optimization program: roadmap and decision register

Date: 2026-11-16
Status: approved order, decisions carried forward from research

This file is the single place where research decisions are carried forward. Every sub-project spec and ADR in this program must read it first, and must update the register when it settles or changes a decision.

Sources:

- `reports/FinalGitGraph backend architecture blueprint.md` (BP): the proposed rebuild.
- `reports/FinalGitGraph blueprint adversarial review.md` (RT): the red-team review of BP.

## Program goal

The owner's goal is a cleanup and optimization of every line of the repository. The research changes how that goal is reached:

- Most of today's storage and query code is scheduled for replacement. That covers `adapters/db.rs` and its 17 LMDB tables, the whole-blob CSR and overlay, `QueryGraph` hydration, the Bloom reader and the 300 s lookback.
- Polishing code that will be deleted is wasted work.
- So the optimization happens in two places: correct, minimal code for whatever survives, and new core code written to the corrected design.

## Cleanup rule

1. Do not polish code that a later sub-project deletes. Fix only correctness and data-loss bugs in it.
2. A module gets line-level cleanup when the sub-project that owns it touches it, or once it is confirmed to survive the rebuild.
3. Dangling code is removed rather than ported. Dangling means unreferenced by CI, docs or code, required by no guardrail, and dead in effect or approved for removal by the owner.
4. Write each piece of logic once and reuse it. Duplicates get merged.
5. No performance or architecture claim is made without in-repo, digest-checked measurement (AGENTS.md rule 8). Every kill criterion gets numeric thresholds before anything is measured.

## Sub-project order

| # | Sub-project | Owns ADR | Depends on |
|---|---|---|---|
| S0 | Toolchain and baseline | none | none |
| S1 | Stop wrong answers: corrected Phase 0 | 0019 | S0 |
| S2 | Portable automation: `cargo xtask` | 0020 | S1 |
| S3 | Typed config and test-artifact contract | 0021 | S2 |
| S4 | Proof against git: oracle and verification contract | 0025 | S3 |
| S5 | Dead-code deletion in the old path | none, or an amendment to 0019 | S4 |
| S6 | New core beside the old | 0022, 0023, 0024 | S4 |
| S7 | Cutover: LMDB and old core removed | 0022 | S6 |
| S8 | Line-level cleanup of surviving modules | per module | S7, or earlier for modules confirmed to survive |

### S0. Toolchain and baseline

- Install rustup at user level on this Mac (no sudo); `rust-toolchain.toml` then installs 1.94.0.
- Install pwsh as a user-level tarball, so the existing gates can run until S2 retires them.
- Remove `targets = ["x86_64-pc-windows-msvc"]` from `rust-toolchain.toml` (BP ledger; the host target is always installed).
- Baseline: fmt, clippy, check, test, the pwsh guardrails and perf smoke. Record the results. If the baseline is red, stop and report to the owner.

S0 result, 2026-11-16, on macOS 15.8.2 arm64 with Apple Git 2.39.5, rustc 1.94.0 and pwsh 7.6.8:

| Step | Result |
|---|---|
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass |
| `cargo check --workspace --locked` | pass |
| `cargo test --workspace --locked` | pass, 53 tests |
| `pwsh scripts/enforce-guardrails.ps1` | pass |
| `pwsh scripts/perf-budget.ps1 -ProbeOnly` | pass |
| `pwsh scripts/perf-budget.ps1` | pass |
| `pwsh scripts/perf-report.ps1` (smoke tier) | **fail**: `git ["rev-parse", "HEAD"] failed: fatal: ambiguous argument 'HEAD'` |
| The same smoke run with `GIT_CONFIG_NOSYSTEM=1` | pass: 501 commits, 580 edges |

Root cause of the smoke failure: the fixture generator runs a bare `git init` (`apps/desktop/src-tauri/src/bin/perf-report.rs:596`) and inherits the machine's git config. Apple Git's system config (`/Library/Developer/CommandLineTools/usr/share/git-core/gitconfig`) sets `init.defaultBranch=main`, so HEAD points to `refs/heads/main`, while the fast-import stream writes `refs/heads/master` (`:712`, `:729`, `:777`, `:821`). CI passes only because Ubuntu's git has no default-branch setting. This is the unpinned-fixture-environment class that RT flags for ADR-0021.

Fixed on 2026-11-16, before S1, at the owner's request:

- The test `branching_fixture_ignores_inherited_git_environment` was written first. It re-runs its own test binary as a child, whose environment exports `GIT_DIR` pointing at a sentinel bare repo (as a git hook does) and sets `init.defaultBranch=trunk` through `GIT_CONFIG_COUNT`. Before the fix, the fixture was redirected into the sentinel repo and HEAD did not resolve. The test mutates no process-wide env.
- Both fixture generators now share one `init_fixture_repo` helper, which runs `git init --initial-branch=master`. The branch stays explicit because git 3.0 changes the default to `main`. The duplicated 11-line init block is gone.
- Every fixture git spawn (`run_git`, `run_git_capture`, `run_git_fast_import`) now goes through one `pinned_git` builder. It clears inherited `GIT_*` variables and sets `GIT_CONFIG_NOSYSTEM=1` and `GIT_CONFIG_GLOBAL` to the null device (`NUL` on Windows). This is ADR-0021's fixture-environment pinning, pulled forward for `perf-report` only. Windows behavior is verified by the CI `windows-latest` test job, not locally.
- The same test also plants a hostile global config (`HOME/.gitconfig` with an `init.templateDir` marker). A mutation check confirmed it fails when the `GIT_CONFIG_GLOBAL` pin is removed. CI therefore checks the pin on each OS it runs.
- `init_fixture_repo` sets `core.ignorecase=false`. `git init` probes the filesystem and writes `true` on APFS and NTFS, which env pinning cannot prevent. The test `fixture_repo_keeps_case_only_paths_distinct` was written first: a fast-import of `Makefile` and `makefile` collapsed to one path on this Mac, and now keeps both.
- After the fix, every S0 step passes on this Mac with no environment workaround: 55 tests, guardrails, perf smoke and perf budget.
- Verified on Windows by CI run 43920277148 (commit `6df0b5c`). Both new tests passed on `windows-latest`, covering the `NUL` global-config pin and the NTFS ignorecase case. Ubuntu and the 25k ingestion budget lane also passed.

Another finding along the way: the red-team's recommended catalog engine collides with the relational-term guardrail. See the LMDB row of the decision register.

### S1. Stop wrong answers (ADR-0019)

This sub-project follows the corrected Phase 0 from RT ("Migration plan" and "Changes by ADR", row 0019):

- Write the failing tests first. Run them as a CLI subprocess with `env_clear()`, with strictly increasing commit dates, and assert on sets rather than counts.
- Cover three cases:
  - a 20,000-commit history with a fork before commit 16,384 that merges after it;
  - an incremental run of more than 8,192 commits;
  - a merge of an old-dated branch.
- The cheapest first assertion uses the query results that `bin/perf-report.rs` already computes and discards (`let _ =`), with its existing fast-import fixture generator (`bin/perf-report.rs:609-626`).
- Delete only these:
  - the per-wave call site (`application/indexer.rs:1595-1606`);
  - the full-mode empty-snapshot init (`:404-407`);
  - `GITGRAPH_LEVEL3_STREAMING_CSR` and `GITGRAPH_LEVEL3_STREAMING_CSR_MODE`.
- Keep `build_incremental_csr_snapshot` and `apply_csr_delta_from_commit_shas`. `compact_graph_overlay_into_snapshot` calls them (`adapters/db.rs:1813-1830`, `:1615`), and the guardrail requires compaction (`scripts/enforce-guardrails.ps1:489-499`).
- Repair existing indexes, with either a format bump or a rebuild on open when stored edges are fewer than the sum of parents.
- For the old-dated-merge pruning bug, either hotfix the walk so it stops only at already-indexed OIDs, or declare incremental mode unsound until S6. The owner chooses in the S1 spec.
- Remove the streaming-mode settings from `scripts/perf-ablation.ps1:443,459` and `scripts/perf-report.ps1:467`.
- Fix the duplicate ADR number 0015 (`0015-level-1-index-session-planner.md` and `0015-lmdb-csr-delta-storage.md`).

Both bugs are so far confirmed only by reading the code. The S1 tests are the first executable proof.

S1 result, 2026-11-16 (details in `docs/adr/0019-remove-streaming-csr-and-age-only-pruning.md`):

- **Bugs reproduced.** Four tests written first failed for the predicted reasons. A 20,000-commit full index lost exactly 2 edges. A 9,000-commit incremental run broke reachability from HEAD. An old-dated merged branch lost 5 commits. A pre-fix checkpoint was still trusted.
- **Owner's choice on pruning.** The owner chose the hotfix. All three walkers now prune a commit older than the cutoff only when the index already holds it, through one shared `cutoff_action` and an `IncrementalCutoff { since_epoch, is_indexed }` input.
- **Repair.** Repair uses a versioned checkpoint key. Each existing index re-indexes in full once.
- **Duplicate ADR number.** The duplicate 0015 was fixed by renumbering the planner ADR to 0026, which keeps 0019 to 0025 free for this program. Nine dead links to a nonexistent 0015 file name were corrected.
- **Gates and perf.** All gates pass. On the host 25k cold lane (median of 3), full index time went from 120.2 to 99.3 ms. Incremental time stayed within noise, and peak RSS went from 89.2 to 83.0 MB.

### S2. Portable automation (ADR-0020)

See `2026-11-16-cargo-xtask-migration-design.md`.

### S3 to S8

Each gets its own spec when it starts. The register below lists what each one inherits.

## Decision register

Status values:

- **Adopted**: BP holds as written.
- **Adopted, corrected**: BP holds after the RT correction listed.
- **Open**: a decision is still needed. The owner ADR decides it.
- **Decided**: settled in this program's brainstorming.

| Topic | BP | RT verdict and correction | Carried-forward decision | Owner | Status |
|---|---|---|---|---|---|
| LMDB (`lmdb 0.8`, 17 tables) | Remove | The idea holds: immutable layers beat a KV engine for an append-only DAG. "No database" breaks, because the ref map, repo registry, derivation attempt counts, `unavailable` facts and lazy line stats have no home. | Remove LMDB at S7. Until then, no LMDB hardening except drain atomicity. The store for mutable state is open. RT recommends an embedded relational WAL catalog, but `scripts/enforce-guardrails.ps1:580-610` bans relational engine names in src, tests, scripts and docs. ADR-0022 must either choose a non-relational store or amend that guardrail with a stated reason, in the same change (AGENTS.md rule 6). redb only at ≥4.5 with SingleWriter. heed needs an `unsafe` ADR. | 0022 | Remove: adopted. Catalog store: open. |
| Whole-blob CSR, overlay, streaming delta | Replace with append-only topo layers and geometric merge | "max-commits" misreads git: drop it or define it. Write amplification must be re-derived against the S1 baseline, since cold layered writes are 4–12x more than one rebuild. The real benefits are partial results during a first index and O(wave) incremental runs. | Streaming path removed in S1. Layers designed in S6 with the corrections. | 0019, 0022 | Adopted, corrected |
| Layer publish protocol | Atomic rename of `MANIFEST`, `WRITER.lock` | Lost layer: no compare-and-swap between compaction and ingest. GC is unsafe across processes. A second writer can start after the lock file is deleted. | Single publisher with a generation CAS (via the catalog), a cross-process shared or exclusive meta lock, GC only by the lock holder, a writer-identity check, unique segment names, no writer mode on NFS, SMB or 9p. | 0022 | Adopted, corrected |
| Layer container | Custom chunk format, little-endian, 64-byte aligned, xxh3 | Arrow IPC was never weighed. The parser must be fuzzed. | Weigh Arrow IPC against the custom format, fuzz whichever is chosen. | 0022 | Open |
| `read()` versus mmap | Read whole layers at open | The real open cost is 0.5–1.2 GB, not 360 MB. | Lazy, per-chunk verified reads. mmap only behind its own `unsafe` ADR. | 0022 | Adopted, corrected |
| bincode on hot paths | Remove | not attacked | Fixed binary layout. | 0022 | Adopted |
| Commit identity | Parent-first dense IDs | Holds: 0 violations on git.git across 200 refreshes. Unshallow, replace refs and grafts break "append-only". | Named renumber triggers force a blue/green renumber, and derived facts are keyed so they can be remapped. | 0023 | Adopted, corrected |
| Committer-time order, 300 s lookback | Replace with a ref-snapshot frontier | The current code drops old-dated merges, a bug the audit missed. | Fix or declare it in S1. Replace it in S6. | 0019, 0024 | Adopted, corrected |
| `QueryGraph` hydration | Replace with an Engine-owned `ArcSwap<Snapshot>` and OID fanout | The memory gain is 6–8x, not 4x. Output order must not depend on IDs. | ID-independent output order with OID-keyed cursors. | 0023 | Adopted, corrected |
| Shortest path and traversal APIs | Keep the APIs, replace the implementations with ID-interval pruning | Interval pruning is wrong for `direction=both` (112 of 120 git.git pairs). | Interval pruning only for `parents` and `children`. Bounded BFS for `both`. Tri-state result that includes "budget exhausted". | 0023 | Adopted, corrected |
| gix | Keep, upgrade 0.80 to 0.9x | Mostly holds. The upgrade is a semantic migration. | Treat the upgrade as a migration with its own fixtures. | 0024 | Adopted, corrected |
| Commit-graph fast path | Keep as bootstrap, add guards | Guards are missing for shallow, replace refs and grafts. | Add the guards. Its digest must equal the object walk's. | 0024 | Adopted, corrected |
| Pack tape walker (ADR 0017) | Replace unless measured faster | The rule is sound. | Keep only on a digest-checked win over gix-pack, by a margin written down before measuring. | 0024 | Adopted |
| Bloom reader | Remove | It can go, but not for the reason BP gave. | Remove at S7. | 0022 | Adopted |
| Async file-change extraction | Keep the idea; the retry queue becomes a set difference | Attempt counts and `unavailable` overrides still need storage. | Store them in the catalog. | 0024 | Adopted, corrected |
| Derived facts | "Never invalidated" | Breaks. Measured: `git replace --graft` changed a diff while the OID stayed the same. | Ingest raw objects with replacements off. Record digests of the replace set, shallow set, mailmap and diff policy. | 0024 | Adopted, corrected |
| Throughput target | ≥100k commits/s | Change sets with merges run about 10.2k commits/s per core, so 100k/s needs at least 10 cores. | Gate topology throughput only. Change sets are reported, not gated, at 100k/s. | 0025 | Adopted, corrected |
| `delta_log`, `planner_sessions`, planner risk fields | Remove (written, never read) | Guardrails require `delta_log` and `append_delta_event`. | Remove the code and its guardrail checks in one change, with a mapping entry. | S5 | Adopted, corrected |
| Rayon, single writer, bounded channels | Keep, fix the bound | not attacked | Cap the channel at 2–4 waves of messages. | S5 or 0024 | Adopted |
| About 37 `GITGRAPH_*` toggles | Typed `IngestConfig` and profiles | Env bans need a scoping mechanism. | Config built once at the boundary. Scope the bans with a config crate or an `#[expect(reason)]` carve-out. | 0021 | Adopted, corrected |
| PowerShell and pnpm | Replace with `cargo xtask` | The prerequisites are Rust, a platform C toolchain and git ≥2.32. Port behavioral gates. Delete each script exactly once. | pnpm, Node, Docker lanes, the D: rule and the Windows helpers are deleted. A parity port. | 0020 | Decided |
| Grep guardrails | Replace with crates, lints and behavior tests | "Port only behavioral gates." | Name-existence checks are ported as a data table, each row tagged with the ADR whose behavior test replaces it. Dropping them before those tests exist would weaken guardrails (rule 6). | 0020, then each owner ADR | Decided |
| Anti-tamper classification of xtask | not covered | not covered | `xtask/src/guardrails/**` is policy, every other `xtask/**` path is implementation. Applied to both guardrail runners in S2 Phase 2. Without it, Phases 2 to 4 could not pass, because they touch only tooling, docs and scripts. | 0020 | Decided |
| Docker perf lane | Demote to optional | not attacked | Deleted. | 0020 | Decided |
| Test runner | libtest is enough; nextest optional | Two runners mean two test semantics. Make nextest the single runner. | Keep `cargo test` through S2 for parity. Decide on nextest in S4. | 0025 | Open |
| Fixtures | Seeded `git fast-import` | The same stream gives different OIDs on macOS and Linux (`core.ignorecase`). | Fully pinned git environment: `env_clear()` plus an allowlist, `GIT_CEILING_DIRECTORIES`, `core.ignorecase=false`, explicit object format, ref format and branch. Golden OIDs asserted on every OS. | 0021 | Adopted, corrected |
| Oracle | `rev-list --all --parents`, `log --raw` | `--all` adds notes commits, honors replace refs and includes refs the product skips. `log` walks HEAD only. | Match the product's ref policy. Add a metadata oracle and an in-process reference model for queries. | 0025 | Adopted, corrected |
| Perf gates | Digest-checked; gungraun per PR on Linux | Instruction counts miss I/O. Windows and macOS go ungated. | Gate deterministic counters (bytes, fsyncs, opens, renames, objects decoded, allocations) on all three OSes. | 0025 | Adopted, corrected |
| Kill criteria | "Pre-registered" | No numbers anywhere. | Each ADR writes numeric thresholds, a reference machine and a core count before measuring. | all ADRs | Adopted, corrected |
| Process model | Tauri in-process; daemon deferred | No Tauri dependency exists. In-process hosting gives no crash isolation. | Choose a split process, or in-process with a supervisor, `catch_unwind` at each command boundary and no synchronous commands. | 0022 or 0024 | Open |
| Novelty claim | "First" verification contract | Refuted by git's own `graph_git_two_modes` test, Sapling's `check_isomorphic_graph` and gitoxide's baselines. | Drop "first" and credit prior art. | 0025 | Adopted, corrected |
| `rust-toolchain.toml` targets line | Remove | not attacked | Removed in S0. | S0 | Decided |
| Duplicate ADR 0015 | Fix | not attacked | Fixed in S1. | 0019 | Decided |
