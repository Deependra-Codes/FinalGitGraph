-- Graph-query hot path hardening:
-- move commit identity to stable integer IDs and store edges as integer IDs.

ALTER TABLE commits RENAME TO commits_v3;
CREATE TABLE commits (
  commit_id INTEGER PRIMARY KEY,
  repo_id INTEGER NOT NULL,
  sha TEXT NOT NULL,
  author_name TEXT,
  author_email TEXT,
  author_time INTEGER,
  committer_time INTEGER,
  subject TEXT,
  body TEXT,
  tree_sha TEXT,
  UNIQUE(repo_id, sha),
  FOREIGN KEY(repo_id) REFERENCES repositories(id) ON DELETE CASCADE
);
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
)
SELECT
  repo_id,
  sha,
  author_name,
  author_email,
  author_time,
  committer_time,
  subject,
  body,
  tree_sha
FROM commits_v3;
DROP TABLE commits_v3;

ALTER TABLE commit_edges RENAME TO commit_edges_v3;
CREATE TABLE commit_edges (
  repo_id INTEGER NOT NULL,
  child_commit_id INTEGER NOT NULL,
  parent_commit_id INTEGER NOT NULL,
  PRIMARY KEY(repo_id, child_commit_id, parent_commit_id),
  FOREIGN KEY(repo_id) REFERENCES repositories(id) ON DELETE CASCADE,
  FOREIGN KEY(child_commit_id) REFERENCES commits(commit_id) ON DELETE CASCADE,
  FOREIGN KEY(parent_commit_id) REFERENCES commits(commit_id) ON DELETE CASCADE
);
INSERT INTO commit_edges (repo_id, child_commit_id, parent_commit_id)
SELECT
  edge.repo_id,
  child.commit_id,
  parent.commit_id
FROM commit_edges_v3 edge
JOIN commits child
  ON child.repo_id = edge.repo_id
 AND child.sha = edge.child_sha
JOIN commits parent
  ON parent.repo_id = edge.repo_id
 AND parent.sha = edge.parent_sha;
DROP TABLE commit_edges_v3;

DROP INDEX IF EXISTS idx_commits_repo_time;
DROP INDEX IF EXISTS idx_commit_edges_repo_parent;

CREATE INDEX IF NOT EXISTS idx_commits_repo_time
  ON commits(repo_id, committer_time DESC, sha DESC);
CREATE INDEX IF NOT EXISTS idx_commits_repo_sha
  ON commits(repo_id, sha);
CREATE INDEX IF NOT EXISTS idx_commit_edges_repo_parent_id
  ON commit_edges(repo_id, parent_commit_id);
CREATE INDEX IF NOT EXISTS idx_commit_edges_repo_child_id
  ON commit_edges(repo_id, child_commit_id);
