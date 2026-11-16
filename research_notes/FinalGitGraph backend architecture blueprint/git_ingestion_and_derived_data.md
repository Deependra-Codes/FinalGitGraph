# Git ingestion and per-commit derived data: throughput, correctness, incrementality (as of 2026-11-01)

Scope: enumerating commits and parent edges, loading commit metadata, computing per-commit file-change sets, renames and line stats, staying incremental, and checking results against an oracle. It ends with what to reuse from gitoxide/git and what to build.
Version context. The repo pins `gix = "0.80.0"` and `gix-pack = "0.67.0"` (`apps/desktop/src-tauri/Cargo.toml`). Upstream `gix` is at 0.90.0 on docs.rs. The changelog lists 0.88.0 on 2026-09-25 and 0.85.0 on 2026-06-22, a roughly monthly cadence, so 0.80 is about 8 to 10 months old (inference). `adapters/git.rs` is 4,948 lines.

Evidence labels:
- **[Measured-local]**: timed in this research session. Setup: Apple M1 Pro (10 cores), Apple Git 2.39.5, a bare clone of `github.com/git/git` with 85,936 commits reachable from `--all`, 64,322 of them non-merges, in a single 321 MiB pack with no bitmap. Each number is one wall-clock run with output sent to `/dev/null`, after one warm-up. Treat them as order-of-magnitude figures, not rigorous benchmarks.
- **[Primary]**: official docs, source code, or mailing-list posts.
- **[Secondary]**: blogs and commentary.
- **[Background]**: knowledge not re-verified this session. These items appear only under Inferences or Gaps.

---

## Q1. What gitoxide (gix) offers in 2026, and which custom FinalGitGraph code is redundant

### Takeaway
gix already covers object access, commit-graph reading, revision walking, tree diff, rename and copy tracking, blob line counts, replace refs, shallow repos, multi-pack-index and merge-base. It lacks changed-path Bloom filters, reachability-bitmap files, commit-graph writing, reftable and promisor (partial-clone) support. gix also ignores corrected commit dates (GDA2) and only reads topological-level generation numbers (v1). The custom Bloom/murmur3 code is therefore not redundant with gix. It is likely redundant with a correctly designed derived index, though, because a full change-set index holds strictly more information than a Bloom filter. The custom pack "tape" walker overlaps heavily with gix-odb and gix-pack.

### Cited Findings
- **commit-graph** (gix-commitgraph):
  - Done: "Read-only access, including timestamps, generation numbers, parents, and extra edges".
  - Not done: "Corrected generation dates", "Bloom filter index and data (changed-path filters)", and "Creating and updating graph files". — [crate-status.md](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)
