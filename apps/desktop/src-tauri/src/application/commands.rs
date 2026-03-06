use std::path::{Path, PathBuf};

use crate::application::indexer::Indexer;
use crate::domain::models::{
    CommandError, CommitWindowCursor, CommitWindowRequest, CommitWindowResponse,
    IndexRepositoryRequest, IndexingMode, IndexingStats, RepositorySummary, ScanRepositoryRequest,
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

fn build_indexer(db_path: Option<&str>) -> Indexer {
    let resolved = db_path
        .map(PathBuf::from)
        .unwrap_or_else(Indexer::default_db_path);
    Indexer::new(resolved)
}
