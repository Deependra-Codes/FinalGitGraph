# ADR 0019: Max Performance Hardware Profile

Date: 2026-04-30
Status: Accepted

## Context

The hardware ingestion lane previously balanced throughput against an implicit small-laptop memory posture. The next target is adaptive maximum throughput on the current machine, with no fixed RSS cap unless the run exhausts memory or corrupts correctness. The public telemetry contract must remain stable while making the hot lane more explicit: commit ingestion CPS is measured separately from CSR rebuild and file-change extraction drain.

## Decision

Add a `max_performance` runtime profile selected with `GITGRAPH_RUNTIME_PROFILE=max_performance` and surfaced through `perf-report.ps1 -MaxPerformance`.

The profile:

1. Enables the hardware lane defaults: mimalloc, native CPU rustflags, preconditioned commit graph, throughput write profile, breadth-first walk, and commit-graph fast path.
2. Scales commit write wave size and commit ingest queue capacity from available parallelism, while preserving explicit env overrides for ablations.
3. Treats memory as telemetry rather than a hard gate in `perf:ingestion:200k:hardware:max`.
4. Emits hardware profile details in perf reports: runtime profile, logical cores, available memory, platform, allocator, rustflags, queue depth, and write-wave batch telemetry.
5. Keeps `full_commit_throughput_per_sec` as the stable public key, but defines it as commit-ingestion lane throughput; full-index wall time remains available as `full_index`, while file-change drain and CSR/query readiness remain separate metrics.

## Consequences

The 200k gate now targets `fixture_generation_commit_count >= 200000` and `full_commit_throughput_per_sec >= 200000` without a default RSS ceiling. A machine with more cores or memory can use larger batches and queues automatically, and a constrained machine can still run with smaller detected capacity or explicit overrides.

The compact SHA and DB storage rewrites remain the next deeper optimization layer. This ADR does not introduce `unsafe`, LMDB `WRITE_MAP`, or asynchronous map writes. Those remain ADR-backed decisions only if safe code and current LMDB flags miss the target.

## Alternatives Considered

Keep the 8GB-oriented fixed budgets: rejected because it hides available hardware and conflicts with the max-performance goal.

Make 200k a separate private script only: rejected because perf gates and reports need a stable public command for repeatable runs.

Keep `full_commit_throughput_per_sec` tied to full-index wall time: rejected because it conflates commit ingestion, CSR rebuild, query prewarm, and file-change drain into one number, making it impossible to reason about the two-tier budget.

## References

1. `scripts/perf-report.ps1 -MaxPerformance`
2. `scripts/perf-ingestion-budget.ps1 -Profile 200k -MaxPerformance`
3. `pnpm run perf:ingestion:200k:hardware:max`
