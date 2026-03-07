use std::collections::{hash_map::Entry, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::sync_channel;

use git2::{Commit, Delta, DiffDelta, DiffFindOptions, Error, ErrorCode, Patch, Repository, Sort};
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
struct DiffRenameScanStats {
    has_add_delete_pair: bool,
    max_file_size_bytes: u64,
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
    let repo = Repository::discover(&canonical_input)
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
    mut on_commit: impl FnMut(&Repository, &Commit<'_>) -> AppResult<()>,
) -> AppResult<()> {
    let repo = open_repository(repo_root)?;
    if repo
        .is_empty()
        .map_err(|error| map_git_error("check repository emptiness", error))?
    {
        return Ok(());
    }

    let mut revwalk = repo
        .revwalk()
        .map_err(|error| map_git_error("create revision walk", error))?;
    revwalk
        .set_sorting(Sort::TOPOLOGICAL | Sort::TIME)
        .map_err(|error| map_git_error("configure revision walk sorting", error))?;

    if let Err(error) = revwalk.push_glob("refs/*") {
        if !is_not_found_error(&error) {
            return Err(map_git_error("seed revision walk from refs/*", error));
        }
        return Ok(());
    }

    if let Some(hide_sha) = hide_ancestor_sha {
        let hide_oid = git2::Oid::from_str(hide_sha)
            .map_err(|error| map_git_error("parse revwalk hide commit sha", error))?;
        if let Err(error) = revwalk.hide(hide_oid) {
            if !is_not_found_error(&error) {
                return Err(map_git_error(
                    "configure revision walk hide boundary",
                    error,
                ));
            }
        }
    }

    for oid_result in revwalk {
        let oid = oid_result.map_err(|error| map_git_error("read walked commit id", error))?;
        let commit = repo
            .find_commit(oid)
            .map_err(|error| map_git_error("resolve walked commit", error))?;

        if let Some(since) = since_epoch {
            if commit.committer().when().seconds() < since {
                continue;
            }
        }

        on_commit(&repo, &commit)?;
    }

    Ok(())
}

fn build_parsed_commit(commit: &Commit<'_>) -> ParsedCommit {
    let parents = commit.parent_ids().map(|id| id.to_string()).collect();
    let author = commit.author();
    let committer = commit.committer();
    let subject = commit
        .summary_bytes()
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        .unwrap_or_default();

    ParsedCommit {
        sha: commit.id().to_string(),
        parents,
        tree_sha: commit.tree_id().to_string(),
        author_name: author.name().unwrap_or_default().to_string(),
        author_email: author.email().unwrap_or_default().to_string(),
        author_time: author.when().seconds(),
        committer_time: committer.when().seconds(),
        subject,
    }
}

pub fn load_refs(repo_root: &Path) -> AppResult<Vec<ParsedRef>> {
    let repo = open_repository(repo_root)?;
    let mut refs = Vec::new();

    append_refs_by_glob(&repo, "refs/heads/*", "head", false, &mut refs)?;
    append_refs_by_glob(&repo, "refs/tags/*", "tag", true, &mut refs)?;

    Ok(refs)
}

fn append_refs_by_glob(
    repo: &Repository,
    glob: &str,
    ref_type: &str,
    peel_to_commit: bool,
    refs: &mut Vec<ParsedRef>,
) -> AppResult<()> {
    let iterator = match repo.references_glob(glob) {
        Ok(iter) => iter,
        Err(error) if is_not_found_error(&error) => return Ok(()),
        Err(error) => {
            return Err(map_git_error(
                &format!("list references by glob {glob}"),
                error,
            ))
        }
    };

    for reference_result in iterator {
        let reference = reference_result
            .map_err(|error| map_git_error(&format!("iterate references by glob {glob}"), error))?;
        let Some(ref_name) = reference.name() else {
            continue;
        };
        let Some(target_sha) = resolve_reference_target(&reference, peel_to_commit) else {
            continue;
        };

        refs.push(ParsedRef {
            ref_name: ref_name.to_string(),
            target_sha,
            ref_type: ref_type.to_string(),
        });
    }

    Ok(())
}

fn resolve_reference_target(
    reference: &git2::Reference<'_>,
    peel_to_commit: bool,
) -> Option<String> {
    if peel_to_commit {
        if let Ok(object) = reference.peel(git2::ObjectType::Commit) {
            return Some(object.id().to_string());
        }
    }

    if let Some(target) = reference.target() {
        return Some(target.to_string());
    }

    if let Ok(resolved) = reference.resolve() {
        if let Some(target) = resolved.target() {
            return Some(target.to_string());
        }
    }

    None
}

pub fn load_file_changes_for_commits(
    repo_root: &Path,
    commit_shas: &[String],
) -> AppResult<Vec<ParsedFileChange>> {
    let mut changes = Vec::new();
    stream_file_changes_for_commits(repo_root, commit_shas, |change| {
        changes.push(change);
        Ok(())
    })?;
    Ok(changes)
}

pub fn stream_file_changes_for_commits(
    repo_root: &Path,
    commit_shas: &[String],
    mut on_change: impl FnMut(ParsedFileChange) -> AppResult<()>,
) -> AppResult<()> {
    if commit_shas.is_empty() {
        return Ok(());
    }

    if commit_shas.len() < FILE_CHANGE_PARSE_PARALLEL_MIN_COMMITS {
        let repo = open_repository(repo_root)?;
        for commit_sha in commit_shas {
            append_file_changes_for_commit(
                &repo,
                commit_sha,
                FileChangeExtractionMode::MetadataOnly,
                None,
                &mut on_change,
            )?;
        }
        return Ok(());
    }

    stream_file_changes_parallel_buffered(
        repo_root,
        commit_shas,
        FileChangeExtractionMode::MetadataOnly,
        None,
        on_change,
    )
}

fn stream_file_changes_parallel_buffered(
    repo_root: &Path,
    commit_shas: &[String],
    extraction_mode: FileChangeExtractionMode,
    line_stat_targets_by_commit: Option<&HashMap<String, HashSet<String>>>,
    mut on_change: impl FnMut(ParsedFileChange) -> AppResult<()>,
) -> AppResult<()> {
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
        return Ok(());
    }

    let (sender, receiver) =
        sync_channel::<(usize, AppResult<Vec<ParsedFileChange>>)>(parallel_plan.channel_capacity);
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
            let chunk_changes = chunk_result?;
            pending_by_index.insert(chunk_index, chunk_changes);

            while let Some(ordered_chunk_changes) = pending_by_index.remove(&next_chunk_index) {
                for change in ordered_chunk_changes {
                    on_change(change)?;
                }
                next_chunk_index = next_chunk_index.saturating_add(1);
            }
        }

        Ok(())
    })?;

    Ok(())
}

