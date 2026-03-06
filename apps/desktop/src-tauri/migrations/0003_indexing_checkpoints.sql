CREATE TABLE IF NOT EXISTS indexing_checkpoints (
  repo_id INTEGER PRIMARY KEY,
  last_committer_time INTEGER NOT NULL,
  last_commit_sha TEXT NOT NULL,
  indexed_at_epoch INTEGER NOT NULL,
  FOREIGN KEY(repo_id) REFERENCES repositories(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_indexing_checkpoints_repo_time
  ON indexing_checkpoints(repo_id, last_committer_time DESC);
