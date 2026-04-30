use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gitgraph_lab_core::adapters::db::open_store;
use gitgraph_lab_core::adapters::git::validate_repository;
use gitgraph_lab_core::application::commands::{
    drain_file_change_extraction_queue, ensure_file_change_stats_for_commits, get_commit_window,
    get_graph_traversal, get_shortest_path, index_commits_with_mode, scan_repository,
};
use gitgraph_lab_core::application::indexer::{InMemoryAdjacencyQueryLatencyMs, Indexer};
use gitgraph_lab_core::domain::models::{GraphDirection, GraphTraversalStrategy, IndexingMode};
use memory_stats::memory_stats;

#[cfg(feature = "perf-mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

type PerfResult<T> = Result<T, String>;

#[derive(Debug, Clone)]
struct Config {
    cycles: usize,
    feature_commits: usize,
    incremental_commits: usize,
    query_runs: usize,
    window_limit: usize,
    max_nodes: usize,
    output_path: Option<PathBuf>,
    keep_fixture: bool,
    precondition_commit_graph: bool,
    fixture_root: Option<PathBuf>,
    reuse_fixture: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cycles: 120,
            feature_commits: 4,
            incremental_commits: 40,
            query_runs: 25,
            window_limit: 200,
            max_nodes: 2_000,
            output_path: None,
            keep_fixture: false,
            precondition_commit_graph: false,
            fixture_root: None,
            reuse_fixture: false,
        }
    }
}

#[derive(Default)]
struct MemoryTracker {
    peak_observed_rss_bytes: u64,
}

impl MemoryTracker {
    fn sample_rss(&mut self) -> Option<u64> {
        let rss_bytes = current_rss_bytes()?;
        if rss_bytes > self.peak_observed_rss_bytes {
            self.peak_observed_rss_bytes = rss_bytes;
        }
        Some(rss_bytes)
    }

    fn peak_observed_rss(&self) -> Option<u64> {
        if self.peak_observed_rss_bytes == 0 {
            None
        } else {
            Some(self.peak_observed_rss_bytes)
        }
    }
}

#[derive(Debug, Clone)]
struct FixtureSummary {
    root_sha: String,
    pre_merge_main_sha: String,
    feature_tip_sha: String,
    head_sha: String,
}

#[derive(Debug, Clone)]
struct FixtureBuildTelemetry {
    generated_commits: usize,
    stream_build: Duration,
    apply_duration: Duration,
    used_fast_import: bool,
}

impl Default for FixtureBuildTelemetry {
    fn default() -> Self {
        Self {
            generated_commits: 0,
            stream_build: Duration::ZERO,
            apply_duration: Duration::ZERO,
            used_fast_import: false,
        }
    }
}

#[derive(Debug, Clone)]
enum FastImportParent<'a> {
    Mark(u64),
    Revision(&'a str),
}

struct ReportInput<'a> {
    fixture_root: &'a Path,
    fixture_before_incremental: &'a FixtureSummary,
    fixture_after_incremental: &'a FixtureSummary,
    config: &'a Config,
    counts: (i64, i64, i64),
    stats_state_counts: &'a gitgraph_lab_core::domain::models::FileChangeStatsStateCounts,
    extraction_queue: &'a gitgraph_lab_core::domain::models::FileChangeExtractionQueueMetrics,
    full_stats: &'a gitgraph_lab_core::domain::models::IndexingStats,
    incremental_stats: &'a gitgraph_lab_core::domain::models::IndexingStats,
    fixture_generation: Duration,
    full_index: Duration,
    incremental_index: Duration,
    query_benchmark: Duration,
    commit_window_jit_hydrated_rows: usize,
    in_memory_graph_hydration: Duration,
    in_memory_query_benchmark: Duration,
    in_memory_adjacency_query_latency_ms: InMemoryAdjacencyQueryLatencyMs,
    file_change_extraction_drain: Duration,
    jit_file_change_enrichment: Duration,
    fixture_build_telemetry: FixtureBuildTelemetry,
    incremental_append_telemetry: FixtureBuildTelemetry,
    core_end_to_end: Duration,
    fixture_generation_rss: Option<u64>,
    scan_repository_rss: Option<u64>,
    full_index_rss: Option<u64>,
    incremental_append_rss: Option<u64>,
    incremental_index_rss: Option<u64>,
    file_change_extraction_drain_rss: Option<u64>,
    query_benchmark_rss: Option<u64>,
    jit_file_change_enrichment_rss: Option<u64>,
    in_memory_query_rss: Option<u64>,
    final_rss: Option<u64>,
    peak_rss: Option<u64>,
    commit_throughput_per_sec: f64,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("perf-report failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> PerfResult<()> {
    let config = parse_config()?;
    let fixture_root = if let Some(root) = &config.fixture_root {
        fs::create_dir_all(root)
            .map_err(|error| format!("create fixture root {}: {error}", root.display()))?;
        root.clone()
    } else {
        create_fixture_root()?
    };
    let repo_dir = fixture_root.join("repo");
    let db_path = fixture_root.join("gitgraph-lab-perf.lmdb");

    let result = run_with_fixture(&config, &fixture_root, &repo_dir, &db_path);
    if !config.keep_fixture && config.fixture_root.is_none() {
        let _ = fs::remove_dir_all(&fixture_root);
    }
    result
}

fn run_with_fixture(
    config: &Config,
    fixture_root: &Path,
    repo_dir: &Path,
    db_path: &Path,
) -> PerfResult<()> {
    log_line(format!(
        "perf-report: start cycles={} feature_commits={} incremental_commits={} query_runs={} window_limit={} max_nodes={} fixture_root={}",
        config.cycles,
        config.feature_commits,
        config.incremental_commits,
        config.query_runs,
        config.window_limit,
        config.max_nodes,
        fixture_root.display()
    ));
    let core_started = Instant::now();
    let mut memory_tracker = MemoryTracker::default();
    let _ = memory_tracker.sample_rss();

    remove_database_files(db_path)?;

    let fixture_generation_started = Instant::now();
    let fixture_build_telemetry = if config.reuse_fixture {
        reset_fixture_to_head(repo_dir)?;
        FixtureBuildTelemetry::default()
    } else {
        generate_branching_fixture(repo_dir, config.cycles, config.feature_commits)?
    };
    let fixture_generation = fixture_generation_started.elapsed();
    log_phase("fixture_generation", fixture_generation);
    let fixture_generation_rss = memory_tracker.sample_rss();

    let fixture = collect_fixture_summary(repo_dir)?;

    if config.precondition_commit_graph {
        log_line("perf-report: preconditioning commit-graph with changed-paths");
        run_repository_preconditioning(repo_dir)?;
    }

    let repo_str = path_to_string(repo_dir);
    let db_str = path_to_string(db_path);

    let scan_started = Instant::now();
    scan_repository(repo_str.as_str(), Some(db_str.as_str())).map_err(|error| error.to_string())?;
    let _repository_scan = scan_started.elapsed();
    log_phase("scan_repository", _repository_scan);
    let scan_repository_rss = memory_tracker.sample_rss();

    let full_started = Instant::now();
    let full_stats =
        index_commits_with_mode(repo_str.as_str(), Some(db_str.as_str()), IndexingMode::Full)
            .map_err(|error| error.to_string())?;
    let full_index = full_started.elapsed();
    log_phase("full_index", full_index);
    let full_index_rss = memory_tracker.sample_rss();

    let incremental_fixture_started = Instant::now();
    let incremental_append_telemetry =
        append_incremental_commits(repo_dir, config.incremental_commits)?;
    let fixture_after_incremental = collect_fixture_summary(repo_dir)?;
    let _incremental_fixture_append = incremental_fixture_started.elapsed();
    log_phase("append_incremental_commits", _incremental_fixture_append);
    let incremental_append_rss = memory_tracker.sample_rss();

    let incremental_started = Instant::now();
    let incremental_stats = index_commits_with_mode(
        repo_str.as_str(),
        Some(db_str.as_str()),
        IndexingMode::Incremental,
    )
    .map_err(|error| error.to_string())?;
    let incremental_index = incremental_started.elapsed();
    log_phase("incremental_index", incremental_index);
    let incremental_index_rss = memory_tracker.sample_rss();

    let extraction_drain_started = Instant::now();
    let _extraction_drain =
        drain_file_change_extraction_queue(repo_str.as_str(), Some(db_str.as_str()), None)
            .map_err(|error| error.to_string())?;
    let file_change_extraction_drain = extraction_drain_started.elapsed();
    log_phase("file_change_extraction_drain", file_change_extraction_drain);
    let file_change_extraction_drain_rss = memory_tracker.sample_rss();

    let query_started = Instant::now();
    let total_query_runs = config.query_runs.max(1);
    let query_log_stride = (total_query_runs / 5).max(1);
    let mut commit_window_jit_hydrated_rows = 0usize;
    for run_index in 0..total_query_runs {
        let commit_window = get_commit_window(
            repo_str.as_str(),
            Some(db_str.as_str()),
            Some(config.window_limit),
            None,
        )
        .map_err(|error| error.to_string())?;
        commit_window_jit_hydrated_rows =
            commit_window_jit_hydrated_rows.saturating_add(commit_window.jit_hydrated_rows);
        let _ = get_graph_traversal(
            repo_str.as_str(),
            Some(db_str.as_str()),
            fixture_after_incremental.head_sha.as_str(),
            GraphTraversalStrategy::Bfs,
            GraphDirection::Parents,
            Some(config.max_nodes),
        )
        .map_err(|error| error.to_string())?;
        let _ = get_graph_traversal(
            repo_str.as_str(),
            Some(db_str.as_str()),
            fixture_after_incremental.head_sha.as_str(),
            GraphTraversalStrategy::Dfs,
            GraphDirection::Parents,
            Some(config.max_nodes),
        )
        .map_err(|error| error.to_string())?;
        let _ = get_graph_traversal(
            repo_str.as_str(),
            Some(db_str.as_str()),
            fixture_after_incremental.head_sha.as_str(),
            GraphTraversalStrategy::Topological,
            GraphDirection::Parents,
            Some(config.max_nodes),
        )
        .map_err(|error| error.to_string())?;
        let _ = get_shortest_path(
            repo_str.as_str(),
            Some(db_str.as_str()),
            fixture_after_incremental.head_sha.as_str(),
            fixture_after_incremental.root_sha.as_str(),
            GraphDirection::Parents,
        )
        .map_err(|error| error.to_string())?;
        let _ = get_shortest_path(
            repo_str.as_str(),
            Some(db_str.as_str()),
            fixture_after_incremental.head_sha.as_str(),
            fixture_after_incremental.pre_merge_main_sha.as_str(),
            GraphDirection::Both,
        )
        .map_err(|error| error.to_string())?;

        if (run_index + 1) % query_log_stride == 0 || run_index + 1 == total_query_runs {
            log_line(format!(
                "perf-report: query_benchmark progress {}/{}",
                run_index + 1,
                total_query_runs
            ));
        }
    }
    let query_benchmark = query_started.elapsed();
    log_phase("query_benchmark", query_benchmark);
    let query_benchmark_rss = memory_tracker.sample_rss();

    let enrich_started = Instant::now();
    let _ = ensure_file_change_stats_for_commits(
        repo_str.as_str(),
        Some(db_str.as_str()),
        std::slice::from_ref(&fixture_after_incremental.head_sha),
        Some(512),
    )
    .map_err(|error| error.to_string())?;
    let jit_file_change_enrichment = enrich_started.elapsed();
    log_phase("jit_file_change_enrichment", jit_file_change_enrichment);
    let jit_file_change_enrichment_rss = memory_tracker.sample_rss();

    let in_memory_indexer = Indexer::new(db_path.to_path_buf());
    let in_memory_benchmark = in_memory_indexer
        .benchmark_in_memory_queries(
            repo_dir,
            fixture_after_incremental.head_sha.as_str(),
            fixture_after_incremental.root_sha.as_str(),
            fixture_after_incremental.pre_merge_main_sha.as_str(),
            config.query_runs,
            config.max_nodes,
        )
        .map_err(|error| error.to_string())?;
    let in_memory_graph_hydration = in_memory_benchmark.hydration;
    let in_memory_query_benchmark = in_memory_benchmark.query_benchmark;
    let in_memory_adjacency_query_latency_ms = in_memory_benchmark.adjacency_latency_ms;
    log_phase("in_memory_graph_hydration", in_memory_graph_hydration);
    log_phase("in_memory_query_benchmark", in_memory_query_benchmark);
    let in_memory_query_rss = memory_tracker.sample_rss();

    let store = open_store(db_path).map_err(|error| error.to_string())?;
    let repo_id = if let Some(existing_repo_id) = store
        .load_repository_id_by_path(repo_str.as_str())
        .map_err(|error| error.to_string())?
    {
        existing_repo_id
    } else {
        let canonical_repo = validate_repository(repo_dir).map_err(|error| error.to_string())?;
        store
            .upsert_repository(
                &canonical_repo.root.display().to_string(),
                &canonical_repo.name,
                &canonical_repo.git_dir.display().to_string(),
            )
            .map_err(|error| error.to_string())?
    };
    let counts = store
        .load_dataset_counts(repo_id)
        .map_err(|error| error.to_string())?;
    let stats_state_counts = store
        .load_file_change_stats_state_counts(repo_id)
        .map_err(|error| error.to_string())?;
    let extraction_queue = store
        .load_file_change_extraction_queue_metrics(repo_id)
        .map_err(|error| error.to_string())?;

    let core_end_to_end = core_started.elapsed();
    log_phase("core_end_to_end", core_end_to_end);
    let final_rss = memory_tracker.sample_rss();
    let peak_rss = memory_tracker.peak_observed_rss();

    let commit_throughput_per_sec = full_stats.commit_throughput.commits_per_sec_indexed;
    log_line(format!(
        "perf-report: commit throughput full_cps={:.3} incremental_cps={:.3} full_bottleneck={}({:.3} ms, share {:.3}) incremental_bottleneck={}({:.3} ms, share {:.3})",
        full_stats.commit_throughput.commits_per_sec_indexed,
        incremental_stats.commit_throughput.commits_per_sec_indexed,
        full_stats.commit_throughput.bottleneck_phase,
        full_stats.commit_throughput.bottleneck_phase_ms,
        full_stats.commit_throughput.bottleneck_phase_share_of_tracked_time,
        incremental_stats.commit_throughput.bottleneck_phase,
        incremental_stats.commit_throughput.bottleneck_phase_ms,
        incremental_stats.commit_throughput.bottleneck_phase_share_of_tracked_time,
    ));

    let report_input = ReportInput {
        fixture_root,
        fixture_before_incremental: &fixture,
        fixture_after_incremental: &fixture_after_incremental,
        config,
        counts,
        stats_state_counts: &stats_state_counts,
        extraction_queue: &extraction_queue,
        full_stats: &full_stats,
        incremental_stats: &incremental_stats,
        fixture_generation,
        full_index,
        incremental_index,
        query_benchmark,
        commit_window_jit_hydrated_rows,
        in_memory_graph_hydration,
        in_memory_query_benchmark,
        in_memory_adjacency_query_latency_ms,
        file_change_extraction_drain,
        jit_file_change_enrichment,
        fixture_build_telemetry,
        incremental_append_telemetry,
        core_end_to_end,
        fixture_generation_rss,
        scan_repository_rss,
        full_index_rss,
        incremental_append_rss,
        incremental_index_rss,
        file_change_extraction_drain_rss,
        query_benchmark_rss,
        jit_file_change_enrichment_rss,
        in_memory_query_rss,
        final_rss,
        peak_rss,
        commit_throughput_per_sec,
    };
    let report = format_report(&report_input);

    if let Some(output_path) = &config.output_path {
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!("create report directory {}: {error}", parent.display())
            })?;
        }
        fs::write(output_path, report.as_bytes())
            .map_err(|error| format!("write report {}: {error}", output_path.display()))?;
        log_line(format!("report_written_to: {}", output_path.display()));
    } else {
        log_line(report);
    }

    log_line(format!(
        "summary: commits={} edges={} file_changes={} full_index_ms={:.3} incremental_index_ms={:.3}",
        counts.0,
        counts.1,
        counts.2,
        duration_ms(full_index),
        duration_ms(incremental_index)
    ));

    Ok(())
}