- The newest crate is gix-commitgraph 0.42.0, which docs.rs dates 22 Oct 2026. Its docs say "Read-only access to the git commitgraph file format". The crate exposes `Graph`, `File`, `Position`, `GENERATION_NUMBER_INFINITY` and `MAX_COMMITS`, and none of its documentation mentions Bloom/BIDX/BDAT or GDA2. — [docs.rs gix-commitgraph](https://docs.rs/gix-commitgraph/latest/gix_commitgraph/)
- gix 0.87.0 added `commit::Info::generation` "That way it's evident if a commit-graph was present for this node." — [gix CHANGELOG](https://github.com/GitoxideLabs/gitoxide/blob/main/gix/CHANGELOG.md)
- **multi-pack-index**: read, write and verify are all done. — [crate-status.md](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)
- **Reachability bitmaps**: the gix-pack "'bitmap' file" item is not done. gix-bitmap has EWAH decode only, with no encode. — [crate-status.md](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)
- **reftable**: the gix-ref reftable store is not done. gix-reftable read/write/transactions/compaction is not done. "Git 3.0 compatibility (SHA-256, reftable)" is not done. gix 0.88.0 merged PR #2970 "better-reftable-error", which gives a clearer error rather than support. — [crate-status.md](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md); [gix CHANGELOG](https://github.com/GitoxideLabs/gitoxide/blob/main/gix/CHANGELOG.md)
- **Revision walk**: the ancestor traversal and include/exclude tips items are done, as is "Graph traversal primitives with commit-graph acceleration (gix-revwalk)". "Stop descending hidden ancestry early (performance)" is not done. — [crate-status.md](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)
- `gix::revision::walk::Sorting` has three variants:
  - `BreadthFirst` returns commits "as they are mentioned in the commit graph". The docs say it is "not to be confused with `git log/rev-list --topo-order`".
  - `ByCommitTime(NewestFirst|OldestFirst)` is a full sort of the queued commits.
  - `ByCommitTimeCutoff{order, seconds}`.
  - There is no topological-order variant. The time-based modes "benefit greatly from an object cache". — [docs.rs Sorting](https://docs.rs/gix/latest/gix/revision/walk/enum.Sorting.html)
- **Tree diff** (gix-diff) is done for both "changes needed to obtain the other tree" and tree-vs-tree.
- **Rename and copy tracking** is done for "Renames and copies by identity and by similarity, plus find-copies-harder". It has a documented deviation from git: it "uses the first-found candidate rather than Git's up-to-four candidates, and ignores filenames". Basename heuristics and directory-rename-by-similarity are not done. — [crate-status.md](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)
- gix 0.86.0 merged PR #2687, "fix/1832-rename-tracker-order-independent", so rename-tracker output used to depend on input order. — [gix CHANGELOG](https://github.com/GitoxideLabs/gitoxide/blob/main/gix/CHANGELOG.md)
- **Blob diff**: line diffs via `imara-diff` are done. `gix::object::blob::diff::Platform::line_counts()` will "Count the amount of removed and inserted lines efficiently". It returns `Option<DiffLineStats>`, which is `None` for binary input, and it builds no patch. `lines()` streams hunks. The docs say the algorithm comes from `diff.algorithm` or a diff driver. This is documented for gix 0.90.0, behind the `blob-diff` feature. — [docs.rs gix Platform](https://docs.rs/gix/latest/gix/object/blob/diff/struct.Platform.html)
- **Object database**: the "dynamic store" has "perfect scaling with cores", and gix-odb offers pack caches, object caches and MRU for per-thread performance. zlib-ng speeds up decompression by about 20%. "Creating an index from a pack alone" is "much faster than git". — [crate-status.md](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)
- gix 0.87.0 says "Make sure `gix-pack` uses the parallel feature". — [gix CHANGELOG](https://github.com/GitoxideLabs/gitoxide/blob/main/gix/CHANGELOG.md)
- **Replace refs**: object replacements (`git replace`) are done. Grafts are not mentioned. — [crate-status.md](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)
- **Shallow and partial clones**:
  - Shallow clone and fetch are done. Pruning non-existing shallow commits is not done.
  - Every promisor/partial-clone item is not done: marking promisor packs, on-demand fetch, and promisor-aware lookups. — [crate-status.md](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)
- **SHA-256**:
  - 0.85.0 added "adopt the remote's object format when cloning" for SHA-256 remotes.
  - 0.87.0 added "Recognize SHA-256 commit signature headers".
  - Unreleased: "Remove outdated note saying SHA-256 is not fully supported".
  - crate-status still lists "gix-ref: prepare for longer hashes" as not done. — [gix CHANGELOG](https://github.com/GitoxideLabs/gitoxide/blob/main/gix/CHANGELOG.md); [crate-status.md](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)
- **Merge base**: merge-base and octopus merge-base are done in gix-revision. — [crate-status.md](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)
- **Blame**: gix-blame does single-file annotation. Its performance "isn't competitive with Git" until three pending improvements land. — [crate-status.md](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)
- Caveat on the source itself: part of crate-status.md is a "plumbing overview" marked "Generated by Codex" with "Reconcile on demand", so individual checkboxes may lag the code. — [crate-status.md](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)

### Inferences
- **Bloom code is not redundant with gix, but it is likely unnecessary for this product.**
  - A changed-path Bloom filter can only answer "might path P have changed in commit C vs its first parent?" A filter of length one marks commits with zero or more than 512 changes (see Q4). For full per-commit change extraction, the Bloom filter can never let you skip computing a commit's change set, except for the rare "no changes" case.
  - It helps only path-limited queries such as "history of file X". Once FinalGitGraph persists exact per-commit change sets, its own index answers those queries exactly, with no false positives. That makes the custom murmur3 v1/v2 parser deletable.
  - One correctness risk if the code is kept: v1 filters are platform-dependent for non-ASCII paths (see Q5).
- **The pack tape walker overlaps with gix.**
  - gix-odb's dynamic store already provides per-thread pack and delta caches with multi-core scaling. gix-pack can traverse and resolve a whole pack via its index, and crate-status says this is faster than git's index-pack.
  - [Background] gix-pack's `index::File::traverse` with delta-tree lookup decodes every object in parallel. That is the natural primitive for a "decode all commits and trees once" cold ingest, but it also inflates blobs. Benchmark before deleting the tape walker. The tape walker may beat it only because it skips blobs.
- **Commit-graph preconditioning.** gix cannot write commit-graphs, so the benchmark's `git commit-graph write --changed-paths` step shells out to git.
  - For a product: after the first ingest, FinalGitGraph's own index (dense IDs, parents and generation numbers) replaces the commit-graph's role.
  - Running `git commit-graph write` should be an optional maintenance hint, never required for correctness or in the hot path.
- **gix upgrade.** Moving from 0.80 to 0.90 brings the order-independent rename tracker fix, generation exposure in `commit::Info`, SHA-256 fixes and parallel gix-pack. Upgrade before any redesign (inference from the changelog entries above).
- **What to reuse vs. build (recommendation):**
  - **Reuse from gix:**
    - ODB access, threaded via `ThreadSafeRepository::to_thread_local()` per worker [Background].
    - Commit-graph reading for parents, root tree, commit time and topological generation.
    - Commit parsing via `CommitRef` decode.
    - Tree-to-tree diff.
    - Exact rename detection, with your own verification of similarity renames (see Q4).
    - `line_counts()` for selective line stats.
    - Replace-ref handling, `.git/shallow` handling, SHA-1/SHA-256 abstraction (gix-hash), and merge-base.
  - **Build:**
    1. The persistent derived index: dense topological IDs, parent edges, generation numbers and per-commit change sets.
    2. The ref-snapshot diff and reachability tombstoning layer.
    3. A tree-pair diff cache keyed by `(tree_oid_a, tree_oid_b)`.
    4. A differential oracle harness.
  - **Do not build:** murmur3/Bloom parsing, a commit-graph writer, bitmap readers, or a reftable reader. For reftable, detect it and fail clearly, or fall back to `git for-each-ref` until gix-reftable lands.

### Gaps
- No gix commit-graph, tree-diff or rev-walk throughput numbers were found, measured or published. Searches for gix vs git tree-diff benchmarks found nothing authoritative.
- Whether `Sorting::BreadthFirst` and `ByCommitTime` read parents from the commit-graph without touching the ODB was not confirmed from docs.
- The exact API names for commit-graph use in `rev_walk` (for example `use_commit_graph`) were not verified on docs.rs this session.
- Whether gix's tree diff prunes equal-OID subtrees was not verified in source, though it is standard (see Q4).

---

## Q2. How fast git's own primitives are, with published and local numbers

### Takeaway
Commit enumeration and metadata are not the bottleneck. Git's single thread walks about 413k commits/s with a commit-graph and about 122k/s without one, and it parses full commit metadata at about 107k commits/s. First-parent tree diffing runs at about 19k non-merge commits/s single-threaded on git.git. Line stats (`--numstat`) are about 7x slower again, at about 2.7k diffs/s. Target ≥100k commits/s for enumeration and metadata, plus parallel and cached tree diff for change sets. Keep line stats lazy or selective. The current design does this, and the data supports it.

### Cited Findings
- **[Measured-local] on git.git, 85,936 commits, Git 2.39.5, M1 Pro:**

  | Command | Time | Throughput |
  |---|---|---|
  | `git -c core.commitGraph=false rev-list --all --parents` | 0.706 s | ≈122k commits/s |
  | `git commit-graph write --reachable` | 0.742 s | |
  | `git rev-list --all --parents` (with commit-graph) | 0.208 s | ≈413k commits/s |
  | `git rev-list --all --parents --topo-order` (with commit-graph) | 0.233 s | |
  | Same `--topo-order` without commit-graph | 0.713 s | |
  | `git log --all --format='%H %P %T %an %ae %at %ct %s'` | 0.799 s | ≈107k commits/s |
  | `git commit-graph write --reachable --changed-paths` (Bloom v1) | 5.954 s | ≈14.4k commits/s |
  | `git log --all --raw --no-renames --format=%H` (non-merge diffs only) | 3.402 s | ≈18.9k diffs/s |
  | Same without commit-graph | 3.718 s | |
  | `git log --all --name-status -M --format=%H` | 3.309 s | rename cost negligible here |
  | `git log --all --raw --no-renames --format=%H --diff-merges=first-parent` (includes merges) | 4.673 s | |
  | `git log --all --numstat --no-renames --format=%H` | 23.776 s | ≈2.7k diffs/s |
  | `git log -- Documentation/RelNotes/2.0.0.txt` with Bloom | 0.102 s | |
  | Same with `commitGraph.readChangedPaths=false` | 0.341 s | 3.3x speedup from Bloom |

  - The topological sort with commit-graph costs only about 12% more than unsorted.
  - The `log --format` row must inflate every commit object. Even with a commit-graph, per-commit metadata costs about 4x a pure graph walk.
  - The `--changed-paths` row is dominated by computing first-parent tree diffs for every commit.
  - The raw `--no-renames` output contains 143,174 `:` change lines, about 2.2 changes per non-merge commit.
- **Microsoft, 2018, Git 2.18 commit-graph (units unlabeled, presumably seconds):**
  - Linux, 722,849 commits: `git merge-base master topic` 0.52 to 0.06; `git branch --contains` 76.20 to 0.04; `git tag --contains` 5.30 to 0.03; `git log --graph -10` 5.90 to 0.74.
  - Windows repo, 2,214,796 commits: `git merge-base A B` 11.40 to 1.80; `git log --graph -10` 24.30 to 5.30; `git status --ahead-behind` 14.30 to 4.70.
  - No hardware was given. — [Microsoft DevBlogs: Supercharging the Git Commit Graph](https://devblogs.microsoft.com/devops/supercharging-the-git-commit-graph/)
- **Changed-path Bloom filters:**
  - The `git log -- <path>` integration reported a "2x to 5x" speedup on the git and Linux repos. — [gitgitgadget PR #497](https://github.com/gitgitgadget/git/pull/497); [git commit 78a1541](https://github.com/git/git/commit/78a1541e11b748d366841c5f0ecaf8eaa09e05f2)
  - A later series reported 10x to 30x for path-limited logs, under conditions that are unclear from the snippet. — [gitgitgadget PR #609](https://github.com/gitgitgadget/git/pull/609)
  - Rename detection is disabled while computing filters, because it caused "significant performance issues". — [gitgitgadget PR #497](https://github.com/gitgitgadget/git/pull/497)
- **Reachability bitmaps:** walking all objects in linux.git took more than 33 s without bitmaps and 1.57 s with them. GitHub added multi-pack bitmaps (MIDX bitmaps) because single-pack bitmaps forced whole-repo repacks. — [GitHub Blog: Scaling monorepo maintenance (2021)](https://github.blog/2021-04-29-scaling-monorepo-maintenance/)
- A 2023 Git change, "bitmap boundary traversal", builds bitmaps only from the have/want boundary. Gitaly put it behind a feature flag because the speedup was unconfirmed. — [GitLab Gitaly MR 6428](https://gitlab.com/gitlab-org/gitaly/-/merge_requests/6428)

### Inferences
- **Enumeration.** At about 413k commits/s single-threaded with a commit-graph, enumeration from a commit-graph can reach ≥100k commits/s with one core, so no exotic walker is needed. Without a commit-graph, the cost is inflating commit objects, at about 120k/s single-threaded in git. Parallel inflate across packs, or gix-pack's parallel traverse, should clear 100k/s. This is unmeasured for gix.
- **Change sets.**
  - 100k commits/s with full first-parent change sets means roughly 5x git's single-thread tree-diff rate on git.git. That needs about 6 or more cores at git-like efficiency, a tree-pair cache, or both.
  - linux.git trees are larger and deeper, so the per-commit cost will be higher there. This is unmeasured; see Gaps.
  - The "metadata-only by default, selective line stats" policy is validated. Full `--numstat` on every commit would cap at a few thousand commits/s per core.
- **Bitmaps.** Bitmaps speed up object reachability ("which objects are reachable"), not commit-graph walking or tree diffs. They matter for gc/repack/fetch, not for this ingestion pipeline. Skip them.

### Gaps
- No published git numbers for full-history `git log --raw` or `--name-status` throughput on linux.git, chromium or other monorepos were found.
- The local numbers come from Git 2.39.5 (2022-era Apple build), not Git 2.5x. Newer Git has more tree-diff and Bloom v2 improvements.
- linux.git and chromium were not cloned (size and time).
- No numbers for `commit-graph write --split` were found or measured.

---

## Q3. How large systems build derived data from git, and design lessons

### Takeaway
The systems that work at scale converge on three ideas:
1. Derived data is a pure function of an immutable changeset plus its parents' derived data, keyed by content hash (Mononoke).
2. The commit DAG is stored as dense integer IDs assigned in topological order, so ancestry becomes range arithmetic (Sapling Segmented Changelog, git commit-graph positions).
3. Ref state is snapshotted per operation, and changes are found by diffing snapshots (jj op log and view), not by trusting incremental hooks.
Precomputing per-commit answers and recomputing asynchronously on a "graph dirty" flag is the pattern Sourcegraph uses.

### Cited Findings
- **Sapling Segmented Changelog:**
  - The server sends "the high-level shape of the commit graph", "taking just a few megabytes", and commit data fills in lazily.
  - Graph queries between two commits take "O(number-of-merges) time".
  - Log and blame on a file can bisect the segment graph in O(log n) rather than O(n), "including in Git repositories".
  - Smartlog takes under a second regardless of repo size.
  - Meta's monorepo has "tens of millions of files, tens of millions of commits, and tens of millions of branches". — [Meta Engineering: Sapling (2022-11-15)](https://engineering.fb.com/2022/11/15/open-source/sapling-source-control-scalable/)
- **sapling-dag crate:**
  - `IdMap` maps vertex names to integer `Id`s.
  - `assign_head` assigns IDs to a head and its ancestors incrementally ("if the head or any of its ancestors already have an id stored in this map, the existing ids will be reused").
  - IDs belong to groups (master and non-master).
  - `IdDag` is built from `PreparedFlatSegments`.
  - A lazy IdMap can represent a node as "first descendant + distance", given known merge parents and heads. — [docs.rs sapling-dag idmap.rs](https://docs.rs/sapling-dag/latest/src/dag/idmap.rs.html)
- **Mononoke derived data** (secondary source only):
  - Mononoke keeps the changeset (Bonsai) as the core unit and stores query structures as separate derived data. Each derived type is defined recursively from the changeset and its parents' derived values.
  - As of 2026-10-15, the Sapling repo's `derived_data` directory includes `blame`, `fastlog`, `inferred_copy_from`, `deleted_manifest`, `skeleton_manifest`, `unodes` and `fsnodes`. — [braindetox.kr critique (2026, secondary)](https://braindetox.kr/en/posts/anthropic_scion_mononoke_derived_data_critique_2026.html)
- **jj operation log and view:**
  - Each operation points to a "view" holding visible heads, bookmarks, tags and working-copy commits.
  - Commands load a fixed snapshot and write a new operation whose parent is the starting operation.
  - Concurrent operation heads are 3-way-merged on their views, and conflicts are recorded, for example a bookmark "moved from A to B or C".
  - Atomicity comes from creating an op-head file named by operation ID, then removing the old one. — [jj docs: concurrency](https://docs.jj-vcs.dev/latest/technical/concurrency/)
- **jj GitBackend:**
  - It uses gitoxide to read and write commits and refs.
  - It keeps a ref per op-log commit under `refs/jj/keep/` so git gc cannot delete them.
  - jj-only metadata (change ID, predecessors) lives in a `StackedTable` under `.jj/repo/store/extra/`.
  - For git-created commits, the change ID is derived from the bit-reversed commit ID. — [jj docs: architecture](https://docs.jj-vcs.dev/latest/technical/architecture/)
- **jj import behavior:**
  - `jj git import` abandons commits no longer reachable from any git ref by default, controlled by `git.abandon-unreachable-commits` (default true). — [jj-cli 0.44.0 import.rs](https://docs.rs/crate/jj-cli/0.44.0/source/src/commands/git/import.rs)
  - Fetch pins visible untracked remote refs, so ancestors of new refs are not wrongly abandoned. — [jj PR #3500](https://github.com/martinvonz/jj/pull/3500)
  - Force-pushing a tracked bookmark to a non-descendant still causes friction. — [jj discussion #9994](https://github.com/jj-vcs/jj/discussions/9994)
- **Sourcegraph code intel:**
  - Sourcegraph moved from query-time graph traversal to precomputing visible uploads per commit, stored in `lsif_nearest_uploads`.
  - These are recomputed when the commit graph or the uploads change. There is one nearest upload per indexing root, and visibility flows from both ancestors and descendants. — [Sourcegraph blog part 1](https://sourcegraph.com/blog/optimizing-a-code-intel-commit-graph); [part 2](https://sourcegraph.com/blog/optimizing-a-code-intelligence-commit-graph-part-2)
  - Repos are flagged dirty and a worker rebuilds the commit graph asynchronously. While the flag is set, results may be stale. — [Sourcegraph docs 4.4: uploads](https://docs.sourcegraph.com/@4.4/code_navigation/explanations/uploads)
- **Microsoft and GitHub:**
  - The commit-graph (positions as integer parents, generation numbers) was built for the 2.2M-commit Windows repo. — [Microsoft DevBlogs](https://devblogs.microsoft.com/devops/supercharging-the-git-commit-graph/)
  - GitHub built MIDX bitmaps to avoid whole-repo repacks. — [GitHub Blog](https://github.blog/2021-04-29-scaling-monorepo-maintenance/)

### Inferences
- **Lesson 1: key derived data by commit OID, not by "position in the walk".**
  - A commit's parents, root tree, and first-parent change set (under a fixed diff config) are pure functions of its OID. History rewrites, force-pushes, gc and repack never invalidate them. They only change which commits are reachable.
  - Exceptions: replace refs, grafts and shallow boundaries change effective parents (Q5). Version the diff config, as Mononoke versions derived-data types [Background].
- **Lesson 2: assign dense u32/u64 IDs in a topological order.**
  - Use a stable "main" group: first-parent history of the default branch, extended append-only. Use a rebuildable "other" group for side branches and rewritten commits.
  - This gives Sapling-style O(#merges) ancestry and lets the store use arrays and bitmaps (roaring) for reachability sets instead of hash maps.
- **Lesson 3: model refs as snapshots.**
  - Each ingest records `{ref -> oid}` as an "operation", like jj's view.
  - Incremental work is the diff between the last snapshot and the current refs. It needs no reflog parsing and no trust in hooks.
- **Lesson 4: precompute per-commit answers, and recompute lazily and asynchronously behind a dirty flag** (Sourcegraph). The current LMDB retry queue is a reasonable form of this. Make it keyed by OID and idempotent.
- [Background, unverified] More systems that were not fetched:
  - jj's default index is a stack of immutable segment files, each adding commits with parent positions and generation numbers on top of a parent segment. That is the same idea as split commit-graph chains.
  - GitHub's Spokes replicates whole repos; derived data isn't its focus.
  - Gitaly wraps the git CLI behind gRPC and maintains commit-graphs/MIDX via housekeeping.
  - Zoekt indexes only branch tips, not history.
  - JGit DFS plus reftable came from Google's Gerrit hosting.
  - Piper and CitC are not git-based, and their lessons are mostly "virtualize the working copy, server-side history".

### Gaps
- No primary Mononoke derived-data documentation was retrieved. The facts about unodes, fsnodes and derivation ordering above rest on a secondary 2026 source.
- No primary Gitaly, Spokes, git-sizer, Zoekt, Piper/CitC, Gerrit/JGit-DFS, libgit2 or Kart sources were fetched in this session (tool budget).
- The Segmented Changelog paper cited in the sapling-dag source was not retrieved.
- jj's index format and its exact ref-import diff algorithm (`git_refs` in the view) were not found in the fetched docs.

---

## Q4. State of the art for scalable per-commit file-change extraction (tree diff, renames, line stats)

### Takeaway
The fast path is first-parent tree-vs-tree diff with equal-OID subtree pruning. It runs in parallel by commit, uses per-thread object caches, and memoizes on `(old_tree_oid, new_tree_oid)`. Exact renames come from blob-OID equality, which is cheap and deterministic. Similarity renames are bounded and treated as best-effort, since gix's algorithm deliberately differs from git's. Line counts come from `imara-diff`'s Histogram algorithm via `line_counts()`, computed lazily. Git's own rename-performance work (merge-ort) shows the big wins come from shrinking the candidate set, not from faster similarity scoring.

### Cited Findings
- **Changed-path Bloom filters (BDAT):**
  - Each filter covers "the paths that were changed between the commit and its first parent, if requested".
  - The header holds the hash version, the number of hashes, and the minimum bits per entry.
  - "Commits with no changes or more than 512 changes have Bloom filters of length one, with either all bits set to zero or one respectively."
  - The current hash version is 2, a 32-bit murmur3 with double hashing (seeds `0x293ae76f`, `0x7e646e2`). — [gitformat-commit-graph](https://git-scm.com/docs/gitformat-commit-graph)
- **CDAT layout:** each commit stores its root tree OID and its first two parent positions (`0x70000000` = none; MSB set means an index into EDGE for octopus merges). It also packs a 30-bit topological level and a 34-bit commit time into 8 bytes. GDA2 stores corrected-commit-date offsets, with GDO2 for overflow. — [gitformat-commit-graph](https://git-scm.com/docs/gitformat-commit-graph)
- **[Measured-local] costs on git.git:**
  - Tree diff for all non-merges is 3.4 s, versus 3.3 s with `-M` rename detection.
  - `--numstat` is 23.8 s, so blob line diffing dominates once enabled.
  - Including merges as a first-parent diff adds about 37% (4.67 s vs 3.40 s).
- **imara-diff:**
  - It offers linear-space Myers, with heuristics against quadratic blowup, and Histogram, ported from git and "heavily optimized".
  - Histogram is "10% - 100%" faster than Myers across workloads.
  - It "closely match[es] the behavior of gnu-diff and git".
  - It is benchmarked on Linux, Rust, VSCode and Helix histories, with tree diffs extracted by gitoxide.
  - The input cap is 2^31 - 2 tokens. — [imara-diff README](https://github.com/pascalkuthe/imara-diff)
- gix `line_counts()` returns removed and inserted counts without building a patch, and returns `None` for binary input. — [docs.rs gix Platform](https://docs.rs/gix/latest/gix/object/blob/diff/struct.Platform.html)
- **gix rename tracker vs git:**
  - It takes the first-found candidate, while git considers up to four candidates. It ignores filenames.
  - It has no "best candidate" heuristics and no find-by-basename.
  - It supports directory tracking by identity only. — [crate-status.md](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)
- **Git merge-ort rename optimizations (Elijah Newren, 2021):**
  - Rename detection is restricted to a caller-provided set of "relevant sources".
  - It skips rename detection entirely when possible.
  - Basename-guided inexact matching uses nearby exact renames as hints. One benchmark in the commit message dropped from about 1800 s to about 187 s.
  - A rename cache avoids repeated detection, with "no behavioral changes" since it uses no heuristic. — [diffcore-rename: guide inexact rename detection based on basenames (mirror)](https://timplab.syktsu.ru/admin/git/commit/bd24aa2f97a0); [lore.kernel.org series v3](https://lore.kernel.org/git/pull.859.v3.git.1621490982.gitgitgadget@gmail.com/)

### Inferences
- **Tree-pair memoization.**
  - [Background] Git and gix tree diffs walk two sorted trees in lockstep and skip any subtree whose OIDs are equal, so cost scales with changed directories × directory width, not repo size.
  - Wide directories, like Linux's `drivers/*`, are the hot spot. Cache decoded tree objects per thread, which gix's object cache does.
  - Also memoize `(tree_a, tree_b) -> change list` for subtrees: cherry-picks, reverts, and merges whose first parent equals a previous commit's tree repeat identical tree pairs. This is unmeasured; validate the hit rate on real repos.
- **Merge commits.** Diff against the first parent only. This matches git's Bloom definition and `--diff-merges=first-parent`. Treat it as the canonical "change set" and store parent-2+ diffs lazily if ever needed. Merges cost noticeably more (measured +37% total when included).
- **Rename policy for an indexer.**
  1. Exact renames: a deleted blob OID equals an added blob OID. These are deterministic and cheap, matching git's exact pass.
  2. Similarity renames: only when the remaining added/deleted counts are under a cap (git's `diff.renameLimit` idea). Version the result with the algorithm and threshold.
  3. Don't promise git-identical similarity renames while gix keeps its "first-found candidate" deviation. Either accept divergence and document it, or implement git's candidate scoring yourself on top of gix's similarity score. An oracle will surface differences (Q6).
- **Line stats.** Use `line_counts()` with the Histogram algorithm, computed lazily per visible commit, or in a background low-priority queue. Cache by `(blob_old, blob_new)`, because blob pairs repeat across cherry-picks and reverts. Skip binary blobs and blobs above a size cap.
- **Parallelism shape.**
  - Partition by commit, since each commit's first-parent diff is independent, and give each worker its own `Repository` handle and object cache.
  - Order the work so consecutive commits on one worker share trees, for cache locality, for example contiguous chunks of the topological order.
  - Rayon with ≤8 threads is reasonable. Contention points are zlib inflate and pack delta resolution, which gix-odb parallelizes.

### Gaps
- There are no 2023–2026 published papers or benchmarks specific to "per-commit change-set extraction at scale". The literature found is git mailing-list work from 2020–2021 and imara-diff's plots, which give no absolute numbers in text.
- Tree-pair cache hit rates on real repos are unknown and need measurement.
- gix tree-diff per-commit cost vs git was not measured, because the Mac environment lacks cargo per the project memory notes.

---

## Q5. Incremental correctness: rewrites, gc, commit-graph staleness, hash formats, shallow/partial clones, submodules, replace/grafts

### Takeaway
Correct incrementality comes from snapshot-diffing refs and recomputing reachability, while never mutating per-OID facts. Git's own commit-graph is only an accelerator with hard validity rules:
- It is ignored when replace objects, grafts or shallow commits are present.
- It does not cover commits written after it.
- It must match the repo's hash algorithm.
v1 Bloom filters are unreliable for non-ASCII paths across platforms. Partial clones (missing blobs or trees) are not supported by gix's ODB, so they must surface as an explicit "unavailable" state, never as fabricated empty diffs.

### Cited Findings
- "The commit-graph will not be read or written when replace-objects or grafts are present." "The commit-graph will not be read or written when shallow commits are present." The reason given is that shallow clones "create grafts of commits by dropping their parents", so generation numbers become invalid once the clone is unshallowed. — [git technical/commit-graph](https://git-scm.com/docs/commit-graph)
- The commit-graph is supplemental: "the existing object database is sufficient". Commits not in the file get `GENERATION_NUMBER_INFINITY`. — [git technical/commit-graph](https://git-scm.com/docs/commit-graph)
- **Split chains:**
  - Files live at `info/commit-graphs/graph-{hash}.graph`, listed by `commit-graph-chain`, with positions cumulative across layers.
  - Merges are governed by `--size-multiple` and `--max-commits`.
  - Unreferenced files are kept until an expiry, which defaults to zero.
  - If the top layer lacks GDA2, Git uses only topological levels. — [git technical/commit-graph](https://git-scm.com/docs/commit-graph)
- The commit-graph header hash version is "1 => SHA-1, 2 => SHA-256". On mismatch, "the commit-graph file should be ignored with a warning". — [gitformat-commit-graph](https://git-scm.com/docs/gitformat-commit-graph)
- **Bloom v1 bug:**
  - "Version 1 Bloom filters have a bug that appears when char is signed and the repository has path names that have characters >= 0x80". Git can still read and write them, "but this ability will be removed in a future version of Git." — [gitformat-commit-graph](https://git-scm.com/docs/gitformat-commit-graph)
  - The fix added v2 via `commitGraph.changedPathsVersion` (-1, 0, 1, 2). The default -1 uses the repository's version, or 1 if none exists. There is no salvage path: filters must be recomputed. — [Taylor Blau patch](https://lore.kernel.org/r/d2f11c082d3bf10d9127c330a7d59b7e47ac4f21.1706741516.git.me@ttaylorr.com); [commitgraph config doc (mirror)](https://gitea.com/lunny/git/src/branch/master/Documentation/config/commitgraph.txt)
  - [Measured-local] Apple Git 2.39.5 writes v1 filters.
- gix handles `git replace` but not partial-clone promisor objects, and shallow-commit pruning is not done. — [crate-status.md](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)
- jj keeps commits alive across git gc via `refs/jj/keep/*`. — [jj architecture](https://docs.jj-vcs.dev/latest/technical/architecture/)
- jj abandons commits that become unreachable from git refs on import (`git.abandon-unreachable-commits`, default true). — [jj-cli import.rs](https://docs.rs/crate/jj-cli/0.44.0/source/src/commands/git/import.rs)
- Concurrent jj operations are reconciled by 3-way-merging views. — [jj concurrency](https://docs.jj-vcs.dev/latest/technical/concurrency/)
- Sapling `IdMap::assign_head` reuses existing IDs for already-known ancestors, which makes it incremental. — [sapling-dag idmap.rs](https://docs.rs/sapling-dag/latest/src/dag/idmap.rs.html)

### Inferences
- **The incremental algorithm** (synthesis of jj and Sapling):
  1. Read the current refs. Under the policy, these are `refs/heads/*` and a detached HEAD.
  2. Diff against the last stored snapshot to get, per ref, `(old, new)`: added, deleted or moved.
  3. Walk new commits from new tips until reaching already-indexed OIDs. The known set is a dense-ID bitmap or hash set. Assign new IDs incrementally.
  4. For moved or deleted refs, check fast-forward with generation numbers or merge-base, where `old` must be an ancestor of `new`. On a non-fast-forward, compute `reachable(old) − reachable(all current tips)`. Mark those commits unreachable (tombstone) and keep their per-OID data, so a later re-push of the same OIDs is free.
  5. Commit the new snapshot and the index updates in one LMDB transaction, which gives jj-style atomic "operations".
- **Why not reflog.** Reflogs can be disabled or expired, and remote-tracking refs are excluded from them. Snapshot diffing is robust to all of that.
- **Safety under gc/repack.**
  - Never persist pack offsets or pack-relative positions as durable keys. The current "pack-aware tape" caches must be keyed by pack checksum and invalidated when the pack set changes. Repack rewrites packs. gc prunes unreachable objects, so a tombstoned commit's objects may vanish.
  - Store everything the UI needs, such as metadata and change sets, in your own DB rather than re-reading git objects for unreachable commits.
- **Commit-graph staleness rules.** Use the commit-graph only when all of these hold:
  - no `.git/shallow`, no `info/grafts`, and no `refs/replace/*` (unless `core.useReplaceRefs=false` / `GIT_NO_REPLACE_OBJECTS`), mirroring git;
  - the hash version matches;
  - each commit lookup falls back to the ODB on miss.

  The current "if commit-graph exists, walk it" logic needs these guards. Otherwise, shallow repos and replace refs produce wrong parent edges.
- **Effective parents.** Make "effective parents" an explicit, versioned input. If replace refs or the shallow set change, recompute derived data only for affected commits. Key it as `(commit_oid, parents_fingerprint)`.
- **Hash formats.** Store OIDs as variable-length (20 or 32 bytes) with a per-repo object-format tag, using gix-hash's `ObjectId`. Never hard-code 20 bytes in the DB schema. Git 3.0's planned move to SHA-256 and reftable defaults is reflected in gix's "Git 3.0 compatibility" item.
- **Reftable.** gix cannot read reftable today. Detect `extensions.refStorage=reftable` and either fail loudly or fall back to `git for-each-ref --format='%(refname) %(objectname)'`. That is one subprocess per refresh, not per commit, which justifies the shell-out.
- **Bloom v1.** A v1 filter written by git on a signed-char platform (x86, MSVC, Apple arm64) hashes non-ASCII paths differently than murmur3 proper. aarch64 Linux uses unsigned char [Background on ABIs]. The reader cannot know the writer's platform, so a mismatch yields false "definitely not changed", a correctness bug. If Bloom code is kept, trust v1 only for ASCII paths, or only v2.
- **Partial clones** (`--filter=blob:none` / `tree:0`). Missing blobs break line stats and similarity renames. Missing trees break change sets entirely. Detect `remote.<name>.promisor` / `extensions.partialClone`. Record per-commit status `{complete, missing_objects}` and offer an explicit fetch action. Never emit "no changes" for a missing object.
- **Submodules.** Gitlink entries (mode 160000) appear in tree diffs as OID changes of a commit that isn't in this repo. Record them as a "submodule pointer moved" change and never try to read the target.
- **Shallow.** Commits in `.git/shallow` have their parents hidden. Mark them as boundary commits with "parents unknown", not "root". Re-derive them if the shallow set shrinks.

### Gaps
- No primary source was found for how jj stores the last-imported git ref state (`git_refs` in the view). The behavior above is inferred from the docs and the import.rs reference.
- Whether gix's commit-graph-backed traversal automatically disables itself for shallow, graft or replace repos was not verified.
- Git's grafts file (`info/grafts`) is deprecated. gix support for it is not mentioned in crate-status.

---

## Q6. What a correctness oracle looks like

### Takeaway
Run differential tests against the git CLI in CI and in an opt-in "verify" command, never in the hot path. Canonical answers come from plumbing with stable, NUL-delimited output: `rev-list --parents`, `log`/`diff-tree --raw -z --no-abbrev` (with and without `-M`), `--numstat`, `merge-base --all`, `for-each-ref` and `commit-graph verify`. Compare sets of facts per OID, and run against a fixture matrix of edge cases plus real repos.

### Cited Findings
- `core.commitGraph=false` makes git use only the ODB, because the commit-graph is "supplemental", so the same query can be answered with the commit-graph on and off as a self-check. — [git technical/commit-graph](https://git-scm.com/docs/commit-graph)
- [Measured-local] The oracle commands are cheap enough for CI on mid-size repos. On git.git, `rev-list --all --parents` takes 0.2–0.7 s, `log --raw` about 3.4 s, `log --name-status -M` about 3.3 s and `log --numstat` about 24 s.
- gix rename output differs from git by design (first-found candidate, filenames ignored). — [crate-status.md](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)
- imara-diff claims close behavioral match with git and gnu-diff, but not an exact one. — [imara-diff README](https://github.com/pascalkuthe/imara-diff)

### Inferences
- **Oracle commands** (canonical, script-parseable; run with `-c core.quotePath=false` and NUL output):

  | Fact | Git command | Notes |
  |---|---|---|
  | Commit set and parents | `git rev-list --parents <tips…>` | Use the same tips as the policy: `refs/heads/*` plus HEAD. Compare as a set of `(oid, [parents])`, ignoring order. |
  | Order invariant | `git rev-list --topo-order` | Assert parent-before-child (or child-before-parent) rather than an exact sequence, because ties are unspecified. |
  | Metadata | `git log --format='%H%x00%P%x00%T%x00%an%x00%ae%x00%at%x00%cn%x00%ce%x00%ct%x00%B' -z` | Compare bytes, including non-UTF-8 author names and the `encoding` header. |
  | Change sets (no renames) | `git log --raw -z --no-abbrev --no-renames --diff-merges=first-parent --format=%H <tips>` or `git diff-tree --stdin -r -z --no-abbrev --no-renames` | Exact equality expected: status letter, old/new mode, old/new OID, path. |
  | Exact renames | Same command with `-M100%` | Exact equality expected for identical-blob renames. |
  | Similarity renames | `-M` (default 50%) | Report divergences, don't fail. Classify each as gix-first-candidate vs git-best-candidate. |
  | Line stats | `git log --numstat -z --no-renames` | Expect equality with `diff.algorithm` pinned on both sides (histogram vs myers counts can differ). Binary shows `-\t-`. |
  | Ancestry | `git merge-base --all A B` and `git merge-base --is-ancestor A B` | Over random pairs. |
  | Refs | `git for-each-ref --format='%(refname)%00%(objectname)'` | |
  | Graph sanity | `git commit-graph verify`, `git fsck --connectivity-only` | Fixture validation. |

- **Fixture matrix** (small synthetic repos built by script; on this machine use `git init` with explicit config):
  - octopus merges (EDGE chunk), empty commits, root commits, multiple roots;
  - commits with more than 512 changes (Bloom all-ones);
  - non-ASCII and invalid-UTF-8 paths (Bloom v1 bug, quoting);
  - mode-only changes, symlinks, type changes (file to symlink), gitlinks (submodules);
  - case-only renames (Windows/macOS case-insensitivity);
  - `git init --object-format=sha256`;
  - `--depth` shallow clones, `git replace` objects, `--filter=blob:none`;
  - force-push and rewrite sequences (rebase, then gc `--prune=now`);
  - repack mid-ingest;
  - split commit-graph chains, and a commit-graph that is stale relative to new commits.
- **Real-repo tier:** git.git (86k commits, about 35 s to clone) in CI. linux.git (more than 1M commits) nightly, with sampling: verify a random 1% of OIDs fully, plus all merges.
- **Property tests:**
  - generation(child) > generation(parent);
  - every non-root commit's first-parent change set, applied to the parent tree, yields the child tree. This is a self-consistency check that needs no git.
  - incremental ingest after N random ref moves equals a from-scratch ingest (the metamorphic test for Q5).

### Gaps
- No published differential-testing harness for gitoxide vs git was found to copy from. gitoxide's own test suite uses git-generated fixtures [Background], but it was not examined this session.
