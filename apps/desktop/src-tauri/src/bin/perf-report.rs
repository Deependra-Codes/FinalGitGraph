use std::collections::{HashMap, VecDeque};
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gitgraph_lab_core::application::commands::{
    get_commit_window, get_graph_traversal, get_shortest_path, index_commits_with_mode,
    scan_repository,
};
use gitgraph_lab_core::domain::models::{GraphDirection, GraphTraversalStrategy, IndexingMode};
use memory_stats::memory_stats;
use rusqlite::{params, Connection};

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
struct LatencySummary {
    runs: usize,
    min_ms: f64,
    median_ms: f64,
    p95_ms: f64,
    max_ms: f64,
    mean_ms: f64,
}

#[derive(Debug, Clone)]
struct PhaseTimingSummary {
    fixture_generation: Duration,
    fixture_summary: Duration,
    repository_scan: Duration,
    full_index: Duration,
    incremental_fixture_append: Duration,
    incremental_index: Duration,
    query_benchmark: Duration,
    in_memory_graph_hydration: Duration,
    in_memory_query_benchmark: Duration,
    core_end_to_end: Duration,
}

#[derive(Debug, Clone)]
struct PhaseMemorySummary {
    fixture_generation_rss_bytes: Option<u64>,
    fixture_summary_rss_bytes: Option<u64>,
    repository_scan_rss_bytes: Option<u64>,
    full_index_rss_bytes: Option<u64>,
    incremental_fixture_append_rss_bytes: Option<u64>,
    incremental_index_rss_bytes: Option<u64>,
    query_benchmark_rss_bytes: Option<u64>,
    in_memory_graph_hydration_rss_bytes: Option<u64>,
    in_memory_query_benchmark_rss_bytes: Option<u64>,
    final_rss_bytes: Option<u64>,
    peak_observed_rss_bytes: Option<u64>,
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

struct QueryLatencySet<'a> {
    commit_window: &'a LatencySummary,
    bfs: &'a LatencySummary,
    dfs: &'a LatencySummary,
    topological: &'a LatencySummary,
    shortest_parents: &'a LatencySummary,
    shortest_both: &'a LatencySummary,
}

struct InMemoryQueryLatencySet<'a> {
    bfs_parents: &'a LatencySummary,
    shortest_parents: &'a LatencySummary,
    shortest_both: &'a LatencySummary,
}

struct InMemoryGraphSummary {
    nodes: usize,
    directed_edges: usize,
}

struct InMemoryGraphContext {
    graph: InMemoryAdjacencyGraph,
    root_node: usize,
    pre_merge_main_node: usize,
    feature_tip_node: usize,
    head_node: usize,
}

struct ReportInput<'a> {
    config: &'a Config,
    fixture_root: &'a Path,
    counts: (i64, i64, i64),
    full_stats: &'a gitgraph_lab_core::domain::models::IndexingStats,
    incremental_stats: &'a gitgraph_lab_core::domain::models::IndexingStats,
    query: QueryLatencySet<'a>,
    in_memory_query: InMemoryQueryLatencySet<'a>,
    in_memory_graph: &'a InMemoryGraphSummary,
    phase_timing: &'a PhaseTimingSummary,
    phase_memory: &'a PhaseMemorySummary,
    estimated_git_process_spawns: usize,
}

#[derive(Debug, Clone)]
struct InMemoryAdjacencyGraph {
    rowid_to_index: HashMap<i64, usize>,
    index_to_rowid: Vec<i64>,
    parent_neighbors: Vec<Vec<usize>>,
    child_neighbors: Vec<Vec<usize>>,
    directed_edges: usize,
}

impl InMemoryAdjacencyGraph {
    fn hydrate(conn: &Connection, repo_id: i64) -> PerfResult<Self> {
        let mut commit_stmt = conn
            .prepare_cached(
                r#"
                SELECT commit_id
                FROM commits
                WHERE repo_id = ?1
                ORDER BY commit_id ASC
            "#,
            )
            .map_err(|error| format!("prepare commit scan for adjacency hydration: {error}"))?;
        let commit_rows = commit_stmt
            .query_map([repo_id], |row| row.get::<_, i64>(0))
            .map_err(|error| format!("query commit ids for adjacency hydration: {error}"))?;

        let mut rowid_to_index = HashMap::new();
        let mut index_to_rowid = Vec::new();
        for row in commit_rows {
            let commit_rowid =
                row.map_err(|error| format!("read commit id for adjacency hydration: {error}"))?;
            rowid_to_index.insert(commit_rowid, index_to_rowid.len());
            index_to_rowid.push(commit_rowid);
        }

        if index_to_rowid.is_empty() {
            return Err("adjacency hydration requires at least one indexed commit".to_string());
        }

        let mut parent_neighbors = vec![Vec::new(); index_to_rowid.len()];
        let mut child_neighbors = vec![Vec::new(); index_to_rowid.len()];
        let mut directed_edges = 0usize;

        let mut edge_stmt = conn
            .prepare_cached(
                r#"
                SELECT child_commit_id, parent_commit_id
                FROM commit_edges
                WHERE repo_id = ?1
            "#,
            )
            .map_err(|error| format!("prepare edge scan for adjacency hydration: {error}"))?;
        let edge_rows = edge_stmt
            .query_map([repo_id], |row| {
                let child_rowid: i64 = row.get(0)?;
                let parent_rowid: i64 = row.get(1)?;
                Ok((child_rowid, parent_rowid))
            })
            .map_err(|error| format!("query commit edges for adjacency hydration: {error}"))?;

        for edge in edge_rows {
            let (child_rowid, parent_rowid) =
                edge.map_err(|error| format!("read edge row during adjacency hydration: {error}"))?;
            let child_index = rowid_to_index.get(&child_rowid).copied().ok_or_else(|| {
                format!(
                    "adjacency hydration missing child commit rowid in commit table: {child_rowid}"
                )
            })?;
            let parent_index = rowid_to_index.get(&parent_rowid).copied().ok_or_else(|| {
                format!(
                    "adjacency hydration missing parent commit rowid in commit table: {parent_rowid}"
                )
            })?;

            parent_neighbors[child_index].push(parent_index);
            child_neighbors[parent_index].push(child_index);
            directed_edges = directed_edges.saturating_add(1);
        }

        for neighbors in &mut parent_neighbors {
            neighbors.sort_unstable_by(|left, right| right.cmp(left));
        }
        for neighbors in &mut child_neighbors {
            neighbors.sort_unstable_by(|left, right| right.cmp(left));
        }

        Ok(Self {
            rowid_to_index,
            index_to_rowid,
            parent_neighbors,
            child_neighbors,
            directed_edges,
        })
    }

