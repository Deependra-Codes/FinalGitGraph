use std::env;

use gitgraph_lab_core::application::commands::{
    drain_file_change_extraction_queue, enrich_pending_file_change_stats, get_commit_window,
    get_graph_traversal, get_shortest_path, index_commits_with_mode, scan_repository,
};
use gitgraph_lab_core::domain::models::{GraphDirection, GraphTraversalStrategy, IndexingMode};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!(
            "Usage:\n  cargo run -- scan <repo_path> [db_path]\n  cargo run -- index <repo_path> [db_path] [full|incremental]\n  cargo run -- enrich <repo_path> [max_items] [db_path]\n  cargo run -- drain <repo_path> [max_commits] [db_path]\n  cargo run -- window <repo_path> [limit] [db_path]\n  cargo run -- traverse <repo_path> <start_sha> [bfs|dfs|topological] [parents|children|both] [max_nodes] [db_path]\n  cargo run -- path <repo_path> <from_sha> <to_sha> [parents|children|both] [db_path]"
        );
        std::process::exit(1);
    }

    let action = &args[1];
    let repo_path = &args[2];

    match action.as_str() {
        "scan" => run_scan(repo_path, args.get(3).map(String::as_str)),
        "index" => run_index(repo_path, &args[3..]),
        "enrich" => run_enrich(repo_path, &args[3..]),
        "drain" => run_drain(repo_path, &args[3..]),
        "window" => run_window(repo_path, &args[3..]),
        "traverse" => run_traverse(repo_path, &args[3..]),
        "path" => run_path(repo_path, &args[3..]),
        _ => {
            eprintln!(
                "Unknown command: {action}. Use 'scan', 'index', 'enrich', 'drain', 'window', 'traverse', or 'path'."
            );
            std::process::exit(1);
        }
    }
}

fn run_scan(repo_path: &str, db_path: Option<&str>) {
    match scan_repository(repo_path, db_path) {
        Ok(summary) => println!(
            "Scanned repo '{}' at {} (git dir: {})",
            summary.name, summary.path, summary.git_dir
        ),
        Err(error) => {
            eprintln!("scan failed: {error}");
            std::process::exit(1);
        }
    }
}

fn run_index(repo_path: &str, tail_args: &[String]) {
    let mut db_path = None;
    let mut mode = IndexingMode::Full;

    if let Some(arg1) = tail_args.first() {
        if let Some(parsed_mode) = IndexingMode::parse(arg1) {
            mode = parsed_mode;
        } else {
            db_path = Some(arg1.as_str());
            if let Some(arg2) = tail_args.get(1) {
                if let Some(parsed_mode) = IndexingMode::parse(arg2) {
                    mode = parsed_mode;
                } else {
                    eprintln!("invalid index mode '{arg2}'. Use full or incremental.");
                    std::process::exit(1);
                }
            }
        }
    }

    match index_commits_with_mode(repo_path, db_path, mode) {
        Ok(stats) => {
            println!(
                "Indexed mode={:?} repo_id={} commits={} edges={} file_changes={} refs={} at epoch={}",
                stats.mode,
                stats.repo_id,
                stats.commits_indexed,
                stats.edges_indexed,
                stats.file_changes_indexed,
                stats.refs_indexed,
                stats.indexed_at_epoch
            );
            println!(
                "Commit throughput seen_commits={} indexed_commits={} cps_seen={:.3} cps_indexed={:.3} ms_per_commit_seen={:.3} ms_per_commit_indexed={:.3}",
                stats.commit_throughput.commits_seen,
                stats.commit_throughput.commits_indexed,
                stats.commit_throughput.commits_per_sec_seen,
                stats.commit_throughput.commits_per_sec_indexed,
                stats.commit_throughput.ms_per_commit_seen,
                stats.commit_throughput.ms_per_commit_indexed
            );
            println!(
                "Commit phase throughput (cps): commit_walk={:.3} commit_upsert={:.3} file_diff_extract={:.3} file_change_upsert={:.3} tx_commit={:.3}",
                stats.commit_throughput.commit_walk_commits_per_sec_seen,
                stats.commit_throughput.commit_upsert_commits_per_sec_seen,
                stats.commit_throughput.file_diff_extract_commits_per_sec_seen,
                stats.commit_throughput.file_change_upsert_commits_per_sec_seen,
                stats.commit_throughput.tx_commit_commits_per_sec_seen
            );
            println!(
                "Commit bottleneck phase={} phase_ms={:.3} phase_share={:.3}",
                stats.commit_throughput.bottleneck_phase,
                stats.commit_throughput.bottleneck_phase_ms,
                stats
                    .commit_throughput
                    .bottleneck_phase_share_of_tracked_time
            );
        }
        Err(error) => {
            eprintln!("index failed: {error}");
            std::process::exit(1);
        }
    }
}

