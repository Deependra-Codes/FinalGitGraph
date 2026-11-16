use std::path::Path;

use super::text::{has_line, read};

/// One file-level code or config contract. It applies only when `file` exists.
/// `owner` is the roadmap item whose behavior test replaces the name check (ADR 0020).
pub struct Contract {
    pub file: &'static str,
    pub require: &'static [&'static str],
    pub any_of: &'static [&'static [&'static str]],
    pub forbid: Option<&'static str>,
    pub message: &'static str,
    pub owner: &'static str,
}

const GIT: &str = "apps/desktop/src-tauri/src/adapters/git.rs";
const INDEXER: &str = "apps/desktop/src-tauri/src/application/indexer.rs";
const DB: &str = "apps/desktop/src-tauri/src/adapters/db.rs";
const PERF_BIN: &str = "apps/desktop/src-tauri/src/bin/perf-report.rs";
const CI: &str = ".github/workflows/rust-ci.yml";
// Spelled in pieces so the repository never contains the driver name the checks ban.
const DRIVER_PATH: &str = concat!(r"\b", "ru", "s", "q", "l", "ite", "::");
const DRIVER_OR_STATEMENTS: &str = concat!(
    r"\b",
    "ru",
    "s",
    "q",
    "l",
    "ite",
    "::",
    r"|\bSELECT\s+|\bINSERT\s+INTO|\bUPDATE\s+\w+\s+SET|\bDELETE\s+FROM|\bCREATE\s+TABLE"
);
const POLICY_BLOCKED: &str = "Application Control policy has blocked this file|os error 4551";

const fn require(
    file: &'static str,
    require: &'static [&'static str],
    message: &'static str,
    owner: &'static str,
) -> Contract {
    Contract {
        file,
        require,
        any_of: &[],
        forbid: None,
        message,
        owner,
    }
}

const fn forbid(
    file: &'static str,
    pattern: &'static str,
    message: &'static str,
    owner: &'static str,
) -> Contract {
    Contract {
        file,
        require: &[],
        any_of: &[],
        forbid: Some(pattern),
        message,
        owner,
    }
}

