use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rusqlite::Transaction;

use crate::adapters::db::{
    delete_file_changes_for_commits, enqueue_file_changes_for_enrichment, upsert_file_changes,
};
use crate::adapters::git::{FileChangeBatchSequentialExtractor, FileDiffTelemetry};
use crate::domain::models::{IndexingMode, ParsedFileChange};
use crate::support::error::{AppError, AppResult};

const FILE_CHANGE_PIPELINE_WORKERS_ENV: &str = "GITGRAPH_FILE_CHANGE_PIPELINE_WORKERS";
const FILE_CHANGE_PIPELINE_WORKERS_INCREMENTAL_ENV: &str =
    "GITGRAPH_FILE_CHANGE_PIPELINE_WORKERS_INCREMENTAL";
const FILE_CHANGE_PIPELINE_WORKERS_FULL_MAX: usize = 8;
const FILE_CHANGE_PIPELINE_WORKERS_INCREMENTAL_MAX: usize = 4;
const FILE_CHANGE_PIPELINE_RESULT_CHANNEL_CAPACITY_MIN: usize = 8;
const FILE_CHANGE_PIPELINE_RESULT_CHANNEL_CAPACITY_MAX: usize = 64;
const FILE_CHANGE_PIPELINE_FULL_COMMIT_BATCH_SIZE_LOW: usize = 128;
const FILE_CHANGE_PIPELINE_FULL_COMMIT_BATCH_SIZE_HIGH: usize = 256;

