use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::domain::models::{ParsedCommit, ParsedFileChange, ParsedRef};
use crate::support::error::{AppError, AppResult};

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
    let inside_flag = run_git_command(&canonical_input, &["rev-parse", "--is-inside-work-tree"])?;
    if inside_flag.trim() != "true" {
        return Err(AppError::NotGitRepository(path.display().to_string()));
    }

    let top_level = run_git_command(&canonical_input, &["rev-parse", "--show-toplevel"])?;
    let root = fs::canonicalize(top_level.trim())?;

    let git_dir_raw = run_git_command(&root, &["rev-parse", "--git-dir"])?;
    let git_dir_candidate = PathBuf::from(git_dir_raw.trim());
    let git_dir = if git_dir_candidate.is_absolute() {
        fs::canonicalize(git_dir_candidate)?
    } else {
        fs::canonicalize(root.join(git_dir_candidate))?
    };

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
    let format = "%H%x1f%P%x1f%T%x1f%an%x1f%ae%x1f%at%x1f%ct%x1f%s%x1e";
    let pretty = format!("--pretty=format:{format}");
    let log_output = run_git_command(
        repo_root,
        &[
            "log",
            "--all",
            "--topo-order",
            "--date-order",
            pretty.as_str(),
        ],
    )?;

    parse_git_log_records(&log_output)
}

pub fn load_commits_since(repo_root: &Path, since_epoch: i64) -> AppResult<Vec<ParsedCommit>> {
    let safe_since = since_epoch.max(0);
    let format = "%H%x1f%P%x1f%T%x1f%an%x1f%ae%x1f%at%x1f%ct%x1f%s%x1e";
    let pretty = format!("--pretty=format:{format}");
    let since = format!("--since=@{safe_since}");

    let log_output = run_git_command(
        repo_root,
        &[
            "log",
            "--all",
            "--topo-order",
            "--date-order",
            since.as_str(),
            pretty.as_str(),
        ],
    )?;

    parse_git_log_records(&log_output)
}

fn parse_git_log_records(log_output: &str) -> AppResult<Vec<ParsedCommit>> {
    let mut commits = Vec::new();
    for record in log_output.split('\u{1e}') {
        let trimmed = record.trim();
        if trimmed.is_empty() {
            continue;
        }

        let fields: Vec<&str> = trimmed.split('\u{1f}').collect();
        if fields.len() != 8 {
            return Err(AppError::Parse(format!(
                "unexpected commit log field count: {}",
                fields.len()
            )));
        }

        let parents = if fields[1].trim().is_empty() {
            Vec::new()
        } else {
            fields[1]
                .split_whitespace()
                .map(|item| item.to_string())
                .collect()
        };

        let author_time = fields[5]
            .parse::<i64>()
            .map_err(|_| AppError::Parse("invalid author_time".to_string()))?;
        let committer_time = fields[6]
            .parse::<i64>()
            .map_err(|_| AppError::Parse("invalid committer_time".to_string()))?;

        commits.push(ParsedCommit {
            sha: fields[0].to_string(),
            parents,
            tree_sha: fields[2].to_string(),
            author_name: fields[3].to_string(),
            author_email: fields[4].to_string(),
            author_time,
            committer_time,
            subject: fields[7].to_string(),
        });
    }

    Ok(commits)
}

pub fn load_refs(repo_root: &Path) -> AppResult<Vec<ParsedRef>> {
    let raw_refs = run_git_command(
        repo_root,
        &["show-ref", "--heads", "--tags", "--dereference"],
    )?;
    let mut refs = Vec::new();

    for line in raw_refs.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let mut parts = trimmed.split_whitespace();
        let target_sha = parts.next().unwrap_or_default().to_string();
        let ref_name = parts.next().unwrap_or_default().to_string();
        if target_sha.is_empty() || ref_name.is_empty() {
            continue;
        }

        let ref_type = if ref_name.starts_with("refs/heads/") {
            "head"
        } else if ref_name.starts_with("refs/tags/") {
            "tag"
        } else {
            "other"
        };

        refs.push(ParsedRef {
            ref_name,
            target_sha,
            ref_type: ref_type.to_string(),
        });
    }

    Ok(refs)
}