pub const CONTRACTS: [Contract; 38] = [
    forbid(
        GIT,
        r#"Command::new\("git"\)"#,
        "Git adapter hot ingestion path must be in-process (no process-spawned git CLI).",
        "ADR 0024",
    ),
    forbid(
        GIT,
        r"(^|\W)git2::",
        "Git adapter must not reintroduce git2/libgit2 APIs after gix migration.",
        "ADR 0024",
    ),
    require(
        GIT,
        &[
            r"^\s*use\s+gix::Repository;",
            r"\bwalk_commits_tape\(",
            r"\bbuild_pack_aware_walk_tape\(",
            r"\bdiff_tree_to_tree\(",
            r"\btrack_rewrites\(",
        ],
        "Git adapter must keep tape-first gix ingestion path (walk_commits_tape + build_pack_aware_walk_tape + diff_tree_to_tree + track_rewrites).",
        "ADR 0024",
    ),
    forbid(
        GIT,
        "fn run_git_command",
        "Legacy full-buffer git command helper must not be reintroduced.",
        "ADR 0024",
    ),
    require(
        GIT,
        &["stream_commits", "stream_file_changes_for_commits"],
        "Git adapter must expose streaming commit/file-change ingestion APIs.",
        "ADR 0024",
    ),
    Contract {
        file: GIT,
        require: &[
            "FILE_CHANGE_PARSE_PARALLEL_MIN_COMMITS",
            "FILE_CHANGE_PARSE_PARALLEL_CHUNK_SIZE",
        ],
        any_of: &[
            &[r"\.par_chunks\("],
            &[
                r"\.chunks\(FILE_CHANGE_PARSE_PARALLEL_CHUNK_SIZE\)",
                r"into_par_iter\(",
            ],
        ],
        forbid: None,
        message: "Git adapter must keep bounded Rayon parallel file-change compute path.",
        owner: "ADR 0024",
    },
    forbid(
        GIT,
        DRIVER_PATH,
        "Git adapter must remain DB-free (no legacy relational driver dependency in adapters/git.rs).",
        "ADR 0022",
    ),
    require(
        GIT,
        &["rename_limit", "rename_threshold", "exact_match_only"],
        "Git adapter must enforce bounded rename detection (rename_limit, rename_threshold, exact_match_only).",
        "ADR 0024",
    ),
    require(
        GIT,
        &["sync_channel", "FILE_CHANGE_PARSE_CHANNEL_CAPACITY"],
        "Git adapter must keep bounded producer/consumer channel for parallel file-change extraction.",
        "ADR 0024",
    ),
    require(
        GIT,
        &[
            "parallel_file_change_plan",
            "FILE_CHANGE_PARSE_PARALLEL_TARGET_CHUNKS_PER_WORKER",
            "FILE_CHANGE_PARSE_CHANNEL_CAPACITY_PER_WORKER",
        ],
        "Git adapter must keep adaptive-but-bounded Rayon planning (parallel_file_change_plan + bounded chunk/channel controls).",
        "ADR 0024",
    ),
    require(
        GIT,
        &[
            "diff_requires_line_stats",
            "Patch::from_diff|PATCH_FROM_DIFF_GUARDRAIL_MARKER",
            "line_counts",
            "collect_patch_line_stats",
        ],
        "Git adapter must keep two-stage selective line-count extraction (diff_requires_line_stats + gix line_counts + guardrail marker).",
        "ADR 0024",
    ),
    require(
        GIT,
        &[
            "pub struct FileDiffTelemetry",
            "diff_build",
            "rename_find",
            "delta_emit",
        ],
        "Git adapter must keep diff sub-phase telemetry contract (diff_build, rename_find, delta_emit).",
        "ADR 0025",
    ),
    require(
        GIT,
        &[
            "parse_hex_object_id",
            "object_cache_bytes_for_workers",
            "object_cache_size_if_unset",
        ],
        "Git adapter must keep SHA validation and bounded object-cache sizing guards.",
        "ADR 0024",
    ),
    require(
        GIT,
        &[
            "FileChangeExtractionMode::MetadataOnly",
            "FileChangeExtractionMode::SelectiveLineStats",
            "load_file_change_line_stats_for_targets",
        ],
        "Git adapter must enforce metadata-only hot ingestion and selective JIT line-stats enrichment path.",
        "ADR 0024",
    ),
    require(
        INDEXER,
        &[r"stream_commits\("],
        "Indexer must use streaming commit ingestion.",
        "ADR 0024",
    ),
    require(
        INDEXER,
        &["FILE_CHANGE_COMMIT_BATCH_SIZE", "FILE_CHANGE_DB_BATCH_SIZE"],
        "Indexer must keep bounded file-change batch buffers.",
        "ADR 0024",
    ),
    require(
        INDEXER,
        &[
            r"let store = open_store\(",
            r"store\.persist_commits\(",
            r"store\.persist_file_changes\(",
            r"store\.synchronize_refs\(",
            r"store\.rebuild_csr_snapshot\(",
        ],
        "Indexer must orchestrate ingestion writes through GraphStore (persist_commits, persist_file_changes, synchronize_refs, rebuild_csr_snapshot).",
        "ADR 0022",
    ),
    require(
        INDEXER,
        &[
            "type NodeId = usize",
            "struct DenseSet",
            "load_graph_snapshot_state",
            "QueryGraph::from_graph_state",
        ],
        "Indexer query hot path must keep integer-based CSR traversal state with overlay-aware hydration (NodeId, DenseSet, load_graph_snapshot_state, QueryGraph::from_graph_state).",
        "ADR 0023",
    ),
    require(
        DB,
        &[
            "use lmdb::",
            "pub struct GraphStore",
            "begin_rw_txn",
            r"txn\.commit\(\)\?;",
        ],
        "DB adapter must keep LMDB GraphStore transaction contract (lmdb, begin_rw_txn, txn.commit).",
        "ADR 0022",
    ),
    require(
        DB,
        &[
            "pub struct CsrSnapshot",
            "parents_offsets",
            "parents_neighbors",
            "children_offsets",
            "children_neighbors",
            "rebuild_csr_snapshot",
        ],
        "DB adapter must keep persisted CSR snapshot contract (CsrSnapshot + rebuild_csr_snapshot).",
        "ADR 0022",
    ),
    require(
        DB,
        &[
            "GraphSnapshotMetadata",
            "graph_snapshot_metadata",
            "load_graph_snapshot_state",
        ],
        "DB adapter must keep explicit graph snapshot metadata/versioning contract.",
        "ADR 0022",
    ),
    require(
        DB,
        &["delta_log", "append_delta_event", "META_NEXT_DELTA_SEQUENCE"],
        "DB adapter must keep append-only delta mutation log contract.",
        "S5",
    ),
    require(
        DB,
        &[
            "graph_overlay_parent_rows",
            "graph_overlay_child_rows",
            "persist_graph_overlay_commits",
            "compact_graph_overlay_into_snapshot",
            "should_compact_graph_overlay",
        ],
        "DB adapter must keep bounded graph overlay adjacency plus compaction contract.",
        "ADR 0022",
    ),
    forbid(
        DB,
        DRIVER_OR_STATEMENTS,
        "DB adapter must remain LMDB-only (no legacy relational imports or relational statements).",
        "ADR 0022",
    ),
    require(
        DB,
        &[
            "fn delete_file_changes_for_commits",
            "repository_commit_file_change_prefix",
            "delete_prefixed_entries",
        ],
        "DB adapter must keep prefix-set cleanup path for incremental file-change deletion.",
        "ADR 0022",
    ),
    require(
        DB,
        &[
            "FileChangeStatsState",
            "enrichment_queue",
            "mark_file_change_enrichment_failed",
            "FILE_CHANGE_ENRICHMENT_MAX_ATTEMPTS",
        ],
        "DB adapter must enforce stats-state + retryable enrichment queue contract.",
        "ADR 0024",
    ),
    require(
        DB,
        &[
            "EnvironmentFlags",
            "resolve_lmdb_runtime_options",
            "LmdbRuntimeOptions",
            "effective_map_size_bytes",
            "GITGRAPH_LMDB_NO_READAHEAD",
            "GITGRAPH_LMDB_WRITE_MAP",
            "GITGRAPH_LMDB_MAP_ASYNC",
            "GITGRAPH_LMDB_NO_MEM_INIT",
        ],
        "DB adapter must keep centralized LMDB mmap/runtime flag parsing and map-size flooring (LmdbRuntimeOptions + validated env flags + effective_map_size_bytes).",
        "ADR 0022",
    ),
    require(
        DB,
        &[
            "commit_window_index",
            "load_commit_window",
            "for_each_prefixed_cursor_from",
            "MDB_SET_RANGE",
        ],
        "DB adapter must keep commit-window hot path on index/range-seek reads.",
        "ADR 0022",
    ),
    require(
        DB,
        &[
            "enrichment_due_index",
            "load_pending_file_change_enrichment_internal",
            "parse_due_index_identity_from_prefixed_key",
        ],
        "DB adapter must keep due-index ordered enrichment acquisition path.",
        "ADR 0024",
    ),
    require(
        "scripts/perf-budget.ps1",
        &[
            r"\[switch\]\$ProbeOnly",
            r"function\s+Invoke-PerfBudgetDocker",
            POLICY_BLOCKED,
            r"Host build phase duration \(s\):",
            r"Host execute phase duration \(s\):",
            "--no-run",
        ],
        "Performance budget script must keep probe mode, resilient fallback, and build/execute timing transparency contract.",
        "S2 Phase 4",
    ),
    require(
        "scripts/perf-report.ps1",
        &[
            r"\[switch\]\$ProbeOnly",
            r"function\s+Invoke-PerfReportDocker",
            POLICY_BLOCKED,
            r"Host build phase duration \(s\):",
            r"Host execute phase duration \(s\):",
        ],
        "Performance report script must keep probe mode, resilient fallback, and build/execute timing transparency contract.",
        "S2 Phase 4",
    ),
    require(
        "scripts/perf-ingestion-budget.ps1",
        &[
            r"\[switch\]\$ProbeOnly",
            r"\$Profile",
            r"\$Lane",
            r#""90k""#,
            r"perf-report\.ps1",
            "MaxFullIndexMs",
            "MaxIncrementalIndexMs",
            "MaxPeakRssBytes",
            "MaxDiffShareRatio",
            "peak_observed_rss",
            "core_end_to_end",
            "fixture_generation",
            "WarmFixture",
        ],
        "Ingestion budget script must keep profile/lane/probe modes and enforce full/incremental/ms + peak RSS + diff-share + warm fixture thresholds.",
        "S2 Phase 4",
    ),
    require(
        PERF_BIN,
        &[
            "phase_timing_ms",
            "phase_memory_bytes",
            "fixture_generation",
            "core_end_to_end",
            "peak_observed_rss",
            "in_memory_graph_hydration",
            "in_memory_adjacency_query_latency_ms",
            "jit_file_change_enrichment",
            "file_change_stats_state_counts",
            "ingest_commit_walk_ms",
            "ingest_file_diff_extract_ms",
            "ingest_file_change_upsert_ms",
            "ingest_tx_commit_ms",
            "full_commit_throughput_per_sec",
            "incremental_commit_throughput_per_sec",
            "aggregate_commit_throughput_per_sec",
            "full_commit_bottleneck_phase",
            "incremental_commit_bottleneck_phase",
            "aggregate_commit_bottleneck_phase",
        ],
        "Performance report binary must emit phase timing + memory visibility contract, including in-memory adjacency, JIT/stats-state, ingestion-phase fields, and commit-throughput bottleneck telemetry.",
        "ADR 0025",
    ),
    require(
        CI,
        &["Performance Budget"],
        "Rust CI workflow must include a Performance Budget step.",
        "ADR 0025",
    ),
    require(
        CI,
        &["Script Contract Probe"],
        "Rust CI workflow must include a Script Contract Probe step.",
        "S2 Phase 4",
    ),
    require(
        "package.json",
        &[r#""quality:perf""#],
        "package.json must include a quality:perf script.",
        "S2 Phase 4",
    ),
    require(
        "package.json",
        &[r#""quality:perf:ingestion""#],
        "package.json must include a quality:perf:ingestion script.",
        "S2 Phase 4",
    ),
    require(
        "package.json",
        &[
            r#""perf:report""#,
            r#""perf:report:smoke""#,
            r#""perf:report:large""#,
            r#""perf:report:25k""#,
            r#""perf:report:25k:precondition""#,
            r#""perf:report:90k""#,
            r#""perf:ingestion:25k""#,
            r#""perf:ingestion:90k""#,
            r#""perf:ingestion:25k:warm""#,
            r#""perf:ingestion:90k:warm""#,
            r#""test:layers""#,
        ],
        "package.json must include perf:report, perf:report:smoke, perf:report:large, perf:report:25k, perf:report:25k:precondition, perf:report:90k, perf:ingestion:25k, perf:ingestion:90k, perf:ingestion:25k:warm, perf:ingestion:90k:warm, and test:layers scripts.",
        "S2 Phase 4",
    ),
];

pub fn check(repo: &Path, failures: &mut Vec<String>) -> Result<(), String> {
    for contract in &CONTRACTS {
        let Some(text) = read(&repo.join(contract.file)) else {
            continue;
        };
        let broken = match contract.forbid {
            Some(pattern) => has_line(&text, pattern)?,
            None => {
                let mut any_group = contract.any_of.is_empty();
                for group in contract.any_of {
                    if all_present(&text, group)? {
                        any_group = true;
                        break;
                    }
                }
                !all_present(&text, contract.require)? || !any_group
            }
        };
        if broken {
            failures.push(format!("{} (owner {})", contract.message, contract.owner));
        }
    }
    Ok(())
}

fn all_present(text: &str, patterns: &[&str]) -> Result<bool, String> {
    for pattern in patterns {
        if !has_line(text, pattern)? {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::{CONTRACTS, DRIVER_OR_STATEMENTS};
    use crate::guardrails::text::compile;

    #[test]
    fn every_contract_pattern_compiles() {
        for contract in &CONTRACTS {
            let patterns = contract
                .require
                .iter()
                .chain(contract.any_of.iter().flat_map(|group| group.iter()))
                .chain(contract.forbid.iter());
            for pattern in patterns {
                assert!(compile(pattern).is_ok(), "{pattern}");
            }
        }
    }

    #[test]
    fn statement_pattern_matches_case_insensitively_like_select_string() {
        let regex = compile(DRIVER_OR_STATEMENTS).expect("compile");
        assert!(regex.is_match("// delete from commits"));
        assert!(!regex.is_match("deleted_from_commits"));
    }
}
