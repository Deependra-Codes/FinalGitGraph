# Infra Baseline (Industry-Grade, Easy-to-Operate)

Date: 2026-03-07

## Goal

Keep local setup simple and deterministic from day one for a critical project.

## Baseline Decisions

1. Pin Rust toolchain in repo with `rust-toolchain.toml`.
2. Keep root workspace `Cargo.lock` committed for reproducible builds.
3. Pin Node package manager version with `packageManager` in `package.json`.
4. Use Visual Studio developer environment loader for MSVC (`cl`, `INCLUDE`, `LIB`) instead of manually editing system PATH.
5. Keep a repeatable preflight command to verify environment readiness.
6. Enforce code quality in CI from start (`fmt`, `clippy`, `check`, `test`).
7. Maintain an explicit Phase 0 engineering system (`docs/engineering-system.md`) with ADR workflow.
8. Set `CARGO_TARGET_DIR` outside OneDrive path to avoid Windows App Control execution blocks (`os error 4551`).
9. Keep a Docker Linux quality-gate fallback for policy-restricted Windows hosts.
10. Expose one-command developer gate via `scripts/dev-loop.ps1`.

## How To Use

1. Open project root in PowerShell.
2. Load MSVC dev env in current shell:
   - `. .\scripts\enter-vs-dev-shell.ps1`
3. Run environment preflight:
   - `.\scripts\preflight.ps1`
4. Run Rust validation:
   - `.\scripts\quality-gate.ps1`

## Why This Is The Best Starting Approach

1. It is explicit and deterministic (toolchain/version pinning).
2. It avoids hidden machine drift (preflight + CI gates).
3. It matches official platform guidance for MSVC build environment handling.
4. It scales to team onboarding with minimal tribal knowledge.

## References (Primary)

1. Rustup overrides and toolchain file:
   - https://rust-lang.github.io/rustup/overrides.html
2. Tauri v2 setup and CLI flow:
   - https://v2.tauri.app/start/prerequisites/
   - https://v2.tauri.app/start/create-project/
3. Tauri v2 Cargo.lock guidance:
   - https://v2.tauri.app/start/migrate/from-tauri-1/
4. Microsoft C++ command-line build environment:
   - https://learn.microsoft.com/en-us/cpp/build/building-on-the-command-line
5. Visual Studio Developer PowerShell:
   - https://learn.microsoft.com/en-us/visualstudio/ide/reference/command-prompt-powershell
6. pnpm installation and Corepack:
   - https://pnpm.io/next/installation
7. Node.js Corepack:
   - https://nodejs.org/api/corepack.html
8. GitHub Actions workflow docs:
   - https://docs.github.com/en/actions/writing-workflows
