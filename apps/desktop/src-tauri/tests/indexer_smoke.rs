use std::fs;
use std::path::Path;
use std::process::Command;

use gitgraph_lab_core::application::commands::{
    ensure_file_change_stats_for_commits, get_commit_window, get_graph_traversal,
    get_shortest_path, index_commits, index_commits_with_mode, scan_repository,
};
use gitgraph_lab_core::domain::models::{GraphDirection, GraphTraversalStrategy, IndexingMode};
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
    assert!(
        incremental_stats.commits_indexed <= 2,
        "incremental checkpoint hide boundary should avoid broad replay"
    );
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

#[test]
fn ingestion_diff_subphase_telemetry_is_consistent_for_zero_and_single_delta_commits() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("create temp dir: {error}"));
    let repo_dir = temp.path().join("repo");
    fs::create_dir_all(&repo_dir).unwrap_or_else(|error| panic!("create repo dir: {error}"));
    init_git_repo(&repo_dir);

    write_file(repo_dir.join("README.md"), "v1\n");
    git_commit(&repo_dir, "v1");

    write_file(repo_dir.join("README.md"), "v2\n");
    git_commit(&repo_dir, "v2");

    run_git(&repo_dir, &["commit", "--allow-empty", "-m", "empty"]);

    let db_path = temp.path().join("index.db");
    let repo_str = repo_dir.to_string_lossy().to_string();
    let db_str = db_path.to_string_lossy().to_string();

    scan_repository(&repo_str, Some(&db_str)).unwrap_or_else(|error| panic!("scan: {error}"));
    let stats = index_commits_with_mode(&repo_str, Some(&db_str), IndexingMode::Full)
        .unwrap_or_else(|error| panic!("full index: {error}"));

    let telemetry = stats.ingestion_phase_timing_ms;
    let subphase_sum = telemetry.file_diff_build
        + telemetry.file_diff_rename_find
        + telemetry.file_diff_delta_emit;
    let extract_delta = (telemetry.file_diff_extract - subphase_sum).abs();
    assert!(
        extract_delta < 0.01,
        "expected file_diff_extract ({}) to match sub-phase sum ({})",
        telemetry.file_diff_extract,
        subphase_sum
    );
}

#[test]
fn indexing_large_history_keeps_file_change_ingestion_correct() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("create temp dir: {error}"));
    let repo_dir = temp.path().join("repo");
    fs::create_dir_all(&repo_dir).unwrap_or_else(|error| panic!("create repo dir: {error}"));
    init_git_repo(&repo_dir);

    let commit_count = 72usize;
    for index in 0..commit_count {
        write_file(repo_dir.join("README.md"), &format!("commit-{index:03}\n"));
        git_commit(&repo_dir, &format!("commit-{index:03}"));
    }

    let db_path = temp.path().join("index.db");
    let repo_str = repo_dir.to_string_lossy().to_string();
    let db_str = db_path.to_string_lossy().to_string();

    scan_repository(&repo_str, Some(&db_str)).unwrap_or_else(|error| panic!("scan: {error}"));
    let stats = index_commits(&repo_str, Some(&db_str))
        .unwrap_or_else(|error| panic!("index large history: {error}"));
    assert_eq!(stats.mode, IndexingMode::Full);
    assert_eq!(stats.commits_indexed, commit_count);
    assert!(stats.file_changes_indexed >= commit_count);

    let conn = Connection::open(db_path).unwrap_or_else(|error| panic!("open db: {error}"));
    let db_commit_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM commits", [], |row| row.get(0))
        .unwrap_or_else(|error| panic!("query commit count: {error}"));
    assert_eq!(db_commit_count, commit_count as i64);

    let db_file_change_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM file_changes", [], |row| row.get(0))
        .unwrap_or_else(|error| panic!("query file_changes count: {error}"));
    assert!(db_file_change_count >= commit_count as i64);
}

