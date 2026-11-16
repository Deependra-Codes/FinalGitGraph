//! The stored commit graph must match git exactly under default settings.
//! Own test binary: nothing here mutates process env, so no test can race another.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gitgraph_lab_core::adapters::db::open_store;
use gitgraph_lab_core::application::commands::{
    get_graph_traversal, get_shortest_path, index_commits_with_mode, scan_repository,
};
use gitgraph_lab_core::domain::models::{GraphDirection, GraphTraversalStrategy, IndexingMode};
use tempfile::TempDir;

const BASE_EPOCH: i64 = 1_600_000_000;
const MASTER_STEP_SECONDS: i64 = 100;

type EdgeSet = BTreeSet<(String, String)>;

#[test]
fn full_index_keeps_every_edge_across_write_waves() {
    assert_default_settings();
    let temp = TempDir::new().expect("create temp dir");
    let repo = new_repo(temp.path());
    let db = temp.path().join("graph-store");

    // 20,000 commits span two 16,384-commit write waves; the merge edge crosses them too.
    let mut stream = String::new();
    for index in 1..=20_000 {
        let from = (index > 1).then(|| format!(":{}", index - 1));
        let merge = (index == 19_000).then(|| ":20050".to_string());
        push_commit(
            &mut stream,
            "master",
            index,
            master_epoch(index),
            from,
            merge,
        );
        if index == 1_000 {
            for offset in 1..=50 {
                let from = Some(format!(":{}", 20_000 + offset - 1)).filter(|_| offset > 1);
                let from = from.or_else(|| Some(":1000".to_string()));
                let epoch = master_epoch(1_000) + offset as i64;
                push_commit(&mut stream, "feature", 20_000 + offset, epoch, from, None);
            }
        }
    }
    fast_import(&repo, &stream);

    scan(&repo, &db);
    let repo_id = index(&repo, &db, IndexingMode::Full);

    let (git_nodes, git_edges) = git_graph(&repo);
    let (stored_nodes, stored_edges) = stored_snapshot_graph(&db, repo_id);
    assert_eq!(stored_nodes, git_nodes);
    assert_eq!(missing(&git_edges, &stored_edges), EdgeSet::new());
    assert_eq!(stored_edges, git_edges);
}

#[test]
fn incremental_index_larger_than_one_wave_keeps_history_connected() {
    assert_default_settings();
    let temp = TempDir::new().expect("create temp dir");
    let repo = new_repo(temp.path());
    let db = temp.path().join("graph-store");

    let mut base = String::new();
    for index in 1..=100 {
        let from = (index > 1).then(|| format!(":{}", index - 1));
        push_commit(&mut base, "master", index, master_epoch(index), from, None);
    }
    fast_import(&repo, &base);
    scan(&repo, &db);
    index(&repo, &db, IndexingMode::Full);

    // 9,000 new commits exceed the 8,192-commit incremental write wave.
    let base_tip = git_out(&repo, &["rev-parse", "master"]);
    let mut more = String::new();
    for index in 101..=9_100 {
        let from = if index == 101 {
            base_tip.clone()
        } else {
            format!(":{}", index - 1)
        };
        push_commit(
            &mut more,
            "master",
            index,
            master_epoch(index),
            Some(from),
            None,
        );
    }
    fast_import(&repo, &more);
    index(&repo, &db, IndexingMode::Incremental);

    assert_reaches_every_commit(&repo, &db);
    let head = git_out(&repo, &["rev-parse", "HEAD"]);
    let root = git_out(&repo, &["rev-list", "--max-parents=0", "HEAD"]);
    let path = get_shortest_path(
        &display(&repo),
        Some(&display(&db)),
        &head,
        &root,
        GraphDirection::Parents,
    )
    .unwrap_or_else(|error| panic!("shortest path: {error}"));
    assert!(path.found, "HEAD cannot reach the root commit");
}

#[test]
fn incremental_index_includes_an_old_dated_merged_branch() {
    assert_default_settings();
    let temp = TempDir::new().expect("create temp dir");
    let repo = new_repo(temp.path());
    let db = temp.path().join("graph-store");

    let mut base = String::new();
    for index in 1..=200 {
        let from = (index > 1).then(|| format!(":{}", index - 1));
        push_commit(&mut base, "master", index, master_epoch(index), from, None);
    }
    fast_import(&repo, &base);
    scan(&repo, &db);
    index(&repo, &db, IndexingMode::Full);

    // A colleague's branch made long ago, merged only now: its commits predate the checkpoint.
    let fork_point = git_out(&repo, &["rev-parse", "master~100"]);
    let master_tip = git_out(&repo, &["rev-parse", "master"]);
    let mut merge = String::new();
    for offset in 1..=5 {
        let from = if offset == 1 {
            fork_point.clone()
        } else {
            format!(":{}", offset - 1)
        };
        let epoch = master_epoch(100) + offset as i64;
        push_commit(&mut merge, "old-branch", offset, epoch, Some(from), None);
    }
    push_commit(
        &mut merge,
        "master",
        6,
        master_epoch(200) + 10_000,
        Some(master_tip),
        Some(":5".to_string()),
    );
    fast_import(&repo, &merge);
    git_out(&repo, &["update-ref", "-d", "refs/heads/old-branch"]);
    index(&repo, &db, IndexingMode::Incremental);

    assert_reaches_every_commit(&repo, &db);
}

