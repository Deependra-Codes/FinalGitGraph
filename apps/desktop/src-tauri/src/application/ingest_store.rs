#[cfg(feature = "lsm-rocksdb")]
use std::collections::HashSet;
use std::path::Path;
#[cfg(feature = "lsm-rocksdb")]
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(feature = "lsm-rocksdb")]
use std::sync::OnceLock;
#[cfg(feature = "lsm-rocksdb")]
use std::thread;
#[cfg(feature = "lsm-rocksdb")]
use std::time::{Duration, Instant};

#[cfg(feature = "lsm-rocksdb")]
use rayon::prelude::*;
#[cfg(feature = "lsm-rocksdb")]
use rocksdb::{
    ColumnFamilyDescriptor, Direction, IteratorMode, Options, WriteBatch, WriteOptions, DB,
};
#[cfg(feature = "lsm-rocksdb")]
use rusqlite::params_from_iter;
#[cfg(feature = "lsm-rocksdb")]
use rusqlite::types::Value;
use rusqlite::Connection;
#[cfg(feature = "lsm-rocksdb")]
use rusqlite::OptionalExtension;

#[cfg(feature = "lsm-rocksdb")]
use crate::adapters::db::{
    enqueue_file_changes_for_enrichment, upsert_commit_edges_from_commit_shas,
};
use crate::adapters::db::{insert_commits_batch, insert_file_changes_batch};
use crate::domain::models::{IndexingMode, ParsedCommit, ParsedFileChange};
#[cfg(feature = "lsm-rocksdb")]
use crate::support::error::AppError;
use crate::support::error::AppResult;

#[cfg(feature = "lsm-rocksdb")]
const LSM_PATH_DEFAULT: &str = ".gitgraph-lab/ingest-rocksdb";
#[cfg(feature = "lsm-rocksdb")]
const LSM_CF_COMMITS: &str = "commits";
#[cfg(feature = "lsm-rocksdb")]
const LSM_CF_FILE_CHANGES: &str = "file_changes";
#[cfg(feature = "lsm-rocksdb")]
const LSM_CF_COMMIT_MATERIALIZATION_QUEUE: &str = "commit_materialization_queue";
#[cfg(feature = "lsm-rocksdb")]
const LSM_CF_FILE_CHANGE_MATERIALIZATION_QUEUE: &str = "file_change_materialization_queue";
#[cfg(feature = "lsm-rocksdb")]
const LSM_CF_UNRESOLVED_COMMIT_EDGES: &str = "unresolved_commit_edges";
#[cfg(feature = "lsm-rocksdb")]
const LSM_CF_CHECKPOINTS: &str = "checkpoints";
#[cfg(feature = "lsm-rocksdb")]
const LSM_CHECKPOINT_FILE_CHANGE_LAST_SEQ_KEY: &[u8] = b"file_changes_last_seq";
#[cfg(feature = "lsm-rocksdb")]
const LSM_CHECKPOINT_REPO_COMMIT_ENQUEUED_SEQ_PREFIX: &[u8] = b"repo_commit_enqueued_seq:";
#[cfg(feature = "lsm-rocksdb")]
const LSM_CHECKPOINT_REPO_COMMIT_APPLIED_SEQ_PREFIX: &[u8] = b"repo_commit_applied_seq:";
#[cfg(feature = "lsm-rocksdb")]
const LSM_CHECKPOINT_REPO_FILE_CHANGE_ENQUEUED_SEQ_PREFIX: &[u8] =
    b"repo_file_change_enqueued_seq:";
#[cfg(feature = "lsm-rocksdb")]
const LSM_CHECKPOINT_REPO_FILE_CHANGE_APPLIED_SEQ_PREFIX: &[u8] = b"repo_file_change_applied_seq:";
#[cfg(feature = "lsm-rocksdb")]
const LSM_MATERIALIZER_BATCH_SIZE_DEFAULT: usize = 2_000;
#[cfg(feature = "lsm-rocksdb")]
const LSM_MATERIALIZER_IDLE_SLEEP_MS: u64 = 50;
#[cfg(feature = "lsm-rocksdb")]
const LSM_MATERIALIZER_IDLE_MAX_CYCLES: usize = 64;
#[cfg(feature = "lsm-rocksdb")]
const SQLITE_MAX_BIND_PARAMETERS_FALLBACK: usize = 999;
#[cfg(feature = "lsm-rocksdb")]
const LSM_UNRESOLVED_EDGE_VALUE: &[u8] = b"1";

#[cfg(feature = "lsm-rocksdb")]
static LSM_PATH: OnceLock<PathBuf> = OnceLock::new();

pub trait IngestStore {
    fn persist_commits(
        &self,
        conn: &Connection,
        repo_id: i64,
        mode: IndexingMode,
        commits: &[ParsedCommit],
    ) -> AppResult<usize>;

    fn persist_file_changes(
        &self,
        conn: &Connection,
        repo_id: i64,
        mode: IndexingMode,
        file_changes: &[ParsedFileChange],
    ) -> AppResult<usize>;
}

#[cfg(feature = "lsm-rocksdb")]
pub fn should_inline_enqueue_file_change_enrichment() -> bool {
    false
}

#[cfg(not(feature = "lsm-rocksdb"))]
pub fn should_inline_enqueue_file_change_enrichment() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Default)]
pub struct LsmShadowMetricsSnapshot {
    pub write_attempts: u64,
    pub write_success: u64,
    pub write_errors: u64,
    pub commit_batches: u64,
    pub file_change_batches: u64,
    pub disabled_batches: u64,
    pub bytes_written: u64,
    pub write_micros: u64,
    pub materialization_batches: u64,
    pub materialization_rows: u64,
    pub materialization_errors: u64,
    pub materialization_micros: u64,
    pub materialization_queue_fetch_micros: u64,
    pub materialization_decode_micros: u64,
    pub materialization_group_micros: u64,
    pub materialization_sqlite_write_micros: u64,
    pub materialization_queue_cleanup_micros: u64,
    pub materialization_checkpoint_update_micros: u64,
    pub materialization_empty_check_micros: u64,
    pub materialization_keys_scanned: u64,
    pub materialization_rows_decoded: u64,
    pub materialization_invalid_rows: u64,
    pub materialization_rows_applied: u64,
    pub materialization_rows_unresolved: u64,
    pub materialization_commit_edges_immediate: u64,
    pub materialization_commit_edges_resolved: u64,
    pub materialization_commit_edges_unresolved_staged: u64,
}

static LSM_SHADOW_WRITE_ATTEMPTS: AtomicU64 = AtomicU64::new(0);
static LSM_SHADOW_WRITE_SUCCESS: AtomicU64 = AtomicU64::new(0);
static LSM_SHADOW_WRITE_ERRORS: AtomicU64 = AtomicU64::new(0);
static LSM_SHADOW_COMMIT_BATCHES: AtomicU64 = AtomicU64::new(0);
static LSM_SHADOW_FILE_CHANGE_BATCHES: AtomicU64 = AtomicU64::new(0);
static LSM_SHADOW_DISABLED_BATCHES: AtomicU64 = AtomicU64::new(0);
static LSM_SHADOW_BYTES_WRITTEN: AtomicU64 = AtomicU64::new(0);
static LSM_SHADOW_WRITE_MICROS: AtomicU64 = AtomicU64::new(0);
static LSM_MATERIALIZATION_BATCHES: AtomicU64 = AtomicU64::new(0);
static LSM_MATERIALIZATION_ROWS: AtomicU64 = AtomicU64::new(0);
static LSM_MATERIALIZATION_ERRORS: AtomicU64 = AtomicU64::new(0);
static LSM_MATERIALIZATION_MICROS: AtomicU64 = AtomicU64::new(0);
static LSM_MATERIALIZATION_QUEUE_FETCH_MICROS: AtomicU64 = AtomicU64::new(0);
static LSM_MATERIALIZATION_DECODE_MICROS: AtomicU64 = AtomicU64::new(0);
static LSM_MATERIALIZATION_GROUP_MICROS: AtomicU64 = AtomicU64::new(0);
static LSM_MATERIALIZATION_SQLITE_WRITE_MICROS: AtomicU64 = AtomicU64::new(0);
static LSM_MATERIALIZATION_QUEUE_CLEANUP_MICROS: AtomicU64 = AtomicU64::new(0);
static LSM_MATERIALIZATION_CHECKPOINT_UPDATE_MICROS: AtomicU64 = AtomicU64::new(0);
static LSM_MATERIALIZATION_EMPTY_CHECK_MICROS: AtomicU64 = AtomicU64::new(0);
static LSM_MATERIALIZATION_KEYS_SCANNED: AtomicU64 = AtomicU64::new(0);
static LSM_MATERIALIZATION_ROWS_DECODED: AtomicU64 = AtomicU64::new(0);
static LSM_MATERIALIZATION_INVALID_ROWS: AtomicU64 = AtomicU64::new(0);
static LSM_MATERIALIZATION_ROWS_APPLIED: AtomicU64 = AtomicU64::new(0);
static LSM_MATERIALIZATION_ROWS_UNRESOLVED: AtomicU64 = AtomicU64::new(0);
static LSM_MATERIALIZATION_COMMIT_EDGES_IMMEDIATE: AtomicU64 = AtomicU64::new(0);
static LSM_MATERIALIZATION_COMMIT_EDGES_RESOLVED: AtomicU64 = AtomicU64::new(0);
static LSM_MATERIALIZATION_COMMIT_EDGES_UNRESOLVED_STAGED: AtomicU64 = AtomicU64::new(0);