pub fn load_file_changes_for_commits(
    repo_root: &Path,
    commit_shas: &[String],
) -> AppResult<Vec<ParsedFileChange>> {
    if commit_shas.is_empty() {
        return Ok(Vec::new());
    }

    let mut changes = Vec::new();
    for chunk in commit_shas.chunks(200) {
        let mut owned_args = vec![
            "show".to_string(),
            "--numstat".to_string(),
            "--format=%x1e%H".to_string(),
            "--no-color".to_string(),
            "--no-ext-diff".to_string(),
        ];

        for sha in chunk {
            owned_args.push(sha.clone());
        }

        let args: Vec<&str> = owned_args.iter().map(String::as_str).collect();
        let output = run_git_command(repo_root, &args)?;
        changes.extend(parse_numstat_output(&output));
    }

    Ok(changes)
}

fn parse_numstat_output(raw_output: &str) -> Vec<ParsedFileChange> {
    let mut current_commit_sha: Option<String> = None;
    let mut changes = Vec::new();

    for line in raw_output.lines() {
        if let Some(sha) = line.strip_prefix('\u{1e}') {
            let sha = sha.trim();
            if !sha.is_empty() {
                current_commit_sha = Some(sha.to_string());
            }
            continue;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Some(commit_sha) = current_commit_sha.as_ref() else {
            continue;
        };

        let mut fields = line.splitn(3, '\t');
        let Some(additions_raw) = fields.next() else {
            continue;
        };
        let Some(deletions_raw) = fields.next() else {
            continue;
        };
        let Some(file_path_raw) = fields.next() else {
            continue;
        };

        let Some((additions, additions_binary)) = parse_numstat_count(additions_raw) else {
            continue;
        };
        let Some((deletions, deletions_binary)) = parse_numstat_count(deletions_raw) else {
            continue;
        };

        let file_path = file_path_raw.trim();
        if file_path.is_empty() {
            continue;
        }

        let binary = additions_binary || deletions_binary;
        let change_type = classify_change_type(file_path, additions, deletions, binary);
        changes.push(ParsedFileChange {
            commit_sha: commit_sha.clone(),
            file_path: file_path.to_string(),
            additions,
            deletions,
            change_type,
        });
    }

    changes
}

fn parse_numstat_count(value: &str) -> Option<(i64, bool)> {
    if value == "-" {
        return Some((0, true));
    }

    value.parse::<i64>().ok().map(|parsed| (parsed, false))
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

fn run_git_command(repo_root: &Path, args: &[&str]) -> AppResult<String> {
    let output = Command::new("git")
        .arg("--no-pager")
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if is_expected_empty_output(&stderr) {
            return Ok(String::new());
        }

        return Err(AppError::GitCommand(format!(
            "{} | args: {:?}",
            stderr, args
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn is_expected_empty_output(stderr: &str) -> bool {
    stderr.contains("No references") || stderr.contains("does not have any commits yet")
}

#[cfg(test)]
mod tests {
    use super::{parse_numstat_output, run_git_command};
    use std::path::Path;

    #[test]
    fn unknown_repo_returns_error() {
        let result = run_git_command(Path::new("c:/non-existent/repo"), &["status"]);
        assert!(result.is_err());
    }

    #[test]
    fn parses_numstat_records() {
        let raw =
            "\u{1e}aaaa\n10\t2\tsrc/lib.rs\n-\t-\tassets/logo.png\n\u{1e}bbbb\n1\t0\told => new\n";
        let parsed = parse_numstat_output(raw);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].commit_sha, "aaaa");
        assert_eq!(parsed[0].change_type, "modify");
        assert_eq!(parsed[1].change_type, "binary");
        assert_eq!(parsed[2].change_type, "rename");
    }
}
