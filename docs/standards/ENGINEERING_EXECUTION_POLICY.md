# Engineering Execution Policy

Status: Active
Date: 2026-04-09
Owner: GitGraph Lab

## Why This Exists

The project needs one default way to convert intent into trustworthy code.

## One-Line Policy

For non-trivial work, use:

**Intent -> Research -> Spec -> Contracts and Types -> Tests -> Implementation -> Verification**

## Change Classes

1. Trivial: typo, comments, formatting, mechanical rename.
2. Non-trivial: behavior, API/schema, automation, performance, architecture, or boundary changes.

## Required Loop

1. Intent: problem, why now, out-of-scope.
2. Research: tradeoffs and constraints when uncertainty exists.
3. Spec: expected behavior, invariants, acceptance examples, test strategy.
4. Types/contracts: model important distinctions before broad logic.
5. Tests: add failing or contract tests for behavior changes.
6. Implementation: smallest coherent slice.
7. Verification: run required gates and document residual risk.

## Anti-Patterns

1. Code-first big drops with post-hoc justification.
2. Premature abstraction without a second concrete use case.
3. "It passes" while structure, ownership, or boundaries degraded.

## Repo-Native Gate Surface

1. `pnpm run guardrails`
2. `pnpm run dev:gate:fast`
3. `pnpm run dev:gate`
4. `pnpm run quality:perf` for traversal/runtime perf-sensitive changes
