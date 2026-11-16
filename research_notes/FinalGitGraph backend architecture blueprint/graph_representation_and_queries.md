# Graph representation, reachability indexing and query algorithms for very large commit DAGs (1M–10M+ commits)

Scope note: research only. Evidence is separated from inference. "Background (unverified this session)" marks things I know from prior literature but did not re-fetch in this session; the report writer should treat them as lower confidence or re-verify before quoting numbers. All dated material checked as of 2026-11-01.

---

## Q1. How do git's generation numbers, commit-graph, Bloom filters and reachability bitmaps speed up ancestry, merge-base and topo-order queries? Measured speedups?

### Takeaway
Git's speedups come from three cheap things: a columnar, mmap-able file with u32 parent positions (no object parsing, O(1) parent hops), a monotone generation value per commit (prunes walks), and per-commit changed-path Bloom filters. On the Linux kernel, reachability queries drop 100–300x (seconds to tens of ms). Topo-order with a limit drops ~300x. Path-limited log drops ~5.7x. Reachability *bitmaps* target object counting (fetch/clone), not commit ancestry. Roaring bitmaps were tried in git and never merged. Pseudo-merge bitmaps (Git 2.46) are still experimental.

### Cited Findings
**commit-graph file format (primary spec)**
- Layout: OIDF fanout = 256 x 4 bytes, where F[i] = number of OIDs whose first byte is <= i. OIDL = all commit OIDs sorted ascending (N x H bytes). CDAT = N x (H + 16) bytes per commit, holding root tree OID, the first two parent positions, and generation + commit time. Optional chunks: GDA2 (N x 4 bytes corrected-commit-date offsets), GDO2 (8-byte overflow), EDGE (octopus parents), BIDX/BDAT (Bloom filters), BASE (chain bases) — [gitformat-commit-graph](https://git-scm.com/docs/gitformat-commit-graph)
- For SHA-1 (H=20) a CDAT row is 36 bytes. For SHA-256 (H=32) it is 48 bytes. These follow from the formula N x (H+16) — [gitformat-commit-graph](https://git-scm.com/docs/gitformat-commit-graph)
- Parents are stored as u32 positions. The value 0x70000000 means "no parent". If a commit has more than 2 parents, the MSB of the second field is set and the rest indexes EDGE. Max commits: "(1 << 30) + (1 << 29) + (1 << 28) - 1 (around 1.8 billion)" — [gitformat-commit-graph](https://git-scm.com/docs/gitformat-commit-graph)
- Generation v1 (topological level) lives in the high 30 bits of a 4-byte field. Commit time takes 34 bits. GDA2 stores corrected-commit-date *offsets* in 31 bits. Larger offsets set the MSB and point into GDO2 (8-byte values) — [gitformat-commit-graph](https://git-scm.com/docs/gitformat-commit-graph)
- Incremental chains: "The graph position for the ith commit in this file's OID Lookup chunk is equal to i plus the number of commits in all base graphs." Each layer appends positions after its bases — [gitformat-commit-graph](https://git-scm.com/docs/gitformat-commit-graph)
- Bloom: BIDX[i] is a cumulative byte offset. Hash v2 is 32-bit MurmurHash3 with seeds 0x293ae76f and 0x7e646e2, using double hashing. A commit with "no changes or more than 512 changes" gets a length-1 filter: all zeros or all ones — [gitformat-commit-graph](https://git-scm.com/docs/gitformat-commit-graph)
- Default Bloom settings in source: `#define DEFAULT_BLOOM_FILTER_SETTINGS { 1, 7, 10, DEFAULT_BLOOM_MAX_CHANGES }` (hash_version 1, 7 hashes, 10 bits per entry, max changed paths). max_changed_paths is "Not written to the commit-graph file" — [git bloom.h](https://raw.githubusercontent.com/git/git/master/bloom.h)

**Generation numbers (definitions and rationale)**
- Topological level: a root is 1. Any other commit is 1 + the max level of its parents. Corrected commit date: a root uses its commit date. Otherwise take the max corrected date of the parents; if that max exceeds the commit's own date, use max + 1, else use the commit date. If gen(X) < gen(Y), then X cannot reach Y. Equal values give no information — [GitHub blog, Git's database internals II](https://github.blog/open-source/git/gits-database-internals-ii-commit-history-queries/)
- Why v2 beats v1: a merge can bring in a commit based on a very old commit. Its topological level is then much smaller than its neighbours' levels, so the cutoff gets weak. Corrected dates keep the generation property and stay close to real dates — [GitHub blog II](https://github.blog/open-source/git/gits-database-internals-ii-commit-history-queries/)

**Measured speedups, Linux kernel (without -> with commit-graph)**
- `git rev-list v5.19`: 6.94s -> 0.98s. `git rev-list v5.0..v5.19`: 2.51s -> 0.30s. `git merge-base v5.0 v5.19`: 2.59s -> 0.24s (about 6x from parsing alone) — [GitHub blog II](https://github.blog/open-source/git/gits-database-internals-ii-commit-history-queries/)
- `git tag --contains v5.19~100`: 7.34s -> 0.04s. `git merge-base --is-ancestor v5.0 v5.19`: 2.64s -> 0.02s. The search stops early through the generation cutoff and depth-first first-parent traversal — [GitHub blog II](https://github.blog/open-source/git/gits-database-internals-ii-commit-history-queries/)
- Incremental topo-order uses Kahn's algorithm with 3 interleaved walks: (1) an in-degree walk driven by a priority queue on generation, (2) an output walk driven by a stack, (3) an exclusion walk for A..B. Timings: `git rev-list --topo-order -n 100 v5.19` 6.88s -> 0.02s. `git log --graph -n 100 v5.19` 7.73s -> 0.03s. `--topo-order -n 100 v5.18..v5.19` 0.39s -> 0.02s — [GitHub blog II](https://github.blog/open-source/git/gits-database-internals-ii-commit-history-queries/)

**Changed-path Bloom filters, Linux kernel (no commit-graph / commit-graph / +Bloom)**
- Each filter covers paths changed versus the **first parent** only. A "no" is definite. When the filter says treesame, git "ignores the other parents". Git also tests path prefixes (A, A/B, A/B/C) to cut false positives. "a random treesame path has a 98% likelihood of being reported as definitely treesame" — [GitHub blog III](https://github.blog/open-source/git/gits-database-internals-iii-file-history-queries/)
- `git log -- drivers/gpu/drm/i915/TODO.txt`: 1.03s / 0.67s / 0.18s. `--full-history`: 17.8s / 11.0s / 3.81s. `git blame Makefile`: 1.04s / 0.82s / 0.21s. `git log -L100,110:`: 9.67s / 2.64s / 1.38s. Blame and -L gain less because filters only skip the initial treesame check — [GitHub blog III](https://github.blog/open-source/git/gits-database-internals-iii-file-history-queries/)

**Reachability bitmaps, Roaring, pseudo-merges**
- Pseudo-merge bitmaps (Git 2.46, experimental) "store the set of objects reachable from multiple commits, rather than any single one". They let a traversal mark many ref tips at once instead of walking manually from refs that lack their own bitmap. Config: `bitmapPseudoMerge.<name>.pattern`, `.threshold`, `.stableThreshold`. No benchmark numbers were published in the release post — [GitHub blog, Highlights from Git 2.46](https://github.blog/open-source/git/highlights-from-git-2-46/)
- Roaring in git: Abhradeep Chakraborty posted the GSoC RFC "[PATCH 0/5] [RFC] introduce Roaring bitmaps to Git" on 2022-09-19, with some tests failing. In March 2023 Taylor Blau said there was no serious traction. In July 2023, Han-Wen Nienhuys's JGit experiment found small file-size differences that "actually favored EWAH". Blau reported that a Roaring+Run variant with XOR-against-other-bitmaps was slightly smaller. I found no evidence of a merge as of the latest material (2023) — [lore: RFC thread](https://lore.kernel.org/git/xmqqczbdl6wl.fsf@gitster.g/T/); [lore: status thread](https://lore.kernel.org/git/ZMlBdb5jEW3s7UWX@nand.local/T/); [public-inbox: Blau reply](https://public-inbox.org/git/ZMgXBc5idN+sR3o1@nand.local/)

### Inferences
- Git commit-graph positions are in **OID-sorted order**, not topological order. Git therefore needs generation numbers to recover topological pruning. A design that numbers nodes topologically gets "id(A) > id(B) implies A is not an ancestor of B" for free, and keeps generation as a second, stronger filter (corrected date for walk heuristics, topo level for exact depth).
- The Linux numbers set a target bar. With an mmap'ed columnar store and generation pruning, is-ancestor and merge-base on a ~1.3M-commit repo run in ~20 ms. Sub-millisecond needs an index beyond generation numbers (see Q2/Q5/Synthesis).
- Theoretical Bloom FP rate at 10 bits/entry and k=7 is (1 - e^(-0.7))^7 ≈ 0.8% (my arithmetic). The blog's "98% treesame" (~2% FP) is looser. Possible causes: filters are sized from the number of changed paths, and short filters behave worse. This is a mild discrepancy, not a contradiction.
- Current FinalGitGraph orders nodes by committer time descending. Corrected commit date exists precisely because raw commit dates can violate parent < child. So "committer-time order = topological" is unsafe on real repos with clock skew.

### Gaps
- No primary measured speedups for classic EWAH reachability bitmaps on commit-ancestry queries were fetched. Bitmaps are mainly an object-counting structure. Background (unverified this session): EWAH bitmaps date from JGit/Git ~2013 and gave large clone/fetch counting wins at GitHub.
- No published numbers for pseudo-merge bitmaps were found. Git's commit-graph chain merge policy (`--split`, `--size-multiple`, `--max-commits`) was not fetched.

---

## Q2. Which DAG reachability indexes exist, and which support cheap insertion of new nodes pointing only to older nodes?

### Takeaway
On generic large DAGs, the state of the art splits into (a) exact 2-hop labeling (PLL/TF), which is fastest per query (<1 µs) but has GB-scale indexes, hours of build time and poor incrementality, and (b) lightweight filter + guided-search methods (GRAIL, FERRARI, Feline, IP+, BFL, PReaCH, O'Reach), which answer most queries in O(1) and fall back to pruned (bidirectional) search. For append-only commit DAGs, label-based filters that only need "new node's label = function of its parents' labels" are the natural fit. Topological levels, generation numbers, BFL-style Bloom labels and interval/dominance filters can all be extended to new sink-to-source appends in O(outdegree) without relabeling.

### Cited Findings
- O'Reach (Hanauer, Schulz, Trummer, 2020/2021) combines topological levels (forward and backward), t extended topological orderings (t=4) built by a modified Tarjan DFS, and k=16 "supportive vertices" with full in/out reachability sets (chosen from 1,200 candidates). It falls back to a pruned bidirectional BFS — [O'Reach (ar5iv)](https://ar5iv.labs.arxiv.org/html/2008.10932)
- O'Reach complexity: init O((t + kp)(n + m)). Index size (12 + 12t + 2⌈k/8⌉)·n bytes = 64n bytes for t=4, k=16. Query O(k + t + 1) for observation tests, O(n + m) worst case with fallback — [O'Reach](https://ar5iv.labs.arxiv.org/html/2008.10932)
- About 95% of negative queries and over 70% of positive queries are answered by O(1) observations. Fallback rate is 28% on positive sets, and above 90% on some instances (e.g., cit-Patents) — [O'Reach](https://ar5iv.labs.arxiv.org/html/2008.10932)
- "Only PPL and O'R +PPL achieved an average query time of less than 1 µs". O'Reach + PReaCH averages "at most 11.1 µs" across all query sets. Speedup from adding O'Reach to fallbacks: geometric mean >= 1.29, up to 4.21 (IP(s), positive) — [O'Reach](https://ar5iv.labs.arxiv.org/html/2008.10932)
- Max index sizes across the benchmark: IP(s) <= 395 MB, BFL(s) 429 MB, IP(d) 440 MB, BFL(d) 754 MB, PReaCH 1.3 GB, O'Reach 1.5 GB, PPL 4.4 GB. TF reached 3.8 GB and needed >64 GB during initialization on some instances. PPL construction reached up to 2.6 h. BFL(s) built fastest, then BFL(d) and PReaCH — [O'Reach](https://ar5iv.labs.arxiv.org/html/2008.10932)
- "Surprisingly, due to cache effects, a higher investment in space doesn't necessarily pay off". Some queries beat a single memory access into a full reachability matrix — [O'Reach abstract](https://arxiv.org/abs/2008.10932)
- Survey: Zhang, Bonifati, Özsu, "Indexing Techniques for Graph Reachability Queries" covers 33 techniques for plain and edge-labeled graphs (arXiv 2311.03542, rev. Jun 2025) — [arXiv](https://arxiv.org/abs/2311.03542)

### Inferences
- Background (unverified this session; verify before quoting numbers). These are qualitative mechanisms only:
  - GRAIL: k random DFS post-order intervals, a negative-only filter, then guided DFS.
  - FERRARI: tree cover with bounded exact/approximate intervals.
  - Feline: 2D dominance from two topological orders.
  - IP+: independent permutation (min-wise) label sets.
  - BFL: Bloom-filter labels of in/out sets on DFS intervals.
  - PReaCH: pruning + contraction-hierarchy-like order, with topological levels and DFS intervals.
  - TF-label: topological folding, 2-hop.
  - PLL/PPL: pruned landmark labeling, 2-hop.
- Append-only fit. Ancestry direction is "B reaches A along parent edges". A new commit u only has edges to existing nodes, so u is a new **source** (no in-edges), not a sink. Then:
  - Topological level / generation: gen(u) = 1 + max gen(parents), or the corrected-date rule. O(deg), exact, no relabel. Already proven in git (Q1).
  - BFL/IP-style "out-set" sketches (Bloom/min-hash of the ancestor set): sketch(u) = OR/min over parents of sketch(p) plus u's own bit. O(deg · sketch) per append, exact maintenance, no relabel. Sketches of *other* nodes' in-sets (descendants) would need updating on append. Only maintain the ancestor-side sketch; it is the one needed to answer "is A an ancestor of B" negatively (A not in sketch(B) means no).
  - GRAIL/Feline interval/dominance labels: append u at the end of every order (post(u) = max + 1). This stays valid but loses discriminative power for queries "is old A an ancestor of new u?" Those always pass the filter. Mitigation: the delta since the last rebuild is small. A query does a short search through new nodes until it hits labeled base nodes, then uses their labels. Rebuild orders at compaction.
  - 2-hop (PLL/TF): incremental insertion of a new source with known out-edges is possible (u's label = union of parents' labels after pruning). But label size grows, and the excellent query times depend on a global landmark order chosen at build time. Build times of hours and GB-sized indexes (above) make them a poor fit for a desktop app at 10M nodes.
- Commit DAGs differ sharply from the benchmark graphs. Outdegree is ~1–2, almost every node reaches the root(s), and history is dominated by long first-parent chains. Hence:
  - Positive queries (A is an ancestor of B) are the common case.
  - Chain/segment decomposition (Q5) gives exact O(1)/O(log) positive answers for same-chain pairs.
  - Generic GRAIL with a single root degenerates to the topological filter. Every ancestor interval starts at the root's post number, so k random orders are needed to get any pruning.

### Gaps
- I could not extract per-dataset query times or the largest dataset sizes from O'Reach Tables 2–8 (cells lost in extraction).
- I found no benchmark of reachability indexes on real commit DAGs (Linux, Chromium, monorepos). I found no paper on reachability indexing specialized for version-control DAGs, other than Sapling's segment approach (Q5).
- Dynamic 2-hop/BFL variants (for example "DBL", dynamic BFL; TOL; Dagger for GRAIL) were not fetched. Their insertion costs are unverified.

---

## Q3. Which compressed/succinct representations fit, and what traversal cost do they impose?

### Takeaway
For commit DAGs, the dominant storage cost is the hashes (20/32 random bytes per commit, incompressible), not the edges. Edges compress to almost nothing once nodes are numbered topologically along first-parent chains: "parent = id - 1" is implicit for most commits. Run-length/segment encoding beats generic graph compressors (WebGraph BV, k²-trees). Those compressors target high-degree web/social graphs with similarity and locality between adjacency lists. Roaring/EWAH/Elias–Fano are useful for *sets of IDs* (ancestor sets, path posting lists), not for the adjacency itself.

### Cited Findings
- WebGraph's format uses gap and differential compression (à la LINK), intervalisation and ζ codes. The Rust port ("WebGraph: The Next Generation (Is in Rust)", Fontana, Vigna, Zacchiroli, WWW 2024 companion) reports roughly 3x faster BFS than Java on the Software Heritage graph (34 billion nodes, 517 billion arcs). It also reports γ-code reads in under 2 ns. The crate is `webgraph` v0.5.0, dual LGPL-2.1+/Apache-2.0 — [WebGraph Rust paper record](https://researchportal.ip-paris.fr/en/publications/webgraph-the-next-generation-is-in-rust/); [webgraph-rs GitHub](https://github.com/vigna/webgraph-rs); [FOSDEM 2024 slides](https://archive.fosdem.org/2024/events/attachments/fosdem-2024-3662--serde-memdbg-sux-dsi-bitstream-webgraph-a-rust-ecosystem-for-large-graph-processing/slides/22353/FOSDEM_2BcnJwz.pdf)
- Sapling's on-disk segment encoding: `SEGMENT := FLAG (1B) + LEVEL (1B) + HIGH (8B) + vlq(HIGH-LOW) + vlq(PARENT_COUNT) + vlq(VLQ, PARENTS)`. HIGH is fixed-width because range lookups need byte order to match integer order. HIGH-LOW is stored instead of LOW because it is "more compact in the worst case where each flat segment has length 1" — [sapling-dag segment.rs](https://docs.rs/sapling-dag/latest/src/dag/segment.rs.html)
- Roaring vs EWAH for git reachability bitmaps: in a JGit experiment, file-size differences were small and favored EWAH. Roaring+Run with XOR was slightly smaller — [lore status thread](https://lore.kernel.org/git/ZMlBdb5jEW3s7UWX@nand.local/T/)
- Succinct Rust crates: `vers-vecs` (v1.9.0) offers pure-Rust rank/select bit vectors, Elias–Fano, wavelet matrices, and an optional SIMD feature. The author's self-reported benchmarks: sucds is better in the average case but much slower in the worst case. Vers EF predecessor queries are worst-case logarithmic, versus linear for sucds. The `elias-fano` crate is inefficient for random-order access — [vers-vecs README](https://docs.rs/crate/vers-vecs/latest/source/readme.md); [lib.rs vers-vecs](https://lib.rs/crates/vers-vecs)
- The webgraph-rs ecosystem uses `dsi-bitstream` (instantaneous codes) and `sux` (pointers into the bitstream) — [FOSDEM 2024 slides](https://archive.fosdem.org/2024/events/attachments/fosdem-2024-3662--serde-memdbg-sux-dsi-bitstream-webgraph-a-rust-ecosystem-for-large-graph-processing/slides/22353/FOSDEM_2BcnJwz.pdf)

### Inferences
- Size arithmetic at N = 10M commits, assuming ~1.1 parents per commit (assumption, not measured):
  - Plain u32 CSR, parents only: offsets 40 MB + neighbours ~44 MB = **~84 MB**. Children CSR is the same again: ~84 MB.
  - Flat-segment encoding: assume a fraction f of commits start a segment (merges, roots, branch forks). At f = 0.1 that is ~1M segments x ~10–18 bytes ≈ **10–18 MB**. It replaces both parent CSR and much of the children lookup.
  - Hashes: 20 B x 10M = **200 MB** (SHA-1), or 320 MB (SHA-256). This is a floor unless hashes stay in an mmap'ed file and are paged in on demand.
- Traversal cost. BV/ζ-coded adjacency costs a sequential decode (a few ns per arc, per the slides' γ figure). It also loses O(1) random access to a node's parents without an Elias–Fano offset index. For outdegree ~1 that overhead dominates any space saving. Recommendation: no WebGraph/k²-tree for adjacency. Use segments plus small explicit parent lists.
- Elias–Fano fits monotone sequences, for example a "commits touching path P" posting list in topological-ID order, or the sorted segment-start IDs. Use it there, or Roaring when sets are dense or clustered.
- Ancestor sets in topological-ID order are mostly long runs. Roaring run containers, or a plain span list (Sapling's SpanSet), are near-optimal. EWAH also handles runs well. This is consistent with the JGit finding that Roaring is not a decisive win.

### Gaps
- No measured bits-per-edge figure for any commit DAG under WebGraph BV was found. The WebGraph paper's own tables were not retrieved.
- Maintenance status of `sucds` and `roaring` (roaring-rs) crates was not verified this session. Background (unverified): roaring-rs is actively maintained under the RoaringBitmap org.
- k²-tree traversal costs were not fetched. They are typically used for adjacency-matrix-style compression of web/RDF graphs, which is irrelevant at outdegree ~1.

---

## Q4. Which dynamic/updatable graph structures (PCSR/PMA, LLAMA, Aspen, Terrace, GraphOne, Teseo, LSGraph, multi-snapshot stores) suit append-only commit DAGs?

### Takeaway
None of the general dynamic graph stores is worth adopting. They solve arbitrary edge insert/delete on skewed, high-degree graphs, and pay 3–11x CSR memory plus read slowdowns. A commit DAG never deletes edges. A node's out-edges (parents) are complete when the node is inserted, and new node IDs are always the maximum. Parent CSR is therefore *naturally append-only*. Only children lists grow in the middle, and a small delta overlay (an LSM-style layer, as in git commit-graph chains) handles that.

### Cited Findings
- "Revisiting the Design of In-Memory Dynamic Graph Storage" (Su et al., arXiv 2502.10959, Feb 2025) benchmarks LLAMA, Aspen, LiveGraph, Teseo and Sortledton. Aspen uses "3.3-10.8x more memory than CSR". The best fine-grained methods use ~4.1–8.9x CSR memory. Existing methods neglect memory-access effects, which degrades them relative to contiguous storage — [arXiv 2502.10959](https://export.arxiv.org/abs/2502.10959). A secondary summary states that CSR consistently outperforms dynamic graph stores on read-heavy workloads — [search summary of the same benchmark](https://export.arxiv.org/pdf/2502.10959)
- Terrace (SIGMOD 2021) uses a hierarchical, degree-aware layout. It reports faster batch inserts than Aspen for batches up to 1M, and 1.7–2.6x faster graph queries than Aspen — [Terrace paper PDF](https://people.eecs.berkeley.edu/~aydin/terrace.pdf)
- LSMGraph (SIGMOD 2024) combines LSM-tree writes with read-optimized CSR for disk-based dynamic graphs. This comes from a secondary search snippet; the paper was not fetched — [rmarcus.info dbscholar](https://rmarcus.info/dbscholar/papers/h6abc8cce62755197)
- Git's own incremental structure is a chain of immutable commit-graph layers. Positions in a layer continue after all base layers — [gitformat-commit-graph](https://git-scm.com/docs/gitformat-commit-graph)

### Inferences
- Fit analysis for commit DAGs:
  - **PCSR/PMA and Terrace** pay for gaps to allow mid-array edge inserts. Commit parent rows never change after insert, so the gaps are wasted.
  - **Aspen (C-trees)** pays 3–11x memory for functional snapshots. Snapshots of an append-only DAG are free anyway: "snapshot = prefix [0, N_at_time)" when IDs are topological and append-ordered. This is the key structural win.
  - **LLAMA** (multi-versioned array snapshots, background) is the closest conceptual match: a read-only base plus delta levels. It is still more general than needed.
  - **GraphOne, Teseo, LiveGraph, Sortledton, LSGraph** target streaming mixed workloads with concurrency. That is unnecessary for a single-writer desktop indexer.
- Recommended structure. An immutable base file (columnar, mmap) plus 0..k small immutable delta layers, merged by size ratio (git-chain-style). Parents and generation are append-only columns. Children are either (a) derived from segments (Sapling computes `children` from segments), or (b) kept as a base children-CSR plus a delta "parent -> new children" list per layer, merged at compaction.
- Versioned/multi-snapshot queries ("graph as of ingestion k") are a single integer bound per layer. No MVCC machinery is needed.

### Gaps
- I did not obtain the full Su et al. numbers (read slowdown factors, insert throughput) or LSGraph (EuroSys 2024) numbers. GraphOne, Teseo, PCSR and LLAMA original papers were not fetched this session.

---

## Q5. Is topological ID assignment the right base? Evidence from Mercurial revlog and Meta/Sapling Segmented Changelog (IdDag) in detail.

### Takeaway
Yes. This is the single most important change. Numbering commits so that every parent ID is lower than its child's ID (append order = topological order) gives:
- free topological order and free snapshots;
- O(deg) append with no relabeling;
- single-pass algorithms (forward scan for generation, backward scan with a bitset for ancestor marking);
- the ability to compress history into "flat segments" (maximal ID runs where id-1 is the parent).

Sapling's IdDag builds hierarchical segments (fanout 16, up to 4 meaningful levels) on top. It answers ancestors/range/gca/is_ancestor in O(#flat segments) ≈ O(#merges), and serves tens of millions of commits.

### Cited Findings
**IdDag definitions (Sapling source, primary)**
- IdDag is a "Structure to store a DAG of integers, with indexes to speed up ancestry queries." A segment is "`(level: int, low: int, high: int, parents: [int])` on a topo-sorted integer DAG" with three invariants. (1) `high` is the only head in the sub-DAG it covers. (2) `parents` have no entries in `low..=high`. (3) For level 0, each x in `low+1..=high` has parent x-1 — [sapling-dag iddag.rs](https://docs.rs/sapling-dag/latest/src/dag/iddag.rs.html)
- `Segment::new` asserts `high >= low`, low and high in the same group, and every parent < low — [sapling-dag segment.rs](https://docs.rs/sapling-dag/latest/src/dag/segment.rs.html)
- Hierarchy constants:
  - `DEFAULT_SEG_SIZE = 16`, annotated "See benches/segment_sizes.rs (D16660078) for this choice". Each high-level segment covers at most `size` level-1 segments.
  - `MAX_MEANINGFUL_LEVEL = 4`, because "it is good enough for an existing large repo" and "level 5 is not built because it cannot merge level 4 segments".
  - "The last segment per level per group is dropped because it's likely to be incomplete."
  — [iddag.rs](https://docs.rs/sapling-dag/latest/src/dag/iddag.rs.html)
- Flags: `HAS_ROOT` is required for correctness. `ONLY_HEAD` means "heads(0..=head) = [head], or ancestors([head]) = 0..=head". It enables an O(1) fast path for ancestors of a main-line commit. It is an optimization only — [segment.rs](https://docs.rs/sapling-dag/latest/src/dag/segment.rs.html); [iddag.rs](https://docs.rs/sapling-dag/latest/src/dag/iddag.rs.html)
- Query algorithms in IdDag:
  - `ancestors` uses the ONLY_HEAD fast path, else walks high-level segments before flat ones.
  - `heads(set)` = set - parents(set).
  - `roots(set)` = set - children(set).
  - `range` ("ids reachable from both sides") is documented as "O(flat segments), or O(merges)".
  - `gca_one`, `common_ancestors`, `is_ancestor` and `heads_ancestors` are implemented via ancestors.
  — [iddag.rs](https://docs.rs/sapling-dag/latest/src/dag/iddag.rs.html)
- Groups. The **master group** holds the main line. It "usually only has 1 head", so a linear scan over flat segments is used. The **non-master group** "can have lots of heads... that are no longer visible", so only flat segments covered by `ancestors` are checked. VIRTUAL spans are excluded from high-level segments — [iddag.rs](https://docs.rs/sapling-dag/latest/src/dag/iddag.rs.html)
- Flat segments are an intermediate product (`PreparedFlatSegments`) produced by `IdMap::assign_head` / `IdDag::all_flat_segments`. They are used to build the IdDag and in the lazy pull protocol, where the payload is ~O(segments) — [sapling-dag PreparedFlatSegments docs](https://docs.rs/sapling-dag/latest/dag/struct.PreparedFlatSegments.html)

**Production evidence (Meta)**
- Sapling downloads "the high-level shape of the commit graph" in "just a few megabytes" and lazily fills commit data. Graph-relationship queries between two commits run in "O(number-of-merges) time" using "nothing but the segments and the position of the two commits in the segments". It can "bisect the segment graph to find the history in O(log n) time, instead of O(n)", also on Git repos. Smartlog takes "less than a second, regardless of how big the repository is". Meta's repo has "tens of millions of files, tens of millions of commits, and tens of millions of branches" — [Meta Engineering, Sapling announcement (2022-11-15)](https://engineering.fb.com/2022/11/15/open-source/sapling-source-control-scalable/)

### Inferences
- Mercurial revlog (background, unverified this session): revision numbers are local and assigned in append order. A revision can only be stored after its parents, so rev numbers are topologically sorted (parent < child). This is the 20-year-old precedent that Sapling's IdDag generalizes. Sapling adds a DFS-based assignment (`assign_head`) so first-parent chains become contiguous, which maximizes flat-segment length.
- Why FinalGitGraph's current order breaks:
  - Committer-time-descending order is not guaranteed topological (Q1, corrected-date rationale).
  - Appends at the end of a descending order break even that monotonicity.
  - The suspected bug (edges dropped when children arrive before parents across batches) is exactly the failure that topological ID assignment prevents. With topological IDs, an ID can only be issued once all parents have IDs. A commit whose parent is still unknown is a hard error, or is parked in a "pending" set (shallow/graft boundary), never silently dropped.
- Single-pass algorithms enabled (parents < child):
  - gen[i] = 1 + max(gen[p]) in one forward scan.
  - The ancestors of a set S, restricted to [lo, hi], come from a backward scan from max(S) with a bitset: if marked[i], mark its parents. Sequential, branch-light, about 1.25 MB of bits per 10M nodes.
  - A..B = backward scan with two bitsets.
  - The topological order *is* the ID order. Kahn's algorithm and depth buckets disappear.
  - A snapshot at an ingestion point = an ID prefix.
- Cache locality: DFS/first-parent-contiguous numbering makes parent hops mostly i -> i-1 (adjacent memory). That is the best case for prefetchers and for column scans.
- Where IdDag is weaker: is_ancestor across many merges costs O(merges in range), which can reach 10^5–10^6 on merge-heavy monorepos. Add O(1) filters (generation, Bloom/ancestor sketch, chain test) and a hierarchical-segment walk with early exit, rather than computing full ancestor sets.

### Gaps
- The Sapling design doc page (sapling-scm.com/docs/internals/segmented-changelog) returned 404. I found no official published segment counts for real repos (for example flat segments for Linux or Meta's monorepo) or benchmark tables. The `benches/segment_sizes.rs` (D16660078) data is internal.
- The Mercurial revlog numbering docs were not fetched this session.
- The exact IdMap DFS assignment rules (which parent is visited first, and how non-master IDs are re-assigned when promoted to master) were not extracted; the rest of iddag.rs past 100k chars and idmap.rs were unread.

---

## Q6. Best algorithms for k-ancestors, merge-base/LCA, is-ancestor, DAG shortest path, topo streaming with limit, range queries (A..B, A...B) and file history

### Takeaway
With topological IDs plus generation numbers plus segments, nearly every query becomes either an O(1) label test or an ordered set operation on ID spans:
- ancestors(B) is a span set;
- A..B = ancestors(B) - ancestors(A);
- A...B = symmetric difference;
- merge-base = heads(ancestors(A) ∩ ancestors(B));
- topo streaming = iterate descending IDs over a span set with a limit;
- file history = (a Bloom-filtered walk) or (a posting list ∩ ancestor spans).

Git's measured approach (generation-pruned walks with priority queues) is the proven fallback.

### Cited Findings
- Git is-ancestor / tag --contains: generation cutoff ("a commit whose generation is below the target's generation cannot lead to it") plus depth-first first-parent traversal. 2.64s -> 0.02s on Linux — [GitHub blog II](https://github.blog/open-source/git/gits-database-internals-ii-commit-history-queries/)
- Git topo-order with a limit: incremental Kahn with a generation-ordered priority queue for in-degree, a stack for output, and a third walk for range exclusion. 6.88s -> 0.02s for `-n 100` — [GitHub blog II](https://github.blog/open-source/git/gits-database-internals-ii-commit-history-queries/)
- Sapling `range` is O(flat segments)/O(merges). heads/roots are set differences with parents/children. gca/common_ancestors/is_ancestor are built on ancestors — [iddag.rs](https://docs.rs/sapling-dag/latest/src/dag/iddag.rs.html)
- Sapling bisects the segment graph for log/blame history in O(log n) — [Meta Engineering](https://engineering.fb.com/2022/11/15/open-source/sapling-source-control-scalable/)
- File history: per-commit Bloom filter vs first parent, plus prefix tests. 1.03s -> 0.18s for `git log -- path` on Linux. Defaults 7 hashes, 10 bits/entry, 512 max changed paths — [GitHub blog III](https://github.blog/open-source/git/gits-database-internals-iii-file-history-queries/); [bloom.h](https://raw.githubusercontent.com/git/git/master/bloom.h); [gitformat-commit-graph](https://git-scm.com/docs/gitformat-commit-graph)
- O'Reach-style query pipeline: O(1) observations first, then a pruned bidirectional BFS fallback — [O'Reach](https://ar5iv.labs.arxiv.org/html/2008.10932)

### Inferences (proposed algorithms; complexity is my analysis)
- **is_ancestor(A, B)**, i.e. does B reach A. Cascade, cheapest first:
  1. A == B gives true.
  2. id(A) > id(B) gives false. This is O(1), free from topological IDs.
  3. gen_level(A) >= gen_level(B) gives false (O(1)), unless A == B.
  4. Same flat segment and id(A) <= id(B) gives true (O(1) via segment lookup: binary search on segment highs, O(log S)).
  5. ONLY_HEAD master segment covering B, with id(A) <= high, gives true (Sapling fast path).
  6. Optional ancestor sketch (a BFL-like 64–256-bit Bloom of the ancestor set, maintained by OR at append): bit of A missing in sketch(B) gives false.
  7. Fallback: a segment walk from B over high-level then flat segments, pruning any segment whose high < id(A) or max-gen < gen(A), with early exit on a hit. O(merges between A and B) worst case. Alternatively, a generation-pruned DFS on first parent (git style).
  Expected: O(1) for most queries; sub-ms for walks touching ≤ ~10^4 segments (assumed ~10–50 ns per segment touch).
- **merge-base(A, B)** and all merge bases:
  - Segments: compute ancestors(A) and ancestors(B) as span sets, restricted below min(id) of candidates. The result is heads(intersection). Cost O(spans).
  - Git-style fallback: two-colour paint walk ordered by generation (corrected date) in a max-heap, stopping when every queued commit is already "stale". Generation v2 gives better pruning than v1 on old-branch merges (Q1).
  - Single-pass alternative: a backward ID scan from max(id A, id B) with 2-bit colours per ID, ending at the first ID where only "both"-coloured, non-dominated nodes remain. O(id range) sequential. Good when A and B are close; bounded by `limit`.
- **k-ancestors / log with limit (--topo-order -n k)**: descending ID iteration over ancestors(B) gives a valid reverse-topological, newest-first stream. With segments: walk spans from high to low, emit up to k. O(k + spans touched). No heap and no in-degree pass needed, because the ID order is already topological. For committer-date ordering (`git log` default), use a max-heap over span cursors keyed by date. O(k log(active spans)).
- **A..B and A...B**: span-set difference / symmetric difference, then stream as above. Both are O(spans).
- **Shortest path in a DAG (unweighted, B ancestor of A)**:
  - Bidirectional BFS restricted to IDs in [id(B), id(A)] and to nodes with gen(B) <= gen(v) <= gen(A).
  - Prune any node v with id(v) < id(B), or with the sketch test failing.
  - Optional exact DP: a backward scan over the ID interval computing dist via min(dist[child]+1). O(interval) and cache-friendly, best when the interval is small.
  - First-parent distance between two commits on the same segment = id difference. O(1).
- **Descendants / "contains" (tag --contains)**: ask "which of the tag tips T have A as an ancestor". Run is_ancestor(A, t) per tag with the cascade above. The id(A) > id(t) filter prunes most tags instantly. Or compute descendants(A) via children segments, restricted to IDs <= max(id(T)).
- **File history `log -- path`**:
  - Baseline: git-compatible Bloom per commit (10 bits/entry, 7 hashes, prefix tests, 512 cap). The walk becomes an ID-ordered scan with a Bloom check per commit.
  - Novel option: an inverted index path -> Roaring/Elias–Fano posting list of topological IDs that touch the path (first-parent diff), intersected with the span set ancestors(HEAD). Cost O(|posting ∩ range| + spans). This is a sub-ms answer for typical files. It costs one entry per (commit, changed path); limit it to directory prefixes and hot paths, or build it lazily.
- **Generation choice**: store both topological level (u32, exact depth, used for ID-independent pruning and shortest-path bounds) and corrected commit date (u64 or offset, used for date-ordered walks and git compatibility).

### Gaps
- No published measurements of span-set merge-base on real repos were found. All complexity claims above are analytical.
- Git's exact paint_down_to_common stopping rules and the ahead/behind (`git rev-list --count A...B`) bitmap path were not fetched.

---

## Q7. Memory layout: SHA interning, OID lookup (fanout + sorted array vs hash maps), cache efficiency, SIMD

### Takeaway
Store hashes as fixed-width binary. Use two columns: id -> oid in topological-ID order (20 B x N), and a sorted OID array + 256-entry fanout + sorted-position -> id permutation (another 20 B + 4 B x N), all mmap'ed. This costs ~44 B/commit with both arrays, or 24 B/commit if the sorted array alone is kept and id -> oid is resolved via a 4-byte inverse permutation. By my estimate that is ~4x smaller than the current `HashMap<String, id>` and needs zero hydration. Lookups take a fanout jump plus interpolation or binary search (SHAs are uniform). Traversal state uses dense bitsets indexed by ID, never hash sets.

### Cited Findings
- Git's OID lookup is an OIDF fanout (256 x 4 bytes, F[i] = count with first byte <= i) plus OIDL sorted OIDs. Rows in CDAT/BIDX are in the same order. Parents are referenced by u32 row position, so "moving from a commit to its parent takes constant time instead of a binary search" — [gitformat-commit-graph](https://git-scm.com/docs/gitformat-commit-graph); [GitHub blog II](https://github.blog/open-source/git/gits-database-internals-ii-commit-history-queries/)
- Hash version 1 = SHA-1 (20 B), 2 = SHA-256 (32 B). A mismatched hash version must be ignored with a warning — [gitformat-commit-graph](https://git-scm.com/docs/gitformat-commit-graph)
- vers-vecs offers SIMD-accelerated rank/select bit vectors in pure Rust (`simd` feature, benchmarks with `target-cpu=native`) — [vers-vecs README](https://docs.rs/crate/vers-vecs/latest/source/readme.md)

### Inferences
- Current cost estimate (my arithmetic, std hashbrown). For `HashMap<String hex, u32>`, each entry holds:
  - a String header (24 B) + a 40-B hex heap allocation (allocator size class ~48 B);
  - a u32 value plus padding: a ~32-B bucket;
  - a ~1-B control byte;
  - load-factor slack (≤ 7/8).
  That is **~85–100 B per commit**: ~0.85–1.0 GB at 10M, built on every query hydration. The binary fanout layout is 24 B/commit (sorted OIDs + permutation), ~240 MB at 10M, mmap'ed and paged in lazily. For lookups, hex-decode once at the API boundary.
- Lookup: the fanout narrows to ~N/256 ≈ 39k entries at 10M. Binary search takes ~15 probes. Interpolation search on the next 8 bytes (uniform SHA) has an expected O(log log n) cost of ~2–4 probes. An Eytzinger/B-tree-ordered copy of the top levels improves cache behaviour (background: Khuong & Morin array-layout results, unverified this session). Abbreviated-SHA (prefix) lookup falls out of the sorted array.
- Column layout (struct of arrays, little-endian; all columns indexed by topological ID except the sorted-OID columns):
  - `oid[id]`: 20 B.
  - `gen_level[id]`: u32.
  - `cdate[id]`: u32 or u64.
  - `corr_offset[id]`: u32 + overflow.
  - `seg_start` bitvector with rank/select (~1.03 bit/commit with vers-vecs-style rank).
  - `extra_parents` Elias–Fano or u32 array for segment starts.
  - optional `anc_sketch[id]`: 8–32 B.
  - Bloom index and data.
  At 10M commits, the core (no sketch, no Bloom) is ≈ 200 (oid) + 240 (sorted + perm) + 40 + 40 + 40 + ~20 ≈ **~580 MB on disk**, but only the touched pages are resident. Dropping `oid[id]`, using the sorted OIDs plus a 4-B inverse permutation, saves ~160 MB. Estimate only.
- Cache: traversal visited/colour state as `u64` bitsets over the ID range (1.25 MB per 10M). The working set of a range query is the ID interval, which is contiguous. Bitset AND/ANDNOT for set algebra auto-vectorizes in Rust (SSE/AVX2/NEON), which keeps it OS- and arch-agnostic without `unsafe`. That matters because AGENTS.md forbids `unsafe` without an ADR. Note: mmap via a crate like memmap2 needs `unsafe` at one call site, which requires an ADR. The alternative is `read`-into-buffer with an aligned Vec (safe, costs one copy).
- SIMD for SHA compare is not worth it. A 20-B compare is ≤ 3 word compares. Algorithmic choices dominate.

### Gaps
- No measured hashbrown per-entry overhead for `String` keys was fetched; the figures above are arithmetic.
- No measurement of interpolation vs binary search on SHA arrays was fetched.
- `memmap2` safety / `unsafe` requirements and cross-platform behaviour were not verified this session.

---

## Q8 (synthesis). What design should FinalGitGraph adopt? A defensible "state-of-the-art for commit DAGs" blueprint

### Takeaway
Replace the "committer-time-ordered CSR + HashMap<String> hydrated per query" with a **topologically numbered, segment-compressed, layered, mmap-able columnar commit index**. It combines git's commit-graph ideas (fanout OID lookup, u32 positions, generation v2, changed-path Bloom filters, layered chains) with Sapling's IdDag ideas (topological IDs, flat plus hierarchical segments, span-set algebra, master/non-master groups). Add an O'Reach-style O(1) filter cascade (topological ID, topological level, same-segment test, append-maintained ancestor sketch) in front of a segment-walk fallback. Each component has production or peer-reviewed precedent. The novelty is the combination, tuned for append-only DAGs, plus the posting-list file-history index.

### Cited Findings
- Git's components and their measured gains (100–300x for reachability, ~5.7x for path-limited log) — [GitHub blog II](https://github.blog/open-source/git/gits-database-internals-ii-commit-history-queries/); [GitHub blog III](https://github.blog/open-source/git/gits-database-internals-iii-file-history-queries/)
- Sapling's segments give O(merges) graph queries at tens of millions of commits, with a few-MB graph shape — [Meta Engineering](https://engineering.fb.com/2022/11/15/open-source/sapling-source-control-scalable/); [iddag.rs](https://docs.rs/sapling-dag/latest/src/dag/iddag.rs.html)
- O(1)-observation filters plus pruned bidirectional BFS answer ~95% of negative and >70% of positive queries in constant time on generic graphs. A bigger index does not necessarily pay off because of cache effects — [O'Reach](https://ar5iv.labs.arxiv.org/html/2008.10932)
- Dynamic graph stores cost 3.3–10.8x CSR memory, and CSR wins on read-heavy workloads — [Su et al. 2025](https://export.arxiv.org/abs/2502.10959)
- Layered immutable files with position continuation are proven in git — [gitformat-commit-graph](https://git-scm.com/docs/gitformat-commit-graph)

### Inferences (the proposed design; every bullet is a design inference, not measured)
1. **ID assignment (fixes the batch-boundary bug class)**
   - Assign u32 topological IDs, parent < child, via an iterative DFS from heads that emits parents first and visits the first parent last, so first-parent chains are contiguous (Sapling `assign_head` style).
   - Ingestion must stream in parent-first order, for example `git rev-list --topo-order --reverse`, or our own DFS over the object DB.
   - A missing parent is never dropped. It becomes a typed "pending/shallow boundary" record, and an edge is attached when the parent arrives (the ID still increases, because the child gets its ID only after the parent).
   - u32 covers 4.29B commits, ample for 10M+.
2. **Groups.** A "main" group (default-branch history, append-only, stable IDs, ONLY_HEAD fast path) and a "side" group (other refs, high ID range, re-assignable at compaction when merged into main). This mirrors Sapling's master/non-master groups and keeps main-line IDs stable forever.
3. **Storage.**
   - One immutable base file plus small immutable delta layers (IDs continue across layers), merged by size ratio in the background.
   - Columns: binary OIDs, fanout + sorted-OID permutation, flat segments (low, high, parents) + hierarchical segments (fanout 16, ≤ 4 levels), topological level, corrected date, Bloom index and data, optional ancestor sketch.
   - Children are derived from segments plus a per-layer delta map.
   - No whole-graph hydration per query; open once, query many.
4. **Query engine.**
   - is_ancestor cascade (Q6), then a segment walk with early exit, then a generation-pruned bidirectional search as a last resort.
   - Set queries (ancestors, A..B, A...B, merge-base, heads, roots) as span-set algebra.
   - Topo/limit streaming by descending ID.
   - Date-order streaming by a heap over span cursors.
   - Shortest path by ID- and generation-bounded bidirectional BFS.
   - Every API takes a `limit`/budget and returns partial results deterministically.
5. **File history.** Git-compatible changed-path Bloom (10 bits/entry, 7 hashes, 512 cap, prefix tests) as the baseline. Optionally an inverted path -> topological-ID posting index (Roaring/Elias–Fano) for sub-ms `log -- path`, intersected with ancestor span sets.
6. **What not to do.**
   - No WebGraph/k²-tree adjacency compression (outdegree ~1, so segments already win).
   - No general dynamic graph store (PCSR/Aspen/Terrace/Teseo).
   - No 2-hop/PLL/TF index: hours to build and GB-scale at large n.
   - No EWAH/Roaring per-commit reachability bitmaps over objects (the object-transfer use case is out of scope). If bitmaps are wanted, make them commit-only Roaring over topological IDs at sparse checkpoints, where run containers collapse ancestor sets to a few runs.
7. **Verification plan for perf claims** (AGENTS.md rule 8 requires traceable verification). Benchmark on linux.git (~1.3M commits) and a synthetic 10M-commit DAG with a controlled merge ratio. Measure:
   - p50/p99 is_ancestor;
   - merge-base;
   - `-n 100` topo;
   - A..B count;
   - `log -- path`;
   - open time and RSS.
   Compare against `git` with commit-graph + Bloom as the external baseline. Wire this into `pnpm run quality:perf` and `perf-report.rs`.

### Gaps
- No public benchmark directly compares IdDag segments against git commit-graph on the same repo. The merge ratios / flat-segment counts for target repos are unknown and should be measured first: they determine whether the O(merges) fallback meets sub-ms.
- Memory numbers above are arithmetic estimates, not measurements.
- A design that needs mmap introduces `unsafe` (in a dependency, at one call site), which needs an ADR per AGENTS.md. The safe alternative (buffered read) costs one copy at open.
