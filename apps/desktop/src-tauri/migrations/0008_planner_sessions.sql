CREATE TABLE IF NOT EXISTS planner_sessions (
  id INTEGER PRIMARY KEY,
  repo_id INTEGER NOT NULL,
  entrypoint TEXT NOT NULL,
  requested_mode TEXT NOT NULL,
  effective_mode TEXT NOT NULL,
  degraded_policy TEXT NOT NULL,
  checkpoint_trust_class TEXT NOT NULL,
  planned_at_epoch INTEGER NOT NULL,
  identity_fingerprint TEXT NOT NULL,
  repository_topology TEXT NOT NULL,
  repository_is_bare INTEGER NOT NULL,
  repository_is_shallow INTEGER NOT NULL,
  repository_object_hash TEXT NOT NULL,
  repository_ref_storage TEXT NOT NULL,
  acceleration_readiness TEXT NOT NULL,
  has_commit_graph INTEGER NOT NULL,
  has_changed_path_bloom_filters INTEGER NOT NULL,
  has_multi_pack_index INTEGER NOT NULL,
  has_sparse_checkout INTEGER NOT NULL,
  has_partial_clone INTEGER NOT NULL,
  has_alternates INTEGER NOT NULL,
  has_split_index INTEGER NOT NULL,
  has_worktree_config INTEGER NOT NULL,
  FOREIGN KEY(repo_id) REFERENCES repositories(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_planner_sessions_repo_planned_at
  ON planner_sessions(repo_id, planned_at_epoch DESC);

CREATE TABLE IF NOT EXISTS planner_session_reasons (
  session_id INTEGER NOT NULL,
  reason_kind TEXT NOT NULL,
  ordinal INTEGER NOT NULL,
  reason TEXT NOT NULL,
  PRIMARY KEY(session_id, reason_kind, ordinal),
  FOREIGN KEY(session_id) REFERENCES planner_sessions(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS planner_session_feature_gates (
  session_id INTEGER NOT NULL,
  ordinal INTEGER NOT NULL,
  feature_gate TEXT NOT NULL,
  PRIMARY KEY(session_id, ordinal),
  FOREIGN KEY(session_id) REFERENCES planner_sessions(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS planner_trace_entries (
  session_id INTEGER NOT NULL,
  ordinal INTEGER NOT NULL,
  code TEXT NOT NULL,
  message TEXT NOT NULL,
  PRIMARY KEY(session_id, ordinal),
  FOREIGN KEY(session_id) REFERENCES planner_sessions(id) ON DELETE CASCADE
);
