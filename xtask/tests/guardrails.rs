//! Every guardrail check fires on its own planted violation, in xtask and (until S2 Phase 4)
//! in `scripts/enforce-guardrails.ps1`, with the same check names (ADR 0020).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

const XTASK: &str = env!("CARGO_BIN_EXE_xtask");
const S: &str = "apps/desktop/src-tauri/src/";

// Spelled in pieces so the repository never contains the words the guardrails ban.
const LEGACY_WORD: &str = concat!("s", "q", "l");
const LEGACY_ENGINE: &str = concat!("s", "q", "l", "ite");
const LEGACY_DRIVER: &str = concat!("ru", "s", "q", "l", "ite");

const T: &str = "Docs/guardrails changes without implementation changes are blocked. Changed files";
const PERF_FILE: &str = "Required performance file missing";

enum Mutation {
    Replace(&'static str, &'static str, &'static str),
    Append(&'static str, &'static str),
    AppendOwned(&'static str, String),
    Write(&'static str, &'static str),
    WriteOwned(String, &'static str),
    Remove(&'static str),
}

struct Fixture {
    name: &'static str,
    mutations: Vec<Mutation>,
    expected: Vec<&'static str>,
    powershell: bool,
}

fn fixture(name: &'static str, mutations: Vec<Mutation>, expected: Vec<&'static str>) -> Fixture {
    Fixture {
        name,
        mutations,
        expected,
        powershell: true,
    }
}

fn src(file: &str) -> &'static str {
    Box::leak(format!("{S}{file}").into_boxed_str())
}

fn fixtures() -> Vec<Fixture> {
    use Mutation::{Append, AppendOwned, Remove, Replace, Write, WriteOwned};
    let git = src("adapters/git.rs");
    let indexer = src("application/indexer.rs");
    let db = src("adapters/db.rs");
    let doc = "docs/phase-1-kickoff.md";
    vec![
        fixture("clean", vec![], vec![]),
        fixture("root-lock", vec![Remove("Cargo.lock")], vec!["Root Cargo.lock is missing."]),
        fixture(
            "nested-lock",
            vec![Write("nested/Cargo.lock", "")],
            vec!["Nested Cargo.lock files must not exist. Found"],
        ),
        fixture(
            "src-root",
            vec![Remove(src(""))],
            vec!["Missing Rust source root", PERF_FILE],
        ),
        fixture("layer", vec![Remove(src("support"))], vec!["Layer directory missing"]),
        fixture(
            "stray-root",
            vec![Write(src("stray.rs"), "")],
            vec!["Unexpected root Rust files found"],
        ),
        fixture(
            "legacy-flat",
            vec![Write(src("indexer.rs"), "")],
            vec![
                "Legacy flat modules should not exist",
                "Unexpected root Rust files found",
            ],
        ),
        fixture(
            "marker",
            vec![Append(src("lib.rs"), "// TODO tidy\n")],
            vec!["Forbidden markers found in Rust source tree at"],
        ),
        fixture(
            "duct-tape",
            vec![Append(src("lib.rs"), "// HACK quick\n")],
            vec!["Duct-tape quality markers are blocked in Rust source tree"],
        ),
        fixture(
            "lint-allow",
            vec![Append(src("lib.rs"), "#[allow(dead_code)]\nfn probe() {}\n")],
            vec!["Lint-suppression duct-tape attributes are blocked in Rust source tree"],
        ),
        fixture(
            "legacy-runtime",
            vec![Write(src("application/graph_queries.rs"), "")],
            vec!["Legacy relational runtime modules must not exist"],
        ),
        fixture(
            "migration",
            vec![WriteOwned(
                format!("apps/desktop/src-tauri/migrations/0001.{LEGACY_WORD}"),
                "",
            )],
            vec!["Legacy relational migration files must be removed for LMDB runtime"],
        ),
        fixture(
            "relational-doc",
            vec![AppendOwned(doc, format!("\n{LEGACY_ENGINE} note\n"))],
            vec!["Forbidden legacy relational terms detected. Sample", T],
        ),
        fixture(
            "git-cli",
            vec![Append(git, "fn probe() { let _ = std::process::Command::new(\"git\"); }\n")],
            vec!["Git adapter hot ingestion path must be in-process"],
        ),
        fixture(
            "git2",
            vec![Append(git, "// git2::Repository\n")],
            vec!["Git adapter must not reintroduce git2/libgit2 APIs after gix migration."],
        ),
        fixture(
            "tape-first",
            vec![Replace(git, "track_rewrites(", "track_rewritez(")],
            vec!["Git adapter must keep tape-first gix ingestion path"],
        ),
        fixture(
            "git-helper",
            vec![Append(git, "fn run_git_command() {}\n")],
            vec!["Legacy full-buffer git command helper must not be reintroduced."],
        ),
        fixture(
            "streaming-api",
            vec![Replace(
                git,
                "stream_file_changes_for_commits",
                "stream_file_changez_for_commits",
            )],
            vec!["Git adapter must expose streaming commit/file-change ingestion APIs."],
        ),
        fixture(
            "rayon",
            vec![Replace(
                git,
                "FILE_CHANGE_PARSE_PARALLEL_MIN_COMMITS",
                "FILE_CHANGE_PARSE_PARALLEL_MIN_C0MMITS",
            )],
            vec!["Git adapter must keep bounded Rayon parallel file-change compute path."],
        ),
        fixture(
            "git-db-free",
            vec![AppendOwned(git, format!("// {LEGACY_DRIVER}::Connection\n"))],
            vec![
                "Forbidden legacy relational terms detected. Sample",
                "Git adapter must remain DB-free",
            ],
        ),
        fixture(
            "rename",
            // Matching is case-insensitive, so the upper-case constant must go too.
            vec![
                Replace(git, "exact_match_only", "exact_match_0nly"),
                Replace(git, "EXACT_MATCH_ONLY", "EXACT_MATCH_0NLY"),
            ],
            vec!["Git adapter must enforce bounded rename detection"],
        ),
        fixture(
            "channel",
            vec![Replace(git, "sync_channel", "sync_channe1")],
            vec!["Git adapter must keep bounded producer/consumer channel for parallel file-change extraction."],
        ),
        fixture(
            "rayon-plan",
            vec![Replace(git, "parallel_file_change_plan", "parallel_file_change_p1an")],
            vec!["Git adapter must keep adaptive-but-bounded Rayon planning"],
        ),
        fixture(
            "line-stats",
            vec![Replace(git, "collect_patch_line_stats", "collect_patch_line_statz")],
            vec!["Git adapter must keep two-stage selective line-count extraction"],
        ),
        fixture(
            "diff-telemetry",
            vec![Replace(git, "pub struct FileDiffTelemetry", "pub struct FileDiffTelemetri")],
            vec!["Git adapter must keep diff sub-phase telemetry contract"],
        ),
        fixture(
            "object-cache",
            vec![Replace(git, "object_cache_size_if_unset", "object_cache_size_if_unzet")],
            vec!["Git adapter must keep SHA validation and bounded object-cache sizing guards."],
        ),
        fixture(
            "metadata-only",
            vec![Replace(
                git,
                "load_file_change_line_stats_for_targets",
                "load_file_change_line_stats_for_targetz",
            )],
            vec!["Git adapter must enforce metadata-only hot ingestion and selective JIT line-stats enrichment path."],
        ),
        fixture(
            "indexer-stream",
            vec![Replace(indexer, "stream_commits(", "stream_commitz(")],
            vec!["Indexer must use streaming commit ingestion."],
        ),
        fixture(
            "indexer-batch",
            vec![Replace(indexer, "FILE_CHANGE_DB_BATCH_SIZE", "FILE_CHANGE_DB_BATCH_SIZ3")],
            vec!["Indexer must keep bounded file-change batch buffers."],
        ),
        fixture(
            "indexer-store",
            vec![Replace(indexer, "store.synchronize_refs(", "store.synchronize_refz(")],
            vec!["Indexer must orchestrate ingestion writes through GraphStore"],
        ),
        fixture(
            "indexer-query",
            vec![Replace(indexer, "struct DenseSet", "struct DenseZet")],
            vec!["Indexer query hot path must keep integer-based CSR traversal state with overlay-aware hydration"],
        ),
        fixture(
            "db-txn",
            vec![Replace(db, "pub struct GraphStore", "pub struct GraphZtore")],
            vec!["DB adapter must keep LMDB GraphStore transaction contract"],
        ),
        fixture(
            "db-csr",
            vec![Replace(db, "children_neighbors", "children_neighborz")],
            vec!["DB adapter must keep persisted CSR snapshot contract"],
        ),
        fixture(
            "db-metadata",
            vec![Replace(db, "GraphSnapshotMetadata", "GraphZnapshotMetadata")],
            vec!["DB adapter must keep explicit graph snapshot metadata/versioning contract."],
        ),
        fixture(
            "db-delta",
            vec![Replace(db, "META_NEXT_DELTA_SEQUENCE", "META_NEXT_DELTA_ZEQUENCE")],
            vec!["DB adapter must keep append-only delta mutation log contract."],
        ),
        fixture(
            "db-overlay",
            vec![Replace(db, "should_compact_graph_overlay", "should_compact_graph_overlai")],
            vec!["DB adapter must keep bounded graph overlay adjacency plus compaction contract."],
        ),
        fixture(
            "db-statements",
            vec![Append(db, "// DELETE FROM commits\n")],
            vec!["DB adapter must remain LMDB-only"],
        ),
        fixture(
            "db-prefix",
            vec![Replace(
                db,
                "repository_commit_file_change_prefix",
                "repository_commit_file_change_prefiks",
            )],
            vec!["DB adapter must keep prefix-set cleanup path for incremental file-change deletion."],
        ),
        fixture(
            "db-enrichment",
            vec![Replace(
                db,
                "mark_file_change_enrichment_failed",
                "mark_file_change_enrichment_faild",
            )],
            vec!["DB adapter must enforce stats-state + retryable enrichment queue contract."],
        ),
        fixture(
            "db-flags",
            vec![Replace(db, "GITGRAPH_LMDB_MAP_ASYNC", "GITGRAPH_LMDB_MAP_ASYNK")],
            vec!["DB adapter must keep centralized LMDB mmap/runtime flag parsing and map-size flooring"],
        ),
        fixture(
            "db-window",
            vec![Replace(db, "MDB_SET_RANGE", "MDB_SET_RANJE")],
            vec!["DB adapter must keep commit-window hot path on index/range-seek reads."],
        ),
        fixture(
            "db-due-index",
            vec![Replace(
                db,
                "parse_due_index_identity_from_prefixed_key",
                "parse_due_index_identity_from_prefixed_kei",
            )],
            vec!["DB adapter must keep due-index ordered enrichment acquisition path."],
        ),
        fixture(
            "perf-budget-script",
            vec![Replace("scripts/perf-budget.ps1", "--no-run", "--nx-run")],
            vec![
                "Performance budget script must keep probe mode, resilient fallback, and build/execute timing transparency contract.",
                T,
            ],
        ),
        fixture(
            "perf-report-script",
            vec![Replace(
                "scripts/perf-report.ps1",
                "function Invoke-PerfReportDocker",
                "function Invoke-PerfReportDokker",
            )],
            vec!["Performance report script must keep probe mode, resilient fallback, and build/execute timing transparency contract."],
        ),
        fixture(
            "ingestion-script",
            vec![Replace(
                "scripts/perf-ingestion-budget.ps1",
                "MaxDiffShareRatio",
                "MaxDiffShareRatyo",
            )],
            vec!["Ingestion budget script must keep profile/lane/probe modes and enforce full/incremental/ms + peak RSS + diff-share + warm fixture thresholds."],
        ),
        fixture(
            "perf-binary",
            vec![Replace(
                src("bin/perf-report.rs"),
                "aggregate_commit_bottleneck_phase",
                "aggregate_commit_bottleneck_faze",
            )],
            vec!["Performance report binary must emit phase timing + memory visibility contract, including in-memory adjacency, JIT/stats-state, ingestion-phase fields, and commit-throughput bottleneck telemetry."],
        ),
        fixture(
            "ci-budget",
            vec![Replace(".github/workflows/rust-ci.yml", "Performance Budget", "Perf Budget")],
            vec!["Rust CI workflow must include a Performance Budget step.", T],
        ),
        fixture(
            "ci-probe",
            vec![Replace(".github/workflows/rust-ci.yml", "Script Contract Probe", "Script Probe")],
            vec!["Rust CI workflow must include a Script Contract Probe step.", T],
        ),
        fixture(
            "package-perf",
            vec![Replace("package.json", "\"quality:perf\":", "\"quality-perf\":")],
            vec!["package.json must include a quality:perf script.", T],
        ),
        fixture(
            "package-ingestion",
            vec![Replace(
                "package.json",
                "\"quality:perf:ingestion\"",
                "\"quality:perf-ingestion\"",
            )],
            vec!["package.json must include a quality:perf:ingestion script.", T],
        ),
        fixture(
            "package-tiers",
            vec![Replace("package.json", "\"test:layers\"", "\"test-layers\"")],
            vec![
                "package.json must include perf:report, perf:report:smoke, perf:report:large, perf:report:25k, perf:report:25k:precondition, perf:report:90k, perf:ingestion:25k, perf:ingestion:90k, perf:ingestion:25k:warm, perf:ingestion:90k:warm, and test:layers scripts.",
                T,
            ],
        ),
        fixture(
            "required-doc",
            vec![Remove("docs/performance-engineering.md")],
            vec!["Required governance doc missing", T],
        ),
        fixture(
            "standards-index",
            vec![Replace("docs/standards/README.md", "REPO_SYNC_POLICY.md", "REPO_SYNC.md")],
            vec!["docs/standards/README.md must list REPO_SYNC_POLICY.md", T],
        ),
        fixture(
            "standards-section",
            vec![Replace("docs/standards/REPO_SYNC_POLICY.md", "## Sync Rules", "## Sync")],
            vec![
                "Standards file REPO_SYNC_POLICY.md is missing required section ## Sync Rules",
                T,
            ],
        ),
        fixture(
            "adr-index",
            vec![Write("docs/adr/0099-probe.md", "# probe\n")],
            vec!["docs/adr/README.md must reference ADR file 0099-probe.md"],
        ),
        fixture(
            "platform-header",
            vec![Replace("scripts/dev-loop.ps1", "# platform: any", "# platform: some")],
            vec!["Script missing platform metadata"],
        ),
        fixture(
            "portable",
            vec![Append("scripts/dev-loop.ps1", "\ncmd /c echo hi\n")],
            vec!["Portable script policy violation in scripts/dev-loop.ps1"],
        ),
        fixture(
            "perf-file",
            vec![Remove("scripts/layer-test-matrix.ps1")],
            vec![PERF_FILE],
        ),
        Fixture {
            name: "scripts-dir",
            mutations: vec![Remove("scripts")],
            expected: vec!["Missing scripts directory", PERF_FILE, PERF_FILE, PERF_FILE, PERF_FILE, T],
            // The PowerShell guardrail cannot run once its own directory is gone.
            powershell: false,
        },
        fixture("tamper-docs", vec![Append(doc, "\nnote\n")], vec![T]),
        fixture(
            "tamper-docs-and-src",
            vec![Append(doc, "\nnote\n"), Append(src("lib.rs"), "// note\n")],
            vec![],
        ),
        fixture(
            "tamper-guardrail-rules",
            vec![Append("xtask/src/guardrails/mod.rs", "// note\n")],
            vec![T],
        ),
        fixture(
            "tamper-xtask-tool",
            vec![Append(doc, "\nnote\n"), Append("xtask/src/main.rs", "// note\n")],
            vec![],
        ),
    ]
}

const WORKERS: usize = 8;

#[test]
fn every_guardrail_fires_on_its_planted_violation_with_powershell_parity() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("create temp dir: {error}"));
    let mut chunks: Vec<Vec<Fixture>> = (0..WORKERS).map(|_| Vec::new()).collect();
    for (index, fixture) in fixtures().into_iter().enumerate() {
        chunks[index % WORKERS].push(fixture);
    }
    // Each worker owns a clone, so the slow PowerShell runs overlap.
    let mismatches: Vec<String> = std::thread::scope(|scope| {
        let workers: Vec<_> = chunks
            .into_iter()
            .enumerate()
            .map(|(index, chunk)| {
                let clone = temp.path().join(format!("clone-{index}"));
                scope.spawn(move || run_fixtures(&clone, chunk))
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| {
                worker
                    .join()
                    .unwrap_or_else(|_| vec!["fixture worker panicked".to_string()])
            })
            .collect()
    });
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

fn run_fixtures(clone: &Path, fixtures: Vec<Fixture>) -> Vec<String> {
    clone_working_state(&repo_root(), clone);
    let mut mismatches = Vec::new();
    for fixture in fixtures {
        reset(clone);
        for mutation in &fixture.mutations {
            apply(clone, mutation);
        }
        let mut expected: Vec<String> = fixture
            .expected
            .iter()
            .map(|name| name.to_string())
            .collect();
        expected.sort();

        let xtask = failure_names(&run(Command::new(XTASK)
            .args(["guardrails", "--repo"])
            .arg(clone)));
        if xtask != expected {
            mismatches.push(format!(
                "{} (xtask): expected {expected:?}, got {xtask:?}",
                fixture.name
            ));
        }
        if fixture.powershell {
            let script = clone.join("scripts/enforce-guardrails.ps1");
            let powershell = failure_names(&run(Command::new("pwsh")
                .args(["-NoProfile", "-File"])
                .arg(script)));
            if powershell != expected {
                mismatches.push(format!(
                    "{} (powershell): expected {expected:?}, got {powershell:?}",
                    fixture.name
                ));
            }
        }
    }
    mismatches
}

fn repo_root() -> PathBuf {
    let xtask_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    xtask_dir.parent().unwrap_or(xtask_dir).to_path_buf()
}

fn git(dir: &Path) -> Command {
    let mut command = Command::new("git");
    for (key, _) in env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    let null_device = if cfg!(windows) { "NUL" } else { "/dev/null" };
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", null_device)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.com",
        ])
        .args(["-c", "commit.gpgsign=false", "-c", "core.ignorecase=false"])
        .arg("-C")
        .arg(dir);
    command
}

fn git_ok(dir: &Path, args: &[&str]) -> String {
    let output = git(dir)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("git {args:?}: {error}"));
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A clone holding the current working tree (tracked and untracked files) as one base commit,
/// then an empty commit, so the anti-tamper fallback `HEAD~1..HEAD` is always empty.
fn clone_working_state(repo: &Path, clone: &Path) {
    let source = repo.to_string_lossy().into_owned();
    let target = clone.to_string_lossy().into_owned();
    git_ok(repo, &["clone", "--quiet", "--local", &source, &target]);
    let listing = git_ok(
        repo,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    );
    for path in listing.split('\0').filter(|path| !path.is_empty()) {
        let from = repo.join(path);
        let to = clone.join(path);
        if from.is_file() {
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent).unwrap_or_else(|error| panic!("create {path}: {error}"));
            }
            fs::copy(&from, &to).unwrap_or_else(|error| panic!("copy {path}: {error}"));
        } else if to.is_file() {
            fs::remove_file(&to).unwrap_or_else(|error| panic!("remove {path}: {error}"));
        }
    }
    git_ok(clone, &["add", "-A"]);
    git_ok(
        clone,
        &["commit", "--quiet", "--allow-empty", "-m", "working state"],
    );
    git_ok(
        clone,
        &["commit", "--quiet", "--allow-empty", "-m", "fixture base"],
    );
}