fn assert_default_settings() {
    let overrides: Vec<String> = env::vars()
        .map(|(key, _)| key)
        .filter(|key| key.starts_with("GITGRAPH_"))
        .collect();
    assert!(
        overrides.is_empty(),
        "graph_oracle checks default settings; unset {overrides:?}"
    );
}

fn master_epoch(index: usize) -> i64 {
    BASE_EPOCH + index as i64 * MASTER_STEP_SECONDS
}

fn push_commit(
    stream: &mut String,
    branch: &str,
    mark: usize,
    epoch: i64,
    from: Option<String>,
    merge: Option<String>,
) {
    let content = format!("{branch}-{mark}\n");
    stream.push_str(&format!(
        "commit refs/heads/{branch}\nmark :{mark}\ncommitter Oracle <oracle@example.com> {epoch} +0000\ndata 0\n"
    ));
    if let Some(from) = from {
        stream.push_str(&format!("from {from}\n"));
    }
    if let Some(merge) = merge {
        stream.push_str(&format!("merge {merge}\n"));
    }
    stream.push_str(&format!(
        "M 100644 inline file.txt\ndata {}\n{content}\n",
        content.len()
    ));
}

/// Fixture git must not see the caller's git setup (hooks export GIT_DIR, machine config changes defaults).
fn git(repo: &Path) -> Command {
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
        .arg("-C")
        .arg(repo);
    command
}

fn git_out(repo: &Path, args: &[&str]) -> String {
    let output = git(repo)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("run git: {error}"));
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn fast_import(repo: &Path, stream: &str) {
    let mut child = git(repo)
        .args(["fast-import", "--quiet"])
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("spawn git fast-import: {error}"));
    child
        .stdin
        .take()
        .unwrap_or_else(|| panic!("fast-import stdin is not piped"))
        .write_all(stream.as_bytes())
        .unwrap_or_else(|error| panic!("write fast-import stream: {error}"));
    let output = child
        .wait_with_output()
        .unwrap_or_else(|error| panic!("wait for git fast-import: {error}"));
    assert!(
        output.status.success(),
        "git fast-import: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn new_repo(root: &Path) -> PathBuf {
    let repo = root.join("repo");
    fs::create_dir_all(&repo).unwrap_or_else(|error| panic!("create repo dir: {error}"));
    git_out(&repo, &["init", "--quiet", "--initial-branch=master"]);
    git_out(&repo, &["config", "core.ignorecase", "false"]);
    fs::canonicalize(&repo).unwrap_or_else(|error| panic!("canonicalize repo dir: {error}"))
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

fn scan(repo: &Path, db: &Path) {
    scan_repository(&display(repo), Some(&display(db)))
        .unwrap_or_else(|error| panic!("scan repository: {error}"));
}

fn index(repo: &Path, db: &Path, mode: IndexingMode) -> i64 {
    index_commits_with_mode(&display(repo), Some(&display(db)), mode)
        .unwrap_or_else(|error| panic!("{mode:?} index: {error}"))
        .repo_id
}

/// Commits and parent edges git reports for the product's ref scope (local branches; HEAD is on master).
fn git_graph(repo: &Path) -> (BTreeSet<String>, EdgeSet) {
    let mut nodes = BTreeSet::new();
    let mut edges = EdgeSet::new();
    for line in git_out(repo, &["rev-list", "--parents", "--branches"]).lines() {
        let mut shas = line.split_whitespace().map(str::to_string);
        let Some(child) = shas.next() else { continue };
        for parent in shas {
            edges.insert((child.clone(), parent));
        }
        nodes.insert(child);
    }
    (nodes, edges)
}

fn stored_snapshot_graph(db: &Path, repo_id: i64) -> (BTreeSet<String>, EdgeSet) {
    let store = open_store(db).unwrap_or_else(|error| panic!("open store: {error}"));
    let snapshot = store
        .load_csr_snapshot(repo_id)
        .unwrap_or_else(|error| panic!("load csr snapshot: {error}"))
        .unwrap_or_else(|| panic!("csr snapshot missing after full index"));
    let mut edges = EdgeSet::new();
    for (node, sha) in snapshot.shas.iter().enumerate() {
        for &parent in snapshot.parents_for(node) {
            edges.insert((sha.clone(), snapshot.shas[parent as usize].clone()));
        }
    }
    (snapshot.shas.into_iter().collect(), edges)
}

fn reachable_from_head(repo: &Path, db: &Path) -> BTreeSet<String> {
    let head = git_out(repo, &["rev-parse", "HEAD"]);
    let response = get_graph_traversal(
        &display(repo),
        Some(&display(db)),
        &head,
        GraphTraversalStrategy::Bfs,
        GraphDirection::Parents,
        Some(1_000_000),
    )
    .unwrap_or_else(|error| panic!("traverse from HEAD: {error}"));
    assert!(!response.truncated, "traversal hit its node budget");
    response.nodes.into_iter().map(|node| node.sha).collect()
}

fn assert_reaches_every_commit(repo: &Path, db: &Path) {
    let reachable = reachable_from_head(repo, db);
    let unreached: Vec<String> = git_graph(repo).0.difference(&reachable).cloned().collect();
    assert!(
        unreached.is_empty(),
        "{} commits unreachable from HEAD, e.g. {:?}",
        unreached.len(),
        unreached.first()
    );
}

fn missing(expected: &EdgeSet, actual: &EdgeSet) -> EdgeSet {
    expected.difference(actual).cloned().collect()
}
