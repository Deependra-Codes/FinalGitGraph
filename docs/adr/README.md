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
2. `0002-internal-module-boundaries.md`
3. `0003-phase2-checkpoint-and-window-contracts.md`
4. `0004-phase2-file-change-ingestion.md`
5. `0005-integer-graph-query-state.md`
6. `0006-streaming-libgit2-ingestion.md`
7. `0007-bounded-rayon-ingestion-compute-single-writer-db.md`
8. `0008-integer-edge-schema-for-graph-hot-path.md`
9. `0009-in-memory-adjacency-graph-for-perf-harness.md`
10. `0010-bounded-rename-detection-and-ingestion-phase-telemetry.md`
11. `0011-core-file-change-ingestion-and-jit-enrichment.md`
12. `0012-gitoxide-ingestion-adapter-migration.md`
13. `0013-quality-perf-workflow-hardening.md`
14. `0014-runtime-query-graph-cache.md`
15. `0015-lmdb-csr-delta-storage.md`
16. `0016-overlay-adjacency-and-lmdb-map-flooring.md`
17. `0017-single-tape-traversal-engine.md`
18. `0018-commit-graph-fast-path-hardware-ingestion.md`
19. `0019-remove-streaming-csr-and-age-only-pruning.md`
20. `0026-level-1-index-session-planner.md`

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