fn run_enrich(repo_path: &str, tail_args: &[String]) {
    let mut max_items: Option<usize> = None;
    let mut db_path = None;

    if let Some(arg1) = tail_args.first() {
        if let Ok(parsed_max) = arg1.parse::<usize>() {
            max_items = Some(parsed_max);
            db_path = tail_args.get(1).map(String::as_str);
        } else {
            db_path = Some(arg1.as_str());
            if let Some(arg2) = tail_args.get(1) {
                match arg2.parse::<usize>() {
                    Ok(parsed_max) => max_items = Some(parsed_max),
                    Err(_) => {
                        eprintln!("invalid max_items '{arg2}'. Use a positive integer.");
                        std::process::exit(1);
                    }
                }
            }
        }
    }

    match enrich_pending_file_change_stats(repo_path, db_path, max_items) {
        Ok(updated) => println!("Enriched file-change stats rows={updated}"),
        Err(error) => {
            eprintln!("enrich failed: {error}");
            std::process::exit(1);
        }
    }
}

fn run_drain(repo_path: &str, tail_args: &[String]) {
    let mut max_commits: Option<usize> = None;
    let mut db_path = None;

    if let Some(arg1) = tail_args.first() {
        if let Ok(parsed_max) = arg1.parse::<usize>() {
            max_commits = Some(parsed_max);
            db_path = tail_args.get(1).map(String::as_str);
        } else {
            db_path = Some(arg1.as_str());
            if let Some(arg2) = tail_args.get(1) {
                match arg2.parse::<usize>() {
                    Ok(parsed_max) => max_commits = Some(parsed_max),
                    Err(_) => {
                        eprintln!("invalid max_commits '{arg2}'. Use a positive integer.");
                        std::process::exit(1);
                    }
                }
            }
        }
    }

    match drain_file_change_extraction_queue(repo_path, db_path, max_commits) {
        Ok(stats) => println!(
            "Drained file-change queue commits={} failed={} file_changes_indexed={} queue_depth={} oldest_queue_age_seconds={}",
            stats.drained_commits,
            stats.failed_commits,
            stats.file_changes_indexed,
            stats.queue.depth,
            stats.queue.oldest_queue_age_seconds
        ),
        Err(error) => {
            eprintln!("drain failed: {error}");
            std::process::exit(1);
        }
    }
}

fn run_window(repo_path: &str, tail_args: &[String]) {
    let mut limit = 100usize;
    let mut db_path = None;

    if let Some(arg1) = tail_args.first() {
        if let Ok(parsed_limit) = arg1.parse::<usize>() {
            limit = parsed_limit;
            db_path = tail_args.get(1).map(String::as_str);
        } else {
            db_path = Some(arg1.as_str());
        }
    }

    match get_commit_window(repo_path, db_path, Some(limit), None) {
        Ok(window) => {
            println!(
                "Commit window size={} has_more={} next_cursor={}",
                window.commits.len(),
                window.has_more,
                window
                    .next_cursor
                    .as_ref()
                    .map(|cursor| format!("{}:{}", cursor.committer_time, cursor.sha))
                    .unwrap_or_else(|| "none".to_string())
            );
            for commit in window.commits {
                println!(
                    "{} {} {}",
                    commit.committer_time, commit.sha, commit.subject
                );
            }
        }
        Err(error) => {
            eprintln!("window failed: {error}");
            std::process::exit(1);
        }
    }
}

