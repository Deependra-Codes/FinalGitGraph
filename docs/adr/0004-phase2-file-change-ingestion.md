# ADR 0004: Phase 2 File-Change Ingestion Pipeline

Date: 2026-03-07
Status: Accepted

## Context

Phase 2 Slice A added checkpointed indexing and commit window APIs, but `file_changes` remained unpopulated.

Risk:

1. downstream risk/conflict features cannot compute file-level signals
2. schema intent and runtime behavior diverge

## Decision

Add file-change ingestion to indexing flows:

1. parse `git show --numstat` output per indexed commit set
2. classify rows (`add`, `delete`, `modify`, `rename`, `binary`)
3. upsert into `file_changes`
4. on incremental mode, clear existing `file_changes` rows for replayed commit set before upsert

## Consequences

Positive:

1. `file_changes` table now reflects indexed commits
2. incremental replay stays idempotent for file-level rows
3. indexing stats now include `file_changes_indexed`

Tradeoffs:

1. incremental mode still replays a lookback window
2. rename classification is parser-based and can be refined later

## Alternatives Considered

1. delay file-change ingestion to later phase
   - rejected because it blocks risk feature foundations
2. parse full patch bodies immediately
   - rejected as higher cost for kickoff slice

## References

1. https://git-scm.com/docs/git-show
2. https://git-scm.com/docs/git-diff#Documentation/git-diff.txt---numstat
3. https://www.legacy_relational_engine.org/lang_upsert.html

