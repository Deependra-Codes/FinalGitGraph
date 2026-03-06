# Phase 1 Kickoff (Week 1)

Date: 2026-03-06

Note: Engineering guardrails were elevated to Phase 0 foundation first. See `docs/engineering-system.md`.

## Goal Alignment

This kickoff maps to SDD Section 16 (Week 1):

1. Tauri + React + Rust scaffold (backend-side Rust scaffold started)
2. SQLite schema + migration pipeline (implemented)
3. Repository scan and commit import foundation (implemented)

## What Was Implemented

1. Rust project scaffold
   - Path: `apps/desktop/src-tauri`
   - Modules: layered internal boundaries (`adapters`, `application`, `domain`, `support`)
2. Migration pipeline
   - `schema_migrations` tracker table
   - Initial migration: `migrations/0001_initial.sql`
   - Hardening migration: `migrations/0002_repo_scoped_commit_keys.sql`
   - WAL and core PRAGMA tuning enabled at DB open
3. Repository ingest foundation
   - `scan_repository(path)`:
     - validates Git repository with `git rev-parse`
     - canonicalizes root and git dir
     - upserts `repositories`
   - `index_commits(path)`:
     - imports commits via `git log --all --topo-order --date-order`
     - imports parent edges into `commit_edges`
     - imports refs via `git show-ref --heads --tags --dereference`
     - persists index timestamp
4. Developer bootstrap
   - CLI entrypoint for local validation (`scan`, `index`)
   - Default SQLite path: `.gitgraph-lab/gitgraph-lab.db`

## Current Gap vs Full Week 1

1. React/Tauri UI shell is not scaffolded yet.
2. File-level change ingestion (`file_changes`) is not yet populated.
3. Progress events and job IDs are not implemented yet.

## Immediate Next Slice

1. Add Tauri command wrappers and minimal desktop shell.
2. Add incremental indexing mode (`full` vs `since last indexed`).
3. Add commit file-stat ingestion pipeline (`git show --numstat` batch strategy).

## Phase 2 Plan Reference

1. Industry execution plan and phase-2 scope:
   - `docs/industry-approach.md`