    fn node_count(&self) -> usize {
        self.index_to_rowid.len()
    }

    fn directed_edge_count(&self) -> usize {
        self.directed_edges
    }

    fn node_for_rowid(&self, rowid: i64) -> Option<usize> {
        self.rowid_to_index.get(&rowid).copied()
    }

    fn bfs(&self, start_node: usize, direction: GraphDirection, max_nodes: usize) -> Vec<usize> {
        if start_node >= self.node_count() {
            return Vec::new();
        }

        let bounded_max_nodes = max_nodes.max(1);
        let mut visited = vec![false; self.node_count()];
        let mut queue = VecDeque::from([start_node]);
        let mut ordered = Vec::new();
        visited[start_node] = true;

        while let Some(source) = queue.pop_front() {
            ordered.push(source);
            if ordered.len() >= bounded_max_nodes {
                break;
            }

            self.for_each_neighbor(source, direction, |neighbor| {
                if !visited[neighbor] {
                    visited[neighbor] = true;
                    queue.push_back(neighbor);
                }
            });
        }

        ordered
    }

    fn shortest_path(
        &self,
        from_node: usize,
        to_node: usize,
        direction: GraphDirection,
    ) -> Option<Vec<usize>> {
        if from_node >= self.node_count() || to_node >= self.node_count() {
            return None;
        }
        if from_node == to_node {
            return Some(vec![from_node]);
        }

        let mut visited = vec![false; self.node_count()];
        let mut previous = vec![usize::MAX; self.node_count()];
        let mut queue = VecDeque::from([from_node]);
        visited[from_node] = true;

        while let Some(source) = queue.pop_front() {
            if source == to_node {
                break;
            }

            self.for_each_neighbor(source, direction, |neighbor| {
                if !visited[neighbor] {
                    visited[neighbor] = true;
                    previous[neighbor] = source;
                    queue.push_back(neighbor);
                }
            });
        }

        if !visited[to_node] {
            return None;
        }

        let mut path = vec![to_node];
        let mut cursor = to_node;
        while cursor != from_node {
            let parent = previous[cursor];
            if parent == usize::MAX {
                return None;
            }
            cursor = parent;
            path.push(cursor);
        }
        path.reverse();
        Some(path)
    }

