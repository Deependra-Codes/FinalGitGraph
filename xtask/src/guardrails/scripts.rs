use std::fs;
use std::path::Path;

use super::text::{compile, has_extension, has_match, read, rel};

const PLATFORM: &str = r"^\s*#\s*platform:\s*(any|windows)\s*$";
const PORTABLE_FORBIDDEN: [(&str, &str); 4] = [
    (
        r"\bcmd\s*/c\b",
        "portable scripts must not use cmd /c directly",
    ),
    ("/bin/bash", "portable scripts must not hardcode /bin/bash"),
    (
        r#"\$env:Path\s*=\s*".*;.*\$env:Path"#,
        "portable scripts must not hardcode ';' path separator",
    ),
    (
        r"[12]>\s*nul\b",
        "portable scripts must not rely on Windows null redirection",
    ),
];
const REQUIRED_PERF_FILES: [&str; 6] = [
    "scripts/perf-budget.ps1",
    "scripts/perf-report.ps1",
    "scripts/perf-ingestion-budget.ps1",
    "scripts/layer-test-matrix.ps1",
    "apps/desktop/src-tauri/src/bin/perf-report.rs",
    ".github/workflows/rust-ci.yml",
];

/// PowerShell script metadata and portability, plus the files the perf gates need.
/// Phase 4 replaces the script checks when the scripts are deleted (ADR 0020).
pub fn check(repo: &Path, failures: &mut Vec<String>) -> Result<(), String> {
    let dir = repo.join("scripts");
    if dir.is_dir() {
        let platform = compile(PLATFORM)?;
        let mut scripts: Vec<_> = fs::read_dir(&dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.is_file() && has_extension(path, &["ps1"]))
                    .collect()
            })
            .unwrap_or_default();
        scripts.sort();
        for script in scripts {
            let relative = rel(repo, &script);
            let content = read(&script).unwrap_or_default();
            let declared = content.lines().take(8).find_map(|line| {
                platform
                    .captures(line)
                    .and_then(|captures| captures.get(1))
                    .map(|value| value.as_str().to_lowercase())
            });
            let Some(declared) = declared else {
                failures.push(format!(
                    "Script missing platform metadata ('# platform: any|windows'): {relative}"
                ));
                continue;
            };
            if declared != "any" || relative == "scripts/enforce-guardrails.ps1" {
                continue;
            }
            let mut violations: Vec<&str> = Vec::new();
            if has_match(&content, r"\$env:USERPROFILE")? && !has_match(&content, r"\$env:HOME")? {
                violations
                    .push("portable scripts using USERPROFILE must also provide HOME fallback");
            }
            for (pattern, message) in PORTABLE_FORBIDDEN {
                if has_match(&content, pattern)? && !violations.contains(&message) {
                    violations.push(message);
                }
            }
            if !violations.is_empty() {
                failures.push(format!(
                    "Portable script policy violation in {relative}: {}",
                    violations.join("; ")
                ));
            }
        }
    } else {
        failures.push(format!("Missing scripts directory: {}", dir.display()));
    }

    for file in REQUIRED_PERF_FILES {
        if !repo.join(file).exists() {
            failures.push(format!("Required performance file missing: {file}"));
        }
    }
    Ok(())
}
