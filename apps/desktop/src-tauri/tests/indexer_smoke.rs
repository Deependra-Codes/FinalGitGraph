use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use gitgraph_lab_core::adapters::db::open_store;
use gitgraph_lab_core::application::commands::{
    drain_file_change_extraction_queue, enrich_pending_file_change_stats, get_commit_window,
    get_graph_traversal, get_shortest_path, index_commits_with_mode, scan_repository,
};
use gitgraph_lab_core::domain::models::{GraphDirection, GraphTraversalStrategy, IndexingMode};
use tempfile::TempDir;

const FILE_CHANGE_SYNC_STRICT_ENV: &str = "GITGRAPH_FILE_CHANGE_SYNC_STRICT";
const COMMIT_WALK_SCOPE_ENV: &str = "GITGRAPH_COMMIT_WALK_SCOPE";
const COMMIT_WALK_SORT_ENV: &str = "GITGRAPH_COMMIT_WALK_SORT";
const INDEX_FAST_MODE_ENV: &str = "GITGRAPH_INDEX_FAST_MODE";

#[test]
fn lmdb_index_and_graph_queries_work_end_to_end() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("create temp dir: {error}"));
    let repo_dir = temp.path().join("repo");
    let db_path = temp.path().join("graph-store");
    create_test_repo(&repo_dir);

    let repo_path = fs::canonicalize(&repo_dir)
        .unwrap_or_else(|error| panic!("canonicalize repo path: {error}"))
        .display()
        .to_string();
    let db_path_str = db_path.display().to_string();
    scan_repository(repo_path.as_str(), Some(db_path_str.as_str()))
        .unwrap_or_else(|error| panic!("scan repository: {error}"));
    let full_stats = index_commits_with_mode(
        repo_path.as_str(),
        Some(db_path_str.as_str()),
        IndexingMode::Full,
    )
    .unwrap_or_else(|error| panic!("full index: {error}"));
    assert!(full_stats.commits_indexed >= 3);

    let head_sha = run_git_capture(&repo_dir, &["rev-parse", "HEAD"]);
    let root_sha = run_git_capture(&repo_dir, &["rev-list", "--max-parents=0", "HEAD"]);

    let traversal = get_graph_traversal(
        repo_path.as_str(),
        Some(db_path_str.as_str()),
        head_sha.as_str(),
        GraphTraversalStrategy::Bfs,
        GraphDirection::Parents,
        Some(64),
    )
    .unwrap_or_else(|error| panic!("bfs traversal: {error}"));
    assert!(!traversal.nodes.is_empty());

    let shortest = get_shortest_path(
        repo_path.as_str(),
        Some(db_path_str.as_str()),
        head_sha.as_str(),
        root_sha.as_str(),
        GraphDirection::Parents,
    )
    .unwrap_or_else(|error| panic!("shortest path: {error}"));
    assert!(shortest.found);
    assert!(!shortest.path.is_empty());

    let store = open_store(&db_path).unwrap_or_else(|error| panic!("open lmdb store: {error}"));
    let repo_id = store
        .load_repository_id_by_path(repo_path.as_str())
        .unwrap_or_else(|error| panic!("load repo id by path: {error}"))
        .expect("repository id should exist");
    let counts = store
        .load_dataset_counts(repo_id)
        .unwrap_or_else(|error| panic!("load dataset counts: {error}"));
    assert!(counts.0 >= 3);
    assert!(counts.1 >= 2);
    let snapshot_metadata = store
        .load_graph_snapshot_metadata(repo_id)
        .unwrap_or_else(|error| panic!("load graph snapshot metadata: {error}"))
        .expect("graph snapshot metadata should exist");
    assert_eq!(snapshot_metadata.snapshot_version, 1);
    assert_eq!(snapshot_metadata.node_count as i64, counts.0);
    assert_eq!(snapshot_metadata.directed_edges as i64, counts.1);
}