    fn for_each_neighbor(
        &self,
        source: usize,
        direction: GraphDirection,
        mut visit: impl FnMut(usize),
    ) {
        match direction {
            GraphDirection::Parents => {
                for &neighbor in &self.parent_neighbors[source] {
                    visit(neighbor);
                }
            }
            GraphDirection::Children => {
                for &neighbor in &self.child_neighbors[source] {
                    visit(neighbor);
                }
            }
            GraphDirection::Both => {
                let parents = &self.parent_neighbors[source];
                let children = &self.child_neighbors[source];
                let mut parent_index = 0usize;
                let mut child_index = 0usize;
                let mut previous_neighbor = None;

                while parent_index < parents.len() || child_index < children.len() {
                    let next_neighbor = if child_index >= children.len()
                        || (parent_index < parents.len()
                            && parents[parent_index] >= children[child_index])
                    {
                        let next = parents[parent_index];
                        parent_index += 1;
                        next
                    } else {
                        let next = children[child_index];
                        child_index += 1;
                        next
                    };

                    if previous_neighbor != Some(next_neighbor) {
                        previous_neighbor = Some(next_neighbor);
                        visit(next_neighbor);
                    }
                }
            }
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("perf-report failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> PerfResult<()> {
    let config = parse_config()?;
    let fixture_root = create_fixture_root()?;
    let repo_dir = fixture_root.join("repo");
    let db_path = fixture_root.join("gitgraph-lab-perf.db");

    let result = run_with_fixture(&config, &fixture_root, &repo_dir, &db_path);
    if !config.keep_fixture {
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
    let overall_started = Instant::now();
    let mut memory_tracker = MemoryTracker::default();
    let _ = memory_tracker.sample_rss();

    log_phase_start("fixture_generation");
    let fixture_generation_started = Instant::now();
    generate_branching_fixture(repo_dir, config.cycles, config.feature_commits)?;
    let fixture_generation = fixture_generation_started.elapsed();
    log_phase_done("fixture_generation", fixture_generation);
    let fixture_generation_rss_bytes = memory_tracker.sample_rss();

    log_phase_start("fixture_summary");
    let fixture_summary_started = Instant::now();
    let mut fixture = collect_fixture_summary(repo_dir)?;
    let fixture_summary = fixture_summary_started.elapsed();
    log_phase_done("fixture_summary", fixture_summary);
    let fixture_summary_rss_bytes = memory_tracker.sample_rss();

    let repo_str = path_to_string(repo_dir);
    let db_str = path_to_string(db_path);

    log_phase_start("repository_scan");
    let repository_scan_started = Instant::now();
    scan_repository(repo_str.as_str(), Some(db_str.as_str())).map_err(|error| error.to_string())?;
    let repository_scan = repository_scan_started.elapsed();
    log_phase_done("repository_scan", repository_scan);
    let repository_scan_rss_bytes = memory_tracker.sample_rss();

    log_phase_start("full_index");
    let full_started = Instant::now();
    let full_stats =
        index_commits_with_mode(repo_str.as_str(), Some(db_str.as_str()), IndexingMode::Full)
            .map_err(|error| error.to_string())?;
    let full_index = full_started.elapsed();
    log_phase_done("full_index", full_index);
    let full_index_rss_bytes = memory_tracker.sample_rss();

    log_phase_start("incremental_fixture_append");
    let incremental_append_started = Instant::now();
    append_incremental_commits(repo_dir, config.incremental_commits)?;
    fixture.head_sha = current_head_sha(repo_dir)?;
    let incremental_fixture_append = incremental_append_started.elapsed();
    log_phase_done("incremental_fixture_append", incremental_fixture_append);
    let incremental_fixture_append_rss_bytes = memory_tracker.sample_rss();

    log_phase_start("incremental_index");
    let incremental_started = Instant::now();
    let incremental_stats = index_commits_with_mode(
        repo_str.as_str(),
        Some(db_str.as_str()),
        IndexingMode::Incremental,
    )
    .map_err(|error| error.to_string())?;
    let incremental_index = incremental_started.elapsed();
    log_phase_done("incremental_index", incremental_index);
    let incremental_index_rss_bytes = memory_tracker.sample_rss();

    let counts = load_repo_counts(db_path)?;

    log_phase_start("query_benchmark");
    let query_started = Instant::now();
    let commit_window = measure_latencies(config.query_runs, || {
        let response = get_commit_window(
            repo_str.as_str(),
            Some(db_str.as_str()),
            Some(config.window_limit),
            None,
        )
        .map_err(|error| error.to_string())?;
        if response.commits.is_empty() {
            return Err("commit window returned empty data".to_string());
        }
        Ok(())
    })?;

    let bfs = measure_latencies(config.query_runs, || {
        let response = get_graph_traversal(
            repo_str.as_str(),
            Some(db_str.as_str()),
            fixture.head_sha.as_str(),
            GraphTraversalStrategy::Bfs,
            GraphDirection::Parents,
            Some(config.max_nodes),
        )
        .map_err(|error| error.to_string())?;
        if response.nodes.is_empty() {
            return Err("bfs traversal returned no nodes".to_string());
        }
        Ok(())
    })?;

    let dfs = measure_latencies(config.query_runs, || {
        let response = get_graph_traversal(
            repo_str.as_str(),
            Some(db_str.as_str()),
            fixture.head_sha.as_str(),
            GraphTraversalStrategy::Dfs,
            GraphDirection::Parents,
            Some(config.max_nodes),
        )
        .map_err(|error| error.to_string())?;
        if response.nodes.is_empty() {
            return Err("dfs traversal returned no nodes".to_string());
        }
        Ok(())
    })?;

    let topological = measure_latencies(config.query_runs, || {
        let response = get_graph_traversal(
            repo_str.as_str(),
            Some(db_str.as_str()),
            fixture.head_sha.as_str(),
            GraphTraversalStrategy::Topological,
            GraphDirection::Parents,
            Some(config.max_nodes),
        )
        .map_err(|error| error.to_string())?;
        if response.nodes.is_empty() {
            return Err("topological traversal returned no nodes".to_string());
        }
        Ok(())
    })?;

    let shortest_parents = measure_latencies(config.query_runs, || {
        let response = get_shortest_path(
            repo_str.as_str(),
            Some(db_str.as_str()),
            fixture.head_sha.as_str(),
            fixture.root_sha.as_str(),
            GraphDirection::Parents,
        )
        .map_err(|error| error.to_string())?;
        if !response.found || response.path.is_empty() {
            return Err("shortest path (parents) did not find a path".to_string());
        }
        Ok(())
    })?;

    let shortest_both = measure_latencies(config.query_runs, || {
        let response = get_shortest_path(
            repo_str.as_str(),
            Some(db_str.as_str()),
            fixture.pre_merge_main_sha.as_str(),
            fixture.feature_tip_sha.as_str(),
            GraphDirection::Both,
        )
        .map_err(|error| error.to_string())?;
        if !response.found || response.path.is_empty() {
            return Err("shortest path (both) did not find a path".to_string());
        }
        Ok(())
    })?;
    let query_benchmark = query_started.elapsed();
    log_phase_done("query_benchmark", query_benchmark);
    let query_benchmark_rss_bytes = memory_tracker.sample_rss();

    log_phase_start("in_memory_graph_hydration");
    let in_memory_graph_hydration_started = Instant::now();
    let in_memory_context = load_in_memory_graph_context(db_path, &fixture)?;
    let in_memory_graph_hydration = in_memory_graph_hydration_started.elapsed();
    log_phase_done("in_memory_graph_hydration", in_memory_graph_hydration);
    let in_memory_graph_hydration_rss_bytes = memory_tracker.sample_rss();

    log_phase_start("in_memory_query_benchmark");
    let in_memory_query_started = Instant::now();
    let in_memory_bfs = measure_latencies(config.query_runs, || {
        let ordered = in_memory_context.graph.bfs(
            in_memory_context.head_node,
            GraphDirection::Parents,
            config.max_nodes,
        );
        if ordered.is_empty() {
            return Err("in-memory bfs (parents) returned no nodes".to_string());
        }
        Ok(())
    })?;

    let in_memory_shortest_parents = measure_latencies(config.query_runs, || {
        let path = in_memory_context.graph.shortest_path(
            in_memory_context.head_node,
            in_memory_context.root_node,
            GraphDirection::Parents,
        );
        match path {
            Some(path) if !path.is_empty() => Ok(()),
            _ => Err("in-memory shortest path (parents) did not find a path".to_string()),
        }
    })?;

    let in_memory_shortest_both = measure_latencies(config.query_runs, || {
        let path = in_memory_context.graph.shortest_path(
            in_memory_context.pre_merge_main_node,
            in_memory_context.feature_tip_node,
            GraphDirection::Both,
        );
        match path {
            Some(path) if !path.is_empty() => Ok(()),
            _ => Err("in-memory shortest path (both) did not find a path".to_string()),
        }
    })?;
    let in_memory_query_benchmark = in_memory_query_started.elapsed();
    log_phase_done("in_memory_query_benchmark", in_memory_query_benchmark);
    let in_memory_query_benchmark_rss_bytes = memory_tracker.sample_rss();

    let in_memory_graph_summary = InMemoryGraphSummary {
        nodes: in_memory_context.graph.node_count(),
        directed_edges: in_memory_context.graph.directed_edge_count(),
    };

    let phase_timing = PhaseTimingSummary {
        fixture_generation,
        fixture_summary,
        repository_scan,
        full_index,
        incremental_fixture_append,
        incremental_index,
        query_benchmark,
        in_memory_graph_hydration,
        in_memory_query_benchmark,
        core_end_to_end: overall_started.elapsed(),
    };
    let phase_memory = PhaseMemorySummary {
        fixture_generation_rss_bytes,
        fixture_summary_rss_bytes,
        repository_scan_rss_bytes,
        full_index_rss_bytes,
        incremental_fixture_append_rss_bytes,
        incremental_index_rss_bytes,
        query_benchmark_rss_bytes,
        in_memory_graph_hydration_rss_bytes,
        in_memory_query_benchmark_rss_bytes,
        final_rss_bytes: memory_tracker.sample_rss(),
        peak_observed_rss_bytes: memory_tracker.peak_observed_rss(),
    };

    let report = format_report(&ReportInput {
        config,
        fixture_root,
        counts,
        full_stats: &full_stats,
        incremental_stats: &incremental_stats,
        query: QueryLatencySet {
            commit_window: &commit_window,
            bfs: &bfs,
            dfs: &dfs,
            topological: &topological,
            shortest_parents: &shortest_parents,
            shortest_both: &shortest_both,
        },
        in_memory_query: InMemoryQueryLatencySet {
            bfs_parents: &in_memory_bfs,
            shortest_parents: &in_memory_shortest_parents,
            shortest_both: &in_memory_shortest_both,
        },
        in_memory_graph: &in_memory_graph_summary,
        phase_timing: &phase_timing,
        phase_memory: &phase_memory,
        estimated_git_process_spawns: estimate_git_process_spawns(config),
    });

    println!("{report}");
    if let Some(output_path) = &config.output_path {
        if let Some(parent) = output_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|error| {
                    format!(
                        "create output directory {}: {error}",
                        parent.to_string_lossy()
                    )
                })?;
            }
        }
        fs::write(output_path, report).map_err(|error| {
            format!("write report to {}: {error}", output_path.to_string_lossy())
        })?;
    }

    Ok(())
}

fn parse_config() -> PerfResult<Config> {
    let mut config = Config::default();
    let mut args = env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--cycles" => {
                config.cycles = parse_next_usize("--cycles", args.next())?;
            }
            "--feature-commits" => {
                config.feature_commits = parse_next_usize("--feature-commits", args.next())?;
            }
            "--incremental-commits" => {
                config.incremental_commits =
                    parse_next_usize("--incremental-commits", args.next())?;
            }
            "--query-runs" => {
                config.query_runs = parse_next_usize("--query-runs", args.next())?;
            }
            "--window-limit" => {
                config.window_limit = parse_next_usize("--window-limit", args.next())?;
            }
            "--max-nodes" => {
                config.max_nodes = parse_next_usize("--max-nodes", args.next())?;
            }
            "--output" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--output requires a path value".to_string())?;
                config.output_path = Some(PathBuf::from(value));
            }
            "--keep-fixture" => {
                config.keep_fixture = true;
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            _ => {
                return Err(format!("unknown argument: {arg}"));
            }
        }
    }

    if config.cycles == 0 {
        return Err("--cycles must be greater than 0".to_string());
    }
    if config.feature_commits == 0 {
        return Err("--feature-commits must be greater than 0".to_string());
    }
    if config.query_runs == 0 {
        return Err("--query-runs must be greater than 0".to_string());
    }
    if config.window_limit == 0 {
        return Err("--window-limit must be greater than 0".to_string());
    }
    if config.max_nodes == 0 {
        return Err("--max-nodes must be greater than 0".to_string());
    }

    Ok(config)
}

