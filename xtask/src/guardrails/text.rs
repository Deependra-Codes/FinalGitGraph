use std::fs;
use std::path::{Path, PathBuf};

use regex::{Regex, RegexBuilder};

/// Case-insensitive, like PowerShell `Select-String` and `-match`.
pub fn compile(pattern: &str) -> Result<Regex, String> {
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .map_err(|error| format!("invalid guardrail pattern `{pattern}`: {error}"))
}

/// 1-based numbers of lines matching `pattern`, scanning line by line like `Select-String`.
pub fn matching_lines(text: &str, pattern: &str) -> Result<Vec<usize>, String> {
    let regex = compile(pattern)?;
    Ok(text
        .lines()
        .enumerate()
        .filter(|(_, line)| regex.is_match(line))
        .map(|(index, _)| index + 1)
        .collect())
}

/// Whole-text match, like PowerShell `-match` on `Get-Content -Raw`.
pub fn has_match(text: &str, pattern: &str) -> Result<bool, String> {
    Ok(compile(pattern)?.is_match(text))
}

/// Case-insensitive substring test, like `-match [regex]::Escape(needle)`.
pub fn contains(text: &str, needle: &str) -> bool {
    text.to_lowercase().contains(&needle.to_lowercase())
}

pub fn has_line(text: &str, pattern: &str) -> Result<bool, String> {
    let regex = compile(pattern)?;
    Ok(text.lines().any(|line| regex.is_match(line)))
}

pub fn read(path: &Path) -> Option<String> {
    fs::read(path)
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// Every file under `root`, skipping directories named in `skip_dirs`.
pub fn walk_files(root: &Path, skip_dirs: &[&str]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let skipped = entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| skip_dirs.contains(&name));
                if !skipped {
                    pending.push(path);
                }
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// Repository-relative path with forward slashes.
pub fn rel(repo: &Path, path: &Path) -> String {
    path.strip_prefix(repo)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Case-insensitive file extension test, like PowerShell `-in`.
pub fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extensions
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(extension))
        })
}

#[cfg(test)]
mod tests {
    use super::{has_line, matching_lines};

    #[test]
    fn matches_per_line_and_case_insensitively() {
        let text = "first\n// todo later\r\nuse gix::Repository;\n";
        assert_eq!(has_line(text, r"\bTODO\b"), Ok(true));
        assert_eq!(has_line(text, r"^\s*use\s+gix::Repository;"), Ok(true));
        assert_eq!(has_line(text, r"^todo"), Ok(false));
        assert_eq!(matching_lines(text, "LATER|FIRST"), Ok(vec![1, 2]));
    }

    #[test]
    fn rejects_invalid_patterns() {
        assert!(has_line("x", "(").is_err());
    }
}
