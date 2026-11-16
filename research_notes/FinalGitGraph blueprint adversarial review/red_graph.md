# Red-team review: graph representation, identity and query-algorithm claims in the FinalGitGraph blueprint

Scope: adversarial review of `reports/FinalGitGraph backend architecture blueprint.md` (sections "Identity: a dense parent-first integer makes the bug unrepresentable" and "Queries: ID order replaces hydration", plus the Phase 6 and kill-criteria tables) and of `research_notes/FinalGitGraph backend architecture blueprint/graph_representation_and_queries.md` (Q5–Q8). Current query code: `apps/desktop/src-tauri/src/application/indexer.rs` (`QueryGraph`, lines 1805–2585). Checked as of 2026-11-01.

Evidence types used:
- Primary sources: sapling-dag source (docs.rs and raw GitHub), jj-lib source, swh-graph docs, git docs and blogs.
- Repo code lines, cited as file:line.
- A small worked counterexample DAG for each logical attack, worked by hand.
- **Local measurement on git.git.** A throwaway Python simulation applies the blueprint's numbering to the real git.git DAG: `git clone --bare --filter=tree:0` of https://github.com/git/git, 85,936 commits, all refs, cloned 2026-11-01. Script: `/private/tmp/claude-502/-Users-singh-deependra-int-Desktop-git-FinalGitGraph/685bd50d-b5cb-476f-a4e4-79aaa3f5ca45/scratchpad/analysis/dagsim.py` and `dagsim2.py`. That scratchpad is temporary. The method is described inline so the blue team can reproduce it. No repo file was modified.
  - Simulation caveats: one repo only, Python, and a walk cost counted in nodes rather than timed.
  - Tip order: `master` first, then the remaining DAG heads by committer time.
  - Incremental model: 200 committer-time slices, each closed under parents.

Labels: "Evidence" means a primary source, a code line or a local measurement. "Inference" means my reasoning.

---

## Q1. Ranked attack list (summary)

### Takeaway
The numbering invariant itself holds. On git.git, a parent-first DFS gives zero parent ≥ child violations, and the three O(1) cascade tests never gave a wrong answer. The blueprint fails around that invariant.
- **Shortest path (wrong answers).** The ID-interval restriction for shortest path is wrong for `direction=both`, which the API and the perf harness use. It gives a wrong answer on 112 of 120 random git.git pairs.
- **Numbering order (lost O(1) answers).** The chosen "first parent last" order throws away the property that would make main-line ancestry exactly O(1). Sapling's own code says that order is wrong for incremental builds.
- **ID-dependent outputs.** Several query outputs depend on IDs, and IDs depend on ingestion history. That contradicts the batch-invariance contract.
- **Other weaknesses.** Shallow deepen breaks "parent < child, no relabel". The topo stream "with no heap" can scan almost the whole repo for `-n 100`. The layer format has no children index. The segments/N ≤ 0.3 kill criterion is arbitrary, and git.git measures 0.41. The memory comparison understates today's cost by about 2x.