fn print_help() {
    println!(
        "Usage: perf-report [options]
  --cycles <n>               Number of branch/merge cycles (default: 120)
  --feature-commits <n>      Feature-branch commits per cycle (default: 4)
  --incremental-commits <n>  New commits before incremental index (default: 40)
  --query-runs <n>           Repetitions per latency metric (default: 25)
  --window-limit <n>         Commit window size for query metric (default: 200)
  --max-nodes <n>            Traversal max_nodes parameter (default: 2000)
  --output <path>            Write report text to file
  --keep-fixture             Keep generated fixture directory for inspection"
    );
}

fn parse_next_usize(flag: &str, value: Option<String>) -> PerfResult<usize> {
    let raw = value.ok_or_else(|| format!("{flag} requires a numeric value"))?;
    raw.parse::<usize>()
        .map_err(|_| format!("{flag} requires a numeric value"))
}

fn create_fixture_root() -> PerfResult<PathBuf> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let root = env::temp_dir().join(format!("gitgraph-lab-perf-{now}"));
    fs::create_dir_all(&root)
        .map_err(|error| format!("create fixture root {}: {error}", root.to_string_lossy()))?;
    Ok(root)
}

fn generate_branching_fixture(
    repo_dir: &Path,
    cycles: usize,
    feature_commits: usize,
) -> PerfResult<()> {
    fs::create_dir_all(repo_dir)
        .map_err(|error| format!("create repo dir {}: {error}", repo_dir.to_string_lossy()))?;
    run_git(repo_dir, &["init"])?;
    run_git(repo_dir, &["config", "user.name", "GitGraph Lab Perf"])?;
    run_git(
        repo_dir,
        &["config", "user.email", "perf@gitgraph-lab.local"],
    )?;

    let default_branch = current_branch_name(repo_dir)?;
    let script = build_branching_fast_import_script(cycles, feature_commits, &default_branch);
    run_git_with_input(repo_dir, &["fast-import", "--date-format=raw"], &script)?;
    run_git(repo_dir, &["checkout", "-f", default_branch.as_str()])?;

    Ok(())
}

