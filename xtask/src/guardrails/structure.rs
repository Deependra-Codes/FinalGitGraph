use std::fs;
use std::path::Path;

use super::text::{has_extension, matching_lines, walk_files};

pub const SRC_ROOT: &str = "apps/desktop/src-tauri/src";
const LAYERS: [&str; 4] = ["application", "adapters", "domain", "support"];
const ROOT_RUST_FILES: [&str; 2] = ["lib.rs", "main.rs"];
const LEGACY_FLAT: [&str; 7] = [
    "app_error.rs",
    "clock.rs",
    "commands.rs",
    "db.rs",
    "git_adapter.rs",
    "indexer.rs",
    "models.rs",
];
const LEGACY_RUNTIME: [&str; 3] = [
    "application/graph_queries.rs",
    "application/query_graph_cache.rs",
    "application/indexer_pipeline.rs",
];
const MARKERS: &str = r"\b(TODO|FIXME)\b|\bdbg!\s*\(";
const DUCT_TAPE: &str =
    r"^\s*//\s*(HACK|WIP|WORKAROUND|TEMP(?:ORARY)?|QUICKFIX|DUCKTAPE|DUCTTAPE)\b";
const LINT_ALLOW: &str = r"#\s*\[\s*allow\s*\(\s*(clippy::|dead_code|unused_)";
// Spelled in pieces so the repository never contains the words this check bans.
const LEGACY_WORD: &str = concat!("s", "q", "l");
const LEGACY_ENGINE: &str = concat!("s", "q", "l", "ite");
const LEGACY_DRIVER: &str = concat!("ru", "s", "q", "l", "ite");
const TERM_ROOTS: [&str; 4] = [SRC_ROOT, "apps/desktop/src-tauri/tests", "scripts", "docs"];
const TERM_EXTENSIONS: [&str; 7] = ["rs", "ps1", "md", "toml", "yml", "yaml", "json"];

/// Repository layout and source hygiene. The code contracts skip on their own when the
/// source root is missing, because their files are inside it.
pub fn check(repo: &Path, failures: &mut Vec<String>) -> Result<(), String> {
    let root_lock = repo.join("Cargo.lock");
    if !root_lock.is_file() {
        failures.push("Root Cargo.lock is missing.".to_string());
    }
    // Skips target/ and .git/: a lock file there is never source (ADR 0020).
    let nested: Vec<String> = walk_files(repo, &[".git", "target"])
        .into_iter()
        .filter(|path| path.file_name().is_some_and(|name| name == "Cargo.lock"))
        .filter(|path| *path != root_lock)
        .map(|path| path.display().to_string())
        .collect();
    if !nested.is_empty() {
        failures.push(format!(
            "Nested Cargo.lock files must not exist. Found: {}",
            nested.join("; ")
        ));
    }
    migrations(repo, failures);
    relational_terms(repo, failures)?;

    let src = repo.join(SRC_ROOT);
    if !src.is_dir() {
        failures.push(format!("Missing Rust source root: {}", src.display()));
        return Ok(());
    }
    for layer in LAYERS {
        if !src.join(layer).exists() {
            failures.push(format!("Layer directory missing: {layer}"));
        }
    }
    let unexpected: Vec<String> = fs::read_dir(&src)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_file() && has_extension(path, &["rs"]))
                .filter_map(|path| path.file_name()?.to_str().map(str::to_string))
                .filter(|name| {
                    !ROOT_RUST_FILES
                        .iter()
                        .any(|allowed| allowed.eq_ignore_ascii_case(name))
                })
                .collect()
        })
        .unwrap_or_default();
    if !unexpected.is_empty() {
        failures.push(format!(
            "Unexpected root Rust files found: {}",
            unexpected.join(", ")
        ));
    }
    let legacy = present(&src, &LEGACY_FLAT);
    if !legacy.is_empty() {
        failures.push(format!(
            "Legacy flat modules should not exist: {}",
            legacy.join(", ")
        ));
    }

    let rust_files: Vec<_> = walk_files(&src, &[])
        .into_iter()
        .filter(|path| has_extension(path, &["rs"]))
        .collect();
    for (pattern, message) in [
        (MARKERS, "Forbidden markers found in Rust source tree at"),
        (
            DUCT_TAPE,
            "Duct-tape quality markers are blocked in Rust source tree",
        ),
        (
            LINT_ALLOW,
            "Lint-suppression duct-tape attributes are blocked in Rust source tree",
        ),
    ] {
        let hits = sample_hits(&rust_files, pattern)?;
        if !hits.is_empty() {
            failures.push(format!("{message}: {}", hits.join(", ")));
        }
    }

    let legacy_runtime = present(&src, &LEGACY_RUNTIME);
    if !legacy_runtime.is_empty() {
        failures.push(format!(
            "Legacy relational runtime modules must not exist: {}",
            legacy_runtime.join(", ")
        ));
    }
    Ok(())
}

fn present(dir: &Path, names: &[&str]) -> Vec<String> {
    names
        .iter()
        .filter(|name| dir.join(name).exists())
        .map(|name| name.to_string())
        .collect()
}

/// `path:line` of the first 10 matching lines across `files`.
fn sample_hits(files: &[std::path::PathBuf], pattern: &str) -> Result<Vec<String>, String> {
    let mut hits = Vec::new();
    for file in files {
        let Some(text) = super::text::read(file) else {
            continue;
        };
        for line in matching_lines(&text, pattern)? {
            hits.push(format!("{}:{line}", file.display()));
        }
    }
    hits.truncate(10);
    Ok(hits)
}

fn migrations(repo: &Path, failures: &mut Vec<String>) {
    let dir = repo.join("apps/desktop/src-tauri/migrations");
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && has_extension(path, &[LEGACY_WORD]))
        .filter_map(|path| path.file_name()?.to_str().map(str::to_string))
        .collect();
    names.sort();
    if !names.is_empty() {
        failures.push(format!(
            "Legacy relational migration files must be removed for LMDB runtime: {}",
            names.join(", ")
        ));
    }
}

fn relational_terms(repo: &Path, failures: &mut Vec<String>) -> Result<(), String> {
    let pattern = format!(r"({LEGACY_ENGINE}|{LEGACY_DRIVER}|\b{LEGACY_WORD}\b)");
    let files: Vec<_> = TERM_ROOTS
        .iter()
        .map(|root| repo.join(root))
        .filter(|root| root.is_dir())
        .flat_map(|root| walk_files(&root, &[]))
        .filter(|path| has_extension(path, &TERM_EXTENSIONS))
        .collect();
    let hits = sample_hits(&files, &pattern)?;
    if !hits.is_empty() {
        failures.push(format!(
            "Forbidden legacy relational terms detected. Sample: {}",
            hits.join(", ")
        ));
    }
    Ok(())
}