#[derive(Debug, Clone, Copy)]
pub(super) struct FileChangePipelineConfig {
    pub(super) worker_count: usize,
    pub(super) result_channel_capacity: usize,
    pub(super) commit_batch_size: usize,
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct FileChangePipelineMetrics {
    pub(super) reorder_buffer_max_depth: usize,
    pub(super) worker_errors: usize,
    pub(super) worker_aborts: usize,
}

#[derive(Default)]
struct PipelineRuntimeMetrics {
    worker_errors: AtomicUsize,
    worker_aborts: AtomicUsize,
}

#[derive(Default)]
pub(super) struct FileChangeFlushStats {
    pub(super) file_changes_indexed: usize,
    pub(super) file_diff_extract: Duration,
    pub(super) file_diff_build: Duration,
    pub(super) file_diff_rename_find: Duration,
    pub(super) file_diff_delta_emit: Duration,
    pub(super) extraction_worker_busy: Duration,
    pub(super) file_change_delete: Duration,
    pub(super) file_change_upsert: Duration,
}

pub(super) struct DiffExtractBatch {
    sequence_id: u64,
    pub(super) commit_shas: Vec<String>,
    pub(super) changes: Vec<ParsedFileChange>,
    pub(super) telemetry: FileDiffTelemetry,
    pub(super) worker_busy: Duration,
}

struct PipelineWorkItem {
    sequence_id: u64,
    commit_shas: Vec<String>,
}

struct WorkerBatchError {
    sequence_id: u64,
    message: String,
}

type WorkerBatchResult = Result<DiffExtractBatch, WorkerBatchError>;

pub(super) fn resolve_file_change_pipeline_config(
    mode: IndexingMode,
    incremental_commit_batch_size: usize,
) -> FileChangePipelineConfig {
    let available_parallelism = std::thread::available_parallelism()
        .map(|parallelism| parallelism.get())
        .unwrap_or(1)
        .max(1);
    let worker_count = resolve_pipeline_worker_count(mode, available_parallelism);
    let result_channel_capacity = worker_count.saturating_mul(2).clamp(
        FILE_CHANGE_PIPELINE_RESULT_CHANNEL_CAPACITY_MIN,
        FILE_CHANGE_PIPELINE_RESULT_CHANNEL_CAPACITY_MAX,
    );
    let commit_batch_size = match mode {
        IndexingMode::Full => {
            if worker_count >= 4 {
                FILE_CHANGE_PIPELINE_FULL_COMMIT_BATCH_SIZE_LOW
            } else {
                FILE_CHANGE_PIPELINE_FULL_COMMIT_BATCH_SIZE_HIGH
            }
        }
        IndexingMode::Incremental => incremental_commit_batch_size.max(1),
    };

    FileChangePipelineConfig {
        worker_count,
        result_channel_capacity,
        commit_batch_size,
    }
}

fn parse_positive_env_usize(key: &str) -> Option<usize> {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
}

fn resolve_pipeline_worker_count(mode: IndexingMode, available_parallelism: usize) -> usize {
    let available = available_parallelism.max(1);
    let default_workers = match mode {
        IndexingMode::Full => available
            .saturating_div(2)
            .max(2)
            .clamp(2, FILE_CHANGE_PIPELINE_WORKERS_FULL_MAX),
        IndexingMode::Incremental => available
            .saturating_div(4)
            .max(1)
            .clamp(1, FILE_CHANGE_PIPELINE_WORKERS_INCREMENTAL_MAX),
    };
    let configured_override = match mode {
        IndexingMode::Full => parse_positive_env_usize(FILE_CHANGE_PIPELINE_WORKERS_ENV),
        IndexingMode::Incremental => {
            parse_positive_env_usize(FILE_CHANGE_PIPELINE_WORKERS_INCREMENTAL_ENV)
                .or_else(|| parse_positive_env_usize(FILE_CHANGE_PIPELINE_WORKERS_ENV))
        }
    };

    configured_override
        .unwrap_or(default_workers)
        .clamp(1, available)
}

pub(super) struct FileChangeExtractionPipeline {
    dispatch_sender: Option<SyncSender<PipelineWorkItem>>,
    result_receiver: Receiver<WorkerBatchResult>,
    in_flight_batches: usize,
    next_dispatch_sequence_id: u64,
    next_receive_sequence_id: u64,
    reorder_buffer: BTreeMap<u64, DiffExtractBatch>,
    max_reorder_buffer_depth: usize,
    dispatcher_handle: Option<JoinHandle<()>>,
    worker_handles: Vec<JoinHandle<()>>,
    runtime_metrics: Arc<PipelineRuntimeMetrics>,
}

impl FileChangeExtractionPipeline {
    pub(super) fn spawn(repo_root: &Path, config: &FileChangePipelineConfig) -> Self {
        let worker_count = config.worker_count.max(1);
        let bounded_capacity = config.result_channel_capacity.max(1);
        let (dispatch_sender, dispatch_receiver) =
            sync_channel::<PipelineWorkItem>(bounded_capacity);
        let (result_sender, result_receiver) = sync_channel::<WorkerBatchResult>(bounded_capacity);

        let runtime_metrics = Arc::new(PipelineRuntimeMetrics::default());
        let mut worker_handles = Vec::with_capacity(worker_count);
        let mut worker_senders = Vec::with_capacity(worker_count);
        let repo_root = repo_root.to_path_buf();

        for _ in 0..worker_count {
            let (worker_sender, worker_receiver) =
                sync_channel::<PipelineWorkItem>(bounded_capacity);
            worker_senders.push(worker_sender);

            let worker_repo_root = repo_root.clone();
            let worker_result_sender = result_sender.clone();
            let worker_runtime_metrics = Arc::clone(&runtime_metrics);
            let worker_handle = thread::spawn(move || {
                run_pipeline_worker(
                    &worker_repo_root,
                    worker_receiver,
                    worker_result_sender,
                    worker_runtime_metrics,
                )
            });
            worker_handles.push(worker_handle);
        }

        let dispatcher_runtime_metrics = Arc::clone(&runtime_metrics);
        let dispatcher_result_sender = result_sender.clone();
        let dispatcher_handle = thread::spawn(move || {
            let mut dispatch_cursor = 0usize;
            while let Ok(work_item) = dispatch_receiver.recv() {
                if worker_senders.is_empty() {
                    dispatcher_runtime_metrics
                        .worker_aborts
                        .fetch_add(1, Ordering::Relaxed);
                    let _ = dispatcher_result_sender.send(Err(WorkerBatchError {
                        sequence_id: work_item.sequence_id,
                        message: "dispatch file-change batch without any extraction workers"
                            .to_string(),
                    }));
                    return;
                }

                let worker_index = dispatch_cursor % worker_senders.len();
                dispatch_cursor = dispatch_cursor.saturating_add(1);
                if let Err(send_error) = worker_senders[worker_index].send(work_item) {
                    dispatcher_runtime_metrics
                        .worker_aborts
                        .fetch_add(1, Ordering::Relaxed);
                    let _ = dispatcher_result_sender.send(Err(WorkerBatchError {
                        sequence_id: send_error.0.sequence_id,
                        message: "dispatch file-change extraction batch to worker".to_string(),
                    }));
                    return;
                }
            }
        });

        drop(result_sender);

        Self {
            dispatch_sender: Some(dispatch_sender),
            result_receiver,
            in_flight_batches: 0,
            next_dispatch_sequence_id: 0,
            next_receive_sequence_id: 0,
            reorder_buffer: BTreeMap::new(),
            max_reorder_buffer_depth: 0,
            dispatcher_handle: Some(dispatcher_handle),
            worker_handles,
            runtime_metrics,
        }
    }

