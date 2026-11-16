use std::fs;
use std::path::Path;

use super::text::{contains, read};

const REQUIRED_DOCS: [&str; 22] = [
    "docs/engineering-system.md",
    "docs/from-scratch-system.md",
    "docs/ai-change-survival-playbook.md",
    "docs/standards/README.md",
    "docs/standards/AGENT_GUARDRAIL_SYSTEM.md",
    "docs/standards/ENGINEERING_EXECUTION_POLICY.md",
    "docs/standards/PERFORMANCE_AND_COMPLEXITY_DISCIPLINE.md",
    "docs/standards/REPO_SYNC_POLICY.md",
    "docs/standards/TYPE_TDD_SPEC_AGENTIC_DISCIPLINE.md",
    "docs/standards/DOCS_INTENT_RESEARCH_PATTERN.md",
    "docs/adr/README.md",
    "docs/adr/0006-streaming-libgit2-ingestion.md",
    "docs/adr/0007-bounded-rayon-ingestion-compute-single-writer-db.md",
    "docs/adr/0008-integer-edge-schema-for-graph-hot-path.md",
    "docs/adr/0009-in-memory-adjacency-graph-for-perf-harness.md",
    "docs/adr/0010-bounded-rename-detection-and-ingestion-phase-telemetry.md",
    "docs/adr/0011-core-file-change-ingestion-and-jit-enrichment.md",
    "docs/adr/0012-gitoxide-ingestion-adapter-migration.md",
    "docs/adr/0015-lmdb-csr-delta-storage.md",
    "docs/adr/0016-overlay-adjacency-and-lmdb-map-flooring.md",
    "docs/performance-engineering.md",
    "docs/automation-guardrails.md",
];

const STANDARDS: [(&str, &[&str]); 6] = [
    (
        "AGENT_GUARDRAIL_SYSTEM.md",
        &[
            "## Why This Exists",
            "## One-Line Policy",
            "## Required Behavior",
            "## Anti-Patterns",
            "## Enforcement",
        ],
    ),
    (
        "ENGINEERING_EXECUTION_POLICY.md",
        &[
            "## Why This Exists",
            "## One-Line Policy",
            "## Change Classes",
            "## Required Loop",
            "## Anti-Patterns",
        ],
    ),
    (
        "PERFORMANCE_AND_COMPLEXITY_DISCIPLINE.md",
        &[
            "## Why This Exists",
            "## One-Line Policy",
            "## Core Rules",
            "## Expected Artifacts For Scale-Sensitive Changes",
        ],
    ),
    (
        "REPO_SYNC_POLICY.md",
        &[
            "## Why This Exists",
            "## One-Line Policy",
            "## Canonical To Derived Map",
            "## Sync Rules",
            "## Enforcement",
        ],
    ),
    (
        "TYPE_TDD_SPEC_AGENTIC_DISCIPLINE.md",
        &[
            "## Why This Exists",
            "## Core Doctrine",
            "## Type-Driven Rules",
            "## TDD Rules",
            "## Spec Rules",
            "## Agentic Loop",
        ],
    ),
    (
        "DOCS_INTENT_RESEARCH_PATTERN.md",
        &["## Core Rule", "## Lifecycle", "## Quality Rules"],
    ),
];

/// Governance docs, the standards index and sections, and the ADR index.
pub fn check(repo: &Path, failures: &mut Vec<String>) {
    for doc in REQUIRED_DOCS {
        if !repo.join(doc).exists() {
            failures.push(format!("Required governance doc missing: {doc}"));
        }
    }

    let standards = repo.join("docs/standards");
    if let Some(index) = read(&standards.join("README.md")) {
        for (file, _) in STANDARDS {
            if !contains(&index, file) {
                failures.push(format!("docs/standards/README.md must list {file}"));
            }
        }
    }
    for (file, sections) in STANDARDS {
        let Some(content) = read(&standards.join(file)) else {
            continue;
        };
        for section in sections {
            if !contains(&content, section) {
                failures.push(format!(
                    "Standards file {file} is missing required section {section}"
                ));
            }
        }
    }

    let adr_dir = repo.join("docs/adr");
    let Some(adr_index) = read(&adr_dir.join("README.md")) else {
        return;
    };
    let mut adrs: Vec<String> = fs::read_dir(&adr_dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().is_file())
                .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
                .filter(|name| name.to_lowercase().ends_with(".md"))
                .filter(|name| !name.eq_ignore_ascii_case("README.md"))
                .collect()
        })
        .unwrap_or_default();
    adrs.sort();
    for adr in adrs {
        if !contains(&adr_index, &adr) {
            failures.push(format!("docs/adr/README.md must reference ADR file {adr}"));
        }
    }
}
