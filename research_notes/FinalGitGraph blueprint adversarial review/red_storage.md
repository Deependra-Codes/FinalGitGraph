# Red-team review: storage, file-format and publishing claims in the FinalGitGraph blueprint (as of 2026-11-01)

Targets:
- `reports/FinalGitGraph backend architecture blueprint.md` (abbreviated **BP**). Sections attacked: "Proposed architecture" diagram; "Storage: checksummed layer files replace all 17 LMDB tables"; "Queries: ID order replaces hydration" (the 24 B/commit figure); "Keep, replace or remove" ledger rows on LMDB, whole-blob CSR and bincode; "Validation and kill criteria".
- `research_notes/FinalGitGraph backend architecture blueprint/storage_engine.md` (abbreviated **SE**), plus contradictions with `incremental_concurrency_consistency.md` (**ICC**), `frontier_novel_architectures.md` (**FNA**) and `graph_representation_and_queries.md` (**GRQ**).

Evidence labels: **[primary]** means I fetched the source this session (crates.io API, GitHub API, raw upstream source, man7, Microsoft Learn, the ALICE PDF). **[code]** means I read it in this repo. **[sim]** means a Python simulation I ran this session; the parameters are stated. **[inference]** means my own reasoning, not verified. Nothing was compiled or benchmarked in the repo.

---

## Q0. Ranked attacks: which storage claims break, and how badly?

### Takeaway
I found three critical attacks.
- **Silent commit loss from concurrent publishes.** The diagram runs background compaction and ingest as two publishers, and no manifest compare-and-swap is specified. A compaction publish can drop a layer that ingest just published. Every file still passes its checksum, so the loss is silent.
- **Wrong ancestry answers after unshallow.** "Append-only by construction" breaks on `git fetch --unshallow` or `--deepen`. Parents then arrive after their children and get higher IDs, so the O(1) `is_ancestor` rule returns wrong answers.
- **The max-commits cap voids one of two headline claims.** In git, `--max-commits` forces cascading merges. Copied literally at git's 64,000, write amplification rises from about 7x to about 62x. Read as a true cap instead, the layer count stops being logarithmic.

Several major attacks follow:
- The 110 GB vs 3.5 GB comparison uses a baseline that Phase 0 itself deletes.
- The 360 MB read-at-open figure leaves out columns the blueprint puts in its own layer.
- "No database" leaves a set of mutable state that does not scale or stay isolated.
- Both fallbacks (redb, heed) conflict with other blueprint goals.
- The Windows `rename` claim is wrong for Rust 1.94.
- The publish/gc protocol has orphan, reuse and grace-window holes.

