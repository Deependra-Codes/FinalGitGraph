# Red-team review: portability, automation, test hygiene, verification oracle and perf-gate claims (FinalGitGraph blueprint)

Scope: `reports/FinalGitGraph backend architecture blueprint.md` sections "One command: cargo xtask replaces pnpm, PowerShell and Docker", "The artifact contract", "A layered oracle that would have caught the edge loss", "Perf gates that validate output before trusting a timer", and Phase 0/1/2 of "The phased plan". Also `research_notes/FinalGitGraph backend architecture blueprint/portability_testing_verification.md`, which this note calls the "portability note". Date: 2026-11-01.

Evidence types used:
- **Local experiment.** Run this session on this Mac (Darwin 24.6, `git version 2.39.5 (Apple Git-154)`), in the session scratchpad. No repo files were changed.
- **Local repo fact.** Read from the FinalGitGraph checkout.
- **Local man page.** Apple Git 2.39.5 man pages under `/Library/Developer/CommandLineTools/usr/share/man/`. The same text is on git-scm.com.
- **Web source.** Fetched this session and linked inline.

Anything else is marked as inference.

## Ranked attack list (summary across all key questions)

### Takeaway
The blueprint's portability story rests on four load-bearing claims. All four break under concrete scenarios:
- "Fixtures built with `git fast-import` are filesystem-independent." Broken by a local experiment.
- "The pinned git env isolates fixtures." It is incomplete compared with the gitoxide source it cites.
- "Only cargo + git." Not true: a C toolchain is needed, and git ≥2.32 must be pinned against Git 3.0 defaults.
- "The leak gate detects leaks." It has many false positives and blind spots.

The oracle also has real blind spots: metadata, query semantics, ref-set and replace-ref semantics, and non-first-parent diffs. Per-PR perf gating leaves Windows and macOS, and all I/O-bound regressions, ungated. None of these sink the overall direction (xtask plus a git oracle plus metamorphic digests). Each needs a specific revision.

### Cited Findings
Ranked by severity: critical first, then major, then minor. Details and sources are in the per-question sections below.

| # | Claim attacked (location) | Attack in one line | Severity |
|---|---|---|---|
| 1 | "Seeded fixtures come from `git fast-import`" + "Windows must equal macOS must equal Linux" (blueprint, oracle section) | `git init` on macOS writes `core.ignorecase=true` into repo-local config, and fast-import honors it. `Makefile` and `makefile` collapse into one tree entry, so the same stream gives a different commit OID on macOS than on Linux (local experiment) | critical |
| 2 | "fixture git commands run with a pinned environment (`GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL` null, fixed dates)" (artifact contract, 4th part) | The cited gix-testtools goes much further. It removes every inherited `GIT_*` variable, sets `XDG_CONFIG_HOME`, idents, `GIT_DEFAULT_HASH`, gpgsign off, `init.defaultBranch`, `protocol.file.allow`, `gc.auto=0` and `maintenance.auto=false`. Without these, hooks leak `GIT_DIR`, inherited `GIT_DEFAULT_HASH` changes OIDs, commit creation fails with no ident, and background gc runs | critical |
| 3 | "The core path depends on cargo and git, nothing else" (one-command section) | A platform C linker is required: MSVC/VS (licensed) on Windows, Xcode CLT on macOS, `cc` on Linux. `lmdb-sys` and `libmimalloc-sys` depend on `cc` until Phase 5 | critical (claim is false as stated) |
| 4 | "xtask checks its version" (no floor given) + fixed dates as the determinism guarantee | `GIT_CONFIG_GLOBAL` exists only since git 2.32, and older git ignores it silently. Git 3.0 (announced, undated) flips the defaults to SHA-256, reftable and `main`. Without explicit `--object-format`/ref-format, fixture OIDs and ref storage depend on the host's git | critical |
| 5 | Leak gate: "fails if `git status --porcelain --ignored` shows anything outside `target/`" | False positives on every dev checkout: `.idea/`, `.vscode/`, `*.log`, `tmp/` and others are already in this repo's `.gitignore`. Default writes by proptest, insta and cargo-mutants also land outside `target/` | major |
| 6 | "Nothing can land outside `target/`, and the leak gate makes any violation a failing build" | Blind spots: writes inside the project's `.git/`; repo discovery escaping upward from fixtures under `target/`; HOME and app-data writes (Windows known folders ignore env); temp leaks without the project prefix (tempfile's default prefix is `.tmp`) | major |
| 7 | "xtask sweeps stale `run-*` directories … not owned by a live process, or older than N hours" (portability note §3; blueprint) | Races with concurrent runs and with a shared `CARGO_TARGET_DIR` (which the portability note itself recommends). The OR condition deletes long live runs. PID liveness is racy | major |
| 8 | "`CARGO_TARGET_TMPDIR` … and an explicit `--artifact-root` from xtask everywhere else" | `CARGO_TARGET_TMPDIR` is compile-time and only for integration tests and benches. libtest has no channel for `--artifact-root`. Plain `cargo test` (IDE runners) gets nothing | major |
| 9 | "A layered oracle that would have caught the edge loss" | Not caught: commit metadata, tree OIDs, query semantics (topological-order validity, BFS depth, truncation, pagination, path minimality), non-first-parent diffs, and ref-set and replace-ref semantics (local experiment). The `git log` command as written walks HEAD only | major |
| 10 | proptest DAGs + "failing seeds persist as permanent regression files" | The seed is random per run by default. Persisted seeds do not survive strategy edits. Shrinking takes up to about 1,024 iterations, each spawning several git processes. Cross-OS comparison over random cases is meaningless | major |
| 11 | "Windows must equal macOS must equal Linux … one content digest" | No fan-in or golden mechanism is specified. The digest algorithm is unspecified (std `DefaultHasher` is not stable across releases). The repo has no `.gitattributes`, so Windows autocrlf can rewrite committed streams and goldens | major |
| 12 | Pinning env "for fixtures" (implicitly isolating the system under test) | The product reads the developer's git config and `GIT_*` env through gix. Clippy bans do not apply inside dependencies (`--cap-lints allow`) | major |
| 13 | "Per-PR gating uses instruction counts from gungraun … Linux only" | Windows and macOS regressions are ungated. Instruction counts miss syscalls, fsync, I/O and contention, which dominate the proposed layer-file design. Valgrind is a system package, not a `cargo install` | major |
| 14 | "Extra tools … installed `--locked` at pinned versions under `target/tools`" | `cargo install` builds in the OS temp dir by default. `cargo clean` deletes the tools, and on Windows it cannot delete running executables. Valgrind and perf cannot be installed this way | major |
| 15 | cargo-mutants `--in-diff` as a gate | By default it copies the tree to a temp dir and writes `mutants.out` in the source dir, both violating the artifact contract. VCS dirs are not copied. The cost per mutant times the oracle and proptest suite is high | major |
| 16 | "Per-process test isolation from nextest is no longer needed for correctness" | Two runners give two semantics. libtest has no per-test timeout. Process-global caches and Windows handle lifetimes differ | major |
| 17 | clippy `disallowed-methods` "bans `std::env::var` outside the config module" | Per-module scoping is not configurable, only by attributes, and repo policy bans lint-bypass attributes. `Path::canonicalize` and the `var_os`/`vars` family are separate items. Dependencies are not linted | major |
| 18 | Windows long paths (not addressed in the blueprint) | Git for Windows long paths are opt-in and off by default. `GIT_CONFIG_NOSYSTEM=1` removes a user's `--system core.longpaths`. `run-<uuid>` plus SHA-256 object paths approach 260 characters | major |
| 19 | Fixture matrix: rebase, `gc --prune=now`, shallow, `--filter=blob:none`, symlinks, case-only renames | Several items need a worktree or a transport, which reintroduces filesystem differences (case collisions, Windows symlink privileges) | major |
| 20 | "The git CLI is already present wherever a clone exists" | False for ZIP downloads, GUI clients with embedded git, and source tarballs. On macOS, `/usr/bin/git` is a CLT shim that prompts for installation | minor |
| 21 | Bencher wall-time lane "on fixed hardware" | No such hardware exists in the plan. Private-repo hosted runners have 2 vCPU VMs | minor |
| 22 | "removes … the Rust 2024 `unsafe set_var` blocker" | The workspace is `edition = "2021"`, so `set_var` is not `unsafe` today. The claim only applies after an edition bump | minor |

