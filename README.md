# GitGraph Lab

This repository now contains the Phase-1 kickoff foundation for GitGraph Lab:

1. Rust core scaffolding under `apps/desktop/src-tauri`
2. LMDB migration pipeline with the SDD baseline schema
3. Repository scan and commit indexing foundation using safe Git plumbing command execution
4. Engineering system and governance baseline in `docs/engineering-system.md`
5. Industry approach and Phase 2 plan in `docs/industry-approach.md`
6. Infra baseline and setup policy in `docs/infra-baseline.md`
7. From-scratch operating model in `docs/from-scratch-system.md`
8. Research log behind decisions in `docs/research-foundations-2026-03-07.md`
9. GitHub branch protection checklist in `docs/branch-protection-setup.md`
10. Engineering discipline standards in `docs/standards/README.md`
11. Architecture-sensitive AI playbook in `docs/ai-change-survival-playbook.md`

## Quick start

```powershell
cd apps/desktop/src-tauri
cargo run -- scan <path-to-git-repo>
cargo run -- index <path-to-git-repo>
cargo run -- index <path-to-git-repo> incremental
cargo run -- window <path-to-git-repo> 50
cargo run -- traverse <path-to-git-repo> <start-sha> bfs parents 200
cargo run -- path <path-to-git-repo> <from-sha> <to-sha> both
```

`index` output now includes `file_changes` count along with commits/edges/refs.

Default DB location: `.gitgraph-lab/gitgraph-lab.db` (relative to current working directory).

## Windows C++ Toolchain Shell

If `where cl` is empty in a normal PowerShell session, load Visual Studio dev env for this session:

```powershell
. .\scripts\enter-vs-dev-shell.ps1
where cl
```

Use x86 mode if needed:

```powershell
. .\scripts\enter-vs-dev-shell.ps1 -Arch x86
```

This script also sets `CARGO_TARGET_DIR` to `%USERPROFILE%\.cargo-target\gitgraph-lab` to avoid App Control/OneDrive execution blocks (`os error 4551`).

## Preflight Check

Run a one-shot environment check before development/testing:

```powershell
.\scripts\preflight.ps1
```

`preflight.ps1` validates Git, Rust, Node/pnpm, MSVC `cl.exe`, `cargo-tauri`, and WebView2 runtime.

## Guardrail Check (Mandatory)

```powershell
pnpm run guardrails
```

Automation policy reference: `docs/automation-guardrails.md`

Combined safe commands:

```powershell
pnpm run safe:fast
pnpm run safe:full
```

## Quality Gate (Required Before Merge)

```powershell
.\scripts\quality-gate.ps1
```

Fast loop during coding:

```powershell
.\scripts\quality-gate.ps1 -Fast
```

You can also run via pnpm scripts:

```powershell
pnpm run quality:rust
pnpm run quality:rust:fast
```

Unified one-command dev loop (recommended):

```powershell
pnpm run dev:gate:fast
pnpm run dev:gate
```

Graph traversal performance budget gate:

```powershell
pnpm run quality:perf
pnpm run quality:perf:ingestion
```

Repository-scale performance report (build/execute split + phase timing + RAM + in-memory adjacency benchmark):

```powershell
pnpm run perf:report
pnpm run perf:report:smoke
pnpm run perf:report:large
pnpm run perf:report:25k
pnpm run perf:report:25k:precondition
pnpm run perf:report:90k
pnpm run perf:ingestion:25k
pnpm run perf:ingestion:90k
```

Layered throttling matrix (section-wise test lanes + RAM-aware perf smoke metrics):

```powershell
pnpm run test:layers
```

Host fallback variants:

```powershell
pnpm run dev:gate:host:fast
pnpm run dev:gate:host
```

If Windows App Control blocks local Rust build scripts (`os error 4551`), run the backend gate in Docker (Linux):

```powershell
pnpm run quality:rust:docker
```

Fast loop:

```powershell
pnpm run quality:rust:docker:fast
```

Note: first Docker run may download Rust components; subsequent runs are cached.

## Architecture Decisions

Use ADRs for architecture-impacting changes:

1. Process: `docs/adr/README.md`
2. First accepted ADR: `docs/adr/0001-rust-workspace-governance.md`

## Pnpm Version Pin

The repository pins pnpm via `packageManager` in `package.json`.

```powershell
corepack enable
pnpm --version
```