fn reset(clone: &Path) {
    git_ok(clone, &["checkout", "--quiet", "--", "."]);
    git_ok(clone, &["clean", "-fdq"]);
}

fn apply(clone: &Path, mutation: &Mutation) {
    match mutation {
        Mutation::Replace(file, from, to) => {
            let path = clone.join(file);
            let text =
                fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {file}: {error}"));
            assert!(text.contains(from), "{file} does not contain {from:?}");
            fs::write(&path, text.replace(from, to))
                .unwrap_or_else(|error| panic!("write {file}: {error}"));
        }
        Mutation::Append(file, extra) => append(clone, file, extra),
        Mutation::AppendOwned(file, extra) => append(clone, file, extra),
        Mutation::Write(file, text) => write(clone, file, text),
        Mutation::WriteOwned(file, text) => write(clone, file, text),
        Mutation::Remove(path) => {
            let path = clone.join(path);
            let result = if path.is_dir() {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_file(&path)
            };
            result.unwrap_or_else(|error| panic!("remove {}: {error}", path.display()));
        }
    }
}

fn append(clone: &Path, file: &str, extra: &str) {
    let path = clone.join(file);
    let mut text = fs::read_to_string(&path).unwrap_or_default();
    text.push_str(extra);
    fs::write(&path, text).unwrap_or_else(|error| panic!("append {file}: {error}"));
}