fn parse_config() -> PerfResult<Config> {
    let mut config = Config::default();
    let args: Vec<String> = env::args().skip(1).collect();
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "--cycles" => {
                index += 1;
                config.cycles = parse_usize_arg(&args, index, "--cycles")?;
            }
            "--feature-commits" => {
                index += 1;
                config.feature_commits = parse_usize_arg(&args, index, "--feature-commits")?;
            }
            "--incremental-commits" => {
                index += 1;
                config.incremental_commits =
                    parse_usize_arg(&args, index, "--incremental-commits")?;
            }
            "--query-runs" => {
                index += 1;
                config.query_runs = parse_usize_arg(&args, index, "--query-runs")?;
            }
            "--window-limit" => {
                index += 1;
                config.window_limit = parse_usize_arg(&args, index, "--window-limit")?;
            }
            "--max-nodes" => {
                index += 1;
                config.max_nodes = parse_usize_arg(&args, index, "--max-nodes")?;
            }
            "--output" => {
                index += 1;
                config.output_path =
                    Some(PathBuf::from(parse_string_arg(&args, index, "--output")?));
            }
            "--fixture-root" => {
                index += 1;
                config.fixture_root = Some(PathBuf::from(parse_string_arg(
                    &args,
                    index,
                    "--fixture-root",
                )?));
            }
            "--keep-fixture" => {
                config.keep_fixture = true;
            }
            "--reuse-fixture" => {
                config.reuse_fixture = true;
            }
            "--precondition-commit-graph" => {
                config.precondition_commit_graph = true;
            }
            unknown => {
                return Err(format!("unknown argument: {unknown}"));
            }
        }
        index += 1;
    }

    Ok(config)
}

fn parse_usize_arg(args: &[String], index: usize, flag: &str) -> PerfResult<usize> {
    let raw = parse_string_arg(args, index, flag)?;
    raw.parse::<usize>()
        .map_err(|error| format!("parse {flag}={raw} as usize: {error}"))
}

fn parse_string_arg<'a>(args: &'a [String], index: usize, flag: &str) -> PerfResult<&'a str> {
    args.get(index)
        .map(String::as_str)
        .ok_or_else(|| format!("missing value for {flag}"))
}

fn create_fixture_root() -> PerfResult<PathBuf> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("read system time: {error}"))?
        .as_millis();

    let base_temp_root = env::var_os("GITGRAPH_PERF_TEMP_ROOT")
        .map(PathBuf::from)
        .or(preferred_windows_perf_temp_root())
        .unwrap_or_else(env::temp_dir);

    fs::create_dir_all(&base_temp_root).map_err(|error| {
        format!(
            "create perf base temp root {}: {error}",
            base_temp_root.display()
        )
    })?;

    let root = base_temp_root.join(format!("gitgraph-lab-perf-{millis}"));
    fs::create_dir_all(&root)
        .map_err(|error| format!("create fixture root {}: {error}", root.display()))?;
    Ok(root)
}

#[cfg(target_os = "windows")]
fn preferred_windows_perf_temp_root() -> Option<PathBuf> {
    if Path::new(r"D:\").exists() {
        return Some(PathBuf::from(r"D:\temp\gitgraph-lab"));
    }
    None
}

#[cfg(not(target_os = "windows"))]
fn preferred_windows_perf_temp_root() -> Option<PathBuf> {
    None
}

fn remove_database_files(db_path: &Path) -> PerfResult<()> {
    if db_path.exists() {
        fs::remove_dir_all(db_path)
            .map_err(|error| format!("remove existing db path {}: {error}", db_path.display()))?;
    }
    let compat = PathBuf::from(format!("{}.lmdb", db_path.to_string_lossy()));
    if compat.exists() {
        fs::remove_dir_all(&compat).map_err(|error| {
            format!(
                "remove existing db compat path {}: {error}",
                compat.display()
            )
        })?;
    }
    Ok(())
}

fn generate_branching_fixture(
    repo_dir: &Path,
    cycles: usize,
    feature_commits: usize,
) -> PerfResult<FixtureBuildTelemetry> {
    if repo_dir.exists() {
        fs::remove_dir_all(repo_dir).map_err(|error| {
            format!(
                "remove existing fixture repo {}: {error}",
                repo_dir.display()
            )
        })?;
    }
    fs::create_dir_all(repo_dir)
        .map_err(|error| format!("create fixture repo {}: {error}", repo_dir.display()))?;

    run_git(repo_dir, &["init"])?;
    run_git(repo_dir, &["config", "user.name", "GitGraph Lab"])?;
    run_git(
        repo_dir,
        &["config", "user.email", "gitgraph-lab@example.com"],
    )?;
    run_git(repo_dir, &["config", "commit.gpgsign", "false"])?;
    run_git(repo_dir, &["config", "tag.gpgsign", "false"])?;
    run_git(repo_dir, &["config", "core.hooksPath", ".git/hooks"])?;
    run_git(repo_dir, &["config", "gc.auto", "0"])?;
    run_git(repo_dir, &["config", "maintenance.auto", "false"])?;

    let stream_build_started = Instant::now();
    let (fast_import_stream, generated_commits) =
        build_branching_fixture_fast_import_stream(cycles, feature_commits);
    let stream_build = stream_build_started.elapsed();
    log_phase("fixture_fast_import_stream_build", stream_build);
    log_line(format!(
        "perf-report: fixture fast-import stream commits={} bytes={}",
        generated_commits,
        fast_import_stream.len()
    ));

    let apply_started = Instant::now();
    let used_fast_import = match run_git_fast_import(repo_dir, fast_import_stream.as_str()) {
        Ok(()) => true,
        Err(error) => {
            log_line(format!(
                "perf-report: fixture fast-import unavailable, falling back to git CLI flow ({error})"
            ));
            generate_branching_fixture_legacy(repo_dir, cycles, feature_commits)?;
            false
        }
    };
    let apply_duration = apply_started.elapsed();
    log_phase("fixture_apply", apply_duration);

    Ok(FixtureBuildTelemetry {
        generated_commits,
        stream_build,
        apply_duration,
        used_fast_import,
    })
}

fn branching_fixture_commit_count(cycles: usize, feature_commits: usize) -> usize {
    let merge_commits_per_cycle = if feature_commits > 0 { 1 } else { 0 };
    1 + cycles.saturating_mul(1 + feature_commits + merge_commits_per_cycle)
}

fn append_fast_import_data_block(stream: &mut String, payload: &str) {
    stream.push_str(&format!("data {}\n", payload.len()));
    stream.push_str(payload);
    stream.push('\n');
}

fn append_fast_import_commit_header(
    stream: &mut String,
    reference: &str,
    mark: u64,
    message: &str,
    timestamp: u64,
    from: Option<FastImportParent<'_>>,
    merge: Option<u64>,
) {
    stream.push_str("commit ");
    stream.push_str(reference);
    stream.push('\n');
    stream.push_str(&format!("mark :{mark}\n"));
    stream.push_str(&format!(
        "author GitGraph Lab <gitgraph-lab@example.com> {timestamp} +0000\n"
    ));
    stream.push_str(&format!(
        "committer GitGraph Lab <gitgraph-lab@example.com> {timestamp} +0000\n"
    ));
    append_fast_import_data_block(stream, message);

    if let Some(parent) = from {
        match parent {
            FastImportParent::Mark(parent_mark) => {
                stream.push_str(&format!("from :{parent_mark}\n"));
            }
            FastImportParent::Revision(revision) => {
                stream.push_str("from ");
                stream.push_str(revision);
                stream.push('\n');
            }
        }
    }

    if let Some(merge_mark) = merge {
        stream.push_str(&format!("merge :{merge_mark}\n"));
    }
}

fn append_fast_import_inline_file(stream: &mut String, path: &str, body: &str) {
    stream.push_str("M 100644 inline ");
    stream.push_str(path);
    stream.push('\n');
    append_fast_import_data_block(stream, body);
}

fn build_branching_fixture_fast_import_stream(
    cycles: usize,
    feature_commits: usize,
) -> (String, usize) {
    let mut stream = String::with_capacity(
        branching_fixture_commit_count(cycles, feature_commits).saturating_mul(256),
    );
    let mut next_mark: u64 = 1;
    let mut timestamp: u64 = 1_700_000_000;

    let seed_mark = next_mark;
    next_mark += 1;
    append_fast_import_commit_header(
        &mut stream,
        "refs/heads/master",
        seed_mark,
        "seed-root",
        timestamp,
        None,
        None,
    );
    append_fast_import_inline_file(&mut stream, "README.md", "gitgraph perf fixture\n");
    stream.push('\n');
    timestamp += 1;

    let mut master_tip_mark = seed_mark;
    for cycle in 0..cycles {
        let main_mark = next_mark;
        next_mark += 1;
        append_fast_import_commit_header(
            &mut stream,
            "refs/heads/master",
            main_mark,
            format!("main-cycle-{cycle}").as_str(),
            timestamp,
            Some(FastImportParent::Mark(master_tip_mark)),
            None,
        );
        append_fast_import_inline_file(
            &mut stream,
            "main.txt",
            format!("cycle={cycle}\n").as_str(),
        );
        stream.push('\n');
        timestamp += 1;

        let mut feature_tip_mark = main_mark;
        for feature_index in 0..feature_commits {
            let feature_mark = next_mark;
            next_mark += 1;
            let feature_parent = if feature_index == 0 {
                main_mark
            } else {
                feature_tip_mark
            };
            append_fast_import_commit_header(
                &mut stream,
                "refs/heads/feature-temp",
                feature_mark,
                format!("feature-cycle-{cycle}-{feature_index}").as_str(),
                timestamp,
                Some(FastImportParent::Mark(feature_parent)),
                None,
            );
            append_fast_import_inline_file(
                &mut stream,
                "feature.txt",
                format!("cycle={cycle} feature={feature_index}\n").as_str(),
            );
            stream.push('\n');
            timestamp += 1;
            feature_tip_mark = feature_mark;
        }

        if feature_commits > 0 {
            let merge_mark = next_mark;
            next_mark += 1;
            append_fast_import_commit_header(
                &mut stream,
                "refs/heads/master",
                merge_mark,
                format!("merge-cycle-{cycle}").as_str(),
                timestamp,
                Some(FastImportParent::Mark(main_mark)),
                Some(feature_tip_mark),
            );
            append_fast_import_inline_file(
                &mut stream,
                "feature.txt",
                format!("cycle={cycle} feature={}\n", feature_commits - 1).as_str(),
            );
            stream.push('\n');
            timestamp += 1;
            master_tip_mark = merge_mark;
        } else {
            master_tip_mark = main_mark;
        }
    }

    stream.push_str("done\n");
    (
        stream,
        branching_fixture_commit_count(cycles, feature_commits),
    )
}

