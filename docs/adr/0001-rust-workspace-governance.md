# ADR 0001: Rust Workspace Governance Baseline

Date: 2026-03-06
Status: Accepted

## Context

The project needs a reliable foundation before feature expansion. Current risk was process drift:

1. missing shared lockfile at workspace root
2. weak repo-level lint policy
3. no single quality-gate command
4. no explicit architecture decision workflow

For a critical Rust + Tauri system, this causes inconsistent local environments and compounding technical debt.

## Decision

Adopt a Phase 0 governance baseline:

1. root Cargo workspace with shared lints and root `Cargo.lock`
2. member crate inherits workspace lints via `[lints] workspace = true`
3. clippy policy disallows `unwrap`/`expect` in production code
4. standard quality gate script (`scripts/quality-gate.ps1`)
5. CI gates for format/lint/build/tests + dependency audit workflow
6. ADR process under `docs/adr`

## Consequences

Positive:

1. deterministic builds and reproducible CI
2. lower architecture drift across AI and human contributors
3. early prevention of common Rust quality regressions

Tradeoffs:

1. slightly stricter contribution process
2. additional upfront maintenance for lint and CI policies

## Alternatives Considered

1. Keep single-crate local conventions only.
   - Rejected because conventions without enforcement drift quickly.
2. Delay lint/CI hardening until after MVP features.
   - Rejected because debt would multiply during high-change phase.

## References

1. https://doc.rust-lang.org/cargo/reference/workspaces.html
2. https://doc.rust-lang.org/cargo/reference/workspaces.html#the-lints-table
3. https://doc.rust-lang.org/cargo/reference/manifest.html#the-rust-version-field
4. https://doc.rust-lang.org/clippy/lint_configuration.html
5. https://v2.tauri.app/start/project-structure/
