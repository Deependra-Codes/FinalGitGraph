# Phase 2 Kickoff (Slice A)

Date: 2026-03-07

## Goal

Start Phase 2 with production-oriented core behavior while keeping strict guardrails active.

## Implemented in This Kickoff

1. Typed command contracts and error codes:
   - Added request/response DTOs in `domain/models.rs`.
   - Added structured command errors (`code`, `message`) mapped from `AppError`.
2. Incremental indexing mode:
   - Added `IndexingMode` (`full`, `incremental`).
   - Added checkpoint storage via migration `0003_indexing_checkpoints.sql`.
   - Incremental mode replays from a short checkpoint lookback window and upserts safely.
3. Commit window API:
   - Added paginated commit-window retrieval with cursor (`committer_time`, `sha`).
   - Added CLI action `window` for local verification.
4. DB adapter hardening for Phase 2:
   - Added `clear_repository_index_data`, `synchronize_refs`, checkpoint load/upsert helpers.
5. Tests:
   - Added checkpoint roundtrip unit test.
   - Added integration test covering incremental indexing + commit window.

## Verification

Validated with repository guardrails and full Docker-backed quality gate:

1. `pnpm run safe:fast`
2. `pnpm run safe:full`

Both passed.

## What This Enables Next

1. File-change ingestion pipeline (`file_changes`) with bounded batch writes.
2. DAG traversal APIs (BFS/DFS/shortest-path).
3. Tauri command exposure over the new typed contracts.

## Phase 2 Slice B Candidate

1. Add `file_changes` ingestion in incremental path.
2. Add deterministic integration fixture with branching and merge commit set.
3. Add first DAG path query endpoint with response contract tests.

## Phase 2 Slice B Status (2026-03-07)

Completed:

1. `file_changes` ingestion from Git numstat output (full + incremental).
2. Incremental replay cleanup for file-change rows.
3. Added parser/unit coverage and integration validation.
