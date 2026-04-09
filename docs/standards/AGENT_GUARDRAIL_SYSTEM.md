# Agent Guardrail System

Status: Active
Date: 2026-04-09
Owner: GitGraph Lab

## Why This Exists

AI-assisted coding can accelerate delivery or amplify chaos.
This repository chooses disciplined acceleration.

## One-Line Policy

For non-trivial work, coding is a late step:

**understanding -> research -> spec -> types/contracts -> tests -> minimal implementation -> verification**

## Required Behavior

1. Research before architecture, algorithm, data-structure, or performance claims.
2. Prefer less code by default (delete, tighten types, or reshape boundaries first).
3. Keep hot-path and memory discipline explicit for scale-sensitive changes.
4. Use typed contracts to remove ambiguity before branching logic spreads.
5. Verify behavior with tests and repo quality gates before claiming done.
6. State what is confirmed vs inferred vs unknown.

## Anti-Patterns

1. Large generated code drops without a narrow spec.
2. Dependency additions without proving existing stack is insufficient.
3. Hot-path complexity regressions justified only by "tests passed".
4. Prompt-only behavior that is not captured in code/tests/docs.

## Enforcement

1. `AGENTS.md` defines the working contract for contributors and agents.
2. `scripts/enforce-guardrails.ps1` enforces required docs and policy sync checks.
3. `pnpm run guardrails`, `pnpm run dev:gate:fast`, and `pnpm run dev:gate` remain mandatory flow gates.