pub fn reset_lsm_shadow_metrics() {
    LSM_SHADOW_WRITE_ATTEMPTS.store(0, Ordering::Relaxed);
    LSM_SHADOW_WRITE_SUCCESS.store(0, Ordering::Relaxed);
    LSM_SHADOW_WRITE_ERRORS.store(0, Ordering::Relaxed);
    LSM_SHADOW_COMMIT_BATCHES.store(0, Ordering::Relaxed);
    LSM_SHADOW_FILE_CHANGE_BATCHES.store(0, Ordering::Relaxed);
    LSM_SHADOW_DISABLED_BATCHES.store(0, Ordering::Relaxed);
    LSM_SHADOW_BYTES_WRITTEN.store(0, Ordering::Relaxed);
    LSM_SHADOW_WRITE_MICROS.store(0, Ordering::Relaxed);
    LSM_MATERIALIZATION_BATCHES.store(0, Ordering::Relaxed);
    LSM_MATERIALIZATION_ROWS.store(0, Ordering::Relaxed);
    LSM_MATERIALIZATION_ERRORS.store(0, Ordering::Relaxed);
    LSM_MATERIALIZATION_MICROS.store(0, Ordering::Relaxed);
    LSM_MATERIALIZATION_QUEUE_FETCH_MICROS.store(0, Ordering::Relaxed);
    LSM_MATERIALIZATION_DECODE_MICROS.store(0, Ordering::Relaxed);
    LSM_MATERIALIZATION_GROUP_MICROS.store(0, Ordering::Relaxed);
    LSM_MATERIALIZATION_SQLITE_WRITE_MICROS.store(0, Ordering::Relaxed);
    LSM_MATERIALIZATION_QUEUE_CLEANUP_MICROS.store(0, Ordering::Relaxed);
    LSM_MATERIALIZATION_CHECKPOINT_UPDATE_MICROS.store(0, Ordering::Relaxed);
    LSM_MATERIALIZATION_EMPTY_CHECK_MICROS.store(0, Ordering::Relaxed);
    LSM_MATERIALIZATION_KEYS_SCANNED.store(0, Ordering::Relaxed);
    LSM_MATERIALIZATION_ROWS_DECODED.store(0, Ordering::Relaxed);
    LSM_MATERIALIZATION_INVALID_ROWS.store(0, Ordering::Relaxed);
    LSM_MATERIALIZATION_ROWS_APPLIED.store(0, Ordering::Relaxed);
    LSM_MATERIALIZATION_ROWS_UNRESOLVED.store(0, Ordering::Relaxed);
    LSM_MATERIALIZATION_COMMIT_EDGES_IMMEDIATE.store(0, Ordering::Relaxed);
    LSM_MATERIALIZATION_COMMIT_EDGES_RESOLVED.store(0, Ordering::Relaxed);
    LSM_MATERIALIZATION_COMMIT_EDGES_UNRESOLVED_STAGED.store(0, Ordering::Relaxed);
}

pub fn lsm_shadow_metrics_snapshot() -> LsmShadowMetricsSnapshot {
    LsmShadowMetricsSnapshot {
        write_attempts: LSM_SHADOW_WRITE_ATTEMPTS.load(Ordering::Relaxed),
        write_success: LSM_SHADOW_WRITE_SUCCESS.load(Ordering::Relaxed),
        write_errors: LSM_SHADOW_WRITE_ERRORS.load(Ordering::Relaxed),
        commit_batches: LSM_SHADOW_COMMIT_BATCHES.load(Ordering::Relaxed),
        file_change_batches: LSM_SHADOW_FILE_CHANGE_BATCHES.load(Ordering::Relaxed),
        disabled_batches: LSM_SHADOW_DISABLED_BATCHES.load(Ordering::Relaxed),
        bytes_written: LSM_SHADOW_BYTES_WRITTEN.load(Ordering::Relaxed),
        write_micros: LSM_SHADOW_WRITE_MICROS.load(Ordering::Relaxed),
        materialization_batches: LSM_MATERIALIZATION_BATCHES.load(Ordering::Relaxed),
        materialization_rows: LSM_MATERIALIZATION_ROWS.load(Ordering::Relaxed),
        materialization_errors: LSM_MATERIALIZATION_ERRORS.load(Ordering::Relaxed),
        materialization_micros: LSM_MATERIALIZATION_MICROS.load(Ordering::Relaxed),
        materialization_queue_fetch_micros: LSM_MATERIALIZATION_QUEUE_FETCH_MICROS
            .load(Ordering::Relaxed),
        materialization_decode_micros: LSM_MATERIALIZATION_DECODE_MICROS.load(Ordering::Relaxed),
        materialization_group_micros: LSM_MATERIALIZATION_GROUP_MICROS.load(Ordering::Relaxed),
        materialization_sqlite_write_micros: LSM_MATERIALIZATION_SQLITE_WRITE_MICROS
            .load(Ordering::Relaxed),
        materialization_queue_cleanup_micros: LSM_MATERIALIZATION_QUEUE_CLEANUP_MICROS
            .load(Ordering::Relaxed),
        materialization_checkpoint_update_micros: LSM_MATERIALIZATION_CHECKPOINT_UPDATE_MICROS
            .load(Ordering::Relaxed),
        materialization_empty_check_micros: LSM_MATERIALIZATION_EMPTY_CHECK_MICROS
            .load(Ordering::Relaxed),
        materialization_keys_scanned: LSM_MATERIALIZATION_KEYS_SCANNED.load(Ordering::Relaxed),
        materialization_rows_decoded: LSM_MATERIALIZATION_ROWS_DECODED.load(Ordering::Relaxed),
        materialization_invalid_rows: LSM_MATERIALIZATION_INVALID_ROWS.load(Ordering::Relaxed),
        materialization_rows_applied: LSM_MATERIALIZATION_ROWS_APPLIED.load(Ordering::Relaxed),
        materialization_rows_unresolved: LSM_MATERIALIZATION_ROWS_UNRESOLVED
            .load(Ordering::Relaxed),
        materialization_commit_edges_immediate: LSM_MATERIALIZATION_COMMIT_EDGES_IMMEDIATE
            .load(Ordering::Relaxed),
        materialization_commit_edges_resolved: LSM_MATERIALIZATION_COMMIT_EDGES_RESOLVED
            .load(Ordering::Relaxed),
        materialization_commit_edges_unresolved_staged:
            LSM_MATERIALIZATION_COMMIT_EDGES_UNRESOLVED_STAGED.load(Ordering::Relaxed),
    }
}

#[cfg(feature = "lsm-rocksdb")]
pub fn initialize_lsm_path(db_path: &Path) -> AppResult<()> {
    let resolved = resolve_lsm_path(db_path);
    if LSM_PATH.get().is_some() {
        return Ok(());
    }
    LSM_PATH
        .set(resolved)
        .map_err(|_| AppError::Parse("lsm path already initialized".to_string()))?;
    Ok(())
}

#[cfg(not(feature = "lsm-rocksdb"))]
pub fn initialize_lsm_path(_db_path: &Path) -> AppResult<()> {
    Ok(())
}

#[cfg(feature = "lsm-rocksdb")]
pub fn ensure_lsm_materializer_started(db_path: &Path) {
    let _ = initialize_lsm_path(db_path);
}

#[cfg(not(feature = "lsm-rocksdb"))]
pub fn ensure_lsm_materializer_started(db_path: &Path) {
    let _ = initialize_lsm_path(db_path);
}

#[cfg(feature = "lsm-rocksdb")]
fn resolve_lsm_path(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .map(|parent| parent.join("ingest-rocksdb"))
        .unwrap_or_else(|| PathBuf::from(LSM_PATH_DEFAULT))
}

#[cfg(feature = "lsm-rocksdb")]
fn lsm_path() -> AppResult<&'static PathBuf> {
    LSM_PATH.get().ok_or_else(|| {
        AppError::Parse("lsm path not initialized; call initialize_lsm_path".to_string())
    })
}

