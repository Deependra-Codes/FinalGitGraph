use std::collections::HashMap;
use std::env;
use std::fmt::Write;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use rusqlite::{params, params_from_iter, types::Value, Connection, OptionalExtension};

use crate::domain::models::{
    CheckpointRepositoryState, CheckpointTrustClass, CommitWindowCursor, CommitWindowItem,
    DegradedModeDecision, DegradedModePolicy, FileChangeExtractionQueueMetrics,
    FileChangeLineStatsUpdate, FileChangeStatsState, FileChangeStatsStateCounts, GraphDirection,
    IndexCheckpoint, IndexSessionPlan, IndexingMode, MaintenanceFreshness, ParsedCommit,
    ParsedFileChange, ParsedRef, PendingFileChangeEnrichment, PlannerCheckpointSnapshot,
    PlannerEntrypoint, PlannerFeatureGate, PlannerHealth, PlannerRiskLevel, PlannerSessionRecord,
    PlannerSessionRepositorySnapshot, PlannerTraceEntry, RepositoryCapabilities,
    RepositoryRefStorage, RepositoryTopology,
};
use crate::support::clock::now_epoch_seconds;
use crate::support::error::{AppError, AppResult};

const MIGRATION_0001: &str = include_str!("../../migrations/0001_initial.sql");
const MIGRATION_0002: &str = include_str!("../../migrations/0002_repo_scoped_commit_keys.sql");
const MIGRATION_0003: &str = include_str!("../../migrations/0003_indexing_checkpoints.sql");
const MIGRATION_0004: &str = include_str!("../../migrations/0004_integer_commit_edges.sql");
const MIGRATION_0005: &str = include_str!("../../migrations/0005_file_change_enrichment_queue.sql");
const MIGRATION_0006: &str = include_str!("../../migrations/0006_file_change_extraction_queue.sql");
const MIGRATION_0007: &str = include_str!("../../migrations/0007_checkpoint_repository_state.sql");
const MIGRATION_0008: &str = include_str!("../../migrations/0008_planner_sessions.sql");
const MIGRATION_0009: &str = include_str!("../../migrations/0009_planner_session_health.sql");
const MIGRATIONS: &[(i64, &str)] = &[
    (1, MIGRATION_0001),
    (2, MIGRATION_0002),
    (3, MIGRATION_0003),
    (4, MIGRATION_0004),
    (5, MIGRATION_0005),
    (6, MIGRATION_0006),
    (7, MIGRATION_0007),
    (8, MIGRATION_0008),
    (9, MIGRATION_0009),
];

const ORDERED_NEIGHBOR_QUERY_CHUNK_SIZE: usize = 400;
const FRONTIER_PULL_QUERY_CHUNK_SIZE: usize = 800;
const ORDERED_NEIGHBOR_STAGE_THRESHOLD: usize = ORDERED_NEIGHBOR_QUERY_CHUNK_SIZE;
const FRONTIER_PULL_STAGE_THRESHOLD: usize = FRONTIER_PULL_QUERY_CHUNK_SIZE;
const MAX_COMMIT_WINDOW_LIMIT: usize = 5_000;
const MAX_COMMIT_WINDOW_FETCH_LIMIT: usize = MAX_COMMIT_WINDOW_LIMIT + 1;
const FILE_CHANGE_ENRICHMENT_MAX_ATTEMPTS: i64 = 5;
const FILE_CHANGE_ENRICHMENT_RETRY_BASE_DELAY_SECONDS: i64 = 5;
const FILE_CHANGE_ENRICHMENT_RETRY_MAX_DELAY_SECONDS: i64 = 300;
const FILE_CHANGE_EXTRACTION_MAX_ATTEMPTS: i64 = 5;
const FILE_CHANGE_EXTRACTION_RETRY_BASE_DELAY_SECONDS: i64 = 5;
const FILE_CHANGE_EXTRACTION_RETRY_MAX_DELAY_SECONDS: i64 = 300;
const SQLITE_MAX_BIND_PARAMETERS_FALLBACK: usize = 999;
const FILE_CHANGE_UPSERT_COLUMNS: usize = 7;
const FILE_CHANGE_ENQUEUE_COLUMNS: usize = 4;
const COMMIT_UPSERT_COLUMNS: usize = 8;
#[cfg(test)]
const FILE_CHANGE_UPSERT_ROWS_PER_CHUNK: usize =
    SQLITE_MAX_BIND_PARAMETERS_FALLBACK / FILE_CHANGE_UPSERT_COLUMNS;
#[cfg(test)]
const COMMIT_UPSERT_ROWS_PER_CHUNK: usize =
    SQLITE_MAX_BIND_PARAMETERS_FALLBACK / COMMIT_UPSERT_COLUMNS;
#[cfg(test)]
const FILE_CHANGE_ENQUEUE_ROWS_PER_CHUNK: usize =
    SQLITE_MAX_BIND_PARAMETERS_FALLBACK / FILE_CHANGE_ENQUEUE_COLUMNS;

#[derive(Debug, Clone, Copy)]
struct EnrichmentRetryDecision {
    next_attempt: i64,
    is_terminal: bool,
    next_enqueued_epoch: i64,
}

#[derive(Debug, Clone, Copy)]
struct ExtractionRetryDecision {
    next_attempt: i64,
    is_terminal: bool,
    next_retry_epoch: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SqliteWriteProfile {
    Balanced,
    Throughput,
}

struct CheckpointRepositoryStateRow {
    identity_fingerprint: Option<String>,
    repository_topology: Option<String>,
    repository_is_bare: Option<i64>,
    repository_is_shallow: Option<i64>,
    repository_object_hash: Option<String>,
    repository_ref_storage: Option<String>,
    repository_has_sparse_checkout: Option<i64>,
    repository_has_partial_clone: Option<i64>,
}

struct PlannerSessionRow {
    session_id: i64,
    repo_id: i64,
    repo_path: String,
    repo_name: String,
    repo_git_dir: String,
    entrypoint: String,
    requested_mode: String,
    effective_mode: String,
    degraded_policy: String,
    checkpoint_trust_class: String,
    risk_level: String,
    maintenance_freshness: String,
    planned_at_epoch: i64,
    identity_fingerprint: String,
    repository_topology: String,
    repository_is_bare: i64,
    repository_is_shallow: i64,
    repository_object_hash: String,
    repository_ref_storage: String,
    acceleration_readiness: String,
    has_commit_graph: i64,
    has_changed_path_bloom_filters: i64,
    has_multi_pack_index: i64,
    has_sparse_checkout: i64,
    has_partial_clone: i64,
    has_alternates: i64,
    has_split_index: i64,
    has_worktree_config: i64,
}

fn sqlite_write_profile() -> SqliteWriteProfile {
    let raw = env::var("GITGRAPH_SQLITE_WRITE_PROFILE")
        .unwrap_or_else(|_| "balanced".to_string())
        .trim()
        .to_ascii_lowercase();
    if raw == "throughput" {
        SqliteWriteProfile::Throughput
    } else {
        SqliteWriteProfile::Balanced
    }
}

pub fn open_database(db_path: &Path) -> AppResult<Connection> {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let conn = Connection::open(db_path)?;
    conn.execute_batch(
        r#"
        PRAGMA foreign_keys=ON;
        PRAGMA journal_mode=WAL;
        PRAGMA synchronous=NORMAL;
        PRAGMA temp_store=MEMORY;
        PRAGMA busy_timeout=5000;
    "#,
    )?;
    if sqlite_write_profile() == SqliteWriteProfile::Throughput {
        conn.execute_batch(
            r#"
            PRAGMA cache_size=-65536;
            PRAGMA mmap_size=268435456;
            PRAGMA wal_autocheckpoint=4000;
        "#,
        )?;
    }
    // Hot graph query paths prepare many repeated statements.
    conn.set_prepared_statement_cache_capacity(256);
    Ok(conn)
}

pub fn run_migrations(conn: &mut Connection) -> AppResult<()> {
    ensure_schema_migrations_table(conn)?;

    for (version, sql) in MIGRATIONS {
        let already_applied: i64 = conn.query_row(
            "SELECT COUNT(1) FROM schema_migrations WHERE version = ?1",
            params![version],
            |row| row.get(0),
        )?;

        if already_applied == 0 {
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at_epoch) VALUES (?1, ?2)",
                params![version, now_epoch_seconds()],
            )?;
            tx.commit()?;
        }
    }

    Ok(())
}

fn ensure_schema_migrations_table(conn: &Connection) -> AppResult<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS schema_migrations (
          version INTEGER PRIMARY KEY,
          applied_at_epoch INTEGER NOT NULL
        );
    "#,
    )?;
    Ok(())
}

pub fn upsert_repository(
    conn: &Connection,
    path: &str,
    name: &str,
    git_dir: &str,
) -> AppResult<i64> {
    conn.execute(
        r#"
        INSERT INTO repositories (path, name, git_dir)
        VALUES (?1, ?2, ?3)
        ON CONFLICT(path) DO UPDATE SET
          name = excluded.name,
          git_dir = excluded.git_dir
    "#,
        params![path, name, git_dir],
    )?;

    let repo_id: i64 = conn.query_row(
        "SELECT id FROM repositories WHERE path = ?1",
        [path],
        |row| row.get(0),
    )?;
    Ok(repo_id)
}

pub fn load_repository_id_by_path(conn: &Connection, path: &str) -> AppResult<Option<i64>> {
    let repo_id = conn
        .query_row(
            "SELECT id FROM repositories WHERE path = ?1 LIMIT 1",
            [path],
            |row| row.get(0),
        )
        .optional()?;
    Ok(repo_id)
}

pub fn clear_repository_index_data(conn: &Connection, repo_id: i64) -> AppResult<()> {
    conn.execute("DELETE FROM commit_edges WHERE repo_id = ?1", [repo_id])?;
    conn.execute("DELETE FROM commits WHERE repo_id = ?1", [repo_id])?;
    conn.execute("DELETE FROM refs WHERE repo_id = ?1", [repo_id])?;
    conn.execute("DELETE FROM file_changes WHERE repo_id = ?1", [repo_id])?;
    Ok(())
}

pub fn upsert_commit_edges_from_commit_shas(
    conn: &Connection,
    repo_id: i64,
    edges: &[(String, String)],
) -> AppResult<usize> {
    if edges.is_empty() {
        return Ok(0);
    }

    conn.execute_batch(
        r#"
        CREATE TEMP TABLE IF NOT EXISTS current_materialization_commit_edges (
          child_commit_sha TEXT NOT NULL,
          parent_commit_sha TEXT NOT NULL,
          PRIMARY KEY(child_commit_sha, parent_commit_sha)
        );
        DELETE FROM current_materialization_commit_edges;
    "#,
    )?;

    {
        let mut insert_stmt = conn.prepare_cached(
            r#"
            INSERT INTO current_materialization_commit_edges (child_commit_sha, parent_commit_sha)
            VALUES (?1, ?2)
            ON CONFLICT(child_commit_sha, parent_commit_sha) DO NOTHING
        "#,
        )?;
        for (child_commit_sha, parent_commit_sha) in edges {
            insert_stmt.execute(params![child_commit_sha, parent_commit_sha])?;
        }
    }

    let inserted = conn.execute(
        r#"
        INSERT INTO commit_edges (repo_id, child_commit_id, parent_commit_id)
        SELECT
          ?1 AS repo_id,
          child.commit_id AS child_commit_id,
          parent.commit_id AS parent_commit_id
        FROM current_materialization_commit_edges edge
        JOIN commits child
          ON child.repo_id = ?1
         AND child.sha = edge.child_commit_sha
        JOIN commits parent
          ON parent.repo_id = ?1
         AND parent.sha = edge.parent_commit_sha
        ON CONFLICT(repo_id, child_commit_id, parent_commit_id) DO NOTHING
    "#,
        [repo_id],
    )?;

    Ok(inserted)
}

pub fn delete_file_changes_for_commits(
    conn: &Connection,
    repo_id: i64,
    commit_shas: &[String],
) -> AppResult<()> {
    if commit_shas.is_empty() {
        return Ok(());
    }

    conn.execute_batch(
        r#"
        CREATE TEMP TABLE IF NOT EXISTS current_pending_file_change_delete_shas (
          sha TEXT PRIMARY KEY
        );
        DELETE FROM current_pending_file_change_delete_shas;
    "#,
    )?;

    {
        let mut insert_stmt = conn.prepare_cached(
            r#"
            INSERT INTO current_pending_file_change_delete_shas (sha)
            VALUES (?1)
            ON CONFLICT(sha) DO NOTHING
        "#,
        )?;
        for sha in commit_shas {
            insert_stmt.execute([sha.as_str()])?;
        }
    }

    conn.execute(
        r#"
        DELETE FROM file_changes
        WHERE repo_id = ?1
          AND commit_sha IN (
            SELECT sha
            FROM current_pending_file_change_delete_shas
          )
    "#,
        [repo_id],
    )?;
    conn.execute(
        r#"
        DELETE FROM file_change_enrichment_queue
        WHERE repo_id = ?1
          AND commit_sha IN (
            SELECT sha
            FROM current_pending_file_change_delete_shas
          )
    "#,
        [repo_id],
    )?;

    Ok(())
}

pub fn prepare_file_change_drain_stage(conn: &Connection) -> AppResult<()> {
    conn.execute_batch(
        r#"
        CREATE TEMP TABLE IF NOT EXISTS current_staged_file_changes_for_drain (
          commit_sha TEXT NOT NULL,
          file_path TEXT NOT NULL,
          additions INTEGER,
          deletions INTEGER,
          change_type TEXT NOT NULL,
          stats_state TEXT NOT NULL,
          requires_line_stats INTEGER NOT NULL,
          PRIMARY KEY(commit_sha, file_path)
        );
        DELETE FROM current_staged_file_changes_for_drain;
    "#,
    )?;
    Ok(())
}

pub fn stage_file_change_for_drain(
    conn: &Connection,
    change: &ParsedFileChange,
) -> AppResult<usize> {
    let mut stmt = conn.prepare_cached(
        r#"
        INSERT INTO current_staged_file_changes_for_drain (
          commit_sha, file_path, additions, deletions, change_type, stats_state, requires_line_stats
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        ON CONFLICT(commit_sha, file_path) DO UPDATE SET
          additions = excluded.additions,
          deletions = excluded.deletions,
          change_type = excluded.change_type,
          stats_state = excluded.stats_state,
          requires_line_stats = excluded.requires_line_stats
    "#,
    )?;
    let inserted = stmt.execute(params![
        change.commit_sha.as_str(),
        change.file_path.as_str(),
        change.additions,
        change.deletions,
        change.change_type.as_str(),
        change.stats_state.as_str(),
        if change.requires_line_stats { 1 } else { 0 },
    ])?;
    Ok(inserted)
}

pub fn merge_file_changes_from_drain_stage(conn: &Connection, repo_id: i64) -> AppResult<usize> {
    let merged = conn.execute(
        r#"
        INSERT INTO file_changes (
          repo_id, commit_sha, file_path, additions, deletions, change_type, stats_state
        )
        SELECT
          ?1,
          stage.commit_sha,
          stage.file_path,
          stage.additions,
          stage.deletions,
          stage.change_type,
          stage.stats_state
        FROM current_staged_file_changes_for_drain stage
        WHERE 1
        ON CONFLICT(repo_id, commit_sha, file_path) DO UPDATE SET
          additions = excluded.additions,
          deletions = excluded.deletions,
          change_type = excluded.change_type,
          stats_state = excluded.stats_state
    "#,
        [repo_id],
    )?;
    Ok(merged)
}

pub fn enqueue_file_changes_for_enrichment_from_drain_stage(
    conn: &Connection,
    repo_id: i64,
) -> AppResult<usize> {
    let now_epoch = now_epoch_seconds();
    let enqueued = conn.execute(
        r#"
        INSERT INTO file_change_enrichment_queue (
          repo_id, commit_sha, file_path, enqueued_at_epoch
        )
        SELECT
          ?1,
          stage.commit_sha,
          stage.file_path,
          ?2
        FROM current_staged_file_changes_for_drain stage
        WHERE stage.stats_state = 'pending'
          AND stage.requires_line_stats = 1
        ON CONFLICT(repo_id, commit_sha, file_path) DO UPDATE SET
          enqueued_at_epoch = excluded.enqueued_at_epoch,
          attempts = 0,
          last_error = NULL
    "#,
        params![repo_id, now_epoch],
    )?;
    Ok(enqueued)
}

fn sqlite_max_bind_parameters(conn: &Connection) -> usize {
    static SQLITE_MAX_BIND_PARAMETERS: OnceLock<usize> = OnceLock::new();
    *SQLITE_MAX_BIND_PARAMETERS.get_or_init(|| {
        conn.query_row("PRAGMA max_variable_number", [], |row| row.get::<_, i64>(0))
            .optional()
            .ok()
            .flatten()
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| *value > 0)
            .unwrap_or(SQLITE_MAX_BIND_PARAMETERS_FALLBACK)
    })
}

fn rows_per_chunk(conn: &Connection, columns_per_row: usize) -> usize {
    if columns_per_row == 0 {
        return 1;
    }
    let max_bind_parameters = sqlite_max_bind_parameters(conn);
    max_bind_parameters
        .checked_div(columns_per_row)
        .unwrap_or(1)
        .max(1)
}

fn cached_file_change_upsert_sql(row_count: usize) -> Arc<str> {
    static CACHE: OnceLock<Mutex<HashMap<usize, Arc<str>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some(cached) = guard.get(&row_count) {
        return Arc::clone(cached);
    }
    let sql: Arc<str> = build_file_change_upsert_sql(row_count).into();
    guard.insert(row_count, Arc::clone(&sql));
    sql
}

fn cached_commit_upsert_sql(row_count: usize) -> Arc<str> {
    static CACHE: OnceLock<Mutex<HashMap<usize, Arc<str>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some(cached) = guard.get(&row_count) {
        return Arc::clone(cached);
    }
    let sql: Arc<str> = build_commit_upsert_sql(row_count).into();
    guard.insert(row_count, Arc::clone(&sql));
    sql
}

fn cached_file_change_insert_sql(row_count: usize) -> Arc<str> {
    static CACHE: OnceLock<Mutex<HashMap<usize, Arc<str>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some(cached) = guard.get(&row_count) {
        return Arc::clone(cached);
    }
    let sql: Arc<str> = build_file_change_insert_sql(row_count).into();
    guard.insert(row_count, Arc::clone(&sql));
    sql
}

fn cached_commit_insert_sql(row_count: usize) -> Arc<str> {
    static CACHE: OnceLock<Mutex<HashMap<usize, Arc<str>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some(cached) = guard.get(&row_count) {
        return Arc::clone(cached);
    }
    let sql: Arc<str> = build_commit_insert_sql(row_count).into();
    guard.insert(row_count, Arc::clone(&sql));
    sql
}

fn cached_file_change_enrichment_enqueue_sql(row_count: usize) -> Arc<str> {
    static CACHE: OnceLock<Mutex<HashMap<usize, Arc<str>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some(cached) = guard.get(&row_count) {
        return Arc::clone(cached);
    }
    let sql: Arc<str> = build_file_change_enrichment_enqueue_sql(row_count).into();
    guard.insert(row_count, Arc::clone(&sql));
    sql
}

