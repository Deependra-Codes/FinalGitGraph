use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use lmdb::{
    Cursor, Database, DatabaseFlags, Environment, EnvironmentFlags, RoTransaction, RwTransaction,
    Transaction, WriteFlags,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::domain::models::{
    CommitWindowCursor, CommitWindowItem, FileChangeExtractionQueueMetrics,
    FileChangeLineStatsUpdate, FileChangeStatsState, FileChangeStatsStateCounts, IndexCheckpoint,
    ParsedCommit, ParsedFileChange, ParsedRef, PendingFileChangeEnrichment,
};
use crate::support::clock::now_epoch_seconds;
use crate::support::error::{AppError, AppResult};

const LMDB_DEFAULT_MAP_SIZE_BYTES: usize = 4 * 1024 * 1024 * 1024;
const LMDB_MIN_MAP_SIZE_BYTES: usize = 512 * 1024 * 1024;
const LMDB_MAX_DBS: u32 = 16;
const LMDB_NO_READAHEAD_ENV: &str = "GITGRAPH_LMDB_NO_READAHEAD";
const LMDB_WRITE_MAP_ENV: &str = "GITGRAPH_LMDB_WRITE_MAP";
const LMDB_MAP_ASYNC_ENV: &str = "GITGRAPH_LMDB_MAP_ASYNC";
const LMDB_NO_MEM_INIT_ENV: &str = "GITGRAPH_LMDB_NO_MEM_INIT";
const GRAPH_OVERLAY_MAX_COMMITS_ENV: &str = "GITGRAPH_GRAPH_OVERLAY_MAX_COMMITS";
const GRAPH_OVERLAY_MAX_EDGES_ENV: &str = "GITGRAPH_GRAPH_OVERLAY_MAX_EDGES";
const REPO_KEY_WIDTH: usize = 20;
const FILE_CHANGE_ENRICHMENT_MAX_ATTEMPTS: i64 = 5;
const FILE_CHANGE_ENRICHMENT_RETRY_BASE_DELAY_SECONDS: i64 = 5;
const FILE_CHANGE_ENRICHMENT_RETRY_MAX_DELAY_SECONDS: i64 = 300;
const FILE_CHANGE_EXTRACTION_MAX_ATTEMPTS: i64 = 5;
const FILE_CHANGE_EXTRACTION_RETRY_BASE_DELAY_SECONDS: i64 = 5;
const FILE_CHANGE_EXTRACTION_RETRY_MAX_DELAY_SECONDS: i64 = 300;
const GRAPH_OVERLAY_MAX_COMMITS_DEFAULT: usize = 512;
const GRAPH_OVERLAY_MAX_EDGES_DEFAULT: usize = 2_048;

const META_NEXT_REPOSITORY_ID: &str = "next_repository_id";
const META_NEXT_DELTA_SEQUENCE: &str = "next_delta_sequence";
const DELTA_LOG_RETENTION_DEFAULT_EVENTS: u64 = 200_000;
const COMMIT_WINDOW_TIME_COMPONENT_WIDTH: usize = 20;
const DUE_TIME_COMPONENT_WIDTH: usize = 20;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct LmdbRuntimeOptions {
    no_readahead: bool,
    write_map: bool,
    map_async: bool,
    no_mem_init: bool,
}

impl LmdbRuntimeOptions {
    fn from_env() -> AppResult<Self> {
        Self::from_bools(
            parse_env_bool_enabled(LMDB_NO_READAHEAD_ENV),
            parse_env_bool_enabled(LMDB_WRITE_MAP_ENV),
            parse_env_bool_enabled(LMDB_MAP_ASYNC_ENV),
            parse_env_bool_enabled(LMDB_NO_MEM_INIT_ENV),
        )
    }

    fn from_bools(
        no_readahead: bool,
        write_map: bool,
        map_async: bool,
        no_mem_init: bool,
    ) -> AppResult<Self> {
        if map_async && !write_map {
            return Err(AppError::InvalidArgument(format!(
                "{LMDB_MAP_ASYNC_ENV}=1 requires {LMDB_WRITE_MAP_ENV}=1"
            )));
        }

        if no_mem_init && write_map {
            return Err(AppError::InvalidArgument(format!(
                "{LMDB_NO_MEM_INIT_ENV}=1 is redundant when {LMDB_WRITE_MAP_ENV}=1; disable one"
            )));
        }

        Ok(Self {
            no_readahead,
            write_map,
            map_async,
            no_mem_init,
        })
    }

    fn environment_flags(self) -> EnvironmentFlags {
        let mut flags = EnvironmentFlags::empty();
        if self.write_map {
            flags |= EnvironmentFlags::WRITE_MAP;
        }
        if self.map_async {
            flags |= EnvironmentFlags::MAP_ASYNC;
        }
        if self.no_mem_init {
            flags |= EnvironmentFlags::NO_MEM_INIT;
        }
        if self.no_readahead && !cfg!(windows) {
            flags |= EnvironmentFlags::NO_READAHEAD;
        }
        flags
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RepositoryRecord {
    id: i64,
    path: String,
    name: String,
    git_dir: String,
    indexed_at_epoch: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitRecord {
    pub sha: String,
    pub parents: Vec<String>,
    pub tree_sha: String,
    pub author_name: String,
    pub author_email: String,
    pub author_time: i64,
    pub committer_time: i64,
    pub subject: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileChangeRecord {
    commit_sha: String,
    file_path: String,
    additions: Option<i64>,
    deletions: Option<i64>,
    change_type: String,
    stats_state: FileChangeStatsState,
    requires_line_stats: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EnrichmentQueueEntry {
    attempts: i64,
    last_error: Option<String>,
    enqueued_at_epoch: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExtractionQueueCommitEntry {
    attempts: i64,
    last_error: Option<String>,
    enqueued_at_epoch: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeltaEvent {
    sequence: u64,
    repo_id: Option<i64>,
    at_epoch: i64,
    kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CsrSnapshot {
    pub generated_at_epoch: i64,
    pub shas: Vec<String>,
    pub parents_offsets: Vec<u32>,
    pub parents_neighbors: Vec<u32>,
    pub children_offsets: Vec<u32>,
    pub children_neighbors: Vec<u32>,
    pub directed_edges: usize,
}

impl CsrSnapshot {
    pub fn node_count(&self) -> usize {
        self.shas.len()
    }

    pub fn parents_for(&self, node_id: usize) -> &[u32] {
        csr_slice(&self.parents_offsets, &self.parents_neighbors, node_id)
    }

    pub fn children_for(&self, node_id: usize) -> &[u32] {
        csr_slice(&self.children_offsets, &self.children_neighbors, node_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphSnapshotMetadata {
    pub snapshot_version: u64,
    pub generated_at_epoch: i64,
    pub node_count: usize,
    pub directed_edges: usize,
    pub pending_overlay_commits: usize,
    pub pending_overlay_edges: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphOverlayParentRow {
    pub child_sha: String,
    pub parent_shas: Vec<String>,
    pub committer_time: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct GraphOverlayChildRow {
    parent_sha: String,
    child_shas: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct GraphSnapshotState {
    pub metadata: GraphSnapshotMetadata,
    pub snapshot: CsrSnapshot,
    pub overlay_parent_rows: Vec<GraphOverlayParentRow>,
    pub overlay_child_rows: HashMap<String, Vec<String>>,
}

#[derive(Clone, Copy)]
struct Databases {
    metadata: Database,
    repositories: Database,
    repository_path_index: Database,
    commits: Database,
    commit_window_index: Database,
    refs: Database,
    file_changes: Database,
    enrichment_queue: Database,
    enrichment_due_index: Database,
    extraction_queue: Database,
    checkpoints: Database,
    csr_snapshots: Database,
    graph_snapshot_metadata: Database,
    graph_overlay_parent_rows: Database,
    graph_overlay_child_rows: Database,
    delta_log: Database,
}

pub struct GraphStore {
    env: Environment,
    dbs: Databases,
    store_dir: PathBuf,
}

impl GraphStore {
    pub fn open(db_path: &Path) -> AppResult<Self> {
        let store_dir = resolve_store_directory(db_path);
        fs::create_dir_all(&store_dir)?;

        let runtime_options = resolve_lmdb_runtime_options()?;
        let mut builder = Environment::new();
        builder
            .set_max_dbs(LMDB_MAX_DBS)
            .set_map_size(resolve_map_size_bytes(&store_dir))
            .set_flags(runtime_options.environment_flags());
        let env = builder.open(&store_dir)?;

        let dbs = Databases {
            metadata: env.create_db(Some("metadata"), DatabaseFlags::empty())?,
            repositories: env.create_db(Some("repositories"), DatabaseFlags::empty())?,
            repository_path_index: env
                .create_db(Some("repository_path_index"), DatabaseFlags::empty())?,
            commits: env.create_db(Some("commits"), DatabaseFlags::empty())?,
            commit_window_index: env
                .create_db(Some("commit_window_index"), DatabaseFlags::empty())?,
            refs: env.create_db(Some("refs"), DatabaseFlags::empty())?,
            file_changes: env.create_db(Some("file_changes"), DatabaseFlags::empty())?,
            enrichment_queue: env.create_db(Some("enrichment_queue"), DatabaseFlags::empty())?,
            enrichment_due_index: env
                .create_db(Some("enrichment_due_index"), DatabaseFlags::empty())?,
            extraction_queue: env.create_db(Some("extraction_queue"), DatabaseFlags::empty())?,
            checkpoints: env.create_db(Some("checkpoints"), DatabaseFlags::empty())?,
            csr_snapshots: env.create_db(Some("csr_snapshots"), DatabaseFlags::empty())?,
            graph_snapshot_metadata: env
                .create_db(Some("graph_snapshot_metadata"), DatabaseFlags::empty())?,
            graph_overlay_parent_rows: env
                .create_db(Some("graph_overlay_parent_rows"), DatabaseFlags::empty())?,
            graph_overlay_child_rows: env
                .create_db(Some("graph_overlay_child_rows"), DatabaseFlags::empty())?,
            delta_log: env.create_db(Some("delta_log"), DatabaseFlags::empty())?,
        };
        {
            let mut txn = env.begin_rw_txn()?;
            ensure_u64_meta(&mut txn, dbs.metadata, META_NEXT_REPOSITORY_ID, 1)?;
            ensure_u64_meta(&mut txn, dbs.metadata, META_NEXT_DELTA_SEQUENCE, 1)?;
            txn.commit()?;
        }

        Ok(Self {
            env,
            dbs,
            store_dir,
        })
    }

    pub fn store_directory(&self) -> &Path {
        &self.store_dir
    }

    pub fn upsert_repository(&self, path: &str, name: &str, git_dir: &str) -> AppResult<i64> {
        let mut txn = self.env.begin_rw_txn()?;
        let repo_id = match txn.get(self.dbs.repository_path_index, &path) {
            Ok(raw) => decode_u64(raw, "decode repository id from path index")? as i64,
            Err(lmdb::Error::NotFound) => {
                let next = read_u64_meta(&txn, self.dbs.metadata, META_NEXT_REPOSITORY_ID)?
                    .unwrap_or(1)
                    .max(1);
                write_u64_meta(
                    &mut txn,
                    self.dbs.metadata,
                    META_NEXT_REPOSITORY_ID,
                    next.saturating_add(1),
                )?;
                let id = i64::try_from(next).map_err(|_| {
                    AppError::Parse(
                        "repository id exceeded i64 while upserting repository".to_string(),
                    )
                })?;
                txn.put(
                    self.dbs.repository_path_index,
                    &path,
                    &encode_u64(next),
                    WriteFlags::empty(),
                )?;
                id
            }
            Err(error) => return Err(error.into()),
        };

        let record = RepositoryRecord {
            id: repo_id,
            path: path.to_string(),
            name: name.to_string(),
            git_dir: git_dir.to_string(),
            indexed_at_epoch: self
                .load_repository_record_from_rw(&txn, repo_id)?
                .and_then(|existing| existing.indexed_at_epoch),
        };

        let key = repo_id.to_be_bytes();
        let value = serialize(&record, "serialize repository record")?;
        txn.put(self.dbs.repositories, &key, &value, WriteFlags::empty())?;
        append_delta_event(
            &mut txn,
            self.dbs,
            Some(repo_id),
            format!("repository_upsert path={path}"),
        )?;
        txn.commit()?;
        Ok(repo_id)
    }

    pub fn load_repository_id_by_path(&self, path: &str) -> AppResult<Option<i64>> {
        let txn = self.env.begin_ro_txn()?;
        match txn.get(self.dbs.repository_path_index, &path) {
            Ok(raw) => Ok(Some(
                i64::try_from(decode_u64(raw, "decode repository id by path")?).map_err(|_| {
                    AppError::Parse(
                        "repository id exceeded i64 while loading repository by path".to_string(),
                    )
                })?,
            )),
            Err(lmdb::Error::NotFound) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn clear_repository_index_data(&self, repo_id: i64) -> AppResult<()> {
        let mut txn = self.env.begin_rw_txn()?;
        let repo_prefix = repository_prefix(repo_id);
        let deleted_commits =
            delete_prefixed_entries(&mut txn, self.dbs.commits, repo_prefix.as_str())?;
        let _ =
            delete_prefixed_entries(&mut txn, self.dbs.commit_window_index, repo_prefix.as_str())?;
        let deleted_refs = delete_prefixed_entries(&mut txn, self.dbs.refs, repo_prefix.as_str())?;
        let deleted_file_changes =
            delete_prefixed_entries(&mut txn, self.dbs.file_changes, repo_prefix.as_str())?;
        let _ = delete_prefixed_entries(&mut txn, self.dbs.enrichment_queue, repo_prefix.as_str())?;
        let _ = delete_prefixed_entries(
            &mut txn,
            self.dbs.enrichment_due_index,
            repo_prefix.as_str(),
        )?;
        let _ = delete_prefixed_entries(&mut txn, self.dbs.extraction_queue, repo_prefix.as_str())?;

        let repo_key = repo_id.to_be_bytes();
        let _ = txn.del(self.dbs.checkpoints, &repo_key, None);
        let _ = txn.del(self.dbs.csr_snapshots, &repo_key, None);
        let _ = txn.del(self.dbs.graph_snapshot_metadata, &repo_key, None);
        let _ = delete_prefixed_entries(
            &mut txn,
            self.dbs.graph_overlay_parent_rows,
            repo_prefix.as_str(),
        )?;
        let _ = delete_prefixed_entries(
            &mut txn,
            self.dbs.graph_overlay_child_rows,
            repo_prefix.as_str(),
        )?;

        append_delta_event(
            &mut txn,
            self.dbs,
            Some(repo_id),
            format!(
                "repository_clear commits={deleted_commits} refs={deleted_refs} file_changes={deleted_file_changes}"
            ),
        )?;
        txn.commit()?;
        Ok(())
    }

    pub fn persist_commits(&self, repo_id: i64, commits: &[ParsedCommit]) -> AppResult<usize> {
        if commits.is_empty() {
            return Ok(0);
        }

        let mut txn = self.env.begin_rw_txn()?;
        for commit in commits {
            let key = repository_commit_key(repo_id, commit.sha.as_str());
            let value = CommitRecord {
                sha: commit.sha.clone(),
                parents: commit.parents.clone(),
                tree_sha: commit.tree_sha.clone(),
                author_name: commit.author_name.clone(),
                author_email: commit.author_email.clone(),
                author_time: commit.author_time,
                committer_time: commit.committer_time,
                subject: commit.subject.clone(),
            };
            let raw = serialize(&value, "serialize commit record")?;
            txn.put(self.dbs.commits, &key, &raw, WriteFlags::empty())?;

            let commit_window_key =
                commit_window_index_key(repo_id, commit.committer_time, commit.sha.as_str())?;
            let commit_window_item = CommitWindowItem {
                sha: commit.sha.clone(),
                author_name: commit.author_name.clone(),
                author_email: commit.author_email.clone(),
                committer_time: commit.committer_time,
                subject: commit.subject.clone(),
                tree_sha: commit.tree_sha.clone(),
            };
            let commit_window_raw =
                serialize(&commit_window_item, "serialize commit-window index item")?;
            txn.put(
                self.dbs.commit_window_index,
                &commit_window_key,
                &commit_window_raw,
                WriteFlags::empty(),
            )?;
        }

        append_delta_event(
            &mut txn,
            self.dbs,
            Some(repo_id),
            format!("commit_upsert count={}", commits.len()),
        )?;
        txn.commit()?;
        Ok(commits.len())
    }

    pub fn synchronize_refs(&self, repo_id: i64, refs: &[ParsedRef]) -> AppResult<usize> {
        let mut txn = self.env.begin_rw_txn()?;
        let prefix = repository_prefix(repo_id);
        let _ = delete_prefixed_entries(&mut txn, self.dbs.refs, prefix.as_str())?;

        for parsed_ref in refs {
            let key = repository_ref_key(repo_id, parsed_ref.ref_name.as_str());
            let raw = serialize(parsed_ref, "serialize parsed ref")?;
            txn.put(self.dbs.refs, &key, &raw, WriteFlags::empty())?;
        }

        append_delta_event(
            &mut txn,
            self.dbs,
            Some(repo_id),
            format!("ref_sync count={}", refs.len()),
        )?;
        txn.commit()?;
        Ok(refs.len())
    }

    pub fn delete_file_changes_for_commits(
        &self,
        repo_id: i64,
        commit_shas: &[String],
    ) -> AppResult<()> {
        if commit_shas.is_empty() {
            return Ok(());
        }

        let mut txn = self.env.begin_rw_txn()?;
        let mut deleted = 0usize;
        let commit_set: HashSet<&str> = commit_shas.iter().map(String::as_str).collect();
        for sha in commit_shas {
            let prefix = repository_commit_file_change_prefix(repo_id, sha.as_str());
            deleted = deleted.saturating_add(delete_prefixed_entries(
                &mut txn,
                self.dbs.file_changes,
                prefix.as_str(),
            )?);
            let _ = delete_prefixed_entries(&mut txn, self.dbs.enrichment_queue, prefix.as_str())?;
        }
        let _ = delete_enrichment_due_entries_for_commits(
            &mut txn,
            self.dbs.enrichment_due_index,
            repo_id,
            &commit_set,
        )?;

        append_delta_event(
            &mut txn,
            self.dbs,
            Some(repo_id),
            format!(
                "file_change_delete commits={} rows={deleted}",
                commit_shas.len()
            ),
        )?;
        txn.commit()?;
        Ok(())
    }

    pub fn persist_file_changes(
        &self,
        repo_id: i64,
        file_changes: &[ParsedFileChange],
    ) -> AppResult<usize> {
        let (applied, _enqueued) =
            self.persist_file_changes_with_enrichment(repo_id, file_changes)?;
        Ok(applied)
    }

    pub fn persist_file_changes_with_enrichment(
        &self,
        repo_id: i64,
        file_changes: &[ParsedFileChange],
    ) -> AppResult<(usize, usize)> {
        if file_changes.is_empty() {
            return Ok((0, 0));
        }

        let mut txn = self.env.begin_rw_txn()?;
        let mut enqueued = 0usize;
        let now_epoch = now_epoch_seconds();
        for change in file_changes {
            let key = repository_file_change_key(
                repo_id,
                change.commit_sha.as_str(),
                change.file_path.as_str(),
            );
            let value = FileChangeRecord {
                commit_sha: change.commit_sha.clone(),
                file_path: change.file_path.clone(),
                additions: change.additions,
                deletions: change.deletions,
                change_type: change.change_type.clone(),
                stats_state: change.stats_state,
                requires_line_stats: change.requires_line_stats,
            };
            let raw = serialize(&value, "serialize file-change record")?;
            txn.put(self.dbs.file_changes, &key, &raw, WriteFlags::empty())?;

            if change.stats_state == FileChangeStatsState::Pending && change.requires_line_stats {
                if let Ok(raw_existing) = txn.get(self.dbs.enrichment_queue, &key) {
                    let existing: EnrichmentQueueEntry = deserialize(
                        raw_existing,
                        "deserialize existing enrichment queue entry before enqueue",
                    )?;
                    let existing_due_key = enrichment_due_index_key(
                        repo_id,
                        existing.enqueued_at_epoch,
                        change.commit_sha.as_str(),
                        change.file_path.as_str(),
                    );
                    let _ = txn.del(self.dbs.enrichment_due_index, &existing_due_key, None);
                }

                let queue = EnrichmentQueueEntry {
                    attempts: 0,
                    last_error: None,
                    enqueued_at_epoch: now_epoch,
                };
                let raw_queue = serialize(&queue, "serialize enrichment queue entry")?;
                txn.put(
                    self.dbs.enrichment_queue,
                    &key,
                    &raw_queue,
                    WriteFlags::empty(),
                )?;
                let due_key = enrichment_due_index_key(
                    repo_id,
                    queue.enqueued_at_epoch,
                    change.commit_sha.as_str(),
                    change.file_path.as_str(),
                );
                txn.put(
                    self.dbs.enrichment_due_index,
                    &due_key,
                    b"",
                    WriteFlags::empty(),
                )?;
                enqueued = enqueued.saturating_add(1);
            }
        }

        append_delta_event(
            &mut txn,
            self.dbs,
            Some(repo_id),
            format!("file_change_upsert count={}", file_changes.len()),
        )?;
        if enqueued > 0 {
            append_delta_event(
                &mut txn,
                self.dbs,
                Some(repo_id),
                format!("enrichment_enqueue count={enqueued}"),
            )?;
        }
        txn.commit()?;
        Ok((file_changes.len(), enqueued))
    }

    pub fn enqueue_file_changes_for_enrichment(
        &self,
        repo_id: i64,
        file_changes: &[ParsedFileChange],
    ) -> AppResult<usize> {
        if file_changes.is_empty() {
            return Ok(0);
        }

        let mut txn = self.env.begin_rw_txn()?;
        let now_epoch = now_epoch_seconds();
        let mut enqueued = 0usize;
        for change in file_changes {
            if change.stats_state != FileChangeStatsState::Pending || !change.requires_line_stats {
                continue;
            }

            let key = repository_file_change_key(
                repo_id,
                change.commit_sha.as_str(),
                change.file_path.as_str(),
            );
            if let Ok(raw_existing) = txn.get(self.dbs.enrichment_queue, &key) {
                let existing: EnrichmentQueueEntry = deserialize(
                    raw_existing,
                    "deserialize existing enrichment queue entry before enqueue",
                )?;
                let existing_due_key = enrichment_due_index_key(
                    repo_id,
                    existing.enqueued_at_epoch,
                    change.commit_sha.as_str(),
                    change.file_path.as_str(),
                );
                let _ = txn.del(self.dbs.enrichment_due_index, &existing_due_key, None);
            }

            let queue = EnrichmentQueueEntry {
                attempts: 0,
                last_error: None,
                enqueued_at_epoch: now_epoch,
            };
            let raw = serialize(&queue, "serialize enrichment queue entry")?;
            txn.put(self.dbs.enrichment_queue, &key, &raw, WriteFlags::empty())?;
            let due_key = enrichment_due_index_key(
                repo_id,
                queue.enqueued_at_epoch,
                change.commit_sha.as_str(),
                change.file_path.as_str(),
            );
            txn.put(
                self.dbs.enrichment_due_index,
                &due_key,
                b"",
                WriteFlags::empty(),
            )?;
            enqueued = enqueued.saturating_add(1);
        }

        if enqueued > 0 {
            append_delta_event(
                &mut txn,
                self.dbs,
                Some(repo_id),
                format!("enrichment_enqueue count={enqueued}"),
            )?;
        }

        txn.commit()?;
        Ok(enqueued)
    }

    pub fn enqueue_commits_for_file_change_extraction(
        &self,
        repo_id: i64,
        commit_shas: &[String],
    ) -> AppResult<usize> {
        if commit_shas.is_empty() {
            return Ok(0);
        }

        let mut txn = self.env.begin_rw_txn()?;
        let now_epoch = now_epoch_seconds();
        let mut enqueued = 0usize;
        let mut seen = HashSet::with_capacity(commit_shas.len());

        for commit_sha in commit_shas {
            if commit_sha.is_empty() || !seen.insert(commit_sha.as_str()) {
                continue;
            }

            let key = repository_extraction_commit_key(repo_id, commit_sha);
            if txn.get(self.dbs.extraction_queue, &key).is_ok() {
                continue;
            }

            let queue = ExtractionQueueCommitEntry {
                attempts: 0,
                last_error: None,
                enqueued_at_epoch: now_epoch,
            };
            let raw = serialize(&queue, "serialize extraction queue commit entry")?;
            txn.put(self.dbs.extraction_queue, &key, &raw, WriteFlags::empty())?;
            enqueued = enqueued.saturating_add(1);
        }

        if enqueued > 0 {
            append_delta_event(
                &mut txn,
                self.dbs,
                Some(repo_id),
                format!("file_change_extraction_enqueue commits={enqueued}"),
            )?;
        }
        txn.commit()?;
        Ok(enqueued)
    }

    pub fn load_pending_file_change_extraction_commits(
        &self,
        repo_id: i64,
        limit: usize,
    ) -> AppResult<Vec<String>> {
        let bounded_limit = limit.max(1);
        let now_epoch = now_epoch_seconds();
        let txn = self.env.begin_ro_txn()?;
        let prefix = repository_prefix(repo_id);
        let cursor = txn.open_ro_cursor(self.dbs.extraction_queue)?;
        let mut pending: Vec<(i64, String)> = Vec::new();

        for_each_prefixed_cursor(&cursor, prefix.as_bytes(), |key, value| {
            let commit_sha = parse_extraction_commit_sha_from_prefixed_key(key, prefix.as_bytes())?;
            let queue: ExtractionQueueCommitEntry = deserialize(
                value,
                "deserialize file-change extraction queue commit entry",
            )?;
            if queue.attempts >= FILE_CHANGE_EXTRACTION_MAX_ATTEMPTS
                || queue.enqueued_at_epoch > now_epoch
            {
                return Ok(true);
            }
            pending.push((queue.enqueued_at_epoch, commit_sha));
            Ok(true)
        })?;

        pending.sort_unstable_by(|left, right| {
            left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1))
        });
        Ok(pending
            .into_iter()
            .take(bounded_limit)
            .map(|(_epoch, sha)| sha)
            .collect())
    }

    pub fn remove_file_change_extraction_commits(
        &self,
        repo_id: i64,
        commit_shas: &[String],
    ) -> AppResult<usize> {
        if commit_shas.is_empty() {
            return Ok(0);
        }

        let mut txn = self.env.begin_rw_txn()?;
        let mut removed = 0usize;
        let mut seen = HashSet::with_capacity(commit_shas.len());
        for commit_sha in commit_shas {
            if commit_sha.is_empty() || !seen.insert(commit_sha.as_str()) {
                continue;
            }
            let key = repository_extraction_commit_key(repo_id, commit_sha);
            match txn.del(self.dbs.extraction_queue, &key, None) {
                Ok(()) => removed = removed.saturating_add(1),
                Err(lmdb::Error::NotFound) => {}
                Err(error) => return Err(error.into()),
            }
        }

        if removed > 0 {
            append_delta_event(
                &mut txn,
                self.dbs,
                Some(repo_id),
                format!("file_change_extraction_dequeue commits={removed}"),
            )?;
        }
        txn.commit()?;
        Ok(removed)
    }

    pub fn mark_file_change_extraction_commits_failed(
        &self,
        repo_id: i64,
        commit_shas: &[String],
        error_message: &str,
    ) -> AppResult<usize> {
        if commit_shas.is_empty() {
            return Ok(0);
        }

        let mut txn = self.env.begin_rw_txn()?;
        let mut terminal_failed = 0usize;
        let mut retry_scheduled = 0usize;
        let mut seen = HashSet::with_capacity(commit_shas.len());

        for commit_sha in commit_shas {
            if commit_sha.is_empty() || !seen.insert(commit_sha.as_str()) {
                continue;
            }

            let key = repository_extraction_commit_key(repo_id, commit_sha);
            let queue = match txn.get(self.dbs.extraction_queue, &key) {
                Ok(raw) => deserialize::<ExtractionQueueCommitEntry>(
                    raw,
                    "deserialize extraction queue entry for failure update",
                )?,
                Err(lmdb::Error::NotFound) => continue,
                Err(error) => return Err(error.into()),
            };

            let next_attempt = queue.attempts.saturating_add(1);
            if next_attempt >= FILE_CHANGE_EXTRACTION_MAX_ATTEMPTS {
                let _ = txn.del(self.dbs.extraction_queue, &key, None);
                terminal_failed = terminal_failed.saturating_add(1);
                continue;
            }

            let updated = ExtractionQueueCommitEntry {
                attempts: next_attempt,
                last_error: Some(error_message.to_string()),
                enqueued_at_epoch: now_epoch_seconds()
                    .saturating_add(extraction_backoff_delay_seconds(next_attempt)),
            };
            let raw = serialize(&updated, "serialize extraction retry queue entry")?;
            txn.put(self.dbs.extraction_queue, &key, &raw, WriteFlags::empty())?;
            retry_scheduled = retry_scheduled.saturating_add(1);
        }

        if terminal_failed > 0 || retry_scheduled > 0 {
            append_delta_event(
                &mut txn,
                self.dbs,
                Some(repo_id),
                format!(
                    "file_change_extraction_failure terminal_failed={terminal_failed} retry_scheduled={retry_scheduled}"
                ),
            )?;
        }
        txn.commit()?;
        Ok(terminal_failed)
    }

    pub fn load_pending_file_change_enrichment(
        &self,
        repo_id: i64,
        limit: usize,
    ) -> AppResult<Vec<PendingFileChangeEnrichment>> {
        self.load_pending_file_change_enrichment_internal(repo_id, limit, None)
    }

    pub fn load_pending_file_change_enrichment_for_commits(
        &self,
        repo_id: i64,
        commit_shas: &[String],
        limit: usize,
    ) -> AppResult<Vec<PendingFileChangeEnrichment>> {
        if commit_shas.is_empty() {
            return Ok(Vec::new());
        }
        let filter: HashSet<String> = commit_shas.iter().cloned().collect();
        self.load_pending_file_change_enrichment_internal(repo_id, limit, Some(&filter))
    }

    fn load_pending_file_change_enrichment_internal(
        &self,
        repo_id: i64,
        limit: usize,
        commit_filter: Option<&HashSet<String>>,
    ) -> AppResult<Vec<PendingFileChangeEnrichment>> {
        let bounded_limit = limit.max(1);
        let now_epoch = now_epoch_seconds();
        let txn = self.env.begin_ro_txn()?;
        let prefix = repository_prefix(repo_id);
        let cursor = txn.open_ro_cursor(self.dbs.enrichment_due_index)?;
        let mut pending = Vec::with_capacity(bounded_limit);

        for_each_prefixed_cursor(&cursor, prefix.as_bytes(), |key, _value| {
            let (due_epoch, commit_sha, file_path) =
                parse_due_index_identity_from_prefixed_key(key, prefix.as_bytes())?;
            if due_epoch > now_epoch {
                return Ok(false);
            }
            if commit_filter
                .as_ref()
                .is_some_and(|set| !set.contains(commit_sha.as_str()))
            {
                return Ok(true);
            }

            let identity_key =
                repository_file_change_key(repo_id, commit_sha.as_str(), file_path.as_str());
            let queue = match txn.get(self.dbs.enrichment_queue, &identity_key) {
                Ok(raw) => deserialize::<EnrichmentQueueEntry>(
                    raw,
                    "deserialize enrichment queue entry from due index lookup",
                )?,
                Err(lmdb::Error::NotFound) => return Ok(true),
                Err(error) => return Err(error.into()),
            };
            if queue.attempts >= FILE_CHANGE_ENRICHMENT_MAX_ATTEMPTS
                || queue.enqueued_at_epoch > now_epoch
                || queue.enqueued_at_epoch != due_epoch
            {
                return Ok(true);
            }

            pending.push(PendingFileChangeEnrichment {
                commit_sha,
                file_path,
            });

            Ok(pending.len() < bounded_limit)
        })?;

        Ok(pending)
    }

    pub fn mark_file_change_enrichment_failed(
        &self,
        repo_id: i64,
        pending: &[PendingFileChangeEnrichment],
        error_message: &str,
    ) -> AppResult<()> {
        if pending.is_empty() {
            return Ok(());
        }

        let mut txn = self.env.begin_rw_txn()?;
        let mut terminal_failed = 0usize;
        let mut retry_scheduled = 0usize;

        for item in pending {
            let key = repository_file_change_key(
                repo_id,
                item.commit_sha.as_str(),
                item.file_path.as_str(),
            );
            let queue = match txn.get(self.dbs.enrichment_queue, &key) {
                Ok(raw) => deserialize::<EnrichmentQueueEntry>(
                    raw,
                    "deserialize enrichment queue entry for failure update",
                )?,
                Err(lmdb::Error::NotFound) => continue,
                Err(error) => return Err(error.into()),
            };
            let previous_due_key = enrichment_due_index_key(
                repo_id,
                queue.enqueued_at_epoch,
                item.commit_sha.as_str(),
                item.file_path.as_str(),
            );
            let _ = txn.del(self.dbs.enrichment_due_index, &previous_due_key, None);

            let next_attempt = queue.attempts.saturating_add(1);
            let terminal = next_attempt >= FILE_CHANGE_ENRICHMENT_MAX_ATTEMPTS;
            if terminal {
                let _ = txn.del(self.dbs.enrichment_queue, &key, None);
                update_file_change_stats_state(
                    &mut txn,
                    self.dbs.file_changes,
                    key.as_bytes(),
                    FileChangeStatsState::Failed,
                )?;
                terminal_failed = terminal_failed.saturating_add(1);
            } else {
                let delay_seconds = backoff_delay_seconds(next_attempt);
                let updated = EnrichmentQueueEntry {
                    attempts: next_attempt,
                    last_error: Some(error_message.to_string()),
                    enqueued_at_epoch: now_epoch_seconds().saturating_add(delay_seconds),
                };
                let raw = serialize(&updated, "serialize enrichment retry queue entry")?;
                txn.put(self.dbs.enrichment_queue, &key, &raw, WriteFlags::empty())?;
                let due_key = enrichment_due_index_key(
                    repo_id,
                    updated.enqueued_at_epoch,
                    item.commit_sha.as_str(),
                    item.file_path.as_str(),
                );
                txn.put(
                    self.dbs.enrichment_due_index,
                    &due_key,
                    b"",
                    WriteFlags::empty(),
                )?;
                update_file_change_stats_state(
                    &mut txn,
                    self.dbs.file_changes,
                    key.as_bytes(),
                    FileChangeStatsState::Pending,
                )?;
                retry_scheduled = retry_scheduled.saturating_add(1);
            }
        }

        append_delta_event(
            &mut txn,
            self.dbs,
            Some(repo_id),
            format!(
                "enrichment_failure terminal_failed={terminal_failed} retry_scheduled={retry_scheduled}"
            ),
        )?;
        txn.commit()?;
        Ok(())
    }

    pub fn apply_file_change_line_stats_updates(
        &self,
        repo_id: i64,
        updates: &[FileChangeLineStatsUpdate],
    ) -> AppResult<usize> {
        if updates.is_empty() {
            return Ok(0);
        }

        let mut txn = self.env.begin_rw_txn()?;
        let mut applied = 0usize;
        for update in updates {
            let key = repository_file_change_key(
                repo_id,
                update.commit_sha.as_str(),
                update.file_path.as_str(),
            );
            let Some(mut record) =
                load_file_change_record(&txn, self.dbs.file_changes, key.as_bytes())?
            else {
                continue;
            };
            record.additions = Some(update.additions);
            record.deletions = Some(update.deletions);
            record.stats_state = FileChangeStatsState::Ready;
            record.requires_line_stats = false;
            let raw = serialize(&record, "serialize updated file-change line stats")?;
            txn.put(self.dbs.file_changes, &key, &raw, WriteFlags::empty())?;
            if let Ok(raw_queue) = txn.get(self.dbs.enrichment_queue, &key) {
                let queue: EnrichmentQueueEntry = deserialize(
                    raw_queue,
                    "deserialize enrichment queue entry before enrichment apply deletion",
                )?;
                let due_key = enrichment_due_index_key(
                    repo_id,
                    queue.enqueued_at_epoch,
                    update.commit_sha.as_str(),
                    update.file_path.as_str(),
                );
                let _ = txn.del(self.dbs.enrichment_due_index, &due_key, None);
            }
            let _ = txn.del(self.dbs.enrichment_queue, &key, None);
            applied = applied.saturating_add(1);
        }

        if applied > 0 {
            append_delta_event(
                &mut txn,
                self.dbs,
                Some(repo_id),
                format!("enrichment_apply count={applied}"),
            )?;
        }

        txn.commit()?;
        Ok(applied)
    }

    pub fn upsert_index_checkpoint(
        &self,
        repo_id: i64,
        checkpoint: &IndexCheckpoint,
    ) -> AppResult<()> {
        let mut txn = self.env.begin_rw_txn()?;
        let key = repo_id.to_be_bytes();
        let raw = serialize(checkpoint, "serialize index checkpoint")?;
        txn.put(self.dbs.checkpoints, &key, &raw, WriteFlags::empty())?;
        append_delta_event(
            &mut txn,
            self.dbs,
            Some(repo_id),
            "index_checkpoint_upsert".to_string(),
        )?;
        txn.commit()?;
        Ok(())
    }

    pub fn load_index_checkpoint(&self, repo_id: i64) -> AppResult<Option<IndexCheckpoint>> {
        let txn = self.env.begin_ro_txn()?;
        let key = repo_id.to_be_bytes();
        match txn.get(self.dbs.checkpoints, &key) {
            Ok(raw) => Ok(Some(deserialize(raw, "deserialize index checkpoint")?)),
            Err(lmdb::Error::NotFound) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn set_repository_indexed_at(&self, repo_id: i64, indexed_at_epoch: i64) -> AppResult<()> {
        let mut txn = self.env.begin_rw_txn()?;
        let Some(mut repository) = self.load_repository_record_from_rw(&txn, repo_id)? else {
            return Err(AppError::Parse(format!(
                "cannot set indexed_at for missing repository id {repo_id}"
            )));
        };

        repository.indexed_at_epoch = Some(indexed_at_epoch);
        let key = repo_id.to_be_bytes();
        let raw = serialize(&repository, "serialize repository indexed_at update")?;
        txn.put(self.dbs.repositories, &key, &raw, WriteFlags::empty())?;
        txn.commit()?;
        Ok(())
    }

    pub fn load_repository_indexed_at(&self, repo_id: i64) -> AppResult<Option<String>> {
        let txn = self.env.begin_ro_txn()?;
        let key = repo_id.to_be_bytes();
        match txn.get(self.dbs.repositories, &key) {
            Ok(raw) => {
                let repository: RepositoryRecord =
                    deserialize(raw, "deserialize repository indexed_at")?;
                Ok(repository.indexed_at_epoch.map(|value| value.to_string()))
            }
            Err(lmdb::Error::NotFound) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn load_commit_window(
        &self,
        repo_id: i64,
        limit: usize,
        cursor: Option<&CommitWindowCursor>,
    ) -> AppResult<Vec<CommitWindowItem>> {
        let bounded_limit = limit.max(1);
        let txn = self.env.begin_ro_txn()?;
        let prefix = repository_prefix(repo_id);
        let cursor_db = txn.open_ro_cursor(self.dbs.commit_window_index)?;
        let start_key = cursor
            .map(|value| commit_window_index_key(repo_id, value.committer_time, value.sha.as_str()))
            .transpose()?;
        let mut skipped_cursor_row = false;
        let mut results = Vec::with_capacity(bounded_limit);

        for_each_prefixed_cursor_from(
            &cursor_db,
            prefix.as_bytes(),
            start_key.as_deref(),
            |key, value| {
                if start_key
                    .as_ref()
                    .is_some_and(|candidate| !skipped_cursor_row && key == candidate.as_slice())
                {
                    skipped_cursor_row = true;
                    return Ok(true);
                }

                let item: CommitWindowItem =
                    deserialize(value, "deserialize commit-window index item")?;
                results.push(item);
                Ok(results.len() < bounded_limit)
            },
        )?;

        if results.is_empty() {
            return self.load_commit_window_legacy(repo_id, bounded_limit, cursor);
        }

        Ok(results)
    }

    fn load_commit_window_legacy(
        &self,
        repo_id: i64,
        limit: usize,
        cursor: Option<&CommitWindowCursor>,
    ) -> AppResult<Vec<CommitWindowItem>> {
        let mut commits = self.load_all_commits(repo_id)?;
        commits.sort_unstable_by(|left, right| {
            right
                .committer_time
                .cmp(&left.committer_time)
                .then_with(|| right.sha.cmp(&left.sha))
        });

        let mut results = Vec::with_capacity(limit.min(commits.len()));
        for commit in commits {
            if cursor.as_ref().is_some_and(|cursor_value| {
                commit.committer_time > cursor_value.committer_time
                    || (commit.committer_time == cursor_value.committer_time
                        && commit.sha >= cursor_value.sha)
            }) {
                continue;
            }

            results.push(CommitWindowItem {
                sha: commit.sha,
                author_name: commit.author_name,
                author_email: commit.author_email,
                committer_time: commit.committer_time,
                subject: commit.subject,
                tree_sha: commit.tree_sha,
            });
            if results.len() >= limit {
                break;
            }
        }

        Ok(results)
    }

    pub fn load_all_commits(&self, repo_id: i64) -> AppResult<Vec<CommitRecord>> {
        let txn = self.env.begin_ro_txn()?;
        let prefix = repository_prefix(repo_id);
        let cursor = txn.open_ro_cursor(self.dbs.commits)?;
        let mut commits = Vec::new();
        for_each_prefixed_cursor(&cursor, prefix.as_bytes(), |_key, value| {
            let commit: CommitRecord = deserialize(value, "deserialize commit record")?;
            commits.push(commit);
            Ok(true)
        })?;
        Ok(commits)
    }

    pub fn initialize_empty_csr_snapshot(&self, repo_id: i64) -> AppResult<()> {
        let snapshot = CsrSnapshot {
            generated_at_epoch: now_epoch_seconds(),
            shas: Vec::new(),
            parents_offsets: vec![0],
            parents_neighbors: Vec::new(),
            children_offsets: vec![0],
            children_neighbors: Vec::new(),
            directed_edges: 0,
        };

        let mut txn = self.env.begin_rw_txn()?;
        let metadata = self.persist_graph_snapshot_state(&mut txn, repo_id, &snapshot)?;
        append_delta_event(
            &mut txn,
            self.dbs,
            Some(repo_id),
            format!(
                "csr_snapshot_initialize_empty version={} nodes={} directed_edges={}",
                metadata.snapshot_version, metadata.node_count, metadata.directed_edges
            ),
        )?;
        txn.commit()?;
        Ok(())
    }

    pub fn rebuild_csr_snapshot(&self, repo_id: i64) -> AppResult<(usize, usize)> {
        let commits = self.load_all_commits(repo_id)?;
        let mut ordered = commits;
        ordered.sort_unstable_by(|left, right| {
            right
                .committer_time
                .cmp(&left.committer_time)
                .then_with(|| right.sha.cmp(&left.sha))
        });

        let node_count = ordered.len();
        let mut sha_to_index = HashMap::with_capacity(node_count);
        for (index, commit) in ordered.iter().enumerate() {
            sha_to_index.insert(commit.sha.clone(), index);
        }

        let mut parent_counts = vec![0usize; node_count];
        let mut child_counts = vec![0usize; node_count];
        let mut directed_edges = 0usize;

        for (child_index, commit) in ordered.iter().enumerate() {
            for parent_sha in &commit.parents {
                let Some(&parent_index) = sha_to_index.get(parent_sha) else {
                    continue;
                };
                parent_counts[child_index] = parent_counts[child_index].saturating_add(1);
                child_counts[parent_index] = child_counts[parent_index].saturating_add(1);
                directed_edges = directed_edges.saturating_add(1);
            }
        }

        let parents_offsets = build_offsets_from_counts(&parent_counts)?;
        let children_offsets = build_offsets_from_counts(&child_counts)?;

        let mut parents_neighbors = vec![0u32; directed_edges];
        let mut children_neighbors = vec![0u32; directed_edges];
        let mut parent_write_positions = offsets_to_write_positions(&parents_offsets)?;
        let mut child_write_positions = offsets_to_write_positions(&children_offsets)?;

        for (child_index, commit) in ordered.iter().enumerate() {
            let child_id = u32::try_from(child_index).map_err(|_| {
                AppError::Parse(
                    "csr snapshot child index exceeded u32 capacity during rebuild".to_string(),
                )
            })?;
            for parent_sha in &commit.parents {
                let Some(&parent_index) = sha_to_index.get(parent_sha) else {
                    continue;
                };
                let parent_id = u32::try_from(parent_index).map_err(|_| {
                    AppError::Parse(
                        "csr snapshot parent index exceeded u32 capacity during rebuild"
                            .to_string(),
                    )
                })?;

                let parent_slot = parent_write_positions[child_index];
                parents_neighbors[parent_slot] = parent_id;
                parent_write_positions[child_index] =
                    parent_write_positions[child_index].saturating_add(1);

                let child_slot = child_write_positions[parent_index];
                children_neighbors[child_slot] = child_id;
                child_write_positions[parent_index] =
                    child_write_positions[parent_index].saturating_add(1);
            }
        }

        let snapshot = CsrSnapshot {
            generated_at_epoch: now_epoch_seconds(),
            shas: ordered.into_iter().map(|commit| commit.sha).collect(),
            parents_offsets,
            parents_neighbors,
            children_offsets,
            children_neighbors,
            directed_edges,
        };

        let mut txn = self.env.begin_rw_txn()?;
        let metadata = self.persist_graph_snapshot_state(&mut txn, repo_id, &snapshot)?;

        append_delta_event(
            &mut txn,
            self.dbs,
            Some(repo_id),
            format!(
                "csr_snapshot_rebuild version={} nodes={} directed_edges={}",
                metadata.snapshot_version, metadata.node_count, metadata.directed_edges
            ),
        )?;
        txn.commit()?;

        Ok((snapshot.node_count(), snapshot.directed_edges))
    }

    pub fn apply_csr_delta_from_commit_shas(
        &self,
        repo_id: i64,
        commit_shas: &[String],
    ) -> AppResult<Option<(usize, usize)>> {
        let Some(existing_snapshot) = self.load_csr_snapshot(repo_id)? else {
            return Ok(None);
        };

        if commit_shas.is_empty() {
            return Ok(Some((
                existing_snapshot.node_count(),
                existing_snapshot.directed_edges,
            )));
        }

        let txn = self.env.begin_ro_txn()?;
        let mut unique_shas = HashSet::new();
        let mut delta_commits = Vec::with_capacity(commit_shas.len());
        for sha in commit_shas {
            if !unique_shas.insert(sha.as_str()) {
                continue;
            }
            let key = repository_commit_key(repo_id, sha.as_str());
            let commit = match txn.get(self.dbs.commits, &key) {
                Ok(raw) => deserialize::<CommitRecord>(raw, "deserialize commit for csr delta")?,
                Err(lmdb::Error::NotFound) => continue,
                Err(error) => return Err(error.into()),
            };
            delta_commits.push(commit);
        }

        let (next_snapshot, changed) =
            build_incremental_csr_snapshot(existing_snapshot, delta_commits)?;
        if !changed {
            return Ok(Some((
                next_snapshot.node_count(),
                next_snapshot.directed_edges,
            )));
        }

        let mut write_txn = self.env.begin_rw_txn()?;
        let metadata =
            self.persist_graph_snapshot_state(&mut write_txn, repo_id, &next_snapshot)?;
        append_delta_event(
            &mut write_txn,
            self.dbs,
            Some(repo_id),
            format!(
                "csr_snapshot_incremental version={} nodes={} directed_edges={}",
                metadata.snapshot_version, metadata.node_count, metadata.directed_edges
            ),
        )?;
        write_txn.commit()?;

        Ok(Some((
            next_snapshot.node_count(),
            next_snapshot.directed_edges,
        )))
    }

    pub fn persist_graph_overlay_commits(
        &self,
        repo_id: i64,
        commit_shas: &[String],
    ) -> AppResult<GraphSnapshotMetadata> {
        let mut txn = self.env.begin_rw_txn()?;
        let mut unique_shas = HashSet::new();
        let mut overlay_commits_added = 0usize;
        let mut overlay_edges_added = 0usize;
        let mut pending_child_links: HashMap<String, Vec<String>> = HashMap::new();

        for sha in commit_shas {
            if !unique_shas.insert(sha.as_str()) {
                continue;
            }

            let overlay_key = repository_graph_overlay_parent_key(repo_id, sha.as_str());
            if txn
                .get(self.dbs.graph_overlay_parent_rows, &overlay_key)
                .is_ok()
            {
                continue;
            }

            let commit_key = repository_commit_key(repo_id, sha.as_str());
            let commit = match txn.get(self.dbs.commits, &commit_key) {
                Ok(raw) => deserialize::<CommitRecord>(
                    raw,
                    "deserialize commit record for graph overlay persistence",
                )?,
                Err(lmdb::Error::NotFound) => continue,
                Err(error) => return Err(error.into()),
            };

            let overlay_row = GraphOverlayParentRow {
                child_sha: commit.sha.clone(),
                parent_shas: commit.parents.clone(),
                committer_time: commit.committer_time,
            };
            let overlay_raw = serialize(&overlay_row, "serialize graph overlay parent row record")?;
            txn.put(
                self.dbs.graph_overlay_parent_rows,
                &overlay_key,
                &overlay_raw,
                WriteFlags::empty(),
            )?;
            overlay_commits_added = overlay_commits_added.saturating_add(1);
            overlay_edges_added = overlay_edges_added.saturating_add(commit.parents.len());
            for parent_sha in &overlay_row.parent_shas {
                pending_child_links
                    .entry(parent_sha.clone())
                    .or_default()
                    .push(overlay_row.child_sha.clone());
            }
        }

        for (parent_sha, mut child_shas) in pending_child_links {
            child_shas.sort_unstable();
            child_shas.dedup();
            self.merge_graph_overlay_child_row_in_txn(
                &mut txn,
                repo_id,
                parent_sha.as_str(),
                &child_shas,
            )?;
        }

        let mut metadata = self
            .load_graph_snapshot_metadata_from_txn(&txn, repo_id)?
            .ok_or_else(|| {
                AppError::InvalidArgument(
                    "cannot persist graph overlay commits without existing graph snapshot metadata"
                        .to_string(),
                )
            })?;
        metadata.pending_overlay_commits = metadata
            .pending_overlay_commits
            .saturating_add(overlay_commits_added);
        metadata.pending_overlay_edges = metadata
            .pending_overlay_edges
            .saturating_add(overlay_edges_added);
        self.store_graph_snapshot_metadata_in_txn(&mut txn, repo_id, &metadata)?;

        if overlay_commits_added > 0 {
            append_delta_event(
                &mut txn,
                self.dbs,
                Some(repo_id),
                format!(
                    "graph_overlay_upsert commits={} edges={}",
                    overlay_commits_added, overlay_edges_added
                ),
            )?;
        }

        txn.commit()?;
        Ok(metadata)
    }

    pub fn load_graph_overlay_parent_rows(
        &self,
        repo_id: i64,
    ) -> AppResult<Vec<GraphOverlayParentRow>> {
        let txn = self.env.begin_ro_txn()?;
        let prefix = repository_prefix(repo_id);
        let cursor = txn.open_ro_cursor(self.dbs.graph_overlay_parent_rows)?;
        let mut rows = Vec::new();
        for_each_prefixed_cursor(&cursor, prefix.as_bytes(), |_key, value| {
            let row: GraphOverlayParentRow =
                deserialize(value, "deserialize graph overlay parent row record")?;
            rows.push(row);
            Ok(true)
        })?;
        Ok(rows)
    }

    pub fn load_graph_overlay_child_rows(
        &self,
        repo_id: i64,
    ) -> AppResult<HashMap<String, Vec<String>>> {
        let txn = self.env.begin_ro_txn()?;
        let prefix = repository_prefix(repo_id);
        let cursor = txn.open_ro_cursor(self.dbs.graph_overlay_child_rows)?;
        let mut rows = HashMap::new();
        for_each_prefixed_cursor(&cursor, prefix.as_bytes(), |_key, value| {
            let row: GraphOverlayChildRow =
                deserialize(value, "deserialize graph overlay child row record")?;
            rows.insert(row.parent_sha, row.child_shas);
            Ok(true)
        })?;
        Ok(rows)
    }

    pub fn clear_graph_overlay(&self, repo_id: i64) -> AppResult<GraphSnapshotMetadata> {
        let mut txn = self.env.begin_rw_txn()?;
        let prefix = repository_prefix(repo_id);
        let deleted_parent_rows = delete_prefixed_entries(
            &mut txn,
            self.dbs.graph_overlay_parent_rows,
            prefix.as_str(),
        )?;
        let deleted_child_rows =
            delete_prefixed_entries(&mut txn, self.dbs.graph_overlay_child_rows, prefix.as_str())?;
        let mut metadata = self
            .load_graph_snapshot_metadata_from_txn(&txn, repo_id)?
            .ok_or_else(|| {
                AppError::InvalidArgument(
                    "cannot clear graph overlay without existing graph snapshot metadata"
                        .to_string(),
                )
            })?;
        metadata.pending_overlay_commits = 0;
        metadata.pending_overlay_edges = 0;
        self.store_graph_snapshot_metadata_in_txn(&mut txn, repo_id, &metadata)?;

        if deleted_parent_rows > 0 || deleted_child_rows > 0 {
            append_delta_event(
                &mut txn,
                self.dbs,
                Some(repo_id),
                format!(
                    "graph_overlay_clear parent_rows={deleted_parent_rows} child_rows={deleted_child_rows}"
                ),
            )?;
        }

        txn.commit()?;
        Ok(metadata)
    }

    pub fn compact_graph_overlay_into_snapshot(
        &self,
        repo_id: i64,
    ) -> AppResult<Option<(usize, usize)>> {
        let overlay_parent_rows = self.load_graph_overlay_parent_rows(repo_id)?;
        if overlay_parent_rows.is_empty() {
            let snapshot = self.load_csr_snapshot(repo_id)?;
            return Ok(snapshot.map(|value| (value.node_count(), value.directed_edges)));
        }

        let overlay_shas: Vec<String> = overlay_parent_rows
            .into_iter()
            .map(|row| row.child_sha)
            .collect();
        let compacted = self.apply_csr_delta_from_commit_shas(repo_id, &overlay_shas)?;
        let _ = self.clear_graph_overlay(repo_id)?;
        Ok(compacted)
    }

    pub fn load_csr_snapshot(&self, repo_id: i64) -> AppResult<Option<CsrSnapshot>> {
        let txn = self.env.begin_ro_txn()?;
        let key = repo_id.to_be_bytes();
        match txn.get(self.dbs.csr_snapshots, &key) {
            Ok(raw) => Ok(Some(deserialize(raw, "deserialize csr snapshot")?)),
            Err(lmdb::Error::NotFound) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn load_graph_snapshot_metadata(
        &self,
        repo_id: i64,
    ) -> AppResult<Option<GraphSnapshotMetadata>> {
        let txn = self.env.begin_ro_txn()?;
        let key = repo_id.to_be_bytes();
        match txn.get(self.dbs.graph_snapshot_metadata, &key) {
            Ok(raw) => Ok(Some(deserialize(
                raw,
                "deserialize graph snapshot metadata",
            )?)),
            Err(lmdb::Error::NotFound) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn load_graph_snapshot_state(&self, repo_id: i64) -> AppResult<Option<GraphSnapshotState>> {
        let Some(snapshot) = self.load_csr_snapshot(repo_id)? else {
            return Ok(None);
        };
        let metadata = self
            .load_graph_snapshot_metadata(repo_id)?
            .unwrap_or_else(|| build_graph_snapshot_metadata(1, &snapshot));
        let overlay_parent_rows = self.load_graph_overlay_parent_rows(repo_id)?;
        let overlay_child_rows = self.load_graph_overlay_child_rows(repo_id)?;
        Ok(Some(GraphSnapshotState {
            metadata,
            snapshot,
            overlay_parent_rows,
            overlay_child_rows,
        }))
    }

    pub fn count_commits(&self, repo_id: i64) -> AppResult<usize> {
        let txn = self.env.begin_ro_txn()?;
        let prefix = repository_prefix(repo_id);
        count_prefixed_entries(&txn, self.dbs.commits, prefix.as_str())
    }

    pub fn load_dataset_counts(&self, repo_id: i64) -> AppResult<(i64, i64, i64)> {
        let commit_count = self.count_commits(repo_id)?;

        let directed_edges = self
            .load_graph_snapshot_metadata(repo_id)?
            .map(|metadata| {
                metadata
                    .directed_edges
                    .saturating_add(metadata.pending_overlay_edges)
            })
            .or_else(|| {
                self.load_csr_snapshot(repo_id)
                    .ok()
                    .flatten()
                    .map(|snapshot| snapshot.directed_edges)
            })
            .unwrap_or(0);

        let txn = self.env.begin_ro_txn()?;
        let prefix = repository_prefix(repo_id);
        let file_change_count =
            count_prefixed_entries(&txn, self.dbs.file_changes, prefix.as_str())?;

        Ok((
            i64::try_from(commit_count).unwrap_or(i64::MAX),
            i64::try_from(directed_edges).unwrap_or(i64::MAX),
            i64::try_from(file_change_count).unwrap_or(i64::MAX),
        ))
    }

    pub fn load_file_change_stats_state_counts(
        &self,
        repo_id: i64,
    ) -> AppResult<FileChangeStatsStateCounts> {
        let txn = self.env.begin_ro_txn()?;
        let prefix = repository_prefix(repo_id);
        let cursor = txn.open_ro_cursor(self.dbs.file_changes)?;
        let mut counts = FileChangeStatsStateCounts::default();

        for_each_prefixed_cursor(&cursor, prefix.as_bytes(), |_key, value| {
            let record: FileChangeRecord =
                deserialize(value, "deserialize file-change for state counts")?;
            match record.stats_state {
                FileChangeStatsState::Pending => counts.pending = counts.pending.saturating_add(1),
                FileChangeStatsState::Ready => counts.ready = counts.ready.saturating_add(1),
                FileChangeStatsState::Failed => counts.failed = counts.failed.saturating_add(1),
            }
            Ok(true)
        })?;

        Ok(counts)
    }

    pub fn load_file_change_extraction_queue_metrics(
        &self,
        repo_id: i64,
    ) -> AppResult<FileChangeExtractionQueueMetrics> {
        let txn = self.env.begin_ro_txn()?;
        let prefix = repository_prefix(repo_id);
        let cursor = txn.open_ro_cursor(self.dbs.extraction_queue)?;
        let mut depth = 0usize;
        let mut oldest_epoch: Option<i64> = None;

        for_each_prefixed_cursor(&cursor, prefix.as_bytes(), |_key, value| {
            let queue: ExtractionQueueCommitEntry =
                deserialize(value, "deserialize extraction queue metrics")?;
            depth = depth.saturating_add(1);
            oldest_epoch = Some(oldest_epoch.map_or(queue.enqueued_at_epoch, |current| {
                current.min(queue.enqueued_at_epoch)
            }));
            Ok(true)
        })?;

        let age_seconds = oldest_epoch
            .map(|epoch| now_epoch_seconds().saturating_sub(epoch))
            .unwrap_or(0);

        Ok(FileChangeExtractionQueueMetrics {
            depth,
            oldest_queue_age_seconds: age_seconds,
        })
    }

    fn load_repository_record_from_rw(
        &self,
        txn: &RwTransaction<'_>,
        repo_id: i64,
    ) -> AppResult<Option<RepositoryRecord>> {
        let key = repo_id.to_be_bytes();
        match txn.get(self.dbs.repositories, &key) {
            Ok(raw) => Ok(Some(deserialize(raw, "deserialize repository record")?)),
            Err(lmdb::Error::NotFound) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn persist_graph_snapshot_state(
        &self,
        txn: &mut RwTransaction<'_>,
        repo_id: i64,
        snapshot: &CsrSnapshot,
    ) -> AppResult<GraphSnapshotMetadata> {
        let key = repo_id.to_be_bytes();
        let snapshot_raw = serialize(snapshot, "serialize graph csr snapshot")?;
        txn.put(
            self.dbs.csr_snapshots,
            &key,
            &snapshot_raw,
            WriteFlags::empty(),
        )?;

        let next_version = self
            .load_graph_snapshot_metadata_from_txn(txn, repo_id)?
            .map(|metadata| metadata.snapshot_version.saturating_add(1))
            .unwrap_or(1);

        let metadata = build_graph_snapshot_metadata(next_version, snapshot);
        self.store_graph_snapshot_metadata_in_txn(txn, repo_id, &metadata)?;
        Ok(metadata)
    }

    fn load_graph_snapshot_metadata_from_txn(
        &self,
        txn: &impl Transaction,
        repo_id: i64,
    ) -> AppResult<Option<GraphSnapshotMetadata>> {
        let key = repo_id.to_be_bytes();
        match txn.get(self.dbs.graph_snapshot_metadata, &key) {
            Ok(raw) => Ok(Some(deserialize(
                raw,
                "deserialize graph snapshot metadata from transaction",
            )?)),
            Err(lmdb::Error::NotFound) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn store_graph_snapshot_metadata_in_txn(
        &self,
        txn: &mut RwTransaction<'_>,
        repo_id: i64,
        metadata: &GraphSnapshotMetadata,
    ) -> AppResult<()> {
        let key = repo_id.to_be_bytes();
        let metadata_raw = serialize(metadata, "serialize graph snapshot metadata")?;
        txn.put(
            self.dbs.graph_snapshot_metadata,
            &key,
            &metadata_raw,
            WriteFlags::empty(),
        )?;
        Ok(())
    }

    fn merge_graph_overlay_child_row_in_txn(
        &self,
        txn: &mut RwTransaction<'_>,
        repo_id: i64,
        parent_sha: &str,
        new_child_shas: &[String],
    ) -> AppResult<()> {
        if new_child_shas.is_empty() {
            return Ok(());
        }

        let key = repository_graph_overlay_child_key(repo_id, parent_sha);
        let mut row = match txn.get(self.dbs.graph_overlay_child_rows, &key) {
            Ok(raw) => deserialize::<GraphOverlayChildRow>(
                raw,
                "deserialize graph overlay child row for merge",
            )?,
            Err(lmdb::Error::NotFound) => GraphOverlayChildRow {
                parent_sha: parent_sha.to_string(),
                child_shas: Vec::new(),
            },
            Err(error) => return Err(error.into()),
        };

        let mut changed = false;
        for child_sha in new_child_shas {
            if row.child_shas.iter().any(|existing| existing == child_sha) {
                continue;
            }
            row.child_shas.push(child_sha.clone());
            changed = true;
        }

        if changed {
            let raw = serialize(&row, "serialize graph overlay child row record")?;
            txn.put(
                self.dbs.graph_overlay_child_rows,
                &key,
                &raw,
                WriteFlags::empty(),
            )?;
        }

        Ok(())
    }
}

pub fn open_store(db_path: &Path) -> AppResult<GraphStore> {
    GraphStore::open(db_path)
}

fn load_file_change_record(
    txn: &RwTransaction<'_>,
    db: Database,
    key: &[u8],
) -> AppResult<Option<FileChangeRecord>> {
    match txn.get(db, &key) {
        Ok(raw) => Ok(Some(deserialize(raw, "deserialize file-change record")?)),
        Err(lmdb::Error::NotFound) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn update_file_change_stats_state(
    txn: &mut RwTransaction<'_>,
    db: Database,
    key: &[u8],
    state: FileChangeStatsState,
) -> AppResult<()> {
    let Some(mut record) = load_file_change_record(txn, db, key)? else {
        return Ok(());
    };
    record.stats_state = state;
    let raw = serialize(&record, "serialize file-change state transition")?;
    txn.put(db, &key, &raw, WriteFlags::empty())?;
    Ok(())
}

fn resolve_store_directory(db_path: &Path) -> PathBuf {
    if db_path.is_dir() {
        return db_path.to_path_buf();
    }

    if db_path.extension().is_some_and(|ext| !ext.is_empty())
        || db_path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().contains('.'))
    {
        return PathBuf::from(format!("{}.lmdb", db_path.to_string_lossy()));
    }

    db_path.to_path_buf()
}

fn resolve_map_size_bytes(store_dir: &Path) -> usize {
    let configured = env::var("GITGRAPH_LMDB_MAP_SIZE_BYTES")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value >= LMDB_MIN_MAP_SIZE_BYTES)
        .unwrap_or(LMDB_DEFAULT_MAP_SIZE_BYTES);
    effective_map_size_bytes(configured, current_lmdb_data_file_size_bytes(store_dir))
}

fn effective_map_size_bytes(configured_map_size: usize, observed_data_file_bytes: usize) -> usize {
    configured_map_size
        .max(observed_data_file_bytes)
        .max(LMDB_MIN_MAP_SIZE_BYTES)
}

fn current_lmdb_data_file_size_bytes(store_dir: &Path) -> usize {
    let data_file = store_dir.join("data.mdb");
    fs::metadata(data_file)
        .ok()
        .and_then(|metadata| usize::try_from(metadata.len()).ok())
        .unwrap_or(0)
}

fn resolve_lmdb_runtime_options() -> AppResult<LmdbRuntimeOptions> {
    LmdbRuntimeOptions::from_env()
}

fn resolve_delta_log_retention_events() -> u64 {
    env::var("GITGRAPH_DELTA_LOG_RETENTION_EVENTS")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(DELTA_LOG_RETENTION_DEFAULT_EVENTS)
}

pub fn graph_overlay_compaction_thresholds() -> (usize, usize) {
    let max_commits = env::var(GRAPH_OVERLAY_MAX_COMMITS_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(GRAPH_OVERLAY_MAX_COMMITS_DEFAULT);
    let max_edges = env::var(GRAPH_OVERLAY_MAX_EDGES_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(GRAPH_OVERLAY_MAX_EDGES_DEFAULT);
    (max_commits, max_edges)
}

pub fn should_compact_graph_overlay(metadata: &GraphSnapshotMetadata) -> bool {
    let (max_commits, max_edges) = graph_overlay_compaction_thresholds();
    metadata.pending_overlay_commits >= max_commits || metadata.pending_overlay_edges >= max_edges
}

fn build_graph_snapshot_metadata(
    snapshot_version: u64,
    snapshot: &CsrSnapshot,
) -> GraphSnapshotMetadata {
    GraphSnapshotMetadata {
        snapshot_version,
        generated_at_epoch: snapshot.generated_at_epoch,
        node_count: snapshot.node_count(),
        directed_edges: snapshot.directed_edges,
        pending_overlay_commits: 0,
        pending_overlay_edges: 0,
    }
}

fn parse_env_bool_enabled(key: &str) -> bool {
    env::var(key)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn repository_prefix(repo_id: i64) -> String {
    format!("{repo_id:0REPO_KEY_WIDTH$}:")
}

fn repository_commit_key(repo_id: i64, sha: &str) -> String {
    format!("{}{}", repository_prefix(repo_id), sha)
}

fn repository_ref_key(repo_id: i64, ref_name: &str) -> String {
    format!("{}{}", repository_prefix(repo_id), ref_name)
}

fn repository_commit_file_change_prefix(repo_id: i64, commit_sha: &str) -> String {
    format!("{}{}:", repository_prefix(repo_id), commit_sha)
}

fn repository_graph_overlay_parent_key(repo_id: i64, sha: &str) -> String {
    format!("{}{}", repository_prefix(repo_id), sha)
}

fn repository_graph_overlay_child_key(repo_id: i64, sha: &str) -> String {
    format!("{}{}", repository_prefix(repo_id), sha)
}

fn repository_file_change_key(repo_id: i64, commit_sha: &str, file_path: &str) -> String {
    format!("{}{}:{file_path}", repository_prefix(repo_id), commit_sha)
}

fn repository_extraction_commit_key(repo_id: i64, commit_sha: &str) -> String {
    format!("{}{}", repository_prefix(repo_id), commit_sha)
}

fn commit_window_index_key(repo_id: i64, committer_time: i64, sha: &str) -> AppResult<Vec<u8>> {
    let prefix = repository_prefix(repo_id);
    let desc_time_component = descending_time_component(committer_time);
    let mut key =
        Vec::with_capacity(prefix.len() + COMMIT_WINDOW_TIME_COMPONENT_WIDTH + 1 + sha.len());
    key.extend_from_slice(prefix.as_bytes());
    key.extend_from_slice(
        format!(
            "{desc_time_component:0COMMIT_WINDOW_TIME_COMPONENT_WIDTH$}",
            COMMIT_WINDOW_TIME_COMPONENT_WIDTH = COMMIT_WINDOW_TIME_COMPONENT_WIDTH
        )
        .as_bytes(),
    );
    key.push(b':');
    key.extend(invert_sha_bytes(sha)?);
    Ok(key)
}

fn enrichment_due_index_key(
    repo_id: i64,
    due_epoch: i64,
    commit_sha: &str,
    file_path: &str,
) -> String {
    let due_component = normalize_epoch_for_index(due_epoch);
    format!(
        "{}{due_component:0DUE_TIME_COMPONENT_WIDTH$}:{commit_sha}:{file_path}",
        repository_prefix(repo_id),
        DUE_TIME_COMPONENT_WIDTH = DUE_TIME_COMPONENT_WIDTH
    )
}

fn parse_due_index_identity_from_prefixed_key(
    key: &[u8],
    prefix: &[u8],
) -> AppResult<(i64, String, String)> {
    let suffix = key.strip_prefix(prefix).ok_or_else(|| {
        AppError::Parse("enrichment due index key missing repository prefix".to_string())
    })?;
    let raw_suffix = std::str::from_utf8(suffix).map_err(|error| {
        AppError::Parse(format!(
            "decode enrichment due index key suffix as utf8 failed: {error}"
        ))
    })?;
    let mut parts = raw_suffix.splitn(3, ':');
    let due_raw = parts.next().ok_or_else(|| {
        AppError::Parse("enrichment due index key missing due-time segment".to_string())
    })?;
    let commit_sha = parts.next().ok_or_else(|| {
        AppError::Parse("enrichment due index key missing commit-sha segment".to_string())
    })?;
    let file_path = parts.next().ok_or_else(|| {
        AppError::Parse("enrichment due index key missing file-path segment".to_string())
    })?;
    let due_epoch_u64 = due_raw.parse::<u64>().map_err(|error| {
        AppError::Parse(format!(
            "parse due-time segment from enrichment due index key: {error}"
        ))
    })?;
    let due_epoch = i64::try_from(due_epoch_u64).unwrap_or(i64::MAX);
    Ok((due_epoch, commit_sha.to_string(), file_path.to_string()))
}

fn parse_extraction_commit_sha_from_prefixed_key(key: &[u8], prefix: &[u8]) -> AppResult<String> {
    let suffix = key.strip_prefix(prefix).ok_or_else(|| {
        AppError::Parse("file-change extraction queue key missing repository prefix".to_string())
    })?;
    if suffix.is_empty() {
        return Err(AppError::Parse(
            "file-change extraction queue key missing commit-sha segment".to_string(),
        ));
    }
    std::str::from_utf8(suffix)
        .map(|value| value.to_string())
        .map_err(|error| {
            AppError::Parse(format!(
                "decode file-change extraction queue key suffix as utf8 failed: {error}"
            ))
        })
}

fn normalize_epoch_for_index(epoch: i64) -> u64 {
    if epoch <= 0 {
        0
    } else {
        u64::try_from(epoch).unwrap_or(u64::MAX)
    }
}

fn descending_time_component(committer_time: i64) -> u64 {
    let monotonic = (committer_time as u64) ^ (1_u64 << 63);
    u64::MAX.saturating_sub(monotonic)
}

fn invert_sha_bytes(sha: &str) -> AppResult<Vec<u8>> {
    if sha.is_empty() {
        return Err(AppError::Parse(
            "cannot build commit-window index key for empty sha".to_string(),
        ));
    }
    Ok(sha.as_bytes().iter().map(|byte| u8::MAX - *byte).collect())
}

fn for_each_prefixed_cursor<'txn, C, F>(cursor: &C, prefix: &[u8], on_entry: F) -> AppResult<()>
where
    C: Cursor<'txn>,
    F: FnMut(&'txn [u8], &'txn [u8]) -> AppResult<bool>,
{
    for_each_prefixed_cursor_from(cursor, prefix, None, on_entry)
}

fn for_each_prefixed_cursor_from<'txn, C, F>(
    cursor: &C,
    prefix: &[u8],
    start_key: Option<&[u8]>,
    mut on_entry: F,
) -> AppResult<()>
where
    C: Cursor<'txn>,
    F: FnMut(&'txn [u8], &'txn [u8]) -> AppResult<bool>,
{
    let seek_key = start_key.unwrap_or(prefix);
    let mut current = match cursor.get(Some(seek_key), None, lmdb_sys::MDB_SET_RANGE) {
        Ok((Some(key), value)) if key.starts_with(prefix) => Some((key, value)),
        Ok((_maybe_key, _value)) => None,
        Err(lmdb::Error::NotFound) => None,
        Err(error) => return Err(error.into()),
    };

    while let Some((key, value)) = current {
        if !on_entry(key, value)? {
            break;
        }

        current = match cursor.get(None, None, lmdb_sys::MDB_NEXT) {
            Ok((Some(next_key), next_value)) if next_key.starts_with(prefix) => {
                Some((next_key, next_value))
            }
            Ok((_maybe_key, _value)) => None,
            Err(lmdb::Error::NotFound) => None,
            Err(error) => return Err(error.into()),
        };
    }

    Ok(())
}

fn delete_prefixed_entries(
    txn: &mut RwTransaction<'_>,
    db: Database,
    prefix: &str,
) -> AppResult<usize> {
    let prefix_bytes = prefix.as_bytes();
    let mut keys_to_delete = Vec::new();
    {
        let cursor = txn.open_rw_cursor(db)?;
        for_each_prefixed_cursor(&cursor, prefix_bytes, |key, _value| {
            keys_to_delete.push(key.to_vec());
            Ok(true)
        })?;
    }

    for key in &keys_to_delete {
        let _ = txn.del(db, key, None);
    }

    Ok(keys_to_delete.len())
}

fn delete_enrichment_due_entries_for_commits(
    txn: &mut RwTransaction<'_>,
    db: Database,
    repo_id: i64,
    commit_shas: &HashSet<&str>,
) -> AppResult<usize> {
    if commit_shas.is_empty() {
        return Ok(0);
    }

    let prefix = repository_prefix(repo_id);
    let prefix_bytes = prefix.as_bytes();
    let mut keys_to_delete = Vec::new();
    {
        let cursor = txn.open_rw_cursor(db)?;
        for_each_prefixed_cursor(&cursor, prefix_bytes, |key, _value| {
            let (_due_epoch, commit_sha, _file_path) =
                parse_due_index_identity_from_prefixed_key(key, prefix_bytes)?;
            if commit_shas.contains(commit_sha.as_str()) {
                keys_to_delete.push(key.to_vec());
            }
            Ok(true)
        })?;
    }

    for key in &keys_to_delete {
        let _ = txn.del(db, key, None);
    }

    Ok(keys_to_delete.len())
}

fn count_prefixed_entries(txn: &RoTransaction<'_>, db: Database, prefix: &str) -> AppResult<usize> {
    let mut count = 0usize;
    let cursor = txn.open_ro_cursor(db)?;
    for_each_prefixed_cursor(&cursor, prefix.as_bytes(), |_key, _value| {
        count = count.saturating_add(1);
        Ok(true)
    })?;
    Ok(count)
}

fn build_offsets_from_counts(counts: &[usize]) -> AppResult<Vec<u32>> {
    let mut offsets = Vec::with_capacity(counts.len().saturating_add(1));
    offsets.push(0);
    let mut running_total = 0usize;
    for &count in counts {
        running_total = running_total.checked_add(count).ok_or_else(|| {
            AppError::Parse("csr offset accumulation overflowed usize capacity".to_string())
        })?;
        let offset = u32::try_from(running_total).map_err(|_| {
            AppError::Parse("csr offset exceeded u32 capacity during rebuild".to_string())
        })?;
        offsets.push(offset);
    }
    Ok(offsets)
}

fn offsets_to_write_positions(offsets: &[u32]) -> AppResult<Vec<usize>> {
    if offsets.is_empty() {
        return Ok(Vec::new());
    }

    let mut positions = Vec::with_capacity(offsets.len().saturating_sub(1));
    for &offset in offsets.iter().take(offsets.len().saturating_sub(1)) {
        let index = usize::try_from(offset).map_err(|_| {
            AppError::Parse("csr write position exceeded usize capacity".to_string())
        })?;
        positions.push(index);
    }
    Ok(positions)
}

pub(crate) fn build_incremental_csr_snapshot(
    mut existing: CsrSnapshot,
    mut delta_commits: Vec<CommitRecord>,
) -> AppResult<(CsrSnapshot, bool)> {
    if delta_commits.is_empty() {
        return Ok((existing, false));
    }

    let old_node_count = existing.shas.len();
    let mut sha_to_index =
        HashMap::with_capacity(old_node_count.saturating_add(delta_commits.len()));
    for (index, sha) in existing.shas.iter().enumerate() {
        sha_to_index.insert(sha.clone(), index);
    }

    delta_commits.sort_unstable_by(|left, right| {
        right
            .committer_time
            .cmp(&left.committer_time)
            .then_with(|| right.sha.cmp(&left.sha))
    });

    let mut new_commits = Vec::new();
    for commit in delta_commits {
        if sha_to_index.contains_key(commit.sha.as_str()) {
            continue;
        }
        let new_index = old_node_count.saturating_add(new_commits.len());
        sha_to_index.insert(commit.sha.clone(), new_index);
        new_commits.push(commit);
    }
    if new_commits.is_empty() {
        return Ok((existing, false));
    }

    let total_node_count = old_node_count.saturating_add(new_commits.len());
    let old_parent_counts = csr_row_counts(&existing.parents_offsets, old_node_count)?;
    let old_child_counts = csr_row_counts(&existing.children_offsets, old_node_count)?;
    let mut parent_counts = old_parent_counts;
    parent_counts.resize(total_node_count, 0);
    let mut child_counts = old_child_counts;
    child_counts.resize(total_node_count, 0);

    let mut new_parent_lists: Vec<Vec<u32>> = vec![Vec::new(); new_commits.len()];
    let mut child_deltas: HashMap<usize, Vec<u32>> = HashMap::new();
    let mut new_edges = 0usize;

    for (offset, commit) in new_commits.iter().enumerate() {
        let child_index = old_node_count.saturating_add(offset);
        let child_id = u32::try_from(child_index).map_err(|_| {
            AppError::Parse(
                "csr delta child index exceeded u32 capacity during incremental apply".to_string(),
            )
        })?;

        let mut parent_ids = Vec::with_capacity(commit.parents.len());
        for parent_sha in &commit.parents {
            let Some(&parent_index) = sha_to_index.get(parent_sha) else {
                continue;
            };
            let parent_id = u32::try_from(parent_index).map_err(|_| {
                AppError::Parse(
                    "csr delta parent index exceeded u32 capacity during incremental apply"
                        .to_string(),
                )
            })?;
            parent_ids.push(parent_id);
        }
        parent_ids.sort_unstable_by(|left, right| right.cmp(left));
        parent_ids.dedup();

        new_edges = new_edges.saturating_add(parent_ids.len());
        parent_counts[child_index] = parent_ids.len();
        for &parent_id in &parent_ids {
            let parent_index = usize::try_from(parent_id).map_err(|_| {
                AppError::Parse(
                    "csr delta parent index exceeded usize capacity during incremental apply"
                        .to_string(),
                )
            })?;
            child_deltas.entry(parent_index).or_default().push(child_id);
        }
        new_parent_lists[offset] = parent_ids;
    }

    for (node_index, delta_list) in &mut child_deltas {
        delta_list.sort_unstable_by(|left, right| right.cmp(left));
        delta_list.dedup();
        child_counts[*node_index] = child_counts[*node_index].saturating_add(delta_list.len());
    }

    let parents_offsets = build_offsets_from_counts(&parent_counts)?;
    let children_offsets = build_offsets_from_counts(&child_counts)?;
    let total_parent_edges =
        usize::try_from(*parents_offsets.last().unwrap_or(&0)).map_err(|_| {
            AppError::Parse(
                "csr parent-edge count exceeded usize capacity during incremental apply"
                    .to_string(),
            )
        })?;
    let total_child_edges =
        usize::try_from(*children_offsets.last().unwrap_or(&0)).map_err(|_| {
            AppError::Parse(
                "csr child-edge count exceeded usize capacity during incremental apply".to_string(),
            )
        })?;

    let mut parents_neighbors = vec![0u32; total_parent_edges];
    let old_parent_len = existing.parents_neighbors.len();
    if old_parent_len > parents_neighbors.len() {
        return Err(AppError::Parse(
            "existing parent CSR exceeds incremental parent CSR capacity".to_string(),
        ));
    }
    parents_neighbors[..old_parent_len].copy_from_slice(&existing.parents_neighbors);
    for (offset, parent_list) in new_parent_lists.iter().enumerate() {
        let node_index = old_node_count.saturating_add(offset);
        let start = usize::try_from(parents_offsets[node_index]).map_err(|_| {
            AppError::Parse(
                "incremental parent offset exceeded usize capacity while writing neighbors"
                    .to_string(),
            )
        })?;
        let end = start.saturating_add(parent_list.len());
        if end > parents_neighbors.len() {
            return Err(AppError::Parse(
                "incremental parent write exceeded neighbor buffer".to_string(),
            ));
        }
        parents_neighbors[start..end].copy_from_slice(parent_list);
    }

    let mut children_neighbors = vec![0u32; total_child_edges];
    let mut child_deltas_owned = child_deltas;
    for node_index in 0..total_node_count {
        let mut write_cursor = usize::try_from(children_offsets[node_index]).map_err(|_| {
            AppError::Parse(
                "incremental child offset exceeded usize capacity while writing neighbors"
                    .to_string(),
            )
        })?;

        if node_index < old_node_count {
            let existing_slice = csr_slice(
                &existing.children_offsets,
                &existing.children_neighbors,
                node_index,
            );
            let end = write_cursor.saturating_add(existing_slice.len());
            if end > children_neighbors.len() {
                return Err(AppError::Parse(
                    "existing child CSR copy exceeded incremental child buffer".to_string(),
                ));
            }
            children_neighbors[write_cursor..end].copy_from_slice(existing_slice);
            write_cursor = end;
        }

        if let Some(delta_slice) = child_deltas_owned.remove(&node_index) {
            let end = write_cursor.saturating_add(delta_slice.len());
            if end > children_neighbors.len() {
                return Err(AppError::Parse(
                    "incremental child delta write exceeded neighbor buffer".to_string(),
                ));
            }
            children_neighbors[write_cursor..end].copy_from_slice(&delta_slice);
            write_cursor = end;
        }

        let expected_end = usize::try_from(children_offsets[node_index.saturating_add(1)])
            .map_err(|_| {
                AppError::Parse(
                    "incremental child offset exceeded usize capacity at row end".to_string(),
                )
            })?;
        if write_cursor != expected_end {
            return Err(AppError::Parse(
                "incremental child row write did not match expected CSR row width".to_string(),
            ));
        }
    }

    existing.generated_at_epoch = now_epoch_seconds();
    existing
        .shas
        .extend(new_commits.into_iter().map(|commit| commit.sha));
    existing.parents_offsets = parents_offsets;
    existing.parents_neighbors = parents_neighbors;
    existing.children_offsets = children_offsets;
    existing.children_neighbors = children_neighbors;
    existing.directed_edges = old_parent_len.saturating_add(new_edges);

    Ok((existing, true))
}

fn csr_row_counts(offsets: &[u32], node_count: usize) -> AppResult<Vec<usize>> {
    if offsets.len() < node_count.saturating_add(1) {
        return Err(AppError::Parse(
            "csr offsets length is smaller than node_count + 1".to_string(),
        ));
    }

    let mut counts = Vec::with_capacity(node_count);
    for node_index in 0..node_count {
        let start = usize::try_from(offsets[node_index]).map_err(|_| {
            AppError::Parse("csr row start offset exceeded usize capacity".to_string())
        })?;
        let end = usize::try_from(offsets[node_index.saturating_add(1)]).map_err(|_| {
            AppError::Parse("csr row end offset exceeded usize capacity".to_string())
        })?;
        if end < start {
            return Err(AppError::Parse(
                "csr row end offset is smaller than start offset".to_string(),
            ));
        }
        counts.push(end - start);
    }
    Ok(counts)
}

fn csr_slice<'a>(offsets: &[u32], neighbors: &'a [u32], node_id: usize) -> &'a [u32] {
    let Some(&start) = offsets.get(node_id) else {
        return &[];
    };
    let Some(&end) = offsets.get(node_id.saturating_add(1)) else {
        return &[];
    };

    let start_index = usize::try_from(start).ok();
    let end_index = usize::try_from(end).ok();
    match (start_index, end_index) {
        (Some(start_idx), Some(end_idx)) if start_idx <= end_idx && end_idx <= neighbors.len() => {
            &neighbors[start_idx..end_idx]
        }
        _ => &[],
    }
}

fn append_delta_event(
    txn: &mut RwTransaction<'_>,
    dbs: Databases,
    repo_id: Option<i64>,
    kind: String,
) -> AppResult<()> {
    let next_sequence = read_u64_meta(txn, dbs.metadata, META_NEXT_DELTA_SEQUENCE)?
        .unwrap_or(1)
        .max(1);
    write_u64_meta(
        txn,
        dbs.metadata,
        META_NEXT_DELTA_SEQUENCE,
        next_sequence.saturating_add(1),
    )?;

    let event = DeltaEvent {
        sequence: next_sequence,
        repo_id,
        at_epoch: now_epoch_seconds(),
        kind,
    };
    let key = next_sequence.to_be_bytes();
    let raw = serialize(&event, "serialize delta event")?;
    txn.put(dbs.delta_log, &key, &raw, WriteFlags::empty())?;

    let retention_limit = resolve_delta_log_retention_events();
    if retention_limit > 0 && next_sequence > retention_limit {
        let oldest_sequence_to_prune = next_sequence.saturating_sub(retention_limit);
        let oldest_key = oldest_sequence_to_prune.to_be_bytes();
        let _ = txn.del(dbs.delta_log, &oldest_key, None);
    }

    Ok(())
}

fn ensure_u64_meta(
    txn: &mut RwTransaction<'_>,
    db: Database,
    key: &str,
    default_value: u64,
) -> AppResult<()> {
    match txn.get(db, &key) {
        Ok(_) => Ok(()),
        Err(lmdb::Error::NotFound) => {
            txn.put(db, &key, &encode_u64(default_value), WriteFlags::empty())?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn read_u64_meta(txn: &impl Transaction, db: Database, key: &str) -> AppResult<Option<u64>> {
    match txn.get(db, &key) {
        Ok(raw) => Ok(Some(decode_u64(raw, "decode metadata u64 value")?)),
        Err(lmdb::Error::NotFound) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn write_u64_meta(
    txn: &mut RwTransaction<'_>,
    db: Database,
    key: &str,
    value: u64,
) -> AppResult<()> {
    txn.put(db, &key, &encode_u64(value), WriteFlags::empty())?;
    Ok(())
}

fn encode_u64(value: u64) -> [u8; 8] {
    value.to_be_bytes()
}

fn decode_u64(raw: &[u8], context: &str) -> AppResult<u64> {
    if raw.len() != 8 {
        return Err(AppError::Parse(format!(
            "{context}: expected 8-byte u64 payload, got {} bytes",
            raw.len()
        )));
    }
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(raw);
    Ok(u64::from_be_bytes(bytes))
}

fn backoff_delay_seconds(attempt: i64) -> i64 {
    let exponent = attempt.saturating_sub(1).clamp(0, 16);
    let factor = 1_i64
        .checked_shl(u32::try_from(exponent).unwrap_or(0))
        .unwrap_or(i64::MAX);
    FILE_CHANGE_ENRICHMENT_RETRY_BASE_DELAY_SECONDS
        .saturating_mul(factor)
        .clamp(
            FILE_CHANGE_ENRICHMENT_RETRY_BASE_DELAY_SECONDS,
            FILE_CHANGE_ENRICHMENT_RETRY_MAX_DELAY_SECONDS,
        )
}

fn extraction_backoff_delay_seconds(attempt: i64) -> i64 {
    let exponent = attempt.saturating_sub(1).clamp(0, 16);
    let factor = 1_i64
        .checked_shl(u32::try_from(exponent).unwrap_or(0))
        .unwrap_or(i64::MAX);
    FILE_CHANGE_EXTRACTION_RETRY_BASE_DELAY_SECONDS
        .saturating_mul(factor)
        .clamp(
            FILE_CHANGE_EXTRACTION_RETRY_BASE_DELAY_SECONDS,
            FILE_CHANGE_EXTRACTION_RETRY_MAX_DELAY_SECONDS,
        )
}

fn serialize<T: Serialize>(value: &T, context: &str) -> AppResult<Vec<u8>> {
    bincode::serialize(value).map_err(|error| AppError::Parse(format!("{context}: {error}")))
}

fn deserialize<T: DeserializeOwned>(raw: &[u8], context: &str) -> AppResult<T> {
    bincode::deserialize(raw).map_err(|error| AppError::Parse(format!("{context}: {error}")))
}

#[cfg(test)]
mod tests {
    use lmdb::EnvironmentFlags;
    use tempfile::TempDir;

    use super::{
        effective_map_size_bytes, open_store, LmdbRuntimeOptions,
        FILE_CHANGE_EXTRACTION_MAX_ATTEMPTS, LMDB_MIN_MAP_SIZE_BYTES,
    };

    #[test]
    fn lmdb_runtime_options_reject_map_async_without_write_map() {
        let result = LmdbRuntimeOptions::from_bools(false, false, true, false);
        assert!(result.is_err());
    }

    #[test]
    fn lmdb_runtime_options_reject_no_mem_init_with_write_map() {
        let result = LmdbRuntimeOptions::from_bools(false, true, false, true);
        assert!(result.is_err());
    }

    #[test]
    fn lmdb_runtime_options_translate_to_expected_environment_flags() {
        let options =
            LmdbRuntimeOptions::from_bools(true, true, true, false).expect("valid lmdb flags");
        let flags = options.environment_flags();

        assert!(flags.contains(EnvironmentFlags::WRITE_MAP));
        assert!(flags.contains(EnvironmentFlags::MAP_ASYNC));
        #[cfg(not(windows))]
        assert!(flags.contains(EnvironmentFlags::NO_READAHEAD));
        #[cfg(windows)]
        assert!(!flags.contains(EnvironmentFlags::NO_READAHEAD));
        assert!(!flags.contains(EnvironmentFlags::NO_MEM_INIT));
    }

    #[test]
    fn lmdb_runtime_options_allow_no_mem_init_without_write_map() {
        let options =
            LmdbRuntimeOptions::from_bools(false, false, false, true).expect("valid lmdb flags");
        let flags = options.environment_flags();

        assert!(flags.contains(EnvironmentFlags::NO_MEM_INIT));
        assert!(!flags.contains(EnvironmentFlags::WRITE_MAP));
    }

    #[test]
    fn effective_map_size_respects_existing_environment_floor() {
        let configured = LMDB_MIN_MAP_SIZE_BYTES;
        let observed = configured.saturating_mul(2);

        let effective = effective_map_size_bytes(configured, observed);

        assert_eq!(effective, observed);
    }

    #[test]
    fn effective_map_size_keeps_configured_budget_when_larger() {
        let configured = LMDB_MIN_MAP_SIZE_BYTES.saturating_mul(4);
        let observed = LMDB_MIN_MAP_SIZE_BYTES;

        let effective = effective_map_size_bytes(configured, observed);

        assert_eq!(effective, configured);
    }

    #[test]
    fn extraction_queue_enqueue_is_idempotent_and_ordered() {
        let temp = TempDir::new().unwrap_or_else(|error| panic!("create temp dir: {error}"));
        let db_path = temp.path().join("graph-store");
        let store = open_store(&db_path).unwrap_or_else(|error| panic!("open store: {error}"));
        let repo_id = 42_i64;
        let commits = vec![
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        ];

        let first_enqueued = store
            .enqueue_commits_for_file_change_extraction(repo_id, &commits)
            .unwrap_or_else(|error| panic!("enqueue extraction commits: {error}"));
        assert_eq!(first_enqueued, 2);

        let second_enqueued = store
            .enqueue_commits_for_file_change_extraction(repo_id, &commits)
            .unwrap_or_else(|error| panic!("enqueue duplicate extraction commits: {error}"));
        assert_eq!(second_enqueued, 0);

        let pending = store
            .load_pending_file_change_extraction_commits(repo_id, 16)
            .unwrap_or_else(|error| panic!("load pending extraction commits: {error}"));
        assert_eq!(
            pending,
            vec![
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string()
            ]
        );
    }

    #[test]
    fn extraction_queue_retries_and_eventually_drops_terminal_failures() {
        let temp = TempDir::new().unwrap_or_else(|error| panic!("create temp dir: {error}"));
        let db_path = temp.path().join("graph-store");
        let store = open_store(&db_path).unwrap_or_else(|error| panic!("open store: {error}"));
        let repo_id = 77_i64;
        let commit = "cccccccccccccccccccccccccccccccccccccccc".to_string();
        let commits = vec![commit.clone()];

        let enqueued = store
            .enqueue_commits_for_file_change_extraction(repo_id, &commits)
            .unwrap_or_else(|error| panic!("enqueue extraction commit: {error}"));
        assert_eq!(enqueued, 1);

        let pending_before_failure = store
            .load_pending_file_change_extraction_commits(repo_id, 4)
            .unwrap_or_else(|error| {
                panic!("load pending extraction commits before failure: {error}")
            });
        assert_eq!(pending_before_failure, vec![commit.clone()]);

        let terminal_failures =
            (0..FILE_CHANGE_EXTRACTION_MAX_ATTEMPTS).fold(0usize, |accumulator, _| {
                accumulator.saturating_add(
                    store
                        .mark_file_change_extraction_commits_failed(
                            repo_id,
                            &commits,
                            "synthetic extraction failure",
                        )
                        .unwrap_or_else(|error| {
                            panic!("mark extraction commit failure for retry path: {error}")
                        }),
                )
            });
        assert_eq!(terminal_failures, 1);

        let pending_after_failure = store
            .load_pending_file_change_extraction_commits(repo_id, 4)
            .unwrap_or_else(|error| {
                panic!("load pending extraction commits after failure: {error}")
            });
        assert!(
            pending_after_failure.is_empty(),
            "expected terminal failures to leave no pending extraction commits"
        );

        let metrics = store
            .load_file_change_extraction_queue_metrics(repo_id)
            .unwrap_or_else(|error| panic!("load extraction queue metrics after failure: {error}"));
        assert_eq!(metrics.depth, 0);
    }
}