fn build_incremental_fast_import_stream(count: usize, base_head_revision: &str) -> (String, usize) {
    let mut stream = String::with_capacity(count.saturating_mul(192) + 16);
    let mut next_mark: u64 = 1;
    let mut previous_mark: Option<u64> = None;
    let mut timestamp: u64 = 1_800_000_000;

    for index in 0..count {
        let mark = next_mark;
        next_mark += 1;
        let parent = if let Some(parent_mark) = previous_mark {
            Some(FastImportParent::Mark(parent_mark))
        } else {
            Some(FastImportParent::Revision(base_head_revision))
        };

        append_fast_import_commit_header(
            &mut stream,
            "refs/heads/master",
            mark,
            format!("incremental-{index}").as_str(),
            timestamp,
            parent,
            None,
        );
        append_fast_import_inline_file(
            &mut stream,
            "incremental.txt",
            format!("incremental={index}\n").as_str(),
        );
        stream.push('\n');
        timestamp += 1;
        previous_mark = Some(mark);
    }

    stream.push_str("done\n");
    (stream, count)
}

fn run_git_fast_import(repo_dir: &Path, stream: &str) -> PerfResult<()> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo_dir)
        .args(["fast-import", "--quiet"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("execute git fast-import: {error}"))?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| "git fast-import stdin unavailable".to_string())?;
        stdin
            .write_all(stream.as_bytes())
            .map_err(|error| format!("write git fast-import stream: {error}"))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|error| format!("wait for git fast-import: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("git fast-import failed: {stderr}"));
    }
    Ok(())
}

fn generate_branching_fixture_legacy(
    repo_dir: &Path,
    cycles: usize,
    feature_commits: usize,
) -> PerfResult<()> {
    if repo_dir.exists() {
        fs::remove_dir_all(repo_dir).map_err(|error| {
            format!(
                "remove existing fixture repo {}: {error}",
                repo_dir.display()
            )
        })?;
    }
    fs::create_dir_all(repo_dir)
        .map_err(|error| format!("create fixture repo {}: {error}", repo_dir.display()))?;

    run_git(repo_dir, &["init"])?;
    run_git(repo_dir, &["config", "user.name", "GitGraph Lab"])?;
    run_git(
        repo_dir,
        &["config", "user.email", "gitgraph-lab@example.com"],
    )?;
    run_git(repo_dir, &["config", "commit.gpgsign", "false"])?;
    run_git(repo_dir, &["config", "tag.gpgsign", "false"])?;
    run_git(repo_dir, &["config", "core.hooksPath", ".git/hooks"])?;
    run_git(repo_dir, &["config", "gc.auto", "0"])?;
    run_git(repo_dir, &["config", "maintenance.auto", "false"])?;

    write_repo_file(repo_dir, "README.md", "gitgraph perf fixture\n")?;
    run_git(repo_dir, &["add", "."])?;
    run_git(repo_dir, &["commit", "--no-verify", "-m", "seed-root"])?;

    let progress_stride = progress_stride(cycles);
    let progress_started = Instant::now();
    for cycle in 0..cycles {
        write_repo_file(repo_dir, "main.txt", format!("cycle={cycle}\n").as_str())?;
        run_git(repo_dir, &["add", "main.txt"])?;
        run_git(
            repo_dir,
            &[
                "commit",
                "--no-verify",
                "-m",
                &format!("main-cycle-{cycle}"),
            ],
        )?;

        run_git(repo_dir, &["checkout", "-B", "feature-temp"])?;
        for feature_index in 0..feature_commits {
            write_repo_file(
                repo_dir,
                "feature.txt",
                format!("cycle={cycle} feature={feature_index}\n").as_str(),
            )?;
            run_git(repo_dir, &["add", "feature.txt"])?;
            run_git(
                repo_dir,
                &[
                    "commit",
                    "--no-verify",
                    "-m",
                    &format!("feature-cycle-{cycle}-{feature_index}"),
                ],
            )?;
        }

        run_git(repo_dir, &["checkout", "master"])?;
        run_git(
            repo_dir,
            &[
                "merge",
                "--no-ff",
                "--no-verify",
                "feature-temp",
                "-m",
                &format!("merge-cycle-{cycle}"),
            ],
        )?;

        if (cycle + 1) % progress_stride == 0 || cycle + 1 == cycles {
            log_progress(
                "fixture_generation_legacy",
                cycle + 1,
                cycles,
                progress_started,
            );
        }
    }

    Ok(())
}

fn reset_fixture_to_head(repo_dir: &Path) -> PerfResult<()> {
    if !repo_dir.exists() {
        return Err(format!(
            "reuse fixture requested but repository is missing: {}",
            repo_dir.display()
        ));
    }
    run_git(repo_dir, &["reset", "--hard", "HEAD"])?;
    run_git(repo_dir, &["clean", "-fd"])?;
    Ok(())
}

fn append_incremental_commits(repo_dir: &Path, count: usize) -> PerfResult<FixtureBuildTelemetry> {
    if count == 0 {
        return Ok(FixtureBuildTelemetry::default());
    }

    let stream_build_started = Instant::now();
    let base_head_sha = run_git_capture(repo_dir, &["rev-parse", "HEAD"])?;
    let (fast_import_stream, generated_commits) =
        build_incremental_fast_import_stream(count, base_head_sha.as_str());
    let stream_build = stream_build_started.elapsed();
    log_phase("incremental_fast_import_stream_build", stream_build);
    log_line(format!(
        "perf-report: incremental fast-import stream commits={} bytes={}",
        generated_commits,
        fast_import_stream.len()
    ));

    let apply_started = Instant::now();
    let used_fast_import = match run_git_fast_import(repo_dir, fast_import_stream.as_str()) {
        Ok(()) => true,
        Err(error) => {
            log_line(format!(
                "perf-report: incremental fast-import unavailable, falling back to git CLI flow ({error})"
            ));
            append_incremental_commits_legacy(repo_dir, count)?;
            false
        }
    };
    let apply_duration = apply_started.elapsed();
    log_phase("incremental_apply", apply_duration);

    Ok(FixtureBuildTelemetry {
        generated_commits,
        stream_build,
        apply_duration,
        used_fast_import,
    })
}

fn append_incremental_commits_legacy(repo_dir: &Path, count: usize) -> PerfResult<()> {
    let progress_stride = progress_stride(count);
    let progress_started = Instant::now();
    for index in 0..count {
        write_repo_file(
            repo_dir,
            "incremental.txt",
            format!("incremental={index}\n").as_str(),
        )?;
        run_git(repo_dir, &["add", "incremental.txt"])?;
        run_git(
            repo_dir,
            &[
                "commit",
                "--no-verify",
                "-m",
                &format!("incremental-{index}"),
            ],
        )?;

        if (index + 1) % progress_stride == 0 || index + 1 == count {
            log_progress(
                "incremental_append_legacy",
                index + 1,
                count,
                progress_started,
            );
        }
    }
    Ok(())
}

fn collect_fixture_summary(repo_dir: &Path) -> PerfResult<FixtureSummary> {
    let head_sha = run_git_capture(repo_dir, &["rev-parse", "HEAD"])?;
    let root_sha = run_git_capture(repo_dir, &["rev-list", "--max-parents=0", "HEAD"])?;
    let pre_merge_main_sha =
        run_git_capture(repo_dir, &["rev-parse", "HEAD^1"]).unwrap_or_else(|_| head_sha.clone());
    let feature_tip_sha =
        run_git_capture(repo_dir, &["rev-parse", "HEAD^2"]).unwrap_or_else(|_| head_sha.clone());

    Ok(FixtureSummary {
        root_sha,
        pre_merge_main_sha,
        feature_tip_sha,
        head_sha,
    })
}

fn run_repository_preconditioning(repo_dir: &Path) -> PerfResult<()> {
    run_git(
        repo_dir,
        &["commit-graph", "write", "--reachable", "--changed-paths"],
    )
}

fn run_git(repo_dir: &Path, args: &[&str]) -> PerfResult<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_dir)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("execute git {:?}: {error}", args))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("git {:?} failed: {stderr}", args));
    }
    Ok(())
}

fn run_git_capture(repo_dir: &Path, args: &[&str]) -> PerfResult<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_dir)
        .args(args)
        .output()
        .map_err(|error| format!("execute git {:?}: {error}", args))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("git {:?} failed: {stderr}", args));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn write_repo_file(repo_dir: &Path, relative_path: &str, body: &str) -> PerfResult<()> {
    let path = repo_dir.join(relative_path);
    fs::write(&path, body).map_err(|error| format!("write {}: {error}", path.display()))
}

