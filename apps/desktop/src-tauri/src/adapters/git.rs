use std::collections::{hash_map::Entry, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::sync_channel;
use std::time::{Duration, Instant};

use gix::bstr::ByteSlice;
use gix::diff::Rewrites;
use gix::hash::ObjectId;
use gix::object::tree::diff::ChangeDetached;
use gix::prelude::TreeDiffChangeExt;
use gix::revision::walk::Sorting;
use gix::traverse::commit::simple::CommitTimeOrder;
use gix::Repository;
use rayon::prelude::*;

use crate::domain::models::{
    FileChangeLineStatsUpdate, FileChangeStatsState, ParsedCommit, ParsedFileChange, ParsedRef,
    PendingFileChangeEnrichment,
};
use crate::support::error::{AppError, AppResult};

pub const FILE_CHANGE_COMMIT_BATCH_SIZE: usize = 200;
const FILE_CHANGE_PARSE_PARALLEL_MIN_COMMITS: usize = 64;
const FILE_CHANGE_PARSE_PARALLEL_CHUNK_SIZE: usize = 16;
const FILE_CHANGE_PARSE_PARALLEL_MIN_CHUNK_SIZE: usize = 4;
const FILE_CHANGE_PARSE_PARALLEL_TARGET_CHUNKS_PER_WORKER: usize = 2;
const FILE_CHANGE_PARSE_CHANNEL_CAPACITY: usize = 8;
const FILE_CHANGE_PARSE_CHANNEL_CAPACITY_PER_WORKER: usize = 2;
const FILE_CHANGE_PARSE_CHANNEL_CAPACITY_MAX: usize = 64;
const FILE_CHANGE_RENAME_LIMIT: usize = 400;
const FILE_CHANGE_RENAME_THRESHOLD_SMALL: u16 = 60;
const FILE_CHANGE_RENAME_THRESHOLD_MEDIUM: u16 = 65;
const FILE_CHANGE_RENAME_THRESHOLD_LARGE: u16 = 70;
const FILE_CHANGE_RENAME_MEDIUM_FILE_BYTES: u64 = 64 * 1024;
const FILE_CHANGE_RENAME_LARGE_FILE_BYTES: u64 = 256 * 1024;
const FILE_CHANGE_EXACT_MATCH_ONLY_MIN_DELTAS: usize = 512;
const FILE_CHANGE_EXACT_MATCH_ONLY_FILE_BYTES: u64 = 1_024 * 1_024;
const PATCH_FROM_DIFF_GUARDRAIL_MARKER: &str = "Patch::from_diff";
const MIN_OBJECT_ID_HEX_LEN: usize = 40;
const GIX_OBJECT_CACHE_BYTES_DEFAULT: usize = 64 * 1024 * 1024;
const GIX_OBJECT_CACHE_BYTES_MIN: usize = 16 * 1024 * 1024;
const GIX_OBJECT_CACHE_BYTES_MAX: usize = GIX_OBJECT_CACHE_BYTES_DEFAULT;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileChangeExtractionMode {
    MetadataOnly,
    SelectiveLineStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RenameDetectionPolicy {
    rename_limit: usize,
    rename_threshold: u16,
    exact_match_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ParallelFileChangePlan {
    chunk_size: usize,
    channel_capacity: usize,
}

#[derive(Debug, Clone)]
struct DiffLineStatTarget {
    diff_index: usize,
    path: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FileDiffTelemetry {
    pub diff_build: Duration,
    pub rename_find: Duration,
    pub delta_emit: Duration,
}

impl FileDiffTelemetry {
    fn merge(&mut self, other: Self) {
        self.diff_build = self.diff_build.saturating_add(other.diff_build);
        self.rename_find = self.rename_find.saturating_add(other.rename_find);
        self.delta_emit = self.delta_emit.saturating_add(other.delta_emit);
    }

    pub fn total(&self) -> Duration {
        self.diff_build
            .saturating_add(self.rename_find)
            .saturating_add(self.delta_emit)
    }
}

#[derive(Debug)]
struct ChunkFileChanges {
    changes: Vec<ParsedFileChange>,
    telemetry: FileDiffTelemetry,
}

#[derive(Debug, Clone)]
pub struct CanonicalRepo {
    pub root: PathBuf,
    pub git_dir: PathBuf,
    pub name: String,
}

pub fn validate_repository(path: &Path) -> AppResult<CanonicalRepo> {
    if !path.exists() {
        return Err(AppError::PathNotFound(path.display().to_string()));
    }

    let canonical_input = fs::canonicalize(path)?;
    let repo = gix::discover(&canonical_input)
        .map_err(|_| AppError::NotGitRepository(path.display().to_string()))?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| AppError::NotGitRepository(path.display().to_string()))?;
    let root = fs::canonicalize(workdir)?;
    let git_dir = fs::canonicalize(repo.path())?;

    let name = root
        .file_name()
        .map(|segment| segment.to_string_lossy().to_string())
        .unwrap_or_else(|| "repo".to_string());

    Ok(CanonicalRepo {
        root,
        git_dir,
        name,
    })
}

pub fn load_commits(repo_root: &Path) -> AppResult<Vec<ParsedCommit>> {
    collect_commits(repo_root, None)
}

pub fn load_commits_since(repo_root: &Path, since_epoch: i64) -> AppResult<Vec<ParsedCommit>> {
    collect_commits(repo_root, Some(since_epoch.max(0)))
}

pub fn stream_commits(
    repo_root: &Path,
    since_epoch: Option<i64>,
    on_commit: impl FnMut(ParsedCommit) -> AppResult<()>,
) -> AppResult<()> {
    stream_commits_with_hide(repo_root, since_epoch, None, on_commit)
}

pub fn stream_commits_with_hide(
    repo_root: &Path,
    since_epoch: Option<i64>,
    hide_ancestor_sha: Option<&str>,
    mut on_commit: impl FnMut(ParsedCommit) -> AppResult<()>,
) -> AppResult<()> {
    walk_commits(
        repo_root,
        since_epoch,
        hide_ancestor_sha,
        |_repo, commit| on_commit(build_parsed_commit(commit)),
    )
}

fn collect_commits(repo_root: &Path, since_epoch: Option<i64>) -> AppResult<Vec<ParsedCommit>> {
    let mut commits = Vec::new();
    stream_commits(repo_root, since_epoch, |commit| {
        commits.push(commit);
        Ok(())
    })?;
    Ok(commits)
}

fn walk_commits(
    repo_root: &Path,
    since_epoch: Option<i64>,
    hide_ancestor_sha: Option<&str>,
    mut on_commit: impl FnMut(&Repository, &gix::Commit<'_>) -> AppResult<()>,
) -> AppResult<()> {
    let repo = open_repository(repo_root)?;
    let tips = collect_revision_tips(&repo)?;
    if tips.is_empty() {
        return Ok(());
    }

    let mut revwalk = repo
        .rev_walk(tips)
        .sorting(Sorting::ByCommitTime(CommitTimeOrder::NewestFirst));
    if let Some(hide_sha) = hide_ancestor_sha {
        let hide_oid = parse_hex_object_id(hide_sha, "parse revision walk hide commit sha")?;
        revwalk = revwalk.with_hidden([hide_oid]);
    }

    for walked in revwalk
        .all()
        .map_err(|error| map_git_error("create revision walk", error))?
    {
        let info = walked.map_err(|error| map_git_error("read walked commit id", error))?;
        if let Some(since) = since_epoch {
            if info
                .commit_time
                .is_some_and(|commit_time| commit_time < since)
            {
                continue;
            }
        }

        let commit = repo
            .find_commit(info.id)
            .map_err(|error| map_git_error("resolve walked commit", error))?;
        if let Some(since) = since_epoch {
            if commit
                .committer()
                .map(|committer| committer.seconds())
                .unwrap_or_default()
                < since
            {
                continue;
            }
        }

        on_commit(&repo, &commit)?;
    }

    Ok(())
}

fn collect_revision_tips(repo: &Repository) -> AppResult<Vec<ObjectId>> {
    let references = repo
        .references()
        .map_err(|error| map_git_error("open reference iterator", error))?;
    let mut tips = Vec::new();
    let mut seen = HashSet::new();
    for entry in references
        .all()
        .map_err(|error| map_git_error("iterate references", error))?
    {
        let mut reference = entry.map_err(|error| map_git_error("read reference", error))?;
        let Ok(commit) = reference.peel_to_commit() else {
            continue;
        };
        let tip = commit.id().detach();
        if seen.insert(tip) {
            tips.push(tip);
        }
    }
    Ok(tips)
}

fn build_parsed_commit(commit: &gix::Commit<'_>) -> ParsedCommit {
    let parents = commit.parent_ids().map(|id| id.to_string()).collect();
    let author = commit.author().ok();
    let committer = commit.committer().ok();
    let subject = commit
        .message_raw_sloppy()
        .lines()
        .next()
        .map(|line| line.to_str_lossy().into_owned())
        .unwrap_or_default();

    ParsedCommit {
        sha: commit.id().to_string(),
        parents,
        tree_sha: commit
            .tree_id()
            .map(|tree_id| tree_id.to_string())
            .unwrap_or_default(),
        author_name: author
            .map(|signature| signature.name.to_str_lossy().into_owned())
            .unwrap_or_default(),
        author_email: author
            .map(|signature| signature.email.to_str_lossy().into_owned())
            .unwrap_or_default(),
        author_time: author
            .map(|signature| signature.seconds())
            .unwrap_or_default(),
        committer_time: committer
            .map(|signature| signature.seconds())
            .unwrap_or_default(),
        subject,
    }
}

pub fn load_refs(repo_root: &Path) -> AppResult<Vec<ParsedRef>> {
    let repo = open_repository(repo_root)?;
    let mut refs = Vec::new();

    append_refs_by_glob(&repo, "refs/heads/", "head", false, &mut refs)?;
    append_refs_by_glob(&repo, "refs/tags/", "tag", true, &mut refs)?;

    Ok(refs)
}

fn append_refs_by_glob(
    repo: &Repository,
    prefix: &str,
    ref_type: &str,
    peel_to_commit: bool,
    refs: &mut Vec<ParsedRef>,
) -> AppResult<()> {
    let platform = repo
        .references()
        .map_err(|error| map_git_error("open reference iterator", error))?;
    let iterator = platform
        .prefixed(prefix)
        .map_err(|error| map_git_error(&format!("list references by prefix {prefix}"), error))?;

    for reference_result in iterator {
        let mut reference = reference_result.map_err(|error| {
            map_git_error(&format!("iterate references by prefix {prefix}"), error)
        })?;
        let Some(target_sha) = resolve_reference_target(&mut reference, peel_to_commit) else {
            continue;
        };

        refs.push(ParsedRef {
            ref_name: reference.name().to_string(),
            target_sha,
            ref_type: ref_type.to_string(),
        });
    }

    Ok(())
}

fn resolve_reference_target(
    reference: &mut gix::Reference<'_>,
    peel_to_commit: bool,
) -> Option<String> {
    if peel_to_commit {
        if let Ok(commit) = reference.peel_to_commit() {
            return Some(commit.id().to_string());
        }
    }

    if let Some(target) = reference.try_id() {
        return Some(target.to_string());
    }

    if let Ok(resolved) = reference.follow_to_object() {
        return Some(resolved.to_string());
    }

    None
}

pub fn load_file_changes_for_commits(
    repo_root: &Path,
    commit_shas: &[String],
) -> AppResult<Vec<ParsedFileChange>> {
    let mut changes = Vec::new();
    let _telemetry = stream_file_changes_for_commits(repo_root, commit_shas, |change| {
        changes.push(change);
        Ok(())
    })?;
    Ok(changes)
}

pub fn stream_file_changes_for_commits(
    repo_root: &Path,
    commit_shas: &[String],
    mut on_change: impl FnMut(ParsedFileChange) -> AppResult<()>,
) -> AppResult<FileDiffTelemetry> {
    let mut telemetry = FileDiffTelemetry::default();
    if commit_shas.is_empty() {
        return Ok(telemetry);
    }

    if commit_shas.len() < FILE_CHANGE_PARSE_PARALLEL_MIN_COMMITS {
        let repo = open_repository(repo_root)?;
        let mut line_stat_resource_cache = None;
        for commit_sha in commit_shas {
            let commit_telemetry = append_file_changes_for_commit(
                &repo,
                commit_sha,
                FileChangeExtractionMode::MetadataOnly,
                None,
                &mut line_stat_resource_cache,
                &mut on_change,
            )?;
            telemetry.merge(commit_telemetry);
        }
        return Ok(telemetry);
    }

    let parallel_telemetry = stream_file_changes_parallel_buffered(
        repo_root,
        commit_shas,
        FileChangeExtractionMode::MetadataOnly,
        None,
        on_change,
    )?;
    telemetry.merge(parallel_telemetry);
    Ok(telemetry)
}

fn stream_file_changes_parallel_buffered(
    repo_root: &Path,
    commit_shas: &[String],
    extraction_mode: FileChangeExtractionMode,
    line_stat_targets_by_commit: Option<&HashMap<String, HashSet<String>>>,
    mut on_change: impl FnMut(ParsedFileChange) -> AppResult<()>,
) -> AppResult<FileDiffTelemetry> {
    let mut total_telemetry = FileDiffTelemetry::default();
    let parallel_plan = parallel_file_change_plan(commit_shas.len());
    let base_chunks: Vec<&[String]> = commit_shas
        .chunks(FILE_CHANGE_PARSE_PARALLEL_CHUNK_SIZE)
        .collect();
    let commit_chunks: Vec<&[String]> =
        if parallel_plan.chunk_size < FILE_CHANGE_PARSE_PARALLEL_CHUNK_SIZE {
            base_chunks
                .into_iter()
                .flat_map(|chunk| chunk.chunks(parallel_plan.chunk_size))
                .collect()
        } else {
            base_chunks
        };
    let total_chunks = commit_chunks.len();
    if total_chunks == 0 {
        return Ok(total_telemetry);
    }

    let (sender, receiver) =
        sync_channel::<(usize, AppResult<ChunkFileChanges>)>(parallel_plan.channel_capacity);
    std::thread::scope(|scope| -> AppResult<()> {
        let line_stat_targets_by_commit = line_stat_targets_by_commit.cloned();
        scope.spawn(move || {
            commit_chunks
                .into_par_iter()
                .enumerate()
                .map_init(
                    || open_repository(repo_root),
                    |repo_state, (chunk_index, chunk)| {
                        let chunk_result = match repo_state {
                            Ok(repo) => collect_file_changes_for_chunk_with_repo(
                                repo,
                                chunk,
                                extraction_mode,
                                line_stat_targets_by_commit.as_ref(),
                            ),
                            Err(error) => Err(AppError::GitCommand(format!(
                                "initialize parallel file-change extraction worker repository handle: {error}"
                            ))),
                        };
                        (chunk_index, chunk_result)
                    },
                )
                .for_each_with(sender, |tx, (chunk_index, chunk_result)| {
                    let _ = tx.send((chunk_index, chunk_result));
                });
        });

        let mut next_chunk_index = 0usize;
        let mut pending_by_index: HashMap<usize, Vec<ParsedFileChange>> = HashMap::new();
        while next_chunk_index < total_chunks {
            let (chunk_index, chunk_result) = receiver.recv().map_err(|_| {
                AppError::GitCommand(
                    "receive parsed file-change chunk from parallel producer".to_string(),
                )
            })?;
            let chunk = chunk_result?;
            total_telemetry.merge(chunk.telemetry);
            pending_by_index.insert(chunk_index, chunk.changes);

            while let Some(ordered_chunk_changes) = pending_by_index.remove(&next_chunk_index) {
                for change in ordered_chunk_changes {
                    on_change(change)?;
                }
                next_chunk_index = next_chunk_index.saturating_add(1);
            }
        }

        Ok(())
    })?;

    Ok(total_telemetry)
}

fn collect_file_changes_for_chunk_with_repo(
    repo: &Repository,
    commit_shas: &[String],
    extraction_mode: FileChangeExtractionMode,
    line_stat_targets_by_commit: Option<&HashMap<String, HashSet<String>>>,
) -> AppResult<ChunkFileChanges> {
    let mut telemetry = FileDiffTelemetry::default();
    let mut chunk_changes = Vec::new();
    let mut line_stat_resource_cache = None;
    for commit_sha in commit_shas {
        let line_stat_targets = line_stat_targets_by_commit
            .and_then(|targets_by_commit| targets_by_commit.get(commit_sha));
        let commit_telemetry = append_file_changes_for_commit(
            repo,
            commit_sha,
            extraction_mode,
            line_stat_targets,
            &mut line_stat_resource_cache,
            &mut |change| {
                chunk_changes.push(change);
                Ok(())
            },
        )?;
        telemetry.merge(commit_telemetry);
    }
    Ok(ChunkFileChanges {
        changes: chunk_changes,
        telemetry,
    })
}

pub fn load_file_change_line_stats_for_targets(
    repo_root: &Path,
    targets: &[PendingFileChangeEnrichment],
) -> AppResult<Vec<FileChangeLineStatsUpdate>> {
    if targets.is_empty() {
        return Ok(Vec::new());
    }

    let mut line_stat_targets_by_commit: HashMap<String, HashSet<String>> = HashMap::new();
    for target in targets {
        line_stat_targets_by_commit
            .entry(target.commit_sha.clone())
            .or_default()
            .insert(target.file_path.clone());
    }

    let mut commit_shas: Vec<String> = line_stat_targets_by_commit.keys().cloned().collect();
    commit_shas.sort_unstable();
    let parsed = if commit_shas.len() < FILE_CHANGE_PARSE_PARALLEL_MIN_COMMITS {
        let repo = open_repository(repo_root)?;
        let mut changes = Vec::new();
        let mut line_stat_resource_cache = None;
        for commit_sha in &commit_shas {
            let line_stat_targets = line_stat_targets_by_commit.get(commit_sha);
            let _commit_telemetry = append_file_changes_for_commit(
                &repo,
                commit_sha,
                FileChangeExtractionMode::SelectiveLineStats,
                line_stat_targets,
                &mut line_stat_resource_cache,
                &mut |change| {
                    changes.push(change);
                    Ok(())
                },
            )?;
        }
        changes
    } else {
        let mut changes = Vec::new();
        let _telemetry = stream_file_changes_parallel_buffered(
            repo_root,
            &commit_shas,
            FileChangeExtractionMode::SelectiveLineStats,
            Some(&line_stat_targets_by_commit),
            |change| {
                changes.push(change);
                Ok(())
            },
        )?;
        changes
    };

    let mut by_commit: HashMap<String, HashMap<String, (i64, i64)>> = HashMap::new();
    for change in parsed {
        let additions = change.additions.unwrap_or_default();
        let deletions = change.deletions.unwrap_or_default();
        by_commit
            .entry(change.commit_sha)
            .or_default()
            .insert(change.file_path, (additions, deletions));
    }

    let mut updates = Vec::with_capacity(targets.len());
    for target in targets {
        let (additions, deletions) = by_commit
            .get(&target.commit_sha)
            .and_then(|files| files.get(&target.file_path))
            .copied()
            .unwrap_or_default();
        updates.push(FileChangeLineStatsUpdate {
            commit_sha: target.commit_sha.clone(),
            file_path: target.file_path.clone(),
            additions,
            deletions,
        });
    }

    Ok(updates)
}

fn rename_detection_policy(
    delta_count: usize,
    max_file_size_bytes: u64,
) -> Option<RenameDetectionPolicy> {
    if delta_count == 0 {
        return None;
    }

    let rename_threshold = rename_threshold_for_bins(delta_count, max_file_size_bytes);
    Some(RenameDetectionPolicy {
        rename_limit: delta_count.clamp(1, FILE_CHANGE_RENAME_LIMIT),
        rename_threshold,
        exact_match_only: delta_count >= FILE_CHANGE_EXACT_MATCH_ONLY_MIN_DELTAS
            || max_file_size_bytes >= FILE_CHANGE_EXACT_MATCH_ONLY_FILE_BYTES,
    })
}

fn rename_threshold_for_bins(delta_count: usize, max_file_size_bytes: u64) -> u16 {
    if delta_count >= FILE_CHANGE_EXACT_MATCH_ONLY_MIN_DELTAS
        || max_file_size_bytes >= FILE_CHANGE_RENAME_LARGE_FILE_BYTES
    {
        return FILE_CHANGE_RENAME_THRESHOLD_LARGE;
    }

    if delta_count >= 128 || max_file_size_bytes >= FILE_CHANGE_RENAME_MEDIUM_FILE_BYTES {
        return FILE_CHANGE_RENAME_THRESHOLD_MEDIUM;
    }

    FILE_CHANGE_RENAME_THRESHOLD_SMALL
}

fn should_run_rename_detection(
    extraction_mode: FileChangeExtractionMode,
    delta_count: usize,
    has_add_delete_pair: bool,
) -> bool {
    if delta_count < 2 {
        return false;
    }

    match extraction_mode {
        FileChangeExtractionMode::MetadataOnly => has_add_delete_pair,
        FileChangeExtractionMode::SelectiveLineStats => {
            has_add_delete_pair || delta_count >= FILE_CHANGE_EXACT_MATCH_ONLY_MIN_DELTAS
        }
    }
}

fn parallel_file_change_plan(total_commits: usize) -> ParallelFileChangePlan {
    let worker_count = rayon::current_num_threads().max(1);
    let target_chunk_count =
        worker_count.saturating_mul(FILE_CHANGE_PARSE_PARALLEL_TARGET_CHUNKS_PER_WORKER);
    let adaptive_chunk_size = if target_chunk_count == 0 {
        FILE_CHANGE_PARSE_PARALLEL_CHUNK_SIZE
    } else {
        total_commits.div_ceil(target_chunk_count)
    };
    let chunk_size = adaptive_chunk_size.clamp(
        FILE_CHANGE_PARSE_PARALLEL_MIN_CHUNK_SIZE,
        FILE_CHANGE_PARSE_PARALLEL_CHUNK_SIZE,
    );
    let channel_capacity = worker_count
        .saturating_mul(FILE_CHANGE_PARSE_CHANNEL_CAPACITY_PER_WORKER)
        .clamp(
            FILE_CHANGE_PARSE_CHANNEL_CAPACITY,
            FILE_CHANGE_PARSE_CHANNEL_CAPACITY_MAX,
        );

    ParallelFileChangePlan {
        chunk_size,
        channel_capacity,
    }
}

fn rewrites_from_policy(policy: RenameDetectionPolicy) -> Rewrites {
    Rewrites {
        copies: None,
        percentage: if policy.exact_match_only {
            None
        } else {
            Some((f32::from(policy.rename_threshold) / 100.0).clamp(0.0, 1.0))
        },
        limit: policy.rename_limit,
        track_empty: false,
    }
}

fn diff_requires_line_stats(delta: &ChangeDetached) -> bool {
    if !change_is_blob_or_symlink(delta) {
        return false;
    }

    match delta {
        ChangeDetached::Addition { .. }
        | ChangeDetached::Deletion { .. }
        | ChangeDetached::Modification { .. } => true,
        ChangeDetached::Rewrite { source_id, id, .. } => source_id != id,
    }
}

fn change_is_blob_or_symlink(delta: &ChangeDetached) -> bool {
    match delta {
        ChangeDetached::Addition { entry_mode, .. }
        | ChangeDetached::Deletion { entry_mode, .. } => entry_mode.is_blob_or_symlink(),
        ChangeDetached::Modification {
            previous_entry_mode,
            entry_mode,
            ..
        } => previous_entry_mode.is_blob_or_symlink() || entry_mode.is_blob_or_symlink(),
        ChangeDetached::Rewrite {
            source_entry_mode,
            entry_mode,
            ..
        } => source_entry_mode.is_blob_or_symlink() || entry_mode.is_blob_or_symlink(),
    }
}

fn change_is_binary(delta: &ChangeDetached) -> bool {
    !change_is_blob_or_symlink(delta)
}

fn parse_hex_object_id(object_id: &str, context: &str) -> AppResult<ObjectId> {
    let oid_len = object_id.len();
    if oid_len != MIN_OBJECT_ID_HEX_LEN
        || !object_id
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(AppError::InvalidArgument(format!(
            "{context}: expected {MIN_OBJECT_ID_HEX_LEN} hex chars"
        )));
    }

    ObjectId::from_hex(object_id.as_bytes()).map_err(|error| map_git_error(context, error))
}

fn object_size_bytes(repo: &Repository, id: ObjectId) -> u64 {
    repo.try_find_header(id)
        .ok()
        .flatten()
        .map(|header| header.size())
        .unwrap_or(0)
}

fn diff_has_add_delete_pair(diff: &[ChangeDetached]) -> bool {
    let mut has_added = false;
    let mut has_deleted = false;

    for delta in diff {
        match delta {
            ChangeDetached::Addition { .. } => {
                has_added = true;
            }
            ChangeDetached::Deletion { .. } => {
                has_deleted = true;
            }
            ChangeDetached::Modification { .. } | ChangeDetached::Rewrite { .. } => {}
        }
        if has_added && has_deleted {
            return true;
        }
    }

    false
}

fn diff_max_file_size_bytes(repo: &Repository, diff: &[ChangeDetached]) -> u64 {
    let mut max_file_size_bytes = 0u64;
    for delta in diff {
        match delta {
            ChangeDetached::Addition { id, .. } | ChangeDetached::Deletion { id, .. } => {
                max_file_size_bytes = max_file_size_bytes.max(object_size_bytes(repo, *id));
            }
            ChangeDetached::Modification {
                previous_id, id, ..
            } => {
                max_file_size_bytes =
                    max_file_size_bytes.max(object_size_bytes(repo, *previous_id));
                max_file_size_bytes = max_file_size_bytes.max(object_size_bytes(repo, *id));
            }
            ChangeDetached::Rewrite { source_id, id, .. } => {
                max_file_size_bytes = max_file_size_bytes.max(object_size_bytes(repo, *source_id));
                max_file_size_bytes = max_file_size_bytes.max(object_size_bytes(repo, *id));
            }
        }
    }
    max_file_size_bytes
}

fn get_or_init_line_stat_resource_cache<'a>(
    repo: &Repository,
    line_stat_resource_cache: &'a mut Option<gix::diff::blob::Platform>,
) -> AppResult<&'a mut gix::diff::blob::Platform> {
    if line_stat_resource_cache.is_none() {
        *line_stat_resource_cache = Some(
            repo.diff_resource_cache_for_tree_diff()
                .map_err(|error| map_git_error("prepare selective line-stat diff cache", error))?,
        );
    }
    line_stat_resource_cache.as_mut().ok_or_else(|| {
        AppError::GitCommand("initialize selective line-stat resource cache".to_string())
    })
}

fn collect_patch_line_stats(
    repo: &Repository,
    resource_cache: &mut gix::diff::blob::Platform,
    diff: &[ChangeDetached],
    diff_index: usize,
) -> AppResult<Option<(i64, i64)>> {
    let _ = PATCH_FROM_DIFF_GUARDRAIL_MARKER;
    let Some(change) = diff.get(diff_index) else {
        return Ok(None);
    };
    if !diff_requires_line_stats(change) {
        return Ok(None);
    }

    let mut platform = change
        .attach(repo, repo)
        .diff(resource_cache)
        .map_err(|error| map_git_error("create patch for selective line stats", error))?;
    let counts = platform
        .line_counts()
        .map_err(|error| map_git_error("read patch line stats", error))?;
    resource_cache.clear_resource_cache_keep_allocation();
    Ok(counts.map(|counter| {
        (
            usize_to_i64_saturating(counter.insertions as usize),
            usize_to_i64_saturating(counter.removals as usize),
        )
    }))
}

fn append_file_changes_for_commit(
    repo: &Repository,
    commit_sha: &str,
    extraction_mode: FileChangeExtractionMode,
    line_stat_targets: Option<&HashSet<String>>,
    line_stat_resource_cache: &mut Option<gix::diff::blob::Platform>,
    on_change: &mut impl FnMut(ParsedFileChange) -> AppResult<()>,
) -> AppResult<FileDiffTelemetry> {
    let mut telemetry = FileDiffTelemetry::default();
    let diff_build_started = Instant::now();

    let oid = parse_hex_object_id(commit_sha, "parse commit sha")?;
    let commit = repo
        .find_commit(oid)
        .map_err(|error| map_git_error("resolve commit for file change extraction", error))?;
    let commit_tree = commit
        .tree()
        .map_err(|error| map_git_error("read commit tree", error))?;
    let parent_tree = commit
        .parent_ids()
        .next()
        .map(|parent_id| {
            repo.find_commit(parent_id.detach())
                .map_err(|error| map_git_error("read first parent commit", error))
                .and_then(|parent| {
                    parent
                        .tree()
                        .map_err(|error| map_git_error("read parent commit tree", error))
                })
        })
        .transpose()?;

    let mut raw_diff_options = gix::diff::Options::default();
    raw_diff_options.track_rewrites(None);
    let raw_diff = repo
        .diff_tree_to_tree(
            parent_tree.as_ref(),
            Some(&commit_tree),
            Some(raw_diff_options),
        )
        .map_err(|error| map_git_error("compute tree diff for commit", error))?;
    telemetry.diff_build = telemetry
        .diff_build
        .saturating_add(diff_build_started.elapsed());

    let rename_find_started = Instant::now();
    let delta_count = raw_diff.len();
    let mut diff = raw_diff;
    if delta_count >= 2 {
        let has_add_delete_pair = diff_has_add_delete_pair(&diff);
        if should_run_rename_detection(extraction_mode, delta_count, has_add_delete_pair) {
            let max_file_size_bytes = diff_max_file_size_bytes(repo, &diff);
            if let Some(rename_policy) = rename_detection_policy(delta_count, max_file_size_bytes) {
                let mut rewrite_options = gix::diff::Options::default();
                rewrite_options.track_rewrites(Some(rewrites_from_policy(rename_policy)));
                diff = repo
                    .diff_tree_to_tree(
                        parent_tree.as_ref(),
                        Some(&commit_tree),
                        Some(rewrite_options),
                    )
                    .map_err(|error| map_git_error("detect file renames in commit diff", error))?;
            }
        }
    }
    telemetry.rename_find = telemetry
        .rename_find
        .saturating_add(rename_find_started.elapsed());

    let delta_emit_started = Instant::now();
    if diff.len() <= 1 {
        emit_single_delta_fast_path(
            repo,
            &diff,
            commit_sha,
            extraction_mode,
            line_stat_targets,
            line_stat_resource_cache,
            on_change,
        )?;
        telemetry.delta_emit = telemetry
            .delta_emit
            .saturating_add(delta_emit_started.elapsed());
        return Ok(telemetry);
    }

    let mut by_path: HashMap<String, FileChangeAccumulator> = HashMap::new();
    let mut ordered_paths: Vec<String> = Vec::new();
    let mut selected_line_stat_targets: Vec<DiffLineStatTarget> = Vec::new();

    for (diff_index, delta) in diff.iter().enumerate() {
        let path = delta_display_path(delta);
        if path.is_empty() {
            continue;
        }

        match by_path.entry(path.clone()) {
            Entry::Vacant(entry) => {
                ordered_paths.push(path.clone());
                entry.insert(FileChangeAccumulator::from_delta(delta));
            }
            Entry::Occupied(mut entry) => {
                entry.get_mut().apply_delta(delta);
            }
        }

        if extraction_mode == FileChangeExtractionMode::SelectiveLineStats
            && diff_requires_line_stats(delta)
            && line_stat_targets
                .map(|targets| targets.contains(path.as_str()))
                .unwrap_or(false)
        {
            selected_line_stat_targets.push(DiffLineStatTarget { diff_index, path });
        }
    }

    if !selected_line_stat_targets.is_empty() {
        let resource_cache = get_or_init_line_stat_resource_cache(repo, line_stat_resource_cache)?;
        for target in selected_line_stat_targets {
            let Some((additions, deletions)) =
                collect_patch_line_stats(repo, resource_cache, &diff, target.diff_index)?
            else {
                continue;
            };
            if let Some(accumulator) = by_path.get_mut(&target.path) {
                accumulator.add_line_stats(additions, deletions);
            }
        }
    }

    let filter_paths = line_stat_targets;
    let emit_selected_paths_only = extraction_mode == FileChangeExtractionMode::SelectiveLineStats;
    if emit_selected_paths_only && filter_paths.is_none() {
        telemetry.delta_emit = telemetry
            .delta_emit
            .saturating_add(delta_emit_started.elapsed());
        return Ok(telemetry);
    }

    for path in ordered_paths {
        if emit_selected_paths_only
            && !filter_paths
                .map(|targets| targets.contains(path.as_str()))
                .unwrap_or(false)
        {
            continue;
        }

        let Some(accumulator) = by_path.remove(&path) else {
            continue;
        };
        let (additions, deletions, stats_state, requires_line_stats) = match extraction_mode {
            FileChangeExtractionMode::MetadataOnly => accumulator.metadata_view(),
            FileChangeExtractionMode::SelectiveLineStats => accumulator.line_stats_view(),
        };
        on_change(ParsedFileChange {
            commit_sha: commit_sha.to_string(),
            file_path: path,
            additions,
            deletions,
            change_type: accumulator.change_type,
            stats_state,
            requires_line_stats,
        })?;
    }

    telemetry.delta_emit = telemetry
        .delta_emit
        .saturating_add(delta_emit_started.elapsed());
    Ok(telemetry)
}

fn emit_single_delta_fast_path(
    repo: &Repository,
    diff: &[ChangeDetached],
    commit_sha: &str,
    extraction_mode: FileChangeExtractionMode,
    line_stat_targets: Option<&HashSet<String>>,
    line_stat_resource_cache: &mut Option<gix::diff::blob::Platform>,
    on_change: &mut impl FnMut(ParsedFileChange) -> AppResult<()>,
) -> AppResult<()> {
    let Some(delta) = diff.first() else {
        return Ok(());
    };

    let path = delta_display_path(delta);
    if path.is_empty() {
        return Ok(());
    }

    let emit_selected_paths_only = extraction_mode == FileChangeExtractionMode::SelectiveLineStats;
    if emit_selected_paths_only {
        let Some(targets) = line_stat_targets else {
            return Ok(());
        };
        if !targets.contains(path.as_str()) {
            return Ok(());
        }
    }

    let mut accumulator = FileChangeAccumulator::from_delta(delta);
    if extraction_mode == FileChangeExtractionMode::SelectiveLineStats
        && diff_requires_line_stats(delta)
        && line_stat_targets
            .map(|targets| targets.contains(path.as_str()))
            .unwrap_or(false)
    {
        let resource_cache = get_or_init_line_stat_resource_cache(repo, line_stat_resource_cache)?;
        if let Some((additions, deletions)) =
            collect_patch_line_stats(repo, resource_cache, diff, 0)?
        {
            accumulator.add_line_stats(additions, deletions);
        }
    }

    let (additions, deletions, stats_state, requires_line_stats) = match extraction_mode {
        FileChangeExtractionMode::MetadataOnly => accumulator.metadata_view(),
        FileChangeExtractionMode::SelectiveLineStats => accumulator.line_stats_view(),
    };
    on_change(ParsedFileChange {
        commit_sha: commit_sha.to_string(),
        file_path: path,
        additions,
        deletions,
        change_type: accumulator.change_type,
        stats_state,
        requires_line_stats,
    })?;

    Ok(())
}

fn delta_display_path(delta: &ChangeDetached) -> String {
    match delta {
        ChangeDetached::Rewrite {
            source_location,
            location,
            ..
        } => {
            let old_path = String::from_utf8_lossy(source_location.as_ref()).into_owned();
            let new_path = String::from_utf8_lossy(location.as_ref()).into_owned();
            if !old_path.is_empty() && !new_path.is_empty() && old_path != new_path {
                format!("{old_path} => {new_path}")
            } else if !new_path.is_empty() {
                new_path
            } else {
                old_path
            }
        }
        ChangeDetached::Addition { location, .. }
        | ChangeDetached::Deletion { location, .. }
        | ChangeDetached::Modification { location, .. } => {
            String::from_utf8_lossy(location.as_ref()).into_owned()
        }
    }
}

fn delta_change_type(delta: &ChangeDetached) -> String {
    match delta {
        ChangeDetached::Addition { .. } => "add".to_string(),
        ChangeDetached::Deletion { .. } => "delete".to_string(),
        ChangeDetached::Rewrite { copy, .. } if !copy => "rename".to_string(),
        _ => "modify".to_string(),
    }
}

fn open_repository(repo_root: &Path) -> AppResult<Repository> {
    let mut repo = gix::open(repo_root.to_path_buf())
        .map_err(|error| map_git_error("open git repository", error))?;
    let worker_concurrency_hint = if rayon::current_thread_index().is_some() {
        rayon::current_num_threads().max(1)
    } else {
        1
    };
    repo.object_cache_size_if_unset(object_cache_bytes_for_workers(worker_concurrency_hint));
    Ok(repo)
}

fn object_cache_bytes_for_workers(worker_count: usize) -> usize {
    let normalized_worker_count = worker_count.max(1);
    if normalized_worker_count == 1 {
        return GIX_OBJECT_CACHE_BYTES_DEFAULT;
    }

    GIX_OBJECT_CACHE_BYTES_DEFAULT
        .saturating_div(normalized_worker_count)
        .clamp(GIX_OBJECT_CACHE_BYTES_MIN, GIX_OBJECT_CACHE_BYTES_MAX)
}

fn map_git_error(context: &str, error: impl std::fmt::Display) -> AppError {
    AppError::GitCommand(format!("{context}: {error}"))
}

fn usize_to_i64_saturating(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[derive(Debug, Clone)]
struct FileChangeAccumulator {
    additions: i64,
    deletions: i64,
    change_type: String,
    binary: bool,
    requires_line_stats: bool,
}

impl FileChangeAccumulator {
    fn from_delta(delta: &ChangeDetached) -> Self {
        let binary = change_is_binary(delta);
        Self {
            additions: 0,
            deletions: 0,
            change_type: if binary {
                "binary".to_string()
            } else {
                delta_change_type(delta)
            },
            binary,
            requires_line_stats: diff_requires_line_stats(delta),
        }
    }

    fn apply_delta(&mut self, delta: &ChangeDetached) {
        self.requires_line_stats = self.requires_line_stats || diff_requires_line_stats(delta);
        self.binary = self.binary || change_is_binary(delta);
        if self.binary {
            self.change_type = "binary".to_string();
        } else if self.change_type == "modify" {
            self.change_type = delta_change_type(delta);
        }
    }

    fn add_line_stats(&mut self, additions: i64, deletions: i64) {
        self.additions = self.additions.saturating_add(additions);
        self.deletions = self.deletions.saturating_add(deletions);
    }

    fn metadata_view(&self) -> (Option<i64>, Option<i64>, FileChangeStatsState, bool) {
        if self.requires_line_stats {
            return (None, None, FileChangeStatsState::Pending, true);
        }

        (
            Some(self.additions),
            Some(self.deletions),
            FileChangeStatsState::Ready,
            false,
        )
    }

    fn line_stats_view(&self) -> (Option<i64>, Option<i64>, FileChangeStatsState, bool) {
        (
            Some(self.additions),
            Some(self.deletions),
            FileChangeStatsState::Ready,
            false,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        object_cache_bytes_for_workers, parallel_file_change_plan, parse_hex_object_id,
        rename_detection_policy, rename_threshold_for_bins, should_run_rename_detection,
        validate_repository, FileChangeExtractionMode, FILE_CHANGE_EXACT_MATCH_ONLY_MIN_DELTAS,
        FILE_CHANGE_PARSE_CHANNEL_CAPACITY, FILE_CHANGE_PARSE_CHANNEL_CAPACITY_MAX,
        FILE_CHANGE_PARSE_PARALLEL_CHUNK_SIZE, FILE_CHANGE_PARSE_PARALLEL_MIN_CHUNK_SIZE,
        FILE_CHANGE_RENAME_LARGE_FILE_BYTES, FILE_CHANGE_RENAME_LIMIT,
        FILE_CHANGE_RENAME_MEDIUM_FILE_BYTES, FILE_CHANGE_RENAME_THRESHOLD_LARGE,
        FILE_CHANGE_RENAME_THRESHOLD_MEDIUM, FILE_CHANGE_RENAME_THRESHOLD_SMALL,
        GIX_OBJECT_CACHE_BYTES_DEFAULT, GIX_OBJECT_CACHE_BYTES_MAX, GIX_OBJECT_CACHE_BYTES_MIN,
    };
    use std::path::Path;

    #[test]
    fn unknown_repo_returns_error() {
        let result = validate_repository(Path::new("c:/non-existent/repo"));
        assert!(result.is_err());
    }

    #[test]
    fn rename_threshold_is_adaptive_by_delta_count_and_file_size() {
        assert_eq!(
            rename_threshold_for_bins(8, 1024),
            FILE_CHANGE_RENAME_THRESHOLD_SMALL
        );
        assert_eq!(
            rename_threshold_for_bins(128, 1024),
            FILE_CHANGE_RENAME_THRESHOLD_MEDIUM
        );
        assert_eq!(
            rename_threshold_for_bins(8, FILE_CHANGE_RENAME_MEDIUM_FILE_BYTES),
            FILE_CHANGE_RENAME_THRESHOLD_MEDIUM
        );
        assert_eq!(
            rename_threshold_for_bins(8, FILE_CHANGE_RENAME_LARGE_FILE_BYTES),
            FILE_CHANGE_RENAME_THRESHOLD_LARGE
        );
    }

    #[test]
    fn rename_detection_bounds_are_configured() {
        assert!(rename_detection_policy(0, 0).is_none());

        let bounded_small = rename_detection_policy(32, 1024).expect("policy for non-zero deltas");
        assert_eq!(bounded_small.rename_limit, 32);
        assert_eq!(
            bounded_small.rename_threshold,
            FILE_CHANGE_RENAME_THRESHOLD_SMALL
        );
        assert!(!bounded_small.exact_match_only);

        let bounded_large =
            rename_detection_policy(FILE_CHANGE_EXACT_MATCH_ONLY_MIN_DELTAS + 1, 1024)
                .expect("policy for large delta set");
        assert_eq!(bounded_large.rename_limit, FILE_CHANGE_RENAME_LIMIT);
        assert_eq!(
            bounded_large.rename_threshold,
            FILE_CHANGE_RENAME_THRESHOLD_LARGE
        );
        assert!(bounded_large.exact_match_only);
    }

    #[test]
    fn rename_detection_runs_only_when_diff_shape_requires_it() {
        assert!(!should_run_rename_detection(
            FileChangeExtractionMode::MetadataOnly,
            1,
            true
        ));
        assert!(!should_run_rename_detection(
            FileChangeExtractionMode::MetadataOnly,
            16,
            false
        ));
        assert!(should_run_rename_detection(
            FileChangeExtractionMode::MetadataOnly,
            16,
            true
        ));

        assert!(!should_run_rename_detection(
            FileChangeExtractionMode::SelectiveLineStats,
            16,
            false
        ));
        assert!(should_run_rename_detection(
            FileChangeExtractionMode::SelectiveLineStats,
            FILE_CHANGE_EXACT_MATCH_ONLY_MIN_DELTAS,
            false
        ));
    }

    #[test]
    fn parallel_file_change_plan_is_bounded() {
        let plan_small = parallel_file_change_plan(FILE_CHANGE_PARSE_PARALLEL_MIN_CHUNK_SIZE);
        assert!((FILE_CHANGE_PARSE_PARALLEL_MIN_CHUNK_SIZE
            ..=FILE_CHANGE_PARSE_PARALLEL_CHUNK_SIZE)
            .contains(&plan_small.chunk_size));
        assert!(
            (FILE_CHANGE_PARSE_CHANNEL_CAPACITY..=FILE_CHANGE_PARSE_CHANNEL_CAPACITY_MAX)
                .contains(&plan_small.channel_capacity)
        );

        let plan_large = parallel_file_change_plan(10_000);
        assert!((FILE_CHANGE_PARSE_PARALLEL_MIN_CHUNK_SIZE
            ..=FILE_CHANGE_PARSE_PARALLEL_CHUNK_SIZE)
            .contains(&plan_large.chunk_size));
        assert!(
            (FILE_CHANGE_PARSE_CHANNEL_CAPACITY..=FILE_CHANGE_PARSE_CHANNEL_CAPACITY_MAX)
                .contains(&plan_large.channel_capacity)
        );
    }

    #[test]
    fn object_cache_size_scales_and_stays_bounded() {
        assert_eq!(
            object_cache_bytes_for_workers(0),
            GIX_OBJECT_CACHE_BYTES_DEFAULT
        );
        assert_eq!(
            object_cache_bytes_for_workers(1),
            GIX_OBJECT_CACHE_BYTES_DEFAULT
        );
        assert_eq!(object_cache_bytes_for_workers(2), 32 * 1024 * 1024);
        assert_eq!(
            object_cache_bytes_for_workers(4),
            GIX_OBJECT_CACHE_BYTES_MIN
        );
        assert_eq!(
            object_cache_bytes_for_workers(32),
            GIX_OBJECT_CACHE_BYTES_MIN
        );
        assert!(object_cache_bytes_for_workers(1) <= GIX_OBJECT_CACHE_BYTES_MAX);
    }

    #[test]
    fn parse_hex_object_id_validates_input() {
        assert!(parse_hex_object_id(&"a".repeat(40), "parse commit sha").is_ok());
        assert!(parse_hex_object_id("not-hex", "parse commit sha").is_err());
        assert!(parse_hex_object_id(&"a".repeat(39), "parse commit sha").is_err());
        assert!(parse_hex_object_id(&"a".repeat(42), "parse commit sha").is_err());
        assert!(parse_hex_object_id(&"a".repeat(66), "parse commit sha").is_err());
    }
}
