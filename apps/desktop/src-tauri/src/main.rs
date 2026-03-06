use std::env;

use gitgraph_lab_core::application::commands::{
    get_commit_window, index_commits_with_mode, scan_repository,
};
use gitgraph_lab_core::domain::models::IndexingMode;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!(
            "Usage:\n  cargo run -- scan <repo_path> [db_path]\n  cargo run -- index <repo_path> [db_path] [full|incremental]\n  cargo run -- window <repo_path> [limit] [db_path]"
        );
        std::process::exit(1);
    }

    let action = &args[1];
    let repo_path = &args[2];

    match action.as_str() {
        "scan" => run_scan(repo_path, args.get(3).map(String::as_str)),
        "index" => run_index(repo_path, &args[3..]),
        "window" => run_window(repo_path, &args[3..]),
        _ => {
            eprintln!("Unknown command: {action}. Use 'scan', 'index', or 'window'.");
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
        Ok(stats) => println!(
            "Indexed mode={:?} repo_id={} commits={} edges={} file_changes={} refs={} at epoch={}",
            stats.mode,
            stats.repo_id,
            stats.commits_indexed,
            stats.edges_indexed,
            stats.file_changes_indexed,
            stats.refs_indexed,
            stats.indexed_at_epoch
        ),
        Err(error) => {
            eprintln!("index failed: {error}");
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
