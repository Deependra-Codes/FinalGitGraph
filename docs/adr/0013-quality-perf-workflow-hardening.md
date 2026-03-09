# ADR 0013: Quality and Performance Workflow Hardening

Date: 2026-03-08
Status: Accepted

## Context

1. Repository-scale indexing and traversal correctness depends on strict queue, pagination, and truncation behavior under high load.
2. Incremental ingestion throughput needs bounded parallelism that still activates on medium-sized batches.
3. Existing CI covered correctness and perf budget gates, but needed additional fast PR perf signal plus scheduled ingestion-scale enforcement.
4. Guardrails and docs must reflect operational behavior so AI/human contributors can change code without weakening contracts.

## Decision

1. Keep commit-window API contract unchanged while guaranteeing sentinel pagination at the max client limit (`5000`) by preserving `limit+1` fetch semantics end-to-end.
2. Keep file-change enrichment state contract unchanged (`pending|ready|failed`) and adopt bounded retry scheduling:
   - non-terminal failures remain `pending` with delayed queue re-pickup
   - terminal transition to `failed` at max attempts
   - re-enqueue from indexing resets `attempts` and `last_error`.
3. Enforce graph traversal budget before expensive neighbor expansion in BFS/reachable paths and before DFS boundary-node expansion.
4. Keep a dedicated bounded Rayon pool for file-change extraction and keep adaptive incremental-friendly activation defaults:
   - `GITGRAPH_FILE_CHANGE_PARALLEL_THREADS`
   - `GITGRAPH_FILE_CHANGE_PARALLEL_MIN_COMMITS`
5. Keep existing required gates and add coverage lanes:
   - PR perf smoke lane in `rust-ci.yml`
   - nightly ingestion budget workflow for cold/warm profiles.

## Consequences

1. Pagination correctness at `limit=5000` now preserves `has_more` and cursor behavior without API changes.
2. Enrichment failures no longer force immediate terminal state; queue retry behavior is deterministic and bounded.
3. Truncated traversal queries avoid unnecessary DB work while preserving deterministic response semantics.
4. Incremental ingestion batches can parallelize earlier with bounded memory and deterministic output ordering.
5. CI signal quality improves: faster perf regression visibility on PRs plus scheduled ingestion-scale enforcement.

## Alternatives Considered

1. Keep immediate terminal failure for enrichment errors.
   - rejected: over-classifies transient failures and reduces recovery without reindex.
2. Use global Rayon pool for file-change extraction.
   - rejected: susceptible to contention from unrelated compute and less deterministic utilization.
3. Add only nightly perf coverage without PR smoke.
   - rejected: slower feedback loop for performance regressions.

## References

1. `docs/performance-engineering.md`
2. `docs/automation-guardrails.md`
3. `.github/workflows/rust-ci.yml`
4. `.github/workflows/nightly-ingestion-perf.yml`
