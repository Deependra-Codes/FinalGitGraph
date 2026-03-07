# ADR 0012: Gitoxide (`gix`) Migration for Git Adapter Hot Path

Date: 2026-03-07

## Context

1. The Git adapter hot path is core to large-history indexing throughput and stability.
2. We need an in-process Git backend that keeps streaming APIs, bounded parallelism, and no process-spawned CLI in ingestion.
3. Ongoing optimization work requires clearer control over tree diff, rewrite tracking, and selective line-stat extraction.

## Decision

1. Replace `git2`/libgit2 dependency in `apps/desktop/src-tauri` with `gix`.
2. Keep existing adapter contracts unchanged:
   - `stream_commits` / `stream_commits_with_hide`
   - `stream_file_changes_for_commits`
   - metadata-first ingestion with selective line-stat enrichment.
3. Implement commit walk with `gix` revision walk over repository refs, including hide-boundary support.
4. Implement tree diff and rewrite detection with `gix` diff options and bounded rename policy (`rename_limit`, `rename_threshold`, `exact_match_only`).
5. Preserve zero/one-delta fast path and diff sub-phase telemetry (`diff_build`, `rename_find`, `delta_emit`).

## Consequences

1. Git adapter no longer depends on libgit2 bindings; migration risk shifts to `gix` API behavior.
2. Existing ingestion/query interfaces and guardrail checks remain intact.
3. Selective line-stat extraction stays deferred and scoped, with `gix` blob line-count APIs.
4. Additional backend parity testing remains important for edge cases (annotated tags, rewrites, binary content classification).

## Alternatives Considered

1. Keep libgit2 and only tune thresholds/chunking.
   - rejected: does not satisfy backend migration requirement.
2. Use Git CLI for hot path.
   - rejected: violates no-process-spawn ingestion rule and increases overhead.
3. Hybrid backend (both libgit2 and `gix` active).
   - rejected: increases complexity and test matrix without clear runtime benefit.

## References

1. `gix` crate docs: https://docs.rs/gix/latest/gix/
2. `gix` revision walk API: https://docs.rs/gix/latest/gix/revision/walk/
3. `gix` tree diff API (`diff_tree_to_tree`): https://docs.rs/gix/latest/gix/struct.Repository.html#method.diff_tree_to_tree
4. `gix` tree diff change and blob line counts: https://docs.rs/gix/latest/gix/object/tree/diff/
