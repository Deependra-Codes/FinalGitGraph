# ADR 0002: Internal Module Boundaries for Core Crate

Date: 2026-03-07
Status: Accepted

## Context

The project started as a flat Rust module layout. As features grow, flat layouts increase coupling and make AI/human changes drift across concerns.

We need clear placement rules before Phase 1 feature expansion.

## Decision

Adopt a layered internal module structure inside `apps/desktop/src-tauri/src`:

1. `application/*`: orchestration and command entry points.
2. `adapters/*`: external I/O boundaries (`git`, database).
3. `domain/*`: data models and domain-facing types.
4. `support/*`: shared technical utilities (`clock`, error types).

Exports are now organized through:

1. `application/mod.rs`
2. `adapters/mod.rs`
3. `domain/mod.rs`
4. `support/mod.rs`

## Consequences

Positive:

1. Predictable location for new code.
2. Lower accidental coupling.
3. Easier future split into workspace crates if needed.

Tradeoffs:

1. Slightly more module nesting.
2. Import paths become longer.

## Alternatives Considered

1. Keep flat layout and rely on conventions.
   - Rejected due high drift risk with rapid iteration.
2. Immediate multi-crate split.
   - Rejected for now to avoid early workspace complexity before feature maturity.

## References

1. https://doc.rust-lang.org/book/ch07-02-defining-modules-to-control-scope-and-privacy.html
2. https://doc.rust-lang.org/cargo/reference/workspaces.html