fn collect_fixture_summary(repo_dir: &Path) -> PerfResult<FixtureSummary> {
    let root_sha = run_git_capture(repo_dir, &["rev-list", "--max-parents=0", "HEAD"])?
        .lines()
        .next()
        .ok_or_else(|| "failed to resolve root sha".to_string())?
        .trim()
        .to_string();
    let head_sha = current_head_sha(repo_dir)?;
    let merge_parent_shas = run_git_capture(repo_dir, &["rev-parse", "HEAD^1", "HEAD^2"])?;
    let mut lines = merge_parent_shas.lines();
    let pre_merge_main_sha = lines
        .next()
        .ok_or_else(|| "failed to resolve merge first parent".to_string())?
        .trim()
        .to_string();
    let feature_tip_sha = lines
        .next()
        .ok_or_else(|| "failed to resolve merge second parent".to_string())?
        .trim()
        .to_string();

    Ok(FixtureSummary {
        root_sha,
        pre_merge_main_sha,
        feature_tip_sha,
        head_sha,
    })
}

fn append_incremental_commits(repo_dir: &Path, count: usize) -> PerfResult<()> {
    if count == 0 {
        return Ok(());
    }

    let default_branch = current_branch_name(repo_dir)?;
    let head_sha = current_head_sha(repo_dir)?;
    let script = build_incremental_fast_import_script(count, &default_branch, &head_sha);
    run_git_with_input(repo_dir, &["fast-import", "--date-format=raw"], &script)?;
    run_git(repo_dir, &["checkout", "-f", default_branch.as_str()])?;

    Ok(())
}

fn build_branching_fast_import_script(
    cycles: usize,
    feature_commits: usize,
    default_branch: &str,
) -> String {
    let mut script = String::new();
    let mut mark = 1u64;
    let mut timestamp = 1i64;

    let mut next_mark = || {
        mark = mark.saturating_add(1);
        mark
    };
    let mut next_timestamp = || {
        timestamp = timestamp.saturating_add(1);
        timestamp
    };

    let seed_mark = next_mark();
    script.push_str(&commit_fast_import_block(
        default_branch,
        seed_mark,
        None,
        &[],
        "seed",
        next_timestamp(),
        &[
            ("README.md", "# GitGraph Lab Perf Fixture\n"),
            ("shared.txt", "seed\n"),
        ],
    ));

    let mut main_tip = seed_mark;
    for cycle in 0..cycles {
        let main_prep_mark = next_mark();
        script.push_str(&commit_fast_import_block(
            default_branch,
            main_prep_mark,
            Some(main_tip),
            &[],
            &format!("main-prep-{cycle}"),
            next_timestamp(),
            &[("shared.txt", &format!("main-prep-{cycle}\n"))],
        ));
        main_tip = main_prep_mark;

        let branch_name = format!("feature-{cycle:04}");
        let mut feature_tip = main_prep_mark;
        for index in 0..feature_commits {
            let feature_mark = next_mark();
            script.push_str(&commit_fast_import_block(
                &branch_name,
                feature_mark,
                Some(feature_tip),
                &[],
                &format!("feature-{cycle}-{index}"),
                next_timestamp(),
                &[(
                    &format!("feature-{cycle:04}.txt"),
                    &format!("feature-{cycle}-{index}\n"),
                )],
            ));
            feature_tip = feature_mark;
        }

        let main_work_mark = next_mark();
        script.push_str(&commit_fast_import_block(
            default_branch,
            main_work_mark,
            Some(main_tip),
            &[],
            &format!("main-work-{cycle}"),
            next_timestamp(),
            &[("main.txt", &format!("main-work-{cycle}\n"))],
        ));
        main_tip = main_work_mark;

        let merge_mark = next_mark();
        script.push_str(&commit_fast_import_block(
            default_branch,
            merge_mark,
            Some(main_tip),
            &[feature_tip],
            &format!("merge-{cycle}"),
            next_timestamp(),
            &[("merge-log.txt", &format!("merge-{cycle}\n"))],
        ));
        main_tip = merge_mark;
    }

    script
}

fn build_incremental_fast_import_script(
    count: usize,
    default_branch: &str,
    base_sha: &str,
) -> String {
    let mut script = String::new();
    let mut mark = 1u64;
    let mut timestamp = 2_000_000_000i64;
    let mut parent = format!("from {base_sha}\n");

    for index in 0..count {
        mark = mark.saturating_add(1);
        timestamp = timestamp.saturating_add(1);
        let mark_line = format!(":{mark}");
        let block = commit_fast_import_block_raw_parent(
            default_branch,
            mark_line.as_str(),
            parent.as_str(),
            &format!("incremental-{index}"),
            timestamp,
            &[("incremental.txt", &format!("incremental-{index}\n"))],
        );
        script.push_str(&block);
        parent = format!("from :{mark}\n");
    }

    script
}