pub fn upsert_file_changes(
    conn: &Connection,
    repo_id: i64,
    file_changes: &[ParsedFileChange],
) -> AppResult<usize> {
    if file_changes.is_empty() {
        return Ok(0);
    }

    let mut inserted = 0usize;
    let chunk_size = rows_per_chunk(conn, FILE_CHANGE_UPSERT_COLUMNS);
    let mut chunks = file_changes.chunks_exact(chunk_size);
    if file_changes.len() >= chunk_size {
        let full_sql = cached_file_change_upsert_sql(chunk_size);
        let mut full_stmt = conn.prepare_cached(full_sql.as_ref())?;
        let mut bind_values = Vec::with_capacity(chunk_size * FILE_CHANGE_UPSERT_COLUMNS);
        for chunk in &mut chunks {
            bind_values.clear();
            for change in chunk {
                bind_values.push(Value::Integer(repo_id));
                bind_values.push(Value::Text(change.commit_sha.clone()));
                bind_values.push(Value::Text(change.file_path.clone()));
                bind_values.push(match change.additions {
                    Some(number) => Value::Integer(number),
                    None => Value::Null,
                });
                bind_values.push(match change.deletions {
                    Some(number) => Value::Integer(number),
                    None => Value::Null,
                });
                bind_values.push(Value::Text(change.change_type.clone()));
                bind_values.push(Value::Text(change.stats_state.as_str().to_string()));
            }

            full_stmt.execute(params_from_iter(bind_values.iter()))?;
            inserted = inserted.saturating_add(chunk.len());
        }
    }

    let remainder = chunks.remainder();
    if !remainder.is_empty() {
        let remainder_sql = cached_file_change_upsert_sql(remainder.len());
        let mut remainder_stmt = conn.prepare_cached(remainder_sql.as_ref())?;
        let mut bind_values = Vec::with_capacity(remainder.len() * FILE_CHANGE_UPSERT_COLUMNS);
        for change in remainder {
            bind_values.push(Value::Integer(repo_id));
            bind_values.push(Value::Text(change.commit_sha.clone()));
            bind_values.push(Value::Text(change.file_path.clone()));
            bind_values.push(match change.additions {
                Some(number) => Value::Integer(number),
                None => Value::Null,
            });
            bind_values.push(match change.deletions {
                Some(number) => Value::Integer(number),
                None => Value::Null,
            });
            bind_values.push(Value::Text(change.change_type.clone()));
            bind_values.push(Value::Text(change.stats_state.as_str().to_string()));
        }
        remainder_stmt.execute(params_from_iter(bind_values.iter()))?;
        inserted = inserted.saturating_add(remainder.len());
    }

    Ok(inserted)
}

pub fn insert_file_changes_batch(
    conn: &Connection,
    repo_id: i64,
    file_changes: &[ParsedFileChange],
) -> AppResult<usize> {
    if file_changes.is_empty() {
        return Ok(0);
    }

    let mut inserted = 0usize;
    let chunk_size = rows_per_chunk(conn, FILE_CHANGE_UPSERT_COLUMNS);
    let mut chunks = file_changes.chunks_exact(chunk_size);
    if file_changes.len() >= chunk_size {
        let full_sql = cached_file_change_insert_sql(chunk_size);
        let mut full_stmt = conn.prepare_cached(full_sql.as_ref())?;
        let mut bind_values = Vec::with_capacity(chunk_size * FILE_CHANGE_UPSERT_COLUMNS);
        for chunk in &mut chunks {
            bind_values.clear();
            for change in chunk {
                bind_values.push(Value::Integer(repo_id));
                bind_values.push(Value::Text(change.commit_sha.clone()));
                bind_values.push(Value::Text(change.file_path.clone()));
                bind_values.push(match change.additions {
                    Some(number) => Value::Integer(number),
                    None => Value::Null,
                });
                bind_values.push(match change.deletions {
                    Some(number) => Value::Integer(number),
                    None => Value::Null,
                });
                bind_values.push(Value::Text(change.change_type.clone()));
                bind_values.push(Value::Text(change.stats_state.as_str().to_string()));
            }

            let applied = full_stmt.execute(params_from_iter(bind_values.iter()))?;
            inserted = inserted.saturating_add(applied);
        }
    }

    let remainder = chunks.remainder();
    if !remainder.is_empty() {
        let remainder_sql = cached_file_change_insert_sql(remainder.len());
        let mut remainder_stmt = conn.prepare_cached(remainder_sql.as_ref())?;
        let mut bind_values = Vec::with_capacity(remainder.len() * FILE_CHANGE_UPSERT_COLUMNS);
        for change in remainder {
            bind_values.push(Value::Integer(repo_id));
            bind_values.push(Value::Text(change.commit_sha.clone()));
            bind_values.push(Value::Text(change.file_path.clone()));
            bind_values.push(match change.additions {
                Some(number) => Value::Integer(number),
                None => Value::Null,
            });
            bind_values.push(match change.deletions {
                Some(number) => Value::Integer(number),
                None => Value::Null,
            });
            bind_values.push(Value::Text(change.change_type.clone()));
            bind_values.push(Value::Text(change.stats_state.as_str().to_string()));
        }
        let applied = remainder_stmt.execute(params_from_iter(bind_values.iter()))?;
        inserted = inserted.saturating_add(applied);
    }

    Ok(inserted)
}

pub fn upsert_commits_batch(
    conn: &Connection,
    repo_id: i64,
    commits: &[ParsedCommit],
) -> AppResult<usize> {
    if commits.is_empty() {
        return Ok(0);
    }

    let mut inserted = 0usize;
    let chunk_size = rows_per_chunk(conn, COMMIT_UPSERT_COLUMNS);
    let mut chunks = commits.chunks_exact(chunk_size);
    if commits.len() >= chunk_size {
        let full_sql = cached_commit_upsert_sql(chunk_size);
        let mut full_stmt = conn.prepare_cached(full_sql.as_ref())?;
        let mut bind_values = Vec::with_capacity(chunk_size * COMMIT_UPSERT_COLUMNS);
        for chunk in &mut chunks {
            bind_values.clear();
            for commit in chunk {
                bind_values.push(Value::Text(commit.sha.clone()));
                bind_values.push(Value::Integer(repo_id));
                bind_values.push(Value::Text(commit.author_name.clone()));
                bind_values.push(Value::Text(commit.author_email.clone()));
                bind_values.push(Value::Integer(commit.author_time));
                bind_values.push(Value::Integer(commit.committer_time));
                bind_values.push(Value::Text(commit.subject.clone()));
                bind_values.push(Value::Text(commit.tree_sha.clone()));
            }

            full_stmt.execute(params_from_iter(bind_values.iter()))?;
            inserted = inserted.saturating_add(chunk.len());
        }
    }

    let remainder = chunks.remainder();
    if !remainder.is_empty() {
        let remainder_sql = cached_commit_upsert_sql(remainder.len());
        let mut remainder_stmt = conn.prepare_cached(remainder_sql.as_ref())?;
        let mut bind_values = Vec::with_capacity(remainder.len() * COMMIT_UPSERT_COLUMNS);
        for commit in remainder {
            bind_values.push(Value::Text(commit.sha.clone()));
            bind_values.push(Value::Integer(repo_id));
            bind_values.push(Value::Text(commit.author_name.clone()));
            bind_values.push(Value::Text(commit.author_email.clone()));
            bind_values.push(Value::Integer(commit.author_time));
            bind_values.push(Value::Integer(commit.committer_time));
            bind_values.push(Value::Text(commit.subject.clone()));
            bind_values.push(Value::Text(commit.tree_sha.clone()));
        }
        remainder_stmt.execute(params_from_iter(bind_values.iter()))?;
        inserted = inserted.saturating_add(remainder.len());
    }

    Ok(inserted)
}

pub fn insert_commits_batch(
    conn: &Connection,
    repo_id: i64,
    commits: &[ParsedCommit],
) -> AppResult<usize> {
    if commits.is_empty() {
        return Ok(0);
    }

    let mut inserted = 0usize;
    let chunk_size = rows_per_chunk(conn, COMMIT_UPSERT_COLUMNS);
    let mut chunks = commits.chunks_exact(chunk_size);
    if commits.len() >= chunk_size {
        let full_sql = cached_commit_insert_sql(chunk_size);
        let mut full_stmt = conn.prepare_cached(full_sql.as_ref())?;
        let mut bind_values = Vec::with_capacity(chunk_size * COMMIT_UPSERT_COLUMNS);
        for chunk in &mut chunks {
            bind_values.clear();
            for commit in chunk {
                bind_values.push(Value::Text(commit.sha.clone()));
                bind_values.push(Value::Integer(repo_id));
                bind_values.push(Value::Text(commit.author_name.clone()));
                bind_values.push(Value::Text(commit.author_email.clone()));
                bind_values.push(Value::Integer(commit.author_time));
                bind_values.push(Value::Integer(commit.committer_time));
                bind_values.push(Value::Text(commit.subject.clone()));
                bind_values.push(Value::Text(commit.tree_sha.clone()));
            }

            let applied = full_stmt.execute(params_from_iter(bind_values.iter()))?;
            inserted = inserted.saturating_add(applied);
        }
    }

    let remainder = chunks.remainder();
    if !remainder.is_empty() {
        let remainder_sql = cached_commit_insert_sql(remainder.len());
        let mut remainder_stmt = conn.prepare_cached(remainder_sql.as_ref())?;
        let mut bind_values = Vec::with_capacity(remainder.len() * COMMIT_UPSERT_COLUMNS);
        for commit in remainder {
            bind_values.push(Value::Text(commit.sha.clone()));
            bind_values.push(Value::Integer(repo_id));
            bind_values.push(Value::Text(commit.author_name.clone()));
            bind_values.push(Value::Text(commit.author_email.clone()));
            bind_values.push(Value::Integer(commit.author_time));
            bind_values.push(Value::Integer(commit.committer_time));
            bind_values.push(Value::Text(commit.subject.clone()));
            bind_values.push(Value::Text(commit.tree_sha.clone()));
        }
        let applied = remainder_stmt.execute(params_from_iter(bind_values.iter()))?;
        inserted = inserted.saturating_add(applied);
    }

    Ok(inserted)
}

pub fn enqueue_file_changes_for_enrichment(
    conn: &Connection,
    repo_id: i64,
    file_changes: &[ParsedFileChange],
) -> AppResult<usize> {
    if file_changes.is_empty() {
        return Ok(0);
    }

    let eligible: Vec<&ParsedFileChange> = file_changes
        .iter()
        .filter(|change| {
            change.stats_state == FileChangeStatsState::Pending && change.requires_line_stats
        })
        .collect();
    if eligible.is_empty() {
        return Ok(0);
    }

    let mut enqueued = 0usize;
    let now_epoch = now_epoch_seconds();
    let chunk_size = rows_per_chunk(conn, FILE_CHANGE_ENQUEUE_COLUMNS);
    let mut chunks = eligible.chunks_exact(chunk_size);
    if eligible.len() >= chunk_size {
        let full_sql = cached_file_change_enrichment_enqueue_sql(chunk_size);
        let mut full_stmt = conn.prepare_cached(full_sql.as_ref())?;
        let mut bind_values = Vec::with_capacity(chunk_size * FILE_CHANGE_ENQUEUE_COLUMNS);
        for chunk in &mut chunks {
            bind_values.clear();
            for change in chunk {
                bind_values.push(Value::Integer(repo_id));
                bind_values.push(Value::Text(change.commit_sha.clone()));
                bind_values.push(Value::Text(change.file_path.clone()));
                bind_values.push(Value::Integer(now_epoch));
            }

            full_stmt.execute(params_from_iter(bind_values.iter()))?;
            enqueued = enqueued.saturating_add(chunk.len());
        }
    }

    let remainder = chunks.remainder();
    if !remainder.is_empty() {
        let remainder_sql = cached_file_change_enrichment_enqueue_sql(remainder.len());
        let mut remainder_stmt = conn.prepare_cached(remainder_sql.as_ref())?;
        let mut bind_values = Vec::with_capacity(remainder.len() * FILE_CHANGE_ENQUEUE_COLUMNS);
        for change in remainder {
            bind_values.push(Value::Integer(repo_id));
            bind_values.push(Value::Text(change.commit_sha.clone()));
            bind_values.push(Value::Text(change.file_path.clone()));
            bind_values.push(Value::Integer(now_epoch));
        }

        remainder_stmt.execute(params_from_iter(bind_values.iter()))?;
        enqueued = enqueued.saturating_add(remainder.len());
    }

    Ok(enqueued)
}

pub fn enqueue_commits_for_file_change_extraction(
    conn: &Connection,
    repo_id: i64,
    commit_shas: &[String],
) -> AppResult<usize> {
    if commit_shas.is_empty() {
        return Ok(0);
    }

    conn.execute_batch(
        r#"
        CREATE TEMP TABLE IF NOT EXISTS current_pending_file_change_extraction_commits (
          sha TEXT PRIMARY KEY
        );
        DELETE FROM current_pending_file_change_extraction_commits;
    "#,
    )?;

    {
        let mut insert_stmt = conn.prepare_cached(
            r#"
            INSERT INTO current_pending_file_change_extraction_commits (sha)
            VALUES (?1)
            ON CONFLICT(sha) DO NOTHING
        "#,
        )?;
        for sha in commit_shas {
            insert_stmt.execute([sha.as_str()])?;
        }
    }

    let now_epoch = now_epoch_seconds();
    conn.execute(
        r#"
        INSERT INTO file_change_extraction_queue (
          repo_id, commit_sha, enqueued_at_epoch, attempts, last_error, next_retry_epoch
        )
        SELECT ?1, pending.sha, ?2, 0, NULL, ?2
        FROM current_pending_file_change_extraction_commits pending
        WHERE 1
        ON CONFLICT(repo_id, commit_sha) DO UPDATE SET
          enqueued_at_epoch = excluded.enqueued_at_epoch,
          attempts = 0,
          last_error = NULL,
          next_retry_epoch = excluded.next_retry_epoch
    "#,
        params![repo_id, now_epoch],
    )?;

    let enqueued: i64 = conn.query_row(
        "SELECT COUNT(1) FROM current_pending_file_change_extraction_commits",
        [],
        |row| row.get(0),
    )?;
    Ok(enqueued.max(0) as usize)
}

