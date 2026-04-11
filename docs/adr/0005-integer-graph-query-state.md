# ADR 0005: Integer Graph Query State for Traversal and Path APIs

Date: 2026-03-07
Status: Accepted

## Context

Graph query implementations (`BFS`, `DFS`, `topological`, shortest-path) previously used SHA `String` values directly in frontier queues, visited sets, and predecessor maps.

Risks:

1. repeated SHA cloning and hashing in hot loops
2. avoidable heap allocation churn
3. poorer CPU cache behavior from string-heavy hash structures

## Decision

Adopt integer-based internal state for graph queries:

1. resolve indexed commits to integer row IDs (`rowid`) in DB adapter
2. intern row IDs into compact in-memory node IDs (`NodeId`)
3. store traversal state using integer IDs and dense visited bit state
4. use batched neighbor retrieval for frontier expansion
5. materialize SHA strings only at command/API response boundaries
6. use direction-optimizing BFS mode selection (`push`/`pull`) for traversal expansion
7. enforce performance regression budget in CI via dedicated perf gate

## Consequences

Positive:

1. lower string allocation/hashing overhead in graph algorithm hot paths
2. improved locality for visited/frontier/predecessor structures
3. clear separation between external identity (`sha`) and internal compute identity (`NodeId`)
4. adaptive BFS can reduce duplicate neighbor expansion work on wide-frontier levels
5. CI prevents silent performance drift in hot traversal paths
6. ordered batched rowid decoding (`Vec<Vec<i64>>`) removes repeated per-source hash lookups in frontier expansion
7. ordered SHA materialization from rowids removes per-node map lookups/cloning in traversal/shortest-path response assembly
8. prepared-statement caching lowers repeated relational_query parse overhead in graph query hot loops

Tradeoffs:

1. additional adapter/query complexity (rowid conversion + batch tables)
2. tighter coupling to current legacy_relational_engine storage semantics in adapter layer
3. dedicated perf test maintenance cost (fixture tuning, threshold review)
4. performance budget fallback adds Docker dependency for policy-restricted host environments

## Implementation Notes (2026-03-07)

1. Added `load_commit_shas_for_rowids` for ordered rowid-to-sha conversion used directly by graph traversal/path response mapping.
2. Added `load_neighbor_rowids_for_sources` returning source-order-aligned neighbor vectors for BFS/topological/shortest-path expansion.
3. Enabled legacy_relational_engine prepared statement cache and switched hot graph query paths to `prepare_cached`.
4. Hardened performance gate script with host-policy detection and Docker retry fallback to keep perf validation deterministic.

## Alternatives Considered

1. Keep SHA-string state and micro-optimize cloning.
   - Rejected: does not address core identity representation cost.
2. Full schema migration to explicit integer commit IDs in all edge tables now.
   - Rejected for current slice due migration scope; rowid-based adapter strategy chosen first.

## References

1. legacy_relational_engine rowid tables: https://www.legacy_relational_engine.org/rowidtable.html
2. petgraph node index model: https://docs.rs/petgraph/latest/petgraph/graph/struct.NodeIndex.html
3. Boost CSR graph representation: https://www.boost.org/libs/graph/doc/compressed_sparse_row.html
4. Ligra graph docs: https://jshun.github.io/ligra/docs/graph.html
5. Direction-Optimizing BFS (Beamer et al., SC'12): https://www.scottbeamer.net/pubs/beamer-sc2012.pdf

