# GitGraph Lab - Industry Approach and Intent Check

Date: 2026-03-27

## Phase 0 Baseline

Before feature-heavy expansion, the repository enforces:

1. deterministic toolchain + workspace lockfile
2. layered Rust architecture (`application`, `adapters`, `domain`, `support`)
3. guardrails + fast/full quality gates
4. ADR-backed architecture decisions

References:

1. `docs/engineering-system.md`
2. `docs/from-scratch-system.md`
3. `docs/automation-guardrails.md`

## Intent Check

Rust remains the intentional core for ingestion/query performance and reliability.
Desktop/UI remains a Tauri/React shell around the Rust core.

## Industry-Grade Engineering Approach

1. Layered architecture:
   - `adapters`: external I/O boundaries (Git + LMDB)
   - `domain`: typed DTOs and contracts
   - `application`: orchestration and use-case entrypoints
2. Data architecture:
   - LMDB-backed typed records
   - persisted CSR snapshots for graph hot paths
   - append-only delta log for mutation chronology
3. Reliability by default:
   - idempotent indexing behavior
   - bounded ingestion buffers
   - retryable enrichment queue with explicit state contract
4. Security and safety:
   - in-process repository access via `gix`
   - canonical path validation and repository boundary checks
5. Observability:
   - phase-level ingestion telemetry
   - phase memory outputs (including peak RSS)
   - warm/cold perf lanes for regression diagnosis
6. Test strategy:
   - unit tests by layer
   - integration smoke coverage for end-to-end indexing/query flows
   - guardrail + perf gates in local/CI loops

## Current Runtime Contract

1. No relational_query runtime dependency.
2. No relational_query migrations in active runtime.
3. No legacy relational_query-era query modules in active application path.
4. Indexing and query flow must satisfy LMDB + CSR + delta contracts.

## Delivery Contract

1. `pnpm run guardrails`
2. `pnpm run dev:gate:fast`
3. `pnpm run dev:gate`
4. `pnpm run quality:perf` when traversal/runtime perf behavior changes
5. `pnpm run quality:perf:ingestion` when ingestion behavior changes

## Architecture Record

LMDB/CSR/no-relational_query runtime decision is documented in:

1. `docs/adr/0015-lmdb-csr-delta-storage.md`

