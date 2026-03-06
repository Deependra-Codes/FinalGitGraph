use std::fs;
use std::path::Path;
use std::process::Command;

use gitgraph_lab_core::application::commands::{
    get_commit_window, index_commits, index_commits_with_mode, scan_repository,
};
use gitgraph_lab_core::domain::models::IndexingMode;
use rusqlite::Connection;
use tempfile::TempDir;

#[test]
fn scan_and_index_repository_smoke_test() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("create temp dir: {error}"));
    let repo_dir = temp.path().join("repo");
    fs::create_dir_all(&repo_dir).unwrap_or_else(|error| panic!("create repo dir: {error}"));
    init_git_repo(&repo_dir);

    write_file(repo_dir.join("README.md"), "# GitGraph Lab\n");
    git_commit(&repo_dir, "initial commit");

    write_file(repo_dir.join("README.md"), "# GitGraph Lab\n\nphase-1\n");
    git_commit(&repo_dir, "update readme");

    let db_path = temp.path().join("index.db");
    let repo_str = repo_dir.to_string_lossy().to_string();
    let db_str = db_path.to_string_lossy().to_string();

    let summary = scan_repository(&repo_str, Some(&db_str))
        .unwrap_or_else(|error| panic!("scan repository: {error}"));
    assert!(summary.is_valid);
    assert_eq!(summary.name, "repo");

    let stats = index_commits(&repo_str, Some(&db_str))
        .unwrap_or_else(|error| panic!("index commits: {error}"));
    assert!(stats.commits_indexed >= 2);
    assert!(stats.edges_indexed >= 1);
    assert!(stats.file_changes_indexed >= 2);
    assert!(stats.refs_indexed >= 1);

    let conn = Connection::open(db_path).unwrap_or_else(|error| panic!("open db: {error}"));
    let commit_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM commits", [], |row| row.get(0))
        .unwrap_or_else(|error| panic!("query commit count: {error}"));
    assert!(commit_count >= 2);
}

#[test]
fn incremental_indexing_and_commit_window_work() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("create temp dir: {error}"));
    let repo_dir = temp.path().join("repo");
    fs::create_dir_all(&repo_dir).unwrap_or_else(|error| panic!("create repo dir: {error}"));
    init_git_repo(&repo_dir);

    write_file(repo_dir.join("README.md"), "v1\n");
    git_commit(&repo_dir, "v1");

    write_file(repo_dir.join("README.md"), "v2\n");
    git_commit(&repo_dir, "v2");

    let db_path = temp.path().join("index.db");
    let repo_str = repo_dir.to_string_lossy().to_string();
    let db_str = db_path.to_string_lossy().to_string();

    scan_repository(&repo_str, Some(&db_str)).unwrap_or_else(|error| panic!("scan: {error}"));
    index_commits(&repo_str, Some(&db_str))
        .unwrap_or_else(|error| panic!("initial full index: {error}"));

    write_file(repo_dir.join("README.md"), "v3\n");
    git_commit(&repo_dir, "v3");

    let incremental_stats =
        index_commits_with_mode(&repo_str, Some(&db_str), IndexingMode::Incremental)
            .unwrap_or_else(|error| panic!("incremental index: {error}"));
    assert_eq!(incremental_stats.mode, IndexingMode::Incremental);
    assert!(incremental_stats.commits_indexed >= 1);
    assert!(incremental_stats.file_changes_indexed >= 1);

    let conn = Connection::open(&db_path).unwrap_or_else(|error| panic!("open db: {error}"));
    let commit_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM commits", [], |row| row.get(0))
        .unwrap_or_else(|error| panic!("query commit count: {error}"));
    assert_eq!(commit_count, 3);

    let file_change_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM file_changes", [], |row| row.get(0))
        .unwrap_or_else(|error| panic!("query file_changes count: {error}"));
    assert!(file_change_count >= 3);

    let first_window = get_commit_window(&repo_str, Some(&db_str), Some(2), None)
        .unwrap_or_else(|error| panic!("first window: {error}"));
    assert_eq!(first_window.commits.len(), 2);
    assert!(first_window.has_more);

    let cursor = first_window
        .next_cursor
        .clone()
        .unwrap_or_else(|| panic!("expected next cursor"));
    let second_window = get_commit_window(&repo_str, Some(&db_str), Some(2), Some(cursor))
        .unwrap_or_else(|error| panic!("second window: {error}"));

    assert!(!second_window.commits.is_empty());
    assert_ne!(first_window.commits[0].sha, second_window.commits[0].sha);
}

fn init_git_repo(repo_dir: &Path) {
    run_git(repo_dir, &["init"]);
    run_git(repo_dir, &["config", "user.name", "GitGraph Lab"]);
    run_git(
        repo_dir,
        &["config", "user.email", "gitgraph-lab@example.com"],
    );
}

fn git_commit(repo_dir: &Path, message: &str) {
    run_git(repo_dir, &["add", "."]);
    run_git(repo_dir, &["commit", "-m", message]);
}

fn run_git(repo_dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_dir)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("execute git: {error}"));

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("git command failed: {:?}: {stderr}", args);
    }
}

fn write_file(path: impl AsRef<Path>, contents: &str) {
    fs::write(path, contents).unwrap_or_else(|error| panic!("write file: {error}"));
}
