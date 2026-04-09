use std::path::{Path, PathBuf};

use crate::application::indexer::Indexer;
use crate::domain::models::{
    CommandError, CommitWindowCursor, CommitWindowRequest, CommitWindowResponse,
    FileChangeExtractionDrainStats, GraphDirection, GraphTraversalRequest, GraphTraversalResponse,
    GraphTraversalStrategy, IndexRepositoryRequest, IndexingMode, IndexingStats, RepositorySummary,
    ScanRepositoryRequest, ShortestPathRequest, ShortestPathResponse,
};

pub fn scan_repository(
    path: &str,
    db_path: Option<&str>,
) -> Result<RepositorySummary, CommandError> {
    scan_repository_with_request(ScanRepositoryRequest {
        path: path.to_string(),
        db_path: db_path.map(ToString::to_string),
    })
}

pub fn scan_repository_with_request(
    request: ScanRepositoryRequest,
) -> Result<RepositorySummary, CommandError> {
    let indexer = build_indexer(request.db_path.as_deref());
    indexer
        .scan_repository(Path::new(&request.path))
        .map_err(|error| error.into_command_error())
}

pub fn index_commits(path: &str, db_path: Option<&str>) -> Result<IndexingStats, CommandError> {
    index_commits_with_mode(path, db_path, IndexingMode::Full)
}

pub fn index_commits_with_mode(
    path: &str,
    db_path: Option<&str>,
    mode: IndexingMode,
) -> Result<IndexingStats, CommandError> {
    index_repository_with_request(IndexRepositoryRequest {
        path: path.to_string(),
        db_path: db_path.map(ToString::to_string),
        mode,
    })
}

pub fn index_repository_with_request(
    request: IndexRepositoryRequest,
) -> Result<IndexingStats, CommandError> {
    let indexer = build_indexer(request.db_path.as_deref());
    indexer
        .index_repository(Path::new(&request.path), request.mode)
        .map_err(|error| error.into_command_error())
}

pub fn enrich_pending_file_change_stats(
    path: &str,
    db_path: Option<&str>,
    max_items: Option<usize>,
) -> Result<usize, CommandError> {
    let indexer = build_indexer(db_path);
    indexer
        .enrich_pending_file_change_stats(Path::new(path), max_items)
        .map_err(|error| error.into_command_error())
}

pub fn drain_file_change_extraction_queue(
    path: &str,
    db_path: Option<&str>,
    max_commits: Option<usize>,
) -> Result<FileChangeExtractionDrainStats, CommandError> {
    let indexer = build_indexer(db_path);
    indexer
        .drain_file_change_extraction_queue(Path::new(path), max_commits)
        .map_err(|error| error.into_command_error())
}

pub fn ensure_file_change_stats_for_commits(
    path: &str,
    db_path: Option<&str>,
    commit_shas: &[String],
    max_items: Option<usize>,
) -> Result<usize, CommandError> {
    let indexer = build_indexer(db_path);
    indexer
        .ensure_file_change_stats_for_commits(Path::new(path), commit_shas, max_items)
        .map_err(|error| error.into_command_error())
}

pub fn get_commit_window(
    path: &str,
    db_path: Option<&str>,
    limit: Option<usize>,
    cursor: Option<CommitWindowCursor>,
) -> Result<CommitWindowResponse, CommandError> {
    get_commit_window_with_request(CommitWindowRequest {
        path: path.to_string(),
        db_path: db_path.map(ToString::to_string),
        limit: limit.unwrap_or(100),
        cursor,
    })
}

pub fn get_commit_window_with_request(
    request: CommitWindowRequest,
) -> Result<CommitWindowResponse, CommandError> {
    let indexer = build_indexer(request.db_path.as_deref());
    indexer
        .get_commit_window(
            Path::new(&request.path),
            Some(request.limit),
            request.cursor,
        )
        .map_err(|error| error.into_command_error())
}

pub fn get_graph_traversal(
    path: &str,
    db_path: Option<&str>,
    start_sha: &str,
    strategy: GraphTraversalStrategy,
    direction: GraphDirection,
    max_nodes: Option<usize>,
) -> Result<GraphTraversalResponse, CommandError> {
    get_graph_traversal_with_request(GraphTraversalRequest {
        path: path.to_string(),
        db_path: db_path.map(ToString::to_string),
        start_sha: start_sha.to_string(),
        strategy,
        direction,
        max_nodes: max_nodes.unwrap_or(2_000),
    })
}

pub fn get_graph_traversal_with_request(
    request: GraphTraversalRequest,
) -> Result<GraphTraversalResponse, CommandError> {
    let indexer = build_indexer(request.db_path.as_deref());
    indexer
        .traverse_graph(
            Path::new(&request.path),
            &request.start_sha,
            request.strategy,
            request.direction,
            request.max_nodes,
        )
        .map_err(|error| error.into_command_error())
}

pub fn get_shortest_path(
    path: &str,
    db_path: Option<&str>,
    from_sha: &str,
    to_sha: &str,
    direction: GraphDirection,
) -> Result<ShortestPathResponse, CommandError> {
    get_shortest_path_with_request(ShortestPathRequest {
        path: path.to_string(),
        db_path: db_path.map(ToString::to_string),
        from_sha: from_sha.to_string(),
        to_sha: to_sha.to_string(),
        direction,
    })
}

pub fn get_shortest_path_with_request(
    request: ShortestPathRequest,
) -> Result<ShortestPathResponse, CommandError> {
    let indexer = build_indexer(request.db_path.as_deref());
    indexer
        .shortest_path(
            Path::new(&request.path),
            &request.from_sha,
            &request.to_sha,
            request.direction,
        )
        .map_err(|error| error.into_command_error())
}

fn build_indexer(db_path: Option<&str>) -> Indexer {
    let resolved = db_path
        .map(PathBuf::from)
        .unwrap_or_else(Indexer::default_db_path);
    Indexer::new(resolved)
}
