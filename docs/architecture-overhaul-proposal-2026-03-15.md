# Architecture Overhaul Proposal: 9k+ Throughput Program

Date: 2026-03-15
Objective: raise sustained indexing throughput from ~600 commits/sec plateau to 9k+ class performance with correctness parity and migration safety.

## 1) Executive Decision

Adopt a three-tier architecture:
- Tier A: In-Memory Execution Layer (bounded queues + hot graph state)
- Tier B: LSM Ingest Layer (primary write path)
- Tier C: legacy_relational_engine Serving Layer (stable read/query model)

Primary ingest engine now: RocksDB.
Migration-safe adapter boundary retained so future Rust-native LSM replacement (for example Fjall) does not require service-wide rewrite.

## 2) Why This Change Is Required

Observed profile behavior (6k / 90k / 100k) shows throughput near-flat around 600 commits/sec while data volume increases. This indicates architecture saturation, not just tuneable local inefficiency. Full-index dominates runtime and does not scale to the target envelope with current orchestration.

## 3) End-State Architecture

### Tier A: In-Memory Execution Layer

Responsibilities:
- absorb burst writes
- sequence and deduplicate commit/edge/file-change events
- maintain hot frontier cache for immediate traversal assistance

Design:
- bounded lock-aware ring buffers
- sequence-numbered event envelopes
- hot adjacency cache with strict TTL/cap
- spill and throttling hooks into Tier B

Constraints:
- no unbounded growth
- memory governor enforces soft and hard caps

### Tier B: LSM Ingest Layer

Responsibilities:
- durable high-throughput write staging
- parallel compaction-managed persistence for event streams

Logical partitions (column-family or equivalent):
- commits
- edges
- file_changes
- checkpoints/materialization_watermarks

Write semantics:
- micro-batch atomic writes
- idempotent sequence keys
- replay-safe checkpoints
- crash recovery from last committed sequence

### Tier C: legacy_relational_engine Serving Layer

Responsibilities:
- keep existing query contracts stable
- preserve read behavior and current API semantics

Materialization service:
- deterministic ordered upsert from Tier B to Tier C
- chunked transactions with adaptive sizing
- lag-aware cadence and recovery resume points

## 4) Cross-Tier Control Plane

- backpressure orchestrator
- memory budget allocator
- compaction debt monitor
- materialization lag monitor
- consistency verifier between Tier B ledger and Tier C state
- recovery coordinator

## 5) Memory Governance (Bang-for-Buck)

Global budgets:
- soft RSS cap: triggers adaptive throttling
- hard RSS cap: forces worker contraction + spill-first behavior

Per-tier budgets:
- Tier A: queue depth and cache bytes
- Tier B: block cache + memtable limits
- Tier C: legacy_relational_engine cache and transaction chunk windows

Adaptive policy examples:
- increase batch size only when lag decreases and RSS remains below soft cap
- reduce workers on compaction stall or memory pressure
- increase materialization cadence when serving lag breaches SLO

## 6) Rollout Strategy

Recommended: phased dual-write migration.

Reason:
- correctness is non-negotiable
- dual-write enables side-by-side parity checks and safe rollback

Not recommended for this program: big-bang cutover.

## 7) 8-12 Week Aggressive Program

Phase 0 (Week 1): observability hardening
- close attribution gaps
- expose per-tier throughput/lag/memory

Phase 1 (Week 1-2): abstraction boundary
- add IngestStore adapter interface
- preserve baseline path intact

Phase 2 (Week 2-4): LSM ingest implementation
- RocksDB-backed write path with sequence keys
- crash-safe checkpoints

Phase 3 (Week 3-5): in-memory tier + governor
- bounded queues, hot cache, backpressure

Phase 4 (Week 4-6): materialization to legacy_relational_engine
- deterministic chunked merge
- parity with existing reads

Phase 5 (Week 6-8): dual-write shadow validation
- compare old/new outputs continuously

Phase 6 (Week 8-10): performance hardening
- tune compaction, worker geometry, batch sizing
- target sustained 6k+, then stretch to 9k+

Phase 7 (Week 10-12): controlled cutover
- progressive traffic shift
- rollback drill validated

## 8) Verification and Acceptance

Correctness gates:
- commit/edge/file-change count parity
- query result parity
- deterministic replay after forced crashes

Performance gates:
- 6k smoke progression milestones
- 90k and 100k sustained throughput validations

Memory gates:
- per-tier cap enforcement
- prolonged soak with stable RSS

Reliability gates:
- failure injection during ingest and materialization
- restart recovery consistency checks

Quality/policy gates:
- guardrails
- fast dev gate during iteration
- full dev gate before merge
- perf gates at each milestone

## 9) Engine Positioning

RocksDB:
- best immediate probability for 9k+ ingest envelope
- mature compaction/telemetry ecosystem

Fjall:
- Rust-native LSM candidate for future migration
- retain adapter for low-friction transition once benchmark parity is proven

Redb:
- excellent Rust-native ACID CoW B-tree
- recommended for control-plane metadata/checkpoints if needed
- not recommended as primary high-churn ingest tier for this throughput objective

## 10) Scope Boundaries

Included:
- ingest architecture redesign
- dual-write migration
- memory governor and telemetry
- staged materialization to existing legacy_relational_engine serving model

Excluded:
- immediate replacement of legacy_relational_engine query layer
- external API behavior changes

## 11) Key Technical Risks and Mitigations

Risk: compaction-driven latency spikes
Mitigation: adaptive worker pools, compaction debt alarms, backpressure integration

Risk: consistency drift between staged and materialized views
Mitigation: sequence-watermark invariants, idempotent merge, parity verifier

Risk: memory growth during burst ingest
Mitigation: strict per-tier budgets + spill/throttle policy

Risk: migration lock-in to chosen engine
Mitigation: adapter boundary + engine-independent logical event format + snapshot export/import

## 12) Immediate Next Handoff

- finalize adapter interfaces and invariants
- define exact metrics contract in perf report output
- run Phase 0 implementation and prove attribution closure before touching write-path internals

## 13) Implementation Status (2026-03-15)

Phase 1 started in code with an ingest adapter seam and initial LSM shadow lane.

Current behavior:
- legacy_relational_engine remains the source of truth for read/query compatibility.
- RocksDB receives mirrored ingest writes when enabled, allowing dual-write validation without changing serving paths.
- Shadow storage is partitioned by column family (`commits`, `file_changes`) for cleaner operational separation.
- LSM keys use deterministic binary encoding (repo id + length-prefixed key parts) to avoid delimiter collisions.
- Shadow writes use batched `WriteBatch` operations with fail-open policy support.

Environment flags:
- `GITGRAPH_LSM_SHADOW_ENABLE=1` enables RocksDB shadow writes.
- `GITGRAPH_LSM_PATH=<path>` overrides default shadow store path (`.gitgraph-lab/ingest-rocksdb`).
- `GITGRAPH_LSM_SHADOW_FAIL_OPEN=1` keeps legacy_relational_engine writes authoritative even if shadow write fails.

Build requirement:
- Compile with cargo feature `lsm-rocksdb` to activate RocksDB-backed shadow lane.