#[test]
fn incremental_index_advances_commit_window() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("create temp dir: {error}"));
    let repo_dir = temp.path().join("repo");
    let db_path = temp.path().join("graph-store");
    create_test_repo(&repo_dir);

    let repo_path = fs::canonicalize(&repo_dir)
        .unwrap_or_else(|error| panic!("canonicalize repo path: {error}"))
        .display()
        .to_string();
    let db_path_str = db_path.display().to_string();
    scan_repository(repo_path.as_str(), Some(db_path_str.as_str()))
        .unwrap_or_else(|error| panic!("scan repository: {error}"));
    let _ = index_commits_with_mode(
        repo_path.as_str(),
        Some(db_path_str.as_str()),
        IndexingMode::Full,
    )
    .unwrap_or_else(|error| panic!("full index: {error}"));
    let before_metadata = {
        let store = open_store(&db_path).unwrap_or_else(|error| panic!("open lmdb store: {error}"));
        let repo_id = store
            .load_repository_id_by_path(repo_path.as_str())
            .unwrap_or_else(|error| panic!("load repo id by path: {error}"))
            .expect("repository id should exist");
        store
            .load_graph_snapshot_metadata(repo_id)
            .unwrap_or_else(|error| {
                panic!("load graph snapshot metadata before incremental: {error}")
            })
            .expect("graph snapshot metadata should exist after full index")
    };

    append_commit(&repo_dir, "incremental-a");
    let incremental_stats = index_commits_with_mode(
        repo_path.as_str(),
        Some(db_path_str.as_str()),
        IndexingMode::Incremental,
    )
    .unwrap_or_else(|error| panic!("incremental index: {error}"));
    assert!(incremental_stats.commits_indexed >= 1);
    let head_sha = run_git_capture(&repo_dir, &["rev-parse", "HEAD"]);
    let root_sha = run_git_capture(&repo_dir, &["rev-list", "--max-parents=0", "HEAD"]);
    let after_metadata = {
        let store = open_store(&db_path).unwrap_or_else(|error| panic!("open lmdb store: {error}"));
        let repo_id = store
            .load_repository_id_by_path(repo_path.as_str())
            .unwrap_or_else(|error| panic!("load repo id by path: {error}"))
            .expect("repository id should exist");
        store
            .load_graph_snapshot_metadata(repo_id)
            .unwrap_or_else(|error| {
                panic!("load graph snapshot metadata after incremental: {error}")
            })
            .expect("graph snapshot metadata should exist after incremental index")
    };
    let after_state = {
        let store = open_store(&db_path).unwrap_or_else(|error| panic!("open lmdb store: {error}"));
        let repo_id = store
            .load_repository_id_by_path(repo_path.as_str())
            .unwrap_or_else(|error| panic!("load repo id by path: {error}"))
            .expect("repository id should exist");
        store
            .load_graph_snapshot_state(repo_id)
            .unwrap_or_else(|error| panic!("load graph snapshot state after incremental: {error}"))
            .expect("graph snapshot state should exist after incremental index")
    };
    assert!(
        after_metadata.snapshot_version >= before_metadata.snapshot_version,
        "expected snapshot version to remain monotonic after incremental index"
    );
    assert!(
        after_metadata.node_count >= before_metadata.node_count,
        "expected snapshot node count to stay monotonic after incremental index"
    );
    assert!(
        after_metadata.pending_overlay_commits >= 1
            || after_metadata.snapshot_version > before_metadata.snapshot_version,
        "expected incremental indexing to either retain bounded overlay commits or compact them into a newer snapshot"
    );
    assert!(
        !after_state.overlay_parent_rows.is_empty()
            || after_metadata.snapshot_version > before_metadata.snapshot_version,
        "expected overlay parent rows to be materialized unless compaction already folded them into the snapshot"
    );

    let shortest = get_shortest_path(
        repo_path.as_str(),
        Some(db_path_str.as_str()),
        head_sha.as_str(),
        root_sha.as_str(),
        GraphDirection::Parents,
    )
    .unwrap_or_else(|error| panic!("overlay-aware shortest path: {error}"));
    assert!(
        shortest.found,
        "expected graph query to reach root from incremental head through snapshot plus overlay state"
    );
    assert_eq!(
        shortest.path.first().map(String::as_str),
        Some(head_sha.as_str()),
        "expected shortest path to start at the incremental head"
    );

    let window = get_commit_window(
        repo_path.as_str(),
        Some(db_path_str.as_str()),
        Some(10),
        None,
    )
    .unwrap_or_else(|error| panic!("load commit window: {error}"));
    assert!(!window.commits.is_empty());
    assert!(
        window
            .commits
            .iter()
            .any(|commit| commit.subject.contains("incremental-a")),
        "expected incremental commit subject to appear in commit window"
    );
}

