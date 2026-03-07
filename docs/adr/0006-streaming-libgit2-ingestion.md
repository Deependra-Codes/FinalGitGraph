# ADR 0006: Streaming libgit2 Ingestion for Indexing Hot Path

Date: 2026-03-07
Status: Accepted

## Context

Indexing previously relied on process-spawned Git CLI calls that fully buffered command output in memory (`Command::output`), then parsed repository-wide commit and file-change vectors before database writes.

Risks for large repositories:

1. Peak memory scaled with total repository history (`O(total_commits + total_file_changes)`).
2. Process launch overhead repeated across many `git show` batches.
3. Latency variance from large string buffering and parsing bursts.

## Decision

Adopt bounded-memory streaming ingestion with `git2`/libgit2 in the Rust adapter layer:

1. use libgit2 revision walking (`revwalk`) for commit enumeration
2. use in-process tree diff traversal for file-change extraction
3. expose streaming adapter APIs:
   - `stream_commits`
   - `stream_file_changes_for_commits`
4. keep indexing memory bounded via two explicit batch controls:
   - commit-sha batch size for file-change extraction
   - database upsert batch size for file-change rows
5. keep algorithm/output contracts unchanged in application/domain APIs

## Consequences

Positive:

1. removes full-buffer Git CLI hot path from indexing.
2. reduces process-spawn overhead and large temporary string allocations.
3. shifts ingestion memory profile to bounded batches (`O(batch_size)`).
4. preserves repository-scoped transactional database semantics.

Tradeoffs:

1. adds `git2` dependency and larger Rust build surface.
2. merge-diff semantics are tied to adapter policy (currently first-parent diff baseline).
3. file-change extraction now depends on libgit2 diff behavior and callback logic.

## Alternatives Considered

1. Keep existing Git CLI buffering and increase chunk sizes.
   - Rejected: does not remove full-buffer memory wall; only moves it.
2. Keep CLI but use streaming plumbing (`rev-list`, `diff-tree --stdin`, `cat-file --batch-command`).
   - Deferred: valid high-performance path, but current decision prioritizes in-process integration and lower process overhead first.

## References

1. Rust `Command::output` / child output collection:
   - https://doc.rust-lang.org/std/process/struct.Command.html
   - https://doc.rust-lang.org/std/process/struct.Child.html
2. libgit2 revwalk sorting:
   - https://libgit2.org/docs/reference/main/revwalk/git_revwalk_sorting.html
   - https://libgit2.org/docs/reference/main/revwalk/git_sort_t.html
3. git2 crate revwalk API:
   - https://docs.rs/git2/latest/git2/struct.Revwalk.html
4. libgit2 diff APIs:
   - https://libgit2.org/docs/reference/main/diff/index.html
   - https://libgit2.org/docs/reference/main/diff/git_diff_foreach.html
5. Git commit-graph / maintenance background:
   - https://git-scm.com/docs/git-commit-graph
   - https://git-scm.com/docs/git-maintenance
