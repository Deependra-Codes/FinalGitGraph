# ADR 0008: Integer Edge Schema for Graph Traversal Hot Paths

Date: 2026-03-07
Status: Accepted

## Context

Graph traversal code already uses integer internal state (`NodeId`/rowid-style IDs), but edge storage still used SHA text pairs.

Risks:

1. repeated SHA-to-ID joins on every neighbor expansion in BFS/DFS/topological/shortest-path.
2. wider indexes and string comparison overhead in the hottest query path.
3. mismatch between integer traversal strategy and string edge storage.

## Decision

Adopt integer identity as the storage hot-path contract:

1. introduce stable integer commit identity (`commit_id`) in `commits`.
2. store edges as `(repo_id, child_commit_id, parent_commit_id)` in `commit_edges`.
3. keep `(repo_id, sha)` unique for external/API identity.
4. rewrite graph neighbor and pull queries to use integer edge columns directly (no SHA edge joins).
5. enforce the contract in `scripts/enforce-guardrails.ps1`.

## Consequences

Positive:

1. removes SHA join-heavy neighbor expansion from graph query hot loops.
2. improves index locality and reduces comparison overhead for traversal queries.
3. aligns storage and algorithm identity models around integer IDs.

Tradeoffs:

1. migration complexity increases (schema + data-copy + edge remap).
2. tests/fixtures must be updated to integer edge columns.
3. ingestion edge insertion path requires integer remap logic.

## Alternatives Considered

1. keep SHA edge schema and only tune SQL/indexes.
   - Rejected: leaves fundamental string-join overhead in the hot path.
2. keep hybrid schema with SHA edges as primary and integer edges as cache.
   - Rejected: doubles maintenance complexity for no immediate contract clarity.
3. move directly to external graph engine.
   - Rejected: disproportionate scope for current phase.

## References

1. Ligra (sparse/dense frontier processing): https://jshun.csail.mit.edu/ligra.pdf
2. GraphIt (graph optimization scheduling): https://arxiv.org/abs/1805.00923
3. SQLite rowid table behavior: https://www.sqlite.org/rowidtable.html
4. Git commit-graph format (positional parent references): https://git-scm.com/docs/gitformat-commit-graph