fn collect_file_changes_for_chunk_with_repo(
    repo: &Repository,
    commit_shas: &[String],
    extraction_mode: FileChangeExtractionMode,
    line_stat_targets_by_commit: Option<&HashMap<String, HashSet<String>>>,
) -> AppResult<Vec<ParsedFileChange>> {
    let mut chunk_changes = Vec::new();
    for commit_sha in commit_shas {
        let line_stat_targets = line_stat_targets_by_commit
            .and_then(|targets_by_commit| targets_by_commit.get(commit_sha));
        append_file_changes_for_commit(
            repo,
            commit_sha,
            extraction_mode,
            line_stat_targets,
            &mut |change| {
                chunk_changes.push(change);
                Ok(())
            },
        )?;
    }
    Ok(chunk_changes)
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
        for commit_sha in &commit_shas {
            let line_stat_targets = line_stat_targets_by_commit.get(commit_sha);
            append_file_changes_for_commit(
                &repo,
                commit_sha,
                FileChangeExtractionMode::SelectiveLineStats,
                line_stat_targets,
                &mut |change| {
                    changes.push(change);
                    Ok(())
                },
            )?;
        }
        changes
    } else {
        let mut changes = Vec::new();
        stream_file_changes_parallel_buffered(
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

fn diff_requires_line_stats(delta: &DiffDelta<'_>) -> bool {
    if delta.old_file().is_binary() || delta.new_file().is_binary() {
        return false;
    }

    match delta.status() {
        Delta::Added | Delta::Deleted | Delta::Modified | Delta::Copied => true,
        Delta::Renamed => delta.old_file().id() != delta.new_file().id(),
        _ => false,
    }
}

fn diff_rename_scan_stats(diff: &git2::Diff<'_>) -> DiffRenameScanStats {
    let mut has_added = false;
    let mut has_deleted = false;
    let mut max_file_size_bytes = 0u64;
    for delta in diff.deltas() {
        max_file_size_bytes = max_file_size_bytes.max(delta.old_file().size());
        max_file_size_bytes = max_file_size_bytes.max(delta.new_file().size());
        match delta.status() {
            Delta::Added => has_added = true,
            Delta::Deleted => has_deleted = true,
            _ => {}
        }
    }

    DiffRenameScanStats {
        has_add_delete_pair: has_added && has_deleted,
        max_file_size_bytes,
    }
}

fn collect_patch_line_stats(
    diff: &git2::Diff<'_>,
    diff_index: usize,
) -> AppResult<Option<(i64, i64)>> {
    let Some(patch) = Patch::from_diff(diff, diff_index)
        .map_err(|error| map_git_error("create patch for selective line stats", error))?
    else {
        return Ok(None);
    };

    let (_context, additions, deletions) = patch
        .line_stats()
        .map_err(|error| map_git_error("read patch line stats", error))?;
    Ok(Some((
        usize_to_i64_saturating(additions),
        usize_to_i64_saturating(deletions),
    )))
}

fn append_file_changes_for_commit(
    repo: &Repository,
    commit_sha: &str,
    extraction_mode: FileChangeExtractionMode,
    line_stat_targets: Option<&HashSet<String>>,
    on_change: &mut impl FnMut(ParsedFileChange) -> AppResult<()>,
) -> AppResult<()> {
    let oid = git2::Oid::from_str(commit_sha)
        .map_err(|error| map_git_error("parse commit sha", error))?;
    let commit = repo
        .find_commit(oid)
        .map_err(|error| map_git_error("resolve commit for file change extraction", error))?;
    let commit_tree = commit
        .tree()
        .map_err(|error| map_git_error("read commit tree", error))?;
    let parent_tree = if commit.parent_count() > 0 {
        Some(
            commit
                .parent(0)
                .map_err(|error| map_git_error("read first parent commit", error))?
                .tree()
                .map_err(|error| map_git_error("read parent commit tree", error))?,
        )
    } else {
        None
    };

    let mut diff_options = git2::DiffOptions::new();
    let mut diff = repo
        .diff_tree_to_tree(
            parent_tree.as_ref(),
            Some(&commit_tree),
            Some(&mut diff_options),
        )
        .map_err(|error| map_git_error("compute tree diff for commit", error))?;

    let delta_count = diff.deltas().len();
    let rename_scan_stats = diff_rename_scan_stats(&diff);
    if should_run_rename_detection(
        extraction_mode,
        delta_count,
        rename_scan_stats.has_add_delete_pair,
    ) {
        if let Some(rename_policy) =
            rename_detection_policy(delta_count, rename_scan_stats.max_file_size_bytes)
        {
            let mut find_options = DiffFindOptions::new();
            find_options
                .renames(true)
                .rename_threshold(rename_policy.rename_threshold)
                .rename_limit(rename_policy.rename_limit)
                .exact_match_only(rename_policy.exact_match_only);
            diff.find_similar(Some(&mut find_options))
                .map_err(|error| map_git_error("detect file renames in commit diff", error))?;
        }
    }

    let mut by_path: HashMap<String, FileChangeAccumulator> = HashMap::new();
    let mut ordered_paths: Vec<String> = Vec::new();
    let mut selected_line_stat_targets: Vec<DiffLineStatTarget> = Vec::new();

    for (diff_index, delta) in diff.deltas().enumerate() {
        let path = delta_display_path(&delta);
        if path.is_empty() {
            continue;
        }

        match by_path.entry(path.clone()) {
            Entry::Vacant(entry) => {
                ordered_paths.push(path.clone());
                entry.insert(FileChangeAccumulator::from_delta(&delta));
            }
            Entry::Occupied(mut entry) => {
                entry.get_mut().apply_delta(&delta);
            }
        }

        if extraction_mode == FileChangeExtractionMode::SelectiveLineStats
            && diff_requires_line_stats(&delta)
            && line_stat_targets
                .map(|targets| targets.contains(path.as_str()))
                .unwrap_or(false)
        {
            selected_line_stat_targets.push(DiffLineStatTarget { diff_index, path });
        }
    }

    for target in selected_line_stat_targets {
        let Some((additions, deletions)) = collect_patch_line_stats(&diff, target.diff_index)?
        else {
            continue;
        };
        if let Some(accumulator) = by_path.get_mut(&target.path) {
            accumulator.add_line_stats(additions, deletions);
        }
    }

    let filter_paths = line_stat_targets;
    let emit_selected_paths_only = extraction_mode == FileChangeExtractionMode::SelectiveLineStats;
    if emit_selected_paths_only && filter_paths.is_none() {
        return Ok(());
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

    Ok(())
}

fn delta_display_path(delta: &DiffDelta<'_>) -> String {
    let old_path = delta
        .old_file()
        .path()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();
    let new_path = delta
        .new_file()
        .path()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();

    if delta.status() == Delta::Renamed
        && !old_path.is_empty()
        && !new_path.is_empty()
        && old_path != new_path
    {
        return format!("{old_path} => {new_path}");
    }

    if !new_path.is_empty() {
        return new_path;
    }
    old_path
}

fn delta_change_type(delta: &DiffDelta<'_>) -> String {
    match delta.status() {
        Delta::Added => "add".to_string(),
        Delta::Deleted => "delete".to_string(),
        Delta::Renamed => "rename".to_string(),
        _ => "modify".to_string(),
    }
}

fn open_repository(repo_root: &Path) -> AppResult<Repository> {
    Repository::open(repo_root).map_err(|error| map_git_error("open git repository", error))
}

fn map_git_error(context: &str, error: Error) -> AppError {
    AppError::GitCommand(format!(
        "{context}: {} (class={:?}, code={:?})",
        error.message(),
        error.class(),
        error.code()
    ))
}

fn is_not_found_error(error: &Error) -> bool {
    error.code() == ErrorCode::NotFound
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
    fn from_delta(delta: &DiffDelta<'_>) -> Self {
        let binary = delta.old_file().is_binary() || delta.new_file().is_binary();
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

    fn apply_delta(&mut self, delta: &DiffDelta<'_>) {
        self.requires_line_stats = self.requires_line_stats || diff_requires_line_stats(delta);
        self.binary = self.binary || delta.old_file().is_binary() || delta.new_file().is_binary();
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
        parallel_file_change_plan, rename_detection_policy, rename_threshold_for_bins,
        should_run_rename_detection, validate_repository, FileChangeExtractionMode,
        FILE_CHANGE_EXACT_MATCH_ONLY_MIN_DELTAS, FILE_CHANGE_PARSE_CHANNEL_CAPACITY,
        FILE_CHANGE_PARSE_CHANNEL_CAPACITY_MAX, FILE_CHANGE_PARSE_PARALLEL_CHUNK_SIZE,
        FILE_CHANGE_PARSE_PARALLEL_MIN_CHUNK_SIZE, FILE_CHANGE_RENAME_LARGE_FILE_BYTES,
        FILE_CHANGE_RENAME_LIMIT, FILE_CHANGE_RENAME_MEDIUM_FILE_BYTES,
        FILE_CHANGE_RENAME_THRESHOLD_LARGE, FILE_CHANGE_RENAME_THRESHOLD_MEDIUM,
        FILE_CHANGE_RENAME_THRESHOLD_SMALL,
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
}
