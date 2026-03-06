use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IndexingMode {
    #[default]
    Full,
    Incremental,
}

impl IndexingMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "full" => Some(Self::Full),
            "incremental" => Some(Self::Incremental),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GraphTraversalStrategy {
    #[default]
    Bfs,
    Dfs,
    Topological,
}

impl GraphTraversalStrategy {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "bfs" => Some(Self::Bfs),
            "dfs" => Some(Self::Dfs),
            "topological" => Some(Self::Topological),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GraphDirection {
    #[default]
    Parents,
    Children,
    Both,
}

impl GraphDirection {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "parents" => Some(Self::Parents),
            "children" => Some(Self::Children),
            "both" => Some(Self::Both),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanRepositoryRequest {
    pub path: String,
    pub db_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexRepositoryRequest {
    pub path: String,
    pub db_path: Option<String>,
    pub mode: IndexingMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitWindowCursor {
    pub committer_time: i64,
    pub sha: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitWindowRequest {
    pub path: String,
    pub db_path: Option<String>,
    pub limit: usize,
    pub cursor: Option<CommitWindowCursor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphTraversalRequest {
    pub path: String,
    pub db_path: Option<String>,
    pub start_sha: String,
    pub strategy: GraphTraversalStrategy,
    pub direction: GraphDirection,
    pub max_nodes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortestPathRequest {
    pub path: String,
    pub db_path: Option<String>,
    pub from_sha: String,
    pub to_sha: String,
    pub direction: GraphDirection,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommandError {
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CommandError {}

#[derive(Debug, Clone, Serialize)]
pub struct RepositorySummary {
    pub path: String,
    pub git_dir: String,
    pub name: String,
    pub is_valid: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct IndexingStats {
    pub repo_id: i64,
    pub mode: IndexingMode,
    pub commits_indexed: usize,
    pub edges_indexed: usize,
    pub file_changes_indexed: usize,
    pub refs_indexed: usize,
    pub indexed_at_epoch: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitWindowItem {
    pub sha: String,
    pub author_name: String,
    pub author_email: String,
    pub committer_time: i64,
    pub subject: String,
    pub tree_sha: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitWindowResponse {
    pub commits: Vec<CommitWindowItem>,
    pub next_cursor: Option<CommitWindowCursor>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphNodeVisit {
    pub sha: String,
    pub depth: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphTraversalResponse {
    pub strategy: GraphTraversalStrategy,
    pub direction: GraphDirection,
    pub nodes: Vec<GraphNodeVisit>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShortestPathResponse {
    pub from_sha: String,
    pub to_sha: String,
    pub direction: GraphDirection,
    pub found: bool,
    pub path: Vec<String>,
    pub visited_nodes: usize,
}

#[derive(Debug, Clone)]
pub struct IndexCheckpoint {
    pub last_committer_time: i64,
    pub last_commit_sha: String,
    pub indexed_at_epoch: i64,
}

#[derive(Debug, Clone)]
pub struct ParsedCommit {
    pub sha: String,
    pub parents: Vec<String>,
    pub tree_sha: String,
    pub author_name: String,
    pub author_email: String,
    pub author_time: i64,
    pub committer_time: i64,
    pub subject: String,
}

#[derive(Debug, Clone)]
pub struct ParsedRef {
    pub ref_name: String,
    pub target_sha: String,
    pub ref_type: String,
}

#[derive(Debug, Clone)]
pub struct ParsedFileChange {
    pub commit_sha: String,
    pub file_path: String,
    pub additions: i64,
    pub deletions: i64,
    pub change_type: String,
}
