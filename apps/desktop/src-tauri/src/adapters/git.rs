use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use git2::{
    Commit, Delta, DiffDelta, DiffFindOptions, DiffLineType, Error, ErrorCode, Repository, Sort,
};
use rayon::prelude::*;

use crate::domain::models::{ParsedCommit, ParsedFileChange, ParsedRef};
use crate::support::error::{AppError, AppResult};

pub const FILE_CHANGE_COMMIT_BATCH_SIZE: usize = 200;
const FILE_CHANGE_PARSE_PARALLEL_MIN_COMMITS: usize = 64;
const FILE_CHANGE_PARSE_PARALLEL_CHUNK_SIZE: usize = 16;

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
    mut on_commit: impl FnMut(ParsedCommit) -> AppResult<()>,
) -> AppResult<()> {
    walk_commits(repo_root, since_epoch, |_repo, commit| {
        on_commit(build_parsed_commit(commit))
    })
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
            append_file_changes_for_commit(&repo, commit_sha, &mut on_change)?;
        }
        return Ok(());
    }

    let chunked_changes = collect_file_changes_for_chunks(repo_root, commit_shas)?;
    for chunk_changes in chunked_changes {
        for change in chunk_changes {
            on_change(change)?;
        }
    }

    Ok(())
}

fn collect_file_changes_for_chunks(
    repo_root: &Path,
    commit_shas: &[String],
) -> AppResult<Vec<Vec<ParsedFileChange>>> {
    commit_shas
        .par_chunks(FILE_CHANGE_PARSE_PARALLEL_CHUNK_SIZE)
        .map(|chunk| collect_file_changes_for_chunk(repo_root, chunk))
        .collect()
}

fn collect_file_changes_for_chunk(
    repo_root: &Path,
    commit_shas: &[String],
) -> AppResult<Vec<ParsedFileChange>> {
    let repo = open_repository(repo_root)?;
    let mut chunk_changes = Vec::new();
    for commit_sha in commit_shas {
        append_file_changes_for_commit(&repo, commit_sha, &mut |change| {
            chunk_changes.push(change);
            Ok(())
        })?;
    }
    Ok(chunk_changes)
}

fn append_file_changes_for_commit(
    repo: &Repository,
    commit_sha: &str,
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

    let mut find_options = DiffFindOptions::new();
    find_options.renames(true);
    diff.find_similar(Some(&mut find_options))
        .map_err(|error| map_git_error("detect file renames in commit diff", error))?;

    let by_path = RefCell::new(HashMap::new());
    let ordered_paths = RefCell::new(Vec::new());

    diff.foreach(
        &mut |delta, _progress| {
            let path = delta_display_path(&delta);
            if path.is_empty() {
                return true;
            }

            let mut by_path = by_path.borrow_mut();
            if !by_path.contains_key(&path) {
                ordered_paths.borrow_mut().push(path.clone());
                by_path.insert(path.clone(), FileChangeAccumulator::from_delta(&delta));
            } else if let Some(accumulator) = by_path.get_mut(&path) {
                accumulator.apply_delta(&delta);
            }
            true
        },
        None,
        None,
        Some(&mut |delta, _hunk, line| {
            let path = delta_display_path(&delta);
            if path.is_empty() {
                return true;
            }

            if let Some(accumulator) = by_path.borrow_mut().get_mut(&path) {
                accumulator.apply_line(line.origin_value());
            }
            true
        }),
    )
    .map_err(|error| map_git_error("walk commit diff lines", error))?;

    let mut by_path = by_path.into_inner();
    let ordered_paths = ordered_paths.into_inner();
    for path in ordered_paths {
        let Some(accumulator) = by_path.remove(&path) else {
            continue;
        };
        on_change(ParsedFileChange {
            commit_sha: commit_sha.to_string(),
            file_path: path.clone(),
            additions: accumulator.additions,
            deletions: accumulator.deletions,
            change_type: accumulator.finalize_change_type(&path),
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

#[derive(Debug, Clone)]
struct FileChangeAccumulator {
    additions: i64,
    deletions: i64,
    change_type: String,
    binary: bool,
}

impl FileChangeAccumulator {
    fn from_delta(delta: &DiffDelta<'_>) -> Self {
        Self {
            additions: 0,
            deletions: 0,
            change_type: delta_change_type(delta),
            binary: delta.old_file().is_binary() || delta.new_file().is_binary(),
        }
    }

    fn apply_delta(&mut self, delta: &DiffDelta<'_>) {
        self.binary = self.binary || delta.old_file().is_binary() || delta.new_file().is_binary();
        if self.change_type == "modify" {
            self.change_type = delta_change_type(delta);
        }
    }

    fn apply_line(&mut self, line_type: DiffLineType) {
        match line_type {
            DiffLineType::Addition | DiffLineType::AddEOFNL => {
                self.additions = self.additions.saturating_add(1);
            }
            DiffLineType::Deletion | DiffLineType::DeleteEOFNL => {
                self.deletions = self.deletions.saturating_add(1);
            }
            DiffLineType::Binary => {
                self.binary = true;
            }
            _ => {}
        }
    }

    fn finalize_change_type(&self, file_path: &str) -> String {
        if self.binary {
            return "binary".to_string();
        }
        if self.change_type != "modify" {
            return self.change_type.clone();
        }
        classify_change_type(file_path, self.additions, self.deletions, false)
    }
}

fn classify_change_type(file_path: &str, additions: i64, deletions: i64, binary: bool) -> String {
    if binary {
        return "binary".to_string();
    }
    if file_path.contains("=>") {
        return "rename".to_string();
    }
    if additions > 0 && deletions == 0 {
        return "add".to_string();
    }
    if additions == 0 && deletions > 0 {
        return "delete".to_string();
    }
    "modify".to_string()
}

#[cfg(test)]
mod tests {
    use super::{classify_change_type, validate_repository};
    use std::path::Path;

    #[test]
    fn unknown_repo_returns_error() {
        let result = validate_repository(Path::new("c:/non-existent/repo"));
        assert!(result.is_err());
    }

    #[test]
    fn classifies_numstat_style_change_types() {
        assert_eq!(classify_change_type("src/lib.rs", 10, 0, false), "add");
        assert_eq!(classify_change_type("src/lib.rs", 0, 10, false), "delete");
        assert_eq!(
            classify_change_type("old/path.rs => new/path.rs", 1, 1, false),
            "rename"
        );
        assert_eq!(
            classify_change_type("assets/logo.png", 0, 0, true),
            "binary"
        );
        assert_eq!(classify_change_type("src/lib.rs", 3, 2, false), "modify");
    }
}
