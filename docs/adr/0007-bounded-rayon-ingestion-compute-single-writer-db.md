# ADR 0007: Bounded Rayon Ingestion Compute with Single-Writer SQLite Contract

Date: 2026-03-07
Status: Accepted

## Context

Indexing previously processed file-change extraction strictly serially inside the ingestion flow.

Risks:

1. underutilized CPU on medium/large repositories during diff-heavy indexing.
2. pressure to parallelize writes, which conflicts with SQLite's single-writer model.
3. potential memory blow-ups if parallel compute is introduced without explicit bounds.

## Decision

Adopt bounded parse/compute parallelism while preserving single-writer DB ownership:

1. keep DB writes transaction-owned in `application/indexer.rs`.
2. use bounded Rayon chunk parallelism in `adapters/git.rs` for file-change extraction.
3. preserve deterministic output order by collecting chunk results in input order.
4. keep git adapter DB-free; no `rusqlite` dependency is allowed in `adapters/git.rs`.
5. enforce this contract via `scripts/enforce-guardrails.ps1`.

## Consequences

Positive:

1. improves ingestion throughput for diff-heavy repositories without violating SQLite write constraints.
2. keeps architecture layering clean (`adapters/git` compute only, `adapters/db` + `application/indexer` write path).
3. keeps memory bounded via explicit chunk sizing and existing file-change DB batch controls.

Tradeoffs:

1. additional tuning surface (`parallel_min_commits`, chunk size).
2. bounded parallel collection can increase peak memory versus fully streaming serial compute for the same commit window.
3. guardrails become stricter and can fail quickly if contract drift is introduced.

## Alternatives Considered

1. parallelize SQLite writes directly.
   - Rejected: SQLite allows one writer at a time; complexity increases with little gain.
2. keep file-change extraction fully serial.
   - Rejected: leaves available CPU throughput untapped on large histories.
3. move immediately to a new storage engine.
   - Rejected: too large in scope for current slice; current architecture can still gain from bounded compute parallelism.

## References

1. SQLite transactions (single writer semantics): https://www.sqlite.org/lang_transaction.html
2. SQLite WAL mode concurrency notes: https://www.sqlite.org/wal.html
3. Rayon iterators: https://docs.rs/rayon/latest/rayon/iter/index.html
4. LSM-tree paper (write-optimized storage background): https://db.cs.berkeley.edu/cs286/papers/lsm-acta1996.pdf
