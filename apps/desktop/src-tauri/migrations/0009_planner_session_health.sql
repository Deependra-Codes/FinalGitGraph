ALTER TABLE planner_sessions
  ADD COLUMN risk_level TEXT NOT NULL DEFAULT 'nominal';

ALTER TABLE planner_sessions
  ADD COLUMN maintenance_freshness TEXT NOT NULL DEFAULT 'unknown';