fn run_traverse(repo_path: &str, tail_args: &[String]) {
    if tail_args.is_empty() {
        eprintln!(
            "traverse usage: cargo run -- traverse <repo_path> <start_sha> [bfs|dfs|topological] [parents|children|both] [max_nodes] [db_path]"
        );
        std::process::exit(1);
    }

    let start_sha = tail_args[0].as_str();
    let mut strategy = GraphTraversalStrategy::Bfs;
    let mut direction = GraphDirection::Parents;
    let mut max_nodes = 2_000usize;

    if let Some(strategy_raw) = tail_args.get(1) {
        match GraphTraversalStrategy::parse(strategy_raw) {
            Some(parsed) => strategy = parsed,
            None => {
                eprintln!(
                    "invalid traversal strategy '{strategy_raw}'. Use bfs, dfs, or topological."
                );
                std::process::exit(1);
            }
        }
    }

    if let Some(direction_raw) = tail_args.get(2) {
        match GraphDirection::parse(direction_raw) {
            Some(parsed) => direction = parsed,
            None => {
                eprintln!(
                    "invalid graph direction '{direction_raw}'. Use parents, children, or both."
                );
                std::process::exit(1);
            }
        }
    }

    if let Some(max_nodes_raw) = tail_args.get(3) {
        match max_nodes_raw.parse::<usize>() {
            Ok(parsed) => max_nodes = parsed,
            Err(_) => {
                eprintln!("invalid max_nodes '{max_nodes_raw}'. Use a positive integer.");
                std::process::exit(1);
            }
        }
    }

    let db_path = tail_args.get(4).map(String::as_str);

    match get_graph_traversal(
        repo_path,
        db_path,
        start_sha,
        strategy,
        direction,
        Some(max_nodes),
    ) {
        Ok(response) => {
            println!(
                "Traversal strategy={:?} direction={:?} nodes={} truncated={}",
                response.strategy,
                response.direction,
                response.nodes.len(),
                response.truncated
            );
            for node in response.nodes {
                println!("{} {}", node.depth, node.sha);
            }
        }
        Err(error) => {
            eprintln!("traverse failed: {error}");
            std::process::exit(1);
        }
    }
}

fn run_path(repo_path: &str, tail_args: &[String]) {
    if tail_args.len() < 2 {
        eprintln!(
            "path usage: cargo run -- path <repo_path> <from_sha> <to_sha> [parents|children|both] [db_path]"
        );
        std::process::exit(1);
    }

    let from_sha = tail_args[0].as_str();
    let to_sha = tail_args[1].as_str();
    let mut direction = GraphDirection::Parents;

    if let Some(direction_raw) = tail_args.get(2) {
        match GraphDirection::parse(direction_raw) {
            Some(parsed) => direction = parsed,
            None => {
                eprintln!(
                    "invalid graph direction '{direction_raw}'. Use parents, children, or both."
                );
                std::process::exit(1);
            }
        }
    }

    let db_path = tail_args.get(3).map(String::as_str);

    match get_shortest_path(repo_path, db_path, from_sha, to_sha, direction) {
        Ok(response) => {
            if response.found {
                println!(
                    "Shortest path direction={:?} length={} visited={}",
                    response.direction,
                    response.path.len(),
                    response.visited_nodes
                );
                for sha in response.path {
                    println!("{sha}");
                }
            } else {
                println!(
                    "No path found direction={:?} visited={}",
                    response.direction, response.visited_nodes
                );
            }
        }
        Err(error) => {
            eprintln!("path failed: {error}");
            std::process::exit(1);
        }
    }
}