### Inferences
- Attacks 1, 2, 4 and 11 compound. Unless fixture creation is fully pinned (config, env, hash, ref format, branch) and verified by committed golden OIDs, the headline claim fails. That claim is "digest compared across Windows, macOS, Linux in CI" (also the proposed ADR novelty wording). It can fail in two ways: a spurious red build, or a green build that compares two different repositories.
- The cheapest high-value fix for the whole cluster is to commit golden fixture identities. Assert each fixture's HEAD OID and full ref list before any oracle runs, on every OS. A divergent fixture then fails loudly instead of silently testing something else.

### Gaps
- No Windows or Linux machine was available this session. Attack 1 was observed on macOS only. The Windows half rests on the documented `git init` case-probe and is an inference.

## Does "fresh clone + Rust toolchain + ONE command, only cargo + git" hold on Windows, macOS and Linux?

### Takeaway
No, as written. Windows needs licensed Visual Studio C++ tooling, macOS needs Xcode Command Line Tools, and Linux needs a C compiler and linker. During Phases 1–4 the repo also compiles C code (`lmdb-sys`). Git needs a pinned minimum version (≥2.32 for `GIT_CONFIG_GLOBAL`). Fixture creation must also be pinned against Git 3.0's announced default flips. The "extra tools under `target/tools`" plan writes outside `target/` by default and cannot cover Valgrind.

