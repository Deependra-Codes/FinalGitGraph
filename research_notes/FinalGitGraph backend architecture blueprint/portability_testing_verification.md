# Portability, Test Hygiene, Correctness Verification and Perf-Regression Gating for FinalGitGraph (gitgraph-lab-core)

Research date: 2026-11-01. Local repo facts checked this session: `rust-toolchain.toml` pins `channel = "1.94.0"`, `targets = ["x86_64-pc-windows-msvc"]`. Every backend `package.json` script runs `powershell -ExecutionPolicy Bypass -File ./scripts/*.ps1`; `scripts/` holds 13 `.ps1` files. Current stable std docs show Rust 1.99.1 (2026-10-13) ([std docs](https://doc.rust-lang.org/std/fs/fn.canonicalize.html)). Pinned 1.94 is therefore about 5 releases behind. That is fine for a pin, but worth noting.

## 1. Cross-platform build/task automation and toolchain pinning

### Takeaway
The Rust-ecosystem default is a **`cargo xtask` binary plus a `.cargo/config.toml` alias**. It needs only `cargo` and `rustc`, so it runs the same way on Windows, macOS and Linux. Several major projects use it: rust-analyzer, cargo, zed, and ruff/uv under the name `cargo dev`. gitoxide is the notable exception: it uses `just`, which needs an `sh` on Windows. Pin the toolchain with `rust-toolchain.toml` and omit `targets`, because the host triple is always installed.

### Cited Findings
- xtask pattern: a workspace binary crate `xtask`, plus alias `xtask = "run --package xtask --"` in `.cargo/config.toml`. "It doesn't require any other binaries besides `cargo` and `rustc`, it fully bootstraps from them." It does not depend on a shell, so it is easier to make cross-platform than bash. Use the `CARGO` env var to call the same cargo. Keep the xtask compile time small. Limitation: it cannot hook a plain `cargo build`. — [matklad/cargo-xtask](https://github.com/matklad/cargo-xtask)
- Listed users: rust-analyzer (releases, perf metrics), helix, bootc, rust-lang/cargo. — [matklad/cargo-xtask](https://github.com/matklad/cargo-xtask)
- rust-analyzer `.cargo/config.toml`: `xtask = "run --package xtask --bin xtask --"`, plus `codegen` and `dist` aliases routed through xtask. — [rust-analyzer config](https://raw.githubusercontent.com/rust-lang/rust-analyzer/master/.cargo/config.toml)
- cargo itself uses several xtask packages: `build-man`, `stale-label`, `bump-check`, `lint-docs`, `spellcheck` = `run --package xtask-<name> --`. — [cargo config](https://raw.githubusercontent.com/rust-lang/cargo/master/.cargo/config.toml)
- zed: `xtask = "run --package xtask --"` (crate at `tooling/xtask`, uses clap, cargo_metadata, guppy). It also has `perf-test`/`perf-compare` aliases that inject a custom runner through `--config`. It sets `[env] MACOSX_DEPLOYMENT_TARGET` and per-OS rustflags in the same file. — [zed config](https://raw.githubusercontent.com/zed-industries/zed/main/.cargo/config.toml), [zed xtask Cargo.toml](https://raw.githubusercontent.com/zed-industries/zed/main/tooling/xtask/Cargo.toml)
- ruff: `dev = "run --package ruff_dev --bin ruff_dev"` and `benchmark = "bench -p ruff_benchmark ..."`. uv: `dev = "run --package uv-dev --features dev"`. Both statically link the CRT on Windows MSVC (`+crt-static`) to avoid a dynamic runtime dependency. — [ruff config](https://raw.githubusercontent.com/astral-sh/ruff/main/.cargo/config.toml), [uv config](https://raw.githubusercontent.com/astral-sh/uv/main/.cargo/config.toml)
- gitoxide uses a `justfile` (`#!/usr/bin/env -S just --justfile`) with recipes wrapping `cargo clippy`, nextest, and "journey tests". — [gitoxide justfile](https://raw.githubusercontent.com/GitoxideLabs/gitoxide/main/justfile)
- just README: it runs without extra dependencies, "(Although if your system doesn't have an `sh`, ..." you must choose another shell. `windows-shell` / `windows-powershell` settings are now deprecated in favor of a `[windows]` attribute on `set shell`. — [casey/just README](https://raw.githubusercontent.com/casey/just/master/README.md)
- Tauri's own repo `.cargo/config.toml` holds only an `[env]` workaround (`__TAURI_WORKSPACE__`) for a Windows `STATUS_ENTRYPOINT_NOT_FOUND` test error. tokio has no `.cargo/config.toml` (HTTP 404 this session). — [tauri config](https://raw.githubusercontent.com/tauri-apps/tauri/dev/.cargo/config.toml)
- Other options exist and are maintained repos: cargo-make (TOML task runner) [repo](https://github.com/sagiegurari/cargo-make), mise (tool versions + tasks) [repo](https://github.com/jdx/mise), devenv (Nix-based) [repo](https://github.com/cachix/devenv).
- `rust-toolchain.toml`: `profile` is `minimal|default|complete`. `components` are additive. For `targets`: "The host platform is automatically included; the targets listed here are additive." Precedence order is `+toolchain` > `RUSTUP_TOOLCHAIN` > `rustup override` > `rust-toolchain.toml` > default. The file needs rustup ≥1.23. — [rustup overrides](https://rust-lang.github.io/rustup/overrides.html)

### Inferences
- **Current lock-in diagnosis.** `targets = ["x86_64-pc-windows-msvc"]` does not break macOS or Linux by itself. Per rustup, the host is added automatically, so non-Windows hosts download an unused Windows std. The real lock-in is the `powershell` invocations, the hardcoded `D:\` paths, and Docker launched through PowerShell. Recommendation: drop the `targets` line, or keep it empty. Keep `channel` and `components = ["rustfmt","clippy"]`, plus `llvm-tools` if coverage is wanted.
- **Recommended replacement:** one `xtask` crate in the workspace with subcommands that mirror the current gates: `guardrails`, `gate --fast`, `gate`, `perf`, `fixtures`, `clean-artifacts`. Each `.ps1` maps to one Rust module. Have `package.json` call `cargo xtask <cmd>` so the pnpm entry points survive, or drop pnpm for backend-only work. Use `std::process::Command` with `env::var("CARGO")`, and do not build shell strings. This removes the PowerShell and Docker hard dependencies while keeping one entry point. It matches what rust-analyzer, cargo, zed, ruff and uv converged on.
- just/cargo-make/mise add a second tool and (just) an `sh` dependency on Windows. They are optional thin wrappers at most. Nix/devenv has no native Windows story, so it fails the "identical on Windows" constraint (inference; Nix-on-Windows status not verified this session). Bazel/Buck2 are disproportionate for a single Cargo workspace (inference).
- The Docker lane can stay as an *optional* xtask subcommand (`cargo xtask gate --in-container`). It must not be required.

### Gaps
- Exact GitHub Actions runner labels for windows-arm64, macOS-x86_64 (deprecation timeline) and linux-arm64 as of Nov 2026 were not verified.
- rustup auto-install behavior changes (1.28, `RUSTUP_AUTO_INSTALL`) are not covered by the overrides page and were not verified.

## 2. OS-specific runtime pitfalls for a Git-ingesting, SQLite-backed desktop backend

### Takeaway
The main pitfalls are:
- Windows path length and verbatim-path leakage from `canonicalize`
- case-insensitive filesystems on Windows and macOS, and Unicode decomposition on macOS
- Windows share-mode/delete semantics versus Unix advisory locks
- Defender/Smart App Control interfering with freshly compiled build-script binaries

App data must live in per-OS standard dirs (Tauri's `appLocalDataDir`/`appCacheDir`), never in the CWD or on `D:\`.

### Cited Findings
- Windows `MAX_PATH` = 260. The `\\?\` prefix allows about 32,767 chars, but disables `/` translation and `.`/`..` handling. Relative paths are always limited to MAX_PATH. Long-path opt-in since Windows 10 1607 needs both the registry `LongPathsEnabled=1` **and** an app manifest `longPathAware`. The registry value is cached per process. Windows does no Unicode normalization: names are opaque WCHAR sequences. — [MS: Maximum Path Length](https://learn.microsoft.com/en-us/windows/win32/fileio/maximum-file-path-limitation)
- Rust `fs::canonicalize` on Windows "converts the path to use extended length path syntax ... may be incompatible with other applications (if passed ... on the command-line, or written to a file ...)". — [std::fs::canonicalize](https://doc.rust-lang.org/std/fs/fn.canonicalize.html). The `dunce` crate exists to strip `\\?\` when safe. — [dunce](https://gitlab.com/kornelski/dunce)
- `remove_dir_all` does not follow symlinks. On Windows it uses `CreateFileW`, `GetFileInformationByHandleEx`, `SetFileInformationByHandle`, `NtCreateFile`, and it protects against symlink TOCTOU on most platforms. — [std::fs::remove_dir_all](https://doc.rust-lang.org/std/fs/fn.remove_dir_all.html)
- Windows sharing: without `FILE_SHARE_DELETE`, "no process can open the file ... if it requests delete access". Delete access covers both delete and rename. — [MS CreateFileW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew)
- SQLite: POSIX advisory locks are per-process across all fds. Closing any fd on the DB file drops the locks of other connections in the same process. Mixing lock styles across apps causes corruption. Unlinking or renaming a DB while it is in use is a corruption source. — [SQLite: How to corrupt](https://sqlite.org/howtocorrupt.html), [SQLite WAL](https://www.sqlite.org/wal.html)
- Git config `core.ignoreCase` exists for case-insensitive filesystems "like APFS, HFS+, FAT, NTFS". `core.precomposeUnicode` (macOS only) "reverts the unicode decomposition of filenames done by Mac OS", which is useful when sharing repos with Linux/Windows. — [git-config](https://git-scm.com/docs/git-config)
- Smart App Control (SAC): when its cloud service can't predict an app's safety, it checks the signature. Unsigned apps or invalid signatures are "untrusted and block[ed]". Turning SAC on later requires resetting or reinstalling Windows. — [MS: What is Smart App Control](https://support.microsoft.com/en-us/topic/what-is-smart-app-control-285ea03d-fa88-4d56-882e-6698afdb7003). A non-Rust tool (PlatformIO `pio.exe`) shows the same "[WinError 4551] An Application Control policy has blocked this file" under Windows 11 SAC. — [PlatformIO forum](https://community.platformio.org/t/winerror-4551-an-application-control-policy-has-blocked-this-file/53857)
- Dev Drive (ReFS, Windows 11 22621.2338+, minimum 50 GB) is intended for repos, package caches and build output. It runs Defender in "performance mode" (asynchronous scanning) when trusted. Microsoft explicitly gives `CARGO_HOME` relocation as an example. The C: drive cannot be a Dev Drive. Block cloning is available from 24H2. — [MS Dev Drive](https://learn.microsoft.com/en-us/windows/dev-drive/)
- nextest notes: process creation is slow on Windows, and can be slow on macOS "when anti-malware protections interfere". — [nextest: why process-per-test](https://nexte.st/docs/design/why-process-per-test/)
- tempfile: Windows and macOS use private per-user temp dirs. Unix shared `/tmp` has security issues. OSes periodically clean old temp files, so relying on paths there is unsafe. Temp *dirs* are created world-readable by default. — [tempfile docs v3.27.0](https://docs.rs/tempfile/latest/tempfile/)
- Tauri v2 path resolution (identifier from `tauri.conf.json`, overridable via `app > appDirectoriesOverride`):
  - `appLocalDataDir` = `$XDG_DATA_HOME|~/.local/share/<id>` (Linux), `~/Library/Application Support/<id>` (macOS), `{FOLDERID_LocalAppData}/<id>` (Windows)
  - `appCacheDir` = `$XDG_CACHE_HOME|~/.cache/<id>`, `~/Library/Caches/<id>`, `{FOLDERID_LocalAppData}/<id>`
  - `appDataDir` on Windows = RoamingAppData
  - `appLogDir` macOS = `~/Library/Logs/<id>`
  — [Tauri v2 path API](https://v2.tauri.app/reference/javascript/api/namespacepath/). XDG spec: [freedesktop basedir](https://specifications.freedesktop.org/basedir-spec/latest/). Non-Tauri equivalents: [directories-rs](https://github.com/dirs-dev/directories-rs) (`ProjectDirs`), [dirs-rs](https://github.com/dirs-dev/dirs-rs).
- uv and ruff link the CRT statically on Windows MSVC so binaries don't need the VC runtime. — [uv config](https://raw.githubusercontent.com/astral-sh/uv/main/.cargo/config.toml)

### Inferences
- **os error 4551 in cargo builds.** Most likely cause: SAC or App Control blocking newly compiled, unsigned `build-script-build.exe` / test executables, given that SAC blocks unsigned apps it can't classify. This is an inference; no Rust-specific primary source was found. Fixes are environmental: SAC off on dev boxes, or a managed App Control policy. Dev Drive is not shown to bypass SAC. Fewer build scripts (fewer `-sys` crates) reduces the exposure.
- **Default DB location.** Replace the CLI default `.gitgraph-lab/` in the CWD with `ProjectDirs::data_local_dir()` (CLI) or Tauri's `app_local_data_dir()`. Both match the table above on all three OSes. Put the derived index under the cache dir if it is rebuildable. Tests must always pass an explicit path.
- **Paths.** Store repo paths as bytes or `OsString`, and never round-trip through `String`. Avoid `canonicalize` for display or persistence, or wrap it with `dunce::simplified`. Compare repo-relative git paths byte-for-byte as git does, and do not apply FS case-folding. On macOS, honor `core.precomposeUnicode` when reading worktree paths. Add a tiny test fixture with an NFD filename and a `Makefile`/`makefile` collision, and run it on all three OS CI legs.
- **mmap and locks.** On Windows a mapped or open file without `FILE_SHARE_DELETE` cannot be renamed or deleted. Consequences: (a) drop gix pack mmaps and SQLite connections before temp-dir cleanup, otherwise `TempDir` drop fails on Windows; (b) never rename or replace the live SQLite DB. Use SQLite's own backup API or WAL checkpoints instead. Keep one connection-owner per process because of the POSIX-lock semantics above.
- Prefer `CARGO_TARGET_DIR` unset (the default `target/`) over `D:\cargo-target`. Devs who want the Dev Drive speed-up can set it in their *user* cargo config (`~/.cargo/config.toml`), not in the repo.

### Gaps
- Whether Rust std automatically adds `\\?\` for long absolute paths without a manifest was not verified this session. It is believed true for recent std but unconfirmed; it needs a check of the std source.
- Rust std's default Windows share mode for `File::open`/`OpenOptions` was not verified.
- Apple's APFS normalization wording could not be retrieved (the page body did not render). The claim "APFS is normalization-insensitive, normalization-preserving" stays unverified.
- Max open files defaults (macOS `ulimit -n` 256 in shells; Windows CRT 512/8192 stdio limits) were not verified.

## 3. Hermetic, leak-proof test and benchmark artifacts

### Takeaway
Destructor-based cleanup (`TempDir`) is necessary but **not sufficient**: it doesn't run on signals, SIGKILL or `static`s, and on Windows it fails on open handles. The robust pattern has five parts:
- every artifact rooted under `target/` (`CARGO_TARGET_TMPDIR` or a fixed `target/gitgraph-test/` root)
- deterministic names
- sweep-on-start of stale per-run dirs
- per-test process isolation (nextest)
- deterministic, seed- or script-hash-keyed fixture generation, as gitoxide does

Then `cargo clean` removes everything.

### Cited Findings
- `CARGO_TARGET_TMPDIR` is set only for integration tests and benches. It is "a path to a directory inside the target directory where integration tests or benchmarks are free to put any data". Cargo creates it "but doesn't manage its content in any way". `CARGO_BIN_EXE_<name>` gives the absolute path of a binary for integration tests. — [Cargo env vars](https://doc.rust-lang.org/cargo/reference/environment-variables.html)
- tempfile: "`TempDir` and `NamedTempFile` will fail if their destructors don't run". Destructors may not run on unhandled signals (e.g. `SIGINT`) or if the value is `static`. `env::override_temp_dir` exists for apps. — [tempfile](https://docs.rs/tempfile/latest/tempfile/). `TempDir::keep()` (replaces deprecated `into_path`) persists a dir. `TempDir::disable_cleanup(bool)` toggles cleanup. — [TempDir docs](https://docs.rs/tempfile/latest/tempfile/struct.TempDir.html)
- nextest runs each test in its own process, which isolates env vars, singletons and crashes. On timeout it sends SIGTERM, then SIGKILL after 10s (job objects on Windows). — [nextest design](https://nexte.st/docs/design/why-process-per-test/)
- nextest exports `NEXTEST_RUN_ID` (a UUID per run, also for setup scripts, since 0.9.138) and `NEXTEST_TEST_GLOBAL_SLOT` (unique while the test is live, reused afterwards). — [nextest env vars](https://nexte.st/docs/configuration/env-vars/)
- nextest setup scripts are still **experimental** (`experimental = ["setup-scripts"]`). They pass env to tests via `$NEXTEST_ENV` and have `leak-timeout`. A non-zero exit aborts the run. They show up in JUnit as `@setup-script:<name>`. — [nextest setup scripts](https://nexte.st/docs/configuration/setup-scripts/)
- Leaky-test detection only catches subprocesses that inherit stdout/stderr. Children with `Stdio::null()` go undetected. — [nextest leaky tests](https://nexte.st/docs/features/leaky-tests/)
- uv's nextest config in production: `experimental = ["setup-scripts"]`, `slow-timeout = { period = "10s", terminate-after = 12 }` as a deadlock stop-gap, per-OS CI profiles, JUnit output, and `test-groups` (`serial`, `io-bound` with `max-threads = 1`) assigned by filter. — [uv nextest.toml](https://raw.githubusercontent.com/astral-sh/uv/main/.config/nextest.toml)
- uv test harness `TestContext` gives each test isolated `temp_dir`, `cache_dir`, `home_dir`, `python_dir` (assert_fs `ChildPath`). It sets the CWD to the temp dir, so tests never touch the user's real HOME or cache. — [uv-test lib.rs](https://raw.githubusercontent.com/astral-sh/uv/main/crates/uv-test/src/lib.rs)
- ripgrep test util: dirs at `env::temp_dir()/ripgrep-tests/<name>/<atomic id>`. It calls `fs::remove_dir_all(&dir)` *before* creating, which bounds leaks to one copy per test name. — [ripgrep tests/util.rs](https://raw.githubusercontent.com/BurntSushi/ripgrep/master/tests/util.rs)
- gitoxide `gix-testtools` (v0.22.0):
  - Fixtures are produced by shell scripts (`scripted_fixture_read_only/_writable`) or Rust closures. Writable variants return a `tempfile::TempDir`. — [gix-testtools docs](https://docs.rs/gix-testtools/latest/gix_testtools/)
  - Script output is cached under `generated-do-not-edit/<archive>/<hash-kind>/<script_identity>-<family>`. `script_identity` is a CRC32 of the script bytes, its args and the post-process version, so editing the script invalidates the cache. Results are also auto-archived to `generated-archives/*.tar[.xz]`, optionally committed.
  - "CI normally sets `GIX_TEST_IGNORE_ARCHIVES` so fixture scripts are rerun and tracked archives are proven reproducible."
  - The git env is pinned: `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=<null device>`, `GIT_ATTR_NOSYSTEM=1`, `GIT_TERMINAL_PROMPT=false`, `GIT_AUTHOR_DATE=2000-01-01 00:00:00 +0000`, `GIT_COMMITTER_DATE=2000-01-02 ...`, fixed names and emails, `GIT_DEFAULT_HASH`. Signing and maintenance are disabled through `GIT_CONFIG_PARAMETERS`.
  — [gix-testtools lib.rs](https://raw.githubusercontent.com/GitoxideLabs/gitoxide/main/tests/tools/src/lib.rs)
- `git fast-import` builds repos from a text stream with explicit author/committer timestamps. That makes it a natural deterministic, fast generator for large synthetic histories. — [git-fast-import](https://git-scm.com/docs/git-fast-import)
- Rust 2024 edition makes `std::env::set_var`/`remove_var` `unsafe` because they "can be unsound ... in a multithreaded program". The guide advises a mechanism other than env vars for multithreaded programs. — [Edition guide: newly unsafe functions](https://doc.rust-lang.org/nightly/edition-guide/rust-2024/newly-unsafe-functions.html)
- cargo's own test suite uses a dedicated `cargo-test-support` crate for sandboxed project/home setup. — [cargo-test-support](https://github.com/rust-lang/cargo/tree/master/crates/cargo-test-support) (internals not inspected this session)

### Inferences
- **Recommended artifact contract** (all OSes):
  1. **One root.** All test, bench and perf artifacts go under `<target>/gitgraph-artifacts/`. That is `env!("CARGO_TARGET_TMPDIR")` in integration tests and benches. For unit tests and `perf-report`, xtask passes it explicitly, e.g. `--artifact-root`, or a root derived from `CARGO_TARGET_DIR`/`cargo metadata`. Never use `std::env::temp_dir()`, `D:\temp`, `tmp/` in the repo, or the CWD. `cargo clean` then removes everything.
  2. **Per-run dir** `run-<NEXTEST_RUN_ID or xtask-generated id>/`, with `TempDir::new_in(run_dir)` per test.
  3. **Sweep-on-start.** xtask (or a nextest setup script once it is stable) deletes sibling `run-*` dirs not owned by a live process, or older than N hours, before each run. This catches SIGKILL, timeout and power-loss leaks that destructors miss. It is the ripgrep pattern made robust.
  4. **Debug opt-in.** `GITGRAPH_KEEP_ARTIFACTS=1` calls `TempDir::disable_cleanup(true)` and prints the path. Off by default.
  5. **Windows delete ordering.** Close SQLite connections and drop gix repo handles (mmaps) before the `TempDir` drops. Declare the guard first so it drops last. Fields drop in declaration order, locals in reverse; struct order needs care. Add a test that asserts the run dir is empty after the suite.
  6. **CI leak check.** After `cargo xtask gate`, assert that no files exist outside `target/` (e.g. `git status --porcelain --ignored` is clean apart from `target/`) and no new entries exist in the OS temp dir with the project prefix. This turns "artifacts stored anywhere" into a failing gate.
- **Fixtures.** Use two tiers. (a) Small hand-written scripted fixtures, with gix-testtools-style hash-keyed caching under `target/`, pinned `GIT_*` env, and an optional committed tarball if git availability on CI is a concern. (b) Generated large histories from a seeded generator emitting a `git fast-import` stream with fixed dates. Key the output dir by `(generator_version, seed, size)` so a regenerated fixture is byte-identical and reusable across runs. Recording the seed in perf reports makes perf fixtures reproducible.
- **Env toggles.** Replace about 40 env vars read ad hoc with one typed `IngestConfig` built *once* at the boundary (CLI/Tauri), then passed down. Tests construct configs directly: no global mutex, no `unsafe set_var` under Rust 2024. Ban `std::env::var` outside the config module with clippy `disallowed-methods` (section 6).
- Docker named volumes: if the optional container lane survives, use bind mounts into `target/` or `--rm` anonymous volumes so `cargo clean` covers them. That is an inference based on Docker semantics, not verified.

### Gaps
- Whether `gix-odb` offers a supported in-memory object store for writing test repos without disk was not verified. Disk-backed fast-import under `target/` is the safe recommendation.
- Internals of `cargo-test-support` (e.g. how it roots test dirs under `target/tmp`) were not inspected.
- The stabilization timeline for nextest setup scripts is unknown (still experimental as of the docs fetched).

## 4. Layered correctness verification with oracles (and what catches "edges dropped across batch boundary")

### Takeaway
The bug class "edges lost when ingestion is split into batches" is caught most directly by two things: (1) a **differential oracle** against real git (`git rev-list --parents`) on generated histories, and (2) a **metamorphic invariant**: output is identical for every batch size, thread count and config toggle. Both are driven by **property-based generation** of random DAGs with shrinking. **Mutation testing** then proves the assertions are not tautological, i.e. not comparing stored metadata to itself. Fuzzing, DST, loom and Miri are secondary for this workload.

### Cited Findings
- `git rev-list` enumerates commits and can print parents (`--parents`). It is the canonical commit-graph oracle. — [git-rev-list](https://git-scm.com/docs/git-rev-list)
- Property-based testing with shrinking: proptest ([repo](https://github.com/proptest-rs/proptest)), quickcheck ([repo](https://github.com/BurntSushi/quickcheck)). Unified fuzz/property harness across engines: bolero ([repo](https://github.com/camshaft/bolero)). Coverage-guided fuzzing: cargo-fuzz/libFuzzer ([repo](https://github.com/rust-fuzz/cargo-fuzz)).
- DST:
  - TigerBeetle's VOPR is deterministic from a seed and the git commit, so any bug reproduces exactly. All non-deterministic parts (clock, network, disk) are stubbed. It can "speed up time arbitrarily". — [TigerBeetle VOPR doc](https://raw.githubusercontent.com/tigerbeetle/tigerbeetle/main/docs/internals/vopr.md)
  - turmoil: "deterministic simulation testing of distributed systems", single-threaded hosts, seeded injection of latency, drops, partitions, crashes and torn writes into the simulated network *and filesystem*. — [turmoil README](https://raw.githubusercontent.com/tokio-rs/turmoil/main/README.md)
  - madsim: a tokio-like async runtime with deterministic simulation, after FoundationDB. — [madsim README](https://raw.githubusercontent.com/madsim-rs/madsim/main/README.md)
- Concurrency/UB checkers: loom (permutation testing of concurrent code) [repo](https://github.com/tokio-rs/loom), Miri (UB interpreter) [repo](https://github.com/rust-lang/miri).
- Mutation testing: cargo-mutants "helps you improve your program's quality by finding places where bugs can be inserted without causing any tests to fail". — [mutants.rs](https://mutants.rs/). `--in-diff DIFF_FILE` tests only mutants overlapping changed hunks (CI/PR use). — [mutants.rs in-diff](https://mutants.rs/in-diff.html). `--test-tool=nextest` or `test_tool = "nextest"` in `.cargo/mutants.toml`. — [mutants.rs nextest](https://mutants.rs/nextest.html)
- Snapshot testing: insta. — [repo](https://github.com/mitsuhiko/insta)
- gix-testtools normalizes hashes in snapshots so one snapshot serves SHA-1 and SHA-256 fixtures. — [gix-testtools docs](https://docs.rs/gix-testtools/latest/gix_testtools/)

### Inferences
- **Recommended layer stack, ordered by bug-finding value per effort for this codebase:**
  1. **Oracle-differential integration tests.** Generate a repo (fast-import, seeded) and ingest it. Then compare: (a) the commit set and the *ordered* parent lists against `git rev-list --all --parents`; (b) per-commit changed paths and renames against `git log --name-status -M --format=%H` (or `git diff-tree -r -M --root`); (c) sampled `merge-base` and ancestry queries against `git merge-base`/`git merge-base --is-ancestor`. Compare canonicalized sets, not counts. Report a minimal diff of missing and extra edges. Git becomes a test-only dependency, already required by gix-testtools-style fixtures.
  2. **Property-based DAG generation.** A proptest strategy builds random histories: linear runs, octopus merges, criss-cross merges, multiple roots, orphan branches, renames, binary files, empty commits, and very long chains. **Batch size and thread count are also generated parameters.** Shrinking reduces a failure to a minimal DAG plus batch size. For the suspected bug, that is typically "a merge commit whose parent lands in the previous batch". Persist failing seeds (proptest regression files) so they become permanent tests.
  3. **Metamorphic relations, no git needed, cheap:**
     - `ingest(batch=k)` == `ingest(batch=1)` for all k
     - full == incremental (ingest a prefix, then the rest) == re-ingest (idempotence)
     - parallel == serial
     - every config/toggle combination == the default output (for toggles that claim to be perf-only)
     - `edge_count == Σ parent_count`
     - every parent of an ingested commit is ingested or explicitly marked shallow/missing
     - topological order is consistent

     These directly target the batch-boundary class and the untested 40-toggle space. Any toggle that is not semantically neutral must be named and tested separately. Generating toggle sets with proptest gives more coverage than pairwise tables for little extra code.
  4. **Mutation testing** (`cargo mutants --in-diff` on PRs, full run nightly) over `adapters/git.rs`, `adapters/db.rs`, `application/indexer.rs`. A surviving mutant in a batching or boundary function is the measurable signal that assertions are weak. It exposes tests that compare stored metadata to itself.
  5. **Snapshot tests (insta)** only for small hand-crafted fixtures and human-readable outputs (CLI, JSON DTOs). They are not an oracle: a wrong first snapshot gets blessed, so snapshots should be cross-checked once against git.
  6. **Fuzzing (cargo-fuzz/bolero)** for parsers and decoders the project owns: diff/patch parsing, config parsing, any custom binary format, DB row decoding. Low value for the edge-loss bug.
  7. **DST.** Only worthwhile if the ingest pipeline grows async stages, backpressure or crash-recovery semantics. A lightweight version is valuable now: a seeded fault-injecting `Store` trait implementation that fails or aborts the transaction at batch N. It then verifies that a resumed ingest yields oracle-equal output. turmoil and madsim target networked async systems and are probably overkill.
  8. **loom** only for custom synchronization primitives. **Miri** only for `unsafe` code; with `forbid(unsafe_code)` it adds little, and IO and git-heavy tests are unsuitable for it.
- Which catch "edges dropped across batch boundary":
  - layers 1 and 3: yes, directly
  - layer 2: yes, with minimization
  - layer 4: detects the absence of such a check
  - layer 5: only if the fixture spans a boundary and the snapshot was correct
  - layers 6-8: generally no
- The perf harness must run the same oracle or metamorphic check on its outputs (section 5). Otherwise the 100k-commit path, where batch boundaries actually occur, stays unverified while the 3-commit tests never cross a boundary.

### Gaps
- Primary literature not fetched this session (recommended for the report writer to cite if retrieved): McKeeman, "Differential Testing for Software" (1998); Chen et al., metamorphic testing survey (ACM CSUR 2018); Hypothesis/QuickCheck papers; FoundationDB "Testing Distributed Systems w/ Deterministic Simulation" talk; empirical mutation-testing studies (e.g. Google's mutation testing at scale, ICSE 2018/2021).
- I found no source quantifying how often batch-boundary bugs are caught by each technique; the ranking above is reasoned, not measured.

## 5. Performance-regression gating that cannot be gamed

### Takeaway
A perf gate is trustworthy only if three things hold. (1) **Every benchmark validates its output** (digest or oracle check) before its timing counts. (2) Gating uses a **low-noise metric**: instruction counts via Valgrind-based gungraun on Linux CI, or wall time only on dedicated stable hardware. (3) Thresholds are **statistical against a history** (Bencher t-test/IQR with minimum sample sizes, or CodSpeed), with explicit failure on alert.

### Cited Findings
- iai-callgrind was renamed **Gungraun** (Dec 2025 rename noted in a rust-lang/rust PR). It is forked from bheisler/iai. The docs.rs version is 0.19.4. — [rust-lang/rust PR #150080](https://github.com/rust-lang/rust/pull/150080), [gungraun docs.rs](https://docs.rs/crate/gungraun/), [Gungraun vs Iai](https://gungraun.github.io/gungraun/latest/html/comparison/iai.html)
- Gungraun uses Valgrind tools (Callgrind, Cachegrind, DHAT) and Linux perf. It measures instruction counts with high precision, consistently "even in" noisy environments, running each benchmark once. — [gungraun README](https://raw.githubusercontent.com/gungraun/gungraun/main/README.md)
- Valgrind platforms: AMD64/Linux is primary, ARM64/Linux is supported. Darwin is supported only up to 10.13 (x86) / 11.0 (AMD64). "Windows is not under consideration." — [Valgrind platforms](https://valgrind.org/info/platforms.html)
- CodSpeed: instrumentation (simulation) mode runs under Valgrind and is deterministic. Walltime mode uses forked `codspeed-criterion-compat-walltime` / `codspeed-divan-compat-walltime` for multiple rounds. Its docs warn walltime "on traditional VMs/Hosted Runners will lead to inconsistent data" and recommend their macro runners. — [codspeed-rust releases](https://gh.r-mj.com/CodSpeedHQ/codspeed-rust/releases), [codspeed crate source](https://docs.rs/crate/codspeed/4.5.0/source/src/walltime_results.rs) (secondary aggregation of CodSpeed docs via search; treat details as moderately reliable)
- Bencher thresholds:
  - Tests available: Percentage, z-score (≥30 samples), Student's t-test, Log-Normal, IQR, Delta-IQR, Static.
  - Options: `--threshold-min-sample-size`, `--threshold-max-sample-size`, `--threshold-window`.
  - Bencher doesn't fail CI by default. `--error-on-alert` makes it fail.
  — [Bencher thresholds](https://bencher.dev/docs/explanation/thresholds/), [bencher repo](https://github.com/bencherdev/bencher)
- Statistical walltime harnesses: criterion ([repo](https://github.com/bheisler/criterion.rs)), divan ([repo](https://github.com/nvzqz/divan)).
- zed exposes `cargo perf-test` / `cargo perf-compare` aliases that swap in a custom `perf` runner crate, a perf-comparison workflow built as cargo aliases. — [zed config](https://raw.githubusercontent.com/zed-industries/zed/main/.cargo/config.toml)

### Inferences
- **Anti-gaming rules for `perf-report` and the perf gates:**
  1. Each perf scenario computes an **output digest**: sorted edge list hash, commit count, path-change count, query-result hash. It compares the digest against the oracle-derived expected value for that seeded fixture, using the same generator as section 4. A mismatch is a hard failure *before* any timing is reported. This closes the hole where the harness discards query results.
  2. Results must be consumed (`std::hint::black_box`) and fixtures pre-generated outside the timed region. Report setup and timed regions separately.
  3. Gate on two metrics: **instruction count** (gungraun, Linux x86_64 and arm64 CI, deterministic, small thresholds ~1-2%) and **walltime** (criterion/divan or the existing harness) on a fixed self-hosted or bare-metal box. Walltime also gets a Bencher/CodSpeed statistical threshold (t-test or IQR, min sample size ≥5-10, rolling window). Windows/macOS walltime is tracked as informational unless dedicated hardware exists, since Valgrind does not run there.
  4. Benchmarks are named, versioned and seeded (fixture seed + generator version in the metric metadata), so a changed fixture cannot silently reset a baseline.
  5. Keep the existing 100k-commit "hardware ingestion fast path" as a scheduled large-scale walltime run, with its digest check. Use instruction-count micro and meso benches (e.g. 5k-20k commits) for per-PR gating.
  6. For same-machine noise, run baseline and candidate interleaved in one job (A/B/A/B) and compare distributions. Do not compare against a stored number from another day. Exclude CPU frequency scaling and thermal effects by pinning governor/affinity on Linux bare-metal runners.
- Telemetry contract: `perf-report.rs` (high-risk path) should emit JSON (scenario id, seed, generator version, digest, digest_ok, metrics). Then xtask/Bencher consume it without scraping `tmp/perf-report-*.txt`. Write it under `target/gitgraph-artifacts/perf/` (section 3).

### Gaps
- Bencher's support for Gungraun output (benchmark adapters page) was not fetched. Bencher's May 2026 post references "Gungraun (formerly Iai-Callgrind)" per search snippet only.
- Current CodSpeed pricing and support for macOS/Windows instrumentation was not verified.
- No primary source was fetched on interleaved A/B benchmarking methodology; it is presented as inference.

## 6. Replacing grep-based architecture guardrails with real enforcement

### Takeaway
Encode the layering in the **Cargo dependency graph** (split `domain`, `adapters`, `application`, `support` into crates, so an illegal import fails to compile). Encode banned APIs in **clippy `disallowed-methods`/`disallowed-types`**. Encode `unsafe`, `dbg!` and `todo!` policy in **workspace `[lints]`**. Encode dependency policy in **cargo-deny**. Keep only a small xtask check for the few rules the compiler can't express.

### Cited Findings
- Cargo `[lints]` table: e.g. `[lints.rust] unsafe_code = "forbid"`, with levels forbid/deny/warn/allow and a priority. — [Cargo manifest: lints](https://doc.rust-lang.org/cargo/reference/manifest.html)
- clippy `disallowed-methods` config entries: `path` (required, fully qualified), `reason`, `replacement`, `allow-invalid` (ignore missing paths). The same mechanism covers `disallowed-types` and `disallowed-macros`. — [Clippy lint configuration](https://doc.rust-lang.org/clippy/lint_configuration.html). Lint index (including `dbg_macro`, `todo`, `unimplemented`): [Clippy lints](https://rust-lang.github.io/rust-clippy/master/index.html)
- cargo-deny (licenses, bans, advisories, sources) [repo](https://github.com/EmbarkStudios/cargo-deny). cargo-semver-checks (public API semver lint) [repo](https://github.com/obi1kenobi/cargo-semver-checks). dylint (custom lints as dynamic libraries) [repo](https://github.com/trailofbits/dylint). cargo-modules (module structure/dependency analysis) [repo](https://github.com/regexident/cargo-modules).
- zed's xtask depends on `guppy` and `cargo_metadata`, i.e. it does workspace-graph analysis in Rust rather than in shell. — [zed xtask Cargo.toml](https://raw.githubusercontent.com/zed-industries/zed/main/tooling/xtask/Cargo.toml)
- zed and gitoxide put `[lints] workspace = true` in member crates, with lint policy centralized at the workspace root. — [zed xtask Cargo.toml](https://raw.githubusercontent.com/zed-industries/zed/main/tooling/xtask/Cargo.toml); gitoxide uses `#[expect(clippy::too_many_arguments)]`, i.e. `expect` instead of `allow` for justified exceptions, in [gix-testtools lib.rs](https://raw.githubusercontent.com/GitoxideLabs/gitoxide/main/tests/tools/src/lib.rs)

### Inferences
- **Recommended mapping of the current 1,000-line guardrail script:**
  - *Layering* becomes crates: `gitgraph-domain` (no deps on gix, rusqlite or IO), `gitgraph-support`, `gitgraph-adapters` (gix, rusqlite; depends on domain and support), `gitgraph-application` (depends on all three), and bins and Tauri depending on application. An adapter import in domain becomes a compile error, so no grep is needed. The repo's "no flat modules in src root" rule becomes moot, because crates own their roots.
  - *Banned APIs* become `clippy.toml` `disallowed-methods`. Candidates: `std::env::var`/`var_os` (outside the config crate), `std::env::set_var`, `std::env::temp_dir`, `std::env::current_dir` (forces explicit paths), `std::process::exit` (outside main), `std::fs::canonicalize` (use `dunce`), `std::thread::sleep` in non-test code. Each gets a `reason` and `replacement`. Banned types such as `std::collections::HashMap` in deterministic-output paths, if determinism matters, go in `disallowed-types`.
  - *Quality markers* become workspace `[lints]`: `rust.unsafe_code = "forbid"`, `clippy.dbg_macro = "deny"`, `clippy.todo = "deny"`, `clippy.unimplemented = "deny"`, `clippy.allow_attributes = "deny"` and `clippy.allow_attributes_without_reason = "deny"` (forces `#[expect(..., reason = "...")]`). Plus `-D warnings` in the gate. The TODO/FIXME *comment* ban is a ~20-line xtask check, or a comment-scanning step; clippy does not lint comments (inference).
  - *Dependency policy* goes to `deny.toml`: licenses, advisories, duplicate versions, git-source bans. cargo-deny `bans` can also restrict where a crate may appear (believed `wrappers` field; verify).
  - *"Function X must exist"* checks are replaced by behavior tests (sections 4-5). A grep for a function name proves nothing about behavior and blocks refactors.
  - cargo-semver-checks has low value for an app-internal workspace, so skip it unless crates are published. dylint is the escape hatch for a truly custom rule, such as "no `rusqlite::Connection` held across `await`", but it needs a nightly toolchain (inference; verify).
- Net effect: the guardrail xtask shrinks to orchestration (`cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo deny check`, `cargo nextest run`, the leak check, and the comment-marker scan). That is identical on all OSes. ADR required per AGENTS.md since this changes architecture boundaries.

### Gaps
- cargo-deny `wrappers` semantics and current cargo-deny version were not verified this session.
- Whether `clippy.toml` `disallowed-methods` can be scoped per crate (it is read per crate dir, falling back to the workspace root) was not verified. Per-crate `clippy.toml` files would be needed to allow `env::var` only in the config crate. Alternatively, use `#[expect(clippy::disallowed_methods, reason=...)]` at the single allowed call site.
- dylint toolchain requirements and maintenance status as of Nov 2026 were not verified.
