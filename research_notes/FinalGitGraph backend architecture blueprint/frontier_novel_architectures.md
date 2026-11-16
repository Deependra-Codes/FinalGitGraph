# Frontier and Novel Architectures for a Local-First Git-History Graph Engine (prior art, novelty gaps, recommended combination; as of Nov 2026)

Scope note: this file covers frontier and novel directions only. Labels used throughout: **Evidence** means a cited source states it. **Inference** means my reasoning from cited facts. **Speculation** means a plausible idea with no supporting source. Every number in an Inferences block is a back-of-envelope estimate, and the assumptions are stated next to it. Local-code facts cite repository paths. They come from a quick grep, not a full code review.

Grounding facts about the current system, from local code:
- The engine already reads git's commit-graph through gitoxide. It has a `CommitGraphFastPath` mode, caches `gix::commitgraph::Graph` per git dir, and has a `walk_commits_from_commit_graph` function — [adapters/git.rs L135, L404, L1196](/Users/singh.deependra-int/Desktop/git/FinalGitGraph/apps/desktop/src-tauri/src/adapters/git.rs)
- Commits are streamed in batches (`stream_commit_batches_with_hide`) and reordered topologically after loading (`reorder_commits_topologically_with_parents`) — [adapters/git.rs L747, L2708-2738](/Users/singh.deependra-int/Desktop/git/FinalGitGraph/apps/desktop/src-tauri/src/adapters/git.rs)
- The CSR graph has full-rebuild, delta and overlay paths: `rebuild_csr_snapshot`, `apply_csr_delta_from_commit_shas`, `persist_graph_overlay_commits` and `build_incremental_csr_snapshot` — [adapters/db.rs L1482, L1577, L1643, L2600](/Users/singh.deependra-int/Desktop/git/FinalGitGraph/apps/desktop/src-tauri/src/adapters/db.rs)
- Dependencies are `gix = "0.80.0"`, `gix-pack = "0.67.0"`, `lmdb = "0.8.0"` and `rayon = "1.11.0"`. `adapters/` plus `application/` total about 11.2k lines, with git.rs at 4,948 and db.rs at 3,255 — [Cargo.toml](/Users/singh.deependra-int/Desktop/git/FinalGitGraph/apps/desktop/src-tauri/Cargo.toml)

---

## Q1. For each direction: what is the closest prior art, what is genuinely new, and has anyone combined these ideas for git-history analytics?

### Takeaway
Nearly every building block already ships in production:
- git's commit-graph, changed-path Bloom filters and MIDX bitmaps
- Sapling's segmented changelog, which git-branchless and gitdag already apply to plain git repos
- jj's layered topological index
- Mononoke and Scion derived data
- Dolt's history-independent prolly trees
- WebGraph-rs and Software Heritage compression
- DBSP and Salsa incrementality
- Certificate Transparency consistency proofs

So none of the individual directions is novel. The searches found no system that combines all four of these for local git-history analytics:
- a deterministic index whose bytes do not depend on batch splits or the host OS
- a Merkle root over that index, with append-only consistency proofs
- topology bootstrapped from git's own commit-graph
- immutable per-segment path filters

Any defensible novelty sits in that combination and in the verification contract, not in any single part.

### Cited Findings