fn write(clone: &Path, file: &str, text: &str) {
    let path = clone.join(file);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|error| panic!("create {file}: {error}"));
    }
    fs::write(&path, text).unwrap_or_else(|error| panic!("write {file}: {error}"));
}

fn run(command: &mut Command) -> Output {
    for key in ["GITHUB_EVENT_NAME", "GITHUB_BASE_REF"] {
        command.env_remove(key);
    }
    for (key, _) in env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    command
        .output()
        .unwrap_or_else(|error| panic!("run {command:?}: {error}"))
}

fn failure_names(output: &Output) -> Vec<String> {
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let mut names: Vec<String> = text
        .lines()
        .map(strip_ansi)
        .filter_map(|line| line.trim_start().strip_prefix("[FAIL] ").map(check_name))
        .collect();
    names.sort();
    names
}

fn strip_ansi(line: &str) -> String {
    let mut plain = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        plain.push(ch);
    }
    plain
}

/// Parity key: the message up to its first `": "`, `" ("`, backtick or control character.
/// PowerShell turns backtick escapes into control characters, so message tails differ.
fn check_name(message: &str) -> String {
    let cut = [": ", " ("]
        .iter()
        .filter_map(|marker| message.find(marker))
        .chain(message.find(|ch: char| ch == '`' || ch.is_control()))
        .min()
        .unwrap_or(message.len());
    message[..cut].trim_end().to_string()
}

#[test]
fn check_name_cuts_dynamic_and_escaped_tails() {
    assert_eq!(
        check_name("Layer directory missing: support"),
        "Layer directory missing"
    );
    assert_eq!(
        check_name("Script missing platform metadata ('# platform: any|windows'): x.ps1"),
        "Script missing platform metadata"
    );
    assert_eq!(
        check_name("Git adapter must enforce bounded rename detection (\rename_limit, x)."),
        "Git adapter must enforce bounded rename detection"
    );
    assert_eq!(check_name("Rule `x` broken"), "Rule");
    assert_eq!(
        strip_ansi("\u{1b}[31;1mRoot Cargo.lock is missing.\u{1b}[0m"),
        "Root Cargo.lock is missing."
    );
}
