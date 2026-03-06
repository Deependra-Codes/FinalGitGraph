# Performance Engineering Guardrails

Date: 2026-03-07

## Goal

Keep graph-query code performant by default on medium/large repositories without weakening reliability or maintainability.

## Non-Negotiable Performance Rules

1. Do not use `String` as frontier identity in graph traversals.
2. Keep traversal/path internal state integer-based (`NodeId`, rowid, compact index).
3. Use batched neighbor loading for frontier expansion where possible.
4. Keep algorithm output contracts unchanged (`sha` remains external DTO identity).
5. Preserve deterministic traversal ordering and typed error semantics.
6. Do not modify docs/guardrails alone to satisfy quality gates; policy edits must be paired with implementation changes.

## Current Strategy in This Repository

1. DB layer resolves commit identity to integer `rowid` for graph query internals.
2. Graph queries intern `rowid` values into compact in-memory IDs.
3. Visited sets use dense bit-like storage instead of `HashSet<String>`.
4. BFS uses direction-optimizing mode selection (`push` vs `pull`) by frontier/remaining ratio.
5. BFS/topological and bidirectional shortest-path expansion use batched neighbor queries.
6. SHA strings are only materialized at API boundaries (request/response).

## Why This Strategy

1. Integer node identities reduce hashing and allocation overhead vs repeated string cloning.
2. Dense frontier/visited state improves CPU-cache behavior compared to sparse string hash tables.
3. Batched DB neighbor retrieval reduces query round-trips and iterator overhead.
4. Direction-optimizing BFS lowers duplicate expansion work when frontier becomes large.

## Guardrail Enforcement

`scripts/enforce-guardrails.ps1` enforces:

1. `application/graph_queries.rs` must not contain:
   - `HashSet<String>`
   - `HashMap<String, String>`
   - `VecDeque<String>`
2. Required governance/performance docs must exist, including this file.
3. Performance CI artifacts must exist:
   - `scripts/perf-budget.ps1`
   - `.github/workflows/rust-ci.yml` contains `Performance Budget` step
   - `package.json` contains `quality:perf` script
4. Anti-tamper rule:
   - if docs/guardrail/policy files are changed, implementation files must also change in the same diff.

## Direction-Optimizing BFS Policy

1. Runtime heuristic:
   - switch to `pull` only when frontier is large enough and frontier-to-remaining ratio crosses threshold.
2. Pull expansion contract:
   - database query returns deduplicated candidate rowids in deterministic order.
   - Rust-side visited set remains source of truth for first-visit semantics.
3. Strategy safety:
   - output contract (`GraphTraversalResponse`) and traversal semantics remain unchanged.

## CI Performance Budget

1. Perf budget test name:
   - `perf_budget_direction_optimizing_bfs`
2. It is intentionally `#[ignore]` in normal test loops and runs only in dedicated CI/perf command.
3. Gate command:
   - `pnpm run quality:perf`
4. Current threshold:
   - adaptive BFS median runtime must be no more than 15% slower than push-only baseline on the fixed overlap-heavy fixture.
   - this is a regression guardrail, not a synthetic peak-throughput claim.

## Change Checklist for Graph Query Work

1. Verify algorithm state uses integer node IDs.
2. Verify batch DB path is used for BFS-like expansion.
3. Verify BFS mode-switch heuristic still satisfies deterministic output and perf budget test.
4. Add or update unit/integration tests covering correctness and error contracts.
5. Run:
   - `pnpm run guardrails`
   - `pnpm run dev:gate:fast`
   - `pnpm run dev:gate`
   - `pnpm run quality:perf`

## References

1. SQLite rowid tables: https://www.sqlite.org/rowidtable.html
2. petgraph `NodeIndex` (`index() -> usize`): https://docs.rs/petgraph/latest/petgraph/graph/struct.NodeIndex.html
3. Boost CSR graph (compact CSR representation): https://www.boost.org/libs/graph/doc/compressed_sparse_row.html
4. Ligra graph API/docs (integer vertex function signatures): https://jshun.github.io/ligra/docs/graph.html
5. Cache locality concern in dynamic graph processing context (Aspen/Ligra ecosystem): https://arxiv.org/abs/1904.08380