#[cfg(feature = "lsm-rocksdb")]
fn record_lsm_write_attempt(is_commit_batch: bool) {
    LSM_SHADOW_WRITE_ATTEMPTS.fetch_add(1, Ordering::Relaxed);
    if is_commit_batch {
        LSM_SHADOW_COMMIT_BATCHES.fetch_add(1, Ordering::Relaxed);
    } else {
        LSM_SHADOW_FILE_CHANGE_BATCHES.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(feature = "lsm-rocksdb")]
fn record_lsm_write_success(bytes_written: u64, elapsed_micros: u64) {
    LSM_SHADOW_WRITE_SUCCESS.fetch_add(1, Ordering::Relaxed);
    LSM_SHADOW_BYTES_WRITTEN.fetch_add(bytes_written, Ordering::Relaxed);
    LSM_SHADOW_WRITE_MICROS.fetch_add(elapsed_micros, Ordering::Relaxed);
}

#[cfg(feature = "lsm-rocksdb")]
fn record_lsm_write_error(elapsed_micros: u64) {
    LSM_SHADOW_WRITE_ERRORS.fetch_add(1, Ordering::Relaxed);
    LSM_SHADOW_WRITE_MICROS.fetch_add(elapsed_micros, Ordering::Relaxed);
}

#[cfg(feature = "lsm-rocksdb")]
fn record_materialization_errors(count: usize) {
    if count == 0 {
        return;
    }
    LSM_MATERIALIZATION_ERRORS
        .fetch_add(u64::try_from(count).unwrap_or(u64::MAX), Ordering::Relaxed);
}

#[cfg(feature = "lsm-rocksdb")]
fn record_materialization_error() {
    record_materialization_errors(1);
}

#[cfg(feature = "lsm-rocksdb")]
fn record_materialization_phase(
    queue_fetch_micros: u64,
    decode_micros: u64,
    group_micros: u64,
    sqlite_write_micros: u64,
    queue_cleanup_micros: u64,
    checkpoint_update_micros: u64,
) {
    LSM_MATERIALIZATION_QUEUE_FETCH_MICROS.fetch_add(queue_fetch_micros, Ordering::Relaxed);
    LSM_MATERIALIZATION_DECODE_MICROS.fetch_add(decode_micros, Ordering::Relaxed);
    LSM_MATERIALIZATION_GROUP_MICROS.fetch_add(group_micros, Ordering::Relaxed);
    LSM_MATERIALIZATION_SQLITE_WRITE_MICROS.fetch_add(sqlite_write_micros, Ordering::Relaxed);
    LSM_MATERIALIZATION_QUEUE_CLEANUP_MICROS.fetch_add(queue_cleanup_micros, Ordering::Relaxed);
    LSM_MATERIALIZATION_CHECKPOINT_UPDATE_MICROS
        .fetch_add(checkpoint_update_micros, Ordering::Relaxed);
}

#[cfg(feature = "lsm-rocksdb")]
fn record_materialization_empty_check(elapsed_micros: u64) {
    LSM_MATERIALIZATION_EMPTY_CHECK_MICROS.fetch_add(elapsed_micros, Ordering::Relaxed);
}

#[cfg(feature = "lsm-rocksdb")]
fn record_materialization_cardinality(
    keys_scanned: usize,
    rows_decoded: usize,
    invalid_rows: usize,
    rows_applied: usize,
    rows_unresolved: usize,
) {
    LSM_MATERIALIZATION_KEYS_SCANNED.fetch_add(
        u64::try_from(keys_scanned).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
    LSM_MATERIALIZATION_ROWS_DECODED.fetch_add(
        u64::try_from(rows_decoded).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
    LSM_MATERIALIZATION_INVALID_ROWS.fetch_add(
        u64::try_from(invalid_rows).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
    LSM_MATERIALIZATION_ROWS_APPLIED.fetch_add(
        u64::try_from(rows_applied).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
    LSM_MATERIALIZATION_ROWS_UNRESOLVED.fetch_add(
        u64::try_from(rows_unresolved).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
}

#[cfg(feature = "lsm-rocksdb")]
fn record_materialization_commit_edge_cardinality(
    immediate_edges: usize,
    resolved_edges: usize,
    unresolved_edges_staged: usize,
) {
    LSM_MATERIALIZATION_COMMIT_EDGES_IMMEDIATE.fetch_add(
        u64::try_from(immediate_edges).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
    LSM_MATERIALIZATION_COMMIT_EDGES_RESOLVED.fetch_add(
        u64::try_from(resolved_edges).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
    LSM_MATERIALIZATION_COMMIT_EDGES_UNRESOLVED_STAGED.fetch_add(
        u64::try_from(unresolved_edges_staged).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
}

#[cfg(feature = "lsm-rocksdb")]
fn elapsed_micros_u64(started: Instant) -> u64 {
    let micros = started.elapsed().as_micros();
    u64::try_from(micros).unwrap_or(u64::MAX)
}

pub struct PrimaryIngestStore;
pub use PrimaryIngestStore as SqliteIngestStore;

impl IngestStore for PrimaryIngestStore {
    fn persist_commits(
        &self,
        conn: &Connection,
        repo_id: i64,
        mode: IndexingMode,
        commits: &[ParsedCommit],
    ) -> AppResult<usize> {
        #[cfg(feature = "lsm-rocksdb")]
        {
            let _ = conn;
            persist_commits_primary_lsm(repo_id, mode, commits)
        }

        #[cfg(not(feature = "lsm-rocksdb"))]
        {
            let _ = mode;
            // Commits are immutable by SHA identity. Insert-only avoids conflict-update overhead.
            let inserted = insert_commits_batch(conn, repo_id, commits)?;
            Ok(inserted)
        }
    }

    fn persist_file_changes(
        &self,
        conn: &Connection,
        repo_id: i64,
        mode: IndexingMode,
        file_changes: &[ParsedFileChange],
    ) -> AppResult<usize> {
        #[cfg(feature = "lsm-rocksdb")]
        {
            let _ = conn;
            persist_file_changes_primary_lsm(repo_id, mode, file_changes)
        }

        #[cfg(not(feature = "lsm-rocksdb"))]
        {
            let _ = mode;
            // Full mode starts from cleared data and incremental mode deletes touched rows before insert.
            let inserted = insert_file_changes_batch(conn, repo_id, file_changes)?;
            Ok(inserted)
        }
    }
}

#[cfg(feature = "lsm-rocksdb")]
fn materializer_batch_size() -> usize {
    LSM_MATERIALIZER_BATCH_SIZE_DEFAULT
}

#[cfg(feature = "lsm-rocksdb")]
fn build_repo_checkpoint_key(prefix: &[u8], repo_id: i64) -> Vec<u8> {
    let mut key = Vec::with_capacity(prefix.len() + std::mem::size_of::<i64>());
    key.extend_from_slice(prefix);
    key.extend_from_slice(&repo_id.to_be_bytes());
    key
}

#[cfg(feature = "lsm-rocksdb")]
fn parse_u64_be_bytes(bytes: &[u8]) -> Option<u64> {
    if bytes.len() != std::mem::size_of::<u64>() {
        return None;
    }
    let array: [u8; std::mem::size_of::<u64>()] = bytes.try_into().ok()?;
    Some(u64::from_be_bytes(array))
}

#[cfg(feature = "lsm-rocksdb")]
fn load_checkpoint_u64(
    db: &DB,
    checkpoint_cf: &impl rocksdb::AsColumnFamilyRef,
    key: &[u8],
) -> Option<u64> {
    db.get_pinned_cf(checkpoint_cf, key)
        .ok()
        .flatten()
        .and_then(|value| parse_u64_be_bytes(value.as_ref()))
}

#[cfg(feature = "lsm-rocksdb")]
fn put_checkpoint_u64(
    batch: &mut WriteBatch,
    checkpoint_cf: &impl rocksdb::AsColumnFamilyRef,
    key: &[u8],
    value: u64,
) {
    batch.put_cf(checkpoint_cf, key, value.to_be_bytes());
}

#[cfg(feature = "lsm-rocksdb")]
fn parse_queue_sequence(key: &[u8]) -> Option<u64> {
    parse_u64_be_bytes(key)
}

#[cfg(feature = "lsm-rocksdb")]
#[derive(Clone)]
struct QueuedFileChange {
    repo_id: i64,
    mode: IndexingMode,
    commit_sha: String,
    file_path: String,
    additions: Option<i64>,
    deletions: Option<i64>,
    change_type: String,
    stats_state: crate::domain::models::FileChangeStatsState,
    requires_line_stats: bool,
}

#[cfg(feature = "lsm-rocksdb")]
#[derive(Clone)]
struct QueuedCommit {
    repo_id: i64,
    mode: IndexingMode,
    sha: String,
    parents: Vec<String>,
    tree_sha: String,
    author_name: String,
    author_email: String,
    author_time: i64,
    committer_time: i64,
    subject: String,
}

#[cfg(feature = "lsm-rocksdb")]
type MaterializedCommitQueueEntry = (Vec<u8>, Option<u64>, QueuedCommit);

#[cfg(feature = "lsm-rocksdb")]
fn write_u32(buffer: &mut Vec<u8>, value: u32) {
    buffer.extend_from_slice(&value.to_be_bytes());
}

#[cfg(feature = "lsm-rocksdb")]
fn write_i64(buffer: &mut Vec<u8>, value: i64) {
    buffer.extend_from_slice(&value.to_be_bytes());
}

#[cfg(feature = "lsm-rocksdb")]
fn write_bool(buffer: &mut Vec<u8>, value: bool) {
    buffer.push(u8::from(value));
}

#[cfg(feature = "lsm-rocksdb")]
fn write_string(buffer: &mut Vec<u8>, value: &str) {
    let bytes = value.as_bytes();
    write_u32(buffer, u32::try_from(bytes.len()).unwrap_or(u32::MAX));
    buffer.extend_from_slice(bytes);
}

#[cfg(feature = "lsm-rocksdb")]
fn write_optional_i64(buffer: &mut Vec<u8>, value: Option<i64>) {
    match value {
        Some(number) => {
            buffer.push(1);
            write_i64(buffer, number);
        }
        None => buffer.push(0),
    }
}

#[cfg(feature = "lsm-rocksdb")]
fn queue_mode_to_byte(mode: IndexingMode) -> u8 {
    match mode {
        IndexingMode::Full => 0,
        IndexingMode::Incremental => 1,
    }
}

#[cfg(feature = "lsm-rocksdb")]
fn queue_stats_state_to_byte(state: crate::domain::models::FileChangeStatsState) -> u8 {
    match state {
        crate::domain::models::FileChangeStatsState::Pending => 0,
        crate::domain::models::FileChangeStatsState::Ready => 1,
        crate::domain::models::FileChangeStatsState::Failed => 2,
    }
}

#[cfg(feature = "lsm-rocksdb")]
fn queue_mode_from_byte(byte: u8) -> Option<IndexingMode> {
    match byte {
        0 => Some(IndexingMode::Full),
        1 => Some(IndexingMode::Incremental),
        _ => None,
    }
}

#[cfg(feature = "lsm-rocksdb")]
fn queue_stats_state_from_byte(byte: u8) -> Option<crate::domain::models::FileChangeStatsState> {
    match byte {
        0 => Some(crate::domain::models::FileChangeStatsState::Pending),
        1 => Some(crate::domain::models::FileChangeStatsState::Ready),
        2 => Some(crate::domain::models::FileChangeStatsState::Failed),
        _ => None,
    }
}

#[cfg(feature = "lsm-rocksdb")]
fn encode_queued_file_change(change: &QueuedFileChange) -> Vec<u8> {
    let mut buffer = Vec::with_capacity(128 + change.commit_sha.len() + change.file_path.len());
    write_i64(&mut buffer, change.repo_id);
    buffer.push(queue_mode_to_byte(change.mode));
    write_string(&mut buffer, &change.commit_sha);
    write_string(&mut buffer, &change.file_path);
    write_optional_i64(&mut buffer, change.additions);
    write_optional_i64(&mut buffer, change.deletions);
    write_string(&mut buffer, &change.change_type);
    buffer.push(queue_stats_state_to_byte(change.stats_state));
    write_bool(&mut buffer, change.requires_line_stats);
    buffer
}

#[cfg(feature = "lsm-rocksdb")]
fn encode_queued_commit(commit: &QueuedCommit) -> Vec<u8> {
    let mut buffer = Vec::with_capacity(256 + commit.sha.len() + commit.subject.len());
    write_i64(&mut buffer, commit.repo_id);
    buffer.push(queue_mode_to_byte(commit.mode));
    write_string(&mut buffer, &commit.sha);
    write_u32(
        &mut buffer,
        u32::try_from(commit.parents.len()).unwrap_or(u32::MAX),
    );
    for parent in &commit.parents {
        write_string(&mut buffer, parent);
    }
    write_string(&mut buffer, &commit.tree_sha);
    write_string(&mut buffer, &commit.author_name);
    write_string(&mut buffer, &commit.author_email);
    write_i64(&mut buffer, commit.author_time);
    write_i64(&mut buffer, commit.committer_time);
    write_string(&mut buffer, &commit.subject);
    buffer
}

#[cfg(feature = "lsm-rocksdb")]
fn read_u32(bytes: &[u8], cursor: &mut usize) -> Option<u32> {
    let end = cursor.saturating_add(4);
    if end > bytes.len() {
        return None;
    }
    let value = u32::from_be_bytes(bytes[*cursor..end].try_into().ok()?);
    *cursor = end;
    Some(value)
}

#[cfg(feature = "lsm-rocksdb")]
fn read_i64(bytes: &[u8], cursor: &mut usize) -> Option<i64> {
    let end = cursor.saturating_add(8);
    if end > bytes.len() {
        return None;
    }
    let value = i64::from_be_bytes(bytes[*cursor..end].try_into().ok()?);
    *cursor = end;
    Some(value)
}

#[cfg(feature = "lsm-rocksdb")]
fn read_bool(bytes: &[u8], cursor: &mut usize) -> Option<bool> {
    let end = cursor.saturating_add(1);
    if end > bytes.len() {
        return None;
    }
    let value = bytes[*cursor] != 0;
    *cursor = end;
    Some(value)
}

#[cfg(feature = "lsm-rocksdb")]
fn read_string(bytes: &[u8], cursor: &mut usize) -> Option<String> {
    let len = usize::try_from(read_u32(bytes, cursor)?).ok()?;
    let end = cursor.saturating_add(len);
    if end > bytes.len() {
        return None;
    }
    let value = std::str::from_utf8(&bytes[*cursor..end]).ok()?.to_string();
    *cursor = end;
    Some(value)
}

#[cfg(feature = "lsm-rocksdb")]
fn read_optional_i64(bytes: &[u8], cursor: &mut usize) -> Option<Option<i64>> {
    let has_value = read_bool(bytes, cursor)?;
    if has_value {
        Some(Some(read_i64(bytes, cursor)?))
    } else {
        Some(None)
    }
}

#[cfg(feature = "lsm-rocksdb")]
fn decode_queued_file_change(bytes: &[u8]) -> Option<QueuedFileChange> {
    let mut cursor = 0usize;
    let repo_id = read_i64(bytes, &mut cursor)?;
    let mode_byte = *bytes.get(cursor)?;
    cursor += 1;
    let mode = queue_mode_from_byte(mode_byte)?;
    let commit_sha = read_string(bytes, &mut cursor)?;
    let file_path = read_string(bytes, &mut cursor)?;
    let additions = read_optional_i64(bytes, &mut cursor)?;
    let deletions = read_optional_i64(bytes, &mut cursor)?;
    let change_type = read_string(bytes, &mut cursor)?;
    let stats_state_byte = *bytes.get(cursor)?;
    cursor += 1;
    let stats_state = queue_stats_state_from_byte(stats_state_byte)?;
    let requires_line_stats = read_bool(bytes, &mut cursor)?;
    Some(QueuedFileChange {
        repo_id,
        mode,
        commit_sha,
        file_path,
        additions,
        deletions,
        change_type,
        stats_state,
        requires_line_stats,
    })
}

#[cfg(feature = "lsm-rocksdb")]
fn decode_queued_commit(bytes: &[u8]) -> Option<QueuedCommit> {
    let mut cursor = 0usize;
    let repo_id = read_i64(bytes, &mut cursor)?;
    let mode_byte = *bytes.get(cursor)?;
    cursor += 1;
    let mode = queue_mode_from_byte(mode_byte)?;
    let sha = read_string(bytes, &mut cursor)?;
    let parent_count = usize::try_from(read_u32(bytes, &mut cursor)?).ok()?;
    let mut parents = Vec::with_capacity(parent_count);
    for _ in 0..parent_count {
        parents.push(read_string(bytes, &mut cursor)?);
    }
    let tree_sha = read_string(bytes, &mut cursor)?;
    let author_name = read_string(bytes, &mut cursor)?;
    let author_email = read_string(bytes, &mut cursor)?;
    let author_time = read_i64(bytes, &mut cursor)?;
    let committer_time = read_i64(bytes, &mut cursor)?;
    let subject = read_string(bytes, &mut cursor)?;

    Some(QueuedCommit {
        repo_id,
        mode,
        sha,
        parents,
        tree_sha,
        author_name,
        author_email,
        author_time,
        committer_time,
        subject,
    })
}

#[cfg(feature = "lsm-rocksdb")]
fn next_materialization_sequence() -> u64 {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_micros())
        .unwrap_or(0);
    let high = u64::try_from(micros)
        .unwrap_or(u64::MAX)
        .checked_shl(16)
        .unwrap_or(u64::MAX);
    high | (seq & 0xFFFF)
}

#[cfg(feature = "lsm-rocksdb")]
fn build_materialization_queue_key(sequence: u64) -> [u8; 8] {
    sequence.to_be_bytes()
}

#[cfg(feature = "lsm-rocksdb")]
pub fn drain_lsm_materializer(conn: &mut Connection, repo_id: i64) -> AppResult<()> {
    let mut idle_cycles = 0usize;
    loop {
        let batch_size = materializer_batch_size();
        let commit_processed = materialize_commit_batch(conn, batch_size)?;
        let file_processed = materialize_file_change_batch(conn, batch_size)?;

        if commit_processed == 0 && file_processed == 0 {
            let empty_check_started = Instant::now();
            let queue_has_entries = lsm_queue_has_entries_for_repo(repo_id)?;
            record_materialization_empty_check(elapsed_micros_u64(empty_check_started));
            if !queue_has_entries {
                break;
            }
            idle_cycles = idle_cycles.saturating_add(1);
            if idle_cycles >= LSM_MATERIALIZER_IDLE_MAX_CYCLES {
                return Err(AppError::Parse(format!(
                    "lsm materialization stalled for repo_id {repo_id}"
                )));
            }
            thread::sleep(Duration::from_millis(LSM_MATERIALIZER_IDLE_SLEEP_MS));
        } else {
            idle_cycles = 0;
        }
    }
    Ok(())
}

#[cfg(feature = "lsm-rocksdb")]
pub fn clear_lsm_repo(repo_id: i64) -> AppResult<()> {
    const DELETE_BATCH_SIZE: usize = 10_000;
    let db = lsm_db()?;
    let prefix = repo_id.to_be_bytes();

    let Some(commits_cf) = db.cf_handle(LSM_CF_COMMITS) else {
        return Err(lsm_write_error(
            "lsm clear commits cf lookup",
            "missing commits cf",
        ));
    };
    let Some(file_changes_cf) = db.cf_handle(LSM_CF_FILE_CHANGES) else {
        return Err(lsm_write_error(
            "lsm clear file_changes cf lookup",
            "missing file_changes cf",
        ));
    };
    let Some(commit_queue_cf) = db.cf_handle(LSM_CF_COMMIT_MATERIALIZATION_QUEUE) else {
        return Err(lsm_write_error(
            "lsm clear commit queue cf lookup",
            "missing commit_materialization_queue cf",
        ));
    };
    let Some(file_change_queue_cf) = db.cf_handle(LSM_CF_FILE_CHANGE_MATERIALIZATION_QUEUE) else {
        return Err(lsm_write_error(
            "lsm clear file-change queue cf lookup",
            "missing file_change_materialization_queue cf",
        ));
    };
    let Some(unresolved_edge_cf) = db.cf_handle(LSM_CF_UNRESOLVED_COMMIT_EDGES) else {
        return Err(lsm_write_error(
            "lsm clear unresolved commit edges cf lookup",
            "missing unresolved_commit_edges cf",
        ));
    };
    let Some(checkpoint_cf) = db.cf_handle(LSM_CF_CHECKPOINTS) else {
        return Err(lsm_write_error(
            "lsm clear checkpoints cf lookup",
            "missing checkpoints cf",
        ));
    };

    let mut pending = 0usize;
    let mut batch = WriteBatch::default();
    for entry in db.iterator_cf(commits_cf, IteratorMode::Start) {
        let Ok((key, _)) = entry else {
            continue;
        };
        if key.starts_with(prefix.as_slice()) {
            batch.delete_cf(commits_cf, key);
            pending = pending.saturating_add(1);
            if pending >= DELETE_BATCH_SIZE {
                db.write_opt(batch, &write_options())
                    .map_err(|error| lsm_write_error("lsm clear commits", error))?;
                batch = WriteBatch::default();
                pending = 0;
            }
        }
    }
    if pending > 0 {
        db.write_opt(batch, &write_options())
            .map_err(|error| lsm_write_error("lsm clear commits", error))?;
    }

    let mut pending = 0usize;
    let mut batch = WriteBatch::default();
    for entry in db.iterator_cf(file_changes_cf, IteratorMode::Start) {
        let Ok((key, _)) = entry else {
            continue;
        };
        if key.starts_with(prefix.as_slice()) {
            batch.delete_cf(file_changes_cf, key);
            pending = pending.saturating_add(1);
            if pending >= DELETE_BATCH_SIZE {
                db.write_opt(batch, &write_options())
                    .map_err(|error| lsm_write_error("lsm clear file_changes", error))?;
                batch = WriteBatch::default();
                pending = 0;
            }
        }
    }
    if pending > 0 {
        db.write_opt(batch, &write_options())
            .map_err(|error| lsm_write_error("lsm clear file_changes", error))?;
    }

    let mut pending = 0usize;
    let mut batch = WriteBatch::default();
    for entry in db.iterator_cf(commit_queue_cf, IteratorMode::Start) {
        let Ok((key, value)) = entry else {
            continue;
        };
        let Some(decoded) = decode_queued_commit(value.as_ref()) else {
            continue;
        };
        if decoded.repo_id == repo_id {
            batch.delete_cf(commit_queue_cf, key);
            pending = pending.saturating_add(1);
            if pending >= DELETE_BATCH_SIZE {
                db.write_opt(batch, &write_options())
                    .map_err(|error| lsm_write_error("lsm clear commit queue", error))?;
                batch = WriteBatch::default();
                pending = 0;
            }
        }
    }
    if pending > 0 {
        db.write_opt(batch, &write_options())
            .map_err(|error| lsm_write_error("lsm clear commit queue", error))?;
    }

    let mut pending = 0usize;
    let mut batch = WriteBatch::default();
    for entry in db.iterator_cf(file_change_queue_cf, IteratorMode::Start) {
        let Ok((key, value)) = entry else {
            continue;
        };
        let Some(decoded) = decode_queued_file_change(value.as_ref()) else {
            continue;
        };
        if decoded.repo_id == repo_id {
            batch.delete_cf(file_change_queue_cf, key);
            pending = pending.saturating_add(1);
            if pending >= DELETE_BATCH_SIZE {
                db.write_opt(batch, &write_options())
                    .map_err(|error| lsm_write_error("lsm clear file-change queue", error))?;
                batch = WriteBatch::default();
                pending = 0;
            }
        }
    }
    if pending > 0 {
        db.write_opt(batch, &write_options())
            .map_err(|error| lsm_write_error("lsm clear file-change queue", error))?;
    }

    let mut pending = 0usize;
    let mut batch = WriteBatch::default();
    for entry in db.iterator_cf(unresolved_edge_cf, IteratorMode::Start) {
        let Ok((key, _)) = entry else {
            continue;
        };
        if key.starts_with(prefix.as_slice()) {
            batch.delete_cf(unresolved_edge_cf, key);
            pending = pending.saturating_add(1);
            if pending >= DELETE_BATCH_SIZE {
                db.write_opt(batch, &write_options())
                    .map_err(|error| lsm_write_error("lsm clear unresolved commit edges", error))?;
                batch = WriteBatch::default();
                pending = 0;
            }
        }
    }
    if pending > 0 {
        db.write_opt(batch, &write_options())
            .map_err(|error| lsm_write_error("lsm clear unresolved commit edges", error))?;
    }

    let mut checkpoint_cleanup = WriteBatch::default();
    let commit_enqueued_key =
        build_repo_checkpoint_key(LSM_CHECKPOINT_REPO_COMMIT_ENQUEUED_SEQ_PREFIX, repo_id);
    let commit_applied_key =
        build_repo_checkpoint_key(LSM_CHECKPOINT_REPO_COMMIT_APPLIED_SEQ_PREFIX, repo_id);
    let file_change_enqueued_key =
        build_repo_checkpoint_key(LSM_CHECKPOINT_REPO_FILE_CHANGE_ENQUEUED_SEQ_PREFIX, repo_id);
    let file_change_applied_key =
        build_repo_checkpoint_key(LSM_CHECKPOINT_REPO_FILE_CHANGE_APPLIED_SEQ_PREFIX, repo_id);
    checkpoint_cleanup.delete_cf(checkpoint_cf, commit_enqueued_key);
    checkpoint_cleanup.delete_cf(checkpoint_cf, commit_applied_key);
    checkpoint_cleanup.delete_cf(checkpoint_cf, file_change_enqueued_key);
    checkpoint_cleanup.delete_cf(checkpoint_cf, file_change_applied_key);
    db.write_opt(checkpoint_cleanup, &write_options())
        .map_err(|error| lsm_write_error("lsm clear repo checkpoints", error))?;

    Ok(())
}

#[cfg(feature = "lsm-rocksdb")]
fn lsm_queue_has_entries_for_repo(repo_id: i64) -> AppResult<bool> {
    let db = lsm_db()?;
    let Some(checkpoint_cf) = db.cf_handle(LSM_CF_CHECKPOINTS) else {
        return Err(lsm_write_error(
            "lsm queue check checkpoints cf lookup",
            "missing checkpoints cf",
        ));
    };
    let commit_enqueued_key =
        build_repo_checkpoint_key(LSM_CHECKPOINT_REPO_COMMIT_ENQUEUED_SEQ_PREFIX, repo_id);
    let commit_applied_key =
        build_repo_checkpoint_key(LSM_CHECKPOINT_REPO_COMMIT_APPLIED_SEQ_PREFIX, repo_id);
    let file_change_enqueued_key =
        build_repo_checkpoint_key(LSM_CHECKPOINT_REPO_FILE_CHANGE_ENQUEUED_SEQ_PREFIX, repo_id);
    let file_change_applied_key =
        build_repo_checkpoint_key(LSM_CHECKPOINT_REPO_FILE_CHANGE_APPLIED_SEQ_PREFIX, repo_id);
    let commit_enqueued = load_checkpoint_u64(db, checkpoint_cf, &commit_enqueued_key);
    let commit_applied = load_checkpoint_u64(db, checkpoint_cf, &commit_applied_key);
    let file_change_enqueued = load_checkpoint_u64(db, checkpoint_cf, &file_change_enqueued_key);
    let file_change_applied = load_checkpoint_u64(db, checkpoint_cf, &file_change_applied_key);
    let has_checkpoint_state = commit_enqueued.is_some()
        || commit_applied.is_some()
        || file_change_enqueued.is_some()
        || file_change_applied.is_some();
    if has_checkpoint_state {
        let pending_commit = commit_enqueued.unwrap_or(0) > commit_applied.unwrap_or(0);
        let pending_file_change =
            file_change_enqueued.unwrap_or(0) > file_change_applied.unwrap_or(0);
        return Ok(pending_commit || pending_file_change);
    }

    let Some(commit_queue_cf) = db.cf_handle(LSM_CF_COMMIT_MATERIALIZATION_QUEUE) else {
        return Err(lsm_write_error(
            "lsm queue check commit cf lookup",
            "missing commit_materialization_queue cf",
        ));
    };
    let Some(file_change_queue_cf) = db.cf_handle(LSM_CF_FILE_CHANGE_MATERIALIZATION_QUEUE) else {
        return Err(lsm_write_error(
            "lsm queue check file-change cf lookup",
            "missing file_change_materialization_queue cf",
        ));
    };

    for entry in db.iterator_cf(commit_queue_cf, IteratorMode::Start) {
        let Ok((_key, value)) = entry else {
            continue;
        };
        if let Some(decoded) = decode_queued_commit(value.as_ref()) {
            if decoded.repo_id == repo_id {
                return Ok(true);
            }
        }
    }

    for entry in db.iterator_cf(file_change_queue_cf, IteratorMode::Start) {
        let Ok((_key, value)) = entry else {
            continue;
        };
        if let Some(decoded) = decode_queued_file_change(value.as_ref()) {
            if decoded.repo_id == repo_id {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

#[cfg(feature = "lsm-rocksdb")]
fn materialize_commit_batch(conn: &mut Connection, batch_size: usize) -> AppResult<usize> {
    let db = lsm_db()?;
    let Some(queue_cf) = db.cf_handle(LSM_CF_COMMIT_MATERIALIZATION_QUEUE) else {
        return Err(lsm_write_error(
            "commit materialization queue cf lookup",
            "missing commit_materialization_queue cf",
        ));
    };
    let Some(unresolved_edge_cf) = db.cf_handle(LSM_CF_UNRESOLVED_COMMIT_EDGES) else {
        return Err(lsm_write_error(
            "unresolved commit edges cf lookup",
            "missing unresolved_commit_edges cf",
        ));
    };
    let Some(checkpoint_cf) = db.cf_handle(LSM_CF_CHECKPOINTS) else {
        return Err(lsm_write_error(
            "lsm checkpoint cf lookup",
            "missing checkpoints cf",
        ));
    };

    let queue_fetch_started = Instant::now();
    let mut raw_entries: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(batch_size);
    for entry in db
        .iterator_cf(queue_cf, IteratorMode::Start)
        .take(batch_size)
    {
        let Ok((key, value)) = entry else {
            continue;
        };
        raw_entries.push((key.to_vec(), value.to_vec()));
    }
    let queue_fetch_elapsed = queue_fetch_started.elapsed();
    let keys_scanned = raw_entries.len();
    if raw_entries.is_empty() {
        return Ok(0);
    }

    let decode_started = Instant::now();
    let mut queue_entries: Vec<MaterializedCommitQueueEntry> =
        Vec::with_capacity(raw_entries.len());
    let mut invalid_keys: Vec<Vec<u8>> = Vec::new();
    for (key, value) in raw_entries {
        match decode_queued_commit(value.as_ref()) {
            Some(decoded) => {
                let sequence = parse_queue_sequence(&key);
                queue_entries.push((key, sequence, decoded));
            }
            None => invalid_keys.push(key),
        };
    }
    let decode_elapsed = decode_started.elapsed();
    let rows_decoded = queue_entries.len();

    record_materialization_errors(invalid_keys.len());

    if queue_entries.is_empty() {
        let queue_cleanup_started = Instant::now();
        if !invalid_keys.is_empty() {
            let mut clear_batch = WriteBatch::default();
            for key in &invalid_keys {
                clear_batch.delete_cf(queue_cf, key);
            }
            if let Err(error) = db.write_opt(clear_batch, &write_options()) {
                record_materialization_error();
                eprintln!("[lsm-materializer] commit queue cleanup failed: {error}");
            }
        }
        let queue_cleanup_elapsed = queue_cleanup_started.elapsed();
        record_materialization_phase(
            u64::try_from(queue_fetch_elapsed.as_micros()).unwrap_or(u64::MAX),
            u64::try_from(decode_elapsed.as_micros()).unwrap_or(u64::MAX),
            0,
            0,
            u64::try_from(queue_cleanup_elapsed.as_micros()).unwrap_or(u64::MAX),
            0,
        );
        record_materialization_cardinality(keys_scanned, 0, invalid_keys.len(), 0, 0);
        return Ok(0);
    }

    let started = Instant::now();
    let group_started = Instant::now();
    let mut grouped: std::collections::HashMap<i64, Vec<MaterializedCommitQueueEntry>> =
        std::collections::HashMap::new();
    for (key, sequence, item) in queue_entries {
        grouped
            .entry(item.repo_id)
            .or_default()
            .push((key, sequence, item));
    }
    let group_elapsed = group_started.elapsed();

    let sqlite_write_started = Instant::now();

    let mut total_rows = 0usize;
    let mut processed_entries: Vec<(i64, Vec<u8>, Option<u64>)> = Vec::new();
    let mut unresolved_entries_to_put: Vec<Vec<u8>> = Vec::new();
    let mut unresolved_entries_to_delete: Vec<Vec<u8>> = Vec::new();
    let mut immediate_edge_count = 0usize;
    let mut resolved_edge_count = 0usize;
    let mut unresolved_staged_count = 0usize;
    for (repo_id, commits) in grouped {
        let parsed_commits: Vec<ParsedCommit> = commits
            .iter()
            .map(|(_, _, item)| ParsedCommit {
                sha: item.sha.clone(),
                parents: item.parents.clone(),
                tree_sha: item.tree_sha.clone(),
                author_name: item.author_name.clone(),
                author_email: item.author_email.clone(),
                author_time: item.author_time,
                committer_time: item.committer_time,
                subject: item.subject.clone(),
            })
            .collect();
        let tx = conn.transaction()?;
        insert_commits_batch(&tx, repo_id, &parsed_commits)?;

        let mut unique_parents: HashSet<String> = HashSet::new();
        for (_, _, item) in &commits {
            for parent in &item.parents {
                unique_parents.insert(parent.clone());
            }
        }
        let parent_list: Vec<String> = unique_parents.into_iter().collect();
        let materialized_parents = fetch_materialized_commit_shas(&tx, repo_id, &parent_list)?;

        let mut immediate_edges: Vec<(String, String)> = Vec::new();
        for (key, sequence, item) in &commits {
            processed_entries.push((repo_id, key.clone(), *sequence));
            for parent in &item.parents {
                if materialized_parents.contains(parent) {
                    immediate_edges.push((item.sha.clone(), parent.clone()));
                    immediate_edge_count = immediate_edge_count.saturating_add(1);
                    // Clear stale unresolved markers if they exist from previous retries/crashes.
                    unresolved_entries_to_delete
                        .push(build_unresolved_edge_key(repo_id, parent, &item.sha));
                } else {
                    unresolved_entries_to_put
                        .push(build_unresolved_edge_key(repo_id, parent, &item.sha));
                    unresolved_staged_count = unresolved_staged_count.saturating_add(1);
                }
            }
        }

        // Resolve children that were waiting on commits materialized in this batch.
        let parent_candidates: HashSet<String> = parsed_commits
            .iter()
            .map(|commit| commit.sha.clone())
            .collect();
        let mut deferred_edges: Vec<(String, String)> = Vec::new();
        for parent_sha in parent_candidates {
            let prefix = build_unresolved_edge_parent_prefix(repo_id, &parent_sha);
            for entry in db.iterator_cf(
                unresolved_edge_cf,
                IteratorMode::From(prefix.as_slice(), Direction::Forward),
            ) {
                let Ok((key, _value)) = entry else {
                    continue;
                };
                if !key.starts_with(prefix.as_slice()) {
                    break;
                }
                let key_vec = key.to_vec();
                match parse_unresolved_edge_key(key.as_ref()) {
                    Some((pending_repo_id, pending_parent_sha, child_sha))
                        if pending_repo_id == repo_id && pending_parent_sha == parent_sha =>
                    {
                        deferred_edges.push((child_sha, parent_sha.clone()));
                        unresolved_entries_to_delete.push(key_vec);
                    }
                    _ => {
                        // Corrupt or malformed unresolved keys should be cleaned up opportunistically.
                        record_materialization_error();
                        unresolved_entries_to_delete.push(key_vec);
                    }
                }
            }
        }

        if !immediate_edges.is_empty() || !deferred_edges.is_empty() {
            resolved_edge_count = resolved_edge_count.saturating_add(deferred_edges.len());
            immediate_edges.extend(deferred_edges);
            upsert_commit_edges_from_commit_shas(&tx, repo_id, &immediate_edges)?;
        }
        tx.commit()?;
        total_rows = total_rows.saturating_add(parsed_commits.len());
    }
    let sqlite_write_elapsed = sqlite_write_started.elapsed();

    let processed_count = processed_entries.len();
    let queue_cleanup_started = Instant::now();
    let mut checkpoint_update_elapsed = Duration::ZERO;
    if processed_count > 0
        || !invalid_keys.is_empty()
        || !unresolved_entries_to_put.is_empty()
        || !unresolved_entries_to_delete.is_empty()
    {
        let mut cleanup_batch = WriteBatch::default();
        let mut repo_applied_seq_max: std::collections::HashMap<i64, u64> =
            std::collections::HashMap::new();
        for (repo_id, key, sequence) in &processed_entries {
            if let Some(sequence) = sequence {
                let entry = repo_applied_seq_max.entry(*repo_id).or_insert(*sequence);
                if *sequence > *entry {
                    *entry = *sequence;
                }
            }
            cleanup_batch.delete_cf(queue_cf, key);
        }
        for key in &invalid_keys {
            cleanup_batch.delete_cf(queue_cf, key);
        }
        for key in &unresolved_entries_to_put {
            cleanup_batch.put_cf(unresolved_edge_cf, key, LSM_UNRESOLVED_EDGE_VALUE);
        }
        for key in &unresolved_entries_to_delete {
            cleanup_batch.delete_cf(unresolved_edge_cf, key);
        }
        let checkpoint_update_started = Instant::now();
        for (repo_id, max_sequence) in repo_applied_seq_max {
            let applied_key =
                build_repo_checkpoint_key(LSM_CHECKPOINT_REPO_COMMIT_APPLIED_SEQ_PREFIX, repo_id);
            put_checkpoint_u64(
                &mut cleanup_batch,
                checkpoint_cf,
                &applied_key,
                max_sequence,
            );
        }
        checkpoint_update_elapsed = checkpoint_update_started.elapsed();
        if let Err(error) = db.write_opt(cleanup_batch, &write_options()) {
            record_materialization_error();
            eprintln!("[lsm-materializer] commit queue cleanup failed: {error}");
        }
    }
    let queue_cleanup_elapsed = queue_cleanup_started.elapsed();

    let elapsed = elapsed_micros_u64(started);
    LSM_MATERIALIZATION_BATCHES.fetch_add(1, Ordering::Relaxed);
    LSM_MATERIALIZATION_ROWS.fetch_add(
        u64::try_from(total_rows).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
    LSM_MATERIALIZATION_MICROS.fetch_add(elapsed, Ordering::Relaxed);
    record_materialization_phase(
        u64::try_from(queue_fetch_elapsed.as_micros()).unwrap_or(u64::MAX),
        u64::try_from(decode_elapsed.as_micros()).unwrap_or(u64::MAX),
        u64::try_from(group_elapsed.as_micros()).unwrap_or(u64::MAX),
        u64::try_from(sqlite_write_elapsed.as_micros()).unwrap_or(u64::MAX),
        u64::try_from(queue_cleanup_elapsed.as_micros()).unwrap_or(u64::MAX),
        u64::try_from(checkpoint_update_elapsed.as_micros()).unwrap_or(u64::MAX),
    );
    record_materialization_cardinality(
        keys_scanned,
        rows_decoded,
        invalid_keys.len(),
        total_rows,
        unresolved_staged_count,
    );
    record_materialization_commit_edge_cardinality(
        immediate_edge_count,
        resolved_edge_count,
        unresolved_staged_count,
    );

    Ok(processed_count)
}

#[cfg(feature = "lsm-rocksdb")]
fn materialize_file_change_batch(conn: &mut Connection, batch_size: usize) -> AppResult<usize> {
    let db = lsm_db()?;
    let Some(queue_cf) = db.cf_handle(LSM_CF_FILE_CHANGE_MATERIALIZATION_QUEUE) else {
        return Err(lsm_write_error(
            "file-change materialization queue cf lookup",
            "missing file_change_materialization_queue cf",
        ));
    };
    let Some(checkpoint_cf) = db.cf_handle(LSM_CF_CHECKPOINTS) else {
        return Err(lsm_write_error(
            "lsm checkpoint cf lookup",
            "missing checkpoints cf",
        ));
    };

    let queue_fetch_started = Instant::now();
    let resume_start_key =
        load_checkpoint_u64(db, checkpoint_cf, LSM_CHECKPOINT_FILE_CHANGE_LAST_SEQ_KEY)
            .and_then(|sequence| sequence.checked_add(1))
            .map(u64::to_be_bytes);
    let iterator_mode = if let Some(start_key) = resume_start_key.as_ref() {
        IteratorMode::From(start_key.as_slice(), Direction::Forward)
    } else {
        IteratorMode::Start
    };

    let mut queue_keys: Vec<Vec<u8>> = Vec::with_capacity(batch_size);
    let mut queue_key_sequences: Vec<Option<u64>> = Vec::with_capacity(batch_size);
    let mut queue_values: Vec<Vec<u8>> = Vec::with_capacity(batch_size);
    for entry in db.iterator_cf(queue_cf, iterator_mode).take(batch_size) {
        let Ok((key, value)) = entry else {
            continue;
        };
        let sequence = parse_queue_sequence(key.as_ref());
        queue_keys.push(key.to_vec());
        queue_key_sequences.push(sequence);
        queue_values.push(value.to_vec());
    }
    let queue_fetch_elapsed = queue_fetch_started.elapsed();
    let keys_scanned = queue_values.len();

    if queue_values.is_empty() {
        return Ok(0);
    }

    let started = Instant::now();
    let decode_started = Instant::now();
    let decoded_results: Vec<Option<QueuedFileChange>> = queue_values
        .par_iter()
        .map(|value| decode_queued_file_change(value))
        .collect();
    let mut decoded: Vec<(QueuedFileChange, Option<u64>)> =
        Vec::with_capacity(decoded_results.len());
    let mut invalid_count = 0usize;
    for (index, maybe_change) in decoded_results.into_iter().enumerate() {
        if let Some(change) = maybe_change {
            let sequence = queue_key_sequences.get(index).copied().flatten();
            decoded.push((change, sequence));
        } else {
            invalid_count = invalid_count.saturating_add(1);
        }
    }
    let decode_elapsed = decode_started.elapsed();
    let rows_decoded = decoded.len();
    record_materialization_errors(invalid_count);

    if decoded.is_empty() {
        let queue_cleanup_started = Instant::now();
        let mut clear_batch = WriteBatch::default();
        for key in &queue_keys {
            clear_batch.delete_cf(queue_cf, key);
        }
        let checkpoint_update_started = Instant::now();
        if let Some(last_sequence) = queue_key_sequences.iter().flatten().copied().max() {
            put_checkpoint_u64(
                &mut clear_batch,
                checkpoint_cf,
                LSM_CHECKPOINT_FILE_CHANGE_LAST_SEQ_KEY,
                last_sequence,
            );
        }
        let checkpoint_update_elapsed = checkpoint_update_started.elapsed();
        if let Err(error) = db.write_opt(clear_batch, &write_options()) {
            record_materialization_error();
            eprintln!("[lsm-materializer] queue cleanup failed: {error}");
        }
        let queue_cleanup_elapsed = queue_cleanup_started.elapsed();
        record_materialization_phase(
            u64::try_from(queue_fetch_elapsed.as_micros()).unwrap_or(u64::MAX),
            u64::try_from(decode_elapsed.as_micros()).unwrap_or(u64::MAX),
            0,
            0,
            u64::try_from(queue_cleanup_elapsed.as_micros()).unwrap_or(u64::MAX),
            u64::try_from(checkpoint_update_elapsed.as_micros()).unwrap_or(u64::MAX),
        );
        record_materialization_cardinality(keys_scanned, 0, invalid_count, 0, 0);
        return Ok(0);
    }

    let group_started = Instant::now();
    let mut grouped: std::collections::HashMap<i64, Vec<ParsedFileChange>> =
        std::collections::HashMap::new();
    let mut repo_applied_seq_max: std::collections::HashMap<i64, u64> =
        std::collections::HashMap::new();
    for (item, sequence) in &decoded {
        grouped
            .entry(item.repo_id)
            .or_default()
            .push(ParsedFileChange {
                commit_sha: item.commit_sha.clone(),
                file_path: item.file_path.clone(),
                additions: item.additions,
                deletions: item.deletions,
                change_type: item.change_type.clone(),
                stats_state: item.stats_state,
                requires_line_stats: item.requires_line_stats,
            });
        if let Some(sequence) = sequence {
            let entry = repo_applied_seq_max
                .entry(item.repo_id)
                .or_insert(*sequence);
            if *sequence > *entry {
                *entry = *sequence;
            }
        }
    }
    let group_elapsed = group_started.elapsed();

    let sqlite_write_started = Instant::now();
    for (repo_id, changes) in grouped {
        let tx = conn.transaction()?;
        insert_file_changes_batch(&tx, repo_id, &changes)?;
        enqueue_file_changes_for_enrichment(&tx, repo_id, &changes)?;
        tx.commit()?;
    }
    let sqlite_write_elapsed = sqlite_write_started.elapsed();

    let queue_cleanup_started = Instant::now();
    let mut cleanup_batch = WriteBatch::default();
    for key in &queue_keys {
        cleanup_batch.delete_cf(queue_cf, key);
    }
    let checkpoint_update_started = Instant::now();
    if let Some(last_sequence) = queue_key_sequences.iter().flatten().copied().max() {
        put_checkpoint_u64(
            &mut cleanup_batch,
            checkpoint_cf,
            LSM_CHECKPOINT_FILE_CHANGE_LAST_SEQ_KEY,
            last_sequence,
        );
    }
    for (repo_id, max_sequence) in repo_applied_seq_max {
        let applied_key =
            build_repo_checkpoint_key(LSM_CHECKPOINT_REPO_FILE_CHANGE_APPLIED_SEQ_PREFIX, repo_id);
        put_checkpoint_u64(
            &mut cleanup_batch,
            checkpoint_cf,
            &applied_key,
            max_sequence,
        );
    }
    let checkpoint_update_elapsed = checkpoint_update_started.elapsed();
    if let Err(error) = db.write_opt(cleanup_batch, &write_options()) {
        record_materialization_error();
        eprintln!("[lsm-materializer] queue cleanup failed: {error}");
    }
    let queue_cleanup_elapsed = queue_cleanup_started.elapsed();

    let elapsed = elapsed_micros_u64(started);
    LSM_MATERIALIZATION_BATCHES.fetch_add(1, Ordering::Relaxed);
    LSM_MATERIALIZATION_ROWS.fetch_add(
        u64::try_from(decoded.len()).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
    LSM_MATERIALIZATION_MICROS.fetch_add(elapsed, Ordering::Relaxed);
    record_materialization_phase(
        u64::try_from(queue_fetch_elapsed.as_micros()).unwrap_or(u64::MAX),
        u64::try_from(decode_elapsed.as_micros()).unwrap_or(u64::MAX),
        u64::try_from(group_elapsed.as_micros()).unwrap_or(u64::MAX),
        u64::try_from(sqlite_write_elapsed.as_micros()).unwrap_or(u64::MAX),
        u64::try_from(queue_cleanup_elapsed.as_micros()).unwrap_or(u64::MAX),
        u64::try_from(checkpoint_update_elapsed.as_micros()).unwrap_or(u64::MAX),
    );
    record_materialization_cardinality(keys_scanned, rows_decoded, invalid_count, decoded.len(), 0);

    Ok(decoded.len())
}

#[cfg(feature = "lsm-rocksdb")]
fn lsm_db() -> AppResult<&'static DB> {
    static DB_INSTANCE: OnceLock<Result<DB, String>> = OnceLock::new();

    let path = lsm_path()?;
    let instance = DB_INSTANCE.get_or_init(|| {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let mut options = Options::default();
        options.create_if_missing(true);
        options.create_missing_column_families(true);
        options.set_max_open_files(256);
        options.increase_parallelism(4);

        let commit_cf = ColumnFamilyDescriptor::new(LSM_CF_COMMITS, Options::default());
        let file_change_cf = ColumnFamilyDescriptor::new(LSM_CF_FILE_CHANGES, Options::default());
        let commit_queue_cf =
            ColumnFamilyDescriptor::new(LSM_CF_COMMIT_MATERIALIZATION_QUEUE, Options::default());
        let queue_cf = ColumnFamilyDescriptor::new(
            LSM_CF_FILE_CHANGE_MATERIALIZATION_QUEUE,
            Options::default(),
        );
        let unresolved_edge_cf =
            ColumnFamilyDescriptor::new(LSM_CF_UNRESOLVED_COMMIT_EDGES, Options::default());
        let checkpoint_cf = ColumnFamilyDescriptor::new(LSM_CF_CHECKPOINTS, Options::default());

        DB::open_cf_descriptors(
            &options,
            path,
            vec![
                commit_cf,
                file_change_cf,
                commit_queue_cf,
                queue_cf,
                unresolved_edge_cf,
                checkpoint_cf,
            ],
        )
        .map_err(|error| format!("initialize rocksdb ingest store: {error}"))
    });

    match instance {
        Ok(db) => Ok(db),
        Err(message) => Err(AppError::Parse(message.clone())),
    }
}

#[cfg(feature = "lsm-rocksdb")]
fn mode_tag(mode: IndexingMode) -> &'static str {
    match mode {
        IndexingMode::Full => "full",
        IndexingMode::Incremental => "incremental",
    }
}

#[cfg(feature = "lsm-rocksdb")]
fn append_key_part(buffer: &mut Vec<u8>, value: &str) {
    let bytes = value.as_bytes();
    let part_len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
    buffer.extend_from_slice(&part_len.to_be_bytes());
    buffer.extend_from_slice(bytes);
}

#[cfg(feature = "lsm-rocksdb")]
fn build_commit_key(repo_id: i64, commit_sha: &str) -> Vec<u8> {
    let mut key = Vec::with_capacity(32 + commit_sha.len());
    key.extend_from_slice(&repo_id.to_be_bytes());
    append_key_part(&mut key, commit_sha);
    key
}

#[cfg(feature = "lsm-rocksdb")]
fn build_file_change_key(repo_id: i64, commit_sha: &str, file_path: &str) -> Vec<u8> {
    let mut key = Vec::with_capacity(48 + commit_sha.len() + file_path.len());
    key.extend_from_slice(&repo_id.to_be_bytes());
    append_key_part(&mut key, commit_sha);
    append_key_part(&mut key, file_path);
    key
}

#[cfg(feature = "lsm-rocksdb")]
fn build_unresolved_edge_key(repo_id: i64, parent_sha: &str, child_sha: &str) -> Vec<u8> {
    let mut key = Vec::with_capacity(48 + parent_sha.len() + child_sha.len());
    key.extend_from_slice(&repo_id.to_be_bytes());
    append_key_part(&mut key, parent_sha);
    append_key_part(&mut key, child_sha);
    key
}

#[cfg(feature = "lsm-rocksdb")]
fn build_unresolved_edge_parent_prefix(repo_id: i64, parent_sha: &str) -> Vec<u8> {
    let mut prefix = Vec::with_capacity(32 + parent_sha.len());
    prefix.extend_from_slice(&repo_id.to_be_bytes());
    append_key_part(&mut prefix, parent_sha);
    prefix
}

#[cfg(feature = "lsm-rocksdb")]
fn parse_unresolved_edge_key(key: &[u8]) -> Option<(i64, String, String)> {
    let mut cursor = 0usize;
    let repo_id = read_i64(key, &mut cursor)?;
    let parent_sha = read_string(key, &mut cursor)?;
    let child_sha = read_string(key, &mut cursor)?;
    if cursor != key.len() {
        return None;
    }
    Some((repo_id, parent_sha, child_sha))
}

#[cfg(feature = "lsm-rocksdb")]
fn write_options() -> WriteOptions {
    let mut options = WriteOptions::default();
    // LSM ingest path prioritizes throughput; SQLite is materialized later.
    options.disable_wal(true);
    options
}

#[cfg(feature = "lsm-rocksdb")]
fn lsm_write_error(context: &str, error: impl std::fmt::Display) -> AppError {
    AppError::Io(std::io::Error::other(format!(
        "rocksdb {context} failed: {error}"
    )))
}

#[cfg(feature = "lsm-rocksdb")]
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

#[cfg(feature = "lsm-rocksdb")]
fn fetch_materialized_commit_shas(
    conn: &Connection,
    repo_id: i64,
    parents: &[String],
) -> AppResult<HashSet<String>> {
    let mut materialized = HashSet::new();
    if parents.is_empty() {
        return Ok(materialized);
    }

    let max_bind_parameters = sqlite_max_bind_parameters(conn);
    let chunk_size = max_bind_parameters.saturating_sub(1).max(1);
    for chunk in parents.chunks(chunk_size) {
        let mut sql = String::from("SELECT sha FROM commits WHERE repo_id = ? AND sha IN (");
        for index in 0..chunk.len() {
            if index > 0 {
                sql.push_str(", ");
            }
            sql.push('?');
        }
        sql.push(')');
        let mut stmt = conn.prepare_cached(&sql)?;
        let mut bind_values = Vec::with_capacity(chunk.len() + 1);
        bind_values.push(Value::Integer(repo_id));
        for sha in chunk {
            bind_values.push(Value::Text(sha.clone()));
        }
        let mut rows = stmt.query(params_from_iter(bind_values.iter()))?;
        while let Some(row) = rows.next()? {
            let sha: String = row.get(0)?;
            materialized.insert(sha);
        }
    }

    Ok(materialized)
}

#[cfg(feature = "lsm-rocksdb")]
fn encode_commit_value(commit: &ParsedCommit, mode: IndexingMode) -> Vec<u8> {
    format!(
        "mode={}\nauthor_name={}\nauthor_email={}\nauthor_time={}\ncommitter_time={}\nsubject={}\ntree_sha={}\nparents={}\n",
        mode_tag(mode),
        commit.author_name,
        commit.author_email,
        commit.author_time,
        commit.committer_time,
        commit.subject,
        commit.tree_sha,
        commit.parents.join(",")
    )
    .into_bytes()
}

#[cfg(feature = "lsm-rocksdb")]
fn encode_file_change_value(change: &ParsedFileChange, mode: IndexingMode) -> Vec<u8> {
    format!(
        "mode={}\nchange_type={}\nstats_state={}\nadditions={}\ndeletions={}\nrequires_line_stats={}\n",
        mode_tag(mode),
        change.change_type,
        change.stats_state.as_str(),
        change
            .additions
            .map(|value| value.to_string())
            .unwrap_or_else(|| "null".to_string()),
        change
            .deletions
            .map(|value| value.to_string())
            .unwrap_or_else(|| "null".to_string()),
        change.requires_line_stats
    )
    .into_bytes()
}

#[cfg(feature = "lsm-rocksdb")]
fn persist_commits_primary_lsm(
    repo_id: i64,
    mode: IndexingMode,
    commits: &[ParsedCommit],
) -> AppResult<usize> {
    if commits.is_empty() {
        return Ok(0);
    }

    let db = lsm_db()?;
    let Some(commits_cf) = db.cf_handle(LSM_CF_COMMITS) else {
        return Err(lsm_write_error(
            "commit primary cf lookup",
            "missing commits cf",
        ));
    };
    let Some(queue_cf) = db.cf_handle(LSM_CF_COMMIT_MATERIALIZATION_QUEUE) else {
        return Err(lsm_write_error(
            "commit materialization queue cf lookup",
            "missing commit_materialization_queue cf",
        ));
    };
    let Some(checkpoint_cf) = db.cf_handle(LSM_CF_CHECKPOINTS) else {
        return Err(lsm_write_error(
            "lsm checkpoint cf lookup",
            "missing checkpoints cf",
        ));
    };

    let started = Instant::now();
    record_lsm_write_attempt(true);
    let mut batch = WriteBatch::default();
    let mut bytes_written = 0u64;
    let mut max_queue_sequence: Option<u64> = None;

    for commit in commits {
        let commit_key = build_commit_key(repo_id, &commit.sha);
        let commit_value = encode_commit_value(commit, mode);
        bytes_written = bytes_written
            .saturating_add(u64::try_from(commit_key.len()).unwrap_or(u64::MAX))
            .saturating_add(u64::try_from(commit_value.len()).unwrap_or(u64::MAX));
        batch.put_cf(commits_cf, commit_key, commit_value);

        let queued = QueuedCommit {
            repo_id,
            mode,
            sha: commit.sha.clone(),
            parents: commit.parents.clone(),
            tree_sha: commit.tree_sha.clone(),
            author_name: commit.author_name.clone(),
            author_email: commit.author_email.clone(),
            author_time: commit.author_time,
            committer_time: commit.committer_time,
            subject: commit.subject.clone(),
        };
        let queue_sequence = next_materialization_sequence();
        let queue_key = build_materialization_queue_key(queue_sequence);
        let queue_value = encode_queued_commit(&queued);
        bytes_written = bytes_written
            .saturating_add(u64::try_from(queue_key.len()).unwrap_or(u64::MAX))
            .saturating_add(u64::try_from(queue_value.len()).unwrap_or(u64::MAX));
        batch.put_cf(queue_cf, queue_key, queue_value);
        max_queue_sequence = Some(
            max_queue_sequence
                .map(|current| current.max(queue_sequence))
                .unwrap_or(queue_sequence),
        );
    }

    if let Some(max_sequence) = max_queue_sequence {
        let enqueued_key =
            build_repo_checkpoint_key(LSM_CHECKPOINT_REPO_COMMIT_ENQUEUED_SEQ_PREFIX, repo_id);
        put_checkpoint_u64(&mut batch, checkpoint_cf, &enqueued_key, max_sequence);
    }

    if let Err(error) = db.write_opt(batch, &write_options()) {
        record_lsm_write_error(elapsed_micros_u64(started));
        return Err(lsm_write_error("commit primary write", error));
    }

    record_lsm_write_success(bytes_written, elapsed_micros_u64(started));
    Ok(commits.len())
}

#[cfg(feature = "lsm-rocksdb")]
fn persist_file_changes_primary_lsm(
    repo_id: i64,
    mode: IndexingMode,
    file_changes: &[ParsedFileChange],
) -> AppResult<usize> {
    if file_changes.is_empty() {
        return Ok(0);
    }

    let db = lsm_db()?;
    let Some(file_changes_cf) = db.cf_handle(LSM_CF_FILE_CHANGES) else {
        return Err(lsm_write_error(
            "file-change primary cf lookup",
            "missing file_changes cf",
        ));
    };
    let Some(queue_cf) = db.cf_handle(LSM_CF_FILE_CHANGE_MATERIALIZATION_QUEUE) else {
        return Err(lsm_write_error(
            "materialization queue cf lookup",
            "missing file_change_materialization_queue cf",
        ));
    };
    let Some(checkpoint_cf) = db.cf_handle(LSM_CF_CHECKPOINTS) else {
        return Err(lsm_write_error(
            "lsm checkpoint cf lookup",
            "missing checkpoints cf",
        ));
    };

    let started = Instant::now();
    record_lsm_write_attempt(false);
    let mut batch = WriteBatch::default();
    let mut bytes_written = 0u64;
    let mut max_queue_sequence: Option<u64> = None;

    for change in file_changes {
        let lsm_key = build_file_change_key(repo_id, &change.commit_sha, &change.file_path);
        let lsm_value = encode_file_change_value(change, mode);
        bytes_written = bytes_written
            .saturating_add(u64::try_from(lsm_key.len()).unwrap_or(u64::MAX))
            .saturating_add(u64::try_from(lsm_value.len()).unwrap_or(u64::MAX));
        batch.put_cf(file_changes_cf, lsm_key, lsm_value);

        let queued = QueuedFileChange {
            repo_id,
            mode,
            commit_sha: change.commit_sha.clone(),
            file_path: change.file_path.clone(),
            additions: change.additions,
            deletions: change.deletions,
            change_type: change.change_type.clone(),
            stats_state: change.stats_state,
            requires_line_stats: change.requires_line_stats,
        };
        let queue_sequence = next_materialization_sequence();
        let queue_key = build_materialization_queue_key(queue_sequence);
        let queue_value = encode_queued_file_change(&queued);
        bytes_written = bytes_written
            .saturating_add(u64::try_from(queue_key.len()).unwrap_or(u64::MAX))
            .saturating_add(u64::try_from(queue_value.len()).unwrap_or(u64::MAX));
        batch.put_cf(queue_cf, queue_key, queue_value);
        max_queue_sequence = Some(
            max_queue_sequence
                .map(|current| current.max(queue_sequence))
                .unwrap_or(queue_sequence),
        );
    }

    if let Some(max_sequence) = max_queue_sequence {
        let enqueued_key =
            build_repo_checkpoint_key(LSM_CHECKPOINT_REPO_FILE_CHANGE_ENQUEUED_SEQ_PREFIX, repo_id);
        put_checkpoint_u64(&mut batch, checkpoint_cf, &enqueued_key, max_sequence);
    }

    if let Err(error) = db.write_opt(batch, &write_options()) {
        record_lsm_write_error(elapsed_micros_u64(started));
        return Err(lsm_write_error("file-change primary write", error));
    }

    record_lsm_write_success(bytes_written, elapsed_micros_u64(started));
    Ok(file_changes.len())
}

#[cfg(all(test, feature = "lsm-rocksdb"))]
mod tests {
    use super::{drain_lsm_materializer, initialize_lsm_path, IngestStore, PrimaryIngestStore};
    use crate::adapters::db::{open_database, run_migrations, upsert_repository};
    use crate::domain::models::{
        FileChangeStatsState, IndexingMode, ParsedCommit, ParsedFileChange,
    };
    use std::path::PathBuf;
    use std::sync::OnceLock;
    use tempfile::TempDir;

    fn test_db_path() -> PathBuf {
        static TEST_DB_PATH: OnceLock<PathBuf> = OnceLock::new();
        TEST_DB_PATH
            .get_or_init(|| {
                let temp =
                    TempDir::new().unwrap_or_else(|error| panic!("create lsm temp dir: {error}"));
                let db_path = temp.path().join("ingest.sqlite");
                std::mem::forget(temp);
                initialize_lsm_path(&db_path)
                    .unwrap_or_else(|error| panic!("initialize lsm path: {error}"));
                db_path
            })
            .clone()
    }

    #[test]
    fn lsm_primary_ingest_materializes_to_sqlite() {
        let db_path = test_db_path();
        let mut conn = open_database(&db_path).unwrap_or_else(|error| panic!("open db: {error}"));
        run_migrations(&mut conn).unwrap_or_else(|error| panic!("run migrations: {error}"));
        let repo_id = upsert_repository(&conn, "repo", "repo", "repo/.git")
            .unwrap_or_else(|error| panic!("insert repo: {error}"));

        let commits = vec![
            ParsedCommit {
                sha: "a1".to_string(),
                parents: Vec::new(),
                tree_sha: "tree1".to_string(),
                author_name: "A".to_string(),
                author_email: "a@example.com".to_string(),
                author_time: 1,
                committer_time: 1,
                subject: "one".to_string(),
            },
            ParsedCommit {
                sha: "b2".to_string(),
                parents: vec!["a1".to_string()],
                tree_sha: "tree2".to_string(),
                author_name: "B".to_string(),
                author_email: "b@example.com".to_string(),
                author_time: 2,
                committer_time: 2,
                subject: "two".to_string(),
            },
        ];
        let file_changes = vec![ParsedFileChange {
            commit_sha: "b2".to_string(),
            file_path: "README.md".to_string(),
            additions: Some(3),
            deletions: Some(1),
            change_type: "modify".to_string(),
            stats_state: FileChangeStatsState::Pending,
            requires_line_stats: true,
        }];

        let store = PrimaryIngestStore;
        let commit_count = store
            .persist_commits(&conn, repo_id, IndexingMode::Full, &commits)
            .unwrap_or_else(|error| panic!("persist commits: {error}"));
        assert_eq!(commit_count, commits.len());
        let file_change_count = store
            .persist_file_changes(&conn, repo_id, IndexingMode::Full, &file_changes)
            .unwrap_or_else(|error| panic!("persist file changes: {error}"));
        assert_eq!(file_change_count, file_changes.len());

        drain_lsm_materializer(&mut conn, repo_id)
            .unwrap_or_else(|error| panic!("drain lsm materializer: {error}"));

        let materialized_commits: i64 = conn
            .query_row(
                "SELECT COUNT(1) FROM commits WHERE repo_id = ?1",
                [repo_id],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("count commits: {error}"));
        assert_eq!(materialized_commits, commits.len() as i64);

        let materialized_file_changes: i64 = conn
            .query_row(
                "SELECT COUNT(1) FROM file_changes WHERE repo_id = ?1",
                [repo_id],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("count file changes: {error}"));
        assert_eq!(materialized_file_changes, file_changes.len() as i64);

        let enrichment_queue_rows: i64 = conn
            .query_row(
                "SELECT COUNT(1) FROM file_change_enrichment_queue WHERE repo_id = ?1",
                [repo_id],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("count enrichment queue: {error}"));
        assert_eq!(enrichment_queue_rows, file_changes.len() as i64);

        // Parent-order skew should not stall queue draining. Child commits materialize immediately,
        // unresolved edges are staged, and get resolved once parent commits arrive.
        let delayed_repo_id = upsert_repository(
            &conn,
            "repo-delayed-parent",
            "repo-delayed-parent",
            "repo-delayed-parent/.git",
        )
        .unwrap_or_else(|error| panic!("insert delayed-parent repo: {error}"));
        let delayed_child = ParsedCommit {
            sha: "child-queued".to_string(),
            parents: vec!["parent-late".to_string()],
            tree_sha: "child-tree".to_string(),
            author_name: "Child".to_string(),
            author_email: "child@example.com".to_string(),
            author_time: 10,
            committer_time: 10,
            subject: "child before parent".to_string(),
        };
        let delayed_parent = ParsedCommit {
            sha: "parent-late".to_string(),
            parents: Vec::new(),
            tree_sha: "parent-tree".to_string(),
            author_name: "Parent".to_string(),
            author_email: "parent@example.com".to_string(),
            author_time: 11,
            committer_time: 11,
            subject: "parent after child".to_string(),
        };
        store
            .persist_commits(
                &conn,
                delayed_repo_id,
                IndexingMode::Full,
                std::slice::from_ref(&delayed_child),
            )
            .unwrap_or_else(|error| panic!("persist delayed child commit: {error}"));
        drain_lsm_materializer(&mut conn, delayed_repo_id)
            .unwrap_or_else(|error| panic!("drain delayed child materializer: {error}"));

        let unresolved_before_parent: i64 = conn
            .query_row(
                r#"
                SELECT COUNT(1)
                FROM commit_edges edge
                JOIN commits child
                  ON child.commit_id = edge.child_commit_id
                JOIN commits parent
                  ON parent.commit_id = edge.parent_commit_id
                WHERE edge.repo_id = ?1
                  AND child.sha = ?2
                  AND parent.sha = ?3
                "#,
                rusqlite::params![delayed_repo_id, "child-queued", "parent-late"],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("count unresolved edge before parent: {error}"));
        assert_eq!(unresolved_before_parent, 0);

        store
            .persist_commits(
                &conn,
                delayed_repo_id,
                IndexingMode::Full,
                std::slice::from_ref(&delayed_parent),
            )
            .unwrap_or_else(|error| panic!("persist delayed parent commit: {error}"));
        drain_lsm_materializer(&mut conn, delayed_repo_id)
            .unwrap_or_else(|error| panic!("drain delayed parent materializer: {error}"));

        let resolved_after_parent: i64 = conn
            .query_row(
                r#"
                SELECT COUNT(1)
                FROM commit_edges edge
                JOIN commits child
                  ON child.commit_id = edge.child_commit_id
                JOIN commits parent
                  ON parent.commit_id = edge.parent_commit_id
                WHERE edge.repo_id = ?1
                  AND child.sha = ?2
                  AND parent.sha = ?3
                "#,
                rusqlite::params![delayed_repo_id, "child-queued", "parent-late"],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("count resolved edge after parent: {error}"));
        assert_eq!(resolved_after_parent, 1);

        let conflicting_root = TempDir::new()
            .unwrap_or_else(|error| panic!("create conflicting lsm temp dir: {error}"));
        let conflicting = conflicting_root.path().join("other.sqlite");
        let conflict_result = initialize_lsm_path(&conflicting);
        assert!(conflict_result.is_ok());
    }
}