#[test]
fn enrichment_endpoint_executes_without_sql_backend() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("create temp dir: {error}"));
    let repo_dir = temp.path().join("repo");
    let db_path = temp.path().join("graph-store");
    create_test_repo(&repo_dir);

    let repo_path = fs::canonicalize(&repo_dir)
        .unwrap_or_else(|error| panic!("canonicalize repo path: {error}"))
        .display()
        .to_string();
    let db_path_str = db_path.display().to_string();
    scan_repository(repo_path.as_str(), Some(db_path_str.as_str()))
        .unwrap_or_else(|error| panic!("scan repository: {error}"));
    let _ = index_commits_with_mode(
        repo_path.as_str(),
        Some(db_path_str.as_str()),
        IndexingMode::Full,
    )
    .unwrap_or_else(|error| panic!("full index: {error}"));

    let enriched =
        enrich_pending_file_change_stats(repo_path.as_str(), Some(db_path_str.as_str()), Some(128))
            .unwrap_or_else(|error| panic!("enrich pending stats: {error}"));
    assert!(enriched <= 128);
}

#[test]
fn async_file_change_indexing_enqueues_then_drain_materializes() {
    let _guard = env_lock().lock().expect("lock env");
    let _strict_mode_guard = EnvVarGuard::set(FILE_CHANGE_SYNC_STRICT_ENV, "0");

    let temp = TempDir::new().unwrap_or_else(|error| panic!("create temp dir: {error}"));
    let repo_dir = temp.path().join("repo");
    let db_path = temp.path().join("graph-store");
    create_test_repo(&repo_dir);

    let repo_path = fs::canonicalize(&repo_dir)
        .unwrap_or_else(|error| panic!("canonicalize repo path: {error}"))
        .display()
        .to_string();
    let db_path_str = db_path.display().to_string();
    scan_repository(repo_path.as_str(), Some(db_path_str.as_str()))
        .unwrap_or_else(|error| panic!("scan repository: {error}"));

    let full_stats = index_commits_with_mode(
        repo_path.as_str(),
        Some(db_path_str.as_str()),
        IndexingMode::Full,
    )
    .unwrap_or_else(|error| panic!("full index in async mode: {error}"));
    assert!(
        full_stats.file_change_commits_enqueued > 0,
        "expected async index mode to enqueue extraction work"
    );
    assert_eq!(
        full_stats.file_changes_indexed, 0,
        "expected async index mode to defer file-change extraction"
    );
    assert!(
        full_stats.file_change_queue_depth_after_index > 0,
        "expected extraction queue backlog after async index"
    );

    let drain_stats = drain_file_change_extraction_queue(
        repo_path.as_str(),
        Some(db_path_str.as_str()),
        Some(128),
    )
    .unwrap_or_else(|error| panic!("drain extraction queue: {error}"));
    assert!(
        drain_stats.drained_commits > 0,
        "expected extraction drain to process queued commits"
    );
    assert!(
        drain_stats.file_changes_indexed > 0,
        "expected extraction drain to materialize file changes"
    );

    let store = open_store(&db_path).unwrap_or_else(|error| panic!("open lmdb store: {error}"));
    let repo_id = store
        .load_repository_id_by_path(repo_path.as_str())
        .unwrap_or_else(|error| panic!("load repo id by path: {error}"))
        .expect("repository id should exist");
    let queue_metrics = store
        .load_file_change_extraction_queue_metrics(repo_id)
        .unwrap_or_else(|error| panic!("load extraction queue metrics: {error}"));
    assert_eq!(queue_metrics.depth, 0);
}