    pub(super) fn dispatch(&mut self, commit_shas: Vec<String>) -> AppResult<Duration> {
        if commit_shas.is_empty() {
            return Ok(Duration::default());
        }
        let Some(dispatch_sender) = &self.dispatch_sender else {
            return Err(AppError::GitCommand(
                "dispatch file-change extraction batch after pipeline shutdown".to_string(),
            ));
        };

        let sequence_id = self.next_dispatch_sequence_id;
        self.next_dispatch_sequence_id = self.next_dispatch_sequence_id.saturating_add(1);
        let dispatch_started = Instant::now();
        dispatch_sender
            .send(PipelineWorkItem {
                sequence_id,
                commit_shas,
            })
            .map_err(|_| {
                AppError::GitCommand("send file-change extraction batch to dispatcher".to_string())
            })?;
        self.in_flight_batches = self.in_flight_batches.saturating_add(1);
        Ok(dispatch_started.elapsed())
    }

    pub(super) fn try_receive(&mut self) -> AppResult<Option<DiffExtractBatch>> {
        if self.in_flight_batches == 0 {
            return Ok(None);
        }
        if let Some(batch) = self.pop_ready_batch() {
            return Ok(Some(batch));
        }

        loop {
            match self.result_receiver.try_recv() {
                Ok(result) => {
                    self.buffer_result(result)?;
                    if let Some(batch) = self.pop_ready_batch() {
                        return Ok(Some(batch));
                    }
                }
                Err(TryRecvError::Empty) => return Ok(None),
                Err(TryRecvError::Disconnected) => {
                    return Err(AppError::GitCommand(
                        "receive file-change extraction batch: worker channel disconnected"
                            .to_string(),
                    ))
                }
            }
        }
    }

    pub(super) fn receive(&mut self) -> AppResult<DiffExtractBatch> {
        if self.in_flight_batches == 0 {
            return Err(AppError::GitCommand(
                "receive file-change extraction batch with no in-flight work".to_string(),
            ));
        }
        if let Some(batch) = self.pop_ready_batch() {
            return Ok(batch);
        }

        loop {
            let result = self.result_receiver.recv().map_err(|_| {
                AppError::GitCommand(
                    "receive file-change extraction batch: worker channel closed".to_string(),
                )
            })?;
            self.buffer_result(result)?;
            if let Some(batch) = self.pop_ready_batch() {
                return Ok(batch);
            }
        }
    }

    pub(super) fn close_input(&mut self) {
        self.dispatch_sender.take();
    }

    pub(super) fn has_in_flight_batches(&self) -> bool {
        self.in_flight_batches > 0
    }

    pub(super) fn metrics(&self) -> FileChangePipelineMetrics {
        FileChangePipelineMetrics {
            reorder_buffer_max_depth: self.max_reorder_buffer_depth,
            worker_errors: self.runtime_metrics.worker_errors.load(Ordering::Relaxed),
            worker_aborts: self.runtime_metrics.worker_aborts.load(Ordering::Relaxed),
        }
    }