#[test]
fn file_change_stats_are_deferred_and_jit_enrichable() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("create temp dir: {error}"));
    let repo_dir = temp.path().join("repo");
    fs::create_dir_all(&repo_dir).unwrap_or_else(|error| panic!("create repo dir: {error}"));
    init_git_repo(&repo_dir);

    write_file(repo_dir.join("README.md"), "v1\n");
    git_commit(&repo_dir, "v1");

    write_file(repo_dir.join("README.md"), "v2\nline-2\n");
    git_commit(&repo_dir, "v2");

    let db_path = temp.path().join("index.db");
    let repo_str = repo_dir.to_string_lossy().to_string();
    let db_str = db_path.to_string_lossy().to_string();

    scan_repository(&repo_str, Some(&db_str)).unwrap_or_else(|error| panic!("scan: {error}"));
    index_commits(&repo_str, Some(&db_str)).unwrap_or_else(|error| panic!("index: {error}"));

    let conn = Connection::open(&db_path).unwrap_or_else(|error| panic!("open db: {error}"));
    let pending_before: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM file_changes WHERE stats_state = 'pending'",
            [],
            |row| row.get(0),
        )
        .unwrap_or_else(|error| panic!("query pending file_changes before jit: {error}"));
    assert!(pending_before >= 1);
    let pending_with_eager_stats: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM file_changes WHERE stats_state = 'pending' AND (additions IS NOT NULL OR deletions IS NOT NULL)",
            [],
            |row| row.get(0),
        )
        .unwrap_or_else(|error| panic!("query pending rows with eager line stats: {error}"));
    assert_eq!(pending_with_eager_stats, 0);
    drop(conn);

    let window = get_commit_window(&repo_str, Some(&db_str), Some(1), None)
        .unwrap_or_else(|error| panic!("window: {error}"));
    let head_sha = window
        .commits
        .first()
        .map(|commit| commit.sha.clone())
        .unwrap_or_else(|| panic!("expected at least one commit in window"));

    let enriched = ensure_file_change_stats_for_commits(
        &repo_str,
        Some(&db_str),
        std::slice::from_ref(&head_sha),
        Some(64),
    )
    .unwrap_or_else(|error| panic!("jit enrich: {error}"));
    assert!(enriched >= 1);

    let conn = Connection::open(db_path).unwrap_or_else(|error| panic!("open db: {error}"));
    let ready_after: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM file_changes WHERE commit_sha = ?1 AND stats_state = 'ready' AND additions IS NOT NULL AND deletions IS NOT NULL",
            [head_sha.as_str()],
            |row| row.get(0),
        )
        .unwrap_or_else(|error| panic!("query ready file_changes after jit: {error}"));
    assert!(ready_after >= 1);
}

#[test]
fn jit_enrichment_is_commit_scoped() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("create temp dir: {error}"));
    let repo_dir = temp.path().join("repo");
    fs::create_dir_all(&repo_dir).unwrap_or_else(|error| panic!("create repo dir: {error}"));
    init_git_repo(&repo_dir);

    write_file(repo_dir.join("README.md"), "v1\n");
    git_commit(&repo_dir, "v1");

    write_file(repo_dir.join("README.md"), "v2\nline-2\n");
    git_commit(&repo_dir, "v2");

    write_file(repo_dir.join("README.md"), "v3\nline-2\nline-3\n");
    git_commit(&repo_dir, "v3");

    let db_path = temp.path().join("index.db");
    let repo_str = repo_dir.to_string_lossy().to_string();
    let db_str = db_path.to_string_lossy().to_string();

    scan_repository(&repo_str, Some(&db_str)).unwrap_or_else(|error| panic!("scan: {error}"));
    index_commits(&repo_str, Some(&db_str)).unwrap_or_else(|error| panic!("index: {error}"));

    let window = get_commit_window(&repo_str, Some(&db_str), Some(3), None)
        .unwrap_or_else(|error| panic!("window: {error}"));
    assert!(
        window.commits.len() >= 2,
        "expected at least two commits for scoped JIT assertion"
    );
    let head_sha = window.commits[0].sha.clone();
    let previous_sha = window.commits[1].sha.clone();

    let conn = Connection::open(&db_path).unwrap_or_else(|error| panic!("open db: {error}"));
    let previous_pending_before: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM file_changes WHERE commit_sha = ?1 AND stats_state = 'pending'",
            [previous_sha.as_str()],
            |row| row.get(0),
        )
        .unwrap_or_else(|error| panic!("query previous commit pending before jit: {error}"));
    assert!(previous_pending_before >= 1);
    drop(conn);

    let enriched = ensure_file_change_stats_for_commits(
        &repo_str,
        Some(&db_str),
        std::slice::from_ref(&head_sha),
        Some(64),
    )
    .unwrap_or_else(|error| panic!("jit enrich: {error}"));
    assert!(enriched >= 1);

    let conn = Connection::open(db_path).unwrap_or_else(|error| panic!("open db: {error}"));
    let head_ready_after: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM file_changes WHERE commit_sha = ?1 AND stats_state = 'ready' AND additions IS NOT NULL AND deletions IS NOT NULL",
            [head_sha.as_str()],
            |row| row.get(0),
        )
        .unwrap_or_else(|error| panic!("query head commit ready after jit: {error}"));
    assert!(head_ready_after >= 1);

    let previous_pending_after: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM file_changes WHERE commit_sha = ?1 AND stats_state = 'pending'",
            [previous_sha.as_str()],
            |row| row.get(0),
        )
        .unwrap_or_else(|error| panic!("query previous commit pending after jit: {error}"));
    assert!(previous_pending_after >= 1);
}