#[test]
fn strict_sync_mode_materializes_file_changes_inline() {
    let _guard = env_lock().lock().expect("lock env");
    let _strict_mode_guard = EnvVarGuard::set(FILE_CHANGE_SYNC_STRICT_ENV, "1");

    let temp = TempDir::new().unwrap_or_else(|error| panic!("create temp dir: {error}"));
    let repo_dir = temp.path().join("repo");
    let db_path = temp.path().join("graph-store");
    create_test_repo(&repo_dir);

    let repo_path = fs::canonicalize(&repo_dir)
        .unwrap_or_else(|error| panic!("canonicalize repo path: {error}"))
        .display()
        .to_string();
    let db_path_str = db_path.display().to_string();
    scan_repository(repo_path.as_str(), Some(db_path_str.as_str()))
        .unwrap_or_else(|error| panic!("scan repository: {error}"));

    let full_stats = index_commits_with_mode(
        repo_path.as_str(),
        Some(db_path_str.as_str()),
        IndexingMode::Full,
    )
    .unwrap_or_else(|error| panic!("full index in strict mode: {error}"));
    assert!(
        full_stats.file_changes_indexed > 0,
        "expected strict mode to materialize file changes inline"
    );
    assert_eq!(
        full_stats.file_change_commits_enqueued, 0,
        "expected strict mode to avoid async extraction backlog"
    );
    assert_eq!(full_stats.file_change_queue_depth_after_index, 0);
}

#[test]
fn commit_walk_scope_frontier_excludes_tag_and_remote_only_refs_until_all_refs_override() {
    let _guard = env_lock().lock().expect("lock env");
    let _fast_mode_guard = EnvVarGuard::set(INDEX_FAST_MODE_ENV, "1");
    let _walk_sort_guard = EnvVarGuard::set(COMMIT_WALK_SORT_ENV, "breadth_first");
    let _strict_mode_guard = EnvVarGuard::set(FILE_CHANGE_SYNC_STRICT_ENV, "0");

    let temp = TempDir::new().unwrap_or_else(|error| panic!("create temp dir: {error}"));
    let repo_dir = temp.path().join("repo");
    let db_path = temp.path().join("graph-store");
    create_test_repo(&repo_dir);

    let default_branch = run_git_capture(&repo_dir, &["rev-parse", "--abbrev-ref", "HEAD"]);

    run_git(&repo_dir, &["checkout", "-b", "tag-only"]);
    append_commit(&repo_dir, "tag-only-history");
    run_git(&repo_dir, &["tag", "tag-only-tip"]);
    run_git(&repo_dir, &["checkout", default_branch.as_str()]);
    run_git(&repo_dir, &["branch", "-D", "tag-only"]);

    run_git(&repo_dir, &["checkout", "-b", "remote-only"]);
    append_commit(&repo_dir, "remote-only-history");
    let remote_only_sha = run_git_capture(&repo_dir, &["rev-parse", "HEAD"]);
    run_git(&repo_dir, &["checkout", default_branch.as_str()]);
    run_git(
        &repo_dir,
        &[
            "update-ref",
            "refs/remotes/origin/remote-only",
            remote_only_sha.as_str(),
        ],
    );
    run_git(&repo_dir, &["branch", "-D", "remote-only"]);

    let repo_path = fs::canonicalize(&repo_dir)
        .unwrap_or_else(|error| panic!("canonicalize repo path: {error}"))
        .display()
        .to_string();
    let db_path_str = db_path.display().to_string();
    scan_repository(repo_path.as_str(), Some(db_path_str.as_str()))
        .unwrap_or_else(|error| panic!("scan repository: {error}"));

    std::env::remove_var(COMMIT_WALK_SCOPE_ENV);
    let full_frontier = index_commits_with_mode(
        repo_path.as_str(),
        Some(db_path_str.as_str()),
        IndexingMode::Full,
    )
    .unwrap_or_else(|error| panic!("full index in default frontier scope: {error}"));
    assert_eq!(
        full_frontier.commit_throughput.commit_walk_scope,
        "frontier"
    );

    let frontier_window = get_commit_window(
        repo_path.as_str(),
        Some(db_path_str.as_str()),
        Some(64),
        None,
    )
    .unwrap_or_else(|error| panic!("load frontier commit window: {error}"));
    assert!(
        !frontier_window
            .commits
            .iter()
            .any(|commit| commit.subject.contains("tag-only-history")),
        "expected tag-only ancestry to be excluded in frontier scope"
    );
    assert!(
        !frontier_window
            .commits
            .iter()
            .any(|commit| commit.subject.contains("remote-only-history")),
        "expected remote-only ancestry to be excluded in frontier scope"
    );

    let _all_refs_guard = EnvVarGuard::set(COMMIT_WALK_SCOPE_ENV, "all_refs");
    let full_all_refs = index_commits_with_mode(
        repo_path.as_str(),
        Some(db_path_str.as_str()),
        IndexingMode::Full,
    )
    .unwrap_or_else(|error| panic!("full index in all_refs scope: {error}"));
    assert_eq!(
        full_all_refs.commit_throughput.commit_walk_scope,
        "all_refs"
    );
    let expected_all_refs_count: usize =
        run_git_capture(&repo_dir, &["rev-list", "--all", "--count"])
            .parse()
            .unwrap_or_else(|error| panic!("parse expected all-ref commit count: {error}"));
    assert_eq!(
        full_all_refs.commits_indexed, expected_all_refs_count,
        "expected all_refs scope to index full --all commit set without duplicates"
    );

    let all_refs_window = get_commit_window(
        repo_path.as_str(),
        Some(db_path_str.as_str()),
        Some(64),
        None,
    )
    .unwrap_or_else(|error| panic!("load all_refs commit window: {error}"));
    assert!(
        all_refs_window
            .commits
            .iter()
            .any(|commit| commit.subject.contains("tag-only-history")),
        "expected tag-only ancestry to be visible in all_refs scope"
    );
    assert!(
        all_refs_window
            .commits
            .iter()
            .any(|commit| commit.subject.contains("remote-only-history")),
        "expected remote-only ancestry to be visible in all_refs scope"
    );

    let full_all_refs_second = index_commits_with_mode(
        repo_path.as_str(),
        Some(db_path_str.as_str()),
        IndexingMode::Full,
    )
    .unwrap_or_else(|error| panic!("second full index in all_refs scope: {error}"));
    assert_eq!(
        full_all_refs_second.commits_indexed, expected_all_refs_count,
        "expected deterministic non-duplicate commit indexing across repeated all_refs full runs"
    );
}