fn format_report(input: &ReportInput<'_>) -> String {
    let full = &input.full_stats.commit_throughput;
    let incremental = &input.incremental_stats.commit_throughput;

    let aggregate_commits_seen = full.commits_seen.saturating_add(incremental.commits_seen);
    let aggregate_commits_indexed = full
        .commits_indexed
        .saturating_add(incremental.commits_indexed);
    let aggregate_end_to_end_ms = full.end_to_end_ms + incremental.end_to_end_ms;

    let aggregate_commit_walk_total_ms =
        full.commit_walk_total_ms + incremental.commit_walk_total_ms;
    let aggregate_commit_callback_ms = full.commit_callback_ms + incremental.commit_callback_ms;
    let aggregate_commit_walk_decode_ms =
        full.commit_walk_decode_ms + incremental.commit_walk_decode_ms;
    let aggregate_commit_walk_parent_resolve_ms =
        full.commit_walk_parent_resolve_ms + incremental.commit_walk_parent_resolve_ms;
    let aggregate_commit_walk_metadata_parse_ms =
        full.commit_walk_metadata_parse_ms + incremental.commit_walk_metadata_parse_ms;
    let aggregate_commit_walk_tip_collection_ms =
        full.commit_walk_tip_collection_ms + incremental.commit_walk_tip_collection_ms;
    let aggregate_commit_walk_hide_expansion_ms =
        full.commit_walk_hide_expansion_ms + incremental.commit_walk_hide_expansion_ms;
    let aggregate_commit_walk_graph_lookup_ms =
        full.commit_walk_graph_lookup_ms + incremental.commit_walk_graph_lookup_ms;
    let aggregate_commit_walk_ordering_ms =
        full.commit_walk_ordering_ms + incremental.commit_walk_ordering_ms;
    let aggregate_commit_walk_emit_ms = full.commit_walk_emit_ms + incremental.commit_walk_emit_ms;
    let aggregate_commit_walk_pack_inflate_ms =
        full.commit_walk_pack_inflate_ms + incremental.commit_walk_pack_inflate_ms;
    let aggregate_commit_walk_pack_base_resolve_ms =
        full.commit_walk_pack_base_resolve_ms + incremental.commit_walk_pack_base_resolve_ms;
    let aggregate_commit_walk_pack_parse_ms =
        full.commit_walk_pack_parse_ms + incremental.commit_walk_pack_parse_ms;
    let aggregate_commit_walk_tape_build_ms =
        full.commit_walk_tape_build_ms + incremental.commit_walk_tape_build_ms;
    let aggregate_commit_walk_graph_lookup_hit_count = full
        .commit_walk_graph_lookup_hit_count
        .saturating_add(incremental.commit_walk_graph_lookup_hit_count);
    let aggregate_commit_walk_graph_lookup_miss_count = full
        .commit_walk_graph_lookup_miss_count
        .saturating_add(incremental.commit_walk_graph_lookup_miss_count);
    let aggregate_commit_walk_graph_assist_hit_count = full
        .commit_walk_graph_assist_hit_count
        .saturating_add(incremental.commit_walk_graph_assist_hit_count);
    let aggregate_commit_walk_graph_assist_miss_count = full
        .commit_walk_graph_assist_miss_count
        .saturating_add(incremental.commit_walk_graph_assist_miss_count);
    let aggregate_commit_walk_nodes_seen_count = full
        .commit_walk_nodes_seen_count
        .saturating_add(incremental.commit_walk_nodes_seen_count);
    let aggregate_commit_walk_nodes_pruned_count = full
        .commit_walk_nodes_pruned_count
        .saturating_add(incremental.commit_walk_nodes_pruned_count);
    let aggregate_commit_walk_queue_max_depth = full
        .commit_walk_queue_max_depth
        .max(incremental.commit_walk_queue_max_depth);
    let aggregate_commit_metadata_pending_count = full
        .commit_metadata_pending_count
        .saturating_add(incremental.commit_metadata_pending_count);
    let aggregate_commit_walk_engine = if full.commit_walk_engine == incremental.commit_walk_engine
    {
        full.commit_walk_engine.as_str()
    } else {
        "mixed"
    };
    let aggregate_commit_walk_boundary_mode =
        if full.commit_walk_boundary_mode == incremental.commit_walk_boundary_mode {
            full.commit_walk_boundary_mode.as_str()
        } else {
            "mixed"
        };
    let aggregate_commit_walk_graph_missing_tip_count = full
        .commit_walk_graph_missing_tip_count
        .saturating_add(incremental.commit_walk_graph_missing_tip_count);
    let aggregate_commit_walk_boundary_stop_count = full
        .commit_walk_boundary_stop_count
        .saturating_add(incremental.commit_walk_boundary_stop_count);
    let aggregate_commit_walk_overlap_skip_count = full
        .commit_walk_overlap_skip_count
        .saturating_add(incremental.commit_walk_overlap_skip_count);
    let aggregate_commit_walk_tape_node_count = full
        .commit_walk_tape_node_count
        .saturating_add(incremental.commit_walk_tape_node_count);
    let aggregate_commit_walk_tape_parent_edges = full
        .commit_walk_tape_parent_edges
        .saturating_add(incremental.commit_walk_tape_parent_edges);
    let aggregate_commit_window_jit_hydrated_rows = full
        .commit_window_jit_hydrated_rows
        .saturating_add(incremental.commit_window_jit_hydrated_rows);
    let aggregate_query_commit_window_prewarm_hydrated_rows = full
        .query_commit_window_prewarm_hydrated_rows
        .saturating_add(incremental.query_commit_window_prewarm_hydrated_rows);
    let aggregate_incremental_csr_streaming_mode_effective = if full
        .incremental_csr_streaming_mode_effective
        == incremental.incremental_csr_streaming_mode_effective
    {
        full.incremental_csr_streaming_mode_effective.as_str()
    } else {
        "mixed"
    };
    let aggregate_commit_walk_ms = full.commit_walk_ms + incremental.commit_walk_ms;
    let aggregate_commit_upsert_ms = full.commit_upsert_ms + incremental.commit_upsert_ms;
    let aggregate_commit_write_wave_txn_count = full
        .commit_write_wave_txn_count
        .saturating_add(incremental.commit_write_wave_txn_count);
    let aggregate_commit_write_wave_batch_median = if aggregate_commit_write_wave_txn_count == 0 {
        0.0
    } else {
        ((full.commit_write_wave_batch_median * full.commit_write_wave_txn_count as f64)
            + (incremental.commit_write_wave_batch_median
                * incremental.commit_write_wave_txn_count as f64))
            / aggregate_commit_write_wave_txn_count as f64
    };
    let aggregate_commit_ingest_queue_max_depth = full
        .commit_ingest_queue_max_depth
        .max(incremental.commit_ingest_queue_max_depth);
    let aggregate_commit_ingest_queue_wait_ms =
        full.commit_ingest_queue_wait_ms + incremental.commit_ingest_queue_wait_ms;
    let aggregate_commit_csr_delta_apply_ms =
        full.commit_csr_delta_apply_ms + incremental.commit_csr_delta_apply_ms;
    let aggregate_commit_csr_overlay_upsert_ms =
        full.commit_csr_overlay_upsert_ms + incremental.commit_csr_overlay_upsert_ms;
    let aggregate_commit_csr_compaction_ms =
        full.commit_csr_compaction_ms + incremental.commit_csr_compaction_ms;
    let aggregate_file_diff_extract_ms =
        full.file_diff_extract_ms + incremental.file_diff_extract_ms;
    let aggregate_file_change_upsert_ms =
        full.file_change_upsert_ms + incremental.file_change_upsert_ms;
    let aggregate_tx_commit_ms = full.tx_commit_ms + incremental.tx_commit_ms;

    let throughput_from_ms = |commits: usize, phase_ms: f64| -> f64 {
        if commits == 0 || phase_ms <= 0.0 {
            return 0.0;
        }
        commits as f64 / (phase_ms / 1_000.0)
    };

    let aggregate_commits_per_sec_seen =
        throughput_from_ms(aggregate_commits_seen, aggregate_end_to_end_ms);
    let aggregate_commits_per_sec_indexed =
        throughput_from_ms(aggregate_commits_indexed, aggregate_end_to_end_ms);

    let aggregate_phase_candidates = [
        ("commit_walk", aggregate_commit_walk_ms),
        ("commit_upsert", aggregate_commit_upsert_ms),
        ("file_diff_extract", aggregate_file_diff_extract_ms),
        ("file_change_upsert", aggregate_file_change_upsert_ms),
        ("tx_commit", aggregate_tx_commit_ms),
    ];
    let mut aggregate_bottleneck_phase = "none";
    let mut aggregate_bottleneck_phase_ms = 0.0f64;
    let mut aggregate_tracked_phase_ms = 0.0f64;
    for (phase_name, phase_ms) in aggregate_phase_candidates {
        aggregate_tracked_phase_ms += phase_ms;
        if phase_ms > aggregate_bottleneck_phase_ms {
            aggregate_bottleneck_phase = phase_name;
            aggregate_bottleneck_phase_ms = phase_ms;
        }
    }
    let aggregate_bottleneck_phase_share = if aggregate_tracked_phase_ms <= 0.0 {
        0.0
    } else {
        aggregate_bottleneck_phase_ms / aggregate_tracked_phase_ms
    };

    let fixture_generation_ms = duration_ms(input.fixture_generation);
    let fixture_stream_build_ms = duration_ms(input.fixture_build_telemetry.stream_build);
    let fixture_apply_ms = duration_ms(input.fixture_build_telemetry.apply_duration);
    let fixture_generation_commits_per_sec = throughput_from_ms(
        input.fixture_build_telemetry.generated_commits,
        fixture_generation_ms,
    );
    let fixture_apply_commits_per_sec = throughput_from_ms(
        input.fixture_build_telemetry.generated_commits,
        fixture_apply_ms,
    );
    let fixture_backend = if input.fixture_build_telemetry.used_fast_import {
        "fast_import"
    } else {
        "legacy_git_cli"
    };

    let incremental_append_stream_build_ms =
        duration_ms(input.incremental_append_telemetry.stream_build);
    let incremental_append_apply_ms =
        duration_ms(input.incremental_append_telemetry.apply_duration);
    let incremental_append_total_ms =
        incremental_append_stream_build_ms + incremental_append_apply_ms;
    let incremental_append_commits_per_sec = throughput_from_ms(
        input.incremental_append_telemetry.generated_commits,
        incremental_append_total_ms,
    );
    let incremental_append_apply_commits_per_sec = throughput_from_ms(
        input.incremental_append_telemetry.generated_commits,
        incremental_append_apply_ms,
    );
    let incremental_append_backend = if input.incremental_append_telemetry.used_fast_import {
        "fast_import"
    } else {
        "legacy_git_cli"
    };

    let mut lines = Vec::new();
    lines.push(format!("fixture_root: {}", input.fixture_root.display()));
    lines.push(format!("fixture_generation: {:.3}", fixture_generation_ms));
    lines.push(format!(
        "fixture_generation_commit_count: {}",
        input.fixture_build_telemetry.generated_commits
    ));
    lines.push(format!("fixture_generation_backend: {fixture_backend}"));
    lines.push(format!(
        "fixture_generation_stream_build_ms: {:.3}",
        fixture_stream_build_ms
    ));
    lines.push(format!(
        "fixture_generation_apply_ms: {:.3}",
        fixture_apply_ms
    ));
    lines.push(format!(
        "fixture_generation_commits_per_sec: {:.3}",
        fixture_generation_commits_per_sec
    ));
    lines.push(format!(
        "fixture_generation_apply_commits_per_sec: {:.3}",
        fixture_apply_commits_per_sec
    ));
    lines.push(format!("full_index: {:.3}", duration_ms(input.full_index)));
    lines.push(format!(
        "incremental_index: {:.3}",
        duration_ms(input.incremental_index)
    ));
    lines.push(format!(
        "incremental_append_commit_count: {}",
        input.incremental_append_telemetry.generated_commits
    ));
    lines.push(format!(
        "incremental_append_backend: {incremental_append_backend}"
    ));
    lines.push(format!(
        "incremental_append_stream_build_ms: {:.3}",
        incremental_append_stream_build_ms
    ));
    lines.push(format!(
        "incremental_append_apply_ms: {:.3}",
        incremental_append_apply_ms
    ));
    lines.push(format!(
        "incremental_append_commits_per_sec: {:.3}",
        incremental_append_commits_per_sec
    ));
    lines.push(format!(
        "incremental_append_apply_commits_per_sec: {:.3}",
        incremental_append_apply_commits_per_sec
    ));
    lines.push(format!(
        "query_benchmark: {:.3}",
        duration_ms(input.query_benchmark)
    ));
    lines.push(format!(
        "query_commit_window_jit_hydrated_rows: {}",
        input.commit_window_jit_hydrated_rows
    ));
    lines.push(format!(
        "in_memory_graph_hydration: {:.3}",
        duration_ms(input.in_memory_graph_hydration)
    ));
    lines.push(format!(
        "in_memory_query_benchmark: {:.3}",
        duration_ms(input.in_memory_query_benchmark)
    ));
    lines.push(format!(
        "file_change_extraction_drain_ms: {:.3}",
        duration_ms(input.file_change_extraction_drain)
    ));
    lines.push(format!(
        "core_end_to_end: {:.3}",
        duration_ms(input.core_end_to_end)
    ));
    lines.push(format!(
        "peak_observed_rss: {}",
        optional_u64_display(input.peak_rss)
    ));
    lines.push(format!(
        "final_rss: {}",
        optional_u64_display(input.final_rss)
    ));
    let runtime_profile =
        env::var("GITGRAPH_RUNTIME_PROFILE").unwrap_or_else(|_| "default".to_string());
    let hardware_logical_cores = env::var("GITGRAPH_MAX_PERF_LOGICAL_CORES").unwrap_or_else(|_| {
        std::thread::available_parallelism()
            .map(|cores| cores.get().to_string())
            .unwrap_or_else(|_| "unknown".to_string())
    });
    let hardware_available_memory_bytes = env::var("GITGRAPH_MAX_PERF_AVAILABLE_MEMORY_BYTES")
        .unwrap_or_else(|_| "unknown".to_string());
    let hardware_platform =
        env::var("GITGRAPH_MAX_PERF_PLATFORM").unwrap_or_else(|_| env::consts::OS.to_string());
    let hardware_allocator = if cfg!(feature = "perf-mimalloc") {
        "mimalloc"
    } else {
        "system"
    };
    let hardware_rustflags = env::var("RUSTFLAGS").unwrap_or_else(|_| "unset".to_string());
    lines.push(format!("runtime_profile: {runtime_profile}"));
    lines.push(format!(
        "hardware_profile_logical_cores: {hardware_logical_cores}"
    ));
    lines.push(format!(
        "hardware_profile_available_memory_bytes: {hardware_available_memory_bytes}"
    ));
    lines.push(format!("hardware_profile_platform: {hardware_platform}"));
    lines.push(format!("hardware_profile_allocator: {hardware_allocator}"));
    lines.push(format!("hardware_profile_rustflags: {hardware_rustflags}"));
    lines.push(format!(
        "commit_throughput_per_sec: {:.3}",
        input.commit_throughput_per_sec
    ));
    lines.push(format!(
        "full_commit_throughput_per_sec: {:.3}",
        full.commits_per_sec_indexed
    ));
    lines.push(format!(
        "full_commit_throughput_seen_per_sec: {:.3}",
        full.commits_per_sec_seen
    ));
    lines.push(format!(
        "incremental_commit_throughput_per_sec: {:.3}",
        incremental.commits_per_sec_indexed
    ));
    lines.push(format!(
        "incremental_commit_throughput_seen_per_sec: {:.3}",
        incremental.commits_per_sec_seen
    ));
    lines.push(format!(
        "aggregate_commit_throughput_per_sec: {:.3}",
        aggregate_commits_per_sec_indexed
    ));
    lines.push(format!(
        "aggregate_commit_throughput_seen_per_sec: {:.3}",
        aggregate_commits_per_sec_seen
    ));
    lines.push(format!(
        "full_commit_ms_per_commit: {:.3}",
        full.ms_per_commit_seen
    ));
    lines.push(format!(
        "incremental_commit_ms_per_commit: {:.3}",
        incremental.ms_per_commit_seen
    ));
    lines.push(format!(
        "full_planner_session_id: {}",
        input.full_stats.planner_session_id
    ));
    lines.push(format!(
        "incremental_planner_session_id: {}",
        input.incremental_stats.planner_session_id
    ));
    lines.push(format!(
        "full_planner_requested_mode: {}",
        input.full_stats.planner_requested_mode.as_str()
    ));
    lines.push(format!(
        "incremental_planner_requested_mode: {}",
        input.incremental_stats.planner_requested_mode.as_str()
    ));
    lines.push(format!(
        "full_planner_effective_mode: {}",
        input.full_stats.planner_effective_mode.as_str()
    ));
    lines.push(format!(
        "incremental_planner_effective_mode: {}",
        input.incremental_stats.planner_effective_mode.as_str()
    ));
    lines.push(format!(
        "full_planner_checkpoint_trusted: {}",
        input.full_stats.planner_checkpoint_trusted
    ));
    lines.push(format!(
        "incremental_planner_checkpoint_trusted: {}",
        input.incremental_stats.planner_checkpoint_trusted
    ));
    lines.push(format!(
        "full_planner_risk_level: {}",
        input.full_stats.planner_risk_level.as_str()
    ));
    lines.push(format!(
        "incremental_planner_risk_level: {}",
        input.incremental_stats.planner_risk_level.as_str()
    ));
    lines.push(format!(
        "full_planner_degraded_policy: {}",
        input.full_stats.planner_degraded_policy.as_str()
    ));
    lines.push(format!(
        "incremental_planner_degraded_policy: {}",
        input.incremental_stats.planner_degraded_policy.as_str()
    ));
    lines.push(format!(
        "full_planner_trace_count: {}",
        input.full_stats.planner_trace_count
    ));
    lines.push(format!(
        "incremental_planner_trace_count: {}",
        input.incremental_stats.planner_trace_count
    ));
    lines.push(format!(
        "aggregate_planner_trace_count: {}",
        input
            .full_stats
            .planner_trace_count
            .saturating_add(input.incremental_stats.planner_trace_count)
    ));
    lines.push(format!(
        "full_lmdb_effective_map_size_bytes: {}",
        input.full_stats.lmdb_effective_map_size_bytes
    ));
    lines.push(format!(
        "incremental_lmdb_effective_map_size_bytes: {}",
        input.incremental_stats.lmdb_effective_map_size_bytes
    ));
    lines.push(format!(
        "full_lmdb_runtime_flags: no_readahead={} write_map={} map_async={} no_mem_init={}",
        input.full_stats.lmdb_no_readahead,
        input.full_stats.lmdb_write_map,
        input.full_stats.lmdb_map_async,
        input.full_stats.lmdb_no_mem_init
    ));
    lines.push(format!(
        "incremental_lmdb_runtime_flags: no_readahead={} write_map={} map_async={} no_mem_init={}",
        input.incremental_stats.lmdb_no_readahead,
        input.incremental_stats.lmdb_write_map,
        input.incremental_stats.lmdb_map_async,
        input.incremental_stats.lmdb_no_mem_init
    ));
    lines.push(format!(
        "full_commit_walk_scope: {}",
        full.commit_walk_scope
    ));
    lines.push(format!(
        "incremental_commit_walk_scope: {}",
        incremental.commit_walk_scope
    ));
    lines.push(format!(
        "full_incremental_csr_streaming_mode_effective: {}",
        full.incremental_csr_streaming_mode_effective
    ));
    lines.push(format!(
        "incremental_incremental_csr_streaming_mode_effective: {}",
        incremental.incremental_csr_streaming_mode_effective
    ));
    lines.push(format!(
        "aggregate_incremental_csr_streaming_mode_effective: {}",
        aggregate_incremental_csr_streaming_mode_effective
    ));
    lines.push(format!(
        "full_commit_walk_engine: {}",
        full.commit_walk_engine
    ));
    lines.push(format!(
        "incremental_commit_walk_engine: {}",
        incremental.commit_walk_engine
    ));
    lines.push(format!(
        "aggregate_commit_walk_engine: {}",
        aggregate_commit_walk_engine
    ));
    lines.push(format!(
        "full_commit_walk_sort_effective: {}",
        full.commit_walk_sort_effective
    ));
    lines.push(format!(
        "incremental_commit_walk_sort_effective: {}",
        incremental.commit_walk_sort_effective
    ));
    lines.push(format!(
        "full_commit_walk_boundary_mode: {}",
        full.commit_walk_boundary_mode
    ));
    lines.push(format!(
        "incremental_commit_walk_boundary_mode: {}",
        incremental.commit_walk_boundary_mode
    ));
    lines.push(format!(
        "aggregate_commit_walk_boundary_mode: {}",
        aggregate_commit_walk_boundary_mode
    ));
    lines.push(format!(
        "full_commit_walk_tip_count: {}",
        full.commit_walk_tip_count
    ));
    lines.push(format!(
        "incremental_commit_walk_tip_count: {}",
        incremental.commit_walk_tip_count
    ));
    lines.push(format!(
        "aggregate_commit_walk_tip_count: {}",
        full.commit_walk_tip_count
            .saturating_add(incremental.commit_walk_tip_count)
    ));
    lines.push(format!(
        "full_commit_walk_pruned_parent_expansions: {}",
        full.commit_walk_pruned_parent_expansions
    ));
    lines.push(format!(
        "incremental_commit_walk_pruned_parent_expansions: {}",
        incremental.commit_walk_pruned_parent_expansions
    ));
    lines.push(format!(
        "aggregate_commit_walk_pruned_parent_expansions: {}",
        full.commit_walk_pruned_parent_expansions
            .saturating_add(incremental.commit_walk_pruned_parent_expansions)
    ));
    lines.push(format!(
        "full_commit_walk_total_ms: {:.3}",
        full.commit_walk_total_ms
    ));
    lines.push(format!(
        "incremental_commit_walk_total_ms: {:.3}",
        incremental.commit_walk_total_ms
    ));
    lines.push(format!(
        "aggregate_commit_walk_total_ms: {:.3}",
        aggregate_commit_walk_total_ms
    ));
    lines.push(format!(
        "full_commit_callback_ms: {:.3}",
        full.commit_callback_ms
    ));
    lines.push(format!(
        "incremental_commit_callback_ms: {:.3}",
        incremental.commit_callback_ms
    ));
    lines.push(format!(
        "aggregate_commit_callback_ms: {:.3}",
        aggregate_commit_callback_ms
    ));
    lines.push(format!(
        "full_commit_walk_decode_ms: {:.3}",
        full.commit_walk_decode_ms
    ));
    lines.push(format!(
        "incremental_commit_walk_decode_ms: {:.3}",
        incremental.commit_walk_decode_ms
    ));
    lines.push(format!(
        "aggregate_commit_walk_decode_ms: {:.3}",
        aggregate_commit_walk_decode_ms
    ));
    lines.push(format!(
        "full_commit_walk_parent_resolve_ms: {:.3}",
        full.commit_walk_parent_resolve_ms
    ));
    lines.push(format!(
        "incremental_commit_walk_parent_resolve_ms: {:.3}",
        incremental.commit_walk_parent_resolve_ms
    ));
    lines.push(format!(
        "aggregate_commit_walk_parent_resolve_ms: {:.3}",
        aggregate_commit_walk_parent_resolve_ms
    ));
    lines.push(format!(
        "full_commit_walk_metadata_parse_ms: {:.3}",
        full.commit_walk_metadata_parse_ms
    ));
    lines.push(format!(
        "incremental_commit_walk_metadata_parse_ms: {:.3}",
        incremental.commit_walk_metadata_parse_ms
    ));
    lines.push(format!(
        "aggregate_commit_walk_metadata_parse_ms: {:.3}",
        aggregate_commit_walk_metadata_parse_ms
    ));
    lines.push(format!(
        "full_commit_walk_tip_collection_ms: {:.3}",
        full.commit_walk_tip_collection_ms
    ));
    lines.push(format!(
        "incremental_commit_walk_tip_collection_ms: {:.3}",
        incremental.commit_walk_tip_collection_ms
    ));
    lines.push(format!(
        "aggregate_commit_walk_tip_collection_ms: {:.3}",
        aggregate_commit_walk_tip_collection_ms
    ));
    lines.push(format!(
        "full_commit_walk_hide_expansion_ms: {:.3}",
        full.commit_walk_hide_expansion_ms
    ));
    lines.push(format!(
        "incremental_commit_walk_hide_expansion_ms: {:.3}",
        incremental.commit_walk_hide_expansion_ms
    ));
    lines.push(format!(
        "aggregate_commit_walk_hide_expansion_ms: {:.3}",
        aggregate_commit_walk_hide_expansion_ms
    ));
    lines.push(format!(
        "full_commit_walk_graph_lookup_ms: {:.3}",
        full.commit_walk_graph_lookup_ms
    ));
    lines.push(format!(
        "incremental_commit_walk_graph_lookup_ms: {:.3}",
        incremental.commit_walk_graph_lookup_ms
    ));
    lines.push(format!(
        "aggregate_commit_walk_graph_lookup_ms: {:.3}",
        aggregate_commit_walk_graph_lookup_ms
    ));
    lines.push(format!(
        "full_commit_walk_ordering_ms: {:.3}",
        full.commit_walk_ordering_ms
    ));
    lines.push(format!(
        "incremental_commit_walk_ordering_ms: {:.3}",
        incremental.commit_walk_ordering_ms
    ));
    lines.push(format!(
        "aggregate_commit_walk_ordering_ms: {:.3}",
        aggregate_commit_walk_ordering_ms
    ));
    lines.push(format!(
        "full_commit_walk_emit_ms: {:.3}",
        full.commit_walk_emit_ms
    ));
    lines.push(format!(
        "incremental_commit_walk_emit_ms: {:.3}",
        incremental.commit_walk_emit_ms
    ));
    lines.push(format!(
        "aggregate_commit_walk_emit_ms: {:.3}",
        aggregate_commit_walk_emit_ms
    ));
    lines.push(format!(
        "full_commit_walk_pack_inflate_ms: {:.3}",
        full.commit_walk_pack_inflate_ms
    ));
    lines.push(format!(
        "incremental_commit_walk_pack_inflate_ms: {:.3}",
        incremental.commit_walk_pack_inflate_ms
    ));
    lines.push(format!(
        "aggregate_commit_walk_pack_inflate_ms: {:.3}",
        aggregate_commit_walk_pack_inflate_ms
    ));
    lines.push(format!(
        "full_commit_walk_pack_base_resolve_ms: {:.3}",
        full.commit_walk_pack_base_resolve_ms
    ));
    lines.push(format!(
        "incremental_commit_walk_pack_base_resolve_ms: {:.3}",
        incremental.commit_walk_pack_base_resolve_ms
    ));
    lines.push(format!(
        "aggregate_commit_walk_pack_base_resolve_ms: {:.3}",
        aggregate_commit_walk_pack_base_resolve_ms
    ));
    lines.push(format!(
        "full_commit_walk_pack_parse_ms: {:.3}",
        full.commit_walk_pack_parse_ms
    ));
    lines.push(format!(
        "incremental_commit_walk_pack_parse_ms: {:.3}",
        incremental.commit_walk_pack_parse_ms
    ));
    lines.push(format!(
        "aggregate_commit_walk_pack_parse_ms: {:.3}",
        aggregate_commit_walk_pack_parse_ms
    ));
    lines.push(format!(
        "full_commit_walk_tape_build_ms: {:.3}",
        full.commit_walk_tape_build_ms
    ));
    lines.push(format!(
        "incremental_commit_walk_tape_build_ms: {:.3}",
        incremental.commit_walk_tape_build_ms
    ));
    lines.push(format!(
        "aggregate_commit_walk_tape_build_ms: {:.3}",
        aggregate_commit_walk_tape_build_ms
    ));
    lines.push(format!(
        "full_commit_walk_graph_lookup_hit_count: {}",
        full.commit_walk_graph_lookup_hit_count
    ));
    lines.push(format!(
        "incremental_commit_walk_graph_lookup_hit_count: {}",
        incremental.commit_walk_graph_lookup_hit_count
    ));
    lines.push(format!(
        "aggregate_commit_walk_graph_lookup_hit_count: {}",
        aggregate_commit_walk_graph_lookup_hit_count
    ));
    lines.push(format!(
        "full_commit_walk_graph_lookup_miss_count: {}",
        full.commit_walk_graph_lookup_miss_count
    ));
    lines.push(format!(
        "incremental_commit_walk_graph_lookup_miss_count: {}",
        incremental.commit_walk_graph_lookup_miss_count
    ));
    lines.push(format!(
        "aggregate_commit_walk_graph_lookup_miss_count: {}",
        aggregate_commit_walk_graph_lookup_miss_count
    ));
    lines.push(format!(
        "full_commit_walk_graph_assist_hit_count: {}",
        full.commit_walk_graph_assist_hit_count
    ));
    lines.push(format!(
        "incremental_commit_walk_graph_assist_hit_count: {}",
        incremental.commit_walk_graph_assist_hit_count
    ));
    lines.push(format!(
        "aggregate_commit_walk_graph_assist_hit_count: {}",
        aggregate_commit_walk_graph_assist_hit_count
    ));
    lines.push(format!(
        "full_commit_walk_graph_assist_miss_count: {}",
        full.commit_walk_graph_assist_miss_count
    ));
    lines.push(format!(
        "incremental_commit_walk_graph_assist_miss_count: {}",
        incremental.commit_walk_graph_assist_miss_count
    ));
    lines.push(format!(
        "aggregate_commit_walk_graph_assist_miss_count: {}",
        aggregate_commit_walk_graph_assist_miss_count
    ));
    lines.push(format!(
        "full_commit_walk_nodes_seen_count: {}",
        full.commit_walk_nodes_seen_count
    ));
    lines.push(format!(
        "incremental_commit_walk_nodes_seen_count: {}",
        incremental.commit_walk_nodes_seen_count
    ));
    lines.push(format!(
        "aggregate_commit_walk_nodes_seen_count: {}",
        aggregate_commit_walk_nodes_seen_count
    ));
    lines.push(format!(
        "full_commit_walk_nodes_pruned_count: {}",
        full.commit_walk_nodes_pruned_count
    ));
    lines.push(format!(
        "incremental_commit_walk_nodes_pruned_count: {}",
        incremental.commit_walk_nodes_pruned_count
    ));
    lines.push(format!(
        "aggregate_commit_walk_nodes_pruned_count: {}",
        aggregate_commit_walk_nodes_pruned_count
    ));
    lines.push(format!(
        "full_commit_walk_queue_max_depth: {}",
        full.commit_walk_queue_max_depth
    ));
    lines.push(format!(
        "incremental_commit_walk_queue_max_depth: {}",
        incremental.commit_walk_queue_max_depth
    ));
    lines.push(format!(
        "aggregate_commit_walk_queue_max_depth: {}",
        aggregate_commit_walk_queue_max_depth
    ));
    lines.push(format!(
        "full_commit_metadata_pending_count: {}",
        full.commit_metadata_pending_count
    ));
    lines.push(format!(
        "incremental_commit_metadata_pending_count: {}",
        incremental.commit_metadata_pending_count
    ));
    lines.push(format!(
        "aggregate_commit_metadata_pending_count: {}",
        aggregate_commit_metadata_pending_count
    ));
    lines.push(format!(
        "full_commit_walk_graph_missing_tip_count: {}",
        full.commit_walk_graph_missing_tip_count
    ));
    lines.push(format!(
        "incremental_commit_walk_graph_missing_tip_count: {}",
        incremental.commit_walk_graph_missing_tip_count
    ));
    lines.push(format!(
        "aggregate_commit_walk_graph_missing_tip_count: {}",
        aggregate_commit_walk_graph_missing_tip_count
    ));
    lines.push(format!(
        "full_commit_walk_boundary_stop_count: {}",
        full.commit_walk_boundary_stop_count
    ));
    lines.push(format!(
        "incremental_commit_walk_boundary_stop_count: {}",
        incremental.commit_walk_boundary_stop_count
    ));
    lines.push(format!(
        "aggregate_commit_walk_boundary_stop_count: {}",
        aggregate_commit_walk_boundary_stop_count
    ));
    lines.push(format!(
        "full_commit_walk_overlap_skip_count: {}",
        full.commit_walk_overlap_skip_count
    ));
    lines.push(format!(
        "incremental_commit_walk_overlap_skip_count: {}",
        incremental.commit_walk_overlap_skip_count
    ));
    lines.push(format!(
        "aggregate_commit_walk_overlap_skip_count: {}",
        aggregate_commit_walk_overlap_skip_count
    ));
    lines.push(format!(
        "full_commit_walk_tape_node_count: {}",
        full.commit_walk_tape_node_count
    ));
    lines.push(format!(
        "incremental_commit_walk_tape_node_count: {}",
        incremental.commit_walk_tape_node_count
    ));
    lines.push(format!(
        "aggregate_commit_walk_tape_node_count: {}",
        aggregate_commit_walk_tape_node_count
    ));
    lines.push(format!(
        "full_commit_walk_tape_parent_edges: {}",
        full.commit_walk_tape_parent_edges
    ));
    lines.push(format!(
        "incremental_commit_walk_tape_parent_edges: {}",
        incremental.commit_walk_tape_parent_edges
    ));
    lines.push(format!(
        "aggregate_commit_walk_tape_parent_edges: {}",
        aggregate_commit_walk_tape_parent_edges
    ));
    lines.push(format!(
        "full_commit_window_jit_hydrated_rows: {}",
        full.commit_window_jit_hydrated_rows
    ));
    lines.push(format!(
        "incremental_commit_window_jit_hydrated_rows: {}",
        incremental.commit_window_jit_hydrated_rows
    ));
    lines.push(format!(
        "aggregate_commit_window_jit_hydrated_rows: {}",
        aggregate_commit_window_jit_hydrated_rows
    ));
    lines.push(format!(
        "full_query_commit_window_prewarm_hydrated_rows: {}",
        full.query_commit_window_prewarm_hydrated_rows
    ));
    lines.push(format!(
        "incremental_query_commit_window_prewarm_hydrated_rows: {}",
        incremental.query_commit_window_prewarm_hydrated_rows
    ));
    lines.push(format!(
        "aggregate_query_commit_window_prewarm_hydrated_rows: {}",
        aggregate_query_commit_window_prewarm_hydrated_rows
    ));
    lines.push(format!(
        "full_commit_write_wave_txn_count: {}",
        full.commit_write_wave_txn_count
    ));
    lines.push(format!(
        "incremental_commit_write_wave_txn_count: {}",
        incremental.commit_write_wave_txn_count
    ));
    lines.push(format!(
        "aggregate_commit_write_wave_txn_count: {}",
        aggregate_commit_write_wave_txn_count
    ));
    lines.push(format!(
        "full_commit_write_wave_batch_median: {:.3}",
        full.commit_write_wave_batch_median
    ));
    lines.push(format!(
        "incremental_commit_write_wave_batch_median: {:.3}",
        incremental.commit_write_wave_batch_median
    ));
    lines.push(format!(
        "aggregate_commit_write_wave_batch_median: {:.3}",
        aggregate_commit_write_wave_batch_median
    ));
    lines.push(format!(
        "full_commit_ingest_queue_max_depth: {}",
        full.commit_ingest_queue_max_depth
    ));
    lines.push(format!(
        "incremental_commit_ingest_queue_max_depth: {}",
        incremental.commit_ingest_queue_max_depth
    ));
    lines.push(format!(
        "aggregate_commit_ingest_queue_max_depth: {}",
        aggregate_commit_ingest_queue_max_depth
    ));
    lines.push(format!(
        "full_commit_ingest_queue_wait_ms: {:.3}",
        full.commit_ingest_queue_wait_ms
    ));
    lines.push(format!(
        "incremental_commit_ingest_queue_wait_ms: {:.3}",
        incremental.commit_ingest_queue_wait_ms
    ));
    lines.push(format!(
        "aggregate_commit_ingest_queue_wait_ms: {:.3}",
        aggregate_commit_ingest_queue_wait_ms
    ));
    lines.push(format!(
        "full_commit_csr_delta_apply_ms: {:.3}",
        full.commit_csr_delta_apply_ms
    ));
    lines.push(format!(
        "incremental_commit_csr_delta_apply_ms: {:.3}",
        incremental.commit_csr_delta_apply_ms
    ));
    lines.push(format!(
        "aggregate_commit_csr_delta_apply_ms: {:.3}",
        aggregate_commit_csr_delta_apply_ms
    ));
    lines.push(format!(
        "full_commit_csr_overlay_upsert_ms: {:.3}",
        full.commit_csr_overlay_upsert_ms
    ));
    lines.push(format!(
        "incremental_commit_csr_overlay_upsert_ms: {:.3}",
        incremental.commit_csr_overlay_upsert_ms
    ));
    lines.push(format!(
        "aggregate_commit_csr_overlay_upsert_ms: {:.3}",
        aggregate_commit_csr_overlay_upsert_ms
    ));
    lines.push(format!(
        "full_commit_csr_compaction_ms: {:.3}",
        full.commit_csr_compaction_ms
    ));
    lines.push(format!(
        "incremental_commit_csr_compaction_ms: {:.3}",
        incremental.commit_csr_compaction_ms
    ));
    lines.push(format!(
        "aggregate_commit_csr_compaction_ms: {:.3}",
        aggregate_commit_csr_compaction_ms
    ));
    lines.push(format!(
        "full_commit_walk_commits_per_sec: {:.3}",
        full.commit_walk_commits_per_sec_seen
    ));
    lines.push(format!(
        "full_commit_upsert_commits_per_sec: {:.3}",
        full.commit_upsert_commits_per_sec_seen
    ));
    lines.push(format!(
        "full_file_diff_extract_commits_per_sec: {:.3}",
        full.file_diff_extract_commits_per_sec_seen
    ));
    lines.push(format!(
        "full_file_change_upsert_commits_per_sec: {:.3}",
        full.file_change_upsert_commits_per_sec_seen
    ));
    lines.push(format!(
        "full_tx_commit_commits_per_sec: {:.3}",
        full.tx_commit_commits_per_sec_seen
    ));
    lines.push(format!(
        "incremental_commit_walk_commits_per_sec: {:.3}",
        incremental.commit_walk_commits_per_sec_seen
    ));
    lines.push(format!(
        "incremental_commit_upsert_commits_per_sec: {:.3}",
        incremental.commit_upsert_commits_per_sec_seen
    ));
    lines.push(format!(
        "incremental_file_diff_extract_commits_per_sec: {:.3}",
        incremental.file_diff_extract_commits_per_sec_seen
    ));
    lines.push(format!(
        "incremental_file_change_upsert_commits_per_sec: {:.3}",
        incremental.file_change_upsert_commits_per_sec_seen
    ));
    lines.push(format!(
        "incremental_tx_commit_commits_per_sec: {:.3}",
        incremental.tx_commit_commits_per_sec_seen
    ));
    lines.push(format!(
        "aggregate_commit_walk_commits_per_sec: {:.3}",
        throughput_from_ms(aggregate_commits_seen, aggregate_commit_walk_ms)
    ));
    lines.push(format!(
        "aggregate_commit_upsert_commits_per_sec: {:.3}",
        throughput_from_ms(aggregate_commits_seen, aggregate_commit_upsert_ms)
    ));
    lines.push(format!(
        "aggregate_file_diff_extract_commits_per_sec: {:.3}",
        throughput_from_ms(aggregate_commits_seen, aggregate_file_diff_extract_ms)
    ));
    lines.push(format!(
        "aggregate_file_change_upsert_commits_per_sec: {:.3}",
        throughput_from_ms(aggregate_commits_seen, aggregate_file_change_upsert_ms)
    ));
    lines.push(format!(
        "aggregate_tx_commit_commits_per_sec: {:.3}",
        throughput_from_ms(aggregate_commits_seen, aggregate_tx_commit_ms)
    ));
    lines.push(format!(
        "full_commit_bottleneck_phase: {}",
        full.bottleneck_phase
    ));
    lines.push(format!(
        "full_commit_bottleneck_phase_ms: {:.3}",
        full.bottleneck_phase_ms
    ));
    lines.push(format!(
        "full_commit_bottleneck_phase_share: {:.3}",
        full.bottleneck_phase_share_of_tracked_time
    ));
    lines.push(format!(
        "incremental_commit_bottleneck_phase: {}",
        incremental.bottleneck_phase
    ));
    lines.push(format!(
        "incremental_commit_bottleneck_phase_ms: {:.3}",
        incremental.bottleneck_phase_ms
    ));
    lines.push(format!(
        "incremental_commit_bottleneck_phase_share: {:.3}",
        incremental.bottleneck_phase_share_of_tracked_time
    ));
    lines.push(format!(
        "aggregate_commit_bottleneck_phase: {}",
        aggregate_bottleneck_phase
    ));
    lines.push(format!(
        "aggregate_commit_bottleneck_phase_ms: {:.3}",
        aggregate_bottleneck_phase_ms
    ));
    lines.push(format!(
        "aggregate_commit_bottleneck_phase_share: {:.3}",
        aggregate_bottleneck_phase_share
    ));
    lines.push(format!(
        "oldest_file_change_extraction_queue_age_seconds: {}",
        input.extraction_queue.oldest_queue_age_seconds
    ));
    lines.push(format!(
        "full_file_change_commits_enqueued: {}",
        input.full_stats.file_change_commits_enqueued
    ));
    lines.push(format!(
        "incremental_file_change_commits_enqueued: {}",
        input.incremental_stats.file_change_commits_enqueued
    ));
    lines.push(format!(
        "full_file_change_queue_depth_after_index: {}",
        input.full_stats.file_change_queue_depth_after_index
    ));
    lines.push(format!(
        "incremental_file_change_queue_depth_after_index: {}",
        input.incremental_stats.file_change_queue_depth_after_index
    ));
    lines.push(format!(
        "full_file_change_queue_oldest_age_seconds_after_index: {}",
        input.full_stats.file_change_queue_oldest_age_seconds
    ));
    lines.push(format!(
        "incremental_file_change_queue_oldest_age_seconds_after_index: {}",
        input.incremental_stats.file_change_queue_oldest_age_seconds
    ));
    lines.push(format!(
        "ingest_commit_walk_ms: {:.3}",
        input.full_stats.ingestion_phase_timing_ms.commit_walk
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_walk
    ));
    lines.push(format!(
        "ingest_commit_walk_total_ms: {:.3}",
        input.full_stats.ingestion_phase_timing_ms.commit_walk_total
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_walk_total
    ));
    lines.push(format!(
        "ingest_commit_callback_ms: {:.3}",
        input.full_stats.ingestion_phase_timing_ms.commit_callback
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_callback
    ));
    lines.push(format!(
        "ingest_commit_walk_decode_ms: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_walk_decode
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_walk_decode
    ));
    lines.push(format!(
        "ingest_commit_walk_parent_resolve_ms: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_walk_parent_resolve
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_walk_parent_resolve
    ));
    lines.push(format!(
        "ingest_commit_walk_metadata_parse_ms: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_walk_metadata_parse
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_walk_metadata_parse
    ));
    lines.push(format!(
        "ingest_commit_walk_tip_collection_ms: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_walk_tip_collection
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_walk_tip_collection
    ));
    lines.push(format!(
        "ingest_commit_walk_hide_expansion_ms: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_walk_hide_expansion
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_walk_hide_expansion
    ));
    lines.push(format!(
        "ingest_commit_walk_graph_lookup_ms: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_walk_graph_lookup
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_walk_graph_lookup
    ));
    lines.push(format!(
        "ingest_commit_walk_ordering_ms: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_walk_ordering
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_walk_ordering
    ));
    lines.push(format!(
        "ingest_commit_walk_emit_ms: {:.3}",
        input.full_stats.ingestion_phase_timing_ms.commit_walk_emit
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_walk_emit
    ));
    lines.push(format!(
        "ingest_commit_walk_pack_inflate_ms: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_walk_pack_inflate
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_walk_pack_inflate
    ));
    lines.push(format!(
        "ingest_commit_walk_pack_base_resolve_ms: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_walk_pack_base_resolve
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_walk_pack_base_resolve
    ));
    lines.push(format!(
        "ingest_commit_walk_pack_parse_ms: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_walk_pack_parse
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_walk_pack_parse
    ));
    lines.push(format!(
        "ingest_commit_walk_tape_build_ms: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_walk_tape_build
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_walk_tape_build
    ));
    lines.push(format!(
        "ingest_commit_walk_graph_lookup_hit_count: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_walk_graph_lookup_hit_count
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_walk_graph_lookup_hit_count
    ));
    lines.push(format!(
        "ingest_commit_walk_graph_lookup_miss_count: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_walk_graph_lookup_miss_count
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_walk_graph_lookup_miss_count
    ));
    lines.push(format!(
        "ingest_commit_walk_graph_assist_hit_count: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_walk_graph_assist_hit_count
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_walk_graph_assist_hit_count
    ));
    lines.push(format!(
        "ingest_commit_walk_graph_assist_miss_count: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_walk_graph_assist_miss_count
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_walk_graph_assist_miss_count
    ));
    lines.push(format!(
        "ingest_commit_walk_nodes_seen_count: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_walk_nodes_seen_count
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_walk_nodes_seen_count
    ));
    lines.push(format!(
        "ingest_commit_walk_nodes_pruned_count: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_walk_nodes_pruned_count
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_walk_nodes_pruned_count
    ));
    lines.push(format!(
        "ingest_commit_walk_queue_max_depth: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_walk_queue_max_depth
            .max(
                input
                    .incremental_stats
                    .ingestion_phase_timing_ms
                    .commit_walk_queue_max_depth,
            )
    ));
    lines.push(format!(
        "ingest_commit_metadata_pending_count: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_metadata_pending_count
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_metadata_pending_count
    ));
    lines.push(format!(
        "ingest_commit_walk_graph_missing_tip_count: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_walk_graph_missing_tip_count
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_walk_graph_missing_tip_count
    ));
    lines.push(format!(
        "ingest_commit_walk_boundary_stop_count: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_walk_boundary_stop_count
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_walk_boundary_stop_count
    ));
    lines.push(format!(
        "ingest_commit_walk_overlap_skip_count: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_walk_overlap_skip_count
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_walk_overlap_skip_count
    ));
    lines.push(format!(
        "ingest_commit_walk_tape_node_count: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_walk_tape_node_count
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_walk_tape_node_count
    ));
    lines.push(format!(
        "ingest_commit_walk_tape_parent_edges: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_walk_tape_parent_edges
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_walk_tape_parent_edges
    ));
    lines.push(format!(
        "ingest_commit_ingest_queue_wait_ms: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_ingest_queue_wait
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_ingest_queue_wait
    ));
    lines.push(format!(
        "ingest_commit_ingest_queue_max_depth: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_ingest_queue_max_depth
            .max(
                input
                    .incremental_stats
                    .ingestion_phase_timing_ms
                    .commit_ingest_queue_max_depth,
            )
    ));
    lines.push(format!(
        "ingest_commit_write_wave_txn_count: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_write_wave_txn_count
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_write_wave_txn_count
    ));
    lines.push(format!(
        "ingest_commit_write_wave_batch_median: {:.3}",
        aggregate_commit_write_wave_batch_median
    ));
    lines.push(format!(
        "ingest_commit_csr_delta_apply_ms: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_csr_delta_apply
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_csr_delta_apply
    ));
    lines.push(format!(
        "ingest_commit_csr_overlay_upsert_ms: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_csr_overlay_upsert
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_csr_overlay_upsert
    ));
    lines.push(format!(
        "ingest_commit_csr_compaction_ms: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .commit_csr_compaction
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .commit_csr_compaction
    ));
    lines.push(format!(
        "ingest_query_commit_window_prewarm_hydrated_rows: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .query_commit_window_prewarm_hydrated_rows
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .query_commit_window_prewarm_hydrated_rows
    ));
    lines.push(format!(
        "ingest_file_diff_extract_ms: {:.3}",
        input.full_stats.ingestion_phase_timing_ms.file_diff_extract
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .file_diff_extract
    ));
    lines.push(format!(
        "ingest_file_diff_build_ms: {:.3}",
        input.full_stats.ingestion_phase_timing_ms.file_diff_build
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .file_diff_build
    ));
    lines.push(format!(
        "ingest_file_diff_rename_find_ms: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .file_diff_rename_find
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .file_diff_rename_find
    ));
    lines.push(format!(
        "ingest_file_diff_delta_emit_ms: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .file_diff_delta_emit
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .file_diff_delta_emit
    ));
    lines.push(format!(
        "ingest_file_change_upsert_ms: {:.3}",
        input
            .full_stats
            .ingestion_phase_timing_ms
            .file_change_upsert
            + input
                .incremental_stats
                .ingestion_phase_timing_ms
                .file_change_upsert
    ));
    lines.push(format!(
        "ingest_tx_commit_ms: {:.3}",
        input.full_stats.ingestion_phase_timing_ms.tx_commit
            + input.incremental_stats.ingestion_phase_timing_ms.tx_commit
    ));
    lines.push(format!("commits_in_db: {}", input.counts.0));
    lines.push(format!("edges_in_db: {}", input.counts.1));
    lines.push(format!("file_changes_in_db: {}", input.counts.2));
    lines.push(format!(
        "file_change_stats_pending: {}",
        input.stats_state_counts.pending
    ));
    lines.push(format!(
        "file_change_stats_ready: {}",
        input.stats_state_counts.ready
    ));
    lines.push(format!(
        "file_change_stats_failed: {}",
        input.stats_state_counts.failed
    ));
    lines.push(format!(
        "fixture_root_sha: {}",
        input.fixture_before_incremental.root_sha
    ));
    lines.push(format!(
        "fixture_pre_merge_main_sha: {}",
        input.fixture_before_incremental.pre_merge_main_sha
    ));
    lines.push(format!(
        "fixture_feature_tip_sha: {}",
        input.fixture_before_incremental.feature_tip_sha
    ));
    lines.push(format!(
        "fixture_head_sha: {}",
        input.fixture_before_incremental.head_sha
    ));
    lines.push(format!(
        "fixture_head_after_incremental: {}",
        input.fixture_after_incremental.head_sha
    ));
    lines.push(format!("query_runs: {}", input.config.query_runs));
    lines.push(format!("window_limit: {}", input.config.window_limit));
    lines.push(format!("max_nodes: {}", input.config.max_nodes));
    lines.push(format!(
        "fixture_generation_rss: {}",
        optional_u64_display(input.fixture_generation_rss)
    ));
    lines.push(format!(
        "full_index_rss: {}",
        optional_u64_display(input.full_index_rss)
    ));
    lines.push(format!(
        "incremental_index_rss: {}",
        optional_u64_display(input.incremental_index_rss)
    ));
    lines.push(format!(
        "file_change_extraction_drain_rss: {}",
        optional_u64_display(input.file_change_extraction_drain_rss)
    ));
    lines.push(format!(
        "jit_file_change_enrichment_ms: {:.3}",
        duration_ms(input.jit_file_change_enrichment)
    ));
    lines.push(format!(
        "phase_timing_ms: fixture_generation={:.3} full_index={:.3} incremental_index={:.3} query_benchmark={:.3} core_end_to_end={:.3}",
        duration_ms(input.fixture_generation),
        duration_ms(input.full_index),
        duration_ms(input.incremental_index),
        duration_ms(input.query_benchmark),
        duration_ms(input.core_end_to_end),
    ));
    lines.push(format!(
        "phase_memory_bytes: fixture_generation_rss={} scan_repository_rss={} full_index_rss={} incremental_append_rss={} incremental_index_rss={} file_change_extraction_drain_rss={} query_benchmark_rss={} jit_file_change_enrichment_rss={} in_memory_query_rss={} final_rss={} peak_observed_rss={}",
        optional_u64_display(input.fixture_generation_rss),
        optional_u64_display(input.scan_repository_rss),
        optional_u64_display(input.full_index_rss),
        optional_u64_display(input.incremental_append_rss),
        optional_u64_display(input.incremental_index_rss),
        optional_u64_display(input.file_change_extraction_drain_rss),
        optional_u64_display(input.query_benchmark_rss),
        optional_u64_display(input.jit_file_change_enrichment_rss),
        optional_u64_display(input.in_memory_query_rss),
        optional_u64_display(input.final_rss),
        optional_u64_display(input.peak_rss),
    ));
    lines.push(format!(
        "file_change_stats_state_counts: pending={} ready={} failed={}",
        input.stats_state_counts.pending,
        input.stats_state_counts.ready,
        input.stats_state_counts.failed,
    ));
    lines.push(format!(
        "in_memory_adjacency_query_latency_ms: bfs_parents={:.3} shortest_path_parents={:.3} shortest_path_both={:.3}",
        input.in_memory_adjacency_query_latency_ms.bfs_parents,
        input.in_memory_adjacency_query_latency_ms.shortest_path_parents,
        input.in_memory_adjacency_query_latency_ms.shortest_path_both,
    ));

    lines.join("\n") + "\n"
}