fn commit_fast_import_block(
    branch: &str,
    mark: u64,
    from_mark: Option<u64>,
    merge_marks: &[u64],
    message: &str,
    timestamp: i64,
    file_updates: &[(&str, &str)],
) -> String {
    let mut from_line = String::new();
    if let Some(parent_mark) = from_mark {
        from_line = format!("from :{parent_mark}\n");
    }
    let merge_lines = merge_marks
        .iter()
        .map(|merge_mark| format!("merge :{merge_mark}\n"))
        .collect::<String>();
    commit_fast_import_block_raw_parent(
        branch,
        &format!(":{mark}"),
        &(from_line + &merge_lines),
        message,
        timestamp,
        file_updates,
    )
}

fn commit_fast_import_block_raw_parent(
    branch: &str,
    mark_ref: &str,
    parent_lines: &str,
    message: &str,
    timestamp: i64,
    file_updates: &[(&str, &str)],
) -> String {
    let mut block = String::new();
    block.push_str(&format!("commit refs/heads/{branch}\n"));
    block.push_str(&format!("mark {mark_ref}\n"));
    block.push_str(&format!(
        "author GitGraph Lab Perf <perf@gitgraph-lab.local> {timestamp} +0000\n"
    ));
    block.push_str(&format!(
        "committer GitGraph Lab Perf <perf@gitgraph-lab.local> {timestamp} +0000\n"
    ));
    block.push_str(&format!("data {}\n{}\n", message.len(), message));
    block.push_str(parent_lines);
    for (path, content) in file_updates {
        block.push_str(&format!(
            "M 100644 inline {}\ndata {}\n{}\n",
            path,
            content.len(),
            content
        ));
    }
    block
}

fn run_git_with_input(repo_dir: &Path, args: &[&str], input: &str) -> PerfResult<()> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo_dir)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("execute git {:?}: {error}", args))?;

    {
        let Some(stdin) = child.stdin.as_mut() else {
            return Err(format!("execute git {:?}: missing stdin pipe", args));
        };
        stdin
            .write_all(input.as_bytes())
            .map_err(|error| format!("write git {:?} stdin: {error}", args))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|error| format!("wait git {:?}: {error}", args))?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(format!("git {:?} failed: {stderr}", args))
}

fn run_git(repo_dir: &Path, args: &[&str]) -> PerfResult<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_dir)
        .args(args)
        .output()
        .map_err(|error| format!("execute git {:?}: {error}", args))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(format!("git {:?} failed: {stderr}", args))
}

fn run_git_capture(repo_dir: &Path, args: &[&str]) -> PerfResult<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_dir)
        .args(args)
        .output()
        .map_err(|error| format!("execute git {:?}: {error}", args))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git {:?} failed: {stderr}", args));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn current_head_sha(repo_dir: &Path) -> PerfResult<String> {
    run_git_capture(repo_dir, &["rev-parse", "HEAD"])
}

fn current_branch_name(repo_dir: &Path) -> PerfResult<String> {
    run_git_capture(repo_dir, &["symbolic-ref", "--short", "HEAD"])
}

fn load_repo_counts(db_path: &Path) -> PerfResult<(i64, i64, i64)> {
    let conn = Connection::open(db_path)
        .map_err(|error| format!("open database {}: {error}", db_path.to_string_lossy()))?;
    let commits = conn
        .query_row("SELECT COUNT(*) FROM commits", [], |row| row.get(0))
        .map_err(|error| format!("count commits: {error}"))?;
    let edges = conn
        .query_row("SELECT COUNT(*) FROM commit_edges", [], |row| row.get(0))
        .map_err(|error| format!("count commit_edges: {error}"))?;
    let file_changes = conn
        .query_row("SELECT COUNT(*) FROM file_changes", [], |row| row.get(0))
        .map_err(|error| format!("count file_changes: {error}"))?;
    Ok((commits, edges, file_changes))
}

fn load_in_memory_graph_context(
    db_path: &Path,
    fixture: &FixtureSummary,
) -> PerfResult<InMemoryGraphContext> {
    let conn = Connection::open(db_path)
        .map_err(|error| format!("open database {}: {error}", db_path.to_string_lossy()))?;
    let repo_id = load_repo_id(&conn)?;
    let graph = InMemoryAdjacencyGraph::hydrate(&conn, repo_id)?;

    let head_rowid = load_commit_rowid_for_sha(&conn, repo_id, fixture.head_sha.as_str())?;
    let root_rowid = load_commit_rowid_for_sha(&conn, repo_id, fixture.root_sha.as_str())?;
    let pre_merge_main_rowid =
        load_commit_rowid_for_sha(&conn, repo_id, fixture.pre_merge_main_sha.as_str())?;
    let feature_tip_rowid =
        load_commit_rowid_for_sha(&conn, repo_id, fixture.feature_tip_sha.as_str())?;

    let head_node = graph
        .node_for_rowid(head_rowid)
        .ok_or_else(|| format!("missing in-memory node for head commit rowid: {head_rowid}"))?;
    let root_node = graph
        .node_for_rowid(root_rowid)
        .ok_or_else(|| format!("missing in-memory node for root commit rowid: {root_rowid}"))?;
    let pre_merge_main_node = graph.node_for_rowid(pre_merge_main_rowid).ok_or_else(|| {
        format!("missing in-memory node for pre-merge commit rowid: {pre_merge_main_rowid}")
    })?;
    let feature_tip_node = graph.node_for_rowid(feature_tip_rowid).ok_or_else(|| {
        format!("missing in-memory node for feature-tip commit rowid: {feature_tip_rowid}")
    })?;

    Ok(InMemoryGraphContext {
        graph,
        root_node,
        pre_merge_main_node,
        feature_tip_node,
        head_node,
    })
}