pub fn load_pending_file_change_extraction_commits(
    conn: &Connection,
    repo_id: i64,
    limit: usize,
) -> AppResult<Vec<String>> {
    let bounded_limit = limit.max(1);
    let now_epoch = now_epoch_seconds();
    let mut stmt = conn.prepare_cached(
        r#"
        SELECT commit_sha
        FROM file_change_extraction_queue
        WHERE repo_id = ?1
          AND attempts < ?2
          AND next_retry_epoch <= ?3
        ORDER BY next_retry_epoch ASC, enqueued_at_epoch ASC, commit_sha ASC
        LIMIT ?4
    "#,
    )?;
    let rows = stmt.query_map(
        params![
            repo_id,
            FILE_CHANGE_EXTRACTION_MAX_ATTEMPTS,
            now_epoch,
            bounded_limit as i64
        ],
        |row| row.get::<_, String>(0),
    )?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

pub fn clear_file_change_extraction_queue_for_commits(
    conn: &Connection,
    repo_id: i64,
    commit_shas: &[String],
) -> AppResult<()> {
    if commit_shas.is_empty() {
        return Ok(());
    }

    conn.execute_batch(
        r#"
        CREATE TEMP TABLE IF NOT EXISTS current_completed_file_change_extraction_commits (
          sha TEXT PRIMARY KEY
        );
        DELETE FROM current_completed_file_change_extraction_commits;
    "#,
    )?;

    {
        let mut insert_stmt = conn.prepare_cached(
            r#"
            INSERT INTO current_completed_file_change_extraction_commits (sha)
            VALUES (?1)
            ON CONFLICT(sha) DO NOTHING
        "#,
        )?;
        for sha in commit_shas {
            insert_stmt.execute([sha.as_str()])?;
        }
    }

    conn.execute(
        r#"
        DELETE FROM file_change_extraction_queue
        WHERE repo_id = ?1
          AND commit_sha IN (
            SELECT sha
            FROM current_completed_file_change_extraction_commits
          )
    "#,
        [repo_id],
    )?;

    Ok(())
}

pub fn mark_file_change_extraction_failed(
    conn: &Connection,
    repo_id: i64,
    commit_shas: &[String],
    error_message: &str,
) -> AppResult<()> {
    if commit_shas.is_empty() {
        return Ok(());
    }

    let now_epoch = now_epoch_seconds();
    let mut load_attempts_stmt = conn.prepare_cached(
        r#"
        SELECT attempts
        FROM file_change_extraction_queue
        WHERE repo_id = ?1
          AND commit_sha = ?2
        LIMIT 1
    "#,
    )?;
    let mut queue_stmt = conn.prepare_cached(
        r#"
        INSERT INTO file_change_extraction_queue (
          repo_id, commit_sha, enqueued_at_epoch, attempts, last_error, next_retry_epoch
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        ON CONFLICT(repo_id, commit_sha) DO UPDATE SET
          attempts = excluded.attempts,
          last_error = excluded.last_error,
          next_retry_epoch = excluded.next_retry_epoch
    "#,
    )?;

    for commit_sha in commit_shas {
        let current_attempts = load_attempts_stmt
            .query_row(params![repo_id, commit_sha.as_str()], |row| {
                row.get::<_, i64>(0)
            })
            .optional()?
            .unwrap_or_default();
        let retry_decision = decide_extraction_retry(current_attempts, now_epoch);
        let final_attempts = if retry_decision.is_terminal {
            FILE_CHANGE_EXTRACTION_MAX_ATTEMPTS
        } else {
            retry_decision.next_attempt
        };

        queue_stmt.execute(params![
            repo_id,
            commit_sha.as_str(),
            now_epoch,
            final_attempts,
            error_message,
            retry_decision.next_retry_epoch,
        ])?;
    }

    Ok(())
}

pub fn load_file_change_extraction_queue_metrics(
    conn: &Connection,
    repo_id: i64,
) -> AppResult<FileChangeExtractionQueueMetrics> {
    let (depth, oldest_epoch): (i64, Option<i64>) = conn.query_row(
        r#"
        SELECT COUNT(1), MIN(enqueued_at_epoch)
        FROM file_change_extraction_queue
        WHERE repo_id = ?1
    "#,
        [repo_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    let now_epoch = now_epoch_seconds();
    let oldest_queue_age_seconds = oldest_epoch
        .map(|epoch| now_epoch.saturating_sub(epoch).max(0))
        .unwrap_or(0);

    Ok(FileChangeExtractionQueueMetrics {
        depth: depth.max(0) as usize,
        oldest_queue_age_seconds,
    })
}

fn build_file_change_upsert_sql(row_count: usize) -> String {
    let mut sql = String::from(
        r#"
        INSERT INTO file_changes (
          repo_id, commit_sha, file_path, additions, deletions, change_type, stats_state
        ) VALUES
    "#,
    );
    append_values_placeholders(&mut sql, row_count, FILE_CHANGE_UPSERT_COLUMNS);
    sql.push_str(
        r#"
        ON CONFLICT(repo_id, commit_sha, file_path) DO UPDATE SET
          additions = excluded.additions,
          deletions = excluded.deletions,
          change_type = excluded.change_type,
          stats_state = excluded.stats_state
    "#,
    );
    sql
}

fn build_file_change_insert_sql(row_count: usize) -> String {
    let mut sql = String::from(
        r#"
        INSERT INTO file_changes (
          repo_id, commit_sha, file_path, additions, deletions, change_type, stats_state
        ) VALUES
    "#,
    );
    append_values_placeholders(&mut sql, row_count, FILE_CHANGE_UPSERT_COLUMNS);
    sql.push_str(
        r#"
        ON CONFLICT(repo_id, commit_sha, file_path) DO NOTHING
    "#,
    );
    sql
}

fn build_commit_upsert_sql(row_count: usize) -> String {
    let mut sql = String::from(
        r#"
        INSERT INTO commits (
          sha, repo_id, author_name, author_email, author_time, committer_time, subject, tree_sha
        ) VALUES
    "#,
    );
    append_values_placeholders(&mut sql, row_count, COMMIT_UPSERT_COLUMNS);
    sql.push_str(
        r#"
        ON CONFLICT(repo_id, sha) DO UPDATE SET
          author_name = excluded.author_name,
          author_email = excluded.author_email,
          author_time = excluded.author_time,
          committer_time = excluded.committer_time,
          subject = excluded.subject,
          tree_sha = excluded.tree_sha
    "#,
    );
    sql
}

fn build_commit_insert_sql(row_count: usize) -> String {
    let mut sql = String::from(
        r#"
        INSERT INTO commits (
          sha, repo_id, author_name, author_email, author_time, committer_time, subject, tree_sha
        ) VALUES
    "#,
    );
    append_values_placeholders(&mut sql, row_count, COMMIT_UPSERT_COLUMNS);
    sql.push_str(
        r#"
        ON CONFLICT(repo_id, sha) DO NOTHING
    "#,
    );
    sql
}

fn build_file_change_enrichment_enqueue_sql(row_count: usize) -> String {
    let mut sql = String::from(
        r#"
        INSERT INTO file_change_enrichment_queue (
          repo_id, commit_sha, file_path, enqueued_at_epoch
        ) VALUES
    "#,
    );
    append_values_placeholders(&mut sql, row_count, FILE_CHANGE_ENQUEUE_COLUMNS);
    sql.push_str(
        r#"
        ON CONFLICT(repo_id, commit_sha, file_path) DO UPDATE SET
          enqueued_at_epoch = excluded.enqueued_at_epoch,
          attempts = 0,
          last_error = NULL
    "#,
    );
    sql
}

fn append_values_placeholders(sql: &mut String, row_count: usize, columns_per_row: usize) {
    for row_index in 0..row_count {
        if row_index > 0 {
            sql.push_str(", ");
        }
        sql.push('(');
        for column_index in 0..columns_per_row {
            if column_index > 0 {
                sql.push_str(", ");
            }
            let parameter_index = row_index * columns_per_row + column_index + 1;
            let _ = write!(sql, "?{parameter_index}");
        }
        sql.push(')');
    }
}

pub fn load_pending_file_change_enrichment(
    conn: &Connection,
    repo_id: i64,
    limit: usize,
) -> AppResult<Vec<PendingFileChangeEnrichment>> {
    let bounded_limit = limit.max(1);
    let now_epoch = now_epoch_seconds();
    let mut stmt = conn.prepare_cached(
        r#"
        SELECT q.commit_sha, q.file_path
        FROM file_change_enrichment_queue q
        JOIN file_changes f
          ON f.repo_id = q.repo_id
         AND f.commit_sha = q.commit_sha
         AND f.file_path = q.file_path
        WHERE q.repo_id = ?1
          AND f.stats_state = 'pending'
          AND q.attempts < ?2
          AND q.enqueued_at_epoch <= ?3
        ORDER BY q.enqueued_at_epoch ASC, q.commit_sha ASC, q.file_path ASC
        LIMIT ?4
    "#,
    )?;
    let rows = stmt.query_map(
        params![
            repo_id,
            FILE_CHANGE_ENRICHMENT_MAX_ATTEMPTS,
            now_epoch,
            bounded_limit as i64
        ],
        |row| {
            Ok(PendingFileChangeEnrichment {
                commit_sha: row.get(0)?,
                file_path: row.get(1)?,
            })
        },
    )?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

pub fn load_pending_file_change_enrichment_for_commits(
    conn: &Connection,
    repo_id: i64,
    commit_shas: &[String],
    limit: usize,
) -> AppResult<Vec<PendingFileChangeEnrichment>> {
    if commit_shas.is_empty() {
        return Ok(Vec::new());
    }

    conn.execute_batch(
        r#"
        CREATE TEMP TABLE IF NOT EXISTS current_pending_file_change_enrichment_commits (
          sha TEXT PRIMARY KEY
        );
        DELETE FROM current_pending_file_change_enrichment_commits;
    "#,
    )?;

    {
        let mut insert_stmt = conn.prepare_cached(
            r#"
            INSERT INTO current_pending_file_change_enrichment_commits (sha)
            VALUES (?1)
            ON CONFLICT(sha) DO NOTHING
        "#,
        )?;
        for sha in commit_shas {
            insert_stmt.execute([sha.as_str()])?;
        }
    }

    let bounded_limit = limit.max(1);
    let now_epoch = now_epoch_seconds();
    let mut stmt = conn.prepare_cached(
        r#"
        SELECT q.commit_sha, q.file_path
        FROM file_change_enrichment_queue q
        JOIN current_pending_file_change_enrichment_commits c
          ON c.sha = q.commit_sha
        JOIN file_changes f
          ON f.repo_id = q.repo_id
         AND f.commit_sha = q.commit_sha
         AND f.file_path = q.file_path
        WHERE q.repo_id = ?1
          AND f.stats_state = 'pending'
          AND q.attempts < ?2
          AND q.enqueued_at_epoch <= ?3
        ORDER BY q.enqueued_at_epoch ASC, q.commit_sha ASC, q.file_path ASC
        LIMIT ?4
    "#,
    )?;
    let rows = stmt.query_map(
        params![
            repo_id,
            FILE_CHANGE_ENRICHMENT_MAX_ATTEMPTS,
            now_epoch,
            bounded_limit as i64
        ],
        |row| {
            Ok(PendingFileChangeEnrichment {
                commit_sha: row.get(0)?,
                file_path: row.get(1)?,
            })
        },
    )?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

pub fn apply_file_change_line_stats_updates(
    conn: &Connection,
    repo_id: i64,
    updates: &[FileChangeLineStatsUpdate],
) -> AppResult<usize> {
    if updates.is_empty() {
        return Ok(0);
    }

    let mut applied = 0usize;
    let mut update_stmt = conn.prepare_cached(
        r#"
        UPDATE file_changes
        SET
          additions = ?4,
          deletions = ?5,
          stats_state = 'ready'
        WHERE repo_id = ?1
          AND commit_sha = ?2
          AND file_path = ?3
    "#,
    )?;
    let mut clear_queue_stmt = conn.prepare_cached(
        r#"
        DELETE FROM file_change_enrichment_queue
        WHERE repo_id = ?1
          AND commit_sha = ?2
          AND file_path = ?3
    "#,
    )?;

    for update in updates {
        let changed = update_stmt.execute(params![
            repo_id,
            update.commit_sha.as_str(),
            update.file_path.as_str(),
            update.additions,
            update.deletions
        ])?;
        if changed > 0 {
            applied = applied.saturating_add(changed);
            clear_queue_stmt.execute(params![
                repo_id,
                update.commit_sha.as_str(),
                update.file_path.as_str()
            ])?;
        }
    }

    Ok(applied)
}

pub fn mark_file_change_enrichment_failed(
    conn: &Connection,
    repo_id: i64,
    failed_items: &[PendingFileChangeEnrichment],
    error_message: &str,
) -> AppResult<()> {
    if failed_items.is_empty() {
        return Ok(());
    }

    let mut pending_stmt = conn.prepare_cached(
        r#"
        UPDATE file_changes
        SET stats_state = 'pending'
        WHERE repo_id = ?1
          AND commit_sha = ?2
          AND file_path = ?3
    "#,
    )?;
    let mut failed_stmt = conn.prepare_cached(
        r#"
        UPDATE file_changes
        SET stats_state = 'failed'
        WHERE repo_id = ?1
          AND commit_sha = ?2
          AND file_path = ?3
    "#,
    )?;
    let mut load_attempts_stmt = conn.prepare_cached(
        r#"
        SELECT attempts
        FROM file_change_enrichment_queue
        WHERE repo_id = ?1
          AND commit_sha = ?2
          AND file_path = ?3
        LIMIT 1
    "#,
    )?;
    let mut queue_stmt = conn.prepare_cached(
        r#"
        INSERT INTO file_change_enrichment_queue (
          repo_id, commit_sha, file_path, enqueued_at_epoch, attempts, last_error
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        ON CONFLICT(repo_id, commit_sha, file_path) DO UPDATE SET
          enqueued_at_epoch = excluded.enqueued_at_epoch,
          attempts = excluded.attempts,
          last_error = excluded.last_error
    "#,
    )?;
    let now_epoch = now_epoch_seconds();

    for failed in failed_items {
        let current_attempts = load_attempts_stmt
            .query_row(
                params![
                    repo_id,
                    failed.commit_sha.as_str(),
                    failed.file_path.as_str()
                ],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or_default();
        let retry_decision = decide_enrichment_retry(current_attempts, now_epoch);

        if retry_decision.is_terminal {
            failed_stmt.execute(params![
                repo_id,
                failed.commit_sha.as_str(),
                failed.file_path.as_str()
            ])?;
        } else {
            pending_stmt.execute(params![
                repo_id,
                failed.commit_sha.as_str(),
                failed.file_path.as_str()
            ])?;
        }
        queue_stmt.execute(params![
            repo_id,
            failed.commit_sha.as_str(),
            failed.file_path.as_str(),
            retry_decision.next_enqueued_epoch,
            retry_decision.next_attempt,
            error_message,
        ])?;
    }

    Ok(())
}

fn decide_enrichment_retry(current_attempts: i64, now_epoch: i64) -> EnrichmentRetryDecision {
    let next_attempt = current_attempts.saturating_add(1);
    let is_terminal = next_attempt >= FILE_CHANGE_ENRICHMENT_MAX_ATTEMPTS;
    let next_enqueued_epoch = if is_terminal {
        now_epoch
    } else {
        now_epoch.saturating_add(enrichment_retry_delay_seconds(next_attempt))
    };
    EnrichmentRetryDecision {
        next_attempt,
        is_terminal,
        next_enqueued_epoch,
    }
}

fn enrichment_retry_delay_seconds(next_attempt: i64) -> i64 {
    let shift = next_attempt.saturating_sub(1).clamp(0, 20) as u32;
    let multiplier = 1_i64 << shift;
    FILE_CHANGE_ENRICHMENT_RETRY_BASE_DELAY_SECONDS
        .saturating_mul(multiplier)
        .min(FILE_CHANGE_ENRICHMENT_RETRY_MAX_DELAY_SECONDS)
}

fn decide_extraction_retry(current_attempts: i64, now_epoch: i64) -> ExtractionRetryDecision {
    let next_attempt = current_attempts.saturating_add(1);
    let is_terminal = next_attempt >= FILE_CHANGE_EXTRACTION_MAX_ATTEMPTS;
    let next_retry_epoch = if is_terminal {
        now_epoch
    } else {
        now_epoch.saturating_add(extraction_retry_delay_seconds(next_attempt))
    };
    ExtractionRetryDecision {
        next_attempt,
        is_terminal,
        next_retry_epoch,
    }
}

fn extraction_retry_delay_seconds(next_attempt: i64) -> i64 {
    let shift = next_attempt.saturating_sub(1).clamp(0, 20) as u32;
    let multiplier = 1_i64 << shift;
    FILE_CHANGE_EXTRACTION_RETRY_BASE_DELAY_SECONDS
        .saturating_mul(multiplier)
        .min(FILE_CHANGE_EXTRACTION_RETRY_MAX_DELAY_SECONDS)
}

pub fn load_file_change_stats_state_counts(
    conn: &Connection,
    repo_id: i64,
) -> AppResult<FileChangeStatsStateCounts> {
    let mut stmt = conn.prepare_cached(
        r#"
        SELECT stats_state, COUNT(1)
        FROM file_changes
        WHERE repo_id = ?1
        GROUP BY stats_state
    "#,
    )?;
    let rows = stmt.query_map([repo_id], |row| {
        let state: String = row.get(0)?;
        let count: i64 = row.get(1)?;
        Ok((state, count))
    })?;

    let mut counts = FileChangeStatsStateCounts::default();
    for row in rows {
        let (state, count) = row?;
        let normalized_count = count.max(0) as usize;
        match FileChangeStatsState::parse_db(state.as_str()) {
            Some(FileChangeStatsState::Pending) => counts.pending = normalized_count,
            Some(FileChangeStatsState::Ready) => counts.ready = normalized_count,
            Some(FileChangeStatsState::Failed) => counts.failed = normalized_count,
            None => {}
        }
    }

    Ok(counts)
}

pub fn synchronize_refs(conn: &Connection, repo_id: i64, refs: &[ParsedRef]) -> AppResult<usize> {
    conn.execute_batch(
        r#"
        CREATE TEMP TABLE IF NOT EXISTS current_refs (
          ref_name TEXT PRIMARY KEY
        );
        DELETE FROM current_refs;
    "#,
    )?;

    {
        let mut current_refs_stmt = conn.prepare(
            r#"
            INSERT INTO current_refs (ref_name)
            VALUES (?1)
            ON CONFLICT(ref_name) DO NOTHING
        "#,
        )?;

        let mut ref_stmt = conn.prepare(
            r#"
            INSERT INTO refs (repo_id, ref_name, target_sha, ref_type)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(repo_id, ref_name) DO UPDATE SET
              target_sha = excluded.target_sha,
              ref_type = excluded.ref_type
        "#,
        )?;

        for reference in refs {
            current_refs_stmt.execute([reference.ref_name.as_str()])?;
            ref_stmt.execute(params![
                repo_id,
                reference.ref_name.as_str(),
                reference.target_sha.as_str(),
                reference.ref_type.as_str()
            ])?;
        }
    }

    conn.execute(
        "DELETE FROM refs WHERE repo_id = ?1 AND ref_name NOT IN (SELECT ref_name FROM current_refs)",
        [repo_id],
    )?;

    Ok(refs.len())
}

pub fn set_repository_indexed_at(
    conn: &Connection,
    repo_id: i64,
    indexed_at_epoch: i64,
) -> AppResult<()> {
    conn.execute(
        "UPDATE repositories SET indexed_at = ?1 WHERE id = ?2",
        params![indexed_at_epoch.to_string(), repo_id],
    )?;
    Ok(())
}

pub fn load_repository_indexed_at(conn: &Connection, repo_id: i64) -> AppResult<Option<String>> {
    let indexed_at = conn
        .query_row(
            "SELECT indexed_at FROM repositories WHERE id = ?1 LIMIT 1",
            [repo_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    Ok(indexed_at)
}

pub fn load_index_checkpoint(
    conn: &Connection,
    repo_id: i64,
) -> AppResult<Option<IndexCheckpoint>> {
    let checkpoint = conn
        .query_row(
            r#"
            SELECT
              last_committer_time,
              last_commit_sha,
              indexed_at_epoch,
              identity_fingerprint,
              repository_topology,
              repository_is_bare,
              repository_is_shallow,
              repository_object_hash,
              repository_ref_storage,
              repository_has_sparse_checkout,
              repository_has_partial_clone
            FROM indexing_checkpoints
            WHERE repo_id = ?1
        "#,
            [repo_id],
            |row| {
                let repository_state_row = CheckpointRepositoryStateRow {
                    identity_fingerprint: row.get(3)?,
                    repository_topology: row.get(4)?,
                    repository_is_bare: row.get(5)?,
                    repository_is_shallow: row.get(6)?,
                    repository_object_hash: row.get(7)?,
                    repository_ref_storage: row.get(8)?,
                    repository_has_sparse_checkout: row.get(9)?,
                    repository_has_partial_clone: row.get(10)?,
                };

                Ok(IndexCheckpoint {
                    last_committer_time: row.get(0)?,
                    last_commit_sha: row.get(1)?,
                    indexed_at_epoch: row.get(2)?,
                    repository_state: load_checkpoint_repository_state(repository_state_row),
                })
            },
        )
        .optional()?;
    Ok(checkpoint)
}

pub fn upsert_index_checkpoint(
    conn: &Connection,
    repo_id: i64,
    checkpoint: &IndexCheckpoint,
) -> AppResult<()> {
    let repository_state = checkpoint.repository_state.as_ref();
    conn.execute(
        r#"
        INSERT INTO indexing_checkpoints (
          repo_id,
          last_committer_time,
          last_commit_sha,
          indexed_at_epoch,
          identity_fingerprint,
          repository_topology,
          repository_is_bare,
          repository_is_shallow,
          repository_object_hash,
          repository_ref_storage,
          repository_has_sparse_checkout,
          repository_has_partial_clone
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
        ON CONFLICT(repo_id) DO UPDATE SET
          last_committer_time = excluded.last_committer_time,
          last_commit_sha = excluded.last_commit_sha,
          indexed_at_epoch = excluded.indexed_at_epoch,
          identity_fingerprint = excluded.identity_fingerprint,
          repository_topology = excluded.repository_topology,
          repository_is_bare = excluded.repository_is_bare,
          repository_is_shallow = excluded.repository_is_shallow,
          repository_object_hash = excluded.repository_object_hash,
          repository_ref_storage = excluded.repository_ref_storage,
          repository_has_sparse_checkout = excluded.repository_has_sparse_checkout,
          repository_has_partial_clone = excluded.repository_has_partial_clone
    "#,
        params![
            repo_id,
            checkpoint.last_committer_time,
            checkpoint.last_commit_sha.as_str(),
            checkpoint.indexed_at_epoch,
            repository_state.map(|state| state.identity_fingerprint.as_str()),
            repository_state.map(|state| state.topology.as_str()),
            repository_state.map(|state| if state.is_bare { 1_i64 } else { 0_i64 }),
            repository_state.map(|state| if state.is_shallow { 1_i64 } else { 0_i64 }),
            repository_state.map(|state| state.object_hash.as_str()),
            repository_state.map(|state| state.ref_storage.as_str()),
            repository_state.map(|state| {
                if state.has_sparse_checkout {
                    1_i64
                } else {
                    0_i64
                }
            }),
            repository_state.map(|state| {
                if state.has_partial_clone {
                    1_i64
                } else {
                    0_i64
                }
            }),
        ],
    )?;
    Ok(())
}

pub fn persist_planner_session(
    conn: &Connection,
    repo_id: i64,
    plan: &IndexSessionPlan,
    planned_at_epoch: i64,
) -> AppResult<i64> {
    conn.execute(
        r#"
        INSERT INTO planner_sessions (
          repo_id,
          entrypoint,
          requested_mode,
          effective_mode,
          degraded_policy,
          checkpoint_trust_class,
          risk_level,
          maintenance_freshness,
          planned_at_epoch,
          identity_fingerprint,
          repository_topology,
          repository_is_bare,
          repository_is_shallow,
          repository_object_hash,
          repository_ref_storage,
          acceleration_readiness,
          has_commit_graph,
          has_changed_path_bloom_filters,
          has_multi_pack_index,
          has_sparse_checkout,
          has_partial_clone,
          has_alternates,
          has_split_index,
          has_worktree_config
        ) VALUES (
          ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19,
          ?20, ?21, ?22, ?23, ?24
        )
    "#,
        params![
            repo_id,
            plan.entrypoint.as_str(),
            match plan.requested_mode {
                crate::domain::models::IndexingMode::Full => "full",
                crate::domain::models::IndexingMode::Incremental => "incremental",
            },
            match plan.effective_mode {
                crate::domain::models::IndexingMode::Full => "full",
                crate::domain::models::IndexingMode::Incremental => "incremental",
            },
            plan.degraded_mode.policy.as_str(),
            plan.checkpoint.trust_class.as_str(),
            plan.health.risk_level.as_str(),
            plan.health.maintenance_freshness.as_str(),
            planned_at_epoch,
            plan.repository.identity_fingerprint.as_str(),
            plan.repository.topology.as_str(),
            bool_to_sqlite(plan.repository.is_bare),
            bool_to_sqlite(plan.repository.is_shallow),
            plan.capabilities.object_hash.as_str(),
            plan.capabilities.ref_storage.as_str(),
            plan.capabilities.acceleration_readiness.as_str(),
            bool_to_sqlite(plan.capabilities.has_commit_graph),
            bool_to_sqlite(plan.capabilities.has_changed_path_bloom_filters),
            bool_to_sqlite(plan.capabilities.has_multi_pack_index),
            bool_to_sqlite(plan.capabilities.has_sparse_checkout),
            bool_to_sqlite(plan.capabilities.has_partial_clone),
            bool_to_sqlite(plan.capabilities.has_alternates),
            bool_to_sqlite(plan.capabilities.has_split_index),
            bool_to_sqlite(plan.capabilities.has_worktree_config),
        ],
    )?;
    let session_id = conn.last_insert_rowid();
    persist_planner_session_reasons(
        conn,
        session_id,
        "checkpoint_invalidation",
        &plan.checkpoint.invalidation_reasons,
    )?;
    persist_planner_session_reasons(
        conn,
        session_id,
        "degraded_mode",
        &plan.degraded_mode.reasons,
    )?;
    persist_planner_session_reasons(conn, session_id, "health", &plan.health.findings)?;
    persist_planner_session_reasons(
        conn,
        session_id,
        "recommendation",
        &plan.health.recommended_actions,
    )?;
    persist_planner_session_feature_gates(conn, session_id, &plan.degraded_mode.disabled_features)?;
    persist_planner_trace_entries(conn, session_id, &plan.planner_trace)?;
    Ok(session_id)
}

fn persist_planner_session_reasons(
    conn: &Connection,
    session_id: i64,
    reason_kind: &str,
    reasons: &[String],
) -> AppResult<()> {
    if reasons.is_empty() {
        return Ok(());
    }

    let mut stmt = conn.prepare_cached(
        r#"
        INSERT INTO planner_session_reasons (session_id, reason_kind, ordinal, reason)
        VALUES (?1, ?2, ?3, ?4)
    "#,
    )?;
    for (ordinal, reason) in reasons.iter().enumerate() {
        stmt.execute(params![
            session_id,
            reason_kind,
            ordinal as i64,
            reason.as_str()
        ])?;
    }
    Ok(())
}

fn persist_planner_session_feature_gates(
    conn: &Connection,
    session_id: i64,
    disabled_features: &[crate::domain::models::PlannerFeatureGate],
) -> AppResult<()> {
    if disabled_features.is_empty() {
        return Ok(());
    }

    let mut stmt = conn.prepare_cached(
        r#"
        INSERT INTO planner_session_feature_gates (session_id, ordinal, feature_gate)
        VALUES (?1, ?2, ?3)
    "#,
    )?;
    for (ordinal, feature) in disabled_features.iter().enumerate() {
        stmt.execute(params![session_id, ordinal as i64, feature.as_str()])?;
    }
    Ok(())
}

fn persist_planner_trace_entries(
    conn: &Connection,
    session_id: i64,
    planner_trace: &[PlannerTraceEntry],
) -> AppResult<()> {
    if planner_trace.is_empty() {
        return Ok(());
    }

    let mut stmt = conn.prepare_cached(
        r#"
        INSERT INTO planner_trace_entries (session_id, ordinal, code, message)
        VALUES (?1, ?2, ?3, ?4)
    "#,
    )?;
    for (ordinal, entry) in planner_trace.iter().enumerate() {
        stmt.execute(params![
            session_id,
            ordinal as i64,
            entry.code.as_str(),
            entry.message.as_str()
        ])?;
    }
    Ok(())
}

pub fn load_latest_planner_session(
    conn: &Connection,
    repo_id: i64,
) -> AppResult<Option<PlannerSessionRecord>> {
    let session_row = conn
        .query_row(
            r#"
            SELECT
              session.id,
              session.repo_id,
              repo.path,
              repo.name,
              repo.git_dir,
              session.entrypoint,
              session.requested_mode,
              session.effective_mode,
              session.degraded_policy,
              session.checkpoint_trust_class,
              session.risk_level,
              session.maintenance_freshness,
              session.planned_at_epoch,
              session.identity_fingerprint,
              session.repository_topology,
              session.repository_is_bare,
              session.repository_is_shallow,
              session.repository_object_hash,
              session.repository_ref_storage,
              session.acceleration_readiness,
              session.has_commit_graph,
              session.has_changed_path_bloom_filters,
              session.has_multi_pack_index,
              session.has_sparse_checkout,
              session.has_partial_clone,
              session.has_alternates,
              session.has_split_index,
              session.has_worktree_config
            FROM planner_sessions session
            JOIN repositories repo
              ON repo.id = session.repo_id
            WHERE session.repo_id = ?1
            ORDER BY session.planned_at_epoch DESC, session.id DESC
            LIMIT 1
        "#,
            [repo_id],
            map_planner_session_row,
        )
        .optional()?;
    session_row
        .map(|row| hydrate_planner_session_record(conn, row))
        .transpose()
}

pub fn load_recent_planner_sessions(
    conn: &Connection,
    repo_id: i64,
    limit: usize,
) -> AppResult<Vec<PlannerSessionRecord>> {
    let normalized_limit = limit.clamp(1, 100);
    let mut stmt = conn.prepare_cached(
        r#"
        SELECT
          session.id,
          session.repo_id,
          repo.path,
          repo.name,
          repo.git_dir,
          session.entrypoint,
          session.requested_mode,
          session.effective_mode,
          session.degraded_policy,
          session.checkpoint_trust_class,
          session.risk_level,
          session.maintenance_freshness,
          session.planned_at_epoch,
          session.identity_fingerprint,
          session.repository_topology,
          session.repository_is_bare,
          session.repository_is_shallow,
          session.repository_object_hash,
          session.repository_ref_storage,
          session.acceleration_readiness,
          session.has_commit_graph,
          session.has_changed_path_bloom_filters,
          session.has_multi_pack_index,
          session.has_sparse_checkout,
          session.has_partial_clone,
          session.has_alternates,
          session.has_split_index,
          session.has_worktree_config
        FROM planner_sessions session
        JOIN repositories repo
          ON repo.id = session.repo_id
        WHERE session.repo_id = ?1
        ORDER BY session.planned_at_epoch DESC, session.id DESC
        LIMIT ?2
    "#,
    )?;
    let rows = stmt
        .query_map(
            params![repo_id, normalized_limit as i64],
            map_planner_session_row,
        )?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|row| hydrate_planner_session_record(conn, row))
        .collect()
}

fn hydrate_planner_session_record(
    conn: &Connection,
    row: PlannerSessionRow,
) -> AppResult<PlannerSessionRecord> {
    let checkpoint_invalidation_reasons =
        load_planner_session_reason_list(conn, row.session_id, "checkpoint_invalidation")?;
    let degraded_reasons = load_planner_session_reason_list(conn, row.session_id, "degraded_mode")?;
    let health_findings = load_planner_session_reason_list(conn, row.session_id, "health")?;
    let recommendations = load_planner_session_reason_list(conn, row.session_id, "recommendation")?;
    let disabled_features = load_planner_session_feature_gates(conn, row.session_id)?;
    let planner_trace = load_planner_trace_entries(conn, row.session_id)?;

    let entrypoint = PlannerEntrypoint::parse_db(row.entrypoint.as_str()).ok_or_else(|| {
        AppError::Parse(format!("invalid planner entrypoint '{}'", row.entrypoint))
    })?;
    let requested_mode = IndexingMode::parse(row.requested_mode.as_str()).ok_or_else(|| {
        AppError::Parse(format!(
            "invalid planner requested_mode '{}'",
            row.requested_mode
        ))
    })?;
    let effective_mode = IndexingMode::parse(row.effective_mode.as_str()).ok_or_else(|| {
        AppError::Parse(format!(
            "invalid planner effective_mode '{}'",
            row.effective_mode
        ))
    })?;
    let degraded_policy =
        DegradedModePolicy::parse_db(row.degraded_policy.as_str()).ok_or_else(|| {
            AppError::Parse(format!(
                "invalid planner degraded_policy '{}'",
                row.degraded_policy
            ))
        })?;
    let checkpoint_trust_class =
        CheckpointTrustClass::parse_db(row.checkpoint_trust_class.as_str()).ok_or_else(|| {
            AppError::Parse(format!(
                "invalid checkpoint trust class '{}'",
                row.checkpoint_trust_class
            ))
        })?;
    let risk_level = PlannerRiskLevel::parse_db(row.risk_level.as_str()).ok_or_else(|| {
        AppError::Parse(format!("invalid planner risk level '{}'", row.risk_level))
    })?;
    let maintenance_freshness = MaintenanceFreshness::parse_db(row.maintenance_freshness.as_str())
        .ok_or_else(|| {
            AppError::Parse(format!(
                "invalid maintenance freshness '{}'",
                row.maintenance_freshness
            ))
        })?;
    let repository_topology = RepositoryTopology::parse_db(row.repository_topology.as_str())
        .ok_or_else(|| {
            AppError::Parse(format!(
                "invalid repository topology '{}'",
                row.repository_topology
            ))
        })?;
    let repository_ref_storage =
        RepositoryRefStorage::parse_db(row.repository_ref_storage.as_str()).ok_or_else(|| {
            AppError::Parse(format!(
                "invalid repository ref storage '{}'",
                row.repository_ref_storage
            ))
        })?;
    let acceleration_readiness =
        crate::domain::models::AccelerationReadiness::parse_db(row.acceleration_readiness.as_str())
            .ok_or_else(|| {
                AppError::Parse(format!(
                    "invalid acceleration readiness '{}'",
                    row.acceleration_readiness
                ))
            })?;

    let capabilities = RepositoryCapabilities {
        object_hash: row.repository_object_hash,
        ref_storage: repository_ref_storage,
        has_commit_graph: row.has_commit_graph != 0,
        has_changed_path_bloom_filters: row.has_changed_path_bloom_filters != 0,
        has_multi_pack_index: row.has_multi_pack_index != 0,
        has_sparse_checkout: row.has_sparse_checkout != 0,
        has_partial_clone: row.has_partial_clone != 0,
        has_alternates: row.has_alternates != 0,
        has_split_index: row.has_split_index != 0,
        has_worktree_config: row.has_worktree_config != 0,
        acceleration_readiness,
    };
    let degraded_mode = DegradedModeDecision {
        policy: degraded_policy,
        reasons: degraded_reasons,
        disabled_features,
    };
    let health = PlannerHealth {
        maintenance_freshness,
        risk_level,
        findings: health_findings,
        recommended_actions: recommendations,
    };
    let repository = PlannerSessionRepositorySnapshot {
        path: row.repo_path,
        git_dir: row.repo_git_dir,
        name: row.repo_name,
        identity_fingerprint: row.identity_fingerprint.clone(),
        topology: repository_topology,
        is_bare: row.repository_is_bare != 0,
        is_shallow: row.repository_is_shallow != 0,
    };
    let checkpoint = PlannerCheckpointSnapshot {
        trust_class: checkpoint_trust_class,
        invalidation_reasons: checkpoint_invalidation_reasons,
    };

    Ok(PlannerSessionRecord {
        session_id: row.session_id,
        repo_id: row.repo_id,
        planned_at_epoch: row.planned_at_epoch,
        plan_signature: build_planner_session_signature(
            entrypoint,
            requested_mode,
            effective_mode,
            degraded_mode.policy,
            checkpoint.trust_class,
            capabilities.acceleration_readiness,
            health.risk_level,
        ),
        repository,
        entrypoint,
        requested_mode,
        effective_mode,
        capabilities,
        checkpoint,
        degraded_mode,
        health,
        planner_trace,
    })
}

fn load_planner_session_reason_list(
    conn: &Connection,
    session_id: i64,
    reason_kind: &str,
) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare_cached(
        r#"
        SELECT reason
        FROM planner_session_reasons
        WHERE session_id = ?1
          AND reason_kind = ?2
        ORDER BY ordinal ASC
    "#,
    )?;
    let rows = stmt
        .query_map(params![session_id, reason_kind], |row| row.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(rows)
}

fn load_planner_session_feature_gates(
    conn: &Connection,
    session_id: i64,
) -> AppResult<Vec<PlannerFeatureGate>> {
    let mut stmt = conn.prepare_cached(
        r#"
        SELECT feature_gate
        FROM planner_session_feature_gates
        WHERE session_id = ?1
        ORDER BY ordinal ASC
    "#,
    )?;
    let rows = stmt
        .query_map([session_id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|raw| {
            PlannerFeatureGate::parse_db(raw.as_str())
                .ok_or_else(|| AppError::Parse(format!("invalid planner feature gate '{raw}'")))
        })
        .collect()
}

fn load_planner_trace_entries(
    conn: &Connection,
    session_id: i64,
) -> AppResult<Vec<PlannerTraceEntry>> {
    let mut stmt = conn.prepare_cached(
        r#"
        SELECT code, message
        FROM planner_trace_entries
        WHERE session_id = ?1
        ORDER BY ordinal ASC
    "#,
    )?;
    let rows = stmt
        .query_map([session_id], |row| {
            Ok(PlannerTraceEntry {
                code: row.get(0)?,
                message: row.get(1)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn map_planner_session_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PlannerSessionRow> {
    Ok(PlannerSessionRow {
        session_id: row.get(0)?,
        repo_id: row.get(1)?,
        repo_path: row.get(2)?,
        repo_name: row.get(3)?,
        repo_git_dir: row.get(4)?,
        entrypoint: row.get(5)?,
        requested_mode: row.get(6)?,
        effective_mode: row.get(7)?,
        degraded_policy: row.get(8)?,
        checkpoint_trust_class: row.get(9)?,
        risk_level: row.get(10)?,
        maintenance_freshness: row.get(11)?,
        planned_at_epoch: row.get(12)?,
        identity_fingerprint: row.get(13)?,
        repository_topology: row.get(14)?,
        repository_is_bare: row.get(15)?,
        repository_is_shallow: row.get(16)?,
        repository_object_hash: row.get(17)?,
        repository_ref_storage: row.get(18)?,
        acceleration_readiness: row.get(19)?,
        has_commit_graph: row.get(20)?,
        has_changed_path_bloom_filters: row.get(21)?,
        has_multi_pack_index: row.get(22)?,
        has_sparse_checkout: row.get(23)?,
        has_partial_clone: row.get(24)?,
        has_alternates: row.get(25)?,
        has_split_index: row.get(26)?,
        has_worktree_config: row.get(27)?,
    })
}

fn build_planner_session_signature(
    entrypoint: PlannerEntrypoint,
    requested_mode: IndexingMode,
    effective_mode: IndexingMode,
    degraded_policy: DegradedModePolicy,
    checkpoint_trust_class: CheckpointTrustClass,
    acceleration_readiness: crate::domain::models::AccelerationReadiness,
    risk_level: PlannerRiskLevel,
) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}|{}",
        entrypoint.as_str(),
        match requested_mode {
            IndexingMode::Full => "full",
            IndexingMode::Incremental => "incremental",
        },
        match effective_mode {
            IndexingMode::Full => "full",
            IndexingMode::Incremental => "incremental",
        },
        degraded_policy.as_str(),
        checkpoint_trust_class.as_str(),
        acceleration_readiness.as_str(),
        risk_level.as_str()
    )
}

fn bool_to_sqlite(value: bool) -> i64 {
    if value {
        1
    } else {
        0
    }
}

fn load_checkpoint_repository_state(
    row: CheckpointRepositoryStateRow,
) -> Option<CheckpointRepositoryState> {
    let topology = RepositoryTopology::parse_db(row.repository_topology.as_deref()?)?;
    let ref_storage = RepositoryRefStorage::parse_db(row.repository_ref_storage.as_deref()?)?;
    Some(CheckpointRepositoryState {
        identity_fingerprint: row.identity_fingerprint?,
        topology,
        is_bare: row.repository_is_bare? != 0,
        is_shallow: row.repository_is_shallow? != 0,
        object_hash: row.repository_object_hash?,
        ref_storage,
        has_sparse_checkout: row.repository_has_sparse_checkout? != 0,
        has_partial_clone: row.repository_has_partial_clone? != 0,
    })
}

pub fn load_commit_window(
    conn: &Connection,
    repo_id: i64,
    limit: usize,
    cursor: Option<&CommitWindowCursor>,
) -> AppResult<Vec<CommitWindowItem>> {
    let limit = limit.min(MAX_COMMIT_WINDOW_FETCH_LIMIT);

    let mut commits = Vec::new();
    if let Some(cursor) = cursor {
        let mut stmt = conn.prepare(
            r#"
            SELECT sha, author_name, author_email, committer_time, subject, tree_sha
            FROM commits
            WHERE repo_id = ?1
              AND (committer_time < ?2 OR (committer_time = ?2 AND sha < ?3))
            ORDER BY committer_time DESC, sha DESC
            LIMIT ?4
        "#,
        )?;
        let rows = stmt.query_map(
            params![
                repo_id,
                cursor.committer_time,
                cursor.sha.as_str(),
                limit as i64
            ],
            map_commit_window_item,
        )?;

        for row in rows {
            commits.push(row?);
        }
    } else {
        let mut stmt = conn.prepare(
            r#"
            SELECT sha, author_name, author_email, committer_time, subject, tree_sha
            FROM commits
            WHERE repo_id = ?1
            ORDER BY committer_time DESC, sha DESC
            LIMIT ?2
        "#,
        )?;
        let rows = stmt.query_map(params![repo_id, limit as i64], map_commit_window_item)?;

        for row in rows {
            commits.push(row?);
        }
    }

    Ok(commits)
}

pub fn commit_exists(conn: &Connection, repo_id: i64, sha: &str) -> AppResult<bool> {
    let exists = conn
        .query_row(
            "SELECT 1 FROM commits WHERE repo_id = ?1 AND sha = ?2 LIMIT 1",
            params![repo_id, sha],
            |_row| Ok(()),
        )
        .optional()?
        .is_some();
    Ok(exists)
}

pub fn count_commits(conn: &Connection, repo_id: i64) -> AppResult<usize> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(1) FROM commits WHERE repo_id = ?1",
        [repo_id],
        |row| row.get(0),
    )?;
    Ok(count.max(0) as usize)
}

pub fn load_commit_rowid(conn: &Connection, repo_id: i64, sha: &str) -> AppResult<Option<i64>> {
    let rowid = conn
        .query_row(
            "SELECT commit_id FROM commits WHERE repo_id = ?1 AND sha = ?2 LIMIT 1",
            params![repo_id, sha],
            |row| row.get(0),
        )
        .optional()?;
    Ok(rowid)
}

pub fn load_commit_shas_by_rowids(
    conn: &Connection,
    repo_id: i64,
    rowids: &[i64],
) -> AppResult<HashMap<i64, String>> {
    if rowids.is_empty() {
        return Ok(HashMap::new());
    }

    conn.execute_batch(
        r#"
        CREATE TEMP TABLE IF NOT EXISTS current_graph_rowids (
          commit_rowid INTEGER PRIMARY KEY
        );
        DELETE FROM current_graph_rowids;
    "#,
    )?;

    {
        let mut insert_stmt = conn.prepare(
            r#"
            INSERT INTO current_graph_rowids (commit_rowid)
            VALUES (?1)
            ON CONFLICT(commit_rowid) DO NOTHING
        "#,
        )?;
        for rowid in rowids {
            insert_stmt.execute([rowid])?;
        }
    }

    let mut stmt = conn.prepare(
        r#"
        SELECT c.commit_id, c.sha
        FROM commits c
        JOIN current_graph_rowids r ON r.commit_rowid = c.commit_id
        WHERE c.repo_id = ?1
    "#,
    )?;
    let rows = stmt.query_map([repo_id], |row| {
        let rowid: i64 = row.get(0)?;
        let sha: String = row.get(1)?;
        Ok((rowid, sha))
    })?;

    let mut result = HashMap::new();
    for row in rows {
        let (rowid, sha) = row?;
        result.insert(rowid, sha);
    }

    Ok(result)
}

pub fn load_commit_shas_for_rowids(
    conn: &Connection,
    repo_id: i64,
    rowids: &[i64],
) -> AppResult<Vec<String>> {
    if rowids.is_empty() {
        return Ok(Vec::new());
    }

    conn.execute_batch(
        r#"
        CREATE TEMP TABLE IF NOT EXISTS current_graph_rowid_order (
          ord INTEGER PRIMARY KEY,
          commit_rowid INTEGER NOT NULL
        );
        DELETE FROM current_graph_rowid_order;
    "#,
    )?;

    {
        let mut insert_stmt = conn.prepare_cached(
            r#"
            INSERT INTO current_graph_rowid_order (ord, commit_rowid)
            VALUES (?1, ?2)
        "#,
        )?;
        for (ord, rowid) in rowids.iter().enumerate() {
            insert_stmt.execute(params![ord as i64, rowid])?;
        }
    }

    let mut stmt = conn.prepare_cached(
        r#"
        SELECT c.sha
        FROM current_graph_rowid_order o
        JOIN commits c ON c.commit_id = o.commit_rowid
        WHERE c.repo_id = ?1
        ORDER BY o.ord ASC
    "#,
    )?;
    let rows = stmt.query_map([repo_id], |row| row.get(0))?;

    let mut result = Vec::with_capacity(rowids.len());
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

pub fn load_neighbor_shas(
    conn: &Connection,
    repo_id: i64,
    sha: &str,
    direction: GraphDirection,
) -> AppResult<Vec<String>> {
    let batch = vec![sha.to_string()];
    let neighbors = load_neighbor_shas_batch(conn, repo_id, &batch, direction)?;
    Ok(neighbors.get(sha).cloned().unwrap_or_default())
}

pub fn load_neighbor_shas_batch(
    conn: &Connection,
    repo_id: i64,
    shas: &[String],
    direction: GraphDirection,
) -> AppResult<HashMap<String, Vec<String>>> {
    if shas.is_empty() {
        return Ok(HashMap::new());
    }

    let mut neighbors = HashMap::with_capacity(shas.len());
    for sha in shas {
        neighbors.entry(sha.clone()).or_default();
    }

    let source_commit_ids = load_commit_ids_for_shas(conn, repo_id, shas)?;
    if source_commit_ids.is_empty() {
        return Ok(neighbors);
    }

    let mut ordered_source_ids = Vec::new();
    let mut ordered_source_shas = Vec::new();
    for sha in shas {
        if let Some(commit_id) = source_commit_ids.get(sha) {
            ordered_source_ids.push(*commit_id);
            ordered_source_shas.push(sha.clone());
        }
    }

    let rowid_neighbors =
        load_neighbor_rowids_batch(conn, repo_id, &ordered_source_ids, direction)?;
    let mut neighbor_ids = Vec::new();
    for source_id in &ordered_source_ids {
        if let Some(source_neighbors) = rowid_neighbors.get(source_id) {
            neighbor_ids.extend(source_neighbors.iter().copied());
        }
    }
    neighbor_ids.sort_unstable();
    neighbor_ids.dedup();
    let neighbor_shas = load_commit_shas_by_rowids(conn, repo_id, &neighbor_ids)?;

    for (source_sha, source_id) in ordered_source_shas.iter().zip(ordered_source_ids.iter()) {
        let mut source_neighbor_shas = Vec::new();
        if let Some(source_neighbors) = rowid_neighbors.get(source_id) {
            for neighbor_id in source_neighbors {
                if let Some(neighbor_sha) = neighbor_shas.get(neighbor_id) {
                    source_neighbor_shas.push(neighbor_sha.clone());
                }
            }
        }
        neighbors.insert(source_sha.clone(), source_neighbor_shas);
    }

    Ok(neighbors)
}

fn load_commit_ids_for_shas(
    conn: &Connection,
    repo_id: i64,
    shas: &[String],
) -> AppResult<HashMap<String, i64>> {
    conn.execute_batch(
        r#"
        CREATE TEMP TABLE IF NOT EXISTS current_graph_nodes (
          sha TEXT PRIMARY KEY
        );
        DELETE FROM current_graph_nodes;
    "#,
    )?;

    {
        let mut insert_stmt = conn.prepare_cached(
            r#"
            INSERT INTO current_graph_nodes (sha)
            VALUES (?1)
            ON CONFLICT(sha) DO NOTHING
        "#,
        )?;
        for sha in shas {
            insert_stmt.execute([sha.as_str()])?;
        }
    }

    let mut stmt = conn.prepare_cached(
        r#"
        SELECT c.sha, c.commit_id
        FROM commits c
        JOIN current_graph_nodes node ON node.sha = c.sha
        WHERE c.repo_id = ?1
    "#,
    )?;
    let rows = stmt.query_map([repo_id], |row| {
        let sha: String = row.get(0)?;
        let commit_id: i64 = row.get(1)?;
        Ok((sha, commit_id))
    })?;

    let mut result = HashMap::new();
    for row in rows {
        let (sha, commit_id) = row?;
        result.insert(sha, commit_id);
    }
    Ok(result)
}

pub fn load_neighbor_rowids(
    conn: &Connection,
    repo_id: i64,
    rowid: i64,
    direction: GraphDirection,
) -> AppResult<Vec<i64>> {
    match direction {
        GraphDirection::Parents => load_parent_rowids(conn, repo_id, rowid),
        GraphDirection::Children => load_child_rowids(conn, repo_id, rowid),
        GraphDirection::Both => {
            let mut combined = load_parent_rowids(conn, repo_id, rowid)?;
            combined.extend(load_child_rowids(conn, repo_id, rowid)?);
            combined.sort();
            combined.dedup();
            combined.reverse();
            Ok(combined)
        }
    }
}

pub fn load_neighbor_rowids_batch(
    conn: &Connection,
    repo_id: i64,
    rowids: &[i64],
    direction: GraphDirection,
) -> AppResult<HashMap<i64, Vec<i64>>> {
    let ordered_neighbors = load_neighbor_rowids_for_sources(conn, repo_id, rowids, direction)?;

    let mut neighbors = HashMap::with_capacity(rowids.len());
    for (ord, source_rowid) in rowids.iter().enumerate() {
        neighbors.insert(
            *source_rowid,
            ordered_neighbors.get(ord).cloned().unwrap_or_default(),
        );
    }

    Ok(neighbors)
}

pub fn load_neighbor_rowids_for_sources(
    conn: &Connection,
    repo_id: i64,
    rowids: &[i64],
    direction: GraphDirection,
) -> AppResult<Vec<Vec<i64>>> {
    if rowids.is_empty() {
        return Ok(Vec::new());
    }

    if rowids.len() > ORDERED_NEIGHBOR_STAGE_THRESHOLD {
        stage_ordered_source_rowids(conn, rowids)?;
        return match direction {
            GraphDirection::Parents => query_neighbor_rowids_for_staged_sources(
                conn,
                repo_id,
                rowids.len(),
                "child_commit_id",
                "parent_commit_id",
            ),
            GraphDirection::Children => query_neighbor_rowids_for_staged_sources(
                conn,
                repo_id,
                rowids.len(),
                "parent_commit_id",
                "child_commit_id",
            ),
            GraphDirection::Both => {
                let mut merged = query_neighbor_rowids_for_staged_sources(
                    conn,
                    repo_id,
                    rowids.len(),
                    "child_commit_id",
                    "parent_commit_id",
                )?;
                let children = query_neighbor_rowids_for_staged_sources(
                    conn,
                    repo_id,
                    rowids.len(),
                    "parent_commit_id",
                    "child_commit_id",
                )?;
                for (merged_neighbors, child_neighbors) in
                    merged.iter_mut().zip(children.into_iter())
                {
                    merged_neighbors.extend(child_neighbors);
                    sort_and_dedup_descending(merged_neighbors);
                }
                Ok(merged)
            }
        };
    }

    let mut neighbors = Vec::with_capacity(rowids.len());
    for rowid_chunk in rowids.chunks(ORDERED_NEIGHBOR_QUERY_CHUNK_SIZE) {
        let mut chunk_neighbors = match direction {
            GraphDirection::Parents => query_neighbor_rowids_for_ordered_sources_chunk(
                conn,
                repo_id,
                rowid_chunk,
                "child_commit_id",
                "parent_commit_id",
                "edge.parent_commit_id DESC",
            )?,
            GraphDirection::Children => query_neighbor_rowids_for_ordered_sources_chunk(
                conn,
                repo_id,
                rowid_chunk,
                "parent_commit_id",
                "child_commit_id",
                "edge.child_commit_id DESC",
            )?,
            GraphDirection::Both => {
                let mut merged = query_neighbor_rowids_for_ordered_sources_chunk(
                    conn,
                    repo_id,
                    rowid_chunk,
                    "child_commit_id",
                    "parent_commit_id",
                    "edge.parent_commit_id DESC",
                )?;
                let children = query_neighbor_rowids_for_ordered_sources_chunk(
                    conn,
                    repo_id,
                    rowid_chunk,
                    "parent_commit_id",
                    "child_commit_id",
                    "edge.child_commit_id DESC",
                )?;

                for (merged_neighbors, child_neighbors) in
                    merged.iter_mut().zip(children.into_iter())
                {
                    merged_neighbors.extend(child_neighbors);
                    merged_neighbors.sort_unstable();
                    merged_neighbors.dedup();
                    merged_neighbors.reverse();
                }
                merged
            }
        };
        neighbors.append(&mut chunk_neighbors);
    }

    Ok(neighbors)
}

pub fn load_pull_neighbor_rowids(
    conn: &Connection,
    repo_id: i64,
    frontier_rowids: &[i64],
    direction: GraphDirection,
) -> AppResult<Vec<i64>> {
    if frontier_rowids.is_empty() {
        return Ok(Vec::new());
    }

    let mut combined = Vec::new();
    if frontier_rowids.len() > FRONTIER_PULL_STAGE_THRESHOLD {
        stage_frontier_rowids(conn, frontier_rowids)?;
        match direction {
            GraphDirection::Parents => {
                combined.extend(query_pull_rowids_for_staged_frontier(
                    conn,
                    repo_id,
                    "parent_commit_id",
                    "child_commit_id",
                )?);
            }
            GraphDirection::Children => {
                combined.extend(query_pull_rowids_for_staged_frontier(
                    conn,
                    repo_id,
                    "child_commit_id",
                    "parent_commit_id",
                )?);
            }
            GraphDirection::Both => {
                combined.extend(query_pull_rowids_for_staged_frontier(
                    conn,
                    repo_id,
                    "parent_commit_id",
                    "child_commit_id",
                )?);
                combined.extend(query_pull_rowids_for_staged_frontier(
                    conn,
                    repo_id,
                    "child_commit_id",
                    "parent_commit_id",
                )?);
            }
        }
    } else {
        for frontier_chunk in frontier_rowids.chunks(FRONTIER_PULL_QUERY_CHUNK_SIZE) {
            match direction {
                GraphDirection::Parents => combined.extend(query_pull_rowids_for_frontier_chunk(
                    conn,
                    repo_id,
                    frontier_chunk,
                    "parent_commit_id",
                    "child_commit_id",
                )?),
                GraphDirection::Children => combined.extend(query_pull_rowids_for_frontier_chunk(
                    conn,
                    repo_id,
                    frontier_chunk,
                    "child_commit_id",
                    "parent_commit_id",
                )?),
                GraphDirection::Both => {
                    combined.extend(query_pull_rowids_for_frontier_chunk(
                        conn,
                        repo_id,
                        frontier_chunk,
                        "parent_commit_id",
                        "child_commit_id",
                    )?);
                    combined.extend(query_pull_rowids_for_frontier_chunk(
                        conn,
                        repo_id,
                        frontier_chunk,
                        "child_commit_id",
                        "parent_commit_id",
                    )?);
                }
            }
        }
    }

    sort_and_dedup_descending(&mut combined);
    Ok(combined)
}

fn load_parent_rowids(conn: &Connection, repo_id: i64, rowid: i64) -> AppResult<Vec<i64>> {
    let mut stmt = conn.prepare_cached(
        r#"
        SELECT parent_commit_id
        FROM commit_edges
        WHERE repo_id = ?1 AND child_commit_id = ?2
        ORDER BY parent_commit_id DESC
    "#,
    )?;
    let rows = stmt.query_map(params![repo_id, rowid], |row| row.get(0))?;

    let mut rowids = Vec::new();
    for row in rows {
        rowids.push(row?);
    }

    Ok(rowids)
}

fn load_child_rowids(conn: &Connection, repo_id: i64, rowid: i64) -> AppResult<Vec<i64>> {
    let mut stmt = conn.prepare_cached(
        r#"
        SELECT child_commit_id
        FROM commit_edges
        WHERE repo_id = ?1 AND parent_commit_id = ?2
        ORDER BY child_commit_id DESC
    "#,
    )?;
    let rows = stmt.query_map(params![repo_id, rowid], |row| row.get(0))?;

    let mut rowids = Vec::new();
    for row in rows {
        rowids.push(row?);
    }

    Ok(rowids)
}

fn stage_ordered_source_rowids(conn: &Connection, rowids: &[i64]) -> AppResult<()> {
    conn.execute_batch(
        r#"
        CREATE TEMP TABLE IF NOT EXISTS current_graph_ordered_sources (
          ord INTEGER PRIMARY KEY,
          commit_rowid INTEGER NOT NULL
        );
        DELETE FROM current_graph_ordered_sources;
    "#,
    )?;

    let mut insert_stmt = conn.prepare_cached(
        r#"
        INSERT INTO current_graph_ordered_sources (ord, commit_rowid)
        VALUES (?1, ?2)
    "#,
    )?;
    for (ord, rowid) in rowids.iter().enumerate() {
        insert_stmt.execute(params![ord as i64, rowid])?;
    }
    Ok(())
}

fn stage_frontier_rowids(conn: &Connection, frontier_rowids: &[i64]) -> AppResult<()> {
    conn.execute_batch(
        r#"
        CREATE TEMP TABLE IF NOT EXISTS current_graph_frontier_rows (
          commit_rowid INTEGER PRIMARY KEY
        );
        DELETE FROM current_graph_frontier_rows;
    "#,
    )?;

    let mut insert_stmt = conn.prepare_cached(
        r#"
        INSERT INTO current_graph_frontier_rows (commit_rowid)
        VALUES (?1)
        ON CONFLICT(commit_rowid) DO NOTHING
    "#,
    )?;
    for rowid in frontier_rowids {
        insert_stmt.execute([rowid])?;
    }
    Ok(())
}

fn query_neighbor_rowids_for_staged_sources(
    conn: &Connection,
    repo_id: i64,
    rowid_count: usize,
    source_id_column: &str,
    neighbor_id_column: &str,
) -> AppResult<Vec<Vec<i64>>> {
    let sql = build_staged_ordered_neighbor_query_sql(source_id_column, neighbor_id_column);
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(params![repo_id], |row| {
        let source_ord: i64 = row.get(0)?;
        let neighbor_rowid: i64 = row.get(1)?;
        Ok((source_ord, neighbor_rowid))
    })?;

    let mut result: Vec<Vec<i64>> = vec![Vec::new(); rowid_count];
    for row in rows {
        let (source_ord, neighbor_rowid) = row?;
        if source_ord < 0 {
            continue;
        }
        let source_index = source_ord as usize;
        if source_index < result.len() {
            result[source_index].push(neighbor_rowid);
        }
    }
    for source_neighbors in &mut result {
        sort_and_dedup_descending(source_neighbors);
    }

    Ok(result)
}

fn query_pull_rowids_for_staged_frontier(
    conn: &Connection,
    repo_id: i64,
    candidate_id_column: &str,
    frontier_id_column: &str,
) -> AppResult<Vec<i64>> {
    let sql = build_staged_pull_query_sql(candidate_id_column, frontier_id_column);
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(params![repo_id], |row| row.get(0))?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

fn build_staged_ordered_neighbor_query_sql(
    source_id_column: &str,
    neighbor_id_column: &str,
) -> String {
    format!(
        r#"
        SELECT src.ord AS source_ord, edge.{neighbor_id_column} AS neighbor_rowid
        FROM current_graph_ordered_sources src
        JOIN commit_edges edge
          ON edge.repo_id = ?1
         AND edge.{source_id_column} = src.commit_rowid
    "#
    )
}

fn build_staged_pull_query_sql(candidate_id_column: &str, frontier_id_column: &str) -> String {
    format!(
        r#"
        SELECT edge.{candidate_id_column} AS neighbor_rowid
        FROM commit_edges edge
        JOIN current_graph_frontier_rows frontier
          ON frontier.commit_rowid = edge.{frontier_id_column}
        WHERE edge.repo_id = ?1
    "#
    )
}

fn sort_and_dedup_descending(rowids: &mut Vec<i64>) {
    rowids.sort_unstable_by(|left, right| right.cmp(left));
    rowids.dedup();
}

fn query_neighbor_rowids_for_ordered_sources_chunk(
    conn: &Connection,
    repo_id: i64,
    rowids: &[i64],
    source_id_column: &str,
    neighbor_id_column: &str,
    neighbor_order: &str,
) -> AppResult<Vec<Vec<i64>>> {
    if rowids.is_empty() {
        return Ok(Vec::new());
    }

    let mut ordered_values = String::new();
    for index in 0..rowids.len() {
        if index > 0 {
            ordered_values.push_str(", ");
        }
        let ord_param = index * 2 + 1;
        let rowid_param = index * 2 + 2;
        ordered_values.push_str(&format!("(?{ord_param}, ?{rowid_param})"));
    }
    let repo_param = rowids.len() * 2 + 1;

    let sql = format!(
        r#"
        WITH ordered(ord, commit_rowid) AS (
          VALUES {ordered_values}
        )
        SELECT ordered.ord AS source_ord, edge.{neighbor_id_column} AS neighbor_rowid
        FROM ordered
        JOIN commit_edges edge
          ON edge.repo_id = ?{repo_param}
         AND edge.{source_id_column} = ordered.commit_rowid
        ORDER BY ordered.ord ASC, {neighbor_order}
    "#
    );

    let mut query_params = Vec::with_capacity(rowids.len() * 2 + 1);
    for (ord, rowid) in rowids.iter().enumerate() {
        query_params.push(Value::Integer(ord as i64));
        query_params.push(Value::Integer(*rowid));
    }
    query_params.push(Value::Integer(repo_id));

    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(params_from_iter(query_params.iter()), |row| {
        let source_ord: i64 = row.get(0)?;
        let neighbor_rowid: i64 = row.get(1)?;
        Ok((source_ord, neighbor_rowid))
    })?;

    let mut result: Vec<Vec<i64>> = vec![Vec::new(); rowids.len()];
    for row in rows {
        let (source_ord, neighbor_rowid) = row?;
        if source_ord < 0 {
            continue;
        }
        let source_index = source_ord as usize;
        if source_index < result.len() {
            result[source_index].push(neighbor_rowid);
        }
    }

    Ok(result)
}

fn query_pull_rowids_for_frontier_chunk(
    conn: &Connection,
    repo_id: i64,
    frontier_rowids: &[i64],
    candidate_id_column: &str,
    frontier_id_column: &str,
) -> AppResult<Vec<i64>> {
    if frontier_rowids.is_empty() {
        return Ok(Vec::new());
    }

    let mut frontier_values = String::new();
    for index in 0..frontier_rowids.len() {
        if index > 0 {
            frontier_values.push_str(", ");
        }
        let frontier_param = index + 1;
        frontier_values.push_str(&format!("(?{frontier_param})"));
    }
    let repo_param = frontier_rowids.len() + 1;

    let sql = format!(
        r#"
        WITH frontier(commit_rowid) AS (
          VALUES {frontier_values}
        )
        SELECT DISTINCT edge.{candidate_id_column} AS neighbor_rowid
        FROM commit_edges edge
        JOIN frontier
          ON frontier.commit_rowid = edge.{frontier_id_column}
        WHERE edge.repo_id = ?{repo_param}
        ORDER BY edge.{candidate_id_column} DESC
    "#
    );

    let mut query_params = Vec::with_capacity(frontier_rowids.len() + 1);
    for frontier_rowid in frontier_rowids {
        query_params.push(Value::Integer(*frontier_rowid));
    }
    query_params.push(Value::Integer(repo_id));

    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(params_from_iter(query_params.iter()), |row| row.get(0))?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

pub fn verify_sqlite_graph_query_plan_guards(conn: &Connection, repo_id: i64) -> AppResult<()> {
    let sample_commit_id = conn
        .query_row(
            "SELECT commit_id FROM commits WHERE repo_id = ?1 ORDER BY commit_id ASC LIMIT 1",
            [repo_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    let Some(sample_commit_id) = sample_commit_id else {
        return Ok(());
    };

    stage_ordered_source_rowids(conn, &[sample_commit_id])?;
    stage_frontier_rowids(conn, &[sample_commit_id])?;

    let ordered_parent_sql =
        build_staged_ordered_neighbor_query_sql("child_commit_id", "parent_commit_id");
    assert_sqlite_query_plan_guard(
        conn,
        ordered_parent_sql.as_str(),
        repo_id,
        "ordered-source parents",
    )?;

    let ordered_child_sql =
        build_staged_ordered_neighbor_query_sql("parent_commit_id", "child_commit_id");
    assert_sqlite_query_plan_guard(
        conn,
        ordered_child_sql.as_str(),
        repo_id,
        "ordered-source children",
    )?;

    let pull_parent_sql = build_staged_pull_query_sql("parent_commit_id", "child_commit_id");
    assert_sqlite_query_plan_guard(conn, pull_parent_sql.as_str(), repo_id, "pull parents")?;

    let pull_child_sql = build_staged_pull_query_sql("child_commit_id", "parent_commit_id");
    assert_sqlite_query_plan_guard(conn, pull_child_sql.as_str(), repo_id, "pull children")?;

    Ok(())
}

fn assert_sqlite_query_plan_guard(
    conn: &Connection,
    sql: &str,
    repo_id: i64,
    label: &str,
) -> AppResult<()> {
    let explain_sql = format!("EXPLAIN QUERY PLAN {sql}");
    let mut stmt = conn.prepare_cached(&explain_sql)?;
    let rows = stmt.query_map(params![repo_id], |row| row.get::<_, String>(3))?;

    let mut details = Vec::new();
    for row in rows {
        details.push(row?);
    }

    let mut found_broad_scan = false;
    let mut found_temp_sort = false;
    for detail in &details {
        let normalized = detail.to_ascii_lowercase();
        if normalized.contains("scan edge") || normalized.contains("scan commit_edges") {
            found_broad_scan = true;
        }
        if normalized.contains("use temp b-tree") {
            found_temp_sort = true;
        }
    }

    if found_broad_scan || found_temp_sort {
        return Err(AppError::Parse(format!(
            "sqlite query-plan guard failed ({label}): broad_scan={found_broad_scan}, temp_sort={found_temp_sort}, details={details:?}"
        )));
    }

    Ok(())
}

fn map_commit_window_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<CommitWindowItem> {
    Ok(CommitWindowItem {
        sha: row.get(0)?,
        author_name: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
        author_email: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
        committer_time: row.get::<_, Option<i64>>(3)?.unwrap_or_default(),
        subject: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
        tree_sha: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use rusqlite::{params, Connection};

    use super::{
        clear_file_change_extraction_queue_for_commits, count_commits,
        delete_file_changes_for_commits, enqueue_commits_for_file_change_extraction,
        enqueue_file_changes_for_enrichment, enqueue_file_changes_for_enrichment_from_drain_stage,
        insert_commits_batch, insert_file_changes_batch, load_commit_shas_by_rowids,
        load_commit_shas_for_rowids, load_file_change_extraction_queue_metrics,
        load_index_checkpoint, load_latest_planner_session, load_neighbor_rowids_batch,
        load_neighbor_rowids_for_sources, load_neighbor_shas_batch,
        load_pending_file_change_enrichment, load_pending_file_change_extraction_commits,
        load_pull_neighbor_rowids, load_recent_planner_sessions, load_repository_id_by_path,
        mark_file_change_enrichment_failed, mark_file_change_extraction_failed,
        merge_file_changes_from_drain_stage, persist_planner_session,
        prepare_file_change_drain_stage, run_migrations, stage_file_change_for_drain,
        upsert_commits_batch, upsert_file_changes, upsert_index_checkpoint,
        verify_sqlite_graph_query_plan_guards, COMMIT_UPSERT_ROWS_PER_CHUNK,
        FILE_CHANGE_ENQUEUE_ROWS_PER_CHUNK, FILE_CHANGE_ENRICHMENT_MAX_ATTEMPTS,
        FILE_CHANGE_ENRICHMENT_RETRY_BASE_DELAY_SECONDS, FILE_CHANGE_EXTRACTION_MAX_ATTEMPTS,
        FILE_CHANGE_UPSERT_ROWS_PER_CHUNK, FRONTIER_PULL_QUERY_CHUNK_SIZE,
        FRONTIER_PULL_STAGE_THRESHOLD, ORDERED_NEIGHBOR_STAGE_THRESHOLD,
    };
    use crate::domain::models::{
        AccelerationReadiness, CheckpointAssessment, CheckpointRepositoryState,
        CheckpointTrustClass, DegradedModeDecision, DegradedModePolicy, FileChangeStatsState,
        GraphDirection, IndexCheckpoint, IndexSessionPlan, IndexingMode, MaintenanceFreshness,
        ParsedCommit, ParsedFileChange, PlannerEntrypoint, PlannerFeatureGate, PlannerHealth,
        PlannerRiskLevel, PlannerTraceEntry, RepositoryCapabilities, RepositoryIdentity,
        RepositoryRefStorage, RepositoryTopology,
    };
    use crate::support::clock::now_epoch_seconds;

    #[test]
    fn migrations_are_idempotent() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        run_migrations(&mut conn).expect("first migration run");
        run_migrations(&mut conn).expect("second migration run");
    }

    #[test]
    fn commits_and_edges_have_repo_scoped_primary_keys() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        run_migrations(&mut conn).expect("migration run");

        let commits_pk = primary_key_columns(&conn, "commits");
        assert_eq!(commits_pk, vec!["commit_id".to_string()]);

        let edges_pk = primary_key_columns(&conn, "commit_edges");
        assert_eq!(
            edges_pk,
            vec![
                "repo_id".to_string(),
                "child_commit_id".to_string(),
                "parent_commit_id".to_string()
            ]
        );
    }

    #[test]
    fn indexing_checkpoint_roundtrip() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        run_migrations(&mut conn).expect("migration run");

        conn.execute(
            "INSERT INTO repositories (path, name, git_dir) VALUES (?1, ?2, ?3)",
            ["/tmp/repo", "repo", "/tmp/repo/.git"],
        )
        .expect("insert repo");

        let repo_id: i64 = conn
            .query_row(
                "SELECT id FROM repositories WHERE path = ?1",
                ["/tmp/repo"],
                |row| row.get(0),
            )
            .expect("fetch repo id");

        let checkpoint = IndexCheckpoint {
            last_committer_time: 123,
            last_commit_sha: "abc123".to_string(),
            indexed_at_epoch: 456,
            repository_state: Some(CheckpointRepositoryState {
                identity_fingerprint: "/tmp/repo/.git::/tmp/repo".to_string(),
                topology: RepositoryTopology::Common,
                is_bare: false,
                is_shallow: false,
                object_hash: "sha1".to_string(),
                ref_storage: RepositoryRefStorage::Files,
                has_sparse_checkout: false,
                has_partial_clone: false,
            }),
        };
        upsert_index_checkpoint(&conn, repo_id, &checkpoint).expect("insert checkpoint");

        let loaded = load_index_checkpoint(&conn, repo_id)
            .expect("load checkpoint")
            .expect("checkpoint should exist");
        assert_eq!(loaded.last_committer_time, 123);
        assert_eq!(loaded.last_commit_sha, "abc123");
        assert_eq!(loaded.indexed_at_epoch, 456);
        assert_eq!(loaded.repository_state, checkpoint.repository_state);
    }

    #[test]
    fn indexing_checkpoint_without_state_snapshot_loads_as_none() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        run_migrations(&mut conn).expect("migration run");

        conn.execute(
            "INSERT INTO repositories (path, name, git_dir) VALUES (?1, ?2, ?3)",
            ["/tmp/repo-legacy", "repo-legacy", "/tmp/repo-legacy/.git"],
        )
        .expect("insert repo");

        let repo_id: i64 = conn
            .query_row(
                "SELECT id FROM repositories WHERE path = ?1",
                ["/tmp/repo-legacy"],
                |row| row.get(0),
            )
            .expect("fetch repo id");

        conn.execute(
            r#"
            INSERT INTO indexing_checkpoints (
              repo_id, last_committer_time, last_commit_sha, indexed_at_epoch
            ) VALUES (?1, ?2, ?3, ?4)
        "#,
            params![repo_id, 123_i64, "legacy-sha", 456_i64],
        )
        .expect("insert legacy checkpoint");

        let loaded = load_index_checkpoint(&conn, repo_id)
            .expect("load checkpoint")
            .expect("checkpoint should exist");
        assert!(loaded.repository_state.is_none());
    }

    #[test]
    fn planner_session_persistence_records_trace_reasons_and_feature_gates() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        run_migrations(&mut conn).expect("migration run");

        conn.execute(
            "INSERT INTO repositories (path, name, git_dir) VALUES (?1, ?2, ?3)",
            [
                "/tmp/planner-repo",
                "planner-repo",
                "/tmp/planner-repo/.git",
            ],
        )
        .expect("insert repo");

        let repo_id: i64 = conn
            .query_row(
                "SELECT id FROM repositories WHERE path = ?1",
                ["/tmp/planner-repo"],
                |row| row.get(0),
            )
            .expect("fetch repo id");

        let session_plan = IndexSessionPlan {
            planner_session_id: None,
            entrypoint: PlannerEntrypoint::Index,
            repository: RepositoryIdentity {
                path: "/tmp/planner-repo".to_string(),
                git_dir: "/tmp/planner-repo/.git".to_string(),
                git_common_dir: "/tmp/planner-repo/.git".to_string(),
                object_dir: "/tmp/planner-repo/.git/objects".to_string(),
                name: "planner-repo".to_string(),
                topology: RepositoryTopology::Common,
                is_bare: false,
                is_shallow: false,
                superproject_root: None,
                identity_fingerprint: "/tmp/planner-repo/.git::/tmp/planner-repo".to_string(),
            },
            capabilities: RepositoryCapabilities {
                object_hash: "sha1".to_string(),
                ref_storage: RepositoryRefStorage::Files,
                has_commit_graph: true,
                has_changed_path_bloom_filters: false,
                has_multi_pack_index: false,
                has_sparse_checkout: false,
                has_partial_clone: false,
                has_alternates: false,
                has_split_index: false,
                has_worktree_config: false,
                acceleration_readiness: AccelerationReadiness::Partial,
            },
            requested_mode: IndexingMode::Incremental,
            effective_mode: IndexingMode::Full,
            checkpoint: CheckpointAssessment {
                present: true,
                trust_class: CheckpointTrustClass::Invalidated,
                invalidation_reasons: vec!["repository_shallow_state_changed".to_string()],
                resume_from_committer_time: None,
                hide_ancestor_sha: None,
            },
            degraded_mode: DegradedModeDecision {
                policy: DegradedModePolicy::FallbackToFull,
                reasons: vec!["repository_shallow_state_changed".to_string()],
                disabled_features: vec![
                    PlannerFeatureGate::IncrementalResume,
                    PlannerFeatureGate::ChangedPathBloomFilters,
                ],
            },
            health: PlannerHealth {
                maintenance_freshness: MaintenanceFreshness::Stale,
                risk_level: PlannerRiskLevel::Critical,
                findings: vec![
                    "repository_shallow_state_changed".to_string(),
                    "commit_graph_behind_packfiles".to_string(),
                ],
                recommended_actions: vec![
                    "treat the next run as a full publish barrier before trusting incremental resume again"
                        .to_string(),
                    "refresh commit-graph metadata with `git maintenance run --task=commit-graph`"
                        .to_string(),
                ],
            },
            planner_trace: vec![PlannerTraceEntry {
                code: "planner_test".to_string(),
                message: "planner trace persisted".to_string(),
            }],
        };

        let session_id =
            persist_planner_session(&conn, repo_id, &session_plan, 123).expect("persist plan");
        assert!(session_id > 0);

        let trace_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM planner_trace_entries WHERE session_id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .expect("count planner trace entries");
        assert_eq!(trace_count, 1);

        let reason_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM planner_session_reasons WHERE session_id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .expect("count planner reasons");
        assert_eq!(reason_count, 6);

        let feature_gate_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM planner_session_feature_gates WHERE session_id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .expect("count planner feature gates");
        assert_eq!(feature_gate_count, 2);
    }

    #[test]
    fn planner_session_roundtrip_loads_latest_and_recent_history() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        run_migrations(&mut conn).expect("migration run");

        conn.execute(
            "INSERT INTO repositories (path, name, git_dir) VALUES (?1, ?2, ?3)",
            [
                "/tmp/history-repo",
                "history-repo",
                "/tmp/history-repo/.git",
            ],
        )
        .expect("insert repo");

        let repo_id: i64 = conn
            .query_row(
                "SELECT id FROM repositories WHERE path = ?1",
                ["/tmp/history-repo"],
                |row| row.get(0),
            )
            .expect("fetch repo id");

        let mut degraded_plan = IndexSessionPlan {
            planner_session_id: None,
            entrypoint: PlannerEntrypoint::Scan,
            repository: RepositoryIdentity {
                path: "/tmp/history-repo".to_string(),
                git_dir: "/tmp/history-repo/.git".to_string(),
                git_common_dir: "/tmp/history-repo/.git".to_string(),
                object_dir: "/tmp/history-repo/.git/objects".to_string(),
                name: "history-repo".to_string(),
                topology: RepositoryTopology::Common,
                is_bare: false,
                is_shallow: false,
                superproject_root: None,
                identity_fingerprint: "/tmp/history-repo/.git::/tmp/history-repo".to_string(),
            },
            capabilities: RepositoryCapabilities {
                object_hash: "sha1".to_string(),
                ref_storage: RepositoryRefStorage::Files,
                has_commit_graph: false,
                has_changed_path_bloom_filters: false,
                has_multi_pack_index: false,
                has_sparse_checkout: false,
                has_partial_clone: false,
                has_alternates: false,
                has_split_index: false,
                has_worktree_config: false,
                acceleration_readiness: AccelerationReadiness::Unavailable,
            },
            requested_mode: IndexingMode::Incremental,
            effective_mode: IndexingMode::Full,
            checkpoint: CheckpointAssessment {
                present: false,
                trust_class: CheckpointTrustClass::Missing,
                invalidation_reasons: vec!["missing_checkpoint".to_string()],
                resume_from_committer_time: None,
                hide_ancestor_sha: None,
            },
            degraded_mode: DegradedModeDecision {
                policy: DegradedModePolicy::FallbackToFull,
                reasons: vec!["missing_checkpoint".to_string()],
                disabled_features: vec![PlannerFeatureGate::IncrementalResume],
            },
            health: PlannerHealth {
                maintenance_freshness: MaintenanceFreshness::Unknown,
                risk_level: PlannerRiskLevel::Critical,
                findings: vec!["missing_checkpoint".to_string()],
                recommended_actions: vec![
                    "treat the next run as a full publish barrier before trusting incremental resume again"
                        .to_string(),
                ],
            },
            planner_trace: vec![PlannerTraceEntry {
                code: "history_1".to_string(),
                message: "degraded scan".to_string(),
            }],
        };
        let nominal_plan = IndexSessionPlan {
            planner_session_id: None,
            entrypoint: PlannerEntrypoint::Index,
            repository: degraded_plan.repository.clone(),
            capabilities: RepositoryCapabilities {
                object_hash: "sha1".to_string(),
                ref_storage: RepositoryRefStorage::Files,
                has_commit_graph: true,
                has_changed_path_bloom_filters: true,
                has_multi_pack_index: true,
                has_sparse_checkout: false,
                has_partial_clone: false,
                has_alternates: false,
                has_split_index: false,
                has_worktree_config: false,
                acceleration_readiness: AccelerationReadiness::Ready,
            },
            requested_mode: IndexingMode::Full,
            effective_mode: IndexingMode::Full,
            checkpoint: CheckpointAssessment {
                present: true,
                trust_class: CheckpointTrustClass::Trusted,
                invalidation_reasons: Vec::new(),
                resume_from_committer_time: None,
                hide_ancestor_sha: None,
            },
            degraded_mode: DegradedModeDecision {
                policy: DegradedModePolicy::Nominal,
                reasons: Vec::new(),
                disabled_features: Vec::new(),
            },
            health: PlannerHealth {
                maintenance_freshness: MaintenanceFreshness::Fresh,
                risk_level: PlannerRiskLevel::Nominal,
                findings: Vec::new(),
                recommended_actions: Vec::new(),
            },
            planner_trace: vec![PlannerTraceEntry {
                code: "history_2".to_string(),
                message: "nominal index".to_string(),
            }],
        };

        let first_session_id =
            persist_planner_session(&conn, repo_id, &degraded_plan, 100).expect("persist first");
        degraded_plan.planner_session_id = Some(first_session_id);
        let second_session_id =
            persist_planner_session(&conn, repo_id, &nominal_plan, 200).expect("persist second");

        let latest = load_latest_planner_session(&conn, repo_id)
            .expect("load latest")
            .expect("latest session");
        assert_eq!(latest.session_id, second_session_id);
        assert_eq!(latest.health.risk_level, PlannerRiskLevel::Nominal);
        assert_eq!(
            latest.capabilities.acceleration_readiness,
            AccelerationReadiness::Ready
        );
        assert_eq!(
            latest.plan_signature,
            "index|full|full|nominal|trusted|ready|nominal"
        );

        let history = load_recent_planner_sessions(&conn, repo_id, 10).expect("load history");
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].session_id, second_session_id);
        assert_eq!(history[1].session_id, first_session_id);
        assert_eq!(
            history[1].degraded_mode.policy,
            DegradedModePolicy::FallbackToFull
        );
        assert_eq!(history[1].health.risk_level, PlannerRiskLevel::Critical);
    }

    #[test]
    fn load_repository_id_by_path_roundtrip() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        run_migrations(&mut conn).expect("migration run");

        let missing = load_repository_id_by_path(&conn, "/tmp/missing-repo")
            .expect("query missing repository id");
        assert!(missing.is_none());

        conn.execute(
            "INSERT INTO repositories (path, name, git_dir) VALUES (?1, ?2, ?3)",
            ["/tmp/read-repo", "read-repo", "/tmp/read-repo/.git"],
        )
        .expect("insert repository");

        let loaded = load_repository_id_by_path(&conn, "/tmp/read-repo")
            .expect("query existing repository id");
        assert!(loaded.is_some());
    }

    #[test]
    fn delete_file_changes_for_commits_removes_requested_shas_only() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        run_migrations(&mut conn).expect("migration run");

        conn.execute(
            "INSERT INTO repositories (path, name, git_dir) VALUES (?1, ?2, ?3)",
            ["/tmp/delete-repo", "delete-repo", "/tmp/delete-repo/.git"],
        )
        .expect("insert repo");
        let repo_id: i64 = conn
            .query_row(
                "SELECT id FROM repositories WHERE path = ?1",
                ["/tmp/delete-repo"],
                |row| row.get(0),
            )
            .expect("fetch repo id");

        let file_changes = vec![
            ParsedFileChange {
                commit_sha: "c1".to_string(),
                file_path: "a.txt".to_string(),
                additions: Some(1),
                deletions: Some(0),
                change_type: "add".to_string(),
                stats_state: FileChangeStatsState::Ready,
                requires_line_stats: false,
            },
            ParsedFileChange {
                commit_sha: "c2".to_string(),
                file_path: "b.txt".to_string(),
                additions: Some(2),
                deletions: Some(1),
                change_type: "modify".to_string(),
                stats_state: FileChangeStatsState::Ready,
                requires_line_stats: false,
            },
            ParsedFileChange {
                commit_sha: "c3".to_string(),
                file_path: "c.txt".to_string(),
                additions: Some(0),
                deletions: Some(4),
                change_type: "delete".to_string(),
                stats_state: FileChangeStatsState::Ready,
                requires_line_stats: false,
            },
        ];
        upsert_file_changes(&conn, repo_id, &file_changes).expect("insert file changes");

        delete_file_changes_for_commits(&conn, repo_id, &["c1".to_string(), "c3".to_string()])
            .expect("delete selected file changes");

        let remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM file_changes WHERE repo_id = ?1",
                [repo_id],
                |row| row.get(0),
            )
            .expect("count remaining file changes");
        assert_eq!(remaining, 1);

        let remaining_sha: String = conn
            .query_row(
                "SELECT commit_sha FROM file_changes WHERE repo_id = ?1 LIMIT 1",
                [repo_id],
                |row| row.get(0),
            )
            .expect("read remaining commit sha");
        assert_eq!(remaining_sha, "c2");
    }

    #[test]
    fn upsert_file_changes_handles_parameter_chunk_boundaries() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        run_migrations(&mut conn).expect("migration run");

        conn.execute(
            "INSERT INTO repositories (path, name, git_dir) VALUES (?1, ?2, ?3)",
            [
                "/tmp/chunked-upsert-repo",
                "chunked-upsert-repo",
                "/tmp/chunked/.git",
            ],
        )
        .expect("insert repo");
        let repo_id: i64 = conn
            .query_row(
                "SELECT id FROM repositories WHERE path = ?1",
                ["/tmp/chunked-upsert-repo"],
                |row| row.get(0),
            )
            .expect("fetch repo id");

        let change_count = FILE_CHANGE_UPSERT_ROWS_PER_CHUNK + 37;
        let file_changes: Vec<ParsedFileChange> = (0..change_count)
            .map(|index| ParsedFileChange {
                commit_sha: format!("sha-{index:04}"),
                file_path: format!("src/file-{index:04}.rs"),
                additions: Some(index as i64),
                deletions: if index % 2 == 0 {
                    Some((index / 2) as i64)
                } else {
                    None
                },
                change_type: "modify".to_string(),
                stats_state: FileChangeStatsState::Ready,
                requires_line_stats: false,
            })
            .collect();

        let inserted = upsert_file_changes(&conn, repo_id, &file_changes)
            .expect("bulk upsert file changes with chunk boundaries");
        assert_eq!(inserted, change_count);

        let persisted: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM file_changes WHERE repo_id = ?1",
                [repo_id],
                |row| row.get(0),
            )
            .expect("count persisted file changes");
        assert_eq!(persisted, change_count as i64);
    }

    #[test]
    fn upsert_commits_batch_handles_parameter_chunk_boundaries() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        run_migrations(&mut conn).expect("migration run");

        conn.execute(
            "INSERT INTO repositories (path, name, git_dir) VALUES (?1, ?2, ?3)",
            [
                "/tmp/chunked-commit-repo",
                "chunked-commit-repo",
                "/tmp/chunked/.git",
            ],
        )
        .expect("insert repo");
        let repo_id: i64 = conn
            .query_row(
                "SELECT id FROM repositories WHERE path = ?1",
                ["/tmp/chunked-commit-repo"],
                |row| row.get(0),
            )
            .expect("fetch repo id");

        let commit_count = COMMIT_UPSERT_ROWS_PER_CHUNK + 53;
        let commits: Vec<ParsedCommit> = (0..commit_count)
            .map(|index| ParsedCommit {
                sha: format!("commit-{index:05}"),
                parents: Vec::new(),
                tree_sha: format!("{index:040x}"),
                author_name: "GitGraph".to_string(),
                author_email: "gitgraph@example.com".to_string(),
                author_time: index as i64,
                committer_time: index as i64,
                subject: format!("subject-{index}"),
            })
            .collect();

        let inserted = upsert_commits_batch(&conn, repo_id, &commits)
            .expect("bulk commit upsert with chunk boundaries");
        assert_eq!(inserted, commit_count);

        let persisted: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM commits WHERE repo_id = ?1",
                [repo_id],
                |row| row.get(0),
            )
            .expect("count persisted commits");
        assert_eq!(persisted, commit_count as i64);
    }

    #[test]
    fn insert_file_changes_batch_handles_parameter_chunk_boundaries() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        run_migrations(&mut conn).expect("migration run");

        conn.execute(
            "INSERT INTO repositories (path, name, git_dir) VALUES (?1, ?2, ?3)",
            [
                "/tmp/chunked-insert-repo",
                "chunked-insert-repo",
                "/tmp/chunked/.git",
            ],
        )
        .expect("insert repo");
        let repo_id: i64 = conn
            .query_row(
                "SELECT id FROM repositories WHERE path = ?1",
                ["/tmp/chunked-insert-repo"],
                |row| row.get(0),
            )
            .expect("fetch repo id");

        let change_count = FILE_CHANGE_UPSERT_ROWS_PER_CHUNK + 17;
        let file_changes: Vec<ParsedFileChange> = (0..change_count)
            .map(|index| ParsedFileChange {
                commit_sha: format!("insert-sha-{index:04}"),
                file_path: format!("src/insert-{index:04}.rs"),
                additions: Some(index as i64),
                deletions: Some((index % 11) as i64),
                change_type: "modify".to_string(),
                stats_state: FileChangeStatsState::Ready,
                requires_line_stats: false,
            })
            .collect();

        let inserted =
            insert_file_changes_batch(&conn, repo_id, &file_changes).expect("bulk insert changes");
        assert_eq!(inserted, change_count);

        let persisted: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM file_changes WHERE repo_id = ?1",
                [repo_id],
                |row| row.get(0),
            )
            .expect("count persisted file changes");
        assert_eq!(persisted, change_count as i64);
    }

    #[test]
    fn insert_commits_batch_handles_parameter_chunk_boundaries() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        run_migrations(&mut conn).expect("migration run");

        conn.execute(
            "INSERT INTO repositories (path, name, git_dir) VALUES (?1, ?2, ?3)",
            [
                "/tmp/chunked-commit-insert-repo",
                "chunked-commit-insert-repo",
                "/tmp/chunked/.git",
            ],
        )
        .expect("insert repo");
        let repo_id: i64 = conn
            .query_row(
                "SELECT id FROM repositories WHERE path = ?1",
                ["/tmp/chunked-commit-insert-repo"],
                |row| row.get(0),
            )
            .expect("fetch repo id");

        let commit_count = COMMIT_UPSERT_ROWS_PER_CHUNK + 29;
        let commits: Vec<ParsedCommit> = (0..commit_count)
            .map(|index| ParsedCommit {
                sha: format!("insert-commit-{index:05}"),
                parents: Vec::new(),
                tree_sha: format!("{index:040x}"),
                author_name: "GitGraph".to_string(),
                author_email: "gitgraph@example.com".to_string(),
                author_time: index as i64,
                committer_time: index as i64,
                subject: format!("insert-subject-{index}"),
            })
            .collect();

        let inserted = insert_commits_batch(&conn, repo_id, &commits).expect("bulk insert commits");
        assert_eq!(inserted, commit_count);

        let persisted: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM commits WHERE repo_id = ?1",
                [repo_id],
                |row| row.get(0),
            )
            .expect("count persisted commits");
        assert_eq!(persisted, commit_count as i64);
    }

    #[test]
    fn enqueue_file_changes_handles_parameter_chunk_boundaries() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        run_migrations(&mut conn).expect("migration run");

        conn.execute(
            "INSERT INTO repositories (path, name, git_dir) VALUES (?1, ?2, ?3)",
            [
                "/tmp/chunked-enqueue-repo",
                "chunked-enqueue-repo",
                "/tmp/chunked/.git",
            ],
        )
        .expect("insert repo");
        let repo_id: i64 = conn
            .query_row(
                "SELECT id FROM repositories WHERE path = ?1",
                ["/tmp/chunked-enqueue-repo"],
                |row| row.get(0),
            )
            .expect("fetch repo id");

        let eligible_count = FILE_CHANGE_ENQUEUE_ROWS_PER_CHUNK + 41;
        let mut file_changes: Vec<ParsedFileChange> = (0..eligible_count)
            .map(|index| ParsedFileChange {
                commit_sha: format!("pending-sha-{index:04}"),
                file_path: format!("src/pending-{index:04}.rs"),
                additions: None,
                deletions: None,
                change_type: "modify".to_string(),
                stats_state: FileChangeStatsState::Pending,
                requires_line_stats: true,
            })
            .collect();
        file_changes.push(ParsedFileChange {
            commit_sha: "not-eligible-sha".to_string(),
            file_path: "src/not-eligible.rs".to_string(),
            additions: None,
            deletions: None,
            change_type: "modify".to_string(),
            stats_state: FileChangeStatsState::Ready,
            requires_line_stats: false,
        });

        upsert_file_changes(&conn, repo_id, &file_changes).expect("insert pending file changes");
        let enqueued = enqueue_file_changes_for_enrichment(&conn, repo_id, &file_changes)
            .expect("enqueue pending file changes");
        assert_eq!(enqueued, eligible_count);

        let queue_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM file_change_enrichment_queue WHERE repo_id = ?1",
                [repo_id],
                |row| row.get(0),
            )
            .expect("count queue rows");
        assert_eq!(queue_count, eligible_count as i64);
    }

    #[test]
    fn drain_stage_merge_and_enrichment_enqueue_are_set_based_and_filtered() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        run_migrations(&mut conn).expect("migration run");

        conn.execute(
            "INSERT INTO repositories (path, name, git_dir) VALUES (?1, ?2, ?3)",
            [
                "/tmp/drain-stage-repo",
                "drain-stage-repo",
                "/tmp/drain/.git",
            ],
        )
        .expect("insert repo");
        let repo_id: i64 = conn
            .query_row(
                "SELECT id FROM repositories WHERE path = ?1",
                ["/tmp/drain-stage-repo"],
                |row| row.get(0),
            )
            .expect("fetch repo id");

        prepare_file_change_drain_stage(&conn).expect("prepare drain stage");
        stage_file_change_for_drain(
            &conn,
            &ParsedFileChange {
                commit_sha: "stage-c1".to_string(),
                file_path: "src/a.rs".to_string(),
                additions: Some(3),
                deletions: Some(1),
                change_type: "modify".to_string(),
                stats_state: FileChangeStatsState::Pending,
                requires_line_stats: true,
            },
        )
        .expect("stage pending row");
        stage_file_change_for_drain(
            &conn,
            &ParsedFileChange {
                commit_sha: "stage-c2".to_string(),
                file_path: "src/b.rs".to_string(),
                additions: Some(5),
                deletions: Some(0),
                change_type: "binary".to_string(),
                stats_state: FileChangeStatsState::Ready,
                requires_line_stats: false,
            },
        )
        .expect("stage ready row");

        let merged =
            merge_file_changes_from_drain_stage(&conn, repo_id).expect("merge staged rows");
        assert_eq!(merged, 2);
        let enqueued = enqueue_file_changes_for_enrichment_from_drain_stage(&conn, repo_id)
            .expect("enqueue staged pending rows");
        assert_eq!(enqueued, 1);

        let persisted: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM file_changes WHERE repo_id = ?1",
                [repo_id],
                |row| row.get(0),
            )
            .expect("count persisted file_changes");
        assert_eq!(persisted, 2);
        let queue_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM file_change_enrichment_queue WHERE repo_id = ?1",
                [repo_id],
                |row| row.get(0),
            )
            .expect("count persisted enrichment queue");
        assert_eq!(queue_count, 1);
    }

    #[test]
    fn extraction_queue_roundtrip_and_metrics() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        run_migrations(&mut conn).expect("migration run");

        conn.execute(
            "INSERT INTO repositories (path, name, git_dir) VALUES (?1, ?2, ?3)",
            [
                "/tmp/extraction-queue",
                "extraction-queue",
                "/tmp/extraction/.git",
            ],
        )
        .expect("insert repo");
        let repo_id: i64 = conn
            .query_row(
                "SELECT id FROM repositories WHERE path = ?1",
                ["/tmp/extraction-queue"],
                |row| row.get(0),
            )
            .expect("fetch repo id");

        let enqueued = enqueue_commits_for_file_change_extraction(
            &conn,
            repo_id,
            &[
                "c3".to_string(),
                "c1".to_string(),
                "c3".to_string(),
                "c2".to_string(),
            ],
        )
        .expect("enqueue extraction commits");
        assert_eq!(enqueued, 3);

        let pending = load_pending_file_change_extraction_commits(&conn, repo_id, 10)
            .expect("load pending extraction commits");
        assert_eq!(
            pending,
            vec!["c1".to_string(), "c2".to_string(), "c3".to_string()]
        );

        let metrics = load_file_change_extraction_queue_metrics(&conn, repo_id)
            .expect("load extraction queue metrics");
        assert_eq!(metrics.depth, 3);
        assert!(metrics.oldest_queue_age_seconds >= 0);

        clear_file_change_extraction_queue_for_commits(&conn, repo_id, &["c2".to_string()])
            .expect("clear completed extraction commit");
        let metrics_after_clear = load_file_change_extraction_queue_metrics(&conn, repo_id)
            .expect("load extraction queue metrics after clear");
        assert_eq!(metrics_after_clear.depth, 2);
    }

    #[test]
    fn extraction_queue_retries_until_terminal_attempt_gate() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        run_migrations(&mut conn).expect("migration run");

        conn.execute(
            "INSERT INTO repositories (path, name, git_dir) VALUES (?1, ?2, ?3)",
            [
                "/tmp/extraction-retry",
                "extraction-retry",
                "/tmp/extraction/.git",
            ],
        )
        .expect("insert repo");
        let repo_id: i64 = conn
            .query_row(
                "SELECT id FROM repositories WHERE path = ?1",
                ["/tmp/extraction-retry"],
                |row| row.get(0),
            )
            .expect("fetch repo id");

        enqueue_commits_for_file_change_extraction(&conn, repo_id, &["retry-sha".to_string()])
            .expect("enqueue extraction commit");

        for attempt in 1..=FILE_CHANGE_EXTRACTION_MAX_ATTEMPTS {
            mark_file_change_extraction_failed(
                &conn,
                repo_id,
                &["retry-sha".to_string()],
                "extraction failure",
            )
            .expect("record extraction failure");

            if attempt < FILE_CHANGE_EXTRACTION_MAX_ATTEMPTS {
                conn.execute(
                    "UPDATE file_change_extraction_queue SET next_retry_epoch = ?1 WHERE repo_id = ?2 AND commit_sha = ?3",
                    params![now_epoch_seconds().saturating_sub(1), repo_id, "retry-sha"],
                )
                .expect("force extraction retry eligibility");
            }
        }

        let pending_after_terminal = load_pending_file_change_extraction_commits(&conn, repo_id, 8)
            .expect("load pending extraction commits after terminal attempt");
        assert!(pending_after_terminal.is_empty());

        let attempts: i64 = conn
            .query_row(
                "SELECT attempts FROM file_change_extraction_queue WHERE repo_id = ?1 AND commit_sha = ?2",
                params![repo_id, "retry-sha"],
                |row| row.get(0),
            )
            .expect("read terminal extraction attempts");
        assert_eq!(attempts, FILE_CHANGE_EXTRACTION_MAX_ATTEMPTS);
    }

    #[test]
    fn neighbor_batch_lookup_returns_repo_scoped_neighbors() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        run_migrations(&mut conn).expect("migration run");

        conn.execute(
            "INSERT INTO repositories (path, name, git_dir) VALUES (?1, ?2, ?3)",
            ["/tmp/repo", "repo", "/tmp/repo/.git"],
        )
        .expect("insert repo");
        let repo_id: i64 = conn
            .query_row(
                "SELECT id FROM repositories WHERE path = ?1",
                ["/tmp/repo"],
                |row| row.get(0),
            )
            .expect("fetch repo id");

        conn.execute(
            "INSERT INTO commits (repo_id, sha) VALUES (?1, ?2)",
            params![repo_id, "c1"],
        )
        .expect("insert c1");
        conn.execute(
            "INSERT INTO commits (repo_id, sha) VALUES (?1, ?2)",
            params![repo_id, "c2"],
        )
        .expect("insert c2");
        conn.execute(
            "INSERT INTO commits (repo_id, sha) VALUES (?1, ?2)",
            params![repo_id, "c3"],
        )
        .expect("insert c3");

        let c1_rowid: i64 = conn
            .query_row(
                "SELECT commit_id FROM commits WHERE repo_id = ?1 AND sha = ?2",
                params![repo_id, "c1"],
                |row| row.get(0),
            )
            .expect("fetch c1 rowid");
        let c2_rowid: i64 = conn
            .query_row(
                "SELECT commit_id FROM commits WHERE repo_id = ?1 AND sha = ?2",
                params![repo_id, "c2"],
                |row| row.get(0),
            )
            .expect("fetch c2 rowid");
        let c3_rowid: i64 = conn
            .query_row(
                "SELECT commit_id FROM commits WHERE repo_id = ?1 AND sha = ?2",
                params![repo_id, "c3"],
                |row| row.get(0),
            )
            .expect("fetch c3 rowid");

        conn.execute(
            "INSERT INTO commit_edges (repo_id, child_commit_id, parent_commit_id) VALUES (?1, ?2, ?3)",
            params![repo_id, c2_rowid, c1_rowid],
        )
        .expect("insert edge c2->c1");
        conn.execute(
            "INSERT INTO commit_edges (repo_id, child_commit_id, parent_commit_id) VALUES (?1, ?2, ?3)",
            params![repo_id, c3_rowid, c2_rowid],
        )
        .expect("insert edge c3->c2");

        let batch = vec!["c1".to_string(), "c2".to_string(), "c3".to_string()];
        let parents = load_neighbor_shas_batch(&conn, repo_id, &batch, GraphDirection::Parents)
            .expect("load parent neighbors");
        assert_eq!(parents.get("c3"), Some(&vec!["c2".to_string()]));
        assert_eq!(parents.get("c2"), Some(&vec!["c1".to_string()]));
        assert_eq!(parents.get("c1"), Some(&Vec::new()));

        let children = load_neighbor_shas_batch(&conn, repo_id, &batch, GraphDirection::Children)
            .expect("load child neighbors");
        assert_eq!(children.get("c1"), Some(&vec!["c2".to_string()]));
        assert_eq!(children.get("c2"), Some(&vec!["c3".to_string()]));
        assert_eq!(children.get("c3"), Some(&Vec::new()));

        let mut both_for_c2 =
            load_neighbor_shas_batch(&conn, repo_id, &batch, GraphDirection::Both)
                .expect("load both neighbors")
                .remove("c2")
                .unwrap_or_default();
        both_for_c2.sort();
        assert_eq!(both_for_c2, vec!["c1".to_string(), "c3".to_string()]);

        let rowid_batch = vec![c1_rowid, c2_rowid, c3_rowid];
        let rowid_parents =
            load_neighbor_rowids_batch(&conn, repo_id, &rowid_batch, GraphDirection::Parents)
                .expect("load rowid parent neighbors");
        assert_eq!(rowid_parents.get(&c3_rowid), Some(&vec![c2_rowid]));
        assert_eq!(rowid_parents.get(&c2_rowid), Some(&vec![c1_rowid]));

        let ordered_children = load_neighbor_rowids_for_sources(
            &conn,
            repo_id,
            &[c2_rowid, c1_rowid, c3_rowid],
            GraphDirection::Children,
        )
        .expect("load ordered rowid children");
        assert_eq!(
            ordered_children,
            vec![vec![c3_rowid], vec![c2_rowid], Vec::new()]
        );

        let pull_children =
            load_pull_neighbor_rowids(&conn, repo_id, &[c1_rowid], GraphDirection::Children)
                .expect("pull children");
        assert_eq!(pull_children, vec![c2_rowid]);

        let pull_parents =
            load_pull_neighbor_rowids(&conn, repo_id, &[c3_rowid], GraphDirection::Parents)
                .expect("pull parents");
        assert_eq!(pull_parents, vec![c2_rowid]);

        let pull_both =
            load_pull_neighbor_rowids(&conn, repo_id, &[c2_rowid], GraphDirection::Both)
                .expect("pull both");
        assert_eq!(pull_both, vec![c3_rowid, c1_rowid]);

        let repo_commit_count = count_commits(&conn, repo_id).expect("count commits");
        assert_eq!(repo_commit_count, 3);

        let rowid_shas =
            load_commit_shas_by_rowids(&conn, repo_id, &rowid_batch).expect("load sha by rowids");
        assert_eq!(rowid_shas.get(&c1_rowid).map(String::as_str), Some("c1"));
        assert_eq!(rowid_shas.get(&c2_rowid).map(String::as_str), Some("c2"));
        assert_eq!(rowid_shas.get(&c3_rowid).map(String::as_str), Some("c3"));

        let ordered_shas =
            load_commit_shas_for_rowids(&conn, repo_id, &[c3_rowid, c1_rowid, c2_rowid])
                .expect("load ordered sha by rowids");
        assert_eq!(
            ordered_shas,
            vec!["c3".to_string(), "c1".to_string(), "c2".to_string()]
        );
    }

    #[test]
    fn neighbor_queries_handle_chunk_boundaries() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        run_migrations(&mut conn).expect("migration run");

        conn.execute(
            "INSERT INTO repositories (path, name, git_dir) VALUES (?1, ?2, ?3)",
            ["/tmp/chunk-repo", "chunk-repo", "/tmp/chunk-repo/.git"],
        )
        .expect("insert repo");
        let repo_id: i64 = conn
            .query_row(
                "SELECT id FROM repositories WHERE path = ?1",
                ["/tmp/chunk-repo"],
                |row| row.get(0),
            )
            .expect("fetch repo id");

        let commit_count = FRONTIER_PULL_QUERY_CHUNK_SIZE + 100;
        {
            let tx = conn.transaction().expect("start insert tx");
            let mut commit_stmt = tx
                .prepare_cached("INSERT INTO commits (repo_id, sha) VALUES (?1, ?2)")
                .expect("prepare commit insert");
            for index in 0..commit_count {
                let sha = format!("c{index:04}");
                commit_stmt
                    .execute(params![repo_id, sha])
                    .expect("insert commit");
            }
            drop(commit_stmt);
            tx.commit().expect("commit inserts");
        }

        let commit_ids = {
            let mut id_stmt = conn
                .prepare_cached(
                    r#"
                SELECT commit_id
                FROM commits
                WHERE repo_id = ?1
                ORDER BY sha ASC
            "#,
                )
                .expect("prepare commit id query");
            let id_rows = id_stmt
                .query_map([repo_id], |row| row.get::<_, i64>(0))
                .expect("query commit ids");
            let mut commit_ids = Vec::with_capacity(commit_count);
            for row in id_rows {
                commit_ids.push(row.expect("read commit id"));
            }
            commit_ids
        };

        {
            let tx = conn.transaction().expect("start edge insert tx");
            let mut edge_stmt = tx
                .prepare_cached(
                    r#"
                    INSERT INTO commit_edges (repo_id, child_commit_id, parent_commit_id)
                    VALUES (?1, ?2, ?3)
                "#,
                )
                .expect("prepare edge insert");
            for index in 1..commit_ids.len() {
                edge_stmt
                    .execute(params![repo_id, commit_ids[index], commit_ids[index - 1]])
                    .expect("insert edge");
            }
            drop(edge_stmt);
            tx.commit().expect("commit edges");
        }

        let source_rowids: Vec<i64> = commit_ids.iter().skip(1).copied().collect();
        assert!(
            source_rowids.len() > ORDERED_NEIGHBOR_STAGE_THRESHOLD,
            "test must cross ordered-neighbor staging threshold"
        );
        let ordered_parents = load_neighbor_rowids_for_sources(
            &conn,
            repo_id,
            &source_rowids,
            GraphDirection::Parents,
        )
        .expect("load ordered parents");
        assert_eq!(ordered_parents.len(), source_rowids.len());
        for (index, parents) in ordered_parents.iter().enumerate() {
            assert_eq!(parents, &vec![commit_ids[index]]);
        }

        let frontier: Vec<i64> = commit_ids
            .iter()
            .take(FRONTIER_PULL_QUERY_CHUNK_SIZE + 50)
            .copied()
            .collect();
        assert!(
            frontier.len() > FRONTIER_PULL_STAGE_THRESHOLD,
            "test must cross pull-frontier staging threshold"
        );

        let pull_children =
            load_pull_neighbor_rowids(&conn, repo_id, &frontier, GraphDirection::Children)
                .expect("load pull children");
        let mut expected_children: Vec<i64> = commit_ids
            .iter()
            .skip(1)
            .take(FRONTIER_PULL_QUERY_CHUNK_SIZE + 50)
            .copied()
            .collect();
        expected_children.sort_unstable_by(|left, right| right.cmp(left));
        assert_eq!(pull_children, expected_children);

        let pull_both = load_pull_neighbor_rowids(&conn, repo_id, &frontier, GraphDirection::Both)
            .expect("load pull both");
        let mut expected_both: Vec<i64> = commit_ids
            .iter()
            .take(FRONTIER_PULL_QUERY_CHUNK_SIZE + 51)
            .copied()
            .collect();
        expected_both.sort_unstable_by(|left, right| right.cmp(left));
        assert_eq!(pull_both, expected_both);
    }

    #[test]
    fn sqlite_graph_query_plan_guard_accepts_indexed_staged_queries() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        run_migrations(&mut conn).expect("migration run");

        conn.execute(
            "INSERT INTO repositories (path, name, git_dir) VALUES (?1, ?2, ?3)",
            [
                "/tmp/plan-guard-repo",
                "plan-guard-repo",
                "/tmp/plan-guard-repo/.git",
            ],
        )
        .expect("insert repo");
        let repo_id: i64 = conn
            .query_row(
                "SELECT id FROM repositories WHERE path = ?1",
                ["/tmp/plan-guard-repo"],
                |row| row.get(0),
            )
            .expect("fetch repo id");

        conn.execute(
            "INSERT INTO commits (repo_id, sha) VALUES (?1, ?2)",
            params![repo_id, "plan-guard-c1"],
        )
        .expect("insert c1");
        conn.execute(
            "INSERT INTO commits (repo_id, sha) VALUES (?1, ?2)",
            params![repo_id, "plan-guard-c2"],
        )
        .expect("insert c2");
        let c1_rowid: i64 = conn
            .query_row(
                "SELECT commit_id FROM commits WHERE repo_id = ?1 AND sha = ?2",
                params![repo_id, "plan-guard-c1"],
                |row| row.get(0),
            )
            .expect("fetch c1 rowid");
        let c2_rowid: i64 = conn
            .query_row(
                "SELECT commit_id FROM commits WHERE repo_id = ?1 AND sha = ?2",
                params![repo_id, "plan-guard-c2"],
                |row| row.get(0),
            )
            .expect("fetch c2 rowid");
        conn.execute(
            "INSERT INTO commit_edges (repo_id, child_commit_id, parent_commit_id) VALUES (?1, ?2, ?3)",
            params![repo_id, c2_rowid, c1_rowid],
        )
        .expect("insert c2->c1 edge");

        verify_sqlite_graph_query_plan_guards(&conn, repo_id)
            .expect("sqlite graph query plan guard");
    }

    #[test]
    fn enrichment_failures_retry_until_terminal_failed() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        run_migrations(&mut conn).expect("migration run");

        conn.execute(
            "INSERT INTO repositories (path, name, git_dir) VALUES (?1, ?2, ?3)",
            ["/tmp/retry-repo", "retry-repo", "/tmp/retry-repo/.git"],
        )
        .expect("insert repo");
        let repo_id: i64 = conn
            .query_row(
                "SELECT id FROM repositories WHERE path = ?1",
                ["/tmp/retry-repo"],
                |row| row.get(0),
            )
            .expect("fetch repo id");

        let file_changes = vec![ParsedFileChange {
            commit_sha: "retry-sha".to_string(),
            file_path: "retry.txt".to_string(),
            additions: None,
            deletions: None,
            change_type: "modify".to_string(),
            stats_state: FileChangeStatsState::Pending,
            requires_line_stats: true,
        }];
        upsert_file_changes(&conn, repo_id, &file_changes).expect("insert pending file change");
        enqueue_file_changes_for_enrichment(&conn, repo_id, &file_changes)
            .expect("enqueue pending file change");

        for attempt in 1..=FILE_CHANGE_ENRICHMENT_MAX_ATTEMPTS {
            let pending = load_pending_file_change_enrichment(&conn, repo_id, 8)
                .expect("load pending for retry attempt");
            assert_eq!(pending.len(), 1, "expected one pending row for attempt");
            mark_file_change_enrichment_failed(&conn, repo_id, &pending, "retryable failure")
                .expect("record enrichment failure");

            let stats_state: String = conn
                .query_row(
                    "SELECT stats_state FROM file_changes WHERE repo_id = ?1 AND commit_sha = ?2 AND file_path = ?3",
                    params![repo_id, "retry-sha", "retry.txt"],
                    |row| row.get(0),
                )
                .expect("load stats state");
            if attempt < FILE_CHANGE_ENRICHMENT_MAX_ATTEMPTS {
                assert_eq!(stats_state, "pending");
                conn.execute(
                    "UPDATE file_change_enrichment_queue SET enqueued_at_epoch = ?1 WHERE repo_id = ?2 AND commit_sha = ?3 AND file_path = ?4",
                    params![
                        now_epoch_seconds().saturating_sub(1),
                        repo_id,
                        "retry-sha",
                        "retry.txt"
                    ],
                )
                .expect("force retry queue due timestamp");
            } else {
                assert_eq!(stats_state, "failed");
            }
        }

        let pending_final = load_pending_file_change_enrichment(&conn, repo_id, 8)
            .expect("load pending after terminal failure");
        assert!(pending_final.is_empty());
    }

    #[test]
    fn enrichment_retry_cooldown_gates_pending_pickup() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        run_migrations(&mut conn).expect("migration run");

        conn.execute(
            "INSERT INTO repositories (path, name, git_dir) VALUES (?1, ?2, ?3)",
            [
                "/tmp/retry-cooldown-repo",
                "retry-cooldown-repo",
                "/tmp/retry/.git",
            ],
        )
        .expect("insert repo");
        let repo_id: i64 = conn
            .query_row(
                "SELECT id FROM repositories WHERE path = ?1",
                ["/tmp/retry-cooldown-repo"],
                |row| row.get(0),
            )
            .expect("fetch repo id");

        let file_changes = vec![ParsedFileChange {
            commit_sha: "retry-cooldown-sha".to_string(),
            file_path: "retry-cooldown.txt".to_string(),
            additions: None,
            deletions: None,
            change_type: "modify".to_string(),
            stats_state: FileChangeStatsState::Pending,
            requires_line_stats: true,
        }];
        upsert_file_changes(&conn, repo_id, &file_changes).expect("insert pending file change");
        enqueue_file_changes_for_enrichment(&conn, repo_id, &file_changes)
            .expect("enqueue pending file change");

        let pending_before = load_pending_file_change_enrichment(&conn, repo_id, 8)
            .expect("load pending before failure");
        assert_eq!(pending_before.len(), 1);

        let before_failure_epoch = now_epoch_seconds();
        mark_file_change_enrichment_failed(
            &conn,
            repo_id,
            &pending_before,
            "transient enrichment failure",
        )
        .expect("record transient enrichment failure");

        let pending_immediate = load_pending_file_change_enrichment(&conn, repo_id, 8)
            .expect("load pending immediately after failure");
        assert!(
            pending_immediate.is_empty(),
            "cooldown gate should defer immediate retry pickup"
        );

        let (attempts, last_error, enqueued_at_epoch): (i64, Option<String>, i64) = conn
            .query_row(
                "SELECT attempts, last_error, enqueued_at_epoch FROM file_change_enrichment_queue WHERE repo_id = ?1 AND commit_sha = ?2 AND file_path = ?3",
                params![repo_id, "retry-cooldown-sha", "retry-cooldown.txt"],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("load queue row after first failure");
        assert_eq!(attempts, 1);
        assert!(last_error.is_some());
        assert!(
            enqueued_at_epoch
                >= before_failure_epoch
                    .saturating_add(FILE_CHANGE_ENRICHMENT_RETRY_BASE_DELAY_SECONDS),
            "enqueued_at_epoch should include retry cooldown delay"
        );

        conn.execute(
            "UPDATE file_change_enrichment_queue SET enqueued_at_epoch = ?1 WHERE repo_id = ?2 AND commit_sha = ?3 AND file_path = ?4",
            params![
                now_epoch_seconds().saturating_sub(1),
                repo_id,
                "retry-cooldown-sha",
                "retry-cooldown.txt"
            ],
        )
        .expect("force queue row due");

        let pending_after_cooldown = load_pending_file_change_enrichment(&conn, repo_id, 8)
            .expect("load pending after cooldown");
        assert_eq!(pending_after_cooldown.len(), 1);
    }

    #[test]
    fn enqueue_resets_attempts_for_reindexed_rows() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        run_migrations(&mut conn).expect("migration run");

        conn.execute(
            "INSERT INTO repositories (path, name, git_dir) VALUES (?1, ?2, ?3)",
            [
                "/tmp/retry-reset-repo",
                "retry-reset-repo",
                "/tmp/retry-reset-repo/.git",
            ],
        )
        .expect("insert repo");
        let repo_id: i64 = conn
            .query_row(
                "SELECT id FROM repositories WHERE path = ?1",
                ["/tmp/retry-reset-repo"],
                |row| row.get(0),
            )
            .expect("fetch repo id");

        let file_changes = vec![ParsedFileChange {
            commit_sha: "retry-reset-sha".to_string(),
            file_path: "retry-reset.txt".to_string(),
            additions: None,
            deletions: None,
            change_type: "modify".to_string(),
            stats_state: FileChangeStatsState::Pending,
            requires_line_stats: true,
        }];
        upsert_file_changes(&conn, repo_id, &file_changes).expect("insert pending file change");
        enqueue_file_changes_for_enrichment(&conn, repo_id, &file_changes)
            .expect("enqueue pending file change");

        for attempt in 1..=FILE_CHANGE_ENRICHMENT_MAX_ATTEMPTS {
            let pending = load_pending_file_change_enrichment(&conn, repo_id, 8)
                .expect("load pending for retry attempt");
            assert_eq!(pending.len(), 1);
            mark_file_change_enrichment_failed(&conn, repo_id, &pending, "persistent failure")
                .expect("record failure");
            if attempt < FILE_CHANGE_ENRICHMENT_MAX_ATTEMPTS {
                conn.execute(
                    "UPDATE file_change_enrichment_queue SET enqueued_at_epoch = ?1 WHERE repo_id = ?2",
                    params![now_epoch_seconds().saturating_sub(1), repo_id],
                )
                .expect("force next retry");
            }
        }

        let terminal_state: String = conn
            .query_row(
                "SELECT stats_state FROM file_changes WHERE repo_id = ?1 AND commit_sha = ?2 AND file_path = ?3",
                params![repo_id, "retry-reset-sha", "retry-reset.txt"],
                |row| row.get(0),
            )
            .expect("load terminal stats state");
        assert_eq!(terminal_state, "failed");

        let reindexed = vec![ParsedFileChange {
            commit_sha: "retry-reset-sha".to_string(),
            file_path: "retry-reset.txt".to_string(),
            additions: None,
            deletions: None,
            change_type: "modify".to_string(),
            stats_state: FileChangeStatsState::Pending,
            requires_line_stats: true,
        }];
        upsert_file_changes(&conn, repo_id, &reindexed).expect("upsert reindexed pending row");
        enqueue_file_changes_for_enrichment(&conn, repo_id, &reindexed)
            .expect("re-enqueue after reindex");

        let (attempts, last_error): (i64, Option<String>) = conn
            .query_row(
                "SELECT attempts, last_error FROM file_change_enrichment_queue WHERE repo_id = ?1 AND commit_sha = ?2 AND file_path = ?3",
                params![repo_id, "retry-reset-sha", "retry-reset.txt"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("load queue row after re-enqueue");
        assert_eq!(attempts, 0);
        assert!(last_error.is_none());
    }

    fn primary_key_columns(conn: &Connection, table_name: &str) -> Vec<String> {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({table_name})"))
            .expect("prepare table_info");
        let mapped = stmt
            .query_map([], |row| {
                let name: String = row.get(1)?;
                let pk_pos: i64 = row.get(5)?;
                Ok((name, pk_pos))
            })
            .expect("query table_info");

        let mut pairs = Vec::new();
        for row in mapped {
            let (name, pk_pos) = row.expect("read row");
            if pk_pos > 0 {
                pairs.push((pk_pos, name));
            }
        }
        pairs.sort_by_key(|(pk_pos, _)| *pk_pos);
        pairs.into_iter().map(|(_, name)| name).collect()
    }
}
