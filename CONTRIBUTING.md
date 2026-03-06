# Contributing

## Environment

1. Run `.\scripts\preflight.ps1` once per shell session.
2. Use `x64` shell by default (`-Arch x64`).
3. Do not bypass toolchain pinning from `rust-toolchain.toml`.
4. Keep `CARGO_TARGET_DIR` outside OneDrive (`%USERPROFILE%\.cargo-target\gitgraph-lab` default from scripts).

## Standard Development Loop

1. Run `pnpm run guardrails`.
2. Use `pnpm run dev:gate:fast` during active development.
3. Run `pnpm run dev:gate` before opening PR / merge.
4. If Windows policy blocks local build scripts (`os error 4551`), keep Docker backend lane as default.

## Coding Rules

1. No `unsafe` without ADR approval.
2. No production `unwrap`/`expect`.
3. Keep adapters (I/O) separated from orchestration logic.
4. Add tests for behavioral changes.
5. Any architecture-impacting change requires an ADR in `docs/adr`.

## CI Contract

A change is acceptable only when both workflows pass:

1. `Rust CI`
2. `Dependency Audit`
