CREATE TABLE IF NOT EXISTS repositories (
  id INTEGER PRIMARY KEY,
  path TEXT NOT NULL UNIQUE,
  name TEXT NOT NULL,
  git_dir TEXT NOT NULL,
  default_branch TEXT,
  indexed_at TEXT
);

CREATE TABLE IF NOT EXISTS commits (
  sha TEXT PRIMARY KEY,
  repo_id INTEGER NOT NULL,
  author_name TEXT,
  author_email TEXT,
  author_time INTEGER,
  committer_time INTEGER,
  subject TEXT,
  body TEXT,
  tree_sha TEXT,
  FOREIGN KEY(repo_id) REFERENCES repositories(id)
);

CREATE TABLE IF NOT EXISTS commit_edges (
  child_sha TEXT NOT NULL,
  parent_sha TEXT NOT NULL,
  repo_id INTEGER NOT NULL,
  PRIMARY KEY(child_sha, parent_sha)
);

CREATE TABLE IF NOT EXISTS refs (
  repo_id INTEGER NOT NULL,
  ref_name TEXT NOT NULL,
  target_sha TEXT NOT NULL,
  ref_type TEXT NOT NULL,
  PRIMARY KEY(repo_id, ref_name)
);

CREATE TABLE IF NOT EXISTS file_changes (
  repo_id INTEGER NOT NULL,
  commit_sha TEXT NOT NULL,
  file_path TEXT NOT NULL,
  additions INTEGER NOT NULL,
  deletions INTEGER NOT NULL,
  change_type TEXT NOT NULL,
  PRIMARY KEY(repo_id, commit_sha, file_path)
);

CREATE TABLE IF NOT EXISTS simulation_runs (
  id INTEGER PRIMARY KEY,
  repo_id INTEGER NOT NULL,
  operation_type TEXT NOT NULL,
  base_ref TEXT,
  source_ref TEXT,
  target_ref TEXT,
  status TEXT NOT NULL,
  started_at TEXT NOT NULL,
  finished_at TEXT
);

CREATE TABLE IF NOT EXISTS simulation_conflicts (
  run_id INTEGER NOT NULL,
  file_path TEXT NOT NULL,
  hunk_index INTEGER NOT NULL,
  conflict_type TEXT NOT NULL,
  effort_score REAL,
  PRIMARY KEY(run_id, file_path, hunk_index)
);

CREATE TABLE IF NOT EXISTS conflict_facts (
  repo_id INTEGER NOT NULL,
  merge_commit_sha TEXT NOT NULL,
  file_path TEXT NOT NULL,
  conflict_count INTEGER NOT NULL,
  recorded_at TEXT NOT NULL,
  PRIMARY KEY(repo_id, merge_commit_sha, file_path)
);

CREATE TABLE IF NOT EXISTS file_risk_scores (
  repo_id INTEGER NOT NULL,
  file_path TEXT NOT NULL,
  score REAL NOT NULL,
  factors_json TEXT NOT NULL,
  computed_at TEXT NOT NULL,
  PRIMARY KEY(repo_id, file_path)
);

CREATE TABLE IF NOT EXISTS blast_radius_runs (
  id INTEGER PRIMARY KEY,
  repo_id INTEGER NOT NULL,
  target_sha TEXT NOT NULL,
  score REAL NOT NULL,
  summary_json TEXT NOT NULL,
  computed_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS forensics_runs (
  id INTEGER PRIMARY KEY,
  repo_id INTEGER NOT NULL,
  file_path TEXT NOT NULL,
  line_start INTEGER,
  line_end INTEGER,
  summary_json TEXT NOT NULL,
  computed_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS pr_risk_scores (
  repo_id INTEGER NOT NULL,
  pr_number INTEGER NOT NULL,
  score REAL NOT NULL,
  band TEXT NOT NULL,
  factors_json TEXT NOT NULL,
  computed_at TEXT NOT NULL,
  PRIMARY KEY(repo_id, pr_number)
);

CREATE TABLE IF NOT EXISTS github_sync_state (
  repo_id INTEGER PRIMARY KEY,
  etag TEXT,
  cursor TEXT,
  last_synced_at TEXT,
  rate_remaining INTEGER,
  rate_reset_epoch INTEGER
);

CREATE INDEX IF NOT EXISTS idx_commits_repo_time
  ON commits(repo_id, committer_time DESC);
CREATE INDEX IF NOT EXISTS idx_commit_edges_parent
  ON commit_edges(parent_sha);
CREATE INDEX IF NOT EXISTS idx_refs_repo_target
  ON refs(repo_id, target_sha);
CREATE INDEX IF NOT EXISTS idx_file_changes_repo_path
  ON file_changes(repo_id, file_path);
CREATE INDEX IF NOT EXISTS idx_conflict_facts_repo_path
  ON conflict_facts(repo_id, file_path);
CREATE INDEX IF NOT EXISTS idx_pr_risk_scores_repo_score
  ON pr_risk_scores(repo_id, score DESC);

