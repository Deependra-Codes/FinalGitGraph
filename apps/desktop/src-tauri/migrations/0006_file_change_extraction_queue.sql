-- Optional fast-index queue for deferred file-change extraction.

CREATE TABLE IF NOT EXISTS file_change_extraction_queue (
  repo_id INTEGER NOT NULL,
  commit_sha TEXT NOT NULL,
  enqueued_at_epoch INTEGER NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0,
  last_error TEXT,
  next_retry_epoch INTEGER NOT NULL,
  PRIMARY KEY(repo_id, commit_sha),
  FOREIGN KEY(repo_id) REFERENCES repositories(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_file_change_extraction_queue_repo_retry
  ON file_change_extraction_queue(repo_id, next_retry_epoch ASC, enqueued_at_epoch ASC);