### Cited Findings
- **C linker on Windows.** rustup: "To compile programs into an exe file, Rust requires a linker, libraries and Windows API import libraries", and these come from Visual Studio. rustup-init "will offer to automatically install the prerequisites" via Visual Studio Community. Community is free for individuals, academic and open-source use, but "not for other uses". Build Tools alone "requires you already have a license". — [rustup: Windows MSVC](https://rust-lang.github.io/rustup/installation/windows-msvc.html)
- **C compiler today.** Local repo fact: `Cargo.lock` lists `cc` as a dependency of `lmdb-sys` and `libmimalloc-sys`. `lmdb-sys` is a non-optional dependency in `apps/desktop/src-tauri/Cargo.toml`. A C compiler (MSVC `cl.exe`, clang or gcc) is therefore required on every OS until LMDB is removed in Phase 5.
- **macOS.** Apple: the Command Line Tools package is installed with `xcode-select --install`, and running a tool such as `git` without it prompts you to install it. An Apple forum thread reports `/usr/bin/git` failing after a macOS 26.6 upgrade until CLT 26 was reinstalled manually. — [Apple: installing the command line tools](https://developer.apple.com/documentation/xcode/installing-the-command-line-tools.md), [Apple discussions](https://discussions.apple.com/thread/256335209)
- **Git version on this Mac.** Local experiment: `git version 2.39.5 (Apple Git-154)` on Darwin 24.6 (macOS 15). The exact git version in Xcode 26 CLT could not be found.
- **git 2.32.** The release notes "introduce GIT_CONFIG_GLOBAL to override the per-user configuration in $HOME/.gitconfig" and replace NOSYSTEM with `GIT_CONFIG_SYSTEM`. The same notes list "git log learned --diff-merges=<style>". — [Git 2.32.0 release notes](https://raw.githubusercontent.com/git/git/master/Documentation/RelNotes/2.32.0.adoc)
- **Git 3.0.** "There is no planned release date for this breaking version yet." Announced changes:
  - "The default hash function for new repositories will be changed from "sha1" to "sha256"."
  - "The default storage format for references in newly created repositories will be changed from "files" to "reftable"."
  - "In new repositories, the default branch name will be `main`."
  - `safe.bareRepository` changes from `all` to `explicit`.
  - Grafts will be removed.
  The page was last updated in 2.55.0, and its newest changelog entry is 2.56.0. — [git BreakingChanges](https://git-scm.com/docs/BreakingChanges)
- **`cargo install` build dir.** `--target-dir` "Defaults to a new temporary folder located in the temporary directory of the platform." `--locked` "Asserts that the exact same dependencies and versions are used as when the existing `Cargo.lock` file was originally generated". — [cargo install](https://doc.rust-lang.org/cargo/commands/cargo-install.html)
- **Valgrind.** gungraun: "Just make sure the `valgrind` binary is in your `$PATH`". Installation is via a package manager or a source build. It was tested only with Valgrind ≥3.20.0, and Cachegrind needs ≥3.22. Linux perf ≥5.9 is needed for perf mode. Benchmarks require debug symbols. — [gungraun prerequisites](https://gungraun.github.io/gungraun/latest/html/installation/prerequisites.html)
- **GUI clients with embedded git.** GitHub Desktop's library dugite fetches an embedded Git distribution. Support threads report that this git is not on the shell PATH. — [dugite issue #165](https://github.com/desktop/dugite/issues/165), [desktop issue #18739](https://github.com/desktop/desktop/issues/18739). Weakly sourced: no official doc was found.
- **Current CI.** Local repo fact: `.github/workflows/rust-ci.yml` runs `ubuntu-latest` and `windows-latest` only, with `shell: pwsh`. There is no macOS leg.

### Inferences
- **Revision for attack 3.** Restate the prerequisite as "Rust toolchain + platform C toolchain + git ≥ N". Add `cargo xtask doctor`, which checks:
  - linker/`cc` presence
  - git version
  - `.git` presence (needed for the leak gate)
  - path length on Windows
  The Windows licence caveat belongs in CONTRIBUTING. "Only cargo" becomes true only after Phase 5 and only if no `-sys` crate remains. Even then the MSVC linker is still needed on Windows.
- **Revision for attack 4.** Set a minimum git version of ≥2.32 because of `GIT_CONFIG_GLOBAL`. Apple Git 2.39.5 passes, so the macOS floor is fine. Always create fixtures with explicit `--object-format=sha1` (and a sha256 variant where intended), an explicit initial branch, and files ref storage. Where that is not expressible on old git, fail with a clear message rather than drift. Add two CI legs: oldest supported git (Apple 2.39.x on macOS) and newest git (a release candidate once 3.0 exists). (Inference: the `--ref-format` init flag arrived with reftable support around 2.45. Not verified this session.)
- **Revision for attack 14.**
  - `cargo install --locked --root target/tools --target-dir target/tools-build`, so builds do not land in the OS temp dir.
  - Or download prebuilt binaries with pinned SHA-256 checksums: faster, and no compile.
  - State plainly that Valgrind and perf are system prerequisites of a Linux-only lane.
  - Deleting `target/` (`cargo clean`) also deletes the tools. On Windows that fails with sharing violations while a tool is running (inference from the Windows delete semantics cited in the portability note §2).
- **"The git CLI is already present wherever a clone exists" (attack 20).** Wrong for ZIP or tarball sources (no `.git`, so the leak gate's `git status` errors) and for GUI-only clients. xtask should detect both cases and say so explicitly.

### Gaps
- The git version shipped with Xcode 26 CLT was not found.
- Whether rustup's `x86_64-pc-windows-gnullvm`/`-gnu` toolchains can compile `lmdb-sys` without a separately installed C compiler was not verified.
- The VS Build Tools licence text itself was not fetched; only rustup's summary was.

## Can fixtures and the cross-OS content digest be byte-identical on Windows, macOS and Linux?

### Takeaway
Not with the blueprint's recipe. On case-insensitive hosts, `git fast-import` itself produces different trees, through repo-local `core.ignorecase` that `git init` sets (local experiment). Several matrix items need a worktree or a transport. The pinned env misses half of what the cited gitoxide tooling pins. The digest's hash, encoding and cross-OS comparison mechanism are unspecified. Byte-identity is achievable, but only by pinning every input and asserting golden fixture OIDs on every OS.

### Cited Findings
- **Local experiment, fast-import case collapse** (Apple Git 2.39.5, with `GIT_CONFIG_NOSYSTEM=1` and `GIT_CONFIG_GLOBAL=/dev/null`):
  - The stream: one commit with `M 100644 :1 Makefile`, `M 100644 :2 makefile`, `M 100755 :1 run.sh`, `M 120000 :2 link`.
  - Plain `git init` wrote `ignorecase = true` and `precomposeunicode = true` into `.git/config`. `git init --bare` wrote the same.
  - fast-import into that repo produced a tree with **3 entries**. `makefile` was dropped, and `Makefile` got the second blob's content. Commit `452154be…`.
  - The same stream with `core.ignorecase=false` produced **4 entries**: commit `23bc481b…`.
  - `git -c core.ignorecase=false fast-import` on a default-initialized repo also produced `23bc481b…`. A command-line override fixes it.
- **Local man page, `core.ignoreCase`.** "The default is false, except git-clone(1) or git-init(1) will probe and set core.ignoreCase true if appropriate when the repository is created." The listed case-insensitive filesystems include "APFS, HFS+, FAT, NTFS". `core.precomposeUnicode` "is only used by Mac OS implementation of Git". — [git-config](https://git-scm.com/docs/git-config) (quoted from the local Apple Git 2.39.5 man page)
- **Local man page.** `core.attributesFile` "default value is $XDG_CONFIG_HOME/git/attributes", and `core.excludesFile` defaults to `$XDG_CONFIG_HOME/git/ignore`. These are files, not config, so `GIT_CONFIG_GLOBAL=/dev/null` does not suppress them (inference from the wording). `gc.autoDetach`: "Make git gc --auto return immediately and run in background … Default is true." — [git-config](https://git-scm.com/docs/git-config)
- **gix-testtools `configure_git_environment`:**
  - It removes every inherited variable whose uppercase name starts with `GIT_`.
  - It removes `SSH_ASKPASS`, `BASH_ENV`, `ENV` and `CDPATH`.
  - It sets `MSYS=…winsymlinks:nativestrict`, `XDG_CONFIG_HOME` (to a fixture-local dir), `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL` (null device), `GIT_ATTR_NOSYSTEM=1` and `GIT_TERMINAL_PROMPT=false`.
  - It sets fixed `GIT_AUTHOR_*`/`GIT_COMMITTER_*` names, emails and dates, and `GIT_DEFAULT_HASH`.
  - Via `GIT_CONFIG_PARAMETERS` it sets `commit.gpgsign=false`, `tag.gpgsign=false`, `init.defaultBranch=main`, `protocol.file.allow=always`, `maintenance.auto=false` and `gc.auto=0`.
  - Read-only fixtures are written to `tests/fixtures/generated-do-not-edit/…` relative to the crate root. Archives go to `tests/fixtures/generated-archives/`. Both are **outside `target/`**.
  — [gix-testtools lib.rs](https://raw.githubusercontent.com/GitoxideLabs/gitoxide/main/tests/tools/src/lib.rs)
- **githooks.** "Environment variables, such as `GIT_DIR`, `GIT_WORK_TREE`, etc., are exported so that Git commands run by the hook can correctly locate the repository. If your hook needs to invoke Git commands in a foreign repository … it should clear these environment variables". — [githooks](https://git-scm.com/docs/githooks)
- **Local experiment, missing ident.** With `GIT_CONFIG_GLOBAL=/dev/null` and no ident env, `git notes add` failed: `fatal: unable to auto-detect email address (got 'singh.deependra-int@MB-QPPXWL7YX7.(none)')`. The blueprint lists fixed dates but not fixed names and emails.
- **Git for Windows long paths.** Support is opt-in and off by default, because of the Win32 260-character limit. — [Git for Windows: long paths](https://gitforwindows.org/git-cannot-create-a-file-or-directory-with-a-long-path)
- **Windows temp path.** `GetTempPath2W` checks TMP, TEMP, USERPROFILE, then the Windows dir. For SYSTEM processes it returns `C:\Windows\SystemTemp`. "The maximum possible return value is MAX_PATH+1 (261)." — [MS GetTempPath2W](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-gettemppath2w)
- **Line endings.** Local repo fact: there is no `.gitattributes` in the repo, and `rustfmt.toml` sets no `newline_style`.

### Inferences
- **Attack 1 failure scenario.** A proptest or matrix fixture contains a case-only rename (`R Foo foo`) or a `Makefile`/`makefile` pair. On Linux the tree has both entries. On macOS, and almost certainly on Windows where init also probes the case-insensitive NTFS, they collapse. Both oracle and product then read the same collapsed repo, so each OS is green on its own. The "case-only renames" matrix item silently tests nothing on macOS and Windows, and any cross-OS digest comparison goes red. **Revision:**
  - Create every fixture with `-c core.ignorecase=false -c core.precomposeunicode=false` (or rewrite `.git/config` after init).
  - Better: write fixture objects with gix from Rust and use the git CLI only as the read-side oracle.
  - Commit golden `(HEAD oid, sorted ref list)` per fixture and assert them before the oracle runs.
- **Attack 2 failure scenarios:**
  - (a) `cargo xtask ci` runs from a husky or pre-push hook, so `GIT_DIR`/`GIT_WORK_TREE` are exported. The fixture `git fast-import` and `update-ref` commands then write into FinalGitGraph's own `.git`, which `git status` never shows.
  - (b) A developer exports `GIT_DEFAULT_HASH=sha256` or a reftable default. Every fixture OID changes, or gix cannot read the refs.
  - (c) The host has no domain-qualified hostname, so commit-creating fixture steps fail (observed locally).
  - (d) `git commit`/`rebase`-style steps trigger detached auto-gc (`gc.autoDetach` default true). A background process repacks while the product ingests, which is nondeterministic. On Windows it holds handles, so `TempDir` drop fails and the "run dir is empty" check goes red.
  - (e) A global `~/.config/git/attributes` (for example `* text=auto eol=crlf`) changes blob bytes for any worktree-based fixture.

  **Revision:** use `env_clear()` and then an allowlist (`PATH`, `SystemRoot`/`windir` on Windows, `TMP`/`TEMP`/`TMPDIR` redirected into the run dir, `HOME`/`XDG_CONFIG_HOME` redirected). Then apply the full gix-testtools set, and add `GIT_CEILING_DIRECTORIES` and `-c core.longpaths=true` on Windows.
- **Attack 18, Windows path length.** Take the common clone root `C:\Users\<name>\source\repos\FinalGitGraph\` (about 45 characters). Add `target\gitgraph-artifacts\run-<36-char uuid>\` (about 67) and a test-named fixture dir (about 40). Add `.git\objects\pack\pack-<64 hex>.idx` for SHA-256 fixtures (about 90). That is about 240 characters, before deeper nesting. With `GIT_CONFIG_NOSYSTEM=1`, the usual fix (`git config --system core.longpaths true`) is dropped. And if `TMP` is redirected into a deep run dir, `GetTempPath2` cannot return it (261 maximum). **Revision:** short run ids (8 hex), short fixture names, an explicit `-c core.longpaths=true`, a doctor check on base path length, and one Windows CI leg cloned into a deliberately deep path.
- **Attack 19, worktree-dependent matrix items:**
  - Rebase, merge and `git add`-based fixtures check files out. That brings back case collisions, NFD/NFC handling via `precomposeUnicode`, and Windows symlinks (gitoxide needs `MSYS=winsymlinks:nativestrict`, i.e. native symlink privileges).
  - Shallow and partial clones need a transport plus server-side config (`protocol.file.allow`, which gix-testtools sets to `always`, and an upload-pack filter setting). Not verified this session.

  **Revision:** express every mutation at object level (fast-import, `commit-tree`, `mktree`, `update-ref`, `repack`, `commit-graph write`, clone via `file://` with explicit `-c` settings). Ban checkout in fixture code.
- **Attack 11, cross-OS mechanism:**
  - Each OS job only checks its own oracle. Nothing compares Windows bytes with Linux bytes unless there are committed golden digests per `(generator_version, seed, size)` or a fan-in job that diffs the matrix's JSON artifacts.
  - The digest must be a specified cryptographic hash (BLAKE3 or SHA-256) over a versioned, length-prefixed little-endian byte encoding, never `std::hash`. (Inference: std documents `DefaultHasher`'s algorithm as unspecified and not to be relied on across releases. Not fetched this session.)
  - Committed fast-import streams, golden files and snapshots need `.gitattributes` `-text`. fast-import's `data <n>` counts exact bytes, so a CRLF rewrite on a Windows checkout corrupts the stream (inference; Git for Windows' installer default for `core.autocrlf` was not verified).
- **Claims tested and not broken.** fast-import accepted `100755` and `120000` (symlink) modes without any checkout (local experiment). Mode bits and symlinks *can* be created identically on all OSes this way. Gitlinks (`160000`) were not tested. Fixed dates are irrelevant for fast-import, because the stream carries the timestamps. Harmless.

### Gaps
- Windows `git init` writing `core.ignorecase=true` and `core.symlinks=false` was not observed directly (no Windows host).
- Whether Git for Windows' `%PROGRAMDATA%\Git\config` is skipped by `GIT_CONFIG_NOSYSTEM=1` was not verified.
- Whether `git fast-import` ever triggers auto-gc was not verified.

## Is the artifact contract and leak gate sound (false positives, false negatives, races, dependency writes)?

### Takeaway
The gate as specified is both too strict and too weak. It flags ignored IDE and log dirs already listed in this repo's `.gitignore`, and default writes by proptest, insta and cargo-mutants. It cannot see writes into `.git/`, HOME or app-data, or temp leaks without a project prefix. Moving fixtures under `target/`, inside the project worktree, adds a new hazard: git and gix repository discovery can escape upward into FinalGitGraph's own repo. The sweep is racy with an OR-ed age rule. `CARGO_TARGET_TMPDIR` does not reach unit tests.

### Cited Findings
- **`.gitignore`.** Local repo fact: it lists `target/`, `apps/desktop/src-tauri/target/`, `.gitgraph-lab/`, `.cargo-install-target/`, `.tmp/`, `tmp/`, `.idea/`, `.vscode/` and `*.log`. `git status --porcelain --ignored` reports every one of these that exists.
- **proptest.** Default `failure_persistence` is `FileFailurePersistence::SourceParallel("proptest-regressions")`. `rng_seed` is "Random, generated when the variable is undefined" (`PROPTEST_RNG_SEED`). `cases` = 256. `max_shrink_iters` defaults to `u32::MAX`, which means four times the number of cases. Timeout 0 means none. — [proptest Config](https://docs.rs/proptest/latest/proptest/test_runner/struct.Config.html). Failures persist "in a separate directory tree rooted at `proptest-regressions`" and store the seed, not the value. "the seed may or may not produce the problematic value" after strategy changes. — [proptest book: failure persistence](https://proptest-rs.github.io/proptest/proptest/failure-persistence.html)
- **insta.** `INSTA_UPDATE=auto` is "the default. `no` for CI environments or `new` otherwise". `new` will "write new snapshots into `.snap.new` files", next to the snapshots, outside `target/`. — [insta advanced](https://insta.rs/docs/advanced/)
- **cargo-mutants.** "copies your tree to a temporary directory before mutating and building it". VCS dirs (`.git` etc.) and `target/` are not copied by default. `--in-place` "builds mutated code in the original source directory". — [mutants.rs build dirs](https://mutants.rs/build-dirs.html). "A `mutants.out` directory is created in the original source directory". Configurable via `--output`, `CARGO_MUTANTS_OUTPUT` or the config file. — [mutants.rs mutants.out](https://mutants.rs/mutants-out.html)
- **`CARGO_TARGET_TMPDIR`.** Listed under "Environment variables Cargo sets for crates", i.e. compile time via `env!`. "Only set when building integration test or benchmark code … Cargo initially creates this directory but doesn't manage its content in any way". — [Cargo env vars](https://doc.rust-lang.org/cargo/reference/environment-variables.html)
- **nextest env.** nextest sets its env vars "at runtime only". On archive reuse, `CARGO_BIN_EXE_<name>` is remapped. The page does not mention `CARGO_TARGET_TMPDIR`. — [nextest env vars](https://nexte.st/docs/configuration/env-vars/). Setup scripts are still "Experimental: This feature is not yet stable". — [nextest setup scripts](https://nexte.st/docs/configuration/setup-scripts/)
- **dirs-sys.** On Windows it calls `SHGetKnownFolderPath` (for example `FOLDERID_LocalAppData`), not env vars. On Unix, `home_dir` reads `HOME` and falls back to `getpwuid_r`. — [dirs-sys lib.rs](https://raw.githubusercontent.com/dirs-dev/dirs-sys-rs/main/src/lib.rs)
- **`File::lock`.** `lock`/`try_lock` are stable since Rust 1.89.0. They map to `flock` on Unix and `LockFileEx` on Windows. "This lock may be advisory or mandatory." A lock is released when all handles close. — [std::fs::File](https://doc.rust-lang.org/std/fs/struct.File.html). The repo pins 1.94.0, so the API is available.
- **Current tests and perf harness.** Local repo fact: `tests/indexer_smoke.rs` runs `Command::new("git").arg("-C").arg(repo_dir)` with the full inherited env and no ceiling dirs. `src/bin/perf-report.rs:535` falls back to `env::temp_dir`.
- **`safe.directory`.** Local man page: "only respected in protected configuration". By default Git "will refuse to even parse a Git config of a repository owned by someone else". — [git-config](https://git-scm.com/docs/git-config)

### Inferences
- **Attack 5, false positives.**
  - Any developer with JetBrains or VS Code open, or a `*.log`, or a legacy `tmp/perf-report-*.txt`, fails the gate before any test runs.
  - Editors write probe and swap files during a long run (for example vim's `4913` write test, `.swp`, Emacs `.#file`), so even a before/after diff is racy.
  - A first proptest failure creates `proptest-regressions/…`. A snapshot change creates `.snap.new`. A mutants run creates `mutants.out/`. All are outside `target/`.

  **Revision:**
  - Locally, compare before/after snapshots (`git status --porcelain=v1 -z --ignored=matching --untracked-files=all`, excluding `target/`) and report only new paths.
  - In CI on a fresh checkout, keep the absolute check.
  - Set `INSTA_UPDATE=no`, `CARGO_MUTANTS_OUTPUT=<target>/mutants.out` and `PROPTEST_DISABLE_FAILURE_PERSISTENCE` in CI. Regressions are instead committed as explicit DAG fixtures (next section).
  - Run the gate's own `git status` with the user's normal config, not the pinned fixture env. Dropping global config removes the `safe.directory` exemptions, which are honored only in protected config, so container or shared-drive checkouts would fail with "dubious ownership".
- **Attack 6, false negatives.**
  - (a) Upward discovery. Fixtures live under `target/gitgraph-artifacts/run-*/…`, which is inside FinalGitGraph's worktree. Consider a test that opens a not-yet-initialized dir, a failed fast-import, or a "not a repo" negative test that uses `git -C` or `gix::discover`. It finds FinalGitGraph's `.git` and silently ingests or mutates the real repo. With the OS temp dir there was normally no enclosing repo, so the blueprint's move creates this hazard.
  - (b) Writes inside `.git/` (objects, refs, config) never appear in `git status`.
  - (c) The blueprint moves the CLI default DB to per-OS app-data dirs. A test that forgets `--db` writes into `~/Library/Application Support/<id>` or `%LOCALAPPDATA%`, which no gate checks. On Windows, env redirection cannot contain it, because known folders ignore env.
  - (d) "New project-prefixed temp entries" misses `tempfile::tempdir()`, whose default prefix is `.tmp`, plus cargo install and cargo-mutants temp dirs.

  **Revision:**
  - Set `GIT_CEILING_DIRECTORIES` to the run root's parent. Always pass `--git-dir`, and use open-not-discover with gix. Assert that each fixture's `rev-parse --absolute-git-dir` is inside the run dir.
  - Redirect `TMP`/`TEMP`/`TMPDIR`/`HOME`/`XDG_*` for test child processes into `run-<id>/{tmp,home}` and assert those dirs are empty.
  - Library code takes only explicit paths. App-data defaults live only in the CLI/Tauri `main`.
  - Hash-check FinalGitGraph's `.git/refs` and `packed-refs` before and after the run.
- **Attack 7, sweep races.**
  - "Not owned by a live process, **or** older than N hours" deletes a live 10M-commit perf run that is older than N hours.
  - PID files suffer PID reuse.
  - The portability note recommends a user-level shared `CARGO_TARGET_DIR` (for Dev Drive). Two worktrees or projects would then share `gitgraph-artifacts/` and sweep each other.
  - On Windows, deleting a dir with an open handle partly fails, so the sweep errors.

  **Revision:** hold `File::lock` on `run-<id>/.lock` for the run's life. The sweeper `try_lock`s, deletes only unlocked dirs ("AND older than N"), treats sharing violations as "skip and warn", and namespaces run dirs by workspace-root hash.
- **Attack 8, unit tests.** libtest and nextest give no channel for `--artifact-root`. It must become an env var read in tests, which needs an exemption from the env ban. Plain `cargo test` from rust-analyzer's run lens has no xtask, so any fallback either leaks (temp) or guesses wrongly when `CARGO_TARGET_DIR` is customized. Because `CARGO_TARGET_TMPDIR` is baked in at compile time, it is also wrong for reused nextest archives on another machine (inference from the docs' silence).

  **Revision:** forbid filesystem or git fixtures in unit tests and keep them in integration tests. If unit tests need disk, derive the root from `std::env::current_exe()` ancestors (the test binary lives in `<target>/<profile>/deps/`). Hard-error with no fallback.

### Gaps
- cargo's own `cargo-test-support` root-finding (believed to use `current_exe`) was not inspected.
- Whether gix writes anything outside the repo during reads was not checked.
- Where Windows Error Reporting crash dumps from crashing test binaries land (`%LOCALAPPDATA%\CrashDumps`) was not verified.

## What can the oracle still miss (blind spots), and does proptest + fast-import scale?

### Takeaway
The oracle catches the specific edge-loss bug. It does not verify:
- commit metadata or trees
- query semantics (topological-order validity, BFS depth, truncation, pagination and tie-breaks, shortest-path minimality)
- non-first-parent merge diffs
- the semantics of which refs and which object view

`rev-list --all` includes notes commits, and honors replace refs by default (both shown by local experiment). The `git log` command as written walks HEAD only. proptest with random seeds and spawned git processes cannot support cross-OS identity and scales poorly. Small DAGs with a generated batch size, plus an in-process reference model, should carry the load.

### Cited Findings
- **Local experiment, notes.** After `git notes add`, `git rev-list --all --parents` printed an extra commit `a5432bf…` from `refs/notes/commits`. A product that indexes only branches, tags and HEAD would "lose" it against the oracle, or be forced to index notes.
- **Local experiment, replace refs.** Chain `c1 <- c2 <- c3`, then `git replace c2 <parentless alt>`:
  - `git rev-list --parents main` printed c2 with **no parent**, and c1 disappeared.
  - `git --no-replace-objects rev-list --parents main` printed the true chain.
  - The oracle's answer therefore depends on a semantic choice that the blueprint never states, even though `git replace` is in its fixture matrix. Git 3.0 also removes grafts. — [git BreakingChanges](https://git-scm.com/docs/BreakingChanges)
- **Local man page.** `rev-list --all`: "Pretend as if all the refs in refs/, along with HEAD, are listed on the command line". That covers notes, stash, remotes and replace refs. — [git-rev-list](https://git-scm.com/docs/git-rev-list)
- **proptest defaults.** 256 cases, a random seed per run, shrink iterations limited to 4× cases, no timeout. — [proptest Config](https://docs.rs/proptest/latest/proptest/test_runner/struct.Config.html)
- **Process creation.** nextest notes that process creation is slow on Windows, and slow on macOS when anti-malware interferes. — [nextest: why process-per-test](https://nexte.st/docs/design/why-process-per-test/) (cited in the portability note §2)

### Inferences
- **Bug classes that pass the blueprint's oracle:**
  1. Wrong author or committer ident, timezone offset, timestamps, `encoding` header, message bytes, or extra headers (`gpgsig`, `mergetag`). The oracle compares only parents and changes.
  2. A wrong tree OID per commit.
  3. Wrong generation numbers.
  4. Topological-order output that is invalid, or unstable between runs. Many valid orders exist, so equality against git is the wrong check; a property check is needed.
  5. A wrong BFS depth or level, wrong `max_nodes` truncation flags, or off-by-one window pagination.
  6. `shortest_path` that finds *a* path but not a minimal one.
  7. Wrong diffs against second and later parents of merges, if the product stores them. `--diff-merges=first-parent` only checks parent 1.
  8. Rename-similarity errors (explicitly "reported, not gated").
  9. Ref-set mismatches (notes, stash, remotes, detached HEAD) that cause false failures.
  10. Replace, shallow and graft semantics. For shallow repos, rev-list prints boundary commits without parents, while the product sees parent OIDs it cannot load.
  11. Commit-graph versus object walk under replace refs. Inference: git does not use the commit-graph when replace objects are present, so the two paths can disagree legitimately.
- **The change-set command needs fixing.** As written (`git log --raw -z --no-abbrev --no-renames --diff-merges=first-parent`) it has no `--all` or explicit refs, so only HEAD's history is diffed. With no `--format`, the `-z` stream mixes commit headers and raw records. Use explicit refs via `--stdin`, `--format=%x00%H`, `--no-replace-objects` (or test both modes), and `-c log.showRoot=true`. Alternatively use `git diff-tree --stdin -r -z --root --no-renames`.
- **Revision: oracle spec ADR.** Name the exact ref set, the replace, shallow and graft policy, and the merge-diff semantics. Add:
  - (a) a metadata oracle: `git cat-file --batch` hashing of raw commit objects, or `git log --format=%H%x00%T%x00%P%x00%an%x00%ae%x00%ad%x00%cn%x00%ce%x00%cd%x00%B`
  - (b) a naive in-process reference model over the oracle parent lists for BFS, ancestry, merge-base and shortest path, plus a property check that every emitted order is a valid topological order
  - (c) `--diff-merges=separate` checks if the product stores per-parent changes
- **Attack 10, scaling and reproducibility.**
  - Each case spawns fast-import, rev-list and log, plus one `merge-base` or `--is-ancestor` per sampled pair. With 20 pairs that is about 23 spawns per case.
  - 256 cases × 23 ≈ 5,900 spawns. Shrinking adds up to 1,024 × 23 ≈ 23,500 more.
  - At about 50–100 ms per git spawn on a Defender-scanned Windows box (inference), that is minutes to tens of minutes per property, on every PR.
  - A 16,384-commit DAG per case is impractical to shrink, because each shrink step regenerates and re-ingests.
  - Seeds are random per run, so Windows, macOS and Linux run *different* DAGs. Cross-OS identity cannot be claimed over proptest cases.
  - Persisted seeds silently stop reproducing after strategy edits.

  **Revision:**
  - Small DAGs (≤200 commits), with batch size drawn from `1..=n`, so boundaries are crossed at small scale. This needs the batch size to be the *real* production wave parameter, not a test-only knob.
  - Ancestry from the in-process model, and git `merge-base` only on a sampled subset.
  - A fixed, committed seed corpus run identically on all OSes, used for golden digests.
  - On failure, dump the shrunk DAG as a fast-import stream file and commit it as a named regression test (bytes, not seeds).
  - Keep the deterministic 20k-commit Phase 0 test for the real 16,384 constant.
- **Claims tested and not broken:**
  - The oracle commands exist in the oldest relevant git, Apple 2.39.5 (`--diff-merges` is in the 2.32 notes).
  - Comparing canonicalized sets rather than counts is right.
  - Metamorphic batch, thread and path invariance directly targets the bug class.

### Gaps
- Whether gix 0.80 honors replace refs and `GIT_NO_REPLACE_OBJECTS` by default was not verified. That determines whether product and oracle even agree by default.
- Actual git spawn latency on GitHub Windows runners was not measured.

## Do the perf gates hold up on all OSes and against gaming?

### Takeaway
"Output validated before timing" is sound. Gating is not "identical on all OSes":
- Per-PR gates run only on Linux, and need a system Valgrind.
- Instruction counts are blind to fsync, syscalls, I/O and lock contention. Those are the costs the proposed immutable-layer storage design (write, sync, rename) adds, and they differ most between OSes.
- Instruction counts of the rayon/crossbeam parallel ingest are scheduling-sensitive under Valgrind (inference).
- The wall-time lane assumes fixed hardware the project does not have.

Hosted arm64 and Intel runners do exist.

### Cited Findings
- **gungraun.** "Gungraun cannot be run on Windows or targets that support neither the Valgrind-based tools nor Linux perf." It claims Valgrind-based measurements are consistent "even in virtualized CI environments". — [gungraun guide](https://gungraun.github.io/gungraun/latest/html/). It needs a `valgrind` binary on `$PATH` (a system package) and debug symbols in the bench profile. — [gungraun prerequisites](https://gungraun.github.io/gungraun/latest/html/installation/prerequisites.html)
- **GitHub-hosted runner labels** (Nov 2026):
  - Linux arm64: `ubuntu-24.04-arm`, `ubuntu-22.04-arm`, `ubuntu-26.04-arm`
  - Windows arm64: `windows-11-arm`, `windows-11-vs2026-arm`
  - macOS Intel: `macos-15-intel`, `macos-26-intel`
  - macOS arm64: `macos-latest`/`14`/`15`/`26`

  Private repos get 2 vCPU on Linux and Windows. "Use of the standard GitHub-hosted runners is free and unlimited on public repositories"; private repos draw on included minutes. — [GitHub-hosted runners](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)
- **Bencher.** Thresholds need history. CI does not fail unless `--error-on-alert` is passed. — [Bencher thresholds](https://bencher.dev/docs/explanation/thresholds/) (cited in the portability note §5; not re-fetched)

### Inferences
- **Attack 13.** Consider a PR that adds one `sync_all()` per batch, or reads every layer file twice, or adds a Windows-only sharing-retry loop. Callgrind's instruction count barely moves, because kernel time is not user instructions. The per-PR gate passes, and only the scheduled wall-time lane can notice, and only on its one OS. **Revision:**
  - Add *deterministic, portable* counters to the perf JSON and gate them on all three OSes on every PR: bytes read and written, fsync/flush calls, files opened and renamed, objects decoded, and allocations or bytes via a counting global allocator.
  - Run gungraun only for single-threaded kernels (`threads = 1`).
  - Label `--full` lanes per OS honestly (for example "Linux-only: instruction counts").
- **Thread scheduling.** Valgrind runs one thread at a time. Rayon work-stealing and crossbeam `Backoff` spinning then make instruction counts depend on scheduling. std `HashMap`'s random SipHash keys also vary probe sequences per process. Both add noise to "~1–2% thresholds" (inference; no gungraun doc on multi-threading was found).
- **Attack 21.** "Fixed hardware" does not exist in the plan. Without a self-hosted bare-metal runner, the Bencher lane compares noisy 2 vCPU VMs. Either budget the hardware or drop the wall-time gate claim and keep it informational.
- **CPU features.** Runner CPU generations can change runtime-detected SIMD paths (memchr, sha1). That can shift instruction-count baselines when GitHub rotates hardware (inference). Record the CPU model in the perf JSON and reset baselines per model.
- **Not broken:**
  - Digest before timing, `black_box`, and setup outside the timed region are sound.
  - "Valgrind does not run on Windows or modern macOS" is correct.
  - Hosted arm64 Linux and Windows and Intel macOS runners do exist, including for private repos, so the five-leg matrix is feasible.

### Gaps
- gungraun's multi-threading guidance was not found.
- Bencher's gungraun adapter support was not re-verified.
- Hosted-runner per-minute multipliers for macOS and arm64 in private repos were not fetched.

## Do clippy disallowed-methods, the crate split and the tooling choices enforce what is claimed? (incl. missed alternatives)

### Takeaway
Clippy bans are useful but leakier than claimed:
- They are not scopable to "outside the config module" by configuration.
- Repo policy bans the attribute-based escape hatch.
- Method-form and sibling APIs bypass a path-only ban.
- Dependencies are never linted.

The crate split for layering is sound. Making nextest optional creates two test semantics. cargo-mutants defaults break the artifact contract.

### Cited Findings
- **clippy.toml discovery.** `CLIPPY_CONF_DIR`, then `CARGO_MANIFEST_DIR`, then the current directory. If none has a file, "walk up the directory tree … until it finds one". No merging or inheritance is documented. Lint scoping to "a single function or module" is documented only with attributes. "The configuration file is unstable and may be deprecated in the future." — [Clippy configuration](https://doc.rust-lang.org/clippy/configuration.html)
- **`--cap-lints`.** Cargo "will pass `--cap-lints allow` when compiling your dependencies". — [rustc lint levels](https://doc.rust-lang.org/rustc/lints/levels.html)
- **Repo policy on lint-bypass attributes.** Local repo fact: AGENTS.md "No quality-marker debt (… lint-bypass attributes)", and `docs/automation-guardrails.md:95` bans "`#[allow(clippy::...)]` … that bypass normal quality gates".
- **Workspace lints.** Local repo fact: workspace `Cargo.toml` has `edition = "2021"`. `[workspace.lints]` already has `unsafe_code = "forbid"`, `dbg_macro`, `todo`, `unwrap_used` and `expect_used` set to deny. `clippy.toml` has `msrv = "1.94"` and allows unwrap and expect in tests.
- **Env-var count.** Rust 2024 makes `set_var`/`remove_var` unsafe. — [Edition guide](https://doc.rust-lang.org/nightly/edition-guide/rust-2024/newly-unsafe-functions.html) (via the portability note). The current smoke test calls `std::env::set_var` safely under edition 2021 (`indexer_smoke.rs` `EnvVarGuard`).

### Inferences
- **Attack 17, scoping.** Banning `std::env::var` everywhere except a config module needs one of these:
  - (a) a separate `gitgraph-config` crate with its own `clippy.toml`. Since no merging is documented, that file must duplicate every other ban and will drift.
  - (b) `#[expect(clippy::disallowed_methods, reason = "…")]` at the call site. This conflicts with AGENTS.md and automation-guardrails rule 95 unless the ADR explicitly carves out `expect` with a reason.

  xtask itself needs `CARGO` from env (the blueprint's `env::var("CARGO")`), so it is a second exemption.
- **Bypasses.**
  - `std::fs::canonicalize` and `std::path::Path::canonicalize` are distinct items; ban both.
  - Ban `var`, `var_os`, `vars` and `vars_os`.
  - `env!`/`option_env!` are compile-time.
  - `tempfile::tempdir()`, `TempDir::new()` and `NamedTempFile::new()` call the temp dir *inside the dependency*. Ban the tempfile constructors without `_in`.
  - gix's internal env and config reads cannot be banned at all. Use gix's isolated open options instead (believed to exist; not verified).
  - `Command::new("git")` without `current_dir`/`env_clear` cannot be expressed as a clippy ban. Route all git spawning through one helper and ban `std::process::Command::new` elsewhere.
- **Missed alternative for paths.** `std::path::absolute` (stable; version not verified this session) absolutizes without touching the filesystem or producing `\\?\`. It covers most "need an absolute path" cases. Keep `dunce::canonicalize` only where symlink resolution is required. macOS `/var` to `/private/var` canonicalization is a classic test-comparison flake (inference).
- **Attack 16, nextest.**
  - With nextest optional, the core lane runs libtest: all tests of a binary run in one process, there is no per-test timeout, process-global caches are shared (the current `QueryGraph` cache is process-global per the blueprint), and Windows handle lifetimes cross tests. The `--full` lane runs nextest with process isolation and timeouts.
  - A test can pass in one lane and fail or hang in the other, so "identically" breaks.
  - **Revision:** make nextest the single runner, fetched by xtask as a checksum-pinned prebuilt binary. Or run the core suite under both runners in CI.
- **Attack 15, cargo-mutants.**
  - Set `output = "target/mutants.out"`. Accept or redirect the temp-dir copy. Never use `--in-place` locally, because an interrupted run leaves mutated source.
  - Give mutants a fast test profile: a fixed small corpus, low `PROPTEST_CASES`, and `--test-package`.
  - Shard and set timeouts. Each mutant costs an incremental build plus the relevant suite, so `--in-diff` on large hunks in `db.rs`/`git.rs` can take hours (inference).
  - Copies exclude `.git`, so any test relying on the enclosing repo breaks under mutants. That is another reason to forbid such dependence.
- **Alternatives.**
  - `cargo xtask` over just, cargo-make or mise stands: just needs `sh`, and the others add a tool.
  - Running the leak gate on GitHub-hosted Windows and macOS fresh checkouts is the cheap way to get the absolute check without dev-machine false positives.
- **Not broken:**
  - Layering via crates makes illegal imports compile errors.
  - `unsafe_code = "forbid"` is already in place.
  - The rejection of just and Nix on Windows grounds.
  - Using the `CARGO` env var (compile-time `env!("CARGO")` for crates and xtask binaries).

### Gaps
- Whether clippy's disallowed-methods matches calls through `use … as` aliases or trait re-exports was not tested; it is believed to match by resolved definition.
- The existence and behavior of gix's isolated-open options in 0.80 were not verified.
- The stabilization version of `std::path::absolute` was not re-checked.
