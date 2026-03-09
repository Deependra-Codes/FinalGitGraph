# Agent Guardrails (Repository Local)

These rules are mandatory for any AI or human-assisted change in this repository.
The goal is simple: ship code that passes quality gates without duct-tape regressions.

## Hard Rules

1. Do not bypass quality gates.
2. Do not introduce `unsafe` without an ADR.
3. Do not reintroduce flat Rust modules in `src` root.
4. Do not add nested `Cargo.lock` files.
5. Do not merge changes with unresolved `TODO`, `FIXME`, or `dbg!` in Rust source.
6. Do not weaken or remove guardrail checks to make a failing change pass.
7. Do not submit docs-only guardrail changes without corresponding implementation work.

## Required Flow Per Change

1. Run `pnpm run guardrails`.
2. Run `pnpm run dev:gate:fast` while iterating.
3. Run `pnpm run dev:gate` before PR/merge.
4. If graph traversal/runtime perf behavior changed, run `pnpm run quality:perf`.
5. If architecture boundary changes, add/update ADR in `docs/adr`.

## First 5 Minutes Checklist (AI + Human)

1. Read this file (`AGENTS.md`) and `docs/automation-guardrails.md`.
2. Read `docs/engineering-system.md` and `docs/from-scratch-system.md`.
3. Read `docs/ai-change-survival-playbook.md` before touching architecture-sensitive paths.
4. Check `git status --short` and avoid reverting unrelated user changes.
5. Identify touched layer(s) before writing code.

## Rust Layering Contract

1. `application/*`: orchestration and command entry points.
2. `adapters/*`: external I/O boundaries (Git, DB).
3. `domain/*`: domain models and DTOs.
4. `support/*`: shared technical utilities.

Placement decision rule:

1. IO boundary logic belongs in `adapters`.
2. Orchestration and flow control belong in `application`.
3. Data contracts and typed DTOs belong in `domain`.
4. Reusable technical helpers belong in `support`.

## Delivery Contract (What "Done" Means)

1. Guardrails pass locally.
2. Fast dev gate passes during iteration.
3. Full dev gate passes before merge.
4. New/changed behavior has tests in the appropriate layer.
5. Architecture-impacting changes include an ADR update.
6. No quality-marker debt (`TODO`, `FIXME`, `dbg!`, duct-tape comments, lint-bypass attributes).

## High-Risk Paths (Require Extra Care)

1. `apps/desktop/src-tauri/src/adapters/git.rs` ingestion and diff extraction path.
2. `apps/desktop/src-tauri/src/adapters/db.rs` hot graph query + ingestion persistence path.
3. `apps/desktop/src-tauri/src/application/indexer.rs` transaction and pipeline orchestration.
4. `apps/desktop/src-tauri/src/application/query_graph_cache.rs` in-memory graph hydration/query path.
5. `apps/desktop/src-tauri/src/bin/perf-report.rs` telemetry contract consumed by perf gates.
6. `scripts/*.ps1` automation contracts and cross-platform behavior.

## Enforcement

1. CI runs guardrail checks and quality gates.
2. `scripts/enforce-guardrails.ps1` blocks policy drift and architectural regressions.
3. PRs must satisfy all required checks before merge.
