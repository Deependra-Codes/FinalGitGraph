# ADR 0003: Phase 2 Checkpointed Indexing and Commit Window Contracts

Date: 2026-03-07
Status: Accepted

## Context

Phase 1 provided full reindexing and scan foundations, but lacked:

1. incremental checkpointed updates
2. typed request/response contracts for command surface
3. paginated read API for commit windows

Without these, command interfaces drift and indexing costs scale poorly over time.

## Decision

Adopt a Phase 2 Slice A contract:

1. introduce `IndexingMode` (`full`, `incremental`)
2. persist checkpoint state in `indexing_checkpoints` table
3. add `CommitWindowRequest`, `CommitWindowResponse`, and cursor model
4. convert command error surface to typed `CommandError { code, message }`
5. keep full mode deterministic; incremental mode replays from short checkpoint lookback and upserts safely

## Consequences

Positive:

1. stable command contracts for Tauri and CLI integration
2. lower indexing cost for routine updates
3. deterministic pagination for commit history queries

Tradeoffs:

1. incremental mode still replays a short lookback window by design
2. file-change ingestion is still pending in next slice

## Alternatives Considered

1. keep full-only indexing
   - rejected because scaling and latency degrade as repository size grows
2. jump directly to fully checkpointed DAG frontier tracking
   - rejected for kickoff due complexity; phased approach chosen

## References

1. https://doc.rust-lang.org/cargo/reference/workspaces.html
2. https://www.sqlite.org/lang_upsert.html
3. https://git-scm.com/docs/git-log
