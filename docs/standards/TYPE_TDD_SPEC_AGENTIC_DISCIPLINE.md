# Type, TDD, Spec, And Agentic Discipline

Status: Active
Date: 2026-04-09
Owner: GitGraph Lab

## Why This Exists

AI-speed execution must stay grounded in explicit behavior proof.

## Core Doctrine

1. Spec defines what the system must do.
2. Types define what the system may represent.
3. Tests prove behavior claims.
4. Agentic loops keep changes bounded and verifiable.

## Type-Driven Rules

1. Prefer enums/tagged states over string flags and parallel booleans.
2. Parse and validate at boundaries; use typed values internally.
3. Model meaningful distinctions explicitly (for example stale/fresh, exact/inferred).

## TDD Rules

1. Red-Green-Refactor for behavior changes.
2. Bug fixes begin with failing regression coverage.
3. Test contracts and invariants over implementation trivia.

## Spec Rules

1. Non-trivial behavior changes require a written mini-spec.
2. Spec must include scope, invariants, contract impact, and test strategy.
3. Scale-sensitive specs include workload and complexity notes.

## Agentic Loop

1. Read intent/spec.
2. Propose contract/type changes.
3. Add tests or verification targets.
4. Implement one bounded slice.
5. Run narrow then full gates.
6. Record verified behavior and residual risk.
