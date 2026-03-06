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

Tradeoffs:

1. additional adapter/query complexity (rowid conversion + batch tables)
2. tighter coupling to current SQLite storage semantics in adapter layer
3. dedicated perf test maintenance cost (fixture tuning, threshold review)

## Alternatives Considered

1. Keep SHA-string state and micro-optimize cloning.
   - Rejected: does not address core identity representation cost.
2. Full schema migration to explicit integer commit IDs in all edge tables now.
   - Rejected for current slice due migration scope; rowid-based adapter strategy chosen first.

## References

1. SQLite rowid tables: https://www.sqlite.org/rowidtable.html
2. petgraph node index model: https://docs.rs/petgraph/latest/petgraph/graph/struct.NodeIndex.html
3. Boost CSR graph representation: https://www.boost.org/libs/graph/doc/compressed_sparse_row.html
4. Ligra graph docs: https://jshun.github.io/ligra/docs/graph.html
5. Direction-Optimizing BFS (Beamer et al., SC'12): https://www.scottbeamer.net/pubs/beamer-sc2012.pdf
