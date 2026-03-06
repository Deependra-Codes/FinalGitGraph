-- Industry hardening: make commit identity repo-scoped to avoid cross-repo data corruption.

ALTER TABLE commits RENAME TO commits_v1;
CREATE TABLE commits (
  sha TEXT NOT NULL,
  repo_id INTEGER NOT NULL,
  author_name TEXT,
  author_email TEXT,
  author_time INTEGER,
  committer_time INTEGER,
  subject TEXT,
  body TEXT,
  tree_sha TEXT,
  PRIMARY KEY(repo_id, sha),
  FOREIGN KEY(repo_id) REFERENCES repositories(id) ON DELETE CASCADE
);
INSERT INTO commits (
  sha,
  repo_id,
  author_name,
  author_email,
  author_time,
  committer_time,
  subject,
  body,
  tree_sha
)
SELECT
  sha,
  repo_id,
  author_name,
  author_email,
  author_time,
  committer_time,
  subject,
  body,
  tree_sha
FROM commits_v1;
DROP TABLE commits_v1;

ALTER TABLE commit_edges RENAME TO commit_edges_v1;
CREATE TABLE commit_edges (
  child_sha TEXT NOT NULL,
  parent_sha TEXT NOT NULL,
  repo_id INTEGER NOT NULL,
  PRIMARY KEY(repo_id, child_sha, parent_sha),
  FOREIGN KEY(repo_id) REFERENCES repositories(id) ON DELETE CASCADE
);
INSERT INTO commit_edges (child_sha, parent_sha, repo_id)
SELECT child_sha, parent_sha, repo_id
FROM commit_edges_v1;
DROP TABLE commit_edges_v1;

DROP INDEX IF EXISTS idx_commits_repo_time;
DROP INDEX IF EXISTS idx_commit_edges_parent;

CREATE INDEX IF NOT EXISTS idx_commits_repo_time
  ON commits(repo_id, committer_time DESC);
CREATE INDEX IF NOT EXISTS idx_commit_edges_repo_parent
  ON commit_edges(repo_id, parent_sha);
