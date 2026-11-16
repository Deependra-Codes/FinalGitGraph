//! Repository guardrails, ported from `scripts/enforce-guardrails.ps1` (ADR 0020).

mod contracts;
mod docs;
mod scripts;
mod structure;
mod tamper;
mod text;

use std::path::Path;

pub fn run(repo: &Path) -> Result<(), String> {
    println!("Checking repository guardrails...");
    let failures = check(repo)?;
    for failure in &failures {
        println!("[FAIL] {failure}");
    }
    if failures.is_empty() {
        println!("Guardrail check passed.");
        Ok(())
    } else {
        Err(format!(
            "Guardrail check failed with {} issue(s).",
            failures.len()
        ))
    }
}

fn check(repo: &Path) -> Result<Vec<String>, String> {
    let mut failures = Vec::new();
    structure::check(repo, &mut failures)?;
    contracts::check(repo, &mut failures)?;
    docs::check(repo, &mut failures);
    scripts::check(repo, &mut failures)?;
    tamper::check(repo, &mut failures)?;
    Ok(failures)
}