#[test]
fn rename_commit_is_recorded_as_rename_change() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("create temp dir: {error}"));
    let repo_dir = temp.path().join("repo");
    fs::create_dir_all(&repo_dir).unwrap_or_else(|error| panic!("create repo dir: {error}"));
    init_git_repo(&repo_dir);

    write_file(repo_dir.join("alpha.txt"), "alpha\n");
    git_commit(&repo_dir, "add alpha");

    run_git(&repo_dir, &["mv", "alpha.txt", "beta.txt"]);
    git_commit(&repo_dir, "rename alpha to beta");
    let rename_sha = current_head_sha(&repo_dir);

    let db_path = temp.path().join("index.db");
    let repo_str = repo_dir.to_string_lossy().to_string();
    let db_str = db_path.to_string_lossy().to_string();

    scan_repository(&repo_str, Some(&db_str)).unwrap_or_else(|error| panic!("scan: {error}"));
    index_commits(&repo_str, Some(&db_str)).unwrap_or_else(|error| panic!("index: {error}"));

    let conn = Connection::open(db_path).unwrap_or_else(|error| panic!("open db: {error}"));
    let mut stmt = conn
        .prepare(
            r#"
            SELECT file_path, change_type
            FROM file_changes
            WHERE commit_sha = ?1
        "#,
        )
        .unwrap_or_else(|error| panic!("prepare file-change rename query: {error}"));
    let rows = stmt
        .query_map([rename_sha.as_str()], |row| {
            let file_path: String = row.get(0)?;
            let change_type: String = row.get(1)?;
            Ok((file_path, change_type))
        })
        .unwrap_or_else(|error| panic!("query rename file changes: {error}"));

    let mut rename_entry_found = false;
    for row in rows {
        let (file_path, change_type) =
            row.unwrap_or_else(|error| panic!("read rename file-change row: {error}"));
        if change_type == "rename"
            && file_path.contains("alpha.txt")
            && file_path.contains("beta.txt")
        {
            rename_entry_found = true;
            break;
        }
    }

    assert!(
        rename_entry_found,
        "expected rename commit to persist rename change_type and path mapping"
    );
}

#[test]
fn graph_traversal_and_shortest_path_work() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("create temp dir: {error}"));
    let repo_dir = temp.path().join("repo");
    fs::create_dir_all(&repo_dir).unwrap_or_else(|error| panic!("create repo dir: {error}"));
    init_git_repo(&repo_dir);

    write_file(repo_dir.join("README.md"), "v1\n");
    git_commit(&repo_dir, "v1");
    let sha1 = current_head_sha(&repo_dir);

    write_file(repo_dir.join("README.md"), "v2\n");
    git_commit(&repo_dir, "v2");
    let sha2 = current_head_sha(&repo_dir);

    write_file(repo_dir.join("README.md"), "v3\n");
    git_commit(&repo_dir, "v3");
    let sha3 = current_head_sha(&repo_dir);

    let db_path = temp.path().join("index.db");
    let repo_str = repo_dir.to_string_lossy().to_string();
    let db_str = db_path.to_string_lossy().to_string();

    scan_repository(&repo_str, Some(&db_str)).unwrap_or_else(|error| panic!("scan: {error}"));
    index_commits(&repo_str, Some(&db_str)).unwrap_or_else(|error| panic!("index: {error}"));

    let traversal = get_graph_traversal(
        &repo_str,
        Some(&db_str),
        &sha3,
        GraphTraversalStrategy::Bfs,
        GraphDirection::Parents,
        Some(10),
    )
    .unwrap_or_else(|error| panic!("bfs traversal: {error}"));
    assert!(!traversal.truncated);
    assert_eq!(traversal.nodes.len(), 3);
    assert_eq!(traversal.nodes[0].sha, sha3);
    assert_eq!(traversal.nodes[1].sha, sha2);
    assert_eq!(traversal.nodes[2].sha, sha1);

    let topological = get_graph_traversal(
        &repo_str,
        Some(&db_str),
        &sha3,
        GraphTraversalStrategy::Topological,
        GraphDirection::Parents,
        Some(10),
    )
    .unwrap_or_else(|error| panic!("topological traversal: {error}"));
    assert_eq!(topological.nodes.len(), 3);
    assert_eq!(topological.nodes[0].sha, sha3);
    assert_eq!(topological.nodes[1].sha, sha2);
    assert_eq!(topological.nodes[2].sha, sha1);

    let path_to_ancestor = get_shortest_path(
        &repo_str,
        Some(&db_str),
        &sha3,
        &sha1,
        GraphDirection::Parents,
    )
    .unwrap_or_else(|error| panic!("path to ancestor: {error}"));
    assert!(path_to_ancestor.found);
    assert_eq!(
        path_to_ancestor.path,
        vec![sha3.clone(), sha2.clone(), sha1.clone()]
    );

    let path_to_descendant = get_shortest_path(
        &repo_str,
        Some(&db_str),
        &sha1,
        &sha3,
        GraphDirection::Children,
    )
    .unwrap_or_else(|error| panic!("path to descendant: {error}"));
    assert!(path_to_descendant.found);
    assert_eq!(
        path_to_descendant.path,
        vec![sha1.clone(), sha2.clone(), sha3.clone()]
    );

    let no_parent_path = get_shortest_path(
        &repo_str,
        Some(&db_str),
        &sha1,
        &sha3,
        GraphDirection::Parents,
    )
    .unwrap_or_else(|error| panic!("no path query: {error}"));
    assert!(!no_parent_path.found);
    assert!(no_parent_path.path.is_empty());
    assert!(no_parent_path.visited_nodes >= 1);
}