#### D1. "Zero-copy over git's own files" (commit-graph / MIDX / bitmaps as the primary index)
- The commit-graph body is split into chunks behind a lookup table, explicitly "to allow extensions that add extra data to the graph". Chunks are OIDF (256×4 B), OIDL (N×H B), CDAT (N×(H+16) B), GDA2, GDO2, EDGE, BIDX, BDAT and BASE. The file ends with an H-byte checksum of everything before it. — [gitformat-commit-graph](https://git-scm.com/docs/gitformat-commit-graph)
- Graph positions follow OID-lookup order. The spec says: "The graph position for the ith commit in this file's OID Lookup chunk is equal to i plus the number of commits in all base graphs." — [gitformat-commit-graph](https://git-scm.com/docs/gitformat-commit-graph)
- The shared chunk format is used by both the `commit-graph` and `multi-pack-index` files. The spec does not say what a reader must do with unknown chunk IDs. — [gitformat-chunk](https://git-scm.com/docs/gitformat-chunk)
- Split chains merge geometrically. Default rule: "If the new tip file would have N commits and the previous tip has M commits and X times N is greater than M, instead merge the two files" (X=2 by default). Unused graph files older than `--expire-time` are deleted. `git commit-graph verify` checks the file "against the object database". — [git-commit-graph](https://git-scm.com/docs/git-commit-graph)
- Git's own derived index has shipped correctness bugs:
  - The GDAT/GDOV chunks were "declared bankrupt" because writers had produced incorrect generation-v2 data, such as a commit's generation being lower than its parent's. They were replaced by GDA2/GDO2, which newer git reads while ignoring the old chunks — [commit-graph-format log](https://git.kilabit.info/git/log/Documentation/technical/commit-graph-format.txt?showmsg=1)
  - Changed-path Bloom v2 was added to fix a murmur3 signedness bug in v1. v1 is still the default — [commitgraph config (mirror)](https://timplab.syktsu.ru/admin/git/commits/commit/1ab5948141e62b52bcb812b04a901b3efaf1b578/Documentation/config/commitgraph.txt); [googlesource commitgraph.adoc](https://code.googlesource.com/git/+/HEAD/Documentation/config/commitgraph.adoc)
- Generation number v2 is the corrected commit date: a commit's own date combined with its parents' corrected dates — [patchwork](https://patchwork.kernel.org/comment/23539709)
- Reachability bitmaps use EWAH compression, which relies on long runs of identical bits. Pack order groups reachable objects together, while SHA order scatters them. MIDX bitmaps use a "pseudo-pack" order — [GitHub: Scaling monorepo maintenance](https://github.blog/2021-04-29-scaling-monorepo-maintenance/); [Taylor Blau on list](https://public-inbox.org/git/YlckmDHRAYnE1J5t@nand.local/); [bitmap-format](https://git-scm.com/docs/bitmap-format)
- Git 2.47 added experimental incremental MIDX chains. They did not yet support MIDX bitmaps — [Highlights from Git 2.47](https://github.blog/open-source/git/highlights-from-git-2-47/)
- `gix-commitgraph` gives read-only access to monolithic files and split `commit-graphs/graph-*.graph` chains through a `Graph` type — [gix-commitgraph lib.rs](https://docs.rs/crate/gix-commitgraph/latest/source/src/lib.rs); [crates.io](https://crates.io/api/v1/crates/gix-commitgraph)

#### D2. Topologically numbered immutable log, segments, and interval reachability
- In Sapling's `dag` crate, "Ids are topologically sorted". A segment is a range `low..=high` with parents, a level and flags. `HAS_ROOT` "must be set correct for correctness", while `ONLY_HEAD` is an optimization only. On disk, `high` is fixed-width because "vlq [u8] order does not match integer order" — [sapling-dag segment.rs](https://docs.rs/sapling-dag/latest/src/dag/segment.rs.html); [sapling-dag crate docs](https://docs.rs/sapling-dag/latest/dag/)
- Meta's own claims for Segmented Changelog:
  - the high-level graph shape downloads in "just a few megabytes"
  - the relationship between any two commits is queried "in O(number-of-merges) time"
  - "bisect the segment graph to find the history in O(log n) time"
  - but "Many of our scale features require using a Sapling-specific server"

  — [Meta Engineering, Sapling announcement (2022-11-15)](https://engineering.fb.com/2022/11/15/open-source/sapling-source-control-scalable/)
- **Segmented changelog already runs on plain git repos, locally.** git-branchless 0.10.0 depends on `esl01-dag ^0.3.0`. The `gitdag` crate "maintains segmented changelog as an index on the Git commit graph", gives a "read-only" view, and its dag is "append-only. It might include vertexes no longer referred by the git repo" — [gitdag GitDag](https://docs.rs/gitdag/latest/gitdag/struct.GitDag.html); [gitdag source](https://docs.rs/crate/gitdag/latest/source/src/gitdag.rs); [git-branchless on docs.rs](https://docs.rs/crate/git-branchless/latest)
- jj's commit index:
  - each segment file is written "in some topological order with parents first"
  - segments form a chain through `parent_file`
  - each entry stores a u32 generation number, up to two parents inline as *global* positions plus an overflow table, a change-ID pointer, and the commit ID
  - lookup is by binary search over sorted IDs
  - source TODOs include "Add pointers to ancestors further back, like a skip list" and "add a fanout table like git's commit graph has?"

  — [jj-lib readonly.rs](https://docs.rs/jj-lib/latest/src/jj_lib/default_index/readonly.rs.html)
- The reachability-index survey by Zhang, Bonifati and Özsu (2023) covers 33 techniques in frameworks such as tree cover, 2-hop labeling and approximate transitive closure. Interval labeling for DAGs assigns postorder numbers and intervals — [arXiv 2311.03542](https://arxiv.org/abs/2311.03542v2); [EBISS 2022 slides](https://cs.ulb.ac.be/conferences/ebiss2022/slides/bonifati_zhang_2.pdf)
- Software Heritage maps Merkle IDs to integers with a minimal perfect hash, compresses with Boldi-Vigna, then renumbers nodes by BFS to restore locality — [SANER 2020 slides](https://upsilon.cc/~zack/talks/2020/2020-02-19-saner-graph.pdf)
- JetBrains IDEs ship a persistent local "Git Log Indexing" feature. It powers fast log filtering, precise file history and branch filtering in file history, and a commit-to-integer map is stored on disk (`VcsLogStorage`) — [JetBrains settings: Log](https://www.jetbrains.com/help/idea/settings-version-control-log.html.md); [JetBrains support thread](https://intellij-support.jetbrains.com/hc/en-us/community/posts/5073856174226/comments/5087612220562)

#### D3. Derived data as pure, content-addressed functions + a "proof-carrying" (Merkle-verifiable) index
- Mononoke's derived data: each derived type is defined recursively, so its value for a changeset comes from that type's values on the parents. This comes from a secondary source quoting a 2022 description — [braindetox critique](https://braindetox.kr/en/posts/anthropic_scion_mononoke_derived_data_critique_2026.html)
- The Mononoke README says it is "meant to scale up to accepting thousands of commits every hour across millions of files" — [Mononoke README](https://raw.githubusercontent.com/facebook/sapling/main/eden/mononoke/README.md)
- Anthropic's Scion (announced 13 Oct 2026):
  - derived data is "computed recursively from the parent changesets, either eagerly or on demand", on the server
  - a node-local daemon shares cached objects
  - the design "sacrifices offline independence"
  - a 15 Oct update credited Mononoke and its Bonsai data model
  - Anthropic says it is not a general git replacement

  — [InfoQ, Oct 2026](https://infoq.com/news/2026/10/anthropic-source-control); [attribution critique](https://braindetox.kr/en/posts/anthropic_scion_mononoke_derived_data_critique_2026.html)
- Dolt's prolly trees are content-addressed B-trees. Their history independence means the same content gives the same tree regardless of change order. Diff "scales with the size of the differences, not the size of the tree", and chunks average about 4 KB — [Dolt prolly-tree docs](https://docs.dolthub.com/architecture/storage-engine/prolly-tree); [Dolt blog 2024](https://dolthub.com/blog/2024-03-03-prolly-trees); [Efficient diff 2020](https://dolthub.com/blog/2020-06-16-efficient-diff-on-prolly-trees)
- Certificate Transparency v2 (RFC 9162):
  - Merkle trees "can be used to show that any particular instance of the log is a superset of any particular previous instance"
  - tree shape is fixed by leaf count
  - consistency proofs prove that the first m entries are unchanged
  - comparing roots plus consistency proofs detects split views

  — [RFC 9162](https://www.rfc-editor.org/rfc/rfc9162.html)
- TerminusDB stores each commit as an immutable delta layer over succinct structures, with periodic rollups. These are vendor claims, including about 13 bytes per triple — [TerminusDB internals 2](https://terminusdb.com/?p=6387); [TerminusDB client intro](https://terminusdb-client.hexdocs.pm/introduction.html)

#### D4. Differential / incremental dataflow
- DBSP (PVLDB 16(7), 2023, best paper) gives a general incremental-view-maintenance algorithm for any DBSP program. That covers relational algebra, aggregation, and both monotonic and non-monotonic recursion, with a Rust implementation — [DBSP paper](https://www.vldb.org/pvldb/vol16/p1601-budiu.pdf); [Feldera blog](https://feldera.com/blog/Best-Research-Paper-VLDB-2023)
- Differential dataflow generalizes incremental dataflow so more prior results are reused when collections change — [Microsoft Research](https://www.microsoft.com/en-us/research/?p=189432)
- Salsa uses red-green re-execution with "backdating". It also assigns *durability* tiers so that queries depending only on high-durability inputs skip per-edge validation. rust-analyzer's example: without durability, any edit forced about 300 ms of standard-library query checks — [rust-analyzer: Durable Incrementality](https://rust-analyzer.github.io/blog/2023/07/24/durable-incrementality.html); [Salsa book](https://salsa-rs.netlify.app/print)
- LISA ("redundancy-free multi-revision analysis") analyzes only the differences between revisions. It was evaluated on 7M revisions of 4,000 projects, with orders-of-magnitude time and space reductions — [SANER 2017](https://files.ifi.uzh.ch/seal/people/alexandru/saner17-multirev.pdf); [EMSE 2019](https://files.ifi.uzh.ch/seal/people/alexandru/emse.pdf)
- A targeted search found no project combining differential dataflow or DBSP with git-history mining — [search results incl. diffhouse, pyrepositoryminer](https://deepai.org/publication/tooling-for-time-and-space-efficient-git-repository-mining)

#### D5. Columnar / vectorized analytics and SQL/PGQ or Cypher over commit graphs
- DuckPGQ (CIDR 2023) adds SQL/PGQ (SQL:2023) to DuckDB through vectorized UDFs, a compact graph layout and multi-source path-finding — [CIDR 2023](https://vldb.org/cidrdb/2023/duckpgq-efficient-property-graph-queries-in-an-analytical-rdbms.html)
- Kùzu's repository was archived in October 2025. Sources disagree on the day (10 Oct vs 11 Oct). The final release was v0.11.3. RyuGraph is a fork that continues it. An acquisition by Apple was reported in Feb 2026, but only by secondary sources — [FreshPorts](https://freshports.org/databases/kuzu); [PuppyGraph](https://www.puppygraph.com/blog/what-is-kuzudb); [ryugraph crate](https://docs.rs/crate/ryugraph/25.9.2); [MacDailyNews](https://macdailynews.com/2026/02/12/apple-acquires-graph-database-maker-kuzu/amp/)
- MergeStat Lite (formerly askgit) queries git with SQL through SQLite virtual tables that read repos at query time, with no import step, via libgit2 — [MergeStat Lite docs](https://docs.mergestat.com/mergestat-lite/); [pkg.go.dev](https://pkg.go.dev/github.com/mergestat/mergestat-lite)
- Boa (ICSE 2013) is a DSL plus cluster infrastructure over about 700k projects, with cross-user query fusion — [ICSE 2013](https://2013.icse-conferences.org/content/boa-language-and-infrastructure-analyzing-ultra-large-scale-software-repositories.html); [Dyer thesis](https://design.cs.iastate.edu/papers/thesis/DyerPhDThesis.html)
- World of Code cross-references authors, projects, commits and blobs across FLOSS — [arXiv 2010.16196](https://arxiv.org/abs/2010.16196)

#### D6. Learned indexes, succinct structures, SIMD/GPU
- PGM-index (PVLDB 13, 2020): in the static setting it matches a cache-optimized B+-tree with 83× less space. In the dynamic setting it is up to 71% faster with 1140× less space — [PGM VLDB final](https://arpi.unipi.it/retrieve/e0d6c92e-d36e-fcf8-e053-d805fe0aa794/_VLDB_Final__The_PGM_index.pdf); [PGM docs](https://pgm.di.unipi.it/docs/)
- WebGraph was rewritten in Rust (WWW'24 Companion) with substantial speedups. Its companion crates include `epserde` for zero-copy memory mapping and `sux` — [IP Paris record](https://researchportal.ip-paris.fr/en/publications/webgraph-the-next-generation-is-in-rust/); [HAL](https://hal-hprints.archives-ouvertes.fr/LTCI/hal-04494627v1)
- swh-graph is Rust, memory-mapped, and numbers nodes 0..n-1 — [lib.rs swh-graph](https://lib.rs/crates/swh-graph); [docs.rs swh-graph](https://docs.rs/swh-graph/)
- Software Heritage: about 1B commits and 5B files compressed to under 100 GB of RAM, on hardware under $300. The compression took about six days on 24 CPUs with 750 GB of RAM — [SANER 2020 paper](https://upsilon.cc/zack/research/publications/saner-2020-swh-graph.pdf); [slides](https://upsilon.cc/~zack/talks/2020/2020-02-19-saner-graph.pdf)

#### D7. Probabilistic and sketch structures
- Git's changed-path Bloom filters gave 10–30× speedups on `git log -- path` in the patch series' benchmarks, and 2–5× on the git and Linux repos in another commit. Writing them is a one-time cost — [gitgitgadget PR 497](https://github.com/gitgitgadget/git/pull/497); [git commit 78a1541](https://github.com/git/git/commit/78a1541e11b748d366841c5f0ecaf8eaa09e05f2)
- Binary fuse filters sit within 13% (3-wise) and 8% (4-wise) of the space lower bound. Measured at 10M entries on an Apple M2 at about 0.39% FPR: 9.0 bits/entry (3-wise) and 8.6 (4-wise) versus 12.0 for Bloom. They are immutable once built — [arXiv 2201.01174](https://arxiv.org/abs/2201.01174v1); [Lemire talk 2023](https://lemire.github.io/talks/2023/fastfilters/fastfilter.html)

#### D8. Verification-first architecture
- TigerBeetle's VOPR is deterministic from a seed plus the git commit, so any bug replays exactly. It injects network faults and corrupted disk reads and writes — [TigerBeetle VOPR](https://docs.tigerbeetle.com/about/vopr)
- Rust options: Turmoil (Tokio), MadSim (overrides libc calls for time and entropy, used in RisingWave), and mad-turmoil (v0.2.0, Nov 2025). An Apr 2025 discussion noted a gap: none of them yet simulated storage I/O faults — [S2 DST blog](https://s2.dev/blog/dst); [mad-turmoil](https://lib.rs/crates/mad-turmoil); [Reddit thread (mirror)](https://pol1.lr.ggtyler.dev/r/rust/comments/1jr8ogo/deterministic_simulation_testing_for_async_rust)
- git's GDAT bankruptcy and the murmur3 bug (see D1) show that derived-index corruption reached release in a mature system — [commit-graph-format log](https://git.kilabit.info/git/log/Documentation/technical/commit-graph-format.txt?showmsg=1)

#### D9. Process-model innovations (daemon, WASM, plugins)
- Scion uses a node-local daemon that shares cached objects across jobs and fetches from regional caches — [InfoQ](https://infoq.com/news/2026/10/anthropic-source-control)
- gitoxide added `wasm32-wasip2` support crate by crate (gix-path 0.11.3, gix-index 0.50.0, gix-fs 0.20.0). Earlier blockers were IO crates such as lock and tempfile. I found no statement that the full `gix` crate builds for WASI — [gix-path release](https://newreleases.io/project/github/GitoxideLabs/gitoxide/release/gix-path-v0.11.3); [PR 1545](https://github.com/GitoxideLabs/gitoxide/pull/1545); [issue 463](https://github.com/Byron/gitoxide/issues/463)

#### D10. Versioned graph DBs / Merkle-DAG systems
- Dolt (prolly trees) and TerminusDB (immutable delta layers with rollups) are covered under D3. Irmin and Merkle-CRDTs were not researched in this session (see Gaps).

### Inferences

Each direction is scored on four axes: Novelty (vs the closest prior art), Impact (scale, correctness, OS-agnosticism), Complexity, and Verdict.

#### D1. Zero-copy over git's own files — Novelty: Low. Impact: High for ingest. Complexity: Low. Verdict: ADOPT as a *bootstrap* only. Do NOT use as the primary index.
- **Core idea.** Read topology (parents, generation, commit date, root tree) straight from git's commit-graph, at memory-bandwidth speed, instead of inflating commit objects.
- **Contrarian point 1 (Evidence-based).** Commit-graph positions follow *OID order* within each layer, not topological order. BFS or topo traversal over raw positions therefore jumps randomly through memory. "Positions as primary IDs" kills locality, the very thing Software Heritage fixed with BFS renumbering. Use commit-graph for *ingest*, then renumber.
- **Contrarian point 2.** Git owns these files and deletes unused ones under `--expire-time`. A long-lived reader must not depend on their lifetime.
  - Inference, not verified in this session: on Windows, open or memory-mapped files usually block delete and replace. A long-lived mmap of `.git/objects/info/commit-graphs/*` could break or stall `git gc`/maintenance. So: parse, copy into our own layer, close. Never write inside `.git`.
- **Contrarian point 3.** Never trust git's derived chunks blindly. git itself shipped bad generation-v2 data and a non-portable Bloom hash. Validate each layer cheaply before use: parent generation < child generation, plus a CDAT parent-position bounds check. Fall back to an object walk on mismatch.
- **What's new vs the current system.** Little. The engine already walks the commit-graph (`walk_commits_from_commit_graph`). The incremental change is the *ownership boundary*: commit-graph becomes the input to a deterministic topology layer, and stops being one fast path among ~40 toggles.
- **Benefit estimate (Inference).** CDAT is H+16 = 36 bytes per commit for SHA-1, and OIDL is 20 bytes. For 10M commits that is about 560 MB. Assuming a warm page cache and ≥2 GB/s sequential read, topology loads in about 0.3 s, roughly 30M commits/s. That is far above the 100k commits/s target. It also shows the real ingest bottleneck is *metadata and tree-diff extraction*, not the graph.
- **Risk.** Many clones have no commit-graph, or a stale one (git's default write policy was not verified here). So the object-walk path stays mandatory, and both paths must produce identical bytes. That is a testable invariant (see D3/D8).
- **Feasibility.** `gix-commitgraph` is pure Rust, read-only, handles split chains, and behaves the same on all three OSes.

#### D2. Topologically numbered immutable log + segments + generation pruning — Novelty: Low. Impact: High for scale. Complexity: Low–Medium. Verdict: ADOPT, reusing the concept or crate. Do NOT claim it as novel.
- **Core idea.**
  - Assign dense u32 IDs in parents-first topological order, so the DAG becomes append-only arrays.
  - Compress linear runs into segments (Sapling IdDag).
  - Prune with generation numbers (corrected commit date).
  - Answer ancestry in O(#merges) instead of O(N).
- **Prior art covers it fully.** Sapling `dag`, plus its *git* deployments `gitdag` and git-branchless (esl01-dag), plus jj's layered parents-first index with global positions. Claiming novelty here would be refuted immediately.
- **Contrarian point 1 (Inference).** Sapling's IDs come from `IdMap::assign_head`, so they depend on the order heads were inserted. gitdag's dag is "append-only… might include vertexes no longer referred". Neither design aims for *byte-identical output independent of ingestion history*. That gap is exactly what D3 exploits.
- **Contrarian point 2 (Inference, unverified).** Segment compression depends on history linearity. Meta's monorepo is rebase-centric, while many git repos use merge-commit workflows.
  - Model: flat segments ≈ (branch points + merges + roots). With merge fraction m, that is at most about 2mN segments.
  - m = 5% at N = 10M gives about 1M segments × ~12 B (VLQ) ≈ 12 MB, compared with ≈ 84 MB for a bidirectional CSR (4(N+1) + 4E per direction, E ≈ 1.1N).
  - m = 30% gives about 6M segments, and compression mostly disappears.
  - Measure segments/N on target repos before committing to the design. Kill criterion: if segments/N > 0.3, keep CSR plus generation pruning only.
- **How this likely explains the suspected edge-loss bug (Hypothesis; unverified, needs code inspection).**
  - The engine streams batches and topo-reorders commits *after* loading them.
  - If the walk is newest-first (git's default), a child in batch k references a parent that only arrives in batch k+1.
  - Any per-batch ID assignment or CSR delta that resolves edges only within the batch then drops those edges.
  - Under a parents-first ingest contract, every parent ID exists when the child is written. A missing parent then becomes a *structural error*, or an explicit boundary/shallow marker. That removes the whole bug class instead of patching one instance.
- **Write-amplification estimate (Inference; the existing delta/overlay paths may already reduce this).**
  - Naive per-batch CSR rewrite with N = 10M and b = 100k: 100 rewrites × an average of 5M nodes × ~17 B ≈ 8.5 GB written.
  - Geometric layer merging (git's X=2 policy): each commit is rewritten about log2(N/b) ≈ 7 times, so ≈ 1.2 GB. That is roughly 7× less, with O(1) mmap startup instead of hydration.
- **Feasibility.** Pure Rust. Check the license of `sapling-dag`/`esl01-dag` before reuse (see Gaps). Otherwise reimplement the format, which is small.

#### D3. Proof-carrying deterministic index ("index Merkle root") — Novelty: **Medium–High (best candidate)**. Impact: High for correctness and OS-agnosticism. Complexity: Low–Medium. Verdict: ADOPT as the core contribution.
- **Core idea.** Define the index as a *pure function* F(snapshot of the reachable git object DAG) → canonical bytes. Commit to those bytes with a hash tree, giving two roots:
  1. **Chain root.** An RFC 9162-style Merkle tree over immutable layer roots. Consecutive versions carry consistency proofs, so an index update provably *only appended* layers.
  2. **Canonical root.** The root after deterministic compaction into one canonical layer. It depends only on the DAG, not on batch boundaries, thread count, OS or engine version (within a format version). Two machines, or the Windows and macOS CI runners, can compare a single 32-byte value.
- **Resolving the "append-only vs canonical" tension (Inference).** Append-only IDs depend on ingestion history, while history independence (Dolt's property) needs canonical order. The two roots split the duty: layers give cheap appends and proofs, and compaction re-canonicalizes. Within a layer, the canonical order is a deterministic Kahn topological sort with ties broken by (generation, OID bytes).
- **What's genuinely new (Inference from the searches):**
  - git has a trailing checksum per file and `verify` against the object database, but no cross-implementation, batch-invariant root.
  - Mononoke and Scion derive data recursively on servers, with no published root or consistency-proof contract for a local client index.
  - Dolt has history-independent roots for *tables*, not for a git-history analytics index.
  - CT has consistency proofs for *certificate logs*, not derived indexes.
  - The combination — *a local git analytics index whose correctness is checked by root equality across batch partitions and operating systems, with append-only proofs between versions* — did not appear in any source found.
- **Why it matters for this codebase.** It turns three current weaknesses into *one property test*:
  - the edge-loss bug: different batch splits lead to different canonical roots, so the bug is detected
  - weak correctness tests: root equality vs an oracle-built index
  - OS lock-in: root equality across the OS matrix
- **Cost (Inference; hash throughput not sourced).** Hash only new layers, O(batch). A full 1 GB index at an assumed ≥1 GB/s/core is about 1 s, and parallelizable.
- **Risks.**
  - (a) A root proves *determinism*, not *truth*: deterministic garbage gets a stable root. It must be anchored by a differential oracle against git (D8).
  - (b) Hidden nondeterminism: HashMap iteration order, rayon reduction order, floating point, timestamps, timezone, and the OS path encoding. Store git's raw path bytes, never OS paths or case-folded paths.
  - (c) bincode is not a written, versioned canonical spec (Inference). The canonical layer bytes need an explicit big-endian chunk layout, like git's.
  - (d) Format evolution: version the root domain, `H(format_version ‖ …)`.
- **Feasibility.** Pure Rust. Hashing (`sha2`/`blake3`) and RFC 9162 implementations exist (e.g., `malt`, `ct-merkle` per search results; unvetted). Behavior is the same on all OSes once paths are bytes.

#### D4. Incremental dataflow (DBSP / differential dataflow / Salsa) — Novelty: Medium for the mapping. Impact: Medium. Complexity: Medium (High if full DBSP). Verdict: ADOPT THE IDEA, NOT THE ENGINE; phase 2.
- **Contrarian point (Inference).** Commit history is *insert-only and immutable*: existing commits never gain parents. Most derived data (topology, per-commit changes, per-path last-touch, blame-like folds) is a *fold over parents*. It is already trivially incremental, as Mononoke and Scion show, so a general IVM engine adds a large dependency for little benefit there.
- Non-monotonic change exists only in the **ref layer**: force-pushes, branch deletion, ref moves, and views like "reachable from main but not release" or "per-branch contributor stats". Model only that layer as Z-sets (±1 weights, DBSP's algebra) over the immutable base.
- **Salsa durability mapping (Speculation, not found in the literature).** Immutable commit layers get *maximal durability* and are never revalidated. Refs and the working tree get *low durability*. UI queries depending only on immutable layers validate in O(1), the same mechanism that saved rust-analyzer about 300 ms per edit.
- **Risk.** Embedding Feldera or DBSP brings compile-time weight and API churn into a Tauri binary.

#### D5. Columnar analytics + SQL/PGQ/Cypher — Novelty: Low. Impact: Medium (ecosystem). Complexity: Low if export-only. Verdict: "Arrow-out, not DB-in".
- Kùzu's archival in Oct 2025 is the cautionary tale: do not put a third-party graph DB in the core.
- Recursive path queries through generic SQL/PGQ joins will not beat segment- and generation-based ancestry (Inference).
- MergeStat Lite already proves "SQL over git without import". Expose the index as Arrow record batches through the C Data Interface, so DuckDB, DataFusion or Polars can be *optional* consumers. That needs zero core coupling.

#### D6. Learned indexes / succinct / SIMD / GPU — Verdict: REJECT learned indexes and GPU. DEFER succinct compression, except for the change relation.
- **Learned indexes (Inference).** SHA-1/SHA-256 OIDs are uniformly distributed, so the key CDF is already linear. A learned model reduces to interpolation search, about 1–2 probes on average, versus about 16 after a 256-entry fanout. That saves well under a microsecond per lookup. PGM's wins come from skewed key distributions and enormous key sets, so it gives no measurable gain at 10M keys. The time index is a sorted (corrected date, ID) array with binary search, also not a bottleneck.
- **GPU (Inference).** About 10M nodes and 11M edges, with topological IDs, makes CPU traversal near-streaming. That is estimated at tens of ms, and PCIe transfer plus launch overhead eats the gain. CUDA is unavailable on macOS, which breaks OS-agnosticism.
- **Where compression does pay (Inference).** The *commit × path change relation*, not the commit DAG. At 10M commits × 5–20 paths that is 50–200M edges: 0.4–1.6 GB as raw u32 pairs. A WebGraph-style or Elias-Fano encoding, or a segment-local path dictionary, is justified there. webgraph-rs and epserde are Rust-native and can be mmapped.

#### D7. Sketches — Novelty: Medium for the combination. Impact: Medium. Complexity: Low. Verdict: ADOPT per-segment, per-layer immutable binary fuse filters. REJECT HLL. DEFER MinHash.
- **Idea (Inference).** Build one binary fuse filter per *segment* (D2) over the union of paths changed in that segment. `log -- path` then skips whole segments, confirming hits against the exact change relation.
- **Why fuse rather than Bloom.** Fuse filters are immutable, which matches immutable layers exactly. They cost about 9 vs 12 bits per key at a similar FPR.
- **Rough cost (Inference).** About 1M segment probes × ~20 ns ≈ 20 ms, versus about 10M per-commit probes ≈ 200 ms. This is hardware-dependent.
- **Prior art.** Sapling already claims O(log n) history through segment bisection, and git has per-commit filters. The new part is the immutable-fuse + segment + exact-confirmation pairing, which is modest novelty.
- **HLL.** Exact distinct counts via author-ID bitmaps are cheap locally (authors are about 10³–10⁵). Approximation buys nothing.
- **MinHash.** Rename/similarity detection is niche. Defer.

#### D8. Verification-first — Novelty: Low individually; the *metamorphic batch-partition oracle bound to a Merkle root* is uncommon. Impact: High. Complexity: Low–Medium. Verdict: ADOPT. It is the proof half of D3.
- **Differential oracle.** On generated DAGs (property-based: random merges, octopus merges, multiple roots, clock skew, shallow boundaries) and on real repos, compare the engine against `git rev-list --topo-order`, `git merge-base --all` and `git log --name-status -m`. The comparison runs over canonical outputs.
- **Metamorphic invariants.**
  - Any batch partition, thread count or ingest path (commit-graph vs object walk) gives the same canonical root.
  - Compaction preserves the root.
  - Consistency proofs verify across every append.
- **DST-lite.** The engine is single-process and local. Simulating crash points around layer-file write → fsync → manifest rename gives most of the value without a simulated runtime. Note the storage-I/O simulation gap in the Rust DST tools.

#### D9. Process model — Verdict: DEFER the daemon and WASM core. ADOPT "the file format is the API".
- **Daemon (Inference).** A daemon adds IPC, lifecycle and version-skew problems, plus local-socket security that differs between Windows named pipes and Unix sockets.
- Immutable layer files with one writer, atomic manifest swaps and generation-numbered filenames (never replaced in place, which is Windows-safe) let *many processes* (UI, CLI, editor plugins) mmap the same index concurrently with no daemon. This is git's own commit-graph-chain pattern.
- **WASM.** WASI lacks mmap, and gix only partly targets WASI. A WASM build of the *pure algorithm kernel*, working on byte slices, is useful as a cross-OS determinism test fixture, not as the product runtime.

#### D10. Versioned DBs / prolly trees / Merkle-CRDTs — Verdict: REJECT as a storage substrate.
- History is append-only, so prolly-tree structural sharing for mutable maps is unnecessary.
- git trees are already Merkle trees for per-commit state.
- TerminusDB's delta layers plus rollups are the same pattern as git's split chains with geometric merging. Reuse the pattern, not the system.

### Gaps
- Not researched in this session: Irmin, Merkle-CRDTs, Nix content-addressed derivations, and Bazel remote cache semantics.
- Mononoke derived-data descriptions came from a secondary source. Primary docs were not retrieved.
- No published segment-count or compression numbers for merge-heavy *git* repos (e.g., Linux) under segmented changelog. Measure before committing.
- Windows behavior for mmapped or open files during `git gc` is stated from general knowledge, not a source. Verify on a Windows runner.
- git's default policy for writing commit-graph files (`gc`/`fetch`/`maintenance`) was not verified, so how often the zero-copy bootstrap is available is unknown.
- Hash throughput figures (SHA-256 with SHA-NI, BLAKE3) are assumed, not sourced.
- Whether the engine's existing `apply_csr_delta`/overlay paths already avoid the naive O(N²/b) rewrite. The write-amplification estimate is an upper bound.
- The edge-loss root cause (newest-first walk plus per-batch resolution) is a hypothesis drawn from function names only.
- Swh-graph current size and bytes-per-edge figures could not be fetched: the docs page blocked automated access.
- DuckPGQ's 2025–26 maintenance status was not checked. Kùzu's status relies partly on secondary sources, which disagree on the archive date.

---

## Q2. Which combination gives the biggest leap in scalability, correctness and OS-agnosticism with the least complexity?

### Takeaway
Combine four things into one coherent design, called here a **Proof-Carrying Topological Log (PCTL)**:
1. **D2** canonical parents-first topological layers with segments and generation pruning
2. **D3** Merkle chain and canonical roots with RFC 9162-style consistency proofs
3. **D1** commit-graph-bootstrapped topology ingest, read-only and never writing to `.git`
4. **D7** immutable per-segment fuse filters over an exact compressed change relation

**D8** (differential plus metamorphic oracles) is the mandatory test harness. D4 (ref-layer Z-sets plus Salsa durability) and D5 (Arrow export) are phase 2. Reject learned indexes, GPU, HLL, embedded graph DBs, full DBSP/Feldera, prolly trees, and a daemon for v1.

### Cited Findings
- Geometric layering with X=2 and immutable chained layers is proven in production: git split commit-graph chains, git 2.47 incremental MIDX, jj's segment chains, and TerminusDB delta layers — [git-commit-graph](https://git-scm.com/docs/git-commit-graph); [Git 2.47 highlights](https://github.blog/open-source/git/highlights-from-git-2-47/); [jj-lib readonly.rs](https://docs.rs/jj-lib/latest/src/jj_lib/default_index/readonly.rs.html); [TerminusDB internals](https://terminusdb.com/?p=6387)
- Segment ancestry costs O(#merges), and history uses O(log n) segment bisection — [Meta Engineering](https://engineering.fb.com/2022/11/15/open-source/sapling-source-control-scalable/)
- Append-only Merkle logs with minimal consistency proofs are standardized — [RFC 9162](https://www.rfc-editor.org/rfc/rfc9162.html)
- History independence (same content gives the same root) is what makes cross-replica root comparison meaningful — [Dolt 2024](https://dolthub.com/blog/2024-03-03-prolly-trees)
- Mature derived indexes ship corruption bugs (git GDAT and murmur3), which argues for verification-first — [commit-graph-format log](https://git.kilabit.info/git/log/Documentation/technical/commit-graph-format.txt?showmsg=1); [commitgraph config](https://code.googlesource.com/git/+/HEAD/Documentation/config/commitgraph.adoc)
- Binary fuse filters: about 9 bits/key vs 12 for Bloom at a similar FPR, and immutable — [Lemire 2023](https://lemire.github.io/talks/2023/fastfilters/fastfilter.html)
- Third-party embedded graph-DB risk: Kùzu was archived in Oct 2025 — [FreshPorts](https://freshports.org/databases/kuzu)

### Inferences

**Ranked gap analysis.** Score = impact for this codebase ÷ complexity, adjusted by how defensible the novelty is.

| Rank | Direction | Novelty vs prior art | Impact | Complexity | Decision |
|---|---|---|---|---|---|
| 1 | D3+D8 proof-carrying deterministic index + oracles | Medium–High (combination not found) | Correctness ++, OS-agnostic ++ | Low–Med | Core v1 |
| 2 | D2 canonical topo layers + segments + generation | Low (Sapling/gitdag/jj) | Scale ++, kills edge-loss class | Low–Med | Core v1 |
| 3 | D1 commit-graph bootstrap (read-only, copy-then-close) | Low (git, gix; partly in engine) | Ingest ++ | Low | Core v1 |
| 4 | D7 per-segment fuse filters + exact change relation | Medium (pairing) | Path queries + | Low–Med | v1.5 |
| 5 | D4 ref-layer Z-sets + Salsa durability | Medium (mapping) | UI latency + | Med | v2 |
| 6 | D5 Arrow export for DuckDB/DataFusion | Low | Ecosystem + | Low | v2 |
| — | D6 learned index / GPU; HLL; Kùzu-style DB; full DBSP; prolly trees; daemon | — | ≈0 or negative at 10M | Med–High | Reject/defer |

**PCTL in one paragraph (Inference/design).** A repo's index is a chain of immutable layer files in a git-chunk-style big-endian format, stored in the app's data directory (never in `.git`). Each layer holds:
- the new commits in canonical parents-first order
- a sorted OID table plus a permutation
- parent arrays as *global* IDs pointing only downward into earlier layers
- a generation (corrected date) column
- flat and high-level segments
- per-segment binary fuse filters of changed paths
- the compressed exact change relation
- a BASE list of prior layer roots

A manifest names the chain and is swapped by atomic rename, with generation-numbered files so nothing is replaced in place (Windows-safe). Layers merge geometrically (X=2), and a full compaction re-canonicalizes and yields the **canonical root**. Topology comes from the commit-graph when present and verified, otherwise from an object walk. Both paths must yield the same root. Readers mmap layers, and the UI, CLI and tools share files, not a daemon.

**What it deletes from today's design (Inference):**
- most of the 17 LMDB tables (keep a small KV only for UI and app state)
- bincode on the hot path
- the per-batch CSR rewrite and hydration step: mmap gives O(1) startup
- most of the ~40 env toggles: format and pipeline choices become fixed, versioned constants, because determinism forbids hidden mode switches

**Expected leap, rough numbers (Inference; assumptions above):**
- Topology ingest: about 30M commits/s from a warm commit-graph.
- The ≥100k commits/s goal becomes bounded by tree-diff extraction, which stays async but now writes into immutable layers.
- Write amplification: about 7× lower than naive per-batch rewrites at N = 10M.
- Ancestry: O(#merges) via segments, with generation pruning as a fallback.
- Hot memory at 10M commits: about 100 MB (parents + generation + segments), plus 200 MB of OIDs mmapped lazily.
- The change relation is the only large structure, and it is compressed and mmapped.

**Correctness leap (Inference).** One CI job runs the same corpus on Windows, macOS and Linux. It asserts:
1. canonical roots are identical across OSes, batch sizes, thread counts and ingest paths
2. differential equality with the git CLI on topo order, merge-base and name-status
3. consistency proofs verify across every append

Any edge lost across batches changes the canonical root, so that whole bug class becomes *detectable by construction*.

**Kill criteria and falsification:**
- (a) If segments/N > 0.3 on target repos, drop segments and keep CSR-in-layers plus generation pruning.
- (b) If canonical compaction exceeds the UI latency budget at 10M commits, keep chain roots for proofs and compute canonical roots only in CI and background jobs.
- (c) If commit-graph bootstrap and object walk ever disagree on a verified repo, that is an engine bug, not a tolerance to configure.

**Migration path (Inference).** Build PCTL beside the LMDB path, behind one read-only query trait. Let the oracle harness gate the switch, then delete the CSR rewrite. This keeps to the repo's layering rules: layer I/O in `adapters`, layer and segment types in `domain`, the oracle harness in tests. Per AGENTS.md, it needs an ADR for the storage boundary change.

### Gaps
- No benchmark exists yet for canonical compaction cost at 10M commits. It must be measured.
- The license of `sapling-dag`/`esl01-dag` was not verified. The Sapling repo licensing was not checked, so reimplementation may be required.
- Hidden nondeterminism in gix itself (e.g., tree-diff rename detection order) was not audited. Rename detection may need canonicalization or exclusion from the root.
- No data was found on JetBrains' local VCS log index internals, which is the closest desktop competitor for the path and user filters.

---

## Q3. What would a credible novelty claim look like, and how do you substantiate it?

### Takeaway
Do not claim "an architecture no one has thought of". The parts are all prior art, and the Oct 2026 Scion/Mononoke attribution dispute shows how fast uncredited overlap is called out. A defensible claim is narrow, falsifiable and comes with explicit prior-art credit: *a local-first git-history index whose bytes are a deterministic, batch- and OS-independent function of the commit DAG, committed by a Merkle root with append-only consistency proofs, bootstrapped zero-copy from git's own commit-graph, and validated by root-equality metamorphic and differential oracles*. Substantiate it with a documented, dated prior-art search log plus a reproducible artifact.

### Cited Findings
- Scion's design was publicly criticized for overlapping with Mononoke's derived data and Bonsai model without credit. Anthropic added explicit Mononoke credit in a 15 Oct 2026 update — [InfoQ](https://infoq.com/news/2026/10/anthropic-source-control); [critique](https://braindetox.kr/en/posts/anthropic_scion_mononoke_derived_data_critique_2026.html)
- Prior art to credit explicitly (each refutes a broader claim):
  - segments and topological IDs — [Sapling dag](https://docs.rs/sapling-dag/latest/dag/)
  - segmented changelog on git — [gitdag](https://docs.rs/gitdag/latest/gitdag/struct.GitDag.html)
  - layered parents-first index — [jj-lib](https://docs.rs/jj-lib/latest/src/jj_lib/default_index/readonly.rs.html)
  - chunked extensible index, split chains, Bloom filters — [git commit-graph](https://git-scm.com/docs/gitformat-commit-graph)
  - recursive derived data — [Scion/Mononoke](https://infoq.com/news/2026/10/anthropic-source-control)
  - history-independent Merkle roots — [Dolt](https://dolthub.com/blog/2024-03-03-prolly-trees)
  - consistency proofs — [RFC 9162](https://www.rfc-editor.org/rfc/rfc9162.html)
  - compressed history graphs — [Software Heritage](https://upsilon.cc/zack/research/publications/saner-2020-swh-graph.pdf); [WebGraph-rs](https://researchportal.ip-paris.fr/en/publications/webgraph-the-next-generation-is-in-rust/)
  - SQL over git without import — [MergeStat](https://docs.mergestat.com/mergestat-lite/)
  - multi-revision redundancy elimination — [LISA](https://files.ifi.uzh.ch/seal/people/alexandru/emse.pdf)
  - local IDE git-log index — [JetBrains](https://www.jetbrains.com/help/idea/settings-version-control-log.html.md)
  - IVM — [DBSP](https://www.vldb.org/pvldb/vol16/p1601-budiu.pdf)
  - durability — [Salsa](https://rust-analyzer.github.io/blog/2023/07/24/durable-incrementality.html)
  - deterministic simulation — [VOPR](https://docs.tigerbeetle.com/about/vopr)
  - reachability indexes — [Zhang/Bonifati/Özsu](https://arxiv.org/abs/2311.03542v2)
- This session's searches found no system combining differential dataflow with git mining — [search result set](https://deepai.org/publication/tooling-for-time-and-space-efficient-git-repository-mining). None of the retrieved sources describes a Merkle-rooted, batch-invariant derived index for git analytics. That is negative evidence from about 45 queries and fetches, not proof of absence.

### Inferences
**Claim template (for an ADR, README or paper):** "To our knowledge, PCTL is the first local git-history index with the following four properties, building on [Sapling segmented changelog; gitdag; jj index; git commit-graph; Mononoke/Scion derived data; Dolt; RFC 9162]."
1. Its on-disk bytes are a pure function of the reachable commit DAG and format version, invariant to ingestion batching, parallelism, ingest path and host OS.
2. That function is committed by a canonical Merkle root, plus a chain root with append-only consistency proofs between index versions.
3. It is bootstrapped read-only from git's commit-graph and cross-checked against an object walk.
4. Its correctness is enforced by root-equality metamorphic tests and differential oracles against the git CLI.

**What not to claim:**
- topological numbering
- segments
- generation numbers
- layered or immutable indexes
- Bloom or fuse path filters
- derived data
- compressed graphs
- SQL over git
- incremental computation
- "zero-copy over git files" in general

**Substantiation protocol (Inference/plan):**
1. **Dated search log** (query, venue, date, hits, verdict), kept in `docs/adr` or the research notes.
2. **Venues to search:**
   - DB/systems: VLDB/PVLDB, SIGMOD, CIDR, ICDE, EuroSys, OSDI/SOSP, FAST, USENIX ATC
   - SE: MSR, ICSE, FSE/ESEC, SANER, ICSME, EMSE journal
   - Security/transparency: verifiable logs, ADS (authenticated data structures)
   - arXiv cs.DB, cs.SE, cs.DS, cs.CR from 2018 to 2026
3. **Search terms:**
   - "verifiable index" git
   - "authenticated data structure" "commit graph"
   - "Merkle root" "derived data" repository
   - "deterministic index" "version control"
   - "history-independent" index git
   - "consistency proof" index repository
   - "reproducible index" "commit-graph"
   - "segmented changelog" git
   - "metamorphic testing" "git"
   - "batch-invariant" ingestion
   - "commit-graph" "merkle"
   - "proof-carrying data" "version control"
4. **Code and community search:**
   - GitHub code search (`commit-graph` + `merkle`, `IdDag` + `root`)
   - crates.io and lib.rs
   - the git list (lore.kernel.org/git) on commit-graph verification and checksum semantics
   - issue trackers of Sapling, jj, gitoxide, git-branchless and Software Heritage
5. **Patents:** Google Patents for "commit graph" + "hash tree" + "index verification".
6. **Expert check (optional):** short questions to maintainers of the closest systems — the Sapling dag author (quark-zju, also the gitdag author), jj, gitoxide, and the git commit-graph and bitmap authors.
7. **Reproducible artifact:**
   - an open format spec
   - a corpus covering Linux-scale, merge-heavy, linear-monorepo-like and synthetic adversarial DAGs
   - published canonical roots that third parties can recompute on any OS
   - benchmarks against `git` with commit-graph, jj and `gitdag` on ancestry, merge-base, topo-order and `log -- path`

**Credit framing (Inference).** Frame the contribution as "a verification contract plus a composition", with prior art cited in the first paragraph. That is both more honest and harder to refute than "novel architecture".

### Gaps
- Not searched: ACM DL and IEEE Xplore full text, patents, non-English literature, and proprietary desktop git clients (GitKraken, Fork, Sublime Merge, Tower) whose index internals are undocumented.
- JetBrains' git log index storage format and its verification approach (if any) are unknown.
- Only secondary sources were seen for Scion's internals. The primary Anthropic post was not opened.
- Whether any git-list proposal (lore.kernel.org) ever specified a reproducible cross-implementation commit-graph hash was not checked.
- Homomorphic or multiset hashes (e.g., lattice-based incremental hashing), an alternative order-independent digest that would avoid canonical compaction, were not researched. They should be evaluated before finalizing D3.
