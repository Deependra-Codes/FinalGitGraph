# Incremental Derived-Data Computation, Concurrency, and Crash Consistency for a Local-First Git-History Indexer

Scope: incremental-update model, data lifecycle (segments/manifests), crash consistency, concurrency for a desktop backend, configuration explosion, process architecture. Research date: 2026-11-01. Evidence is in "Cited Findings". Design recommendations are in "Inferences" and are labeled as inference. The repository facts cited below come from reading `apps/desktop/src-tauri/src/adapters/db.rs` in this repo on this date.

---

## Q1. Which incremental computation model fits "derived data as a pure function of an immutable commit DAG"?

### Takeaway
Mononoke's "derived data" model fits best: per-commit facts are computed once from the commit and its parents' derived values, stored under the commit id, versioned by type, and always regenerable. Build-systems theory explains why: a git commit id already is a deep content hash of all its history. DBSP's Z-set delta algebra is the right tool for aggregate views over the commit set. Salsa/Adapton-style dependency tracking only fits the small, mutable, ref-dependent layer (branch tips, query results).

### Cited Findings

**Mononoke derived data (Meta, Sapling server, Rust)**
- Definition: derived data is "information computed from Bonsai changesets and file contents that can always be regenerated from the core data". Bonsai changesets and file content blobs "constitute the source of truth for repository history." — [Mononoke docs 2.3 Derived Data](https://github.com/facebook/sapling/blob/main/eden/mononoke/docs/2.3-derived-data.md)
- It is kept off the write path: "Derived data computation is not on this critical path. The next commit can proceed immediately". It "can be computed asynchronously after a commit has been accepted", which "allows Mononoke to maintain high commit throughput." — [Mononoke docs 2.3](https://github.com/facebook/sapling/blob/main/eden/mononoke/docs/2.3-derived-data.md)
- Derivation: `derive_single` computes data "for one changeset" using "the derived data for its parents". "When deriving a changeset, all dependencies are derived first". `derive_batch` "allows implementations to optimize derivation across multiple changesets". — [Mononoke docs 2.3](https://github.com/facebook/sapling/blob/main/eden/mononoke/docs/2.3-derived-data.md)
- Storage: "Derived data is stored in the blobstore using content-addressed keys", for example `derived_root_fsnode.<ChangesetId>`. "Mappings from changeset IDs to derived data are also stored in the metadata database for some types." — [Mononoke docs 2.3](https://github.com/facebook/sapling/blob/main/eden/mononoke/docs/2.3-derived-data.md)
- Versioning: "New versions are implemented as separate derived data types (e.g., Blame → Blame V2)". "Configuration controls which version is active per repository". Types can be "rederived using a mapping key prefix". — [Mononoke docs 2.3](https://github.com/facebook/sapling/blob/main/eden/mononoke/docs/2.3-derived-data.md)
- Backfill: sequential mode derives "in topological order from repository roots to heads". The parallel mode will "Divide the commit graph into slices, derive data for slice boundaries using predecessor dependencies". The remote derivation service gives "Deduplication of derivation work (multiple requests for the same changeset coalesce)". — [Mononoke docs 2.3](https://github.com/facebook/sapling/blob/main/eden/mononoke/docs/2.3-derived-data.md)
- Real derived types in the repo include blame, fastlog, fsnodes, unodes, changeset_info, skeleton_manifest(_v2), deleted_manifest, history_manifest, inferred_copy_from, plus bulk_derivation and derivation_pipeline_driver crates. — [sapling/eden/mononoke/derived_data tree](https://github.com/facebook/sapling/tree/main/eden/mononoke/derived_data)

**Git generation numbers (an example of a parent-derived pure function)**
- Corrected committer date (generation v2): a root commit uses its committer date. Any other commit uses the max of its own date and (1 + the max parent value). Topological level (v1) is 1 + max parent level. Property: "N <= M, then A cannot reach B". Chains use strict inequality because of uncomputed values. — [git commit-graph technical doc](https://github.com/git/git/blob/master/Documentation/technical/commit-graph.adoc)

**Build Systems à la Carte (Mokhov, Mitchell, Peyton Jones; ICFP 2018, PACMPL 2 Article 79)**
- "Definition 2.1 (Minimality). A build system is minimal if it executes tasks at most once per build and only if they transitively depend on inputs that changed since the previous build." — [Build Systems à la Carte PDF](https://www.microsoft.com/en-us/research/uploads/prod/2018/03/build-systems-a-la-carte.pdf)
- Verifying traces record hashes of the dependencies used last time. "A constructive trace additionally stores the resulting value", which enables shared cloud caches. Deep constructive traces (Buck, Nix) hash only terminal inputs. "all traces except for deep traces (§4.2.4) support early cutoff." — [Build Systems à la Carte PDF](https://www.microsoft.com/en-us/research/uploads/prod/2018/03/build-systems-a-la-carte.pdf)
- The design space is 3 schedulers (topological, restarting, suspending) × 4 rebuilders (dirty bit, verifying traces, constructive traces, deep constructive traces). That gives "12 possible build systems, 8 of which are inhabited by existing build systems". Table 1 rows: Make is topological/static/minimal/no cutoff. Excel is restarting/dynamic/not minimal. Shake is suspending/dynamic/minimal/cutoff. Bazel is restarting/dynamic*/not minimal/cutoff/cloud. — [Build Systems à la Carte PDF](https://www.microsoft.com/en-us/research/uploads/prod/2018/03/build-systems-a-la-carte.pdf). A revised version appeared in JFP vol 30 (E11). — [JFP version](https://microsoft.com/en-us/research/uploads/prod/2020/04/build-systems-jfp.pdf)
- Real systems often store value hashes in the trace and keep "a separate content-addressable cache which associates hashes with their actual contents." — [Build Systems à la Carte PDF](https://www.microsoft.com/en-us/research/uploads/prod/2018/03/build-systems-a-la-carte.pdf)

**DBSP / Feldera (VLDB 2023 Best Paper)**
- PVLDB 16(7): 1601–1614, 2023. It won the VLDB 2023 best paper award and the 2024 ACM SIGMOD Research Highlights. The VLDB author list is Budiu, Chajed, McSherry, Ryzhyk, Tannen. The arXiv v1 listing omits Chajed. — [VLDB PDF](https://www.vldb.org/pvldb/vol16/p1601-budiu.pdf); [Feldera award post](https://www.feldera.com/blog/Best-Research-Paper-VLDB-2023); [arXiv 2203.16684](https://arxiv.org/abs/2203.16684); [SIGMOD highlights](https://feldera.com/blog/sigmod-research-highlights)
- Core definition: the incremental version of a query is Q^Δ = D ∘ ↑Q ∘ I, where I is stream integration and D is differentiation. "The incremental version of a query is a stateful streaming operator which computes directly on changes and produces changes." — [VLDB PDF](https://www.vldb.org/pvldb/vol16/p1601-budiu.pdf)
- Chain rule: "to incrementalize a composite query you can incrementalize each sub-query independently". Theorem 3.3: an LTI (linear) operator equals its own incremental version. Theorem 3.4: a bilinear operator (join) has (a×b)^Δ = a×b + z⁻¹(I(a))×b + a×z⁻¹(I(b)). — [VLDB PDF](https://www.vldb.org/pvldb/vol16/p1601-budiu.pdf)
- Z-sets (weighted multisets, also called z-relations) turn set operations into abelian-group operations, so inserts and deletes are uniform. Incremental `distinct` does "work proportional to the size of the input change". Primitive operations are faster than non-incremental ones "by a factor of O(|DB|/|ΔDB|)". — [VLDB PDF](https://www.vldb.org/pvldb/vol16/p1601-budiu.pdf)
- Recursion (monotonic and non-monotonic, for example transitive closure) is supported via nested streams. A Rust library and runtime are MIT-licensed at github.com/vmware/database-stream-processor. — [VLDB PDF](https://www.vldb.org/pvldb/vol16/p1601-budiu.pdf)

**Differential dataflow (CIDR 2013, McSherry, Murray, Isaacs, Isard)**
- It extends incremental computation to "arbitrarily nested iteration", is implemented in the Naiad prototype, and makes incrementally-updated strongly connected components tractable. — [MSR publication page](https://www.microsoft.com/en-us/research/publication/differential-dataflow/)

**Salsa / rust-analyzer**
- In Salsa, "Everything else in your program is ultimately a deterministic function of these inputs". Tracked functions are memoized. "If you call a tracked function twice, Salsa checks if the inputs have changed; if not, it can return the memoized value." It uses the red-green algorithm. — [Salsa overview](https://salsa-rs.github.io/salsa/overview.html)
- Durability: derived queries take "the minimum durability among the immediate inputs". The version becomes "a version vector", so durable subgraphs skip validation. Before durability, validating standard-library queries after one edit cost "about 300ms". Early cutoff means "a query whose result is unchanged despite a changed input". — [rust-analyzer blog: Durable Incrementality (2023)](https://rust-analyzer.github.io/blog/2023/07/24/durable-incrementality.html)
- rust-analyzer "keeps all this input data in memory and never does any IO". The model is "computed lazily (on-demand)". — [rust-analyzer architecture](https://rust-analyzer.github.io/book/contributing/architecture.html)

**Adapton (PLDI 2014) and Buck2 DICE**
- Adapton is demand-driven: programs "recompute only what observers demand", using a "demanded computation graph". — [Adapton PLDI 2014](https://people.mpi-sws.org/~hammer/adapton-pldi2014.pdf); [bib record](https://bibtex.github.io/PLDI-2014-HammerKHF.html)
- Buck2's DICE is "inspired by Adapton and Salsa". Computations run in parallel on tokio executors, and duplicate requests are deduplicated. It provides dependency tracking, invalidation, early cutoff, and concurrent requests. — [Buck2 Modern DICE](https://buck2.build/docs/insights_and_knowledge/modern_dice/). Meta says the framework was heavily inspired by "Adapton... Salsa, Skip and the Shake build system". — [Meta: Future of Buck (2021)](https://developers.facebook.com/blog/post/2021/07/01/future-of-buck/)

**Sapling segmented changelog (`sapling-dag` crate)**
- IdDag stores a DAG of integers with ancestry indexes and is paired with IdMap (hash ↔ id). `assign_head` is incremental, and assigning an id to a head also assigns ids to its ancestors. Segments are built on demand and stored via an IdDagStore trait (IndexedLog backend or in-memory). An integrity module checks for problems such as overlapping segments. — [sapling-dag iddag.rs](https://docs.rs/sapling-dag/0.1.0/src/dag/iddag.rs.html); [sapling-dag idmap.rs](https://docs.rs/crate/sapling-dag/latest/source/src/idmap.rs)

### Inferences
- **The commit id is a free "deep constructive trace" key (inference).** A git commit sha hashes its tree and its parent shas, so it hashes its entire history. Any function f(history up to C) can therefore be cached under key `(C, f_version)` and never invalidated. In Build-Systems-à-la-Carte terms this is a deep constructive trace. Its known weakness, no early cutoff, does not matter: commit inputs never change, so nothing can be cut off.
- **Recommend a three-tier incremental model (inference):**
  1. **Tier A: immutable per-commit facts (Mononoke model).** Examples: parents as dense ids, generation number v2, changed paths, a per-commit changed-path Bloom filter, author id. Each is a pure function of `(commit, parents' tier-A facts)`. Derive in parent-before-child order, store under the dense commit id, and version each type (e.g. `file_changes@v2`). There is no invalidation, only "is it derived yet?".
  2. **Tier B: aggregate views over the commit set (DBSP algebra).** Examples: per-author counts, per-path churn, monthly histograms. Input is a Z-set delta of commits (+1 for newly reachable commits, −1 when a force-push or GC makes commits unreachable). Linear operators (filter, map, count, sum) apply directly to deltas (Theorem 3.3). Joins use the bilinear formula. Borrow the algebra; do not embed Feldera, whose SQL/compiler/runtime stack is too heavy for a desktop library. The `dbsp` Rust crate is a candidate to evaluate later.
  3. **Tier C: ref-dependent and query-dependent results (Salsa/verifying-trace model).** Examples: "commits reachable from main but not feature", graph layout for a viewport. Memoize in memory keyed by `(snapshot_version, ref_tips_hash, query)`. This is the only tier that needs invalidation, and it is small.
- **Replace the time-based checkpoint (inference).** Today the system checkpoints on max committer time plus sha with a 300 s lookback. Committer time is not a sound frontier. Git itself needed "corrected commit dates" because raw dates are unreliable (skew), so a 300 s window is a heuristic that can miss commits. The sound boundary is set difference on the DAG: new = reachable(current ref tips) − indexed. Walk from the tips and stop at any commit already in the index (O(1) membership via sha→id map). Store the indexed ref-tip set in the manifest. This is git's `rev-list NEW --not OLD` semantics and needs no clock assumption. Generation numbers can prune the walk further.
- **File-change extraction becomes a tier-A derived type (inference).** The retry queue and the delete/extract/persist triple disappear. The pending work is `{indexed commits} − {commits with file_changes@vN}`, recomputed on startup. Re-derivation is idempotent because the output is a pure function of the commit, so a crash at any point is harmless.
- Salsa/Adapton/DICE dependency tracking costs memory per edge. For 10M immutable commits that overhead buys nothing in tiers A and B (inference, from Salsa's design point: in-memory, mutable inputs).

### Gaps
- I did not retrieve DBSP/Feldera benchmark numbers or a list of the `dbsp` crate's current maturity and features.
- Mononoke's docs do not state idempotency/determinism requirements explicitly. Lease-based deduplication was not found in the doc, only coalescing in the remote derivation service.
- Materialize internals were not researched beyond its lineage (McSherry, differential dataflow).
- I found no published measurement of Salsa memory overhead per tracked edge.

---

## Q2. Append-only immutable segments + manifest: how do Tantivy and git commit-graph chains do it, and how do readers get consistent versions without locks?

### Takeaway
git's split commit-graph chain is almost exactly the structure this indexer needs. It has an immutable base plus incremental layers, global positions as cumulative offsets, a geometric merge rule (size multiple 2, max commits), lockfile+rename to publish the chain file, and delayed deletion so concurrent readers never see missing files. Tantivy shows the same pattern for search: immutable segments, an atomically updated `meta.json`, and Searchers that hold a snapshot list of segment readers. In Rust, an `ArcSwap<Snapshot>` gives wait-free-ish reader access. left-right does not fit a large graph (2× memory, writer waits for readers).

### Cited Findings

**git commit-graph chains**
- Chains "enable fast writes of new commit data without rewriting the entire commit history". Files are `info/commit-graphs/graph-{hash}.graph`. A plain-text `commit-graph-chain` lists hashes from lowest to highest. — [git commit-graph doc](https://github.com/git/git/blob/master/Documentation/technical/commit-graph.adoc)
- Positions are global: a commit at position i in the top layer has position X0 + X1 + i, where Xn is the commit count of layer n. Each non-base file records the hashes of all lower layers. — [git commit-graph doc](https://github.com/git/git/blob/master/Documentation/technical/commit-graph.adoc)
- Merge rule: new commits go to a new layer. Layer N+1 merges into layer N if layer N has fewer than X times the commits of layer N+1 (`--size-multiple`, default 2), or if layer N+1 exceeds C commits (`--max-commits`; example value 64,000). Merges cascade. — [git commit-graph doc](https://github.com/git/git/blob/master/Documentation/technical/commit-graph.adoc)
- Publish sequence: write the merged layer to a temp file while holding `commit-graph-chain.lock`, rename it to `graph-{hash}`, then write the new chain into `commit-graph-chain.lock`. — [git commit-graph doc](https://github.com/git/git/blob/master/Documentation/technical/commit-graph.adoc)
- Delayed deletion: "another process could read the commit-graph-chain file before it is rewritten" and then look for deleted files. So unreferenced files get their mtime updated and are deleted only after an expiry window (default 0, configurable). — [git commit-graph doc](https://github.com/git/git/blob/master/Documentation/technical/commit-graph.adoc)
- Mixed generation versions: corrected dates are written only if the top layer has them, which guarantees all lower layers have them too. `--split=replace` collapses to one layer. — [git commit-graph doc](https://github.com/git/git/blob/master/Documentation/technical/commit-graph.adoc)

**git lockfile protocol (publish primitive)**
- Purpose: "Mutual exclusion and atomic file updates". To change a file, git creates `<filename>.lock` (O_CREAT|O_EXCL, so competing lockers fail), writes the new contents, "then atomically rename the lockfile to its final destination to commit the changes and unlock the file". Rollback removes the lockfile. — [git lockfile.h](https://raw.githubusercontent.com/git/git/master/lockfile.h)

**Tantivy**
- "A tantivy index is a collection of smaller independent immutable segments." "On commit, one segment per indexing thread is written to disk, and the `meta.json` is then updated atomically." "A small `meta.json` file is in charge of keeping track of the list of segments, as well as the schema." — [Tantivy ARCHITECTURE.md](https://github.com/quickwit-oss/tantivy/blob/main/ARCHITECTURE.md)
- Deletes are immutable alive bitsets named `segment_id.commit_opstamp.del`. Opstamps are "an incremental id that identifies any operation applied to the index." Merges run in background threads, purge deletes, and reduce segment count. — [Tantivy ARCHITECTURE.md](https://github.com/quickwit-oss/tantivy/blob/main/ARCHITECTURE.md)
- A Searcher "captures a snapshot of the index at one point of time, by holding a list of SegmentReader". Commits, GC, and merges do not affect a held Searcher because work "will happen on an immutable snapshot of the index." — [Tantivy ARCHITECTURE.md](https://github.com/quickwit-oss/tantivy/blob/main/ARCHITECTURE.md)

**Lakehouse table formats (Iceberg/Delta)**
- Not retrieved. Both spec pages were truncated by the fetcher. See Gaps.

**Reader-side primitives (Rust)**
- `ArcSwap` is "a container for an `Arc` that can be changed atomically", "optimized for read-mostly scenarios, with consistent performance characteristics". Typical uses are config, routing tables, and "periodically renewed data snapshots". A `RwLock<Arc<T>>` "suffers from CPU-level contention (on the lock and on the reference count of the Arc)", and "an update may be blocked for arbitrary long time by a steady inflow of readers". ArcSwap "has better performance characteristics than the RwLock, both in contended and non-contended scenarios." Version 1.9 fixed orderings that were "too weak", which "probably introduces some performance degradation." — [arc-swap docs](https://docs.rs/arc-swap/latest/arc_swap/)
- left-right "keeps two copies of the backing data structure". Reads are wait-free. Each oplog entry is applied twice, so it is "effectively doubling the memory use". It "only supports a single writer", and `publish` waits for readers to depart. — [left-right docs](https://docs.rs/left-right/latest/left_right/)

**LMDB as the current substrate (MVCC copy-on-write B+tree)**
- "only one write transaction may be active at a time". "readers don't block writers" and "writers cannot block readers". It supports access "from multiple processes and threads". — [lmdb.h](https://raw.githubusercontent.com/LMDB/lmdb/mdb.master/libraries/liblmdb/lmdb.h)
- "Read transactions prevent reuse of pages freed by newer write transactions, thus the database can grow quickly." "Stale reader transactions left behind by an aborted program cause further writes to grow the database quickly". The fix is `mdb_reader_check`. — [lmdb.h](https://raw.githubusercontent.com/LMDB/lmdb/mdb.master/libraries/liblmdb/lmdb.h)
- "The size of the memory map is also the maximum size of the database". If another process grows the data past the current mapsize, `mdb_txn_begin()` returns `MDB_MAP_RESIZED`. — [lmdb.h](https://raw.githubusercontent.com/LMDB/lmdb/mdb.master/libraries/liblmdb/lmdb.h)

### Inferences
- **Replace "rewrite whole CSR blob per wave" with append-only CSR segments (inference, modeled on commit-graph chains).** Each write wave produces one immutable segment holding dense ids `[base, base+n)`, a CSR of parent edges in global ids, and tier-A columns. A manifest lists segments in order (the git `commit-graph-chain` equivalent). Write cost per wave becomes O(wave), not O(total), which removes the quadratic total work of full-blob rewrites at 1M–10M commits.
- **The streaming-CSR edge-drop bug candidate is a structural consequence of id ordering (inference).** Global-position CSR works only when every edge points to an id that already exists, i.e. parents get ids before children. That is git's layer property and Sapling's `assign_head` ("assigning an id to a head also assigns ids to its ancestors"). A newest-first walk emits children before parents, so parent references cross into future batches and cannot resolve at segment-seal time. That is exactly an "edges crossing batch boundaries" failure mode. There are three sound fixes:
  - (a) Assign ids in parent-first topological order. This needs the DAG shape before numbering.
  - (b) Store unresolved parent shas in an "open" segment and resolve them at seal time against sha→id.
  - (c) Keep child→parent edges as sha-keyed pending edges in a small overlay until the parent arrives.
  
  Option (b) is the simplest correct choice. This needs verification against the actual code.
- **Merge policy (inference):** copy git's geometric rule (merge when the lower layer has fewer than 2× the upper layer's commits, plus a max-commits cap) for logarithmic segment count. Run merges on a background thread and publish them like any other manifest change.
- **Reader isolation (inference):** the in-memory query graph becomes `ArcSwap<Arc<Snapshot>>`, where `Snapshot { manifest_version, segments: Vec<Arc<Segment>>, ref_tips }`. Queries `load()` once and run to completion on that snapshot. The writer builds the new snapshot and `store()`s it after the manifest commit. Old segments drop when the last `Arc` drops, which is RCU-style reclamation without hand-written epochs. This replaces the process-global single-entry cache. left-right is rejected: 2× memory on a multi-GB graph, and a writer that waits for readers inverts the desired priority.
- **LMDB plus long UI reads plus whole-blob rewrites is an evidenced growth hazard (inference from the LMDB caveats).** A UI query holding a read txn pins every page the writer frees, so each wave's full CSR rewrite allocates fresh pages. Moving the large immutable graph segments out of LMDB into plain mmap'd files greatly reduces LMDB's role, leaving only manifest, sha→id map, and small KV. Alternatively, never hold LMDB read txns across a whole user query: copy what is needed or use segment files.

### Gaps
- The Apache Iceberg spec (atomic metadata-pointer swap, optimistic concurrency) and the Delta Lake PROTOCOL.md (ordered atomic JSON commits, put-if-absent) could not be fetched; the pages were truncated. Do not cite their specifics from these notes.
- Tantivy docs.rs pages for IndexWriter (single-writer lock, `prepare_commit`) and IndexReader `ReloadPolicy` returned 404. Those details are unverified here.
- The git doc does not explicitly describe the final rename of `commit-graph-chain.lock` to `commit-graph-chain`. The lockfile.h protocol implies it.
- crossbeam-epoch docs were not fetched.

---

## Q3. Crash consistency: manifest swap vs WAL vs shadow paging; fsync semantics per OS; "rebuildable cache"

### Takeaway
The derived index is a rebuildable cache, because git is the source of truth (Mononoke: derived data "can always be regenerated from the core data"). The design target is therefore atomicity and detectability, not durability. Never expose a torn state, and on doubt fall back or re-derive. Losing the last few waves after power loss is acceptable. Use shadow paging plus manifest swap: write immutable files, then publish by lockfile+rename of a checksummed manifest. Keep the previous manifest, and validate on open. That design keeps correctness independent of fsync-ordering subtleties, which OSDI'14 showed vary across file systems and break real applications. Replace "full index clears all data first" with build-new-generation-then-swap.

### Cited Findings
- Pillai et al., "All File Systems Are Not Created Equal: On the Complexity of Crafting Crash-Consistent Applications", OSDI 2014. The BOB tool showed persistence properties "vary widely among six popular Linux file systems". The ALICE tool found 60 crash vulnerabilities across 11 widely used systems (databases, key-value stores, version control systems, distributed systems, virtualization), "many with serious consequences". — [USENIX OSDI'14 page](https://www.usenix.org/conference/osdi14/technical-sessions/presentation/pillai)
- LMDB copy-on-write "eliminates the need of any special recovery procedures after a system crash". It is "fully transactional with full ACID semantics". — [lmdb.h](https://raw.githubusercontent.com/LMDB/lmdb/mdb.master/libraries/liblmdb/lmdb.h)
- LMDB durability flags:
  - MDB_NOSYNC: "a system crash can corrupt the database or lose the last transactions". With an ordered filesystem and without MDB_WRITEMAP, transactions "only lose D (durability)".
  - MDB_NOMETASYNC: "a system crash may undo the last committed transaction", but integrity is kept.
  - MDB_WRITEMAP "loses protection from application bugs". The docs also say: "Do not mix processes with and without MDB_WRITEMAP on the same environment", which "can defeat durability".
  
  — [lmdb.h](https://raw.githubusercontent.com/LMDB/lmdb/mdb.master/libraries/liblmdb/lmdb.h)
- **Repo fact:** `adapters/db.rs` reads `GITGRAPH_LMDB_WRITE_MAP` and `GITGRAPH_LMDB_MAP_ASYNC` per process (lines 26–27, 62–63). It sets `EnvironmentFlags::WRITE_MAP` and `MAP_ASYNC` from them (lines 97–100) and validates that MAP_ASYNC requires WRITE_MAP (line 76). — local file `apps/desktop/src-tauri/src/adapters/db.rs`
- SQLite: "Transactions are durable across application crashes regardless of the synchronous setting or journal mode". In WAL with synchronous=NORMAL, a commit "might roll back following a power loss or system crash", but the database stays consistent. "fullfsync ... default value ... is off. Only Mac OS X supports F_FULLFSYNC." — [SQLite PRAGMA docs](https://www.sqlite.org/pragma.html)
- macOS: Apple's fsync(2) man page tells applications that need strict write ordering, such as databases, to use F_FULLFSYNC (see fcntl(2)). F_FULLFSYNC asks the drive to flush all buffered data to permanent storage. — [Apple fsync(2) archive](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fsync.2.html). An Apple engineer said F_FULLFSYNC is implemented with FLUSH_TRACK_CACHE on ATA, and some third-party FireWire bridges do not pass it through. — [PostgreSQL list, "Apple Darwin disabled fsync?"](https://www.postgresql.org/message-id/a06010200be3da9564694%40%5B17.202.21.231%5D). SQLite developers reported F_FULLFSYNC "slowed things down a lot" (search-result summary; secondary).
- Rust `std::fs::rename` maps to `rename` on Unix and to `MoveFileExW` with a fallback to `SetFileInformationByHandle` on Windows. "On Windows 10 version 1607 and above, the behavior is the same as Unix if the filesystem supports `FileRenameInfoEx`." It fails across mount points. The docs make no atomicity statement. — [std::fs::rename](https://doc.rust-lang.org/std/fs/fn.rename.html)
- Rust `File::sync_all` "Attempts to sync all OS-internal file content and metadata to disk". `sync_data` "might not synchronize file metadata", and "some platforms may simply implement this in terms of `sync_all`". — [std::fs::File](https://doc.rust-lang.org/std/fs/struct.File.html)
- Windows refuses to delete a file while a memory mapping of it exists, even with FILE_SHARE_DELETE. The mapping can outlive the handle. — [OSR community thread](https://community.osr.com/t/deleting-an-opened-memory-mapped-file/19414); [OpenJDK nio-dev](https://mail.openjdk.org/pipermail/nio-dev/2019-January/005713.html)
- git delays deletion of unreferenced commit-graph layers because concurrent readers may still resolve the old chain. — [git commit-graph doc](https://github.com/git/git/blob/master/Documentation/technical/commit-graph.adoc)

### Inferences
- **Recommended protocol (inference; each step is a pattern cited above):**
  1. Write the segment file to `seg-<uuid>.tmp`, `sync_data`, and rename to `seg-<content-hash>.seg`. The content hash in the name makes publishing idempotent.
  2. Write `MANIFEST.lock` with `{generation, version, ordered segment list with lengths and xxh3/blake3 checksums, indexed ref-tip set, derived-type versions, schema version}`. Then `sync_data` and rename to `MANIFEST` (git lockfile protocol). Keep `MANIFEST.prev`.
  3. On POSIX, fsync the directory after the renames, for durability only.
  4. On open, verify the manifest checksum and each segment's length and checksum, or lazily verify a footer. On failure, use `MANIFEST.prev`. If that fails too, rebuild from git.
  
  Correctness then depends only on "rename is atomic or detectably not", plus checksums. Durability tuning (F_FULLFSYNC, directory fsync) becomes an optional knob, because lost work is re-derivable.
- **Full reindex = blue/green generation (inference).** Build generation N+1 in a fresh directory or namespace while readers keep using generation N. Publish by manifest swap. A crash mid-build leaves N intact plus orphan files for GC. This replaces "clear all data first", which today leaves partial data and relies on the next run's fallback.
- **Avoid MAP_ASYNC/NOSYNC for anything not rebuildable (inference).** The LMDB docs say a system crash can corrupt the DB under NOSYNC-class modes. If LMDB is kept, prefer NOMETASYNC, which keeps integrity and may lose only the last txn, or full sync. Treat a corrupt env as "rebuild". Per-process env-var control of WRITE_MAP risks violating LMDB's "do not mix" rule once a CLI and a desktop app share an env. Make the flag a persisted property of the env, recorded in its manifest, not a per-process switch.
- **GC must be delayed and retry-tolerant (inference).** Windows cannot delete mmap'd files and readers may hold old snapshots, so GC runs only after the last `Arc<Segment>` drops in-process. Cross-process, use an expiry window (git-style mtime grace) and retry on Windows "access denied".
- **No WAL needed (inference).** The writer's in-flight wave is fully reconstructible from git (frontier = manifest ref tips), so on restart work resumes from the last published manifest. That is effectively a "redo from source" log.

### Gaps
- I did not verify whether Rust's `sync_all` on macOS issues F_FULLFSYNC or plain fsync. Check the std source before relying on it.
- Windows guarantees for MoveFileEx replace-atomicity and for directory-flush equivalents were not retrieved from Microsoft docs.
- The exact LMDB text for MDB_MAPASYNC was not captured; the fetch summarized NOSYNC/NOMETASYNC/WRITEMAP only.
- The OSDI'14 per-filesystem tables (which properties hold on ext4/btrfs/xfs) were not extracted. macOS APFS and NTFS were not in that study.
- Follow-up work ("Can Applications Recover from fsync Failures?", ATC 2020; PostgreSQL fsyncgate) was not fetched.

---

## Q4. Concurrency model for a desktop backend (indexing + concurrent UI queries)

### Takeaway
Use single-writer/multi-reader with snapshot isolation. One owner "engine" holds the writer thread. The ingest pipeline uses bounded channels for backpressure, CPU-parallel stages run on Rayon, and readers are queries against immutable `Arc` snapshots. Use Tokio only at the IPC/async edge. Priority cannot rely on OS QoS portably: rust-analyzer's QoS mapping works only on Apple platforms. So implement priority in-app: separate interactive and background pools, cooperative cancellation tokens checked at bounded intervals, and an indexer that yields at wave boundaries.

### Cited Findings
- Tokio: blocking work should use `spawn_blocking`. For CPU-bound code, "you should use a separate thread pool dedicated to CPU bound tasks" and "you could consider using the rayon library". Task switching "can only happen at .await points", and "code that spends a long time without reaching an .await will prevent other tasks from running." — [tokio docs](https://docs.rs/tokio/latest/tokio/index.html)
- rust-analyzer threading: "`Analysis` is an immutable snapshot of the state", while "`AnalysisHost` is a state to which you can transactionally `apply_change`". Requests that mutate state or could block typing run on the main thread, and "All other requests are processed in background". "Each LSP-request is protected by a `catch_unwind`". — [rust-analyzer architecture](https://rust-analyzer.github.io/book/contributing/architecture.html)
- Salsa cancellation in rust-analyzer: on change, "salsa bumps this counter and waits until all other threads using salsa finish". A stale computation "panics with a special value", converted to `Result<T, Cancelled>`. The highlighting job is cancelled because "its results are now stale, and it also blocks modification of the inputs." — [rust-analyzer architecture](https://rust-analyzer.github.io/book/contributing/architecture.html)
- rust-analyzer `ThreadIntent`: `Worker` is work not on the typing critical path and `LatencySensitive` is work caused by typing. On Apple these map to QoS Utility and UserInitiated. Elsewhere the code is a stub with one Default class, and a FIXME notes that Windows QoS APIs are unused. The thread pool takes an intent per `spawn`. — [rust-analyzer stdx/thread/intent.rs](https://rust-lang.github.io/rust-analyzer/src/stdx/thread/intent.rs.html); [pool.rs](https://rust-lang.github.io/rust-analyzer/src/stdx/thread/pool.rs.html)
- Buck2 DICE runs computations in parallel on tokio executors with deduplication of identical requests. — [Buck2 Modern DICE](https://buck2.build/docs/insights_and_knowledge/modern_dice/)
- LMDB: a single write txn at a time, and "writers can never deadlock". Without MDB_NOTLS, "A thread can only use one transaction at a time". "Avoid suspending a process with active transactions." — [lmdb.h](https://raw.githubusercontent.com/LMDB/lmdb/mdb.master/libraries/liblmdb/lmdb.h)
- left-right writers wait for readers on `publish`. ArcSwap avoids the RwLock "steady inflow of readers" writer-starvation problem. — [left-right](https://docs.rs/left-right/latest/left_right/); [arc-swap](https://docs.rs/arc-swap/latest/arc_swap/)

### Inferences
- **Concurrency inversion vs Salsa (inference).** Salsa makes writers wait for readers and cancels them, because inputs mutate. Here the inputs are immutable segments, so the writer never waits and readers never need cancelling for staleness. A reader on snapshot v keeps a consistent answer while v+1 publishes. Cancellation is needed only for user intent: a superseded viewport, a closed panel, or a timeout.
- **Recommended topology (inference):**
  - Walker thread → bounded channel → Rayon parallel derive stages (file-change diff, Bloom filters; already CPU-bound per Tokio guidance) → bounded channel → single sealer/writer thread (segment write plus manifest publish) → `ArcSwap` publish.
  - Bounded channels remain the backpressure mechanism.
  - Queries run on a separate small "interactive" Rayon pool (or dedicated threads) sized to leave cores for the UI, so a 10M-commit index job cannot starve them.
  - On macOS, optionally set QoS like rust-analyzer. Never depend on it for correctness or for fairness elsewhere.
- **Priority by construction (inference):** the indexer checks an `AtomicBool`/epoch "interactive demand" flag at each wave boundary and shrinks its wave size or sleeps briefly while UI queries are in flight. Adaptive wave sizing already exists, so this is a policy input, not a new mechanism.
- **Cancellation (inference):** each query carries a cancellation token checked every N nodes visited (bounded latency, e.g. every 4–16k nodes). Results stream in chunks over the IPC channel (progress events), so the UI can render early and abort. Reuse rust-analyzer's `catch_unwind` per request boundary to contain panics.
- **Long reads must not pin writer resources (inference).** If LMDB stays, queries must not hold LMDB read txns for the query duration (page-reuse caveat). Snapshot data should live in `Arc`-owned segment mmaps instead.

### Gaps
- Rayon has no documented priority scheduling; I did not verify the docs. Per-OS thread priority APIs (Windows SetThreadPriority, Linux nice/SCHED_IDLE) were not researched.
- I did not verify which async runtime Tauri v2 uses internally or how its command handlers are scheduled.
- I found no published benchmarks comparing Rayon vs Tokio for mixed git-object-decompression plus diff workloads.

---

## Q5. Configuration explosion (~37 env vars): evidence and best practice

### Takeaway
Empirical studies show most knobs are rarely set, and enumerated knobs see only 2–3 values used in practice. Interaction faults cluster in low-order combinations (1–2 parameters mostly, all faults found by 4–6-way coverage in NIST studies). Collapse the env-var space into a few typed, named profiles plus hardware/repo auto-tuning. Delete knobs that change semantics rather than performance. Test the rest with pairwise covering arrays and a "config changes speed, never results" differential oracle.

### Cited Findings
- Xu et al., "Hey, You Have Given Me Too Many Knobs! Understanding and Dealing with Over-Designed Configuration in System Software", ESEC/FSE 2015, pp. 307–319. The study covered thousands of customers of a commercial storage system and hundreds of users of two open-source systems. — [paper PDF](https://cs.unibg.it/esecfse_proceedings/fse15/p307-xu.pdf); [bib](https://bibtex.github.io/ESEC-FSE-2015-XuJFZPT.html)
- Reported findings, from a secondary summary not verified against the PDF:
  - Only about 6–17% of parameters were set by a majority of users, and up to 54% were rarely set by anyone.
  - For multi-option enumerated parameters, usually only 2–3 options were used.
  - Parameters with visible external effects were set more than internal-implementation ones.
  
  — [Never Work In Theory summary](https://neverworkintheory.org/2016/06/09/too-many-knobs.html)
- NIST: "most software bugs and failures are caused by one or two parameters, with progressively fewer by three or more" (studies 1999–2004). Multiple studies found 4-way to 6-way coverage "was able to detect all faults found with exhaustive testing". This is an empirical bound, not a proof. — [NIST: Interactions involved in software failures](https://csrc.nist.gov/Projects/automated-combinatorial-testing-for-software/combinatorial-methods-in-testing/interactions-involved-in-software-failures). Standard citation: Kuhn, Wallace, Gallo, IEEE TSE 30(6):418–421, 2004.
- **Repo fact:** 37 distinct `"GITGRAPH_*"` string literals exist across .rs/.ps1/.ts/.json files. Examples: `GITGRAPH_COMMIT_INGEST_PIPELINED`, `GITGRAPH_COMMIT_WALK_SORT`, `GITGRAPH_LEVEL3_STREAMING_CSR`, `GITGRAPH_LEVEL3_STREAMING_CSR_MODE`, `GITGRAPH_LEVEL3_TOPO_EXTRACTION_ORDER`, `GITGRAPH_FILE_CHANGE_SYNC_STRICT`, `GITGRAPH_LMDB_WRITE_MAP`, `GITGRAPH_LMDB_MAP_ASYNC`, `GITGRAPH_LMDB_NO_MEM_INIT`. db.rs already encodes cross-knob constraints (MAP_ASYNC requires WRITE_MAP; NO_MEM_INIT is redundant with WRITE_MAP). — local grep of this repo, 2026-11-01
- Mononoke selects derived-data versions via per-repo configuration ("Configuration controls which version is active per repository"), which is a typed, persisted setting, not an ambient per-process toggle. — [Mononoke docs 2.3](https://github.com/facebook/sapling/blob/main/eden/mononoke/docs/2.3-derived-data.md)

### Inferences
- **Classify every knob (inference):**
  - **Semantic** knobs change results or on-disk format. Examples: streaming-CSR mode, walk sort/scope, topo extraction order, SYNC_STRICT, WRITE_MAP. These must become one fixed code path, or a persisted, versioned property of the index recorded in the manifest. They are never per-run env vars.
  - **Performance** knobs keep results byte-identical. Examples: batch sizes, queue capacities, thread counts, prewarm limits. These derive from a typed `Profile` (e.g. `Balanced | Throughput | LowMemory`) plus auto-tuning from `available_parallelism`, RAM, and repo size (commit count from the previous manifest).
  - **Debug/experiment** knobs: keep behind one validated `DevOverrides` struct, parsed in one place, echoed into perf telemetry, and compiled only in a dev/perf feature.
- **Test strategy (inference from NIST):** generate a pairwise (t=2) covering array over the remaining performance knobs, with t=3 for the hot-path subset. Run each config against a fixed fixture set and assert identical outputs (same manifest content hash, same query answers). That metamorphic oracle turns the combinatorial space into a correctness property, which is how a cross-batch edge-drop bug would surface automatically.
- Expected payoff, consistent with Xu et al.: most of the 37 knobs can be removed with no user-visible loss. The few with "external effect" (memory ceiling, background-indexing aggressiveness) become the user-facing settings for the desktop UI.

### Gaps
- I did not verify Xu et al.'s exact percentages in the primary PDF; they come from a secondary summary.
- I found no direct study of auto-tuning vs fixed profiles for embedded storage engines. The RocksDB auto-tuning literature (e.g. SILK, ML-based tuners) was not researched.

---

## Q6. Process architecture and multi-process safety (in-process library vs sidecar vs daemon)

### Takeaway
Ship the core as a library with one long-lived `Engine` owner per process. Host it in-process inside the Tauri app first, with the CLI as a second host of the same library. Coordinate across processes with an OS file lock on an index-dir writer lockfile. LMDB already supports multi-process readers, but forbids opening an env twice in one process and forbids remote filesystems. Move to a per-user daemon (Watchman model) only if several frontends must share one live index. Tauri sidecars give a well-supported bundling path for that later step.

### Cited Findings
- LMDB: "Do not have open an LMDB database twice in the same process at the same time." "Use an MDB_env* in the process which opened it, not after fork()." "Do not use LMDB databases on remote filesystems, even between processes on the same host." "There is normally no pure read-only mode, since readers need write access to locks and lock file." "the lockfile is always reset on first open of the environment". Stale writers are cleared automatically on Windows. — [lmdb.h](https://raw.githubusercontent.com/LMDB/lmdb/mdb.master/libraries/liblmdb/lmdb.h)
- Rust std file locks (stable since 1.89.0): `lock`/`lock_shared`/`try_lock` map to `flock` (LOCK_EX/LOCK_SH/LOCK_NB) on Unix and to `LockFileEx` on Windows. Locks "may be advisory or mandatory" and are released when the handle and any duplicates close. On Windows a file opened only for append cannot be locked. — [std::fs::File](https://doc.rust-lang.org/std/fs/struct.File.html)
- Watchman runs as a single long-lived process per user. The binary "will take care of spawning the server process if necessary". The server "will create a unix domain socket for communication with its clients", with JSON or BSER PDUs and server-pushed `subscribe` messages. Clients find the socket via `watchman get-sockname`. — [Watchman socket interface](https://facebook.github.io/watchman/docs/socket-interface.html); [Watchman CLI options (Debian 4.9.0 copy)](https://sources.debian.org/src/watchman/4.9.0-9/website/_docs/cli-options.markdown)
- rust-analyzer: the server speaks LSP over stdio and is "stateless, a-la HTTP". Proc macros run in a separate process (`proc-macro-srv-cli`) for isolation. — [rust-analyzer architecture](https://rust-analyzer.github.io/book/contributing/architecture.html)
- Tauri v2 sidecars: external binaries listed in `bundle.externalBin`, each with a `-$TARGET_TRIPLE` suffix copy. They are spawned via `app.shell().sidecar(name)` with stdout events and stdin writes, and need explicit `shell:allow-spawn`/`allow-execute` capability permissions. The page does not document lifecycle or kill-on-exit. — [Tauri v2 sidecar docs](https://v2.tauri.app/develop/sidecar/)
- Git-style lockfiles give cross-process mutual exclusion via O_CREAT|O_EXCL creation, which works on all OSes but leaves stale `.lock` files after a crash. — [git lockfile.h](https://raw.githubusercontent.com/git/git/master/lockfile.h)

### Inferences
- **v1 recommendation (inference):**
  - `gitgraph-core` (library) exposes `Engine::open(index_dir)`, which owns exactly one LMDB env or segment store, one writer thread, and an `ArcSwap` snapshot.
  - The Tauri app holds one `Engine` per repo for its lifetime. This fixes "every CLI command opens the env fresh", and that per-command pattern would violate LMDB's "do not open twice in the same process" rule if commands ran concurrently in a long-lived Tauri process.
  - The CLI is a thin host of the same `Engine`.
- **Cross-process writer exclusion (inference):** take `File::try_lock` on `index_dir/WRITER.lock`, using `flock`/`LockFileEx`, which the kernel releases automatically on crash. Prefer it over a git-style O_EXCL lockfile, which can go stale after a crash. A second process that fails to get the lock opens read-only snapshots: it reads the manifest and mmaps segments. It can also tell the user that "indexing is running in another window".
- **Index location (inference):** keep the index on local disk in the OS cache directory, keyed by canonical repo path plus repo identity. LMDB is unsafe on network filesystems and needs write access even for readers. Network-mounted repos would otherwise break.
- **When to go daemon (inference):** if a CLI, the desktop app, and editor plugins must share one live, always-fresh index plus file-watching, adopt the Watchman shape: a per-user, lazily spawned daemon on a Unix socket or Windows named pipe, with subscriptions for progress and new-snapshot events. Tauri can then bundle it as a sidecar. Until then a daemon adds IPC serialization cost and lifecycle bugs without benefit, since LMDB or segment files already give multi-process readers.

### Gaps
- Sapling EdenFS daemon architecture was not researched.
- I did not verify how Tauri v2 handles sidecar termination when the app exits, or Windows named-pipe parity with Watchman's Unix socket.
- I did not verify whether `flock` and `LockFileEx` interoperate safely with LMDB's own lock file on all three OSes; the two are separate files, which should be safe, but this is untested.

---

## Q7. Integrated recommendation: what should the architecture be? (synthesis of Q1–Q6)

### Takeaway
Treat the index as a log-structured, content-addressed, rebuildable materialization of the git DAG:

- immutable, parent-first-numbered segments
- tier-A per-commit derived facts keyed by dense commit id and type version
- DBSP-style delta maintenance for aggregates
- in-memory memoization only for ref- and query-dependent results
- manifest-swap publishing with checksums and a previous-manifest fallback
- one writer per index (OS file lock), snapshot readers via `ArcSwap`
- typed profiles instead of env vars

Every element has a production precedent: git commit-graph chains, Tantivy, Mononoke, LMDB, rust-analyzer, Watchman, and DBSP.

### Cited Findings
- Precedents, each cited in detail above:
  - layered immutable graph files plus a chain manifest plus delayed GC ([git commit-graph](https://github.com/git/git/blob/master/Documentation/technical/commit-graph.adoc))
  - immutable segments plus an atomic `meta.json` plus snapshot searchers ([Tantivy](https://github.com/quickwit-oss/tantivy/blob/main/ARCHITECTURE.md))
  - commit-keyed, versioned, regenerable derived data ([Mononoke](https://github.com/facebook/sapling/blob/main/eden/mononoke/docs/2.3-derived-data.md))
  - Z-set delta algebra ([DBSP](https://www.vldb.org/pvldb/vol16/p1601-budiu.pdf))
  - immutable `Analysis` snapshots plus a mutable host ([rust-analyzer](https://rust-analyzer.github.io/book/contributing/architecture.html))
  - parent-first integer id assignment ([sapling-dag](https://docs.rs/crate/sapling-dag/latest/source/src/idmap.rs))
  - lockfile+rename publish ([git lockfile.h](https://raw.githubusercontent.com/git/git/master/lockfile.h))

### Inferences
- **Data lifecycle (inference):**
  1. **discover:** frontier diff of ref tips against the manifest.
  2. **number:** assign parent-first dense ids, recorded in an append-only sha→id map.
  3. **derive:** tier-A facts in parallel on Rayon, deterministic.
  4. **seal:** write an immutable segment, all edges resolved, with checksum.
  5. **publish:** manifest lockfile+rename, then `ArcSwap` store.
  6. **compact:** geometric merge at factor 2 in the background, published the same way.
  7. **GC:** delete unreferenced files after the last in-process reference drops and an mtime grace window passes, with Windows retry.
  8. **re-derive:** bumping a derived type version creates a new key namespace that is backfilled lazily or in the background, Mononoke-style, while old-version data keeps serving.
- **Throughput rationale for ≥100k commits/s (inference, needs measurement):**
  - Per-wave cost becomes O(wave) instead of O(total) (no full CSR rewrite).
  - The writer does sequential file appends plus one small manifest write instead of B+tree page churn.
  - Derive stages scale with cores.
  - Visibility latency equals the seal interval and is tunable by the profile.
  
  This is not verified; `pnpm run quality:perf` must confirm it before any performance claim (AGENTS.md rule 8).
- **What to delete (inference):**
  - the 300 s lookback heuristic
  - the async file-change retry queue (replaced by a derivable "missing derived key" set)
  - "clear all data on full index" (replaced by generation swap)
  - the process-global single-entry graph cache (replaced by an Engine-owned `ArcSwap`)
  - semantic env-var toggles
- **ADR impact (inference):** this changes the adapters (storage format), application (pipeline/orchestration), and domain (snapshot/manifest DTOs) layers. Per repo AGENTS.md it requires an ADR in `docs/adr`. Land it incrementally behind one persisted format version, not an env var.

### Gaps
- All throughput and latency outcomes are unmeasured inferences. No benchmark of segment files vs LMDB-blob for this workload was found or run.
- The bug hypothesis that a newest-first walk causes cross-batch edge drops in streaming CSR is unverified against the code. It is offered as a mechanism to test (fixture: a merge commit whose parent lands in the next batch).
- Handling of shallow clones, grafts, and replace objects needs a policy. git itself disables commit-graph reads in those cases ([git commit-graph](https://github.com/git/git/blob/master/Documentation/technical/commit-graph.adoc)); no policy is designed here.