#[test]
fn branching_graph_queries_work() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("create temp dir: {error}"));
    let repo_dir = temp.path().join("repo");
    fs::create_dir_all(&repo_dir).unwrap_or_else(|error| panic!("create repo dir: {error}"));
    init_git_repo(&repo_dir);

    write_file(repo_dir.join("README.md"), "base\n");
    git_commit(&repo_dir, "base");

    let default_branch = current_branch_name(&repo_dir);
    run_git(&repo_dir, &["checkout", "-b", "feature"]);
    write_file(repo_dir.join("feature.txt"), "feature-1\n");
    git_commit(&repo_dir, "feature-1");
    let feature_sha = current_head_sha(&repo_dir);

    run_git(&repo_dir, &["checkout", &default_branch]);
    write_file(repo_dir.join("main.txt"), "main-1\n");
    git_commit(&repo_dir, "main-1");
    let main_sha = current_head_sha(&repo_dir);

    run_git(
        &repo_dir,
        &["merge", "--no-ff", "feature", "-m", "merge feature"],
    );
    let merge_sha = current_head_sha(&repo_dir);

    let db_path = temp.path().join("index.db");
    let repo_str = repo_dir.to_string_lossy().to_string();
    let db_str = db_path.to_string_lossy().to_string();

    scan_repository(&repo_str, Some(&db_str)).unwrap_or_else(|error| panic!("scan: {error}"));
    index_commits(&repo_str, Some(&db_str)).unwrap_or_else(|error| panic!("index: {error}"));

    let mixed_direction = get_shortest_path(
        &repo_str,
        Some(&db_str),
        &main_sha,
        &feature_sha,
        GraphDirection::Both,
    )
    .unwrap_or_else(|error| panic!("mixed direction path: {error}"));
    assert!(mixed_direction.found);
    assert_eq!(mixed_direction.path.first(), Some(&main_sha));
    assert_eq!(mixed_direction.path.last(), Some(&feature_sha));
    assert_eq!(mixed_direction.path.len(), 3);

    let directional_miss = get_shortest_path(
        &repo_str,
        Some(&db_str),
        &main_sha,
        &feature_sha,
        GraphDirection::Parents,
    )
    .unwrap_or_else(|error| panic!("directional miss query: {error}"));
    assert!(!directional_miss.found);

    let truncated = get_graph_traversal(
        &repo_str,
        Some(&db_str),
        &merge_sha,
        GraphTraversalStrategy::Bfs,
        GraphDirection::Parents,
        Some(2),
    )
    .unwrap_or_else(|error| panic!("truncated traversal query: {error}"));
    assert!(truncated.truncated);
    assert_eq!(truncated.nodes.len(), 2);

    let invalid_topological = get_graph_traversal(
        &repo_str,
        Some(&db_str),
        &merge_sha,
        GraphTraversalStrategy::Topological,
        GraphDirection::Both,
        Some(10),
    )
    .unwrap_err();
    assert_eq!(invalid_topological.code, "invalid_argument");

    let missing_sha_path = get_shortest_path(
        &repo_str,
        Some(&db_str),
        "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
        &feature_sha,
        GraphDirection::Both,
    )
    .unwrap_err();
    assert_eq!(missing_sha_path.code, "invalid_argument");
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
    let _ = run_git_capture(repo_dir, args);
}

fn current_head_sha(repo_dir: &Path) -> String {
    run_git_capture(repo_dir, &["rev-parse", "HEAD"])
        .trim()
        .to_string()
}

fn current_branch_name(repo_dir: &Path) -> String {
    run_git_capture(repo_dir, &["symbolic-ref", "--short", "HEAD"])
        .trim()
        .to_string()
}

fn run_git_capture(repo_dir: &Path, args: &[&str]) -> String {
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

    String::from_utf8_lossy(&output.stdout).to_string()
}

fn write_file(path: impl AsRef<Path>, contents: &str) {
    fs::write(path, contents).unwrap_or_else(|error| panic!("write file: {error}"));
}
