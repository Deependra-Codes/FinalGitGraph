-- Core vs enrichment split for file-change ingestion:
-- keep metadata on hot path, defer expensive line stats via queue + JIT enrichment.

ALTER TABLE file_changes RENAME TO file_changes_v4;
CREATE TABLE file_changes (
  repo_id INTEGER NOT NULL,
  commit_sha TEXT NOT NULL,
  file_path TEXT NOT NULL,
  additions INTEGER,
  deletions INTEGER,
  change_type TEXT NOT NULL,
  stats_state TEXT NOT NULL DEFAULT 'ready' CHECK (stats_state IN ('pending', 'ready', 'failed')),
  PRIMARY KEY(repo_id, commit_sha, file_path),
  FOREIGN KEY(repo_id) REFERENCES repositories(id) ON DELETE CASCADE
);
INSERT INTO file_changes (
  repo_id,
  commit_sha,
  file_path,
  additions,
  deletions,
  change_type,
  stats_state
)
SELECT
  repo_id,
  commit_sha,
  file_path,
  additions,
  deletions,
  change_type,
  'ready' AS stats_state
FROM file_changes_v4;
DROP TABLE file_changes_v4;

CREATE INDEX IF NOT EXISTS idx_file_changes_repo_path
  ON file_changes(repo_id, file_path);
CREATE INDEX IF NOT EXISTS idx_file_changes_repo_stats_state
  ON file_changes(repo_id, stats_state);

CREATE TABLE IF NOT EXISTS file_change_enrichment_queue (
  repo_id INTEGER NOT NULL,
  commit_sha TEXT NOT NULL,
  file_path TEXT NOT NULL,
  enqueued_at_epoch INTEGER NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0,
  last_error TEXT,
  PRIMARY KEY(repo_id, commit_sha, file_path),
  FOREIGN KEY(repo_id) REFERENCES repositories(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_file_change_enrichment_queue_repo_enqueued
  ON file_change_enrichment_queue(repo_id, enqueued_at_epoch ASC);
