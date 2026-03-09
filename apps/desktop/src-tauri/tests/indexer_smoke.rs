use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use gitgraph_lab_core::application::commands::{
    drain_file_change_extraction_queue, ensure_file_change_stats_for_commits, get_commit_window,
    get_graph_traversal, get_shortest_path, index_commits, index_commits_with_mode,
    scan_repository,
};
use gitgraph_lab_core::domain::models::{GraphDirection, GraphTraversalStrategy, IndexingMode};
use rusqlite::{params, Connection};
use tempfile::TempDir;

fn env_test_guard() -> std::sync::MutexGuard<'static, ()> {
    static ENV_GUARD: OnceLock<Mutex<()>> = OnceLock::new();
    ENV_GUARD
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| panic!("acquire env test guard: {error}"))
}

struct EnvVarRestore {
    key: &'static str,
    original: Option<String>,
}

impl EnvVarRestore {
    fn set(key: &'static str, value: &str) -> Self {
        let original = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, original }
    }

    fn capture(key: &'static str) -> Self {
        Self {
            key,
            original: std::env::var(key).ok(),
        }
    }
}

impl Drop for EnvVarRestore {
    fn drop(&mut self) {
        if let Some(original) = &self.original {
            std::env::set_var(self.key, original);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

#[test]
fn scan_and_index_repository_smoke_test() {
    let _guard = env_test_guard();
    let (temp, repo_dir) = create_test_repo();

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
    let _guard = env_test_guard();
    let (temp, repo_dir) = create_test_repo();

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
fn fast_mode_indexes_commits_edges_and_drains_extraction_queue() {
    let _guard = env_test_guard();
    let _fast_mode_env = EnvVarRestore::set("GITGRAPH_INDEX_FAST_MODE", "1");
    let _write_profile_env = EnvVarRestore::set("GITGRAPH_SQLITE_WRITE_PROFILE", "throughput");
    let _walk_sort_env = EnvVarRestore::set("GITGRAPH_COMMIT_WALK_SORT", "breadth_first");

    let (temp, repo_dir) = create_test_repo();
    write_file(repo_dir.join("README.md"), "v1\n");
    git_commit(&repo_dir, "v1");
    write_file(repo_dir.join("README.md"), "v2\n");
    git_commit(&repo_dir, "v2");
    write_file(repo_dir.join("README.md"), "v3\n");
    git_commit(&repo_dir, "v3");

    let db_path = temp.path().join("index.db");
    let repo_str = repo_dir.to_string_lossy().to_string();
    let db_str = db_path.to_string_lossy().to_string();

    scan_repository(&repo_str, Some(&db_str)).unwrap_or_else(|error| panic!("scan: {error}"));
    let stats = index_commits_with_mode(&repo_str, Some(&db_str), IndexingMode::Full)
        .unwrap_or_else(|error| panic!("fast-mode full index: {error}"));
    assert!(stats.commits_indexed >= 3);
    assert!(stats.edges_indexed >= 2);
    assert_eq!(
        stats.file_changes_indexed, 0,
        "fast mode should defer file-change extraction from main indexing path"
    );

    let conn = Connection::open(&db_path).unwrap_or_else(|error| panic!("open db: {error}"));
    let queue_depth_before: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM file_change_extraction_queue",
            [],
            |row| row.get(0),
        )
        .unwrap_or_else(|error| panic!("count extraction queue rows before drain: {error}"));
    assert!(queue_depth_before >= 1);
    drop(conn);

    let drain_stats = drain_file_change_extraction_queue(&repo_str, Some(&db_str), None)
        .unwrap_or_else(|error| panic!("drain extraction queue: {error}"));
    assert!(drain_stats.drained_commits >= 1);
    assert_eq!(drain_stats.failed_commits, 0);
    assert_eq!(drain_stats.queue.depth, 0);
    assert!(drain_stats.batch_stats.batches >= 1);
    assert!(drain_stats.phase_timing_ms.diff_extract >= 0.0);

    let conn = Connection::open(&db_path).unwrap_or_else(|error| panic!("open db: {error}"));
    let file_change_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM file_changes", [], |row| row.get(0))
        .unwrap_or_else(|error| panic!("count file_changes after drain: {error}"));
    assert!(file_change_count >= 3);
}

#[test]
fn fast_mode_drain_matches_synchronous_file_change_and_enrichment_state() {
    let _guard = env_test_guard();
    let (temp, repo_dir) = create_test_repo();

    write_file(repo_dir.join("README.md"), "v1\n");
    git_commit(&repo_dir, "v1");
    write_file(repo_dir.join("README.md"), "v2\n");
    git_commit(&repo_dir, "v2");
    run_git(&repo_dir, &["mv", "README.md", "README-renamed.md"]);
    git_commit(&repo_dir, "rename readme");
    write_file(repo_dir.join("README-renamed.md"), "v3\n");
    git_commit(&repo_dir, "v3");

    let baseline_db_path = temp.path().join("index-baseline.db");
    let fast_db_path = temp.path().join("index-fast.db");
    let repo_str = repo_dir.to_string_lossy().to_string();
    let baseline_db_str = baseline_db_path.to_string_lossy().to_string();
    let fast_db_str = fast_db_path.to_string_lossy().to_string();

    let _fast_mode_restore = EnvVarRestore::set("GITGRAPH_INDEX_FAST_MODE", "0");
    scan_repository(&repo_str, Some(&baseline_db_str))
        .unwrap_or_else(|error| panic!("baseline scan: {error}"));
    index_commits_with_mode(&repo_str, Some(&baseline_db_str), IndexingMode::Full)
        .unwrap_or_else(|error| panic!("baseline full index: {error}"));

    let baseline_file_changes = load_file_change_snapshot(&baseline_db_path);
    let baseline_enrichment_queue = load_enrichment_queue_snapshot(&baseline_db_path);

    drop(_fast_mode_restore);
    let _fast_mode_env = EnvVarRestore::set("GITGRAPH_INDEX_FAST_MODE", "1");
    let _write_profile_env = EnvVarRestore::set("GITGRAPH_SQLITE_WRITE_PROFILE", "throughput");
    let _walk_sort_env = EnvVarRestore::set("GITGRAPH_COMMIT_WALK_SORT", "breadth_first");
    scan_repository(&repo_str, Some(&fast_db_str))
        .unwrap_or_else(|error| panic!("fast scan: {error}"));
    index_commits_with_mode(&repo_str, Some(&fast_db_str), IndexingMode::Full)
        .unwrap_or_else(|error| panic!("fast full index: {error}"));
    drain_file_change_extraction_queue(&repo_str, Some(&fast_db_str), None)
        .unwrap_or_else(|error| panic!("fast drain: {error}"));

    let fast_file_changes = load_file_change_snapshot(&fast_db_path);
    let fast_enrichment_queue = load_enrichment_queue_snapshot(&fast_db_path);
    assert_eq!(baseline_file_changes, fast_file_changes);
    assert_eq!(baseline_enrichment_queue, fast_enrichment_queue);
}

#[test]
fn drain_failure_rolls_back_staged_changes_for_batch() {
    let _guard = env_test_guard();
    let _fast_mode_env = EnvVarRestore::set("GITGRAPH_INDEX_FAST_MODE", "1");
    let _write_profile_env = EnvVarRestore::set("GITGRAPH_SQLITE_WRITE_PROFILE", "throughput");
    let _walk_sort_env = EnvVarRestore::set("GITGRAPH_COMMIT_WALK_SORT", "breadth_first");
    let _drain_batch_env =
        EnvVarRestore::set("GITGRAPH_FILE_CHANGE_PIPELINE_COMMIT_BATCH_DRAIN", "1000");

    let (temp, repo_dir) = create_test_repo();
    write_file(repo_dir.join("README.md"), "v1\n");
    git_commit(&repo_dir, "v1");
    write_file(repo_dir.join("README.md"), "v2\n");
    git_commit(&repo_dir, "v2");

    let db_path = temp.path().join("index-failure.db");
    let repo_str = repo_dir.to_string_lossy().to_string();
    let db_str = db_path.to_string_lossy().to_string();

    scan_repository(&repo_str, Some(&db_str)).unwrap_or_else(|error| panic!("scan: {error}"));
    index_commits_with_mode(&repo_str, Some(&db_str), IndexingMode::Full)
        .unwrap_or_else(|error| panic!("fast full index: {error}"));

    let conn = Connection::open(&db_path).unwrap_or_else(|error| panic!("open db: {error}"));
    let repo_id: i64 = conn
        .query_row("SELECT id FROM repositories LIMIT 1", [], |row| row.get(0))
        .unwrap_or_else(|error| panic!("load repo id: {error}"));
    let valid_sha: String = conn
        .query_row(
            "SELECT sha FROM commits WHERE repo_id = ?1 ORDER BY sha LIMIT 1",
            params![repo_id],
            |row| row.get(0),
        )
        .unwrap_or_else(|error| panic!("load valid sha: {error}"));
    conn.execute(
        "DELETE FROM file_change_extraction_queue WHERE repo_id = ?1",
        [repo_id],
    )
    .unwrap_or_else(|error| panic!("clear extraction queue: {error}"));

    let now_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|error| panic!("system clock before unix epoch: {error}"))
        .as_secs() as i64;
    let invalid_sha = "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz".to_string();
    for sha in [&valid_sha, &invalid_sha] {
        conn.execute(
            r#"
            INSERT INTO file_change_extraction_queue (
              repo_id, commit_sha, enqueued_at_epoch, attempts, last_error, next_retry_epoch
            ) VALUES (?1, ?2, ?3, 0, NULL, ?3)
        "#,
            params![repo_id, sha, now_epoch],
        )
        .unwrap_or_else(|error| panic!("seed extraction queue for {sha}: {error}"));
    }
    drop(conn);

    let drain_stats = drain_file_change_extraction_queue(&repo_str, Some(&db_str), None)
        .unwrap_or_else(|error| panic!("drain with invalid sha: {error}"));
    assert_eq!(drain_stats.drained_commits, 0);
    assert_eq!(drain_stats.failed_commits, 2);
    assert_eq!(drain_stats.file_changes_indexed, 0);

    let conn = Connection::open(&db_path).unwrap_or_else(|error| panic!("open db: {error}"));
    let file_change_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM file_changes WHERE repo_id = ?1",
            params![repo_id],
            |row| row.get(0),
        )
        .unwrap_or_else(|error| panic!("count file_changes after failed drain: {error}"));
    assert_eq!(file_change_count, 0);

    let attempts: Vec<(String, i64)> = {
        let mut stmt = conn
            .prepare(
                r#"
                SELECT commit_sha, attempts
                FROM file_change_extraction_queue
                WHERE repo_id = ?1
                ORDER BY commit_sha ASC
            "#,
            )
            .unwrap_or_else(|error| panic!("prepare queue attempts query: {error}"));
        let rows = stmt
            .query_map(params![repo_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap_or_else(|error| panic!("query queue attempts: {error}"));
        let mut values = Vec::new();
        for row in rows {
            values.push(row.unwrap_or_else(|error| panic!("read queue attempt row: {error}")));
        }
        values
    };
    assert_eq!(attempts.len(), 2);
    assert!(attempts.iter().all(|(_, attempt)| *attempt >= 1));
}

#[test]
fn commit_window_has_more_is_correct_at_max_limit_boundary() {
    let _guard = env_test_guard();
    let (temp, repo_dir) = create_test_repo();

    write_file(repo_dir.join("README.md"), "seed\n");
    git_commit(&repo_dir, "seed");

    let db_path = temp.path().join("index.db");
    let repo_str = repo_dir.to_string_lossy().to_string();
    let db_str = db_path.to_string_lossy().to_string();

    scan_repository(&repo_str, Some(&db_str)).unwrap_or_else(|error| panic!("scan: {error}"));

    let mut conn = Connection::open(&db_path).unwrap_or_else(|error| panic!("open db: {error}"));
    let repo_id: i64 = conn
        .query_row("SELECT id FROM repositories LIMIT 1", [], |row| row.get(0))
        .unwrap_or_else(|error| panic!("load repo id: {error}"));

    let total_commits = 5_002usize;
    let tx = conn
        .transaction()
        .unwrap_or_else(|error| panic!("open transaction: {error}"));
    {
        let mut insert_stmt = tx
            .prepare(
                r#"
                INSERT INTO commits (
                  repo_id,
                  sha,
                  author_name,
                  author_email,
                  author_time,
                  committer_time,
                  subject,
                  body,
                  tree_sha
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
            )
            .unwrap_or_else(|error| panic!("prepare commit insert: {error}"));

        for index in 1..=total_commits {
            let sha = format!("{index:040x}");
            let tree_sha = format!("{:040x}", index + total_commits);
            insert_stmt
                .execute(params![
                    repo_id,
                    sha,
                    "Perf Bot",
                    "perf-bot@example.com",
                    index as i64,
                    index as i64,
                    format!("synthetic-commit-{index}"),
                    "",
                    tree_sha,
                ])
                .unwrap_or_else(|error| panic!("insert synthetic commit {index}: {error}"));
        }
    }
    tx.commit()
        .unwrap_or_else(|error| panic!("commit synthetic inserts: {error}"));
    drop(conn);

    let first_window = get_commit_window(&repo_str, Some(&db_str), Some(5_000), None)
        .unwrap_or_else(|error| panic!("first window at max limit: {error}"));
    assert_eq!(first_window.commits.len(), 5_000);
    assert!(first_window.has_more);
    let first_last_sha = first_window
        .commits
        .last()
        .map(|commit| commit.sha.clone())
        .unwrap_or_else(|| panic!("expected first window tail sha"));
    let cursor = first_window
        .next_cursor
        .clone()
        .unwrap_or_else(|| panic!("expected next cursor at max limit"));

    let second_window = get_commit_window(&repo_str, Some(&db_str), Some(5_000), Some(cursor))
        .unwrap_or_else(|error| panic!("second window at max limit: {error}"));
    assert_eq!(second_window.commits.len(), total_commits - 5_000);
    assert!(!second_window.has_more);
    assert!(second_window.next_cursor.is_none());
    assert_ne!(first_last_sha, second_window.commits[0].sha);
}

#[test]
fn ingestion_diff_subphase_telemetry_is_consistent_for_zero_and_single_delta_commits() {
    let _guard = env_test_guard();
    let (temp, repo_dir) = create_test_repo();

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
    let _guard = env_test_guard();
    let (temp, repo_dir) = create_test_repo();

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
fn full_index_results_are_stable_across_pipeline_worker_counts() {
    let _guard = env_test_guard();
    let (temp, repo_dir) = create_test_repo();

    let commit_count = 48usize;
    for index in 0..commit_count {
        write_file(repo_dir.join("README.md"), &format!("commit-{index:03}\n"));
        git_commit(&repo_dir, &format!("commit-{index:03}"));
    }

    let db_single_path = temp.path().join("index-single.db");
    let db_fanout_path = temp.path().join("index-fanout.db");
    let repo_str = repo_dir.to_string_lossy().to_string();
    let db_single_str = db_single_path.to_string_lossy().to_string();
    let db_fanout_str = db_fanout_path.to_string_lossy().to_string();

    let _worker_restore = EnvVarRestore::capture("GITGRAPH_FILE_CHANGE_PIPELINE_WORKERS");
    let _worker_incremental_restore =
        EnvVarRestore::capture("GITGRAPH_FILE_CHANGE_PIPELINE_WORKERS_INCREMENTAL");

    std::env::set_var("GITGRAPH_FILE_CHANGE_PIPELINE_WORKERS", "1");
    std::env::set_var("GITGRAPH_FILE_CHANGE_PIPELINE_WORKERS_INCREMENTAL", "1");
    scan_repository(&repo_str, Some(&db_single_str))
        .unwrap_or_else(|error| panic!("scan single-worker config: {error}"));
    let single_stats = index_commits_with_mode(&repo_str, Some(&db_single_str), IndexingMode::Full)
        .unwrap_or_else(|error| panic!("single-worker full index: {error}"));

    std::env::set_var("GITGRAPH_FILE_CHANGE_PIPELINE_WORKERS", "4");
    std::env::set_var("GITGRAPH_FILE_CHANGE_PIPELINE_WORKERS_INCREMENTAL", "4");
    scan_repository(&repo_str, Some(&db_fanout_str))
        .unwrap_or_else(|error| panic!("scan fanout config: {error}"));
    let fanout_stats = index_commits_with_mode(&repo_str, Some(&db_fanout_str), IndexingMode::Full)
        .unwrap_or_else(|error| panic!("fanout full index: {error}"));

    assert_eq!(single_stats.commits_indexed, fanout_stats.commits_indexed);
    assert_eq!(single_stats.edges_indexed, fanout_stats.edges_indexed);
    assert_eq!(
        single_stats.file_changes_indexed,
        fanout_stats.file_changes_indexed
    );

    let single_conn =
        Connection::open(&db_single_path).unwrap_or_else(|error| panic!("open single db: {error}"));
    let fanout_conn =
        Connection::open(&db_fanout_path).unwrap_or_else(|error| panic!("open fanout db: {error}"));

    let single_commits: i64 = single_conn
        .query_row("SELECT COUNT(*) FROM commits", [], |row| row.get(0))
        .unwrap_or_else(|error| panic!("count single commits: {error}"));
    let fanout_commits: i64 = fanout_conn
        .query_row("SELECT COUNT(*) FROM commits", [], |row| row.get(0))
        .unwrap_or_else(|error| panic!("count fanout commits: {error}"));
    assert_eq!(single_commits, fanout_commits);

    let single_edges: i64 = single_conn
        .query_row("SELECT COUNT(*) FROM commit_edges", [], |row| row.get(0))
        .unwrap_or_else(|error| panic!("count single edges: {error}"));
    let fanout_edges: i64 = fanout_conn
        .query_row("SELECT COUNT(*) FROM commit_edges", [], |row| row.get(0))
        .unwrap_or_else(|error| panic!("count fanout edges: {error}"));
    assert_eq!(single_edges, fanout_edges);

    let single_file_changes: i64 = single_conn
        .query_row("SELECT COUNT(*) FROM file_changes", [], |row| row.get(0))
        .unwrap_or_else(|error| panic!("count single file_changes: {error}"));
    let fanout_file_changes: i64 = fanout_conn
        .query_row("SELECT COUNT(*) FROM file_changes", [], |row| row.get(0))
        .unwrap_or_else(|error| panic!("count fanout file_changes: {error}"));
    assert_eq!(single_file_changes, fanout_file_changes);
}

#[test]
fn file_change_stats_are_deferred_and_jit_enrichable() {
    let _guard = env_test_guard();
    let (temp, repo_dir) = create_test_repo();

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
    let _guard = env_test_guard();
    let (temp, repo_dir) = create_test_repo();

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
    let _guard = env_test_guard();
    let (temp, repo_dir) = create_test_repo();

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
    let _guard = env_test_guard();
    let (temp, repo_dir) = create_test_repo();

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
    let _guard = env_test_guard();
    let (temp, repo_dir) = create_test_repo();

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

    let truncated_dfs = get_graph_traversal(
        &repo_str,
        Some(&db_str),
        &merge_sha,
        GraphTraversalStrategy::Dfs,
        GraphDirection::Parents,
        Some(1),
    )
    .unwrap_or_else(|error| panic!("truncated dfs traversal query: {error}"));
    assert!(truncated_dfs.truncated);
    assert_eq!(truncated_dfs.nodes.len(), 1);
    assert_eq!(truncated_dfs.nodes[0].sha, merge_sha);

    let truncated_topological = get_graph_traversal(
        &repo_str,
        Some(&db_str),
        &merge_sha,
        GraphTraversalStrategy::Topological,
        GraphDirection::Parents,
        Some(1),
    )
    .unwrap_or_else(|error| panic!("truncated topological traversal query: {error}"));
    assert!(truncated_topological.truncated);
    assert_eq!(truncated_topological.nodes.len(), 1);
    assert_eq!(truncated_topological.nodes[0].sha, merge_sha);

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

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileChangeSnapshotRow {
    commit_sha: String,
    file_path: String,
    additions: Option<i64>,
    deletions: Option<i64>,
    change_type: String,
    stats_state: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EnrichmentQueueSnapshotRow {
    commit_sha: String,
    file_path: String,
    attempts: i64,
    last_error: Option<String>,
}

fn load_file_change_snapshot(path: &Path) -> Vec<FileChangeSnapshotRow> {
    let conn = Connection::open(path).unwrap_or_else(|error| panic!("open db snapshot: {error}"));
    let mut stmt = conn
        .prepare(
            r#"
            SELECT commit_sha, file_path, additions, deletions, change_type, stats_state
            FROM file_changes
            ORDER BY commit_sha ASC, file_path ASC
        "#,
        )
        .unwrap_or_else(|error| panic!("prepare file_change snapshot query: {error}"));
    let rows = stmt
        .query_map([], |row| {
            Ok(FileChangeSnapshotRow {
                commit_sha: row.get(0)?,
                file_path: row.get(1)?,
                additions: row.get(2)?,
                deletions: row.get(3)?,
                change_type: row.get(4)?,
                stats_state: row.get(5)?,
            })
        })
        .unwrap_or_else(|error| panic!("query file_change snapshot rows: {error}"));

    let mut snapshot = Vec::new();
    for row in rows {
        snapshot.push(row.unwrap_or_else(|error| panic!("read file_change snapshot row: {error}")));
    }
    snapshot
}

fn load_enrichment_queue_snapshot(path: &Path) -> Vec<EnrichmentQueueSnapshotRow> {
    let conn = Connection::open(path).unwrap_or_else(|error| panic!("open db snapshot: {error}"));
    let mut stmt = conn
        .prepare(
            r#"
            SELECT commit_sha, file_path, attempts, last_error
            FROM file_change_enrichment_queue
            ORDER BY commit_sha ASC, file_path ASC
        "#,
        )
        .unwrap_or_else(|error| panic!("prepare enrichment queue snapshot query: {error}"));
    let rows = stmt
        .query_map([], |row| {
            Ok(EnrichmentQueueSnapshotRow {
                commit_sha: row.get(0)?,
                file_path: row.get(1)?,
                attempts: row.get(2)?,
                last_error: row.get(3)?,
            })
        })
        .unwrap_or_else(|error| panic!("query enrichment queue snapshot rows: {error}"));

    let mut snapshot = Vec::new();
    for row in rows {
        snapshot.push(
            row.unwrap_or_else(|error| panic!("read enrichment queue snapshot row: {error}")),
        );
    }
    snapshot
}

fn create_test_repo() -> (TempDir, std::path::PathBuf) {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("create temp dir: {error}"));
    let repo_dir = temp.path().join("repo");
    fs::create_dir_all(&repo_dir).unwrap_or_else(|error| panic!("create repo dir: {error}"));
    init_git_repo(&repo_dir);
    (temp, repo_dir)
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