### Cited Findings
See Q2–Q9 for the full evidence. Headline evidence:
- `direction=both` shortest path restricted to [min id, max id]: no path found in 11/120 random git.git pairs, and a longer-than-shortest path in 101/120 — [local git.git measurement](https://github.com/git/git), script `dagsim.py`
- Main-line O(1) coverage: with first-parent-FIRST numbering, all 24,442 commits on master's first-parent chain satisfy ancestors(c) = [0, id(c)]. With the blueprint's first-parent-LAST order, only 16 do — [local git.git measurement](https://github.com/git/git), `dagsim2.py`
- Sapling: "the above visit order (first parent last) is not optimal for incremental build case with pushrebase branches". Code: `// Assume incremental updates with pushrebase.` → `VisitOrder::FirstFirst` — [sapling idmap.rs](https://github.com/facebook/sapling/blob/main/eden/scm/lib/dag/src/idmap.rs)
- Topo stream from `master`, limit 100: full ingest and 200-refresh incremental ingest return different sequences, with a 92/100 set overlap — [local git.git measurement](https://github.com/git/git)
- Linear descending-ID scan, `-n 100` from one tag head (v1.4.4.5 lineage): 77,495 of 85,936 IDs scanned to emit 100 commits — [local git.git measurement](https://github.com/git/git)

### Inferences
Ranked attacks (severity: critical > major > minor):

| # | Claim attacked (location) | Attack | Severity | Proposed revision |
|---|---|---|---|---|
| 1 | "Shortest path runs as a bidirectional BFS restricted to the ID interval `[id(B), id(A)]` and to generation bounds". Blueprint, *Queries*; note Q6 "Shortest path in a DAG"; note Q8 item 4 | Valid only for directed paths. For `direction=both` (exposed by the API, used at `indexer.rs:1276` and `bin/perf-report.rs:306`) the shortest undirected path usually leaves the interval through a common ancestor or common descendant. It gave a wrong answer on 112/120 git.git pairs (Q2) | **Critical** | Apply the interval and generation bounds only for `parents` and `children`, with the interval written as [min(id), max(id)]. For `both`, run an unrestricted bidirectional BFS with a budget, and add a tri-state result (found / not found / budget exhausted) |
| 2 | "Assignment uses an iterative DFS from the tips that visits the first parent last…" (blueprint *Identity*), cited as "Sapling `assign_head` style" (note Q8 item 1) | Mis-cited and suboptimal. Sapling uses FirstLast only for from-scratch builds and FirstFirst for incremental ones, and reserves ID ranges for mainline growth. First-parent-last loses the main-line prefix property: 16 vs 24,442 qualifying master commits on git.git. Main-line ancestry queries then fall to walks 42% of the time instead of 0% (Q3, Q4) | **Major** | Number the main tip first with first-parent-FIRST, or keep FirstLast and add a 4-byte "main-line containment" column. Pre-register the choice in ADR-0023 with this git.git data |
| 3 | "Topological streaming … becomes 'iterate descending IDs over the visible set'"; "Every API … returns partial results deterministically"; metamorphic relation "Full ingest must equal prefix-then-rest ingest" | IDs depend on ingestion history: refresh cadence, tip order, ingest path. So every ID-ordered or ID-truncated output does too. A 6-commit hand counterexample and git.git (92/100 overlap) show it. The oracle cannot derive expected values for these queries from git (Q5, Q9) | **Major** | Define query outputs by content-only tie-breaks (for example generation, then corrected date, then OID), or exclude ID-ordered outputs from the invariance contract and test them for topological validity only |
| 4 | "A parent that is truly absent, such as a shallow boundary or a missing object, becomes a typed boundary record" plus "parent < child" with no relabeling (blueprint *Identity*); note Q8 item 1: "an edge is attached when the parent arrives (the ID still increases…)" | Contradiction. A shallow commit C already holds an ID. After `fetch --deepen` or `--unshallow`, its parent P must get an ID lower than C's, which is impossible without renumbering. The same happens when grafts or replace refs are removed. The note's "parked in a pending set" (Q5) contradicts the report's "typed boundary record" | **Major** | State explicitly that deepen, unshallow and graft or replace changes trigger a blue/green full renumber. Or reserve an ID gap below boundary commits. Test it in the fixture matrix (which already lists shallow clones) |
| 5 | "Topological streaming with a limit becomes 'iterate descending IDs …', with no in-degree pass or heap" (blueprint *Queries*); note Q6 "O(k + spans touched). No heap" | Without segments, a linear backward scan costs the length of the ID interval, not k. On git.git the median is 156 IDs for k=100, but the maximum is 77,495 (90% of the repo) from a late-numbered old tag. At 10M commits that is a near-full scan for `-n 100` (Q9) | **Major** | Use a max-heap keyed by ID over the frontier, at O(k log w). It needs no in-degree pass and still gives descending-ID order |
| 6 | "Expected: O(1) for most queries" (note Q6), backed by O'Reach's ~95% negative and >70% positive O(1) rates on generic graphs (note Q2, Q8) | Not true for commit DAGs under this cascade. On git.git, positive pairs reach the walk 92% of the time (1,383/1,500), uniform pairs 45%, and main-line B 42%. The walk peaks at 73,516 nodes (86% of the repo). O'Reach's rates come from non-VCS benchmark graphs (Q3) | **Major** | Drop the "most queries O(1)" claim. Add the exact-positive interval test and a main-line prefix or containment test. Gate on measured walk p99 |
| 7 | Layer chunk list "[hdr \| OID fanout + sorted OIDs + perm \| parents (global ids) \| gen level \| corrected date \| …]" (blueprint diagram); note Q8: "Children are derived from segments plus a per-layer delta map" | No children index is stored, and segments are deferred. Yet `direction=children`, `direction=both`, the backward side of the bidirectional BFS and descendants all need children. Old commits gain children in every new layer, so a mutable overlay (today's `overlay_child_neighbors` HashMap) or an O(N) inversion at open returns. That contradicts "ID order replaces hydration" | **Major** | Add a children chunk per layer plus a "new children of old IDs" delta chunk, merged at compaction. Or state the open-time inversion cost and memory (about 84 MB at 10M) |
| 8 | Kill criterion "Segments/N > 0.3: keep CSR-in-layers with generation pruning only" (blueprint kill table; Phase 6) | Arbitrary. It comes from a storage-only model (frontier note D2) and ignores the 16x-per-level hierarchy that gives O(merges) queries. git.git measures 0.41 (Sapling flat segments) and would be killed. Linux, at 7–9% merges, would pass. So the outcome depends on workflow, not on query latency (Q6) | **Major** | Replace it with a latency criterion: p99 is_ancestor, merge-base and range for CSR+generation versus segments on the target repos, at a pre-registered budget |
| 9 | "`A..B`, `A...B` and merge-base become set algebra over ID bitsets (1.25 MB per 10M commits) or span lists" | Without segments, ancestors(B) as a set costs an O(id(B)) backward scan. "heads(ancestors(A) ∩ ancestors(B))" needs both full sets. The early-stop rules (git's "all stale" rule) are not stated. Cost is O(ID range), which includes unrelated interleaved branches (Q9) | **Major** | Specify the colored descending-ID paint walk with stale counting (git's `paint_down_to_common` semantics on ID order) and its stop rule. Claim O(ID range) cost, not O(spans), until segments exist |
| 10 | Memory: "about 85–100 bytes per commit … roughly 1 GB at 10M"; "about 24 bytes per commit for sorted OIDs plus permutation"; "10M-commit topology layer set of about 360 MB"; note Q3 "10–18 MB" segment encoding | The current cost is undercounted. `QueryGraph` also keeps `id_to_sha: Vec<String>`, a second copy, so the total is about 158–196 B/commit. The 24 B figure needs either an indirect binary search or a second permutation (28 B). 360 MB counts only a CSR, while the report's own columns come to at least 520 MB, or about 1 GB with subjects. 10–18 MB assumes f=0.1, but git.git measures 0.41, giving 41–74 MB (Q7) | Minor (the direction of the argument survives) | Correct the numbers in the ADR. Measure with a heap profiler before citing them (AGENTS rule 8) |
| 11 | "Force-pushed-away commits keep their per-OID data and drop out of the visible set" (blueprint *Derived data*); "dense u32 ID" | IDs are never reclaimed, because parents and change layers use global IDs. Every query (children, both, topo stream, OID lookup of gc'd commits) must AND with a visible bitset that the query section never mentions. Density decays with rebase churn (Q5) | Minor | Add an explicit `visible` bitset per snapshot to the query kernel contract. Renumber only at blue/green rebuild. Track the dead-ID ratio as a compaction trigger |
| 12 | "Otherwise a generation-pruned walk runs. That walk is git's proven fallback: … 2.64 s to 0.02 s" | That number comes from git with commit-graph parsing plus corrected-commit-date (v2) pruning plus first-parent DFS. The cascade specifies topological level, which git itself calls weaker after old-branch merges (Q3) | Minor | Specify corrected commit date for walk ordering and level plus ID for O(1) cuts. Attribute the timing correctly |
| 13 | Sapling description precision (note Q5/Q6/Q8) | `is_ancestor` in Sapling computes the full `ancestors` set with no early exit. "Master group linear scan" is in `descendants_intersection`, not general. The note's "groups, re-assignable at compaction" contradicts immutable global-ID layers, and the report silently drops groups (Q6) | Minor | Cite exactly. Decide on groups explicitly in ADR-0023 |
| 14 | "Software Heritage made the same move when it renumbered nodes to restore locality" (blueprint *Identity*) | SWH renumbers by BFS and then Layered Label Propagation on a symmetrized graph, for compression. That order is not parent-first, so the analogy does not support topological IDs (Q8) | Minor | Drop it, or reword it as "renumbering for locality has precedent" |
| 15 | "u32 covers 4.29B commits" (note Q8 item 1) | If the octopus-overflow bit trick is copied (as jj and git do), the effective cap is 2^31 (jj) or about 1.8B (git). SWH already holds 2.7B commits. This is fine for a desktop single repo (Q8) | Minor | State "≤ 2^31 IDs per index" and reserve the high bit |
| 16 | "Every API takes a budget, returns partial results deterministically" versus "Keep the API" (component ledger) | `ShortestPathResponse` has only `found: bool`, so a budget makes "not found" ambiguous. The current shortest path has no budget, and its predecessor arrays are `Vec<Option<usize>>` at 16 B per slot, sized to the max visited ID (`indexer.rs:2581-2585`) | Minor | Add an `exhausted`/`truncated` flag. This is an API change and must be stated as one |
| 17 | "Same contracts" for DFS and topological traversal (ledger) | Today's contract is inconsistent. Base CSR keeps git parent order (`db.rs:1527`), but `attach_overlay` sorts overlay parents by descending ID (`indexer.rs:1980`), which loses first-parent identity for overlay commits. Topological truncation returns a BFS ball, not an ID prefix (Q9) | Minor | Pin parent order to git's as part of the oracle. Document the new truncation semantics as a contract change |
| 18 | Notes disagree on the ingest order: "`git rev-list --topo-order --reverse`, or our own DFS" (note Q8) versus "DFS … first parent last" (report) | Different orders give different IDs, and so different ID-dependent outputs per ingest path. That breaks attack 3's invariance on the commit-graph versus object-walk axis too | Minor | Make the in-engine DFS the only ID assigner. Treat git output only as an edge source |

### Gaps
- Only git.git (merge-heavy, 25% merges) was simulated. linux.git (about 1.4M commits) was not cloned, and Chromium's merge ratio was not found.
- No timings: walk costs are node counts from Python, not Rust ns.

---

## Q2. Is restricting shortest path to [id(B), id(A)] valid for all directions? Is "shortest path" well defined and useful?

### Takeaway
It is valid for `parents` and for `children` with the bounds mirrored, because every directed hop strictly changes the ID. It is invalid for `direction=both`, the undirected shortest path that the current API exposes and the perf harness measures. On git.git the restricted search gave a wrong answer for 93% of random pairs.

### Cited Findings
- The current API takes `direction` (`Parents | Children | Both`) for shortest path. `Both` expands parents and children of each node (`for_each_neighbor`, `indexer.rs:2440-2462`). The backward side uses `reverse_direction(Both) = Both` (`indexer.rs:2537-2543`). Shortest path has no node budget (`indexer.rs:2066-2132`) — [indexer.rs](/Users/singh.deependra-int/Desktop/git/FinalGitGraph/apps/desktop/src-tauri/src/application/indexer.rs)
- The in-memory benchmark measures `shortest_path(start_sha, pre_merge_main_sha, GraphDirection::Both)` (`indexer.rs:1274-1278`), and `bin/perf-report.rs:306` passes `GraphDirection::Both` — [indexer.rs](/Users/singh.deependra-int/Desktop/git/FinalGitGraph/apps/desktop/src-tauri/src/application/indexer.rs); [perf-report.rs](/Users/singh.deependra-int/Desktop/git/FinalGitGraph/apps/desktop/src-tauri/src/bin/perf-report.rs)
- Blueprint: "Shortest path runs as a bidirectional BFS restricted to the ID interval `[id(B), id(A)]` and to generation bounds" — [blueprint](/Users/singh.deependra-int/Desktop/git/FinalGitGraph/reports/FinalGitGraph%20backend%20architecture%20blueprint.md). The note scopes it to "(unweighted, B ancestor of A)" with "gen(B) <= gen(v) <= gen(A)" — [note Q6](/Users/singh.deependra-int/Desktop/git/FinalGitGraph/research_notes/FinalGitGraph%20backend%20architecture%20blueprint/graph_representation_and_queries.md)
- git.git measurement over 120 uniform random pairs (A, B). An unrestricted undirected BFS found a path for every pair. The same BFS restricted to IDs in [min(id A, id B), max(id A, id B)] found **no path in 11/120** and **a strictly longer path in 101/120** — [local git.git measurement](https://github.com/git/git), `dagsim.py`

### Inferences
- **Hand counterexample (direction=both).** Root R has two children X and Y, which are two branch tips. With parent-first DFS (tips X then Y), R=0, X=1, Y=2. `shortest_path(X, Y, both)`: the true answer is X–R–Y (length 2). The interval [1, 2] excludes R. The generation bound fails too: gen(R)=1 < gen(X)=gen(Y)=2. So the restricted search reports `found=false` for two connected commits. Adding a merge M = merge(X, Y) with id 3 does not help: both length-2 paths, X–R–Y and X–M–Y, leave the interval.
- **Directed proof sketch.** Along `parents`, each hop strictly decreases the ID (parent < child) and the topological level. So every node on a path from A down to B lies in [id(B), id(A)] and in [gen(B), gen(A)]. The restriction is sound for `parents`. It is sound for `children` with roles swapped. The blueprint's literal `[id(B), id(A)]` assumes `from` is the descendant. For `direction=children`, `from` is the ancestor, so an implementation that copies the notation verbatim would search an empty interval. Write it as [min, max].
- **Is "shortest path" useful in a DAG?** It is well defined: the fewest edges in a directed (or undirected) path. In a merge-heavy history, though, the shortest parents-path from HEAD to an old commit usually cuts through some merge's second parent, an arbitrary-looking route. Users usually ask "which merge brought X into main?" or "what is X relative to HEAD" (name-rev or describe style). Both are first-parent or containment questions. The undirected `both` distance is mostly "how far apart are two branches through their merge base". This is opinion, but it argues for keeping `both` correct and cheap rather than removing it, since the perf harness depends on it.
- The current implementation is correct as a bidirectional BFS. It detects the meeting on discovery, level by level, and that yields an optimal path (my proof: no shorter path can exist without an earlier meeting). The redesign must not lose that by changing to stop on the first meeting under pruning.

### Gaps
- I did not check the frontend, because a grep with zsh globs failed. I do not know whether the UI calls `direction=both` shortest path directly or only through perf.

---

## Q3. Does the is_ancestor cascade hold (ID order, generation level, same first-parent run, generation-pruned walk)?

### Takeaway
All three O(1) rules are logically sound. git.git gave zero wrong answers over 9,000 sampled queries across six pair distributions. The weakness is coverage, not correctness. Positive queries almost always fall to the walk under the blueprint's numbering. Git's speedup figure is attributed to a different generation definition than the cascade specifies.

### Cited Findings
- Generation definitions: topological level (root = 1, child = 1 + max(parents)) and corrected commit date. "If gen(X) < gen(Y), then X cannot reach Y." Git moved to corrected dates because a merge that brings in an old-based commit weakens the topological-level cutoff — [GitHub blog, Git's database internals II](https://github.blog/open-source/git/gits-database-internals-ii-commit-history-queries/) (as quoted in [note Q1](/Users/singh.deependra-int/Desktop/git/FinalGitGraph/research_notes/FinalGitGraph%20backend%20architecture%20blueprint/graph_representation_and_queries.md))
- Git's `--is-ancestor v5.0 v5.19` speedup (2.64 s → 0.02 s) "stops early through the generation cutoff and depth-first first-parent traversal" — [GitHub blog II](https://github.blog/open-source/git/gits-database-internals-ii-commit-history-queries/)
- git.git cascade (blueprint order: ID test, then level test, then same first-parent run, where a run is a maximal ID range in which each x has first parent x−1; I added a DFS-interval test as a probe). The fallback walk is a DFS pruned by `id(v) ≥ id(A)` and `level(v) ≥ level(A)`. 1,500 pairs per row — [local git.git measurement](https://github.com/git/git), `dagsim.py`/`dagsim2.py`:
  - Uniform random pairs, blueprint (first-parent-LAST) numbering: id 770, gen 49, run 0, interval 12, **walk 669**. Walk median 289 nodes, max 29,341.
  - Positive pairs (B random, A from a random parent walk of 1–3,000 steps): **walk 1,383/1,500 (92%)**, run 50, interval 67. Walk median 692, p99 1,618.
  - A random, B on master's first-parent chain, blueprint numbering: id 859, gen 1, interval 10, **walk 630 (42%)**, walk max 24,692.
  - A random, B one of the other DAG heads: gen 430, id 12, **walk 1,050**, walk max 73,516 (of 85,936).
  - Cascade errors against ground truth: **0** in every row.
- Over 400 sampled git.git merges, the first parent was an ancestor of the second parent (a fast-forwardable `--no-ff` merge) in only 2 — [local git.git measurement](https://github.com/git/git)

### Inferences
- **Rule "id(A) > id(B) ⇒ B cannot reach A": holds.** If A is an ancestor of B, every hop down from B decreases the ID, so id(A) < id(B). It does not break under incremental appends either. A new commit's parents are either old (lower IDs) or new and assigned first by the DFS.
- **Rule "gen_level(A) ≥ gen_level(B), A ≠ B ⇒ false": holds.** The level is strictly increasing along child edges. It earns its place: it pruned 430/1,500 queries for late-numbered heads, where the ID test is weak. The ID and the level are different linear and graded extensions, so neither subsumes the other.
- **Rule "same contiguous first-parent run ⇒ true": correct in direction and with merges.** Inside a run, following first parents from B visits B−1, B−2, … down to A. A merge inside the run (first parent = x−1) does not break this. It needs id(A) ≤ id(B), which the earlier ID test guarantees. But it fires rarely (≤ 50/1,500), because runs end at every fork point.
- **Coverage attack.** The note's "Expected: O(1) for most queries" quotes O'Reach's generic-graph rates (about 95% of negatives, >70% of positives). On a commit DAG the common positive query is precisely what this cascade cannot answer in O(1) (92% walks). The walk's worst case is about the whole repo: 73,516 of 85,936 nodes. That is the same worst case git has without reachability bitmaps.
- **Generation mismatch.** The blueprint uses `gen_level` (topological level) for the O(1) cut and an unspecified "generation-pruned walk". The 2.64 s → 0.02 s figure it cites comes from git's commit-graph, where walks are ordered and pruned by generation v2 (corrected commit date) when present, plus first-parent DFS. The ADR should state which generation drives the walk. Level is fine for the O(1) cut, but it is the weaker walk-ordering key (per the GitHub blog).
- Rule 4 of the note's cascade ("Same flat segment") needs a run-boundary index. The kill table's fallback "keep CSR-in-layers with generation pruning only" would delete it. A run-start bitvector costs about 1 bit per commit and does not need segments, so keep it regardless.

### Gaps
- Git's exact walk ordering (`repo_is_descendant_of`, `paint_down_to_common`) was not fetched this session.
- No Rust timings, so "sub-ms" remains unverified.

---

## Q4. Does "first parent last" DFS give contiguous first-parent chains in real histories? Does locality survive multiple tips? Is this really Sapling's scheme?

### Takeaway
On git.git, first-parent-last does give the claimed locality: 82.5% of first-parent hops are i→i−1, and master's first-parent chain breaks only 986 times in 24,442 hops. But it costs something more valuable. The second parent's subtree is numbered first and "steals" the shared old history. So no main-line commit except the tip has the "ancestors = [0, id]" property, while first-parent-FIRST gives it to every main-line commit. Sapling's own source treats the order as a per-situation choice. It uses FirstFirst for incremental builds and reserves ID ranges for mainline growth. The blueprint copies neither.

### Cited Findings
- Sapling `assign_head` comment: "If `D` is empty or already assigned, it's better to assign A last … (A+C) can then always form a segment". "If `D` is not empty (and not assigned), it _might_ be better to assign D and A first. This provides benefits for A and D to be continuous, with the downside that A and C are not continuous." — [sapling idmap.rs](https://github.com/facebook/sapling/blob/main/eden/scm/lib/dag/src/idmap.rs) (raw source, lines ~71–97)
- Same file: "However, the above visit order (first parent last) is not optimal for incremental build case with pushrebase branches. Because pushrebase uses the first parent as the mainline." "We choose first parent last order if `covered` is empty, or when visiting ancestors of non-first parents." Code: `// Assume re-building from scratch.` → `VisitOrder::FirstLast`, else `// Assume incremental updates with pushrebase.` → `VisitOrder::FirstFirst` — [sapling idmap.rs](https://github.com/facebook/sapling/blob/main/eden/scm/lib/dag/src/idmap.rs)
- Same file, `assign_head` docs: "`reserved_ids` specifies what ranges are reserved for future growth of other important heads (usually a couple of mainline branches that are long-lived, growing, and used by many people). This is useful to reduce fragmentation." Also: "This function needs roughly `O(N)` heap memory." — [sapling idmap.rs](https://github.com/facebook/sapling/blob/main/eden/scm/lib/dag/src/idmap.rs)
- git.git, full ingest, tips = master and then the other heads — [local git.git measurement](https://github.com/git/git), `dagsim.py`/`dagsim2.py`:
  - First-parent-LAST (the blueprint's order): 0 parent ≥ child violations; first parent == id−1 for 82.5% of non-root commits; Sapling flat segments/N = 0.410; first-parent runs/N = 0.175; master first-parent chain breaks 986/24,442. **Master chain commits with ancestors == [0, id]: 16/24,442.**
  - First-parent-FIRST: 0 violations; first parent == id−1 for 60.3%; flat segments/N = 0.439; first-parent runs/N = 0.397; master chain breaks 15,753/24,442 (one per merge). **Master chain commits with ancestors == [0, id]: 24,442/24,442.**
  - Cascade under FIRST, A random and B on master's chain: id 849 + interval 651, **0 walks**. Under LAST: 630 walks (42%).
  - Cascade under FIRST, uniform pairs: interval 190, walk 491. Under LAST: interval 12, walk 669.
- git.git shape: 21,614 merges of 85,936 commits (25.2%), 37 octopus merges, 9 roots. Master's first-parent chain has 24,443 commits, of which 15,753 are merges (64%) — [local count on git.git](https://github.com/git/git) (`git rev-list --all --merges --count`, etc.)

### Inferences
- **Hand example of the "steal".** Main is c0…c9 by first parent. Side branch s1, s2 forks at c3 and merges at c7 = merge(c6, s2). Under first-parent-last, s2 is visited first and dives through c3 down to c0: c0=0 … c3=3, s1=4, s2=5. Then c4=6, c5=7, c6=8, c7=9, c8=10, c9=11. Query is_ancestor(c1, c6): the ID test passes, the levels differ, the runs differ ([0..3] vs [6..8]), and low(c6)=6, so it falls to the walk. Under first-parent-first: c0…c6 = 0…6, s1=7, s2=8, c7=9, … Every main commit has ancestors = [0, id], so `id(A) ≤ id(B)` alone answers it exactly.
- **This is the decisive trade-off the blueprint did not weigh.** The ID-adjacency it optimizes (82.5% vs 60.3%) matters for prefetching parent hops. The prefix property it gives up turns the most common product question ("is X in main?", "tag --contains" against main, "is the deployed commit included?") into an exact O(1) answer. That is Sapling's ONLY_HEAD fast path, applied to every main-line segment.
- **The two can be had together.** Keep first-parent-last IDs for locality, and add a 4-byte column `main_rank[c]`: the index k of the first main-line commit whose ancestry contains c. Then is_ancestor(A, main_k) = `main_rank[A] ≤ k`, exact and O(1). It can be appended: the commits newly assigned during ingestion of main_k all get k. It must be recomputed if main is rewritten. This is a design inference with no precedent cited.
- **Locality under multiple tips and refreshes** degrades but is not fatal on git.git. After 200 refreshes, first parent == id−1 drops from 82.5% to 80.5%, runs/N rises from 0.175 to 0.195, and master chain breaks rise from 986 to 1,043. On a repo with many active refs (PR refs, remote branches), every refresh interleaves other branches' commits between main-line appends. Sapling counters this with `reserved_ids`. The blueprint's dense IDs have no counter.
- **Contiguity is per fork-free stretch, not per chain.** Main-line contiguity breaks at every fork point under first-parent-last, and at every merge under first-parent-first. "First-parent chains get contiguous IDs" should read "first-parent stretches between fork points get contiguous IDs".
- **Unresolved detail in Sapling's code.** Read literally, the stack code under `FirstLast` reverses the parent list, pushes it, and pops the first parent first. That seems to contradict the enum's "Visit the first parent last". `Assign` then re-reads `parent_ids` without reversing it under `FirstLast` and with reversal under `FirstFirst`, which is consistent with that literal reading. I could not resolve this without running Sapling. The comment-level intent quoted above is unambiguous, and that is the evidence I rely on.

### Gaps
- No simulation on a rebase-centric repo (for example a Gerrit-style linear history). There, both orders converge, because there are almost no merges.
- I did not check whether gitdag (the Sapling-on-git crate) overrides the visit order.

---

## Q5. Incremental appends, missing parents, ref-scope changes, force-push/gc: does parent-first numbering stay valid without relabeling?

### Takeaway
It stays valid for ordinary appends and for ref-scope expansion: zero violations across a 200-refresh simulation. It breaks for shallow deepen/unshallow and for graft or replace removal. In those cases a parent appears after its child already holds an ID. IDs, and every ID-ordered output, depend on ingestion history, which contradicts the verification contract. Force-push and gc leave dead IDs that every query must filter, and the IDs are never reclaimed.

### Cited Findings
- Blueprint: "A parent that is truly absent, such as a shallow boundary or a missing object, becomes a typed boundary record, never a `continue`", and "Each ID is issued only after all its parents have IDs" — [blueprint *Identity*](/Users/singh.deependra-int/Desktop/git/FinalGitGraph/reports/FinalGitGraph%20backend%20architecture%20blueprint.md)
- Note: "A commit whose parent is still unknown is a hard error, or is parked in a 'pending' set (shallow/graft boundary)". Also: "an edge is attached when the parent arrives (the ID still increases, because the child gets its ID only after the parent)" — [note Q5, Q8](/Users/singh.deependra-int/Desktop/git/FinalGitGraph/research_notes/FinalGitGraph%20backend%20architecture%20blueprint/graph_representation_and_queries.md)
- Blueprint's metamorphic relations: "Full ingest must equal prefix-then-rest ingest". These are compared through a content digest over OID-sorted records that "does not depend on IDs". Perf scenarios compare "query-result hashes … with the oracle-derived expected values" — [blueprint, oracle and perf sections](/Users/singh.deependra-int/Desktop/git/FinalGitGraph/reports/FinalGitGraph%20backend%20architecture%20blueprint.md)
- gitdag (Sapling on git): its dag is "append-only. It might include vertexes no longer referred by the git repo" — [gitdag docs](https://docs.rs/gitdag/latest/gitdag/struct.GitDag.html) (as quoted in the frontier note)
- Sapling: "Existing `id`s that are ancestors of `head` will get re-assigned if they have a higher `group`" — [sapling idmap.rs](https://github.com/facebook/sapling/blob/main/eden/scm/lib/dag/src/idmap.rs)
- git.git, 200 committer-time refreshes (each closed under parents) with first-parent-last DFS appends: 0 parent ≥ child violations. Topo stream from master, descending ID, limit 10: same sequence. Limit 100: different sequence, set overlap 92/100. Limit 1,000: overlap 991/1,000 — [local git.git measurement](https://github.com/git/git)

### Inferences
- **Ordinary appends and ref-scope expansion: claim holds.** In a full (non-shallow) repo, every indexed commit already has its whole ancestry indexed, because the DFS closes under parents. Commits that newly enter scope (a branch added to the ref filter) get fresh high IDs, and so do their unindexed ancestors, even if old in time. Parent < child is preserved. The only casualty is the "ID ≈ age" intuition.
- **Shallow deepen breaks the invariant. Hand counterexample.** A `--depth=1` clone has commit C, whose parent P is missing, so C=0 with a typed boundary. `git fetch --deepen=1` adds P, whose parent Q is still missing. P needs an ID < 0. No relabel-free assignment exists. The same holds for `git replace` or graft removal, which exposes "new" parents of indexed commits. The note's two versions disagree. "Parked in a pending set" means shallow commits are not queryable. The report's "typed boundary record" means they are queryable, but the invariant breaks on deepen. The fixture matrix lists shallow clones and `git replace` but not deepen. Treat deepen, unshallow and replace or graft changes as a full blue/green renumber, or reserve an ID gap below boundary commits (Sapling-style reserved ranges).
- **ID-history dependence. Hand counterexample.** At state S1, main is R←a←b and feature is R←x. Ingest with tips [b, x]: R=0, a=1, b=2, x=3. At S2, m = merge(b, x) and c←m, so incremental gives m=4, c=5. A full ingest of S2 from tip c visits x first (first parent last): R=0, x=1, a=2, b=3, m=4, c=5. "Topo stream from c, limit 3, descending ID" returns [c, m, x] after the incremental ingest and [c, m, b] after the full ingest. Both are valid topological prefixes, but they differ. So any ID-defined output (topo stream with limit, truncation sets, BFS and DFS tie-breaks by neighbor ID order, `max_nodes` cut-offs) differs between "full" and "prefix-then-rest" ingest. The git oracle cannot produce a single expected value. The ID-independent digest hides this, because it never looks at query outputs.
- **Force-push and gc.** Dead IDs stay in parent arrays and children lists. So `direction=children/both`, descendants, topo streams and OID lookup must all AND with a per-snapshot `visible` bitset (ancestors of the current tips, computed by an O(N) scan per manifest generation, about 1.25 MB at 10M). The query section of the blueprint never mentions it. A gc'd commit's OID would otherwise still resolve as "indexed". Global-ID parent arrays and ID-range-keyed change layers ("seal them into change layers that cover ID ranges") make in-place renumbering at compaction impossible. That is why Sapling's group reassignment, which the note borrowed as "re-assignable at compaction", silently disappeared from the report. Density degrades with rebase churn, without bound, until a full rebuild.

### Gaps
- I did not measure the dead-ID growth rate for a realistic desktop workflow (daily rebases, many remote branches).
- I did not check how gix reports shallow boundaries or whether replace refs are honored by default in gix 0.80 or 0.9x.

---

## Q6. Sapling IdDag claims, and whether the segments/N ≤ 0.3 kill criterion is justified. Real merge ratios.

### Takeaway
The note's IdDag facts check out against source. Default segment size 16 and max meaningful level 4 are confirmed. ONLY_HEAD exists and works as described. `range` and `descendants` are "O(flat segments), or O(merges)". But Sapling's `is_ancestor` computes the full ancestor set. The 0.3 threshold comes from a storage-only back-of-envelope estimate, and it ignores the 16x-per-level hierarchy. git.git (0.41) would fail it, while Linux (7–9% merges) would pass. The criterion encodes workflow, not latency.

### Cited Findings
- Segment definition: "`high` is the *only* head in the sub DAG", "`parents` do not have entries within `low..=high`", and for level 0 "for any integer `x` in `low+1..=high` range, `x`'s parents must be `x - 1`" — [sapling-dag iddag.rs](https://docs.rs/sapling-dag/latest/src/dag/iddag.rs.html) (sapling-dag 0.2.0, MIT header per the fetched page)
- `static DEFAULT_SEG_SIZE: AtomicUsize = AtomicUsize::new(16);` ("See benches/segment_sizes.rs (D16660078)"). `const MAX_MEANINGFUL_LEVEL: Level = 4;` ("good enough for an existing large repo (level 5 is not built because it cannot merge level 4 segments)") — [iddag.rs](https://docs.rs/sapling-dag/latest/src/dag/iddag.rs.html)
- ONLY_HEAD: "ONLY_HEAD means it heads(0..=head) = [head], or ancestors([head]) = 0..=head". The `ancestors` fast path pushes `Id::MIN..=id` and breaks — [iddag.rs](https://docs.rs/sapling-dag/latest/src/dag/iddag.rs.html)
- `range`, `descendants` and `descendants_intersection`: "This is O(flat segments), or O(merges)." No O() statement for `ancestors`, `is_ancestor`, `gca_one`, `gca_all` or `common_ancestors` — [iddag.rs](https://docs.rs/sapling-dag/latest/src/dag/iddag.rs.html)
- `is_ancestor` is `let set = self.ancestors(descendant_id.into())?; Ok(set.contains(ancestor_id))`, with no early exit — [iddag.rs](https://docs.rs/sapling-dag/latest/src/dag/iddag.rs.html)
- The master versus non-master scan strategy ("For the master group, use linear scan for flat segments … usually only has 1 [head]"; "For the non-master group, only check flat segments covered by `ancestors`") is in `descendants_intersection` — [iddag.rs](https://docs.rs/sapling-dag/latest/src/dag/iddag.rs.html)
- Meta: graph relationship queries in "O(number-of-merges) time"; the repo has "tens of millions of commits" — [Meta Engineering, Sapling (2022-11-15)](https://engineering.fb.com/2022/11/15/open-source/sapling-source-control-scalable/)
- Origin of the 0.3 threshold: frontier note D2 models flat segments as ≈ 2mN. "m = 30% gives about 6M segments, and compression mostly disappears … Kill criterion: if segments/N > 0.3, keep CSR plus generation pruning only" — [frontier_novel_architectures.md:150-155](/Users/singh.deependra-int/Desktop/git/FinalGitGraph/research_notes/FinalGitGraph%20backend%20architecture%20blueprint/frontier_novel_architectures.md)
- Merge ratios:
  - git.git: 25.2% (21,614/85,936) — [local count](https://github.com/git/git)
  - Linux, whole history (2017 snapshot): 46,930 merges of 649,306 commits (7.2%) — [Destroy All Software, 2017](https://www.destroyallsoftware.com/blog/2017/the-biggest-and-weirdest-commits-in-linux-kernel-git-history) (via search summary; page not fetched)
  - Linux 5.1 cycle: "1,126 merge commits – nearly 9% of the total" — [kernel docs, Rebasing and merging](https://docs.kernel.org/maintainer/rebasing-and-merging.html) (via search summary; page not fetched)
- git.git measured Sapling-definition flat segments/N: 0.410 (first-parent-last) and 0.439 (first-parent-first) — [local git.git measurement](https://github.com/git/git)

### Inferences
- The model "flat segments ≈ 2m·N" is validated. git.git at m = 0.252 predicts ≤ 0.50 and measures 0.41. For Linux at m ≈ 0.07–0.09, the prediction is about 0.12–0.18 (inference, not measured). So the criterion keeps segments for Linux and kills them for git.git and any merge-commit-workflow repo with m ≳ 0.15.
- **Why the threshold is arbitrary.** It asks "do segments compress storage versus CSR?" The blueprint uses segments for query latency ("a later accelerator"). Even at 0.41 flat segments per commit, level-1 segments (fanout 16) cut the walk to about 2.6% of N, and level 2 to about 0.16%. Merge-heavy repos are exactly where O(N) commit walks hurt and the hierarchy pays. The criterion is inverted relative to its purpose.
- **Precision issues in the notes.** (a) The note's cascade step "segment walk … with early exit" is new design, not Sapling's behavior, because Sapling's `is_ancestor` builds the full ancestor set. (b) "O(merges)" is documented for range and descendants, not for is_ancestor. (c) The master-group linear-scan remark is specific to `descendants_intersection`. (d) The note claims a "flat segments + hierarchical segments" layer column in v1 storage (note Q8 item 3), while the report defers all segments. One of the two must change.
- **Missed build-versus-buy option.** sapling-dag is published (MIT header on the fetched source), and gitdag already maintains a segmented changelog over git commit graphs. Adopting the crate behind the query trait, gated on a latency criterion, is cheaper than writing segments later. A license and maintenance check is still needed.

### Gaps
- Chromium's merge ratio: no reliable source found.
- linux.git segments/N was not measured; the clone was too large for this session.
- The benchmark behind Sapling's DEFAULT_SEG_SIZE (D16660078) is internal.

---

## Q7. Memory arithmetic checks

### Takeaway
The direction of the argument holds: binary sorted OIDs are far smaller than hex `String` maps. Every specific number needs correction. Today's cost is about 2x higher than stated, because `QueryGraph` stores two String copies. "24 B" needs a design the notes do not describe. The 360 MB layer estimate omits most of the report's own columns. "10–18 MB" assumes a merge ratio git.git exceeds 4x.

### Cited Findings
- `QueryGraph` holds both `sha_to_id: HashMap<String, NodeId>` and `id_to_sha: Vec<String>` (`indexer.rs:1872-1874`). The map is filled with `sha_to_id.insert(sha.clone(), index)` (`indexer.rs:1909-1912`), so each SHA is heap-allocated twice — [indexer.rs](/Users/singh.deependra-int/Desktop/git/FinalGitGraph/apps/desktop/src-tauri/src/application/indexer.rs)
- Shortest-path predecessor arrays: `ensure_slot` resizes `Vec<Option<T>>` to the max visited ID, plus one (`indexer.rs:2581-2585`). It is used per side (`indexer.rs:2425-2426`) — [indexer.rs](/Users/singh.deependra-int/Desktop/git/FinalGitGraph/apps/desktop/src-tauri/src/application/indexer.rs)
- `DenseSet` is a `Vec<u64>` bitset grown to the max ID (`indexer.rs:1805-1826`) — [indexer.rs](/Users/singh.deependra-int/Desktop/git/FinalGitGraph/apps/desktop/src-tauri/src/application/indexer.rs)
- Origin of 360 MB: "At 10M commits, the CSR is about 20 B sha + about 8 B parent offset/neighbor + about 8 B child offset/neighbor per commit, roughly 360 MB" — [storage_engine.md:217](/Users/singh.deependra-int/Desktop/git/FinalGitGraph/research_notes/FinalGitGraph%20backend%20architecture%20blueprint/storage_engine.md)
- The report's layer columns: "OID fanout + sorted OIDs + perm | parents (global ids) | gen level | corrected date | commit time | author ids | subject heap" — [blueprint diagram](/Users/singh.deependra-int/Desktop/git/FinalGitGraph/reports/FinalGitGraph%20backend%20architecture%20blueprint.md)
- git.git mean subject length: 45.9 bytes (`git log --all --format=%s`) — [local measurement](https://github.com/git/git)
- jj stores parent positions as u32 with values `>= 0x8000_0000` reserved for overflow ("u32: (< 0x8000_0000) global index position for parent 1") — [jj-lib readonly.rs](https://docs.rs/jj-lib/latest/src/jj_lib/default_index/readonly.rs.html)

### Inferences
- **HashMap<String, usize> alone.** This arithmetic rests on hashbrown internals I did not re-fetch: power-of-two buckets, max load 7/8, 1 control byte per bucket. Per bucket: 24 B String + 8 B usize + 1 B control = 33 B. Buckets per entry range from 8/7 up to 16/7, depending on where N falls between powers of two. That gives 37.7–75.4 B per entry, plus a 48 B allocation for the 40-byte hex string (48-byte size class assumed): **≈ 86–123 B/commit**. At N = 10M the capacity is 11.43M, rounded to 2^24 buckets, so 16.78M × 33 B + 10M × 48 B ≈ 1.03 GB. The note's 85–100 B is the lower half of that range.
- **Plus `id_to_sha`.** 24 B header + 48 B allocation = 72 B. So the **current total is ≈ 158–196 B/commit (about 1.6–2.0 GB at 10M)**, not 85–100. The blueprint's "about 4x less memory for lookup" understates the win. It is closer to 6–8x versus 24–28 B.
- **"24 B/commit for sorted OIDs plus permutation".** Sorted OIDs (20 B) plus pos→id (4 B) answer OID→ID only. Returning SHAs for results (ID→OID) needs a second 4 B inverse permutation, for **28 B**. The 24 B figure is reachable another way: store OIDs in ID order (20 B) plus a sorted-position→ID index (4 B), and binary-search indirectly through `oid[perm[mid]]`. That costs a random access per probe, about 15 probes after the fanout at 10M. The note's sentence ("24 B/commit if the sorted array alone is kept and id→oid is resolved via a 4-byte inverse permutation") is arithmetically inconsistent. For SHA-256 the figures become 36–40 B/commit.
- **1.25 MB bitset per 10M commits: correct** (10^7 bits / 8 = 1.25 × 10^6 B). But A..B needs 2 bitsets and merge-base about 3 colors. Today's shortest path allocates `Vec<Option<usize>>` at 16 B/slot (no niche in usize), about 160 MB per side at 10M. The redesign fixes that, but the blueprint never names it.
- **"360 MB topology layer set".** With the report's own columns at 10M commits:
  - sorted OIDs 200 MB, permutation 40 MB, inverse permutation 40 MB;
  - parents as two u32 slots (git CDAT style) 80 MB;
  - level 40 MB, corrected-date offset 40 MB, commit time 40 MB, author ID 40 MB.
  - Total ≈ **520 MB without the subject heap**. At git.git's 45.9 B average subject, add ≈ 460 MB, for **≈ 1 GB**. A "single sequential read" of 1 GB at open is a different cold-start story than 360 MB.
- **"10–18 MB" segment encoding** (note Q3) assumes f = 0.1. At git.git's measured 0.41 and the same 10–18 B per segment, it is 41–74 MB at 10M, close to the 84 MB plain parent CSR. It still does not replace children.
- **u32 range.** With the overflow-bit convention (jj style), the usable range is 2^31 ≈ 2.1B. Git's commit-graph limit is about 1.8B. Both are ample for a desktop repo. "4.29B" holds only without a reserved bit.

### Gaps
- No heap-profiler measurement of `QueryGraph` was taken. hashbrown layout constants and the macOS, glibc and Windows allocator size classes were not re-verified this session.

---

## Q8. Missed alternatives, u32 at Software-Heritage scale, SHA-256

### Takeaway
The cheapest high-value additions are two 4-byte, append-maintainable labels the blueprint never considered: a DFS-interval "low" for exact positives, and a main-line prefix or containment rank. A max-heap for topo streams is also missing. FELINE-style second orders and Roaring checkpoints are plausible but unmeasured. u32 is fine for desktop scale. The Software Heritage analogy does not support topological IDs.

### Cited Findings
- O'Reach combines forward and backward topological levels, t = 4 extended topological orderings and k = 16 supportive vertices, with an index of 64n bytes — [O'Reach (ar5iv 2008.10932)](https://ar5iv.labs.arxiv.org/html/2008.10932) (as quoted in note Q2)
- DFS-interval probe on git.git, low(v) = the first ID assigned inside v's DFS subtree; exact positive when low(B) ≤ id(A) ≤ id(B). Answered: blueprint order, uniform 12/1,500 and main-line B 10/1,500. First-parent-first order, uniform 190/1,500 and main-line B 651/1,500 with 0 walks. Errors: 0 — [local git.git measurement](https://github.com/git/git)
- jj reorders index-order (creation-order) output for display. `TopoGroupedGraph` is a depth-first adapter that keeps branches grouped. A maintainer says the order of non-linear commits is "essentially creation order, with tweaks to group nodes topologically" — [jj-lib graph module](https://docs.rs/jj-lib/latest/jj_lib/graph/index.html); [jj issue #5273](https://github.com/jj-vcs/jj/issues/5273) (via search summary)
- swh-graph orders nodes by BFS and then LLP. "The LLP algorithm finds locality-preserving orders by clustering together nodes in close proximity". It runs on "a simplified (loopless, symmetric) graph". The BFS pass is an intermediate compression step — [swh-graph compression.rst](https://raw.githubusercontent.com/SoftwareHeritage/swh-graph/master/docs/compression.rst)
- The Software Heritage dataset (export 2022-12-07) has "2.7 billion commits". The in-memory graph is about 25B nodes and 350B edges — [arXiv 2310.10295](https://arxiv.org/html/2310.10295v1)
- Git's commit-graph max is "(1 << 30) + (1 << 29) + (1 << 28) - 1 (around 1.8 billion)" — [gitformat-commit-graph](https://git-scm.com/docs/gitformat-commit-graph) (as quoted in note Q1)

### Inferences
- **DFS-interval labels (tree-cover intervals; the GRAIL and Agrawal-style lineage is background, not re-verified).** With parent-first DFS, v's DFS subtree is exactly the ID range [low(v), id(v)], and all of it is ancestors of v. One u32 per commit, computed for free during assignment and valid under appends, gives an exact positive test. It strictly generalizes the "same run" rule. Its power depends on the visit order (Q4).
- **Main-line prefix or containment rank** (Q4): exact O(1) for B on main under any numbering. It is the commit-DAG analogue of Sapling's ONLY_HEAD and costs 4 B/commit.
- **Second topological order (FELINE-style 2D dominance).** Store a second linear extension, for example the first-parent-first order next to first-parent-last IDs, as a u32 negative filter. O'Reach shows multiple orderings help on generic graphs. Unmeasured on commit DAGs.
- **Heap-based descending-ID stream** (Q9): replaces the O(ID interval) scan.
- **Commit-only Roaring ancestor bitmaps at sparse checkpoints** (the note's own §6 suggestion) give exact answers for checkpointed tips. Not costed here. Git's EWAH experience is object-oriented and does not transfer directly.
- **2-hop rejection: not attacked.** The PLL and TF build-time and size figures support it.
- **Software Heritage analogy.** SWH renumbers for compression locality on an undirected, symmetrized graph. The resulting order is not parent-first, so it gives no support for "parent < child" IDs. Its lesson ("clustered orders compress best") cuts the other way.
- **u32.** Fine for any single local repo. Meta's monorepo is "tens of millions" of commits. SWH-scale (2.7B commits) would exceed the 2^31 overflow-bit range in a combined-forks index, but that is out of scope for a desktop app.
- **SHA-256.** A per-repo 20/32-byte OID width with fanout plus sorted array works unchanged. Memory figures scale (Q7). Readiness of the ingest side (gix) for SHA-256 and compat object-format repos is unverified (see Gaps).

### Gaps
- gix SHA-256 support status in 0.80 and 0.9x was not checked this session.
- No FELINE or GRAIL papers were re-fetched. The interval-label precedent is background knowledge.

---

## Q9. Do existing API behaviors (DFS order, BFS depth, topological traversal incl. direction=children, max_nodes truncation) survive the redesign?

### Takeaway
BFS depth semantics and DFS can be carried over unchanged. The topological traversal cannot be replaced by "descending ID" without changing its contract:
- It returns a different node set under truncation.
- It needs ascending order for `children`.
- Its output depends on ingestion history.
- Its "no heap" scan can touch nearly the whole repo for a 100-node answer.
Shortest path needs a budget flag, and children need an index the layer format lacks.

### Cited Findings
- Current topological traversal (`indexer.rs:2245-2338`):
  - It first collects a **BFS ball** of up to `max_nodes` (`collect_reachable_bfs`, `indexer.rs:2340-2389`).
  - It then runs Kahn's algorithm with depth buckets, so ready nodes go out by smallest BFS depth.
  - It returns `(node, BFS depth)` pairs and rejects `direction=both` (`indexer.rs:2039-2046`).
- BFS returns `(node, level)` and marks `truncated` when the frontier exceeds the remaining budget (`indexer.rs:2134-2173`). DFS marks visited on push and reports `has_more` (`indexer.rs:2214-2243`) — [indexer.rs](/Users/singh.deependra-int/Desktop/git/FinalGitGraph/apps/desktop/src-tauri/src/application/indexer.rs)
- Base CSR keeps git parent order (`for parent_sha in &commit.parents`, `db.rs:1527-1541`). Overlay parents are re-sorted by descending ID (`parent_ids.sort_unstable_by(|left, right| right.cmp(left))`, `indexer.rs:1980`) — [db.rs](/Users/singh.deependra-int/Desktop/git/FinalGitGraph/apps/desktop/src-tauri/src/adapters/db.rs); [indexer.rs](/Users/singh.deependra-int/Desktop/git/FinalGitGraph/apps/desktop/src-tauri/src/application/indexer.rs)
- The blueprint ledger says "BFS/DFS/topological/shortest-path APIs: Keep the API, replace the implementations — Same contracts, ID-order algorithms, budgets and cancellation". The query text says "Topological streaming with a limit becomes 'iterate descending IDs over the visible set', with no in-degree pass or heap" — [blueprint](/Users/singh.deependra-int/Desktop/git/FinalGitGraph/reports/FinalGitGraph%20backend%20architecture%20blueprint.md)
- git.git, linear descending-ID scan with a marked set (blueprint numbering), IDs scanned to emit k ancestors from 306 starts (6 heads + 300 random commits): k = 100, median 156, p90 927, **max 77,495**. k = 1,000, median 1,153, max 84,033. The worst start is a head in the v1.4.4.5 tag lineage, numbered last (ID 85,935) with old ancestors — [local git.git measurement](https://github.com/git/git)

### Inferences
- **Truncation set changes.** Today, `max_nodes = k` returns the k nodes nearest the start by BFS level, ordered topologically. "First k in descending ID" returns the k highest-ID ancestors. Under first-parent-last numbering, that is mostly the start's first-parent stretch plus whatever was numbered just below it, a different set. That is a contract change, not "same contracts".
- **Direction=children.** A topological stream of descendants is *ascending* ID. It also needs the children index (attack 7) and the visible filter (Q5).
- **Depth field.** It can still be produced in a descending-ID stream by forward relaxation: dist[p] = min(dist[p], dist[u] + 1) when u is emitted. All children of p have higher IDs, so dist[p] is final when p is reached. The blue team can cite this. Depth survives, but the truncation set does not.
- **Cost.** Without a heap, the scan is O(ID interval), not O(k): up to 90% of git.git for `-n 100`. A max-heap over frontier IDs gives the same descending order at O(k log w) and touches only ancestors. Git's three-walk Kahn exists because git lacks a topological numbering, not because a heap is expensive.
- **UX.** jj's experience suggests raw index order (creation order) is a poor display order for branchy history. jj layers a topo-grouping adapter on top. A graph-rendering UI will likely need the same.
- **DFS order.** It is preserved only if parent order is stored exactly as in git. Today's overlay path already violates this (indexer.rs:1980), so the oracle should pin "ordered parent lists" (the blueprint does list that) and the DFS output for octopus and merge fixtures.
- **Shortest path.** Adding a budget requires a third result state. Today's `found: false` would become ambiguous. The `both` direction must not inherit the interval restriction (Q2).

### Gaps
- I did not inspect the Tauri `commands.rs` or the frontend for which of these semantics the UI relies on.

---

## Q10. Claims tested and not broken

### Takeaway
The core numbering invariant and the three O(1) cascade rules are sound. Most failures are in the parts built around them.

### Cited Findings
- Parent < child after a full first-parent-last DFS on git.git: 0 violations. After 200 incremental refreshes, including late-entering refs: 0 violations — [local git.git measurement](https://github.com/git/git)
- The cascade (`id(A) > id(B) ⇒ false`; `level(A) ≥ level(B), A ≠ B ⇒ false`; same first-parent run with id(A) ≤ id(B) ⇒ true): 0 wrong answers over 9 sampled distributions of 1,500 queries each, across both visit orders — [local git.git measurement](https://github.com/git/git)
- "Most parent hops become i → i−1": 82.5% of first-parent hops on git.git under first-parent-last, and master's first-parent chain is 96% contiguous (986 breaks in 24,442 hops) — [local git.git measurement](https://github.com/git/git)
- Merges whose first parent is an ancestor of the second (they would break the "merge continues its first parent's run" pattern): 2/400 sampled, which is negligible — [local git.git measurement](https://github.com/git/git)
- Sapling constants and flags as described in note Q5 (16, 4, ONLY_HEAD, HAS_ROOT, "O(flat segments), or O(merges)" for range) — [iddag.rs](https://docs.rs/sapling-dag/latest/src/dag/iddag.rs.html)
- The jj index format is "in some topological order with parents first", with u32 global parent positions and overflow handling, and it stores a u32 generation number per entry — [jj-lib readonly.rs](https://docs.rs/jj-lib/latest/src/jj_lib/default_index/readonly.rs.html)

### Inferences
- The shortest-path interval restriction is valid for `parents` (and mirrored for `children`). Only `both` is broken.
- The 1.25 MB-per-10M bitset figure is arithmetically correct.
- u32 IDs suffice for a single desktop repo, even with a reserved overflow bit.
- Rejecting 2-hop labeling for a desktop app is consistent with the cited O'Reach build and size figures. Not re-verified, not attacked.
- Git's commit-graph positions are OID-sorted, so renumbering is required. That is consistent with the gitformat chunk description.

### Gaps
- Every measurement above comes from one merge-heavy repository. Confirm on linux.git and on a linear, rebase-centric repo before the ADR cites them, per AGENTS.md rule 8.
