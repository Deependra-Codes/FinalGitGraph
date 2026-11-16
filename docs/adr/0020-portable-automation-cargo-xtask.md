# ADR 0020: Portable Automation with cargo xtask

Date: 2026-11-16
Status: Accepted (Phase 1 of 4)

## Context

Every quality gate ran through PowerShell, called as `powershell` from `package.json`. That command exists only on Windows. Several scripts required a D: drive, `dev:gate` ran in Docker by default, `scripts/preflight.ps1` checked only Windows tools, and CI had no macOS leg. No gate could run on a macOS or Linux workstation.

This is sub-project S2 of `docs/superpowers/specs/2026-11-16-cleanup-program-roadmap.md`. The full design is in `docs/superpowers/specs/2026-11-16-cargo-xtask-migration-design.md`.

## Decision

1. Replace the 13 PowerShell scripts and `package.json` with a Rust `xtask` workspace crate, reached through the cargo alias `xtask = "run --quiet --package xtask --"`. Prerequisites become Rust 1.94.0, a platform C toolchain and git 2.32 or later.
2. Delete instead of porting:
   - `package.json` and the Node/pnpm requirement;
   - the Docker gate lanes;
   - the D: drive rule (`doctor` warns about OneDrive target dirs on Windows instead);
   - `enter-vs-dev-shell.ps1`, `safe-install.ps1` and `faculty-server-sandbox.ps1`;
   - confirmed-dangling flags.
3. Port in four phases, one PR each. Run old and new side by side, and delete a script only after its xtask replacement matches it:

   | Phase | Scope |
   |---|---|
   | 1 | `doctor` and `gate`, plus a macOS CI leg |
   | 2 | guardrails, with a per-check mapping table in this ADR |
   | 3 | perf commands |
   | 4 | delete the scripts and switch AGENTS.md, docs and CI |
4. Keep every guardrail (AGENTS.md rule 6). Each of the 60 `Add-Failure` sites in `scripts/enforce-guardrails.ps1` is ported, re-targeted, or replaced by an equal or stronger check. The mapping table arrives in Phase 2.

## Phase 1 Results

- `cargo xtask gate [--fast]` runs the same four steps as `scripts/quality-gate.ps1`, in the same order:
  1. `cargo fmt --all -- --check`
  2. `cargo clippy --workspace --all-targets -- -D warnings`
  3. `cargo check --workspace --locked`
  4. `cargo test --workspace --locked`

  It stops at the first failure, and `--fast` skips the test step. It does not run guardrails yet, matching the old gate. Guardrails join in Phase 2.
- `cargo xtask doctor` replaces `scripts/preflight.ps1`. It checks rustc 1.94.0, git 2.32 or later, a working C compiler (vswhere on Windows) and `Cargo.lock`. It drops the MSVC `cl.exe`, WebView2 and `cargo-tauri` checks: the product has no Tauri dependency, and cc-rs finds MSVC through vswhere.

Gate parity was measured on 2026-11-16 on macOS 15.8.2 arm64, comparing `pwsh scripts/quality-gate.ps1 -SkipPreflight` with `cargo xtask gate`:

| Tree | Old exit (failing step) | New exit (failing step) |
|---|---|---|
| clean | 0 | 0 |
| rustfmt violation | 1 (rustfmt check) | 1 (fmt) |
| clippy `clone_on_copy` | 1 (clippy) | 1 (clippy) |
| failing integration test | 1 (cargo test) | 1 (test) |
| failing integration test, `-Fast` / `--fast` | 0 | 0 |

CI now runs `cargo xtask doctor` and `cargo xtask gate` on `ubuntu-latest`, `windows-latest` and `macos-latest`, alongside the old steps. The repository is public, so macOS runner minutes are free.

## Consequences

- During Phases 1 to 3, CI runs both the old steps and the xtask steps, which costs some extra CI time. Phase 4 removes the old steps.
- `xtask` has no dependencies in Phase 1. Phase 2 adds `regex` 1.12.3, which is already in `Cargo.lock`.
- The guardrails do not yet know about `xtask/`. Phase 2 classifies `xtask/**` as policy in the anti-tamper rule.

## Alternatives Considered

1. **Keep PowerShell and make it portable** by switching to `pwsh` and dropping the D: drive rule. This is a smaller change, but it keeps three languages and untestable scripts. The owner chose xtask.
2. **A Docker dev container for every gate.** Perf numbers from a VM do not represent native macOS or Windows, and native Windows and macOS would stay untested.
3. **A big-bang replacement with no parity runs.** A lost guardrail would be invisible, which breaks AGENTS.md rule 6.