fn load_repo_id(conn: &Connection) -> PerfResult<i64> {
    conn.query_row(
        "SELECT id FROM repositories ORDER BY id ASC LIMIT 1",
        [],
        |row| row.get(0),
    )
    .map_err(|error| format!("resolve repository id: {error}"))
}

fn load_commit_rowid_for_sha(conn: &Connection, repo_id: i64, sha: &str) -> PerfResult<i64> {
    conn.query_row(
        r#"
        SELECT commit_id
        FROM commits
        WHERE repo_id = ?1 AND sha = ?2
        LIMIT 1
    "#,
        params![repo_id, sha],
        |row| row.get(0),
    )
    .map_err(|error| format!("resolve commit rowid for sha {sha}: {error}"))
}

fn measure_latencies(
    runs: usize,
    mut operation: impl FnMut() -> PerfResult<()>,
) -> PerfResult<LatencySummary> {
    operation()?;

    let mut samples = Vec::with_capacity(runs);
    for _ in 0..runs {
        let started = Instant::now();
        operation()?;
        samples.push(started.elapsed());
    }

    Ok(summarize(samples))
}

fn summarize(samples: Vec<Duration>) -> LatencySummary {
    if samples.is_empty() {
        return LatencySummary {
            runs: 0,
            min_ms: 0.0,
            median_ms: 0.0,
            p95_ms: 0.0,
            max_ms: 0.0,
            mean_ms: 0.0,
        };
    }

    let mut nanos: Vec<u128> = samples.iter().map(Duration::as_nanos).collect();
    nanos.sort_unstable();

    let len = nanos.len();
    let sum_nanos: u128 = nanos.iter().copied().sum();
    let min = nanos[0];
    let median = nanos[len / 2];
    let p95_index = ((len * 95).div_ceil(100)).saturating_sub(1);
    let p95 = nanos[p95_index.min(len - 1)];
    let max = nanos[len - 1];
    let mean = sum_nanos as f64 / len as f64;

    LatencySummary {
        runs: len,
        min_ms: nanos_to_ms(min),
        median_ms: nanos_to_ms(median),
        p95_ms: nanos_to_ms(p95),
        max_ms: nanos_to_ms(max),
        mean_ms: nanos_to_ms_f64(mean),
    }
}

