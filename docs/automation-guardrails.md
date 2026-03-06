# Automation Guardrails

Date: 2026-03-07

## Goal

Prevent CI/runtime script failures caused by hidden platform assumptions while keeping delivery speed and performance work unblocked.

## Design Principles

1. Platform intent must be explicit in every script.
2. Portable scripts must avoid OS-specific command/runtime assumptions.
3. CI must probe automation contracts before expensive build/test stages.
4. Guardrails should target reliability classes, not one-off incidents.

## Script Platform Contract

Every script in `scripts/*.ps1` must declare one metadata line near the top:

1. `# platform: any` for cross-platform scripts.
2. `# platform: windows` for Windows-only scripts.

`scripts/enforce-guardrails.ps1` enforces this contract.

## Portable Script Policy (`# platform: any`)

Portable scripts are rejected if they use brittle OS-specific primitives:

1. direct `$env:USERPROFILE` dependence
2. direct `cmd /c`
3. hardcoded `/bin/bash`
4. hardcoded `;` PATH separator writes
5. Windows-only `nul` redirection patterns

These are class-level checks, not tied to a single historical failure.

## CI Automation Guardrails

`Rust CI` includes:

1. `Guardrail Policy Check` as an early hard gate.
2. `Script Contract Probe` via `./scripts/perf-budget.ps1 -ProbeOnly` on both OS runners.
3. `Performance Budget` execution on Ubuntu (release perf gate).

This ordering fails fast on automation regressions before expensive lanes.

## Performance Script Resilience Contract

`scripts/perf-budget.ps1` must keep three properties:

1. `-ProbeOnly` mode for zero-cost contract validation in CI.
2. host execution path for normal fast environments.
3. resilient fallback path when host policy blocks test binaries (for example Windows Application Control), with the same perf test executed in Docker.

`scripts/enforce-guardrails.ps1` verifies this contract so reliability does not depend on local machine policy details.

## Non-Goals

1. No requirement to make Windows-only scripts cross-platform.
2. No slowdown of traversal/runtime algorithm paths due to automation checks.
3. No policy that blocks performance-oriented refactors by default.

## References

1. GitHub workflow shell behavior (`cmd` default on Windows, different defaults elsewhere): https://docs.github.com/en/actions/writing-workflows/workflow-syntax-for-github-actions
2. GitHub-hosted runner path/env guidance (`HOME`, `GITHUB_WORKSPACE`): https://docs.github.com/en/actions/reference/github-hosted-runners-reference
3. Building and testing PowerShell in GitHub Actions (`pwsh`): https://docs.github.com/actions/automating-builds-and-tests/building-and-testing-powershell
4. PowerShell automatic variables (`$IsWindows`): https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_automatic_variables
5. PowerShell environment variables across platforms: https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_environment_variables
6. PSScriptAnalyzer GitHub Action reference: https://github.com/microsoft/psscriptanalyzer-action