### Cited Findings
(Detailed evidence for each attack is under Q1–Q6. This list holds the decisive facts only.)
- git's merge loop keeps merging while `g->num_commits <= st_mult(size_mult, num_commits) || (max_commits && num_commits > max_commits)`, and `max_commits` defaults to `0` in code. [primary] — [git commit-graph.c, split_graph_merge_strategy, lines ~2289–2330](https://github.com/git/git/blob/master/commit-graph.c)
- git's technical doc gives the purpose of the second condition: it "bounds the total number of commits in a `graph-{hashN}` file and not in the `commit-graph` file". It calls "64,000 for the maximum number of commits" a value that "could be extracted into config settings". [primary] — [technical/commit-graph.adoc, "Merge Strategy"](https://github.com/git/git/blob/master/Documentation/technical/commit-graph.adoc)
- At the 1.94.0 tag, Rust std on Windows calls `MoveFileExW(old, new, MOVEFILE_REPLACE_EXISTING)` first. It retries with `FileRenameInfoEx` + `FILE_RENAME_FLAG_POSIX_SEMANTICS` only "if `MoveFileExW` fails with ERROR_ACCESS_DENIED". Any other error is returned as-is. [primary] — [rust 1.94.0 library/std/src/sys/fs/windows.rs lines 1311–1378](https://github.com/rust-lang/rust/blob/1.94.0/library/std/src/sys/fs/windows.rs)
- Every released redb version locks the whole file for a writer. The 4.5.0 entry, still dated "2026-XX-XX", is the first to add `ConcurrencyMode::SingleWriter`, which "allows one process to write, and any number of processes may open the database read-only". It also says "`ConcurrencyMode::ExclusiveWriter` (the default) locks the whole file, as earlier versions do". [primary] — [redb CHANGELOG](https://github.com/cberner/redb/blob/master/CHANGELOG.md)
- ALICE (OSDI'14) on git's protocol: "object files, which are never modified; they are created as temporary files, and then linked to their permanent file names … pointers … updated using update-via-rename. … We find many ordering dependencies in the Git protocol … Git developers expect total ordering from the file system." Unique vulnerabilities in Table 3(a): Git 9, Mercurial 10, LMDB 1, SQLite-WAL 0, total 60. [primary, text extracted from the PDF] — [Pillai et al., OSDI'14](https://www.usenix.org/system/files/conference/osdi14/osdi14-paper-pillai.pdf)

### Inferences
**Ranked attack list.** Severity: critical = silent wrong answers, or a headline claim voided. Major = user-visible failure or a large error in a claim. Minor = a precision error or a cheap fix.

| # | Sev | Claim attacked (where) | Attack type | Core evidence | Proposed revision |
|---|---|---|---|---|---|
| A1 | **critical** | BP diagram: "compact : geometric merge … in background, published the same way". ICC Q2: "Run merges on a background thread and publish them like any other manifest change." | Concurrency / crash-consistency hole (lost update) | Q4 scenario: compaction reads gen N = [A,B], ingest publishes gen N+1 = [A,B,D], compaction publishes [C] and drops D. git avoids this by holding `commit-graph-chain.lock` for the whole write, merge included. | Use a single publisher with a manifest compare-and-swap on `generation`. A compaction must replace exactly its input sub-sequence, rebased onto the current manifest. Add a test: concurrent compaction + ingest, assert the digest is unchanged. |
| A2 | **critical** | BP "Storage…": "The graph needs no key-value engine, because it is append-only by construction." BP "Identity…": an absent parent "becomes a typed boundary record". | Missed failure mode (mutability of the ODB) | Q2 scenario: a shallow tip has ID 0. `git fetch --unshallow` adds its parent P with ID ≥ 1, which breaks "parent < child". The rule "If id(A) > id(B), B cannot reach A" then returns false for a real parent. | Make boundary resolution (deepen, unshallow, objects that later appear) trigger a blue/green renumber. Key derived data by OID or OID-sorted position so it survives a renumber. Add deepen/unshallow to the fixture matrix. |
| A3 | **critical** (for the write-amplification headline) | BP "Storage…": a layer merges "when it exceeds a max-commits cap". BP diagram: "(size multiple 2, max-commits cap)". SE Q3: "or if level N+1 exceeds `--max-commits` (default 64,000)". | Mis-cited semantics and default; arithmetic | git code: exceeding C *forces* cascading merges; the code default is 0. [sim]: N = 10M, wave 16,384, X = 2, C = 64,000 gives **62.5x** amplification and 125 base rewrites, vs **6.9x** without C. Read as a real cap (never merge above C), the layer count becomes about N/C ≈ 156. | Drop "max-commits" or define it explicitly, with its simulated effect, in the ADR. Pre-register the amplification and maximum layer count from a simulation. |
| A4 | major | BP "Storage…": "**110 GB written** … **3.5 GB**" (a 30x claim). SE Q3 arithmetic. | Strawman baseline; per-commit sizes inconsistent; notes contradict each other | Q3: the 110 GB baseline is the streaming path that Phase 0 deletes. The Phase-0-fixed cold index writes the CSR once (about 0.36–0.65 GB). The layered design writes 2.5–8.2 GB [sim], 4–12x *more*. Today's blob is about 65 B/commit (hex `String` SHAs), not 36. FNA gives 7 rewrites and 1.2 GB for the same quantity. | Compare against the Phase-0 baseline, state the cold-index cost honestly, and justify layering by incremental and early-queryability needs. |
| A5 | major | BP "Storage…": "a 10M-commit topology layer set of about 360 MB is one sequential read". BP ledger: read() instead of mmap. | Arithmetic omission; contradiction with other notes; cold-start failure mode | Q3: the blueprint's own chunk list costs about 52–60 B/commit for topology alone and about 110–120 B with metadata, so 0.6–1.2 GB. Every CLI invocation reads all of it. Each process holds a private copy. GRQ and FNA assumed "O(1) mmap startup". The redb citation describes demand-paged pread, not read-everything. | Use lazy, per-chunk verified pread with a small cache (redb's real model), or mmap behind an ADR. Re-derive the open cost from the real chunk list. |
| A6 | major | BP "Storage…": "v1 needs no embedded database at all … What remains mutable is the manifest … and a tiny repo registry, and both fit atomic-rename files." | Incomplete mutable-state inventory; scalability; isolation (lost update) | Q1: 100k-ref snapshots get rewritten on every publish. Registry read-modify-write across two processes loses updates. The WRITER.lock scope blocks the CLI. Derivation-failure attempt counts are still needed (poison pill). `unavailable` status contradicts "never invalidated". The registry sits in a cache dir. | Put the manifest/registry/refs/attempts in SQLite WAL (multi-process, ALICE 0 vulnerabilities), or at least move refs into a content-addressed sidecar and lock the registry. |
| A7 | major | BP "Storage…": "redb (pinned ≥4.4.0) is the fallback". SE Q5: "redb does support multi-process access with a single writer". BP kill table: "heed 0.23 on LMDB 1.0-dev". | Factual error (SE); fallback contradicts the multi-process goal | redb ≤ 4.4.0 locks the whole file; SingleWriter is unreleased (4.5.0). heed's `EnvOpenOptions::open` is `pub unsafe fn` (AGENTS rule 2 then needs an ADR) and runs on the 1.0-dev branch. | Name SQLite WAL as the first fallback for multi-process state. For redb, require ≥ 4.5 SingleWriter once released and flag it experimental. Budget an `unsafe` ADR for heed. |
| A8 | major | BP "Storage…": "Its Windows `rename` uses POSIX-semantics `FileRenameInfoEx` with replace-if-exists". SE Q3: "`rename` calls `rename_inner(..., …POSIX_SEMANTICS)`". BP diagram: "retry on Windows" (gc only). | Factual error (order of calls); OS-portability hole | Q5: MoveFileExW comes first, POSIX semantics only on ACCESS_DENIED, and no fallback on SHARING_VIOLATION. git-for-windows retries rename *and* unlink with delays of 0/1/10/20/40 ms because "some other process had the file open at the wrong moment". | Add bounded retry and backoff to *every* rename and delete on Windows, publish included. Treat a persistent failure as "publish failed, keep the old manifest". Test with a reader handle that lacks FILE_SHARE_DELETE. |
| A9 | major | BP diagram: "gc : delete unreferenced files after last in-process Arc drops + grace window". BP: "falls back to `MANIFEST.prev`". SE Q6: "Orphan `.tmp` files and unreferenced segments get deleted at startup." BP: "A segment name is never reused". | Crash-consistency / gc holes; precedent mismatch; internal contradiction | Q4: in-process Arcs say nothing about other processes. git's default `--expire-time` is *now*, and git readers can fall back to parsing objects, which FinalGitGraph cannot. No step in the publish sequence writes MANIFEST.prev. A startup sweep by a non-lock-holder deletes in-flight files. Content-hash names plus deterministic layers mean names *are* reused. | Only the lock holder runs gc, under the publish mutex, with an mtime grace period. Use names `seg-<gen>-<hash>`. Specify how MANIFEST.prev is written and retained, and give readers a "reload on ENOENT" protocol. |
| A10 | major (low probability, cheap fix) | BP publish sequence (no directory fsync); BP "Publishing uses only `std`, with the same semantics on every OS". | Crash-consistency hole; contradiction with SE Q3 step 5; "same semantics" overclaim | Linux fsync(2): "an explicit fsync() on a file descriptor for the directory is also needed". ALICE: LevelDB "does not explicitly persist the directory entries of ldb files; a crash might cause the files to vanish". The blueprint copies the git protocol (9 ALICE vulnerabilities) in place of LMDB (1). | Fsync the directory on Unix after the segment rename and again after the manifest rename. Document the Windows gap. Run ALICE-style crash tests, not only kill-between-steps. |
| A11 | minor | BP: "writes and syncs `MANIFEST.lock` and renames it over `MANIFEST` (git's lockfile protocol ([git lockfile.h]))". | Ambiguity / contradiction with ICC | lockfile.h: git creates `<file>.lock` "with `O_CREAT\|O_EXCL`" and removes it through atexit/signal handlers, which do not run on SIGKILL or power loss. ICC Q6 says to prefer flock over an O_EXCL lockfile, "which can go stale after a crash". | State that MANIFEST.lock is a plain temp file (create + truncate) and is guarded only by WRITER.lock, or use a unique temp name. |
| A12 | minor | BP: "Rust's `sync_all` already issues `F_FULLFSYNC` on Apple". BP: "the design aims for atomicity and detectability rather than strict durability". BP throughput table. | Cost not counted / contradiction | Q5: std gives only the most expensive flush on macOS. The design says it does not need durability, and SE notes `F_BARRIERFSYNC` exists. The throughput table (git `rev-list`) includes no publish fsyncs. | Measure publish cost per OS in `quality:perf`. Consider reducing fsync frequency (one per publish, or batching). |
| A13 | minor | BP: the layer format "follows git's chunk design" (a custom format). | Missed alternative / base-rate risk | git's own commit-graph needed `GDA2`/`GDO2` because "a previous version of Git wrote possibly erroneous data", and Bloom v1 has a signed-`char` bug (both cited in BP). Arrow IPC is a specified LE, 64-byte-aligned container, and BP Phase 7 adds "Arrow export" anyway. | Evaluate Arrow IPC (or a strict subset of it) as the layer container before inventing one. Fuzz the parser. |
| A14 | minor | BP "Queries…": "about 24 bytes per commit for sorted OIDs plus permutation" and "about 4x less memory". | Arithmetic (one direction only) | GRQ: 24 B holds "if the sorted array alone is kept and id -> oid is resolved via a 4-byte inverse permutation", which makes 28 B for both directions. SHA-256 needs 40 B. Lookups also cost one binary search per layer. | Quote 28 B (SHA-1) / 40 B (SHA-256), which is about 3–3.5x, and count the per-layer lookup cost. |
| A15 | minor | BP / SE: peak disk use and ENOSPC during compaction are not covered | Missed failure mode | Q4: a base merge needs free space equal to the merged output while the inputs, plus MANIFEST.prev's layers, are still kept. | Preflight a free-space check before a merge. On ENOSPC, abort the merge and keep the old generation. |

---

## Q1. Is "no database in v1" sound? What mutable state remains, and does rewriting manifest files scale and stay atomic?

### Takeaway
No, not as written. Atomic rename gives *atomic visibility*. It gives no *isolation* for read-modify-write by two processes, and no cheap partial update.
- The blueprint's inventory leaves out state the current code keeps for good reason: derivation retry attempts and per-ref rows.
- The manifest as specified ({ref → oid} + layer list + derived versions) is rewritten whole on every publish. That is O(refs) bytes plus a full flush on every wave.
- The blueprint also contradicts itself. It rejects SQLite because of "a C build step", then says "the C toolchain is present wherever Rust links on all three OSes".

### Cited Findings
- **Current mutable state.** `db.rs:307-329` creates 17 named DBs. Among them, `repositories`, `repository_path_index`, `refs`, `enrichment_queue`, `enrichment_due_index`, `extraction_queue` and `checkpoints` hold mutable, non-graph state. `db.rs:31-36` sets `FILE_CHANGE_ENRICHMENT_MAX_ATTEMPTS: i64 = 5` and retry delays of 5–300 s. `META_NEXT_REPOSITORY_ID` is a persistent counter. [code] — `apps/desktop/src-tauri/src/adapters/db.rs`
- **What the blueprint says remains.** "the manifest (ref tips, layer list, derived-type versions, schema version) and a tiny repo registry, and both fit atomic-rename files." It adds that redb "gets added only when a mutable record set outgrows a manifest file". [BP "Storage…" para 5]
- **Derivation idempotence claim.** "Pending work is 'indexed IDs minus IDs covered by `fc@v1`', recomputed at startup … a crash at any point is harmless." Also: a commit with missing objects "gets an explicit `unavailable` status in the layer", and facts keyed by `(commit, fact_version)` are "never invalidated". [BP "Derived data…"]
- **Writer lock scope.** "The `Engine` owns exactly one writer, taken by `File::try_lock` on `index_dir/WRITER.lock` … Other processes (a second app window, the CLI) open read-only snapshots". [BP "Storage…" para 6]
- **Rust 1.94 `try_lock`** uses `flock(fd, LOCK_EX | LOCK_NB)` on most Unix targets (some targets use `fcntl(F_SETLK)`), and `LockFileEx` on Windows. [primary] — [unix.rs 1.94.0 ~L1532](https://github.com/rust-lang/rust/blob/1.94.0/library/std/src/sys/fs/unix.rs), [windows.rs 1.94.0 ~L458](https://github.com/rust-lang/rust/blob/1.94.0/library/std/src/sys/fs/windows.rs)
- **git's lockfile guarantee covers replacement only.** "lockfiles only block other writers. Readers do not block, but they are guaranteed to see either the old contents of the file or the new contents of the file (assuming that the filesystem implements `rename(2)` atomically)." [primary] — [git lockfile.h](https://github.com/git/git/blob/master/lockfile.h)
- **Index location.** BP puts the index in "the OS cache directory (`~/Library/Caches/<id>`, `$XDG_CACHE_HOME/<id>`, `{FOLDERID_LocalAppData}/<id>`)". [BP "Storage…" para 6]
- **SQLite WAL.** One writer and many readers across processes, and "readers do not block writers and a writer does not block readers". [cited in SE from the SQLite WAL docs] ALICE found **0** unique vulnerabilities for SQLite-WAL, vs 9 for Git. [primary] — [ALICE Table 3(a)](https://www.usenix.org/system/files/conference/osdi14/osdi14-paper-pillai.pdf)
- **BP contradiction.** SQLite is "the alternative if its longer crash record matters more than a C build step", yet "In practice the C toolchain is present wherever Rust links on all three OSes". [BP "Storage…" para 5]

### Inferences
- **A6a. Ref snapshots at scale (worked scenario) [inference].**
  - Repos with 100k+ refs are realistic: a mirror that fetched `refs/pull/*`, or Gerrit `refs/changes/*`. At about 50 B of ref name + 20 B OID + framing, the manifest's `{ref -> oid}` costs about 7 MB.
  - BP publishes the manifest after every wave, every compaction and every change-layer seal. A 10M-commit ingest with 16k waves has at least 611 wave publishes [sim], plus compactions and change layers.
  - The unchanged ref map is re-serialized and re-flushed every time. That is ≥ 611 × 7 MB ≈ 4.3 GB of manifest writes, each followed by `F_FULLFSYNC`/`FlushFileBuffers`.
  - Every reader refresh also re-reads and re-checksums 7 MB.
  - Fix: store refs in a content-addressed `refs-<hash>` file referenced by hash from the manifest, written only when tips change (the packed-refs/reftable pattern). Or keep refs as rows in SQLite.
- **A6b. Registry lost update [inference].**
  - The app reads registry {R0} and writes {R0,R1}. The CLI, at the same time, reads {R0} and writes {R0,R2}. Both renames are atomic and the last one wins, so R1 is lost silently.
  - Atomic rename prevents torn files. It does not serialize read-modify-write.
  - The only lock specified is WRITER.lock, and the long-lived app process holds it for its lifetime.
- **A6c. WRITER.lock scope [inference].**
  - If `index_dir` is per user, all repos share one lock. While the Tauri app runs, `cli index <other repo>` always fails `try_lock`, so the CLI is read-only whenever the app is open.
  - If `index_dir` is per repo, the shared registry and `next_repository_id` need a separate lock that BP never mentions. Either way the spec is incomplete.
- **A6d. Poison-pill derivation [inference].**
  - Today's queue caps retries at 5 attempts (`db.rs:31`). Under "pending = indexed − covered", a commit whose diff fails deterministically is re-selected at every startup forever. Example causes: a gix error on a malformed tree, an OOM on a huge tree, a timeout.
  - The alternative is to write a permanent "failed" status into an *immutable* change layer. Then transient failures become permanent: a pack locked by antivirus on Windows, ENOMEM, a cancelled run.
  - Either way, mutable per-commit attempt state is still needed, and BP's inventory leaves it out.
- **A6e. `unavailable` vs "never invalidated" [inference].**
  - The `unavailable` status depends on the local ODB, which is mutable. A partial-clone user may later fetch the missing blobs, for example with a refetch.
  - An immutable `(commit, fc@v1)` fact recorded as `unavailable` is then wrong forever, unless a new fact version or a mutable overlay supersedes it.
- **A6f. Registry in a cache directory [inference; XDG wording from the spec, not re-fetched this session].**
  - The XDG Base Directory spec defines `$XDG_CACHE_HOME` for "non-essential data files". Users and cleanup tools routinely wipe `~/.cache` and `~/Library/Caches`.
  - The graph is rebuildable from git. The *registry* is not: git does not know which repos the user added.
  - Fix: registry in the config/data dir, layers in the cache dir.
- **Contradiction with SE's evidence.** SE Q5 recommends redb or SQLite for mutable state. BP overrides it with no new evidence for manifest files. It cites only redb's 2026 bugs, which say nothing for or against SQLite. Given BP's own toolchain sentence, SQLite WAL is the lower-risk default for the mutable set: multi-process, ALICE 0, and row-level updates. A custom manifest has no crash-test record at all.

### Gaps
- I did not measure ref counts on real large repos (Android/Chromium/Gerrit mirrors). The 100k figure is a scenario, not a citation.
- I did not fetch the XDG spec or Apple's Caches-purge documentation this session.
- BP never says whether `index_dir` is per user or per repo, so A6c is conditional.

---

## Q2. Does "append-only by construction" hold, and is the layer list really the only non-monotonic state?

### Takeaway
No. The parent-first, append-only invariant holds only when no commit is ever indexed before all its ancestors are visible. Shallow clones, `--filter`, missing objects and later deepening break it. When they do, the O(1) rejection rules in the query kernel give **silent wrong answers** unless storage forces a full renumber. BP has no renumber path except "rebuild from git", and nothing triggers it.

### Cited Findings
- **The invariant.** "Each ID is issued only after all its parents have IDs, so every parent ID is lower than its child's." [BP "Identity…"]
- **Boundary handling.** "A parent that is truly absent, such as a shallow boundary or a missing object, becomes a typed boundary record, never a `continue`." [BP "Identity…"]
- **The O(1) rule that depends on it.** "If `id(A) > id(B)`, B cannot reach A (O(1), free from the numbering)." [BP "Queries…"]
- **Only refs are called non-monotonic.** "Only the ref layer is non-monotonic." [BP "Derived data…", para 2]
- **The fixture matrix** covers "shallow clones, `git replace`, `--filter=blob:none`". It does not cover deepen or unshallow *after* the first index. [BP "A layered oracle…"]

### Inferences
- **A2 worked failure [inference].**
  1. `git clone --depth 1`. Tip C has a boundary record for parent P. C gets ID 0.
  2. The user runs `git fetch --unshallow`. P and its ancestors become visible.
  3. Incremental ingest walks from the tips, sees C as already indexed, and either stops or appends P and its ancestors with IDs ≥ 1.
  4. Now id(P) > id(C) while P is C's parent. `is_ancestor(P, C)` takes the O(1) branch "id(A) > id(B) ⇒ false", which is a wrong answer with no error. A topo stream by descending ID also emits P before its child C.
  5. Every derived change layer covers *ID ranges*, so a renumber invalidates all of `fc@v1`. That is the expensive part, at about 19k diffs/s/core per BP's own table.
- **Fix.**
  - Treat "a boundary record became resolvable" as a format-level event that forces a blue/green rebuild of topology.
  - Key derived facts by OID, or by OID-sorted position plus a per-layer OID permutation, so a renumber can remap them instead of recomputing.
  - Add "index shallow, then unshallow" and "index with missing blobs, then backfill" to the fixture matrix and the metamorphic relations: full ingest must equal shallow-then-deepen ingest.

### Gaps
- BP does not say whether incremental ingest re-checks boundary records. If it ignores them, the result is a stale-but-consistent graph rather than wrong answers. The spec is silent either way.

---

## Q3. Do the numbers hold: 360 MB at open, 110 GB vs 3.5 GB, ~24 B/commit, and is read() + xxh3 at every open acceptable?

### Takeaway
The arithmetic is internally correct, but it describes the wrong objects.
- **360 MB** prices a 36 B/commit structure. That is not the layer BP specifies, which carries perm, generation, corrected date, commit time, authors, subject heap and reverse edges.
- **110 GB** prices a path Phase 0 deletes.
- **3.5 GB** assumes 36 B/commit and about 9–10 rewrites. git's real rule gives 6.9 rewrites, and the real per-commit size is about 3x larger.
- **24 B** covers OID→ID only.
- xxh3 cost at open is negligible. The problem is that read()-everything makes every open O(index size), CLI calls included, with private, unshared RSS.

### Cited Findings
- **SE's 360 MB arithmetic.** "about 20 B sha + about 8 B parent offset/neighbor + about 8 B child offset/neighbor per commit, roughly 360 MB". "Rewriting it whole every 16k commits means about 625 rewrites averaging about 180 MB, roughly 110 GB". "rewrites each commit about log2(625) ≈ 9–10 times, roughly 3.5 GB". [SE Q3 Inferences]
- **BP's own layer contents.** "a 256-entry OID fanout, sorted OIDs, an OID-to-ID permutation, parent arrays in global IDs with an overflow list for octopus merges, topological level, corrected commit date, commit time, interned author IDs and a subject string heap". [BP "Storage…" para 2] SE adds a reverse-edge (children) chunk per layer. [SE Q3]
- **Today's blob.** `pub struct CsrSnapshot { generated_at_epoch: i64, shas: Vec<String>, parents_offsets: Vec<u32>, parents_neighbors: Vec<u32>, children_offsets: Vec<u32>, children_neighbors: Vec<u32>, directed_edges: usize }`, serialized with bincode 1.3.3. [code] — `db.rs:208-216`
- **FNA gives a different estimate for the same quantity.** "Naive per-batch CSR rewrite with N = 10M and b = 100k: … ≈ 8.5 GB written. Geometric layer merging …: each commit is rewritten about log2(N/b) ≈ 7 times, so ≈ 1.2 GB." It also claims "O(1) mmap startup instead of hydration". [FNA D2]
- **GRQ.** "24 B/commit if the sorted array alone is kept and id -> oid is resolved via a 4-byte inverse permutation", and the layout is "mmap'ed and paged in lazily". [GRQ Q-synthesis lines ~258, ~271]
- **Phase 0 deletes the baseline.** Phase 0 "remove[s] the streaming-CSR apply path in both modes", after which "Full mode then always reaches `rebuild_csr_snapshot` after the walk". [BP "The phased plan"]
- **[sim] simulation of git's exact loop** (`while prev <= 2*num || (C && num > C) merge`), N = 10,000,000:

  | wave | C (max-commits) | commits written | amplification | max layers | full base rewrites |
  |---|---|---|---|---|---|
  | 16,384 | 0 (git code default) | 68.9M | **6.9x** | 7 | 14 |
  | 65,536 | 0 | 53.9M | 5.4x | 5 | 11 |
  | 16,384 | 64,000 (git doc value) | 624.7M | **62.5x** | 3 | 125 |
  | 65,536 | 64,000 | 762.1M | **76.2x** | 2 | 152 |

  - Bytes for the wave = 16,384 rows:
    - geometric: 2.5 GB at 36 B/commit, 4.5 GB at 65 B, 8.2 GB at 119 B;
    - with C = 64,000: 22.5 GB, 40.6 GB, 74.3 GB.
  - Today's whole-blob path: 611 waves, so 110 GB at 36 B and 199 GB at 65 B.
  - A Phase-0 single rebuild: 0.65 GB at 65 B.
- **xxHash README** (reference machine, from RAM): XXH3 (SSE2) 31.5 GB/s, XXH3 (AVX2) 59.4 GB/s, XXH128 (SSE2) 29.6 GB/s, memcpy 28.0 GB/s. [primary] — [xxHash README](https://github.com/Cyan4973/xxHash/blob/dev/README.md)
- **redb 0.14.0 (2023-03-26).** "The mmap backend has been removed because it was infeasible to prove that it was sound … the remaining `File` based backed is within a factor of ~2x on all workloads that I've benchmarked". [primary] — [redb CHANGELOG](https://github.com/cberner/redb/blob/master/CHANGELOG.md). BP restates this as "lost at most about 2x".

### Inferences
- **A5. Real open cost [inference; the per-column byte widths are my estimates].**
  - Topology-only columns BP needs for its query cascade: sorted OIDs 20 + perm 4 + inverse perm 4 + parent offsets/neighbors about 8.4 (assuming about 1.1 parents/commit) + generation 4 + corrected date 8 + commit time 8 ≈ 56 B. Adding children (about 8.4) gives about 65 B, which is **about 0.65 GB at 10M**, not 360 MB.
  - Author id 4 + subject offset 4 + a subject heap of about 40–60 B brings the total to about 110–125 B, or about 1.1–1.25 GB.
  - At disk speeds (my estimates, not measured): NVMe at about 3 GB/s gives 0.2–0.4 s; SATA SSD at about 0.5 GB/s gives 1.3–2.5 s; HDD or USB at about 100–150 MB/s gives 4–12 s.
  - xxh3 at about 30 GB/s adds only about 40 ms per GB. The cost is the I/O, not the hash, so I could not break the "checksum is cheap" sub-claim.
- **A5 consequences [inference].**
  - **CLI.** Each CLI process (`gitgraph is-ancestor A B`) reads and verifies the full index before answering an O(1) query. This contradicts "Small queries use a hash map sized to their visit budget, so `max_nodes=10` never allocates a 10M-entry array". The query is small, but the open is not.
  - **Memory.** Private heap copies, not a shared page cache. The app plus one CLI hold about 2x RSS. During a base merge, an old snapshot pinned by a long query plus the new merged layer can double peak RSS again.
  - **Mutually defeating plans.** BP keeps mmap "an opt-in for larger-than-RAM repos" *and* verifies checksums at open. Verifying a larger-than-RAM file at open reads it all from disk every time, which defeats the mmap.
  - **Mis-applied citation.** redb's no-mmap engine reads pages on demand (pread plus a user-space cache). It does not read the whole file at open, so citing redb's "~2x" for read-everything compares different designs. "At most" also overstates "on all workloads that I've benchmarked".
  - **Revision.** Use lazy per-chunk reads with per-chunk xxh3 verified on first touch, plus a bounded cache, or a background scrub instead of a full verify at open. Rebase the cold-start kill criterion on the real chunk list.
- **A4. Write amplification [inference + sim].**
  - The 30x headline compares the new design with the streaming path that Phase 0 removes. Against the Phase-0 baseline, a cold full index writes 4–12x *more* bytes: 2.5–8.2 GB vs about 0.36–0.65 GB.
  - Geometric layering is still the right call for (a) making partial results queryable during a long first index and (b) many incremental runs. The ADR should argue that case, not the 110 GB strawman.
  - The notes also contradict each other: SE says about 9–10 rewrites and 3.5 GB, FNA about 7 rewrites and 1.2 GB. Neither uses BP's actual layer width, and SE's log2 estimate overstates git's real `<=` rule (6.9x).
  - Note: today's blob is about 65 B/commit (bincode 1 encodes each `String` with a u64 length plus 40 hex bytes). So SE *understates* today's 110 GB, which would be about 199 GB. That flatters the old path, not the new one.
- **A3. The max-commits trap [primary code + sim].**
  - Copying git literally (exceeding C triggers merging, and the merge cascades to the base) at git's documented 64,000 means a full rewrite of the base about every 64k commits: 62–76x amplification. That is back within 2–5x of today's quadratic path.
  - Reading "cap" the other way (never merge a layer above C) makes the layer count about N/C ≈ 156 at 10M. OID lookup becomes about 156 binary searches, and `children(c)` a union over about 156 layers. That breaks "keeps the number of layers logarithmic".
  - SE's "(default 64,000)" is wrong for git's code, where `max_commits = 0` unless set. Separately, git's `--expire-time` defaults to the current time, so git's own default grace period is zero.
- **A14 [inference].** With id→OID needed for every result row, the lookup structure costs 28 B (SHA-1) or 40 B (SHA-256). Against the 85–100 B estimate that is about 3–3.5x less memory, not "about 4x".

### Gaps
- The per-column widths (subject length, parents per commit) are estimates. No corpus statistic was fetched.
- No disk-throughput numbers were measured on target hardware. The cold-start ranges are illustrative.
- The simulation models only the merge rule. It does not model background merge timing, I/O contention or per-layer overheads.

---

## Q4. Crash consistency and concurrency of publish, compaction and gc: orphans, disk-full, concurrent writers, MANIFEST.prev, grace windows

### Takeaway
The publish protocol is the right *shape*, but it is under-specified exactly where custom formats usually fail.
- Two in-process publishers have no CAS (A1).
- Content-hash names are reused, which contradicts "never reused" (A9).
- The gc grace is tied to in-process Arcs, while other processes are explicitly supported (A9).
- No step writes MANIFEST.prev, and gc can delete its layers (A9).
- A startup orphan sweep can run in a process that does not hold the lock (A9).
- The directory fsync was dropped (A10).
- Peak-space/ENOSPC policy is missing (A15).

The checksums do not help with A1: every file in a lost-update manifest is valid.

### Cited Findings
- **BP publish sequence.** "The writer writes `seg-<gen>.tmp`, calls `File::sync_all`, renames it to `seg-<hash>.seg`, then writes and syncs `MANIFEST.lock` and renames it over `MANIFEST`". BP also says: "A segment name is never reused, so a reader never holds a file that a writer is replacing." Recovery: "If verification fails, it falls back to `MANIFEST.prev`, and if that fails too, it rebuilds from git." [BP "Storage…" para 3]
- **BP determinism goals.** "a physical canonical root over byte-identical compacted layers". "A full re-index becomes blue/green: build generation N+1 next to N and swap the manifest." [BP "Novelty…", "Storage…"]
- **git's write protocol.** Write a temp file "all while holding a `commit-graph-chain.lock` lock-file", rename to `graph-{hash}`, then write the chain into the lock and rename it. The merge decision runs in the same locked write. [primary] — [technical/commit-graph.adoc](https://github.com/git/git/blob/master/Documentation/technical/commit-graph.adoc), [commit-graph.c](https://github.com/git/git/blob/master/commit-graph.c)
- **git's expiry.** "if `--expire-time=<datetime>` is not specified, let `datetime` be the current time. After writing the split commit-graph, delete all unused commit-graph whose modified times are older than `datetime`." In code, `expire_time = time(NULL)` unless set, and files are skipped only `if (st.st_mtime > expire_time)`. [primary] — [git-commit-graph.adoc](https://github.com/git/git/blob/master/Documentation/git-commit-graph.adoc), [commit-graph.c expire_commit_graphs ~L2504–2540](https://github.com/git/git/blob/master/commit-graph.c)
- **SE.** "Orphan `.tmp` files and unreferenced segments get deleted at startup." [SE Q6 item 4] **ICC:** "Keep `MANIFEST.prev`" with no mechanism given. [ICC Q3]
- **lockfile.h.** Locks are created "with `O_CREAT|O_EXCL`" and cleaned up by "an `atexit(3)` handler and a signal handler". [primary] — [git lockfile.h](https://github.com/git/git/blob/master/lockfile.h). **ICC Q6** says to prefer flock "over a git-style O_EXCL lockfile, which can go stale after a crash".
- **Linux fsync(2).** "Calling fsync() does not necessarily ensure that the entry in the directory containing the file has also reached disk. For that an explicit fsync() on a file descriptor for the directory is also needed." [primary] — [man7 fsync(2)](https://man7.org/linux/man-pages/man2/fsync.2.html)
- **ALICE.**
  - "LevelDB does not explicitly persist the directory entries of ldb files; a crash might cause the files to vanish, resulting in unavailability."
  - "many of these vulnerabilities (roughly half) manifest on current file systems such as Linux ext3, ext4, and btrfs."
  - "The single LMDB vulnerability is discussed in a mailing list"; "LMDB requires that the final pointer update (106 bytes) in the copy-on-write tree to be atomic."
  - Source: [primary] — [ALICE PDF](https://www.usenix.org/system/files/conference/osdi14/osdi14-paper-pillai.pdf)

### Inferences
- **A1. Lost layer (critical) [inference].**
  1. The manifest is at gen N = [A(8M), B(1M)]. The background compactor decides to merge A+B and starts writing C (minutes at 10M).
  2. Meanwhile, ingest seals wave D and publishes gen N+1 = [A, B, D].
  3. The compactor finishes and publishes "gen N+1" or "N+2" = [C], built from the layer list *it* read.
  4. D's commits vanish from the visible index. Every remaining file checksums fine and MANIFEST parses, so neither "verify on open" nor the `.prev` fallback fires.
  5. Later, the ref-snapshot frontier treats D's commits as indexed, because MANIFEST.tips moved forward at the end of the run. They are never re-ingested. This is the same silent-loss class as the current wave-boundary bug.
  6. Fix: one publisher thread (or mutex). The compactor submits "replace [A,B] with C". The publisher applies it only if [A,B] is still a contiguous sub-sequence of the *current* manifest, and keeps D. Include `generation` in MANIFEST and check it.
- **A9a. Name reuse vs gc [inference].**
  - Content-hash names plus BP's determinism goals mean identical inputs produce identical names.
  - Scenario 1: crash after `rename(seg-<gen>.tmp → seg-H.seg)` and before the manifest rename leaves orphan seg-H. On restart, the same wave is re-sealed, and its rename targets seg-H again (the name is reused). If a gc or startup sweep listed seg-H as unreferenced just before, it can delete it just *after* the new manifest references it. The manifest then points at a missing file, and recovery falls back or rebuilds.
  - Scenario 2: a blue/green rebuild of an unchanged repo writes byte-identical files under the *live* generation's names. That is exactly the "rename over a file a reader holds" case BP says cannot happen. On Windows the rename over a target that is mapped or held without FILE_SHARE_DELETE fails (see Q5).
  - Fix: names `seg-<gen>-<hash>` (unique per publish). gc runs only under the publish mutex and only on files older than an mtime grace, as git's `st_mtime > expire_time` check does.
- **A9b. Cross-process grace [inference].**
  - "After last in-process Arc drops" protects only the writer's own process. A second process that read MANIFEST gen N and is still reading layers (0.65–1.2 GB at 10M, seconds on HDD, or suspended by laptop sleep) can find a layer gone (Unix: ENOENT if it had not opened it yet; Windows: see Q5). The same applies to any lazy reader of change layers or an mmap opt-in.
  - git can afford a zero default grace because a git reader that loses the commit-graph falls back to parsing objects, which is slow but correct. FinalGitGraph's only fallback is "rebuild from git", which takes minutes at 10M.
  - Fix: specify a reader protocol (re-read MANIFEST and retry on ENOENT/ACCESS_DENIED, bounded) and a default grace in minutes, not zero.
- **A9c. MANIFEST.prev [inference].**
  - The specified sequence never creates MANIFEST.prev.
  - Option 1: `rename(MANIFEST → MANIFEST.prev)` before `rename(MANIFEST.lock → MANIFEST)`. This opens a window with no MANIFEST.
  - Option 2: a copy or hard link. This costs extra flushes and differs per OS.
  - Also, once gc deletes the layers that only `.prev` references, the fallback is dead. Concrete: after a base merge A+B→C, `.prev` = [A,B]. Once grace expires, A and B are deleted. Later C fails its checksum (bit rot, antivirus, a sync client), `.prev` fails, and a full rebuild follows.
  - Fix: gc keeps `.prev`'s layers. Accept that steady-state disk is then up to about 2x around merges.
- **A9d. Startup sweep by a non-holder [inference].** If the CLI starts while the app is between "rename seg" and "rename MANIFEST", its startup sweep deletes the app's just-renamed segment. Only the WRITER.lock holder may sweep, and only files older than the grace.
- **A10. Directory fsync [inference from primary sources].**
  - SE's own step 5 says "On Unix, fsync the directory". BP's sequence drops it.
  - With only file fsyncs, POSIX does not order the persistence of the two renames. A crash can persist the MANIFEST rename while losing the segment rename, which is ALICE's "files vanish" failure in LevelDB.
  - The result is detectable (missing file), then `.prev` or a rebuild. So the cost is availability, not wrong answers. It is low-probability on ext4's ordered journal (inference) and cheap to fix.
  - "Same semantics on every OS" is false: Unix gets a directory fsync, while Windows has no std equivalent (SE gap) and relies on NTFS metadata journaling.
- **A10b. Pedigree [inference from ALICE].**
  - BP cites ALICE as the reason to rely on "rename is atomic or detectably not". But the protocol it copies (temp + rename + pointer file update-via-rename) is the one ALICE found with "many ordering dependencies" (Git 9 unique vulnerabilities). The engine it removes (LMDB) had 1, and the best-known alternative (SQLite-WAL) had 0.
  - Caveat: ALICE tested 2014 versions under abstract persistence models. The checksum + rebuild design converts most vulnerabilities into rebuilds rather than corruption, *except* A1-type logical loss, which no checksum catches.
- **A11. MANIFEST.lock [inference].** If implemented literally as git's lockfile (O_EXCL), a SIGKILL or power loss leaves a stale `MANIFEST.lock`, and every later publish fails ("File exists"), as git's own `index.lock` does. If not O_EXCL, the git citation is misleading. Specify one.
- **A15. Disk-full mid-compaction [inference].**
  - A base merge at 10M needs free space for the merged output (about 0.65–1.25 GB for topology + metadata, more if change layers merge too) while the inputs, `.prev`'s layers and old snapshots are kept.
  - ENOSPC may surface at `write`, `sync_all` or close (delayed allocation).
  - Policy needed: free-space preflight, abort the merge, delete the tmp, keep the old generation, never retry a failed fsync on the same file. Rewrite instead (see Gaps on fsync error semantics).
- **Concurrent writers with an advisory lock [inference; mostly could not break].** Every writer is FinalGitGraph's own binary and takes WRITER.lock. std opens files with close-on-exec (inference: std uses `O_CLOEXEC`), so child `git` processes do not inherit the lock. The residual risk is non-cooperating writers: backup restores or sync clients writing into the cache dir. Checksums plus unique names catch those.

### Gaps
- PostgreSQL "fsyncgate" (a retried fsync can falsely succeed after an error on Linux) is relevant to A15. I did not fetch its primary source this session.
- I did not run an ALICE-style tool on the proposed protocol. The ordering hole in A10 is reasoned from POSIX and the ALICE findings, not reproduced.

---

## Q5. Windows and macOS semantics: rename, delete with open handles, sync_all mapping in Rust 1.94

### Takeaway
- `sync_all` maps as claimed: F_FULLFSYNC on Apple, FlushFileBuffers on Windows, fsync on other Unix.
- The Windows `rename` description is **wrong for 1.94** (and for current `main`). std calls `MoveFileExW` first and uses POSIX-semantics `FileRenameInfoEx` only after `ERROR_ACCESS_DENIED`. A `ERROR_SHARING_VIOLATION` from a handle opened without `FILE_SHARE_DELETE` (antivirus, Search indexer, backup tools, a non-Rust reader) gets no fallback, so the publish fails.
- With read(), FinalGitGraph's own readers close their handles, so they cannot block gc *indefinitely*. The mmap opt-in or third-party handles can.
- Publish needs the same retry loop as gc, which git-for-windows applies to both rename and unlink.

### Cited Findings
- **Rust 1.94.0 `rename`.** `if unsafe { c::MoveFileExW(old.as_ptr(), new.as_ptr(), c::MOVEFILE_REPLACE_EXISTING) } == 0 { … // if MoveFileExW fails with ERROR_ACCESS_DENIED then try to move the file while ignoring the readonly attribute. This is accomplished by calling SetFileInformationByHandle with FileRenameInfoEx. … Flags: FILE_RENAME_FLAG_REPLACE_IF_EXISTS | FILE_RENAME_FLAG_POSIX_SEMANTICS … } else { return Err(err) }`. [primary] — [windows.rs 1.94.0 L1311–1378](https://github.com/rust-lang/rust/blob/1.94.0/library/std/src/sys/fs/windows.rs)
- **Rust `main`** refactors this into `rename_inner(old, new, move_flags, rename_flags)` with the same order (MoveFileExW first), and adds `rename_noreplace`. SE cited this `master` shape and described it as POSIX-first. [primary] — [windows.rs main L1329–1410](https://github.com/rust-lang/rust/blob/main/library/std/src/sys/fs/windows.rs)
- **Rust 1.94.0 `unlink`.** `DeleteFileW` first. Only on `ERROR_ACCESS_DENIED` does it open with `DELETE` access and call `posix_delete` (`FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE`). Otherwise "return the original error". `posix_delete` is "supported for Windows 10 1607 (aka RS1) and later. However some filesystem drivers will not support it even then, e.g. FAT32." [primary] — [windows.rs 1.94.0 L845–875, L1288–1309](https://github.com/rust-lang/rust/blob/1.94.0/library/std/src/sys/fs/windows.rs)
- **Rust 1.94.0 defaults.** `share_mode: FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE` (L206). `fsync` calls `FlushFileBuffers` (L400–401). [primary] — same file
- **Rust 1.94.0 Unix.** `fsync` uses `fcntl(fd, F_FULLFSYNC)` on `target_vendor = "apple"` and `fsync(fd)` elsewhere. `datasync` uses F_FULLFSYNC on Apple and `fdatasync` on Linux/FreeBSD/Android and others. [primary] — [unix.rs 1.94.0 L1381–1420](https://github.com/rust-lang/rust/blob/1.94.0/library/std/src/sys/fs/unix.rs)
- **Microsoft DeleteFileW.**
  - "The DeleteFile function fails if an application attempts to delete a file that has other handles open for normal I/O or as a memory-mapped file (FILE_SHARE_DELETE must have been specified when other handles were opened)."
  - "The DeleteFile function marks a file for deletion on close. Therefore, the file deletion does not occur until the last handle to the file is closed. Subsequent calls to CreateFile to open the file fail with ERROR_ACCESS_DENIED."
  - Source: [primary] — [Microsoft Learn DeleteFileW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-deletefilew)
- **Microsoft FILE_RENAME_INFORMATION.** "FILE_RENAME_POSIX_SEMANTICS … If FILE_RENAME_REPLACE_IF_EXISTS is also specified, allow replacing a file even if there are existing handles to it. Existing handles to the replaced file continue to be valid for operations such as read and write." [primary] — [Microsoft Learn _FILE_RENAME_INFORMATION](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_rename_information)
- **git-for-windows.** `retry_ask_yes_no` with `static const int delay[] = { 0, 1, 10, 20, 40 };` and the comment "We assume that some other process had the file open at the wrong moment and retry." It is used by `mingw_unlink` ("Unlink of file '%s' failed. Should I try again?") *and* by `mingw_rename` ("Rename from '%s' to '%s' failed. Should I try again?"). [primary] — [git-for-windows compat/mingw.c ~L249–270, ~L628, ~L3145](https://github.com/git-for-windows/git/blob/main/compat/mingw.c)

### Inferences
- **A8. Publish failure on Windows [inference from the primary code].**
  - Case 1: a reader handle *with* FILE_SHARE_DELETE (any Rust reader) is open on MANIFEST during the rename. MoveFileExW is expected to fail with ACCESS_DENIED (not verified), std's POSIX-semantics retry runs, and per the Microsoft docs it succeeds. Fine.
  - Case 2: Defender, the Search indexer, a backup tool or a Python script (CRT `open` does not request FILE_SHARE_DELETE; not verified this session) holds MANIFEST or the `.tmp`. MoveFileExW can then return ERROR_SHARING_VIOLATION, std does not fall back, and the publish errors out.
  - BP's diagram puts "retry on Windows" only on gc. Git's Windows port needs retry for rename too.
- **Can a segment be blocked indefinitely? [inference].**
  - With read(), FinalGitGraph's handles live only during the read, so they cannot block gc indefinitely. I could not break this sub-claim.
  - It does break under (a) the mmap opt-in: a mapping opened without FILE_SHARE_DELETE makes DeleteFile fail for the mapping's lifetime, and even with FILE_SHARE_DELETE the name stays delete-pending until unmap; and (b) third-party long-lived handles.
  - A delete-pending name also blocks *re-creating* the same name: "Subsequent calls to CreateFile … fail with ERROR_ACCESS_DENIED". Combined with content-hash name reuse (A9a), a re-published identical segment collides with its own delete-pending ghost. Another reason for unique `seg-<gen>-<hash>` names.
- **A12. macOS flush cost [inference].**
  - Every publish costs 2 F_FULLFSYNCs (segment + manifest). Apple documents that F_FULLFSYNC "asks the drive to flush all buffered data" (quoted in SE). That is a device-wide flush, not a per-file one.
  - BP explicitly does not need durability. Ordering-only `F_BARRIERFSYNC` (noted in SE) would do, but std does not expose it. Calling libc `fcntl` needs `unsafe`, which needs an ADR.
  - The throughput section's 100k commits/s argument comes from `git rev-list`, which does no fsync per wave. Publish cost is missing from the throughput budget.

### Gaps
- I did not test which error code MoveFileExW returns when the target is open with FILE_SHARE_DELETE vs without it. Expectations above are labeled as such.
- I fetched no F_FULLFSYNC latency measurement on Apple SSDs this session, so A12 is unquantified.
- No primary Microsoft source on the power-loss durability of a rename on NTFS without a directory flush (same gap as SE).

---

## Q6. Are the cited version/date facts correct, and would keeping LMDB via heed, or redb/SQLite for everything, be simpler and lower-risk than a custom format?

### Takeaway
All dated facts I could check are correct:
- lmdb 0.8.0 on 2018-03-03;
- the lmdb-sys submodule at 60d5002;
- LMDB 0.9.36 on 2026/08/06;
- the Windows pre-extension on 0.9;
- redb's mmap removal in 0.14.0 (2023-03-26), and its 4.0.0 and 4.4.0 fixes;
- heed 0.23.1 (2026-10-26) on lmdb-master-sys 0.2.6;
- Kùzu archived, with its last push on 2025-10-10.

The *inferences* built on them are where it breaks:
- SE's "redb does support multi-process access with a single writer" is false for every released redb.
- The heed fallback needs `unsafe` and an unreleased LMDB branch.

Simpler alternatives exist that BP does not weigh:
- **SQLite WAL for all mutable state.** It is multi-process and ALICE 0. Keep immutable layer files for the graph.
- **Arrow IPC as the layer container.**

Both cut the custom surface to "immutable files plus a DB row naming them".

### Cited Findings
- **crates.io API, queried 2026-11-01 [primary].**
  - `lmdb` max 0.8.0, released 2018-03-03; the previous release was 0.7.2 on 2017-09-09. — [crates.io lmdb](https://crates.io/api/v1/crates/lmdb)
  - `heed` 0.23.1 (2026-10-26), 0.23.0 (2026-10-22), 0.22.1 (2026-04-07). — [crates.io heed](https://crates.io/api/v1/crates/heed)
  - `lmdb-master-sys` 0.2.6 (2026-04-07). — [crates.io lmdb-master-sys](https://crates.io/api/v1/crates/lmdb-master-sys)
  - `redb` max 4.4.0 (2026-10-13). There is a parallel maintenance branch, 2.6.4 (2026-10-02). — [crates.io redb](https://crates.io/api/v1/crates/redb)
- **lmdb-rs submodule.** `lmdb-sys/lmdb` is a submodule at `60d500206a108b2c64ca7e36b0113b2cd3711b98` → `https://github.com/LMDB/lmdb`. [primary] — [GitHub contents API](https://api.github.com/repos/danburkert/lmdb-rs/contents/lmdb-sys/lmdb?ref=master). That this commit is "Release 0.9.21" was not re-verified.
- **LMDB CHANGES.**
  - "LMDB 0.9.37 Engineering"; "LMDB 0.9.36 Release (2026/08/06)", including "ITS#10454 - fix metapage sync on Mac OSX" and "ITS#10538 - fix large writes on Windows".
  - 0.9.34 (2026/01/26) includes "ITS#10296 - fix fdatasync on MacOS" and "ITS#9564 - fix race condition freeing spilled pages at end of transaction".
  - Source: [primary] — [LMDB 0.9 CHANGES](https://raw.githubusercontent.com/LMDB/lmdb/mdb.RE/0.9/libraries/liblmdb/CHANGES)
- **LMDB 0.9 Windows.** "Windows won't create mappings for zero length files. and won't map more than the file size. Just set the maxsize right now." Then `if (!(flags & MDB_WRITEMAP) && (… SetEndOfFile …))`, followed by `CreateFileMapping(env->me_fd, …, sizehi, sizelo, …)` with the full map size. [primary] — [mdb.c mdb.RE/0.9 ~L4125–4145](https://raw.githubusercontent.com/LMDB/lmdb/mdb.RE/0.9/libraries/liblmdb/mdb.c)
- **redb CHANGELOG [primary]** — [redb CHANGELOG](https://github.com/cberner/redb/blob/master/CHANGELOG.md)
  - 0.14.0 (2023-03-26): mmap backend removed.
  - 3.0.0 (2025-08-09): "Multiple processes may open the same database file for reading by using the new `ReadOnlyDatabase` type … this acquires a shared lock on the database file."
  - 3.1.3 (2026-04-02): "Fix a data loss bug which can occur when the guard returned from `Table::get_mut()` is dropped after the transaction has been committed."
  - 4.0.0 (2026-04-02): "critical bug where the accessor could outlive the `Table` … This could cause data loss".
  - 4.3.0 (2026-09-14): "experimental support for multi-process read-write access … behind the `experimental-multiprocess` feature flag".
  - 4.4.0 (2026-10-13): fixes for "unbounded memory growth in the page cache", "`Durability::None` commits being lost on a clean close", and "`compact()` waiting forever".
  - 4.5.0 ("2026-XX-XX", unreleased): adds `ConcurrencyMode::SingleWriter`; "`ExclusiveWriter` (the default) locks the whole file, as earlier versions do".
- **heed.**
  - `pub unsafe fn open<P: AsRef<Path>>(&self, path: P)`, documented with "Memory map constructors are marked `unsafe` because of the potential for Undefined Behavior (UB) using the map if the underlying file is subsequently modified".
  - A global `OPENED_ENV` registry returns `Err(Error::EnvAlreadyOpened)` if the same canonical path is already open. The docs say "Calling `EnvOpenOptions::open` twice in the same process, at the same time is OK".
  - Source: [primary] — [heed env_open_options.rs ~L198–250, ~L412–440](https://github.com/meilisearch/heed/blob/main/heed/src/envs/env_open_options.rs), [heed envs/mod.rs L43](https://github.com/meilisearch/heed/blob/main/heed/src/envs/mod.rs)
- **Kùzu.** `archived: true`, `pushed_at: 2025-10-10T15:34:00Z`. [primary] — [GitHub API kuzudb/kuzu](https://api.github.com/repos/kuzudb/kuzu)
- **ALICE Table 3(a).** Unique static vulnerabilities: LevelDB 1.10: 10, LevelDB 1.15: 6, LMDB: 1, GDBM: 5, HSQLDB: 10, SQLite-Roll: 1, SQLite-WAL: 0, PostgreSQL: 1, Git: 9, Mercurial: 10, VMWare: 1, HDFS: 2, ZooKeeper: 4, total 60. [primary; extracted from flattened PDF text, and the column sum matches the paper's stated 60] — [ALICE PDF](https://www.usenix.org/system/files/conference/osdi14/osdi14-paper-pillai.pdf)
- **Format-revision precedent (already cited in BP).** git's `GDA2`/`GDO2` exist because "a previous version of Git wrote possibly erroneous data in these chunks with the IDs "GDAT" and "GDOV"". Bloom v1 filters "have a bug that appears when char is signed". — [gitformat-commit-graph](https://git-scm.com/docs/gitformat-commit-graph)

### Inferences
- **A7. redb fallback [inference from primary sources].**
  - BP requires "Other processes (a second app window, the CLI) open read-only snapshots from the same files without a daemon".
  - With any released redb (≤ 4.4.0), a writer process takes a whole-file lock, and `ReadOnlyDatabase` needs a shared lock. So the CLI cannot read while the app writes, and vice versa.
  - SE Q5's "redb does support multi-process access with a single writer" is false today. It becomes true only with the unreleased 4.5.0 SingleWriter mode, which follows an *experimental* 4.3.0 flag.
  - Together with three data-loss fixes in 2026 (3.1.3, 4.0.0, 4.4.0), this makes redb the weakest fallback for shared mutable state.
- **A7. heed fallback [inference].**
  - It fixes the double-open hazard (closing SE's Q1 gap: heed *does* guard it).
  - The cost: (a) `unsafe` at the open call site, so AGENTS.md rule 2 requires an ADR; (b) it depends on LMDB's `mdb.master` 1.0-dev branch, which has no release in the 0.9 CHANGES feed; (c) it keeps mmap semantics, so the Windows mapped-file rename and delete limits remain.
  - Still, for the *mutable KV* part, heed is a mature engine (ALICE: LMDB 1 vulnerability) vs a new manifest protocol (0 test history).
- **Missed alternative M1: "immutable layers + SQLite WAL catalog" [inference].**
  - Keep the immutable layer files, which are the right call for the CSR. Put the *catalog* in one SQLite WAL database: the layer list with checksums, tips, registry, derived coverage, attempt counts and status overrides.
  - Publishing a layer becomes: write + fsync the file (unique name), then one SQLite transaction that inserts the layer row and deletes merged rows, with a CAS on generation inside the transaction.
  - This removes MANIFEST.lock/.prev, the lost-update risk (A1, A6b), the full manifest rewrite for 100k refs (A6a) and the WRITER.lock scope problem (SQLite serializes writers across processes).
  - gc becomes "files not in the catalog and older than the grace", run by whoever holds SQLite's write lock.
  - Cost: a C dependency, which BP itself says is already satisfiable. Also, per the portability note: never rename or replace the live DB file.
- **Missed alternative M2: Arrow IPC as the layer container [inference].**
  - SE quotes Arrow IPC as little-endian, recommending 64-byte alignment, with a footer index and "Relocatable without 'pointer swizzling'".
  - Using it, or a strict subset, gives a spec'd format with external tooling (pyarrow, DuckDB) for debugging and fuzzing. BP Phase 7's "Arrow export" then comes free.
  - BP never compares a custom chunk format with Arrow. git's own format needed two meaning-level revisions (GDA2, Bloom v2) despite heavy review. That base rate argues for borrowing a spec'd container where possible.
- **"redb or heed for everything" [inference].** Neither makes the graph cheaper than immutable CSR layers. The KV per-key overhead argument in SE is sound, and I could not break it. The case for an engine is strongest for the *mutable* set, which is where BP chose "no DB".

### Gaps
- I did not verify that `60d5002` is tagged "Release 0.9.21" (taken from SE).
- I did not verify that LMDB 1.0 remains unreleased beyond the absence of a release in the 0.9 CHANGES feed and heed's dependence on `mdb.master`.
- No crash-consistency torture test exists for redb 4.x or for the proposed manifest protocol (same as SE's gap).

### Claims tested and NOT broken (so the judge knows they were tried)
- lmdb 0.8.0 released 2018-03-03 (crates.io API).
- lmdb-sys submodule commit 60d5002 (GitHub API).
- LMDB 0.9.36 released 2026/08/06 and 0.9.37 in engineering, about 15 point releases after 0.9.21. The recent macOS sync and Windows large-write fixes *strengthen* BP's "old C code" point.
- Windows pre-extension to the map size on the 0.9 branch (mdb.c). The WRITEMAP path skips `SetEndOfFile` but still maps the full size.
- redb mmap removal in 0.14.0 on 2023-03-26. BP's "in 2023" is right; only the "at most ~2x" paraphrase overreaches (A5).
- redb data-loss fixes in 4.0.0 and the 4.4.0 fixes, as quoted.
- heed 0.23.1 (2026-10-26) on lmdb-master-sys 0.2.6. heed rejects a double-open with `EnvAlreadyOpened`.
- Kùzu archived; last push 2025-10-10.
- `sync_all` maps to F_FULLFSYNC (Apple), FlushFileBuffers (Windows) and fsync (other Unix) at Rust 1.94.0. std's default share mode includes FILE_SHARE_DELETE. `File::try_lock` maps to flock/LockFileEx.
- FILE_RENAME_POSIX_SEMANTICS lets existing handles stay valid (Microsoft docs). Once std's fallback reaches it, the "readers keep the old file" property holds.
- The xxh3 cost at open is negligible next to I/O (xxHash README: 31.5–59.4 GB/s).
- As pure arithmetic, 36 B × 10M = 360 MB, and 611 waves × about 180 MB ≈ 110 GB. 3.5 GB is within 1.4x of the simulated 2.5 GB at the same 36 B/commit.
- KV per-key overhead vs dense arrays for an append-only DAG. The immutable CSR layer *idea* survives; the attacks target the protocol and the numbers.
- "C toolchain present wherever Rust links" (MSVC Build Tools / Xcode CLT / cc). Only cross-compilation needs an extra cross C compiler (inference).
- "The index is a rebuildable cache, so durability loss is acceptable." It holds for graph and derived data. It fails only for the registry (A6f).