fn format_report(input: &ReportInput<'_>) -> String {
    let full_seconds = input.phase_timing.full_index.as_secs_f64();
    let incremental_seconds = input.phase_timing.incremental_index.as_secs_f64();
    let full_commit_rate = safe_rate(input.full_stats.commits_indexed as f64, full_seconds);
    let full_edge_rate = safe_rate(input.full_stats.edges_indexed as f64, full_seconds);
    let full_file_change_rate =
        safe_rate(input.full_stats.file_changes_indexed as f64, full_seconds);
    let incremental_commit_rate = safe_rate(
        input.incremental_stats.commits_indexed as f64,
        incremental_seconds,
    );
    let incremental_edge_rate = safe_rate(
        input.incremental_stats.edges_indexed as f64,
        incremental_seconds,
    );
    let incremental_file_change_rate = safe_rate(
        input.incremental_stats.file_changes_indexed as f64,
        incremental_seconds,
    );
    let in_memory_bfs_speedup = safe_ratio(
        input.query.bfs.median_ms,
        input.in_memory_query.bfs_parents.median_ms,
    );
    let in_memory_shortest_parents_speedup = safe_ratio(
        input.query.shortest_parents.median_ms,
        input.in_memory_query.shortest_parents.median_ms,
    );
    let in_memory_shortest_both_speedup = safe_ratio(
        input.query.shortest_both.median_ms,
        input.in_memory_query.shortest_both.median_ms,
    );

    let attributed_core = input.phase_timing.fixture_generation.as_secs_f64()
        + input.phase_timing.fixture_summary.as_secs_f64()
        + input.phase_timing.repository_scan.as_secs_f64()
        + input.phase_timing.full_index.as_secs_f64()
        + input.phase_timing.incremental_fixture_append.as_secs_f64()
        + input.phase_timing.incremental_index.as_secs_f64()
        + input.phase_timing.query_benchmark.as_secs_f64()
        + input.phase_timing.in_memory_graph_hydration.as_secs_f64()
        + input.phase_timing.in_memory_query_benchmark.as_secs_f64();
    let unattributed_core =
        (input.phase_timing.core_end_to_end.as_secs_f64() - attributed_core).max(0.0);

    format!(
        "=== GitGraph Lab Performance Report ===
fixture_root: {}
config:
  cycles: {}
  feature_commits_per_cycle: {}
  incremental_commits: {}
  query_runs: {}
  window_limit: {}
  traversal_max_nodes: {}

workload_shape:
  estimated_git_process_spawns: {}

phase_timing_ms:
  fixture_generation: {:.3}
  fixture_summary: {:.3}
  repository_scan: {:.3}
  full_index: {:.3}
  incremental_fixture_append: {:.3}
  incremental_index: {:.3}
  query_benchmark: {:.3}
  in_memory_graph_hydration: {:.3}
  in_memory_query_benchmark: {:.3}
  core_end_to_end: {:.3}
  unattributed_core: {:.3}

phase_memory_bytes:
  fixture_generation_rss: {}
  fixture_summary_rss: {}
  repository_scan_rss: {}
  full_index_rss: {}
  incremental_fixture_append_rss: {}
  incremental_index_rss: {}
  query_benchmark_rss: {}
  in_memory_graph_hydration_rss: {}
  in_memory_query_benchmark_rss: {}
  final_rss: {}
  peak_observed_rss: {}

dataset_counts:
  commits_in_db: {}
  edges_in_db: {}
  file_changes_in_db: {}

full_index:
  duration_ms: {:.3}
  commits_indexed: {}
  edges_indexed: {}
  file_changes_indexed: {}
  commit_throughput_per_sec: {:.3}
  edge_throughput_per_sec: {:.3}
  file_change_throughput_per_sec: {:.3}

incremental_index:
  duration_ms: {:.3}
  commits_indexed: {}
  edges_indexed: {}
  file_changes_indexed: {}
  commit_throughput_per_sec: {:.3}
  edge_throughput_per_sec: {:.3}
  file_change_throughput_per_sec: {:.3}

query_latency_ms:
  commit_window: {}
  bfs_parents: {}
  dfs_parents: {}
  topological_parents: {}
  shortest_path_parents: {}
  shortest_path_both: {}

in_memory_adjacency_graph:
  nodes: {}
  directed_edges: {}

in_memory_adjacency_query_latency_ms:
  bfs_parents: {}
  shortest_path_parents: {}
  shortest_path_both: {}

in_memory_vs_sqlite_median_speedup:
  bfs_parents: {:.3}x
  shortest_path_parents: {:.3}x
  shortest_path_both: {:.3}x
",
        input.fixture_root.to_string_lossy(),
        input.config.cycles,
        input.config.feature_commits,
        input.config.incremental_commits,
        input.config.query_runs,
        input.config.window_limit,
        input.config.max_nodes,
        input.estimated_git_process_spawns,
        duration_ms(input.phase_timing.fixture_generation),
        duration_ms(input.phase_timing.fixture_summary),
        duration_ms(input.phase_timing.repository_scan),
        duration_ms(input.phase_timing.full_index),
        duration_ms(input.phase_timing.incremental_fixture_append),
        duration_ms(input.phase_timing.incremental_index),
        duration_ms(input.phase_timing.query_benchmark),
        duration_ms(input.phase_timing.in_memory_graph_hydration),
        duration_ms(input.phase_timing.in_memory_query_benchmark),
        duration_ms(input.phase_timing.core_end_to_end),
        unattributed_core * 1_000.0,
        optional_u64_display(input.phase_memory.fixture_generation_rss_bytes),
        optional_u64_display(input.phase_memory.fixture_summary_rss_bytes),
        optional_u64_display(input.phase_memory.repository_scan_rss_bytes),
        optional_u64_display(input.phase_memory.full_index_rss_bytes),
        optional_u64_display(input.phase_memory.incremental_fixture_append_rss_bytes),
        optional_u64_display(input.phase_memory.incremental_index_rss_bytes),
        optional_u64_display(input.phase_memory.query_benchmark_rss_bytes),
        optional_u64_display(input.phase_memory.in_memory_graph_hydration_rss_bytes),
        optional_u64_display(input.phase_memory.in_memory_query_benchmark_rss_bytes),
        optional_u64_display(input.phase_memory.final_rss_bytes),
        optional_u64_display(input.phase_memory.peak_observed_rss_bytes),
        input.counts.0,
        input.counts.1,
        input.counts.2,
        duration_ms(input.phase_timing.full_index),
        input.full_stats.commits_indexed,
        input.full_stats.edges_indexed,
        input.full_stats.file_changes_indexed,
        full_commit_rate,
        full_edge_rate,
        full_file_change_rate,
        duration_ms(input.phase_timing.incremental_index),
        input.incremental_stats.commits_indexed,
        input.incremental_stats.edges_indexed,
        input.incremental_stats.file_changes_indexed,
        incremental_commit_rate,
        incremental_edge_rate,
        incremental_file_change_rate,
        format_latency(input.query.commit_window),
        format_latency(input.query.bfs),
        format_latency(input.query.dfs),
        format_latency(input.query.topological),
        format_latency(input.query.shortest_parents),
        format_latency(input.query.shortest_both),
        input.in_memory_graph.nodes,
        input.in_memory_graph.directed_edges,
        format_latency(input.in_memory_query.bfs_parents),
        format_latency(input.in_memory_query.shortest_parents),
        format_latency(input.in_memory_query.shortest_both),
        in_memory_bfs_speedup,
        in_memory_shortest_parents_speedup,
        in_memory_shortest_both_speedup
    )
}

fn format_latency(summary: &LatencySummary) -> String {
    format!(
        "runs={} min={:.3} median={:.3} p95={:.3} max={:.3} mean={:.3}",
        summary.runs,
        summary.min_ms,
        summary.median_ms,
        summary.p95_ms,
        summary.max_ms,
        summary.mean_ms
    )
}

fn safe_rate(numerator: f64, denominator_seconds: f64) -> f64 {
    if denominator_seconds <= 0.0 {
        return 0.0;
    }
    numerator / denominator_seconds
}

fn safe_ratio(numerator: f64, denominator: f64) -> f64 {
    if denominator <= 0.0 {
        return 0.0;
    }
    numerator / denominator
}

fn nanos_to_ms(nanos: u128) -> f64 {
    nanos as f64 / 1_000_000.0
}

fn nanos_to_ms_f64(nanos: f64) -> f64 {
    nanos / 1_000_000.0
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn optional_u64_display(value: Option<u64>) -> String {
    value
        .map(|bytes| bytes.to_string())
        .unwrap_or_else(|| "n/a".to_string())
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn current_rss_bytes() -> Option<u64> {
    memory_stats().map(|stats| stats.physical_mem as u64)
}

fn estimate_git_process_spawns(config: &Config) -> usize {
    let fixture_generation = 6usize;
    let fixture_summary = 3usize;
    let incremental_refresh = if config.incremental_commits == 0 {
        1
    } else {
        5
    };
    fixture_generation
        .saturating_add(fixture_summary)
        .saturating_add(incremental_refresh)
}

fn log_phase_start(phase: &str) {
    eprintln!("[perf-report] phase={phase} status=start");
}

fn log_phase_done(phase: &str, duration: Duration) {
    eprintln!(
        "[perf-report] phase={phase} status=done duration_ms={:.3}",
        duration_ms(duration)
    );
}
