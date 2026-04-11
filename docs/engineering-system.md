# Engineering System (No-Duct-Tape Baseline)

Date: 2026-03-07

## Goal

Create a deterministic, industry-grade Rust system from day zero so both humans and AI agents can ship without architecture drift.

## Discipline Standards

Repo-level engineering discipline is defined in `docs/standards/`:

1. `AGENT_GUARDRAIL_SYSTEM.md`
2. `ENGINEERING_EXECUTION_POLICY.md`
3. `PERFORMANCE_AND_COMPLEXITY_DISCIPLINE.md`
4. `REPO_SYNC_POLICY.md`
5. `TYPE_TDD_SPEC_AGENTIC_DISCIPLINE.md`
6. `DOCS_INTENT_RESEARCH_PATTERN.md`

## Phase Model

1. Phase 0 (now): engineering system and guardrails.
2. Phase 1: core product capabilities (indexing, DAG APIs, simulator primitives).
3. Phase 2: scale hardening (crate split, telemetry, perf budgets, security policies).

## Non-Negotiable Rules

1. Toolchain is pinned in-repo (`rust-toolchain.toml`).
2. Workspace lockfile is mandatory (`Cargo.lock` at repo root).
3. Workspace lint policy is inherited by every crate (`[lints] workspace = true`).
4. No `unsafe` code unless approved by ADR.
5. No `unwrap`/`expect` in production code (tests explicitly exempted in `clippy.toml`).
6. Every change passes one command before merge: `.\scripts\quality-gate.ps1`.
7. Security checks run continuously in CI (dependency audit workflow).
8. Build artifacts run from `%USERPROFILE%\.cargo-target\gitgraph-lab` (outside OneDrive) to avoid Windows App Control blocks.

## Repository Topology

```text
/
  Cargo.toml                  # workspace members + shared lint policy
  Cargo.lock                  # shared lockfile for deterministic builds
  rust-toolchain.toml         # pinned Rust toolchain/components/target
  clippy.toml                 # repo-wide lint behavior (including test exceptions)
  rustfmt.toml                # stable formatting style behavior
  .cargo/config.toml          # cargo aliases + target-dir normalization
  scripts/
    enter-vs-dev-shell.ps1    # loads MSVC env vars for Windows builds
    preflight.ps1             # environment readiness checks
    enforce-guardrails.ps1    # static repository policy checks
    quality-gate.ps1          # fmt + clippy + check + test gate
    quality-gate-docker.ps1   # dockerized backend quality gate
    dev-loop.ps1              # one-command development gate switch
  apps/desktop/src-tauri/     # current Rust product crate
  docs/adr/                   # architecture decision records
  docs/automation-guardrails.md
```

## Rust Architecture Contract

Current single-crate internal boundary contract:

1. `application/*`: orchestration and command entry points.
2. `adapters/*`: external I/O boundaries (`git`, database).
3. `domain/*`: serializable domain/application DTOs.
4. `support/*`: technical cross-cutting utilities and error surface.

Crate split trigger (Phase 2):

1. Split to `crates/domain`, `crates/adapters`, `crates/application` when:
2. build times become a regular bottleneck, or
3. adapter churn causes domain regressions, or
4. shared logic is needed by both CLI and Tauri command surfaces.

## Delivery Loop (AI + Human Safe)

1. `pnpm run guardrails`
2. `pnpm run dev:gate:fast` during iteration (Docker backend default).
3. `pnpm run dev:gate` before PR/merge.
4. `pnpm run quality:perf` for graph traversal performance-sensitive changes.
5. If host backend run is needed, use `dev:gate:host*` variants.
6. Implement change only after touched layer(s) are explicitly identified.

Definition of done:

1. format/lint/build/tests green locally and in CI
2. no hidden toolchain drift
3. architecture-impacting changes captured in ADR
4. no unchecked TODO/temporary scaffolding in merged path

## ADR Workflow

1. Any architecture change touching boundaries, data model, runtime model, or security requires an ADR.
2. Add a new file under `docs/adr` using the template in `docs/adr/README.md`.
3. ADR status lifecycle: `Proposed` -> `Accepted` -> `Superseded` (if replaced later).

## Performance Governance

1. Performance-sensitive graph query code follows `docs/performance-engineering.md`.
2. Guardrails reject legacy relational_query runtime modules and enforce LMDB + CSR contracts in active ingestion/query paths.
3. Rust CI includes dedicated `Performance Budget` gate for direction-optimizing BFS.
4. Rust CI includes cross-platform `Script Contract Probe` for automation script portability.
5. Guardrails enforce script platform metadata and portable-script policy.
6. Guardrails block docs/guardrail-only diffs when no implementation files are changed.
7. Any future identity/storage strategy change for graph query internals must be documented via ADR.

## Phase 2 (System Hardening)

1. Introduce crate-level boundaries (`domain`, `adapters`, `application`).
2. Add integration tests with ephemeral git repos and deterministic fixtures.
3. Add `cargo-deny` policy (`licenses`, `bans`, `sources`) and gate in CI.
4. Add tracing + metrics (`tracing` spans around indexing stages).
5. Add performance budgets (index throughput, memory ceiling, query p95).
6. Add Tauri capability policy files and command-level permission review.

## Source-Backed Notes

Direct from official docs:

1. Cargo workspaces share a single lockfile and output directory.
2. Workspace-level lints can be inherited by members.
3. `rust-version` is a first-class Cargo package field.
4. Tauri Windows prerequisites include WebView2 + Microsoft C++ Build Tools.
5. `src-tauri` can exist as a member of a Cargo workspace.
6. Tauri v2 introduces capability-based security configuration.
7. Corepack is the Node-supported way to pin and use package manager versions.

Inference applied for this repo:

1. We keep a root workspace lockfile (not per-crate lockfile) to enable deterministic multi-crate growth.
2. We enforce lint/test gates before feature expansion to avoid compounding debt in Phase 1.

## References

1. https://doc.rust-lang.org/cargo/reference/workspaces.html
2. https://doc.rust-lang.org/cargo/reference/workspaces.html#the-lints-table
3. https://doc.rust-lang.org/cargo/reference/manifest.html#the-rust-version-field
4. https://rust-lang.github.io/rustup/overrides.html
5. https://doc.rust-lang.org/clippy/lint_configuration.html
6. https://doc.rust-lang.org/edition-guide/rust-2024/rustfmt-style-edition.html
7. https://v2.tauri.app/start/prerequisites/
8. https://v2.tauri.app/start/project-structure/
9. https://v2.tauri.app/security/capabilities-for-windows-and-webviews/
10. https://learn.microsoft.com/en-us/cpp/build/building-on-the-command-line
11. https://learn.microsoft.com/en-us/visualstudio/ide/reference/command-prompt-powershell
12. https://nodejs.org/api/corepack.html
13. https://pnpm.io/installation
14. https://rustsec.org/
15. https://github.com/EmbarkStudios/cargo-deny

