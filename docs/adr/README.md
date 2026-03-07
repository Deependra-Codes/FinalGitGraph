# ADR Process

Use Architecture Decision Records (ADRs) for choices that affect long-term direction.

## When ADR Is Required

1. module/crate boundary changes
2. database schema strategy changes
3. runtime model changes (jobs, queues, concurrency model)
4. security model changes (permissions, capabilities, secrets)
5. testing/quality gate policy changes

## File Naming

Use `NNNN-short-title.md`, for example:

1. `0001-rust-workspace-governance.md`
2. `0002-indexing-checkpoint-strategy.md`
3. `0006-streaming-libgit2-ingestion.md`
4. `0012-gitoxide-ingestion-adapter-migration.md`

## Template

```md
# ADR NNNN: Title

Date: YYYY-MM-DD
Status: Proposed | Accepted | Superseded

## Context

Problem statement and constraints.

## Decision

What we are choosing.

## Consequences

Tradeoffs, risks, migration impact.

## Alternatives Considered

Rejected options and why.

## References

Primary sources / benchmarks / docs.
```