fn optional_u64_display(value: Option<u64>) -> String {
    value
        .map(|bytes| bytes.to_string())
        .unwrap_or_else(|| "n/a".to_string())
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn log_phase(label: &str, duration: Duration) {
    log_line(format!(
        "perf-report: phase {label} done in {:.3} ms",
        duration_ms(duration)
    ));
}

fn progress_stride(total_units: usize) -> usize {
    if total_units <= 100 {
        1
    } else {
        (total_units / 100).max(1)
    }
}

fn log_progress(label: &str, completed_units: usize, total_units: usize, started: Instant) {
    let elapsed_seconds = started.elapsed().as_secs_f64();
    let remaining_units = total_units.saturating_sub(completed_units);
    let eta_seconds = if completed_units == 0 {
        0.0
    } else {
        elapsed_seconds * (remaining_units as f64 / completed_units as f64)
    };
    log_line(format!(
        "perf-report: {label} progress {completed_units}/{total_units} elapsed_s={elapsed_seconds:.3} eta_s={eta_seconds:.3}"
    ));
}

fn log_line(message: impl AsRef<str>) {
    println!("{}", message.as_ref());
    let _ = io::stdout().flush();
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn current_rss_bytes() -> Option<u64> {
    memory_stats().map(|stats| stats.physical_mem as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gitgraph_lab_core::domain::models::{
        CommitThroughputTelemetry, FileChangeExtractionQueueMetrics, FileChangeStatsStateCounts,
        IndexingMode, IndexingStats, IngestionPhaseTimingMs,
    };

    fn test_indexing_stats(
        mode: IndexingMode,
        commit_throughput: CommitThroughputTelemetry,
        ingestion_phase_timing_ms: IngestionPhaseTimingMs,
    ) -> IndexingStats {
        IndexingStats {
            repo_id: 1,
            mode,
            planner_session_id: 11,
            planner_requested_mode: mode,
            planner_effective_mode: mode,
            planner_checkpoint_trusted: true,
            planner_risk_level: Default::default(),
            planner_degraded_policy: Default::default(),
            planner_trace_count: 0,
            lmdb_effective_map_size_bytes: 0,
            lmdb_no_readahead: false,
            lmdb_write_map: false,
            lmdb_map_async: false,
            lmdb_no_mem_init: false,
            commits_indexed: commit_throughput.commits_indexed,
            edges_indexed: 0,
            file_changes_indexed: 0,
            file_change_commits_enqueued: 0,
            file_change_queue_depth_after_index: 0,
            file_change_queue_oldest_age_seconds: 0,
            refs_indexed: 0,
            indexed_at_epoch: 0,
            commit_walk_total_ms: commit_throughput.commit_walk_total_ms,
            commit_callback_ms: commit_throughput.commit_callback_ms,
            commit_walk_decode_ms: commit_throughput.commit_walk_decode_ms,
            commit_walk_parent_resolve_ms: commit_throughput.commit_walk_parent_resolve_ms,
            commit_walk_metadata_parse_ms: commit_throughput.commit_walk_metadata_parse_ms,
            commit_walk_tip_collection_ms: commit_throughput.commit_walk_tip_collection_ms,
            commit_walk_hide_expansion_ms: commit_throughput.commit_walk_hide_expansion_ms,
            commit_walk_graph_lookup_ms: commit_throughput.commit_walk_graph_lookup_ms,
            commit_walk_ordering_ms: commit_throughput.commit_walk_ordering_ms,
            commit_walk_emit_ms: commit_throughput.commit_walk_emit_ms,
            commit_walk_pack_inflate_ms: commit_throughput.commit_walk_pack_inflate_ms,
            commit_walk_pack_base_resolve_ms: commit_throughput.commit_walk_pack_base_resolve_ms,
            commit_walk_pack_parse_ms: commit_throughput.commit_walk_pack_parse_ms,
            commit_walk_tape_build_ms: commit_throughput.commit_walk_tape_build_ms,
            commit_walk_graph_lookup_hit_count: commit_throughput
                .commit_walk_graph_lookup_hit_count,
            commit_walk_graph_lookup_miss_count: commit_throughput
                .commit_walk_graph_lookup_miss_count,
            commit_walk_graph_assist_hit_count: commit_throughput
                .commit_walk_graph_assist_hit_count,
            commit_walk_graph_assist_miss_count: commit_throughput
                .commit_walk_graph_assist_miss_count,
            commit_walk_nodes_seen_count: commit_throughput.commit_walk_nodes_seen_count,
            commit_walk_nodes_pruned_count: commit_throughput.commit_walk_nodes_pruned_count,
            commit_walk_queue_max_depth: commit_throughput.commit_walk_queue_max_depth,
            commit_metadata_pending_count: commit_throughput.commit_metadata_pending_count,
            commit_walk_boundary_mode: commit_throughput.commit_walk_boundary_mode.clone(),
            commit_walk_graph_missing_tip_count: commit_throughput
                .commit_walk_graph_missing_tip_count,
            commit_walk_boundary_stop_count: commit_throughput.commit_walk_boundary_stop_count,
            commit_walk_overlap_skip_count: commit_throughput.commit_walk_overlap_skip_count,
            commit_walk_tape_node_count: commit_throughput.commit_walk_tape_node_count,
            commit_walk_tape_parent_edges: commit_throughput.commit_walk_tape_parent_edges,
            commit_window_jit_hydrated_rows: commit_throughput.commit_window_jit_hydrated_rows,
            query_commit_window_prewarm_hydrated_rows: commit_throughput
                .query_commit_window_prewarm_hydrated_rows,
            incremental_csr_streaming_mode_effective: commit_throughput
                .incremental_csr_streaming_mode_effective
                .clone(),
            commit_write_wave_txn_count: commit_throughput.commit_write_wave_txn_count,
            commit_write_wave_batch_median: commit_throughput.commit_write_wave_batch_median,
            commit_ingest_queue_max_depth: commit_throughput.commit_ingest_queue_max_depth,
            commit_ingest_queue_wait_ms: commit_throughput.commit_ingest_queue_wait_ms,
            commit_csr_delta_apply_ms: commit_throughput.commit_csr_delta_apply_ms,
            commit_csr_overlay_upsert_ms: commit_throughput.commit_csr_overlay_upsert_ms,
            commit_csr_compaction_ms: commit_throughput.commit_csr_compaction_ms,
            ingestion_phase_timing_ms,
            commit_throughput,
        }
    }

    fn count_lines_with_prefix(stream: &str, prefix: &str) -> usize {
        stream
            .lines()
            .filter(|line| line.starts_with(prefix))
            .count()
    }

    #[test]
    fn report_includes_boundary_telemetry_keys() {
        let config = Config::default();
        let full = test_indexing_stats(
            IndexingMode::Full,
            CommitThroughputTelemetry {
                commits_seen: 4,
                commits_indexed: 4,
                end_to_end_ms: 100.0,
                commits_per_sec_seen: 40.0,
                commits_per_sec_indexed: 40.0,
                ms_per_commit_seen: 25.0,
                ms_per_commit_indexed: 25.0,
                commit_walk_scope: "frontier".to_string(),
                commit_walk_engine: "tape".to_string(),
                commit_walk_sort_effective: "commit_time_newest (auto)".to_string(),
                commit_walk_boundary_mode: "checkpoint_stop".to_string(),
                commit_walk_tip_count: 2,
                commit_walk_total_ms: 10.0,
                commit_walk_ms: 8.0,
                commit_walk_commits_per_sec_seen: 50.0,
                commit_walk_graph_missing_tip_count: 1,
                commit_walk_boundary_stop_count: 2,
                commit_walk_overlap_skip_count: 0,
                commit_walk_pack_inflate_ms: 1.5,
                commit_walk_pack_base_resolve_ms: 2.5,
                commit_walk_pack_parse_ms: 3.5,
                commit_walk_tape_build_ms: 4.5,
                commit_walk_tape_node_count: 7,
                commit_walk_tape_parent_edges: 8,
                incremental_csr_streaming_mode_effective: "enabled".to_string(),
                bottleneck_phase: "commit_walk".to_string(),
                ..Default::default()
            },
            IngestionPhaseTimingMs {
                commit_walk_pack_inflate: 1.5,
                commit_walk_pack_base_resolve: 2.5,
                commit_walk_pack_parse: 3.5,
                commit_walk_tape_build: 4.5,
                commit_walk_graph_missing_tip_count: 1.0,
                commit_walk_boundary_stop_count: 2.0,
                commit_walk_overlap_skip_count: 0.0,
                commit_walk_tape_node_count: 7.0,
                commit_walk_tape_parent_edges: 8.0,
                ..Default::default()
            },
        );
        let incremental = test_indexing_stats(
            IndexingMode::Incremental,
            CommitThroughputTelemetry {
                commits_seen: 1,
                commits_indexed: 1,
                end_to_end_ms: 50.0,
                commits_per_sec_seen: 20.0,
                commits_per_sec_indexed: 20.0,
                ms_per_commit_seen: 50.0,
                ms_per_commit_indexed: 50.0,
                commit_walk_scope: "frontier".to_string(),
                commit_walk_engine: "tape".to_string(),
                commit_walk_sort_effective: "commit_time_newest (auto)".to_string(),
                commit_walk_boundary_mode: "checkpoint_stop".to_string(),
                commit_walk_tip_count: 1,
                commit_walk_total_ms: 5.0,
                commit_walk_ms: 4.0,
                commit_walk_commits_per_sec_seen: 25.0,
                commit_walk_graph_missing_tip_count: 3,
                commit_walk_boundary_stop_count: 4,
                commit_walk_overlap_skip_count: 5,
                commit_walk_pack_inflate_ms: 10.0,
                commit_walk_pack_base_resolve_ms: 11.0,
                commit_walk_pack_parse_ms: 12.0,
                commit_walk_tape_build_ms: 13.0,
                commit_walk_tape_node_count: 14,
                commit_walk_tape_parent_edges: 15,
                incremental_csr_streaming_mode_effective: "enabled".to_string(),
                bottleneck_phase: "commit_walk".to_string(),
                ..Default::default()
            },
            IngestionPhaseTimingMs {
                commit_walk_pack_inflate: 10.0,
                commit_walk_pack_base_resolve: 11.0,
                commit_walk_pack_parse: 12.0,
                commit_walk_tape_build: 13.0,
                commit_walk_graph_missing_tip_count: 3.0,
                commit_walk_boundary_stop_count: 4.0,
                commit_walk_overlap_skip_count: 5.0,
                commit_walk_tape_node_count: 14.0,
                commit_walk_tape_parent_edges: 15.0,
                ..Default::default()
            },
        );
        let fixture = FixtureSummary {
            root_sha: "root".to_string(),
            pre_merge_main_sha: "pre-merge".to_string(),
            feature_tip_sha: "feature".to_string(),
            head_sha: "head".to_string(),
        };
        let report_input = ReportInput {
            fixture_root: Path::new("fixture"),
            fixture_before_incremental: &fixture,
            fixture_after_incremental: &fixture,
            config: &config,
            counts: (0, 0, 0),
            stats_state_counts: &FileChangeStatsStateCounts::default(),
            extraction_queue: &FileChangeExtractionQueueMetrics::default(),
            full_stats: &full,
            incremental_stats: &incremental,
            fixture_generation: Duration::ZERO,
            full_index: Duration::ZERO,
            incremental_index: Duration::ZERO,
            query_benchmark: Duration::ZERO,
            commit_window_jit_hydrated_rows: 0,
            in_memory_graph_hydration: Duration::ZERO,
            in_memory_query_benchmark: Duration::ZERO,
            in_memory_adjacency_query_latency_ms: Default::default(),
            file_change_extraction_drain: Duration::ZERO,
            jit_file_change_enrichment: Duration::ZERO,
            fixture_build_telemetry: FixtureBuildTelemetry::default(),
            incremental_append_telemetry: FixtureBuildTelemetry::default(),
            core_end_to_end: Duration::ZERO,
            fixture_generation_rss: None,
            scan_repository_rss: None,
            full_index_rss: None,
            incremental_append_rss: None,
            incremental_index_rss: None,
            file_change_extraction_drain_rss: None,
            query_benchmark_rss: None,
            jit_file_change_enrichment_rss: None,
            in_memory_query_rss: None,
            final_rss: None,
            peak_rss: None,
            commit_throughput_per_sec: 0.0,
        };

        let report = format_report(&report_input);
        assert!(report.contains("full_commit_walk_engine: tape"));
        assert!(report.contains("full_commit_walk_boundary_mode: checkpoint_stop"));
        assert!(report.contains("aggregate_commit_walk_graph_missing_tip_count: 4"));
        assert!(report.contains("aggregate_commit_walk_boundary_stop_count: 6"));
        assert!(report.contains("aggregate_commit_walk_overlap_skip_count: 5"));
        assert!(report.contains("aggregate_commit_walk_pack_inflate_ms: 11.500"));
        assert!(report.contains("aggregate_commit_walk_pack_base_resolve_ms: 13.500"));
        assert!(report.contains("aggregate_commit_walk_pack_parse_ms: 15.500"));
        assert!(report.contains("aggregate_commit_walk_tape_build_ms: 17.500"));
        assert!(report.contains("aggregate_commit_walk_tape_node_count: 21"));
        assert!(report.contains("aggregate_commit_walk_tape_parent_edges: 23"));
        assert!(report.contains("ingest_commit_walk_graph_missing_tip_count: 4.000"));
        assert!(report.contains("ingest_commit_walk_boundary_stop_count: 6.000"));
        assert!(report.contains("ingest_commit_walk_overlap_skip_count: 5.000"));
        assert!(report.contains("ingest_commit_walk_pack_inflate_ms: 11.500"));
        assert!(report.contains("ingest_commit_walk_pack_base_resolve_ms: 13.500"));
        assert!(report.contains("ingest_commit_walk_pack_parse_ms: 15.500"));
        assert!(report.contains("ingest_commit_walk_tape_build_ms: 17.500"));
        assert!(report.contains("ingest_commit_walk_tape_node_count: 21.000"));
        assert!(report.contains("ingest_commit_walk_tape_parent_edges: 23.000"));
    }

    #[test]
    fn branching_fast_import_stream_counts_match_expected_shape() {
        let cycles = 3;
        let feature_commits = 2;
        let (stream, generated_commits) =
            build_branching_fixture_fast_import_stream(cycles, feature_commits);

        assert_eq!(
            generated_commits,
            branching_fixture_commit_count(cycles, feature_commits)
        );
        assert_eq!(
            count_lines_with_prefix(stream.as_str(), "commit "),
            generated_commits
        );
        assert_eq!(count_lines_with_prefix(stream.as_str(), "merge :"), cycles);
        assert!(stream.ends_with("done\n"));
    }

    #[test]
    fn branching_fast_import_stream_omits_merge_when_feature_commits_are_zero() {
        let cycles = 4;
        let (stream, generated_commits) = build_branching_fixture_fast_import_stream(cycles, 0);
        assert_eq!(generated_commits, 1 + cycles);
        assert_eq!(count_lines_with_prefix(stream.as_str(), "merge :"), 0);
    }

    #[test]
    fn incremental_fast_import_stream_chains_parent_marks_after_first_commit() {
        let base_head = "0123456789abcdef0123456789abcdef01234567";
        let (stream, generated_commits) = build_incremental_fast_import_stream(4, base_head);
        assert_eq!(generated_commits, 4);
        assert_eq!(count_lines_with_prefix(stream.as_str(), "commit "), 4);
        assert_eq!(count_lines_with_prefix(stream.as_str(), "from "), 4);
        assert!(stream.contains(format!("from {base_head}").as_str()));
        assert!(stream.contains("from :1"));
        assert!(stream.contains("from :2"));
        assert!(stream.contains("from :3"));
        assert!(stream.ends_with("done\n"));
    }
}