    pub(super) fn join_worker(&mut self) -> AppResult<()> {
        let mut panic_detected = false;

        if let Some(dispatcher_handle) = self.dispatcher_handle.take() {
            if dispatcher_handle.join().is_err() {
                panic_detected = true;
                self.runtime_metrics
                    .worker_aborts
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        while let Some(worker_handle) = self.worker_handles.pop() {
            if worker_handle.join().is_err() {
                panic_detected = true;
                self.runtime_metrics
                    .worker_aborts
                    .fetch_add(1, Ordering::Relaxed);
            }
        }

        if panic_detected {
            return Err(AppError::GitCommand(
                "file-change extraction worker thread panicked during join".to_string(),
            ));
        }
        Ok(())
    }

    fn pop_ready_batch(&mut self) -> Option<DiffExtractBatch> {
        let batch = self.reorder_buffer.remove(&self.next_receive_sequence_id)?;
        self.next_receive_sequence_id = self.next_receive_sequence_id.saturating_add(1);
        self.in_flight_batches = self.in_flight_batches.saturating_sub(1);
        Some(batch)
    }

    fn buffer_result(&mut self, result: WorkerBatchResult) -> AppResult<()> {
        match result {
            Ok(batch) => {
                if batch.sequence_id < self.next_receive_sequence_id {
                    return Err(AppError::GitCommand(format!(
                        "received duplicate stale file-change batch sequence {}",
                        batch.sequence_id
                    )));
                }
                if self
                    .reorder_buffer
                    .insert(batch.sequence_id, batch)
                    .is_some()
                {
                    return Err(AppError::GitCommand(
                        "received duplicate file-change extraction batch sequence".to_string(),
                    ));
                }
                self.max_reorder_buffer_depth =
                    self.max_reorder_buffer_depth.max(self.reorder_buffer.len());
                Ok(())
            }
            Err(error) => {
                self.in_flight_batches = self.in_flight_batches.saturating_sub(1);
                Err(AppError::GitCommand(format!(
                    "extract file-change metadata in pipeline worker (sequence {}): {}",
                    error.sequence_id, error.message
                )))
            }
        }
    }
}

fn run_pipeline_worker(
    repo_root: &Path,
    worker_receiver: Receiver<PipelineWorkItem>,
    result_sender: SyncSender<WorkerBatchResult>,
    runtime_metrics: Arc<PipelineRuntimeMetrics>,
) {
    let mut extractor = match FileChangeBatchSequentialExtractor::new(repo_root) {
        Ok(extractor) => extractor,
        Err(error) => {
            runtime_metrics
                .worker_errors
                .fetch_add(1, Ordering::Relaxed);
            let _ = result_sender.send(Err(WorkerBatchError {
                sequence_id: 0,
                message: format!(
                    "initialize file-change extraction worker repository handle: {error}"
                ),
            }));
            return;
        }
    };

    while let Ok(work_item) = worker_receiver.recv() {
        let worker_busy_started = Instant::now();
        let mut changes = Vec::new();
        let telemetry =
            match extractor.batch_sequential_with_reused_repo(&work_item.commit_shas, |change| {
                changes.push(change);
                Ok(())
            }) {
                Ok(telemetry) => telemetry,
                Err(error) => {
                    runtime_metrics
                        .worker_errors
                        .fetch_add(1, Ordering::Relaxed);
                    let _ = result_sender.send(Err(WorkerBatchError {
                        sequence_id: work_item.sequence_id,
                        message: error.to_string(),
                    }));
                    return;
                }
            };
        let batch = DiffExtractBatch {
            sequence_id: work_item.sequence_id,
            commit_shas: work_item.commit_shas,
            changes,
            telemetry,
            worker_busy: worker_busy_started.elapsed(),
        };
        if result_sender.send(Ok(batch)).is_err() {
            runtime_metrics
                .worker_aborts
                .fetch_add(1, Ordering::Relaxed);
            return;
        }
    }
}

pub(super) fn dispatch_pending_commit_batch(
    pipeline: &mut FileChangeExtractionPipeline,
    pending_commit_shas: &mut Vec<String>,
) -> AppResult<Duration> {
    if pending_commit_shas.is_empty() {
        return Ok(Duration::default());
    }
    pipeline.dispatch(std::mem::take(pending_commit_shas))
}

pub(super) fn drain_ready_file_change_batches(
    tx: &Transaction<'_>,
    repo_id: i64,
    mode: IndexingMode,
    pipeline: &mut FileChangeExtractionPipeline,
    pending_file_changes: &mut Vec<ParsedFileChange>,
    file_change_db_batch_size: usize,
) -> AppResult<FileChangeFlushStats> {
    let mut combined_stats = FileChangeFlushStats::default();
    while let Some(batch) = pipeline.try_receive()? {
        let flush_stats = apply_extracted_file_change_batch(
            tx,
            repo_id,
            mode,
            batch,
            pending_file_changes,
            file_change_db_batch_size,
        )?;
        combine_flush_stats(&mut combined_stats, flush_stats);
    }
    Ok(combined_stats)
}

pub(super) fn drain_all_file_change_batches(
    tx: &Transaction<'_>,
    repo_id: i64,
    mode: IndexingMode,
    pipeline: &mut FileChangeExtractionPipeline,
    pending_file_changes: &mut Vec<ParsedFileChange>,
    file_change_db_batch_size: usize,
) -> AppResult<FileChangeFlushStats> {
    let mut combined_stats = FileChangeFlushStats::default();
    while pipeline.has_in_flight_batches() {
        let batch = pipeline.receive()?;
        let flush_stats = apply_extracted_file_change_batch(
            tx,
            repo_id,
            mode,
            batch,
            pending_file_changes,
            file_change_db_batch_size,
        )?;
        combine_flush_stats(&mut combined_stats, flush_stats);
    }
    Ok(combined_stats)
}

fn apply_extracted_file_change_batch(
    tx: &Transaction<'_>,
    repo_id: i64,
    mode: IndexingMode,
    batch: DiffExtractBatch,
    pending_file_changes: &mut Vec<ParsedFileChange>,
    file_change_db_batch_size: usize,
) -> AppResult<FileChangeFlushStats> {
    let mut stats = FileChangeFlushStats::default();
    stats.file_diff_build = stats
        .file_diff_build
        .saturating_add(batch.telemetry.diff_build);
    stats.file_diff_rename_find = stats
        .file_diff_rename_find
        .saturating_add(batch.telemetry.rename_find);
    stats.file_diff_delta_emit = stats
        .file_diff_delta_emit
        .saturating_add(batch.telemetry.delta_emit);
    stats.file_diff_extract = stats
        .file_diff_extract
        .saturating_add(batch.telemetry.total());
    stats.extraction_worker_busy = stats
        .extraction_worker_busy
        .saturating_add(batch.worker_busy);

    if mode == IndexingMode::Incremental {
        let delete_started = Instant::now();
        delete_file_changes_for_commits(tx, repo_id, &batch.commit_shas)?;
        stats.file_change_delete = stats
            .file_change_delete
            .saturating_add(delete_started.elapsed());
    }

    for change in batch.changes {
        pending_file_changes.push(change);
        if pending_file_changes.len() >= file_change_db_batch_size {
            let upsert_started = Instant::now();
            stats.file_changes_indexed = stats
                .file_changes_indexed
                .saturating_add(upsert_file_changes(tx, repo_id, pending_file_changes)?);
            enqueue_file_changes_for_enrichment(tx, repo_id, pending_file_changes)?;
            stats.file_change_upsert = stats
                .file_change_upsert
                .saturating_add(upsert_started.elapsed());
            pending_file_changes.clear();
        }
    }

    Ok(stats)
}

fn combine_flush_stats(target: &mut FileChangeFlushStats, stats: FileChangeFlushStats) {
    target.file_changes_indexed = target
        .file_changes_indexed
        .saturating_add(stats.file_changes_indexed);
    target.file_diff_extract = target
        .file_diff_extract
        .saturating_add(stats.file_diff_extract);
    target.file_diff_build = target.file_diff_build.saturating_add(stats.file_diff_build);
    target.file_diff_rename_find = target
        .file_diff_rename_find
        .saturating_add(stats.file_diff_rename_find);
    target.file_diff_delta_emit = target
        .file_diff_delta_emit
        .saturating_add(stats.file_diff_delta_emit);
    target.extraction_worker_busy = target
        .extraction_worker_busy
        .saturating_add(stats.extraction_worker_busy);
    target.file_change_delete = target
        .file_change_delete
        .saturating_add(stats.file_change_delete);
    target.file_change_upsert = target
        .file_change_upsert
        .saturating_add(stats.file_change_upsert);
}

#[cfg(test)]
mod tests {
    use super::{
        resolve_file_change_pipeline_config, FileChangeExtractionPipeline, FileChangePipelineConfig,
    };
    use crate::domain::models::IndexingMode;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use tempfile::TempDir;

    #[test]
    fn pipeline_config_uses_bounded_worker_and_queue_defaults() {
        let full = resolve_file_change_pipeline_config(IndexingMode::Full, 200);
        assert!(full.worker_count >= 1);
        assert!((128..=256).contains(&full.commit_batch_size));
        assert!(full.result_channel_capacity >= 8);

        let incremental = resolve_file_change_pipeline_config(IndexingMode::Incremental, 200);
        assert!(incremental.worker_count >= 1);
        assert_eq!(incremental.commit_batch_size, 200);
        assert!(incremental.result_channel_capacity >= 8);
    }

    #[test]
    fn pipeline_orders_batches_by_sequence_even_when_worker_completion_differs() {
        let (temp, repo_dir) = create_test_repo();
        write_commits(&repo_dir, 180);
        let commit_shas = list_commit_shas(&repo_dir);
        assert!(commit_shas.len() >= 140);

        let config = FileChangePipelineConfig {
            worker_count: 4,
            result_channel_capacity: 16,
            commit_batch_size: 128,
        };
        let mut pipeline = FileChangeExtractionPipeline::spawn(&repo_dir, &config);
        let first = commit_shas[0..120].to_vec();
        let second = commit_shas[120..140].to_vec();

        let _ = pipeline
            .dispatch(first.clone())
            .unwrap_or_else(|error| panic!("dispatch first batch: {error}"));
        let _ = pipeline
            .dispatch(second.clone())
            .unwrap_or_else(|error| panic!("dispatch second batch: {error}"));
        pipeline.close_input();

        let first_batch = pipeline
            .receive()
            .unwrap_or_else(|error| panic!("receive first ordered batch: {error}"));
        assert_eq!(first_batch.commit_shas, first);
        let second_batch = pipeline
            .receive()
            .unwrap_or_else(|error| panic!("receive second ordered batch: {error}"));
        assert_eq!(second_batch.commit_shas, second);

        pipeline
            .join_worker()
            .unwrap_or_else(|error| panic!("join pipeline worker handles: {error}"));
        drop(temp);
    }

    #[test]
    fn pipeline_join_succeeds_without_in_flight_work() {
        let (temp, repo_dir) = create_test_repo();
        let config = FileChangePipelineConfig {
            worker_count: 2,
            result_channel_capacity: 8,
            commit_batch_size: 128,
        };
        let mut pipeline = FileChangeExtractionPipeline::spawn(&repo_dir, &config);
        pipeline.close_input();
        assert!(!pipeline.has_in_flight_batches());
        pipeline
            .join_worker()
            .unwrap_or_else(|error| panic!("join idle pipeline: {error}"));
        drop(temp);
    }

    #[test]
    fn pipeline_propagates_worker_failure_for_invalid_sha() {
        let (temp, repo_dir) = create_test_repo();
        write_commits(&repo_dir, 1);
        let config = FileChangePipelineConfig {
            worker_count: 2,
            result_channel_capacity: 8,
            commit_batch_size: 128,
        };
        let mut pipeline = FileChangeExtractionPipeline::spawn(&repo_dir, &config);
        let _ = pipeline
            .dispatch(vec!["not-a-valid-hex-sha".to_string()])
            .unwrap_or_else(|error| panic!("dispatch invalid batch: {error}"));
        pipeline.close_input();

        let received_error = pipeline.receive().err();
        assert!(received_error.is_some());
        let metrics = pipeline.metrics();
        assert!(metrics.worker_errors >= 1);

        let _ = pipeline.join_worker();
        drop(temp);
    }

    fn create_test_repo() -> (TempDir, PathBuf) {
        let temp = TempDir::new().unwrap_or_else(|error| panic!("create temp dir: {error}"));
        let repo_dir = temp.path().join("repo");
        fs::create_dir_all(&repo_dir).unwrap_or_else(|error| panic!("create repo dir: {error}"));
        run_git(&repo_dir, &["init"]);
        run_git(&repo_dir, &["config", "user.name", "GitGraph Lab"]);
        run_git(
            &repo_dir,
            &["config", "user.email", "gitgraph-lab@example.com"],
        );
        (temp, repo_dir)
    }

    fn write_commits(repo_dir: &Path, count: usize) {
        for index in 0..count {
            fs::write(repo_dir.join("README.md"), format!("commit-{index}\n"))
                .unwrap_or_else(|error| panic!("write commit file: {error}"));
            run_git(repo_dir, &["add", "."]);
            run_git(repo_dir, &["commit", "-m", &format!("commit-{index}")]);
        }
    }

    fn list_commit_shas(repo_dir: &Path) -> Vec<String> {
        let raw = run_git_capture(repo_dir, &["rev-list", "--topo-order", "--all"]);
        raw.lines().map(|line| line.trim().to_string()).collect()
    }

    fn run_git(repo_dir: &Path, args: &[&str]) {
        let _ = run_git_capture(repo_dir, args);
    }

    fn run_git_capture(repo_dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo_dir)
            .args(args)
            .output()
            .unwrap_or_else(|error| panic!("execute git: {error}"));
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            panic!("git command failed: {:?}: {stderr}", args);
        }
        String::from_utf8_lossy(&output.stdout).to_string()
    }
}
