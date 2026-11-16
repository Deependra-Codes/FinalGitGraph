use std::path::Path;

use super::text::compile;
use crate::run;

const POLICY_FILES: [&str; 6] = [
    "scripts/enforce-guardrails.ps1",
    "scripts/perf-budget.ps1",
    ".github/workflows/rust-ci.yml",
    "README.md",
    "CONTRIBUTING.md",
    "package.json",
];
const IMPLEMENTATION_FILES: [&str; 2] = ["apps/desktop/src-tauri/Cargo.toml", "Cargo.toml"];
const IMPLEMENTATION_PATTERNS: [&str; 3] = [
    r"^apps/desktop/src-tauri/src/.+\.rs$",
    r"^apps/desktop/src-tauri/tests/.+\.rs$",
    concat!(
        r"^apps/desktop/src-tauri/migrations/.+\.",
        "s",
        "q",
        "l",
        "$"
    ),
];
/// Guardrail rules are policy; every other xtask path is tooling, which counts as implementation.
const GUARDRAIL_RULES: &str = "xtask/src/guardrails/";

/// Blocks policy changes (docs, guardrails, CI) that arrive without implementation changes.
pub fn check(repo: &Path, failures: &mut Vec<String>) -> Result<(), String> {
    let changed = changed_files(repo);
    if changed.is_empty() {
        return Ok(());
    }
    let mut policy = false;
    let mut implementation = false;
    for path in &changed {
        policy |= is_policy(path);
        implementation |= is_implementation(path)?;
    }
    if policy && !implementation {
        failures.push(format!(
            "Docs/guardrails changes without implementation changes are blocked. Changed files: {}",
            changed.join(", ")
        ));
    }
    Ok(())
}

/// Same order as the PowerShell script: local changes, then the PR range, then the last commit.
fn changed_files(repo: &Path) -> Vec<String> {
    let mut changed = git_lines(repo, &["diff", "--name-only", "--", "."]);
    changed.extend(git_lines(
        repo,
        &["diff", "--name-only", "--cached", "--", "."],
    ));
    let base = std::env::var("GITHUB_BASE_REF").unwrap_or_default();
    let pull_request = std::env::var("GITHUB_EVENT_NAME").is_ok_and(|name| name == "pull_request");
    if changed.is_empty() && pull_request && !base.trim().is_empty() {
        let fetched = run::git(repo)
            .args(["fetch", "--no-tags", "--depth=200", "origin", base.as_str()])
            .output()
            .is_ok_and(|output| output.status.success());
        if fetched {
            changed.extend(git_lines(
                repo,
                &["diff", "--name-only", &format!("origin/{base}...HEAD")],
            ));
        }
    }
    let has_parent = run::git(repo)
        .args(["rev-parse", "--verify", "--quiet", "HEAD~1"])
        .output()
        .is_ok_and(|output| output.status.success());
    if changed.is_empty() && has_parent {
        changed.extend(git_lines(repo, &["diff", "--name-only", "HEAD~1..HEAD"]));
    }
    let mut normalized: Vec<String> = changed
        .iter()
        .map(|path| {
            let path = path.trim().replace('\\', "/");
            path.strip_prefix("./").map(str::to_string).unwrap_or(path)
        })
        .filter(|path| !path.is_empty())
        .collect();
    normalized.sort_by_key(|path| path.to_lowercase());
    normalized.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    normalized
}

fn git_lines(repo: &Path, args: &[&str]) -> Vec<String> {
    run::stdout(run::git(repo).args(args))
        .map(|text| text.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

fn is_policy(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.starts_with("docs/")
        || lower.starts_with(GUARDRAIL_RULES)
        || POLICY_FILES
            .iter()
            .any(|file| file.eq_ignore_ascii_case(path))
}

fn is_implementation(path: &str) -> Result<bool, String> {
    let lower = path.to_lowercase();
    if lower.starts_with("xtask/") && !lower.starts_with(GUARDRAIL_RULES) {
        return Ok(true);
    }
    if IMPLEMENTATION_FILES
        .iter()
        .any(|file| file.eq_ignore_ascii_case(path))
    {
        return Ok(true);
    }
    for pattern in IMPLEMENTATION_PATTERNS {
        if compile(pattern)?.is_match(path) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::{is_implementation, is_policy};

    #[test]
    fn guardrail_rules_are_policy_and_other_xtask_code_is_implementation() {
        assert!(is_policy("xtask/src/guardrails/contracts.rs"));
        assert_eq!(
            is_implementation("xtask/src/guardrails/contracts.rs"),
            Ok(false)
        );
        assert!(!is_policy("xtask/src/gate.rs"));
        assert_eq!(is_implementation("xtask/src/gate.rs"), Ok(true));
        assert_eq!(is_implementation("xtask/Cargo.toml"), Ok(true));
    }

    #[test]
    fn keeps_the_powershell_classification() {
        assert!(is_policy("docs/adr/README.md"));
        assert!(is_policy("Scripts/Enforce-Guardrails.ps1"));
        assert!(!is_policy("scripts/dev-loop.ps1"));
        assert_eq!(
            is_implementation("apps/desktop/src-tauri/src/lib.rs"),
            Ok(true)
        );
        assert_eq!(
            is_implementation("apps/desktop/src-tauri/tests/x.rs"),
            Ok(true)
        );
        assert_eq!(is_implementation("Cargo.toml"), Ok(true));
        assert_eq!(is_implementation("Cargo.lock"), Ok(false));
        assert_eq!(is_implementation("scripts/dev-loop.ps1"), Ok(false));
    }
}
