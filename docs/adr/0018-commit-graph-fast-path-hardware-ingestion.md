# ADR 0018: Commit Graph Fast Path Hardware Ingestion

Date: 2026-04-30
Status: Accepted

## Context

The 90k ingestion lane regressed below the 100k+ full commit-throughput target after the active traversal path consolidated around tape-based pack traversal. Recent reports showed the full-index bottleneck in commit walk, while older preconditioned commit-graph runs showed the commit-graph path could get close to the target when paired with throughput-oriented batching.

The repository also needs file-change extraction to remain visible as product work, so the performance lane must not hide deferred file-change drain cost inside a commit-only throughput claim.

## Decision

Use a two-tier hardware ingestion lane:

1. A hard full-index commit graph ingestion gate targeting `full_commit_throughput_per_sec >= 100000`.
2. A separate file-change extraction drain budget reported by `file_change_extraction_drain_ms`.

The Git adapter may use a commit-graph fast path for preconditioned full walks and fall back to tape traversal for missing tips or unsupported boundaries. The indexer sends batches of parsed commits to the single DB writer while queue-depth telemetry continues to count commits, not channel messages.

Hardware mode uses the existing `perf-mimalloc` feature and `-C target-cpu=native`. On Windows, `-HardwareMax` runs through the Docker-backed perf lane used by the repository quality gates so the measurement is not dominated by host filesystem policy. The main ingestion hot path defaults to CSR rebuild mode rather than per-batch streaming CSR application; the full CSR snapshot is still rebuilt after commit ingestion so query benchmarks and runtime queries continue to exercise a materialized graph. `GITGRAPH_LEVEL3_STREAMING_CSR=1` remains available for explicit streaming-CSR ablations. Full reindex stats derive the post-index extraction queue depth from the just-enqueued commit count instead of scanning the freshly cleared 100k queue.

## Consequences

The hot commit-walk path is faster when commit-graph metadata exists and remains correct when the graph is stale. Partial fallback must dedupe commits already emitted by the commit-graph path. The DB writer remains single-owner and file-change extraction stays asynchronous unless strict mode is requested.

The hardware lane is machine-sensitive by design; normal guardrails and dev gates remain the required correctness gates before interpreting perf numbers. The 100k CPS number represents the default commit graph ingestion lane, while CSR rebuild and file-change drain remain separately visible through their existing telemetry.

## Alternatives Considered

1. Keep tape-only traversal and rely on allocator/native CPU flags. Rejected because current telemetry identifies commit walk as the dominant bottleneck.
2. Include strict inline file-change extraction in the 100k CPS target. Rejected because it would mix two different workloads and obscure the separate drain budget.
3. Make commit-graph traversal the only path. Rejected because stale commit graphs and incremental checkpoint boundaries require safe fallback.

## References

1. `tmp/perf-report-ingestion-budget-90k.txt`
2. `tmp/perf-report-90k-mimalloc-docker.txt`
3. `docs/automation-guardrails.md`
