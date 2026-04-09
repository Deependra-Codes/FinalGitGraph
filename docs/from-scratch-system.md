# From-Scratch System (AI + Human Smooth Mode)

Date: 2026-03-07

## Objective

Run this project with zero duct-tape by default:

1. deterministic tooling
2. deterministic code placement
3. deterministic quality gates
4. deterministic release lanes

## Industry Operating Model

Use a hybrid two-lane model:

1. Backend lane (Rust quality): Docker Linux
2. Desktop lane (Tauri/WebView2 runtime): Windows host/runner

Why:

1. Rust code quality checks are stable in containerized Linux.
2. Tauri desktop runtime behavior and packaging are platform-specific and need Windows validation.
3. On this machine, Windows Code Integrity policy blocks host Rust build scripts (`os error 4551`), so Docker is mandatory for backend lane.

## Project Structure Contract

Rust core crate layout (`apps/desktop/src-tauri/src`):

1. `application/`: use-cases, orchestration, command APIs
2. `adapters/`: Git + DB boundaries
3. `domain/`: domain models and types
4. `support/`: cross-cutting technical utilities

Placement rule:

1. If code touches disk/process/network/DB, it belongs in `adapters`.
2. If code coordinates steps, it belongs in `application`.
3. If code defines business data contracts, it belongs in `domain`.
4. If code is generic utility for all layers, it belongs in `support`.

## One-Command Dev Loop

Default (recommended):

1. `pnpm run guardrails`
2. `pnpm run dev:gate:fast`

Available commands:

1. `pnpm run dev:gate` (Docker backend full gate)
2. `pnpm run dev:gate:fast` (Docker backend fast gate)
3. `pnpm run dev:gate:host` (host backend full gate)
4. `pnpm run dev:gate:host:fast` (host backend fast gate)
5. `pnpm run safe:fast` (guardrails + fast gate)
6. `pnpm run safe:full` (guardrails + full gate)

## CI Contract

`Rust CI` runs backend quality on:

1. `ubuntu-latest`
2. `windows-latest`

`Dependency Audit` runs RustSec advisory checks on PR/push/schedule.

Merge requirement:

1. both CI workflows green
2. PR checklist completed
3. ADR added if architecture boundary changed

## AI + Human Collaboration Contract

For every change:

1. classify change as feature/fix/refactor
2. identify touched layer(s) before coding
3. apply smallest valid diff
4. run `dev:gate:fast` during iteration
5. run full gate before PR
6. update docs/ADR in same PR when architecture changes
7. for non-trivial work, follow `docs/standards/ENGINEERING_EXECUTION_POLICY.md`
8. for architecture-sensitive paths, read `docs/ai-change-survival-playbook.md` first

## Phase 0 Exit Criteria

Phase 0 is complete only when:

1. repo has deterministic toolchain + lockfile + lint policy
2. backend quality gate is one-command and reproducible
3. module boundaries are documented and enforced by practice
4. CI lanes reflect local operating model
5. architecture decisions are captured by ADR

## Phase 1 Start Criteria

Start feature work only after Phase 0 is stable for at least 3 consecutive PRs.

Phase 1 initial scope (completed):

1. typed command surface and DTO contracts
2. incremental indexing checkpoint model
3. deterministic integration fixtures

Phase 2 kickoff reference:

1. `docs/phase-2-kickoff.md`

## References

1. Cargo workspaces and shared lockfile:
   - https://doc.rust-lang.org/cargo/reference/workspaces.html
2. Workspace lint inheritance:
   - https://doc.rust-lang.org/cargo/reference/workspaces.html#the-lints-table
3. Rust module system:
   - https://doc.rust-lang.org/book/ch07-02-defining-modules-to-control-scope-and-privacy.html
4. Rustup toolchain override:
   - https://rust-lang.github.io/rustup/overrides.html
5. Tauri v2 prerequisites:
   - https://v2.tauri.app/start/prerequisites/
6. Tauri v2 project structure:
   - https://v2.tauri.app/start/project-structure/
7. Tauri capability security model:
   - https://v2.tauri.app/security/capabilities-for-windows-and-webviews/
8. Node Corepack:
   - https://nodejs.org/docs/latest-v24.x/api/corepack.html
9. pnpm installation and Corepack usage:
   - https://pnpm.io/installation
10. GitHub Actions hardening guidance:
   - https://docs.github.com/en/actions/security-for-github-actions/security-guides/security-hardening-for-github-actions
11. RustSec:
   - https://rustsec.org/
