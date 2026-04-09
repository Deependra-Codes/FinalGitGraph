# AI Change Survival Playbook

Date: 2026-04-09

This playbook is mandatory context before touching architecture-sensitive paths.

## Trigger Paths

Treat changes under these paths as architecture-sensitive:

1. `apps/desktop/src-tauri/src/adapters/git.rs`
2. `apps/desktop/src-tauri/src/adapters/db.rs`
3. `apps/desktop/src-tauri/src/application/indexer.rs`
4. `apps/desktop/src-tauri/src/bin/perf-report.rs`
5. `scripts/*.ps1`

## Before Coding

1. Identify touched layer(s): `application`, `adapters`, `domain`, `support`.
2. Write a short intent note: goal, constraints, out-of-scope.
3. For non-trivial or perf-sensitive changes, write/update a mini-spec with invariants.
4. Confirm whether ADR update is required.
5. Confirm relevant quality/perf gates to run.

## While Coding

1. Prefer smallest coherent change set.
2. Keep DB writes single-owner and avoid cross-layer leakage.
3. Avoid new dependencies unless existing stack is insufficient.
4. Preserve hot-path complexity and memory discipline.
5. Add tests/regressions with behavior changes.

## Verification Order

1. `pnpm run guardrails`
2. `pnpm run dev:gate:fast`
3. `pnpm run dev:gate`
4. `pnpm run quality:perf` (when traversal/runtime perf behavior changed)
5. `pnpm run quality:perf:ingestion` (when ingestion behavior changed)

## Done Criteria

1. Gates pass.
2. No quality markers (`TODO`, `FIXME`, `dbg!`, duct-tape comments, lint bypass).
3. Specs/docs/ADR are updated for architecture-visible changes.
4. Residual risks are explicitly documented.
