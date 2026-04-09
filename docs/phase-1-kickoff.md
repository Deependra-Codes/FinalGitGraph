# Phase 1 Kickoff (Historical Snapshot)

Date: 2026-03-06

This document is a historical kickoff snapshot.
Current runtime architecture is governed by ADR 0015 and uses LMDB + CSR + delta with no SQL runtime path.

## Historical Outcome (Carried Forward)

1. Rust core scaffold established in `apps/desktop/src-tauri`.
2. Layered internal module boundaries established (`adapters`, `application`, `domain`, `support`).
3. Repository scan/index command foundations established.
4. Guardrail-first development flow established.

## Current Source of Truth

For active architecture and operating model, use:

1. `docs/engineering-system.md`
2. `docs/from-scratch-system.md`
3. `docs/automation-guardrails.md`
4. `docs/adr/0015-lmdb-csr-delta-storage-no-sql.md`