fn create_test_repo(repo_dir: &Path) {
    fs::create_dir_all(repo_dir).unwrap_or_else(|error| panic!("create repo dir: {error}"));
    run_git(repo_dir, &["init"]);
    run_git(repo_dir, &["config", "user.name", "GitGraph Lab"]);
    run_git(
        repo_dir,
        &["config", "user.email", "gitgraph-lab@example.com"],
    );
    append_commit(repo_dir, "seed-a");
    append_commit(repo_dir, "seed-b");
    append_commit(repo_dir, "seed-c");
}

fn append_commit(repo_dir: &Path, message: &str) {
    let filename = format!("{message}.txt");
    let body = format!("{message}\n");
    fs::write(repo_dir.join(&filename), body).unwrap_or_else(|error| {
        panic!(
            "write fixture file {}: {error}",
            repo_dir.join(&filename).display()
        )
    });
    run_git(repo_dir, &["add", "."]);
    run_git(repo_dir, &["commit", "-m", message]);
}

fn run_git(repo_dir: &Path, args: &[&str]) {
    let _ = run_git_capture(repo_dir, args);
}

fn run_git_capture(repo_dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_dir)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("execute git {:?}: {error}", args));
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("git {:?} failed: {stderr}", args);
    }
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn env_lock() -> &'static Mutex<()> {
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

struct EnvVarGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(value) = self.previous.take() {
            std::env::set_var(self.key, value);
        } else {
            std::env::remove_var(self.key);
        }
    }
}
