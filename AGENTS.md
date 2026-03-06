# Agent Guardrails (Repository Local)

These rules are mandatory for any AI or human-assisted change in this repository.

## Hard Rules

1. Do not bypass quality gates.
2. Do not introduce `unsafe` without an ADR.
3. Do not reintroduce flat Rust modules in `src` root.
4. Do not add nested `Cargo.lock` files.
5. Do not merge changes with unresolved `TODO`, `FIXME`, or `dbg!` in Rust source.

## Required Flow Per Change

1. Run `pnpm run guardrails`.
2. Run `pnpm run dev:gate:fast` while iterating.
3. Run `pnpm run dev:gate` before PR/merge.
4. If architecture boundary changes, add/update ADR in `docs/adr`.

## Rust Layering Contract

1. `application/*`: orchestration and command entry points.
2. `adapters/*`: external I/O boundaries (Git, DB).
3. `domain/*`: domain models and DTOs.
4. `support/*`: shared technical utilities.

## Enforcement

1. CI runs guardrail checks and quality gates.
2. PRs must satisfy all required checks before merge.
