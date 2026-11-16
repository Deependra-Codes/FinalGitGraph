# Red-team review: incremental-update, concurrency, process-model and crash-consistency claims of the FinalGitGraph blueprint

Targets:
- `reports/FinalGitGraph backend architecture blueprint.md`. Called "BP" below, with line numbers.
- `research_notes/FinalGitGraph backend architecture blueprint/incremental_concurrency_consistency.md`. Called "ICC" below, with line numbers.
- The other prior notes, where they contradict these two.
- The current code in `apps/desktop/src-tauri/src/application/indexer.rs`, `adapters/db.rs`, `adapters/git.rs` and `Cargo.toml`. All code was read, not run.

Research date: 2026-11-01. "Inference" marks reasoning or a constructed failure scenario, not a sourced fact. Severity ranks the consequence: **critical** means silent corruption, wrong answers or a double writer. **major** means a failure that users will hit, or a claim that is wrong as stated. **minor** means cost, clarity, or a missing detail.

---

## Q0. Ranked attack list: which claims break, and how badly?

### Takeaway
The single-writer, immutable-layer, snapshot-reader shape holds up. Its cross-process edges do not. Two attacks are critical. First, a GC "grace window" with no reader registration is not safe for readers in other processes, and the git precedent cited for it works only because git's commit-graph is optional. Second, a path-based `WRITER.lock` that can be deleted lets two writers run at once. Several major attacks follow:
- in-process hosting gives no crash isolation (Rust aborts the process on OOM);
- dense IDs, and so user-visible topological order, depend on ingest history, which contradicts the "pure function of the DAG" claim;
- network and virtualized filesystems break `flock`;
- a `MANIFEST.prev` fallback that GC can quietly invalidate;
- visibility and index growth after force-pushes;
- blue/green disk pressure;
- a priority mechanism ("shrink waves") that does not act on query latency.

### Cited Findings
- [F1] git deletes unused split commit-graph files after a write when their mtime is older than `--expire-time`, whose default is "the current time", i.e. no grace at all. — [git-commit-graph docs](https://git-scm.com/docs/git-commit-graph)
- [F2] When a chain names a missing layer, git prints `warning(_("unable to find all commit-graph files"))`, stops, and returns the **partial chain** loaded so far. The lower layers stay in use. Commits missing from the graph are then parsed from the object database. — [git commit-graph.c](https://github.com/git/git/blob/master/commit-graph.c) (code read this session via raw GitHub)
- [F3] Tantivy uses two lock files. `.tantivy-writer.lock` is non-blocking ("Only one process should be able to write tantivy's index at a time"). `.tantivy-meta.lock` is blocking: "The meta lock file is here to protect the segment files being opened by `IndexReader::reload()` from being garbage collected. It makes it possible for another process to safely consume our index in-writing. Ideally, we may have preferred `RWLock` semantics here, but it is difficult to achieve on Windows. Opening segment readers is a very fast process." Its writer-lock doc also says "If the process is killed and this file remains, it is safe to remove it manually", and that depending on platform "the lock might rely on the creation and deletion of this filepath." — [tantivy directory_lock.rs](https://github.com/quickwit-oss/tantivy/blob/main/src/directory/directory_lock.rs)
- [F4] Rust `File::lock`/`try_lock`/`lock_shared` have been stable since 1.89.0. `try_lock` returns `Result<(), TryLockError>` with `WouldBlock`. "No *other* file handle to this file, in this or any other process, may acquire another lock." "This lock may be advisory or mandatory." It is released "when this file (along with any other file descriptors/handles duplicated or inherited from it) is closed, or if the `unlock` method is called." The docs say nothing about NFS or network filesystems. — [std::fs::File](https://doc.rust-lang.org/std/fs/struct.File.html)
- [F5] In std's source, Unix `try_lock` is `flock(fd, LOCK_EX | LOCK_NB)`. Targets outside a fixed list (linux, android, apple, the BSDs, illumos and others) return `Unsupported`. Apple `fsync` and `datasync` both call `fcntl(fd, F_FULLFSYNC)`. — [rust std sys/fs/unix.rs](https://github.com/rust-lang/rust/blob/master/library/std/src/sys/fs/unix.rs)
- [F6] In std's Windows source:
  - The default share mode is `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE`.
  - `rename` first calls `MoveFileExW(MOVEFILE_REPLACE_EXISTING)`. Only on `ACCESS_DENIED` does it fall back to `SetFileInformationByHandle(FileRenameInfoEx)` with `REPLACE_IF_EXISTS | POSIX_SEMANTICS`.
  - `unlink` calls `DeleteFileW` and falls back to a POSIX-semantics delete on `ACCESS_DENIED`.
  - `lock` is `LockFileEx` over offset 0, length `u32::MAX, u32::MAX` (the whole file). `try_lock` maps `ERROR_LOCK_VIOLATION` to `WouldBlock`.
  - `fsync` is `FlushFileBuffers`, and `datasync` calls `fsync`.
  
  — [rust std sys/fs/windows.rs](https://github.com/rust-lang/rust/blob/master/library/std/src/sys/fs/windows.rs)
- [F7] flock(2) on Linux:
  - Locks "are associated with an open file description", and duplicates made by `fork`/`dup` "refer to the same lock".
  - "flock() places advisory locks only."
  - "Since Linux 2.6.12, NFS clients support flock() locks by emulating them as fcntl(2) byte-range locks on the entire file."
  - Up to Linux 5.4, "flock() is not propagated over SMB". Since 5.5 it is emulated with SMB byte-range locks and is no longer advisory: "any IO on a locked file will always fail with EACCES when done from a separate file descriptor."
  
  — [flock(2)](https://man7.org/linux/man-pages/man2/flock.2.html)
- [F8] jj is lock-free by design: "To avoid depending on lock files, Jujutsu takes a different approach." "One problem with using lock files is that they don't work when the clone is in a distributed file system." The view "contains the set of visible head commits, bookmarks, tags, and the working-copy commit in each workspace". Concurrent operations are merged three-way, and conflicts are recorded. Caveat: "repository corruption is possible because the backend is not entirely lock-free". — [jj concurrency](https://docs.jj-vcs.dev/latest/technical/concurrency/)
- [F9] `handle_alloc_error`: if the binary links `std`, it will "print a message to standard error and abort the process". This "can be replaced with `set_alloc_error_hook`", which the page links to (stability not shown on the page). — [std::alloc::handle_alloc_error](https://doc.rust-lang.org/std/alloc/fn.handle_alloc_error.html)
- [F10] Rust 1.81: "the non-unwind ABIs (e.g., "C") will now abort on uncaught unwinds". — [Rust 1.81.0 release post](https://blog.rust-lang.org/2024/09/05/Rust-1.81.0/)
- [F11] Tauri v2: "Commands without the *async* keyword are executed on the main thread unless defined with `#[tauri::command(async)]`." Async commands run "using `async_runtime::spawn`". The page says nothing about panics. — [Tauri v2 Calling Rust](https://v2.tauri.app/develop/calling-rust/)
- [F12] Rayon `ThreadPool::install`: "If the current thread is part of a different thread pool, it will try to keep busy while the `op` completes in its target pool, similar to calling `ThreadPool::yield_now()` in a loop. Therefore, it may potentially schedule other tasks to run on the current thread in the meantime." Yields are "cooperative". The page documents no task priority or preemption. — [rayon ThreadPool](https://docs.rs/rayon/latest/rayon/struct.ThreadPool.html)
- [F13] Windows "Access is denied (os error 5)" during renames is widely attributed to antivirus holding files. One rustup report fixed it by turning off McAfee On-Access Scanner (a rename under `.rustup\tmp`). — [rustup issue #1912](https://github.com/rust-lang/rustup.rs/issues/1912); [users.rust-lang.org thread](https://users.rust-lang.org/t/rust-installer-causing-error-access-is-denied-os-error-5/92625) (search-result summaries; anecdotal)
- [F14] On WSL2, `/mnt/c` is a 9p (plan9/virtio-plan9/virtiofs) mount, not a native Linux filesystem. — [wsl.dev drvfs](https://wsl.dev/technical-documentation/drvfs/) (search-result snippet only)
- [F15] Repo facts, read 2026-11-01:
  - `Cargo.toml` (workspace and `apps/desktop/src-tauri`) has **no `tauri` dependency** and no `[profile]` section, so the default is `panic = "unwind"`.
  - `indexer.rs:53-54` defines a process-global `static RUNTIME_QUERY_GRAPH_CACHE: OnceLock<Mutex<…>>`. `indexer.rs:1339-1355` treats a poisoned mutex as a cache miss (`let Ok(guard) = cache.lock() else { return None; }`), so after one panic the cache is silently disabled for the life of the process.
  - `indexer.rs:509-511` turns a writer-thread panic into `AppError::Parse("commit ingest writer thread panicked")`.
  - `indexer.rs:403-404` runs `clear_repository_index_data` before a full index.
  - `git.rs:2099-2107` `load_refs` reads only `refs/heads/` and `refs/tags/`: no `HEAD`, remotes, stash or worktree HEADs.
- [F16] Contradictions between the prior notes, found by grep:
  - `frontier_novel_architectures.md` (D9, about line 223) says many processes "mmap the same index concurrently", but BP:129 says layers are `read()`, not mmapped.
  - `portability_testing_verification.md` (about line 42, line 65) puts data in `appLocalDataDir`/`data_local_dir`, but BP:133 and ICC:253 say "OS cache directory".

### Inferences (the ranked attacks)

**A1. CRITICAL: cross-process GC "grace window" without reader registration**
- Claim: BP:104 "gc : delete unreferenced files after last in-process Arc drops + grace window; retry on Windows". BP:133 "Other processes … open read-only snapshots from the same files without a daemon". ICC:159 "Cross-process, use an expiry window (git-style mtime grace)". ICC:296.
- Attack (interleaving):
  1. Reader process R (the CLI or a second app instance) reads MANIFEST g7 = [L1, L2, L3].
  2. Writer W merges L2+L3 into L4, publishes g8 = [L1, L4], and marks L2 and L3 unreferenced.
  3. R stalls. Causes include laptop sleep, SIGSTOP or a debugger, an antivirus scan, a slow disk, or simply reading about 360 MB of topology layers.
  4. The grace window expires and GC deletes L2.
  5. R opens L2 and gets ENOENT.
- Consequences: R either errors, or, following BP's own recovery rule (BP:127, "falls back to MANIFEST.prev … rebuilds from git"), starts a needless rebuild that it cannot run without the writer lock. The "last in-process Arc" condition says nothing about other processes. No finite grace window bounds a suspended process.
- Precedent misread: git's default grace is zero [F1]. git survives missing layers because the commit-graph is an optional accelerator, and a partial chain degrades to parsing objects [F2]. In FinalGitGraph the layers are the only copy of the index, so a missing layer means no answer.
- Revision: adopt Tantivy's answer [F3] with a real RW lock, which std now provides on all three OSes [F4][F6]:
  1. The reader takes `lock_shared` on `META.lock`, reads MANIFEST, opens every layer handle, then unlocks.
  2. GC takes the exclusive `lock` before unlinking.
  3. After the handles are open, deletion is harmless. On Unix an unlinked file stays readable. On Windows std opens with `FILE_SHARE_DELETE` and falls back to a POSIX-semantics delete [F6], provided layers are `read()`, not mmapped.
  4. Add an optimistic fallback: on ENOENT, re-read MANIFEST and retry, bounded, before any fallback or rebuild.
  5. Never let a reader process trigger a rebuild.

**A2. CRITICAL: the writer lock is bound to a path that can be deleted, so two writers can run**
- Claim: BP:96 and BP:133 "holds index_dir/WRITER.lock via File::try_lock … released by the kernel on crash". ICC:252 prefers it over an O_EXCL lockfile.
- Attack: flock state lives on the open file description, not on the path [F7]. Rust says nothing about path identity [F4]. Scenario:
  1. App A holds the lock on inode X.
  2. The index lives in an OS *cache* directory (BP:133), which by convention is safe to delete. Cleaners, `rm -rf ~/.cache/<id>`, an in-app "clear cache" button, or a blue/green step that removes a generation directory holding the lock all delete it.
  3. CLI B recreates `index_dir` and `WRITER.lock` (inode Y) and gets `try_lock` → `Ok(())`.
  4. A's later path-based renames now land in B's directory.
  5. Both writers assign dense IDs from their own view and publish MANIFESTs alternately.
- Every layer passes its own checksum, but the ID spaces collide. BP's detection model ("checksums + MANIFEST.prev", BP:127) cannot see this, which is why it is critical: silent, valid-looking corruption.
- Revision:
  1. After locking, compare (dev, inode) or the Windows file ID of the handle against the path, and re-check before every publish (standard practice; not sourced this session).
  2. Keep the lock file at a stable path that is never deleted.
  3. Put `writer_epoch` and `parent_manifest_hash` in the manifest. Refuse to publish unless the current MANIFEST's hash equals the last one this writer published (compare-and-swap by verification).

**A3. MAJOR: in-process hosting gives no crash isolation; "released by the kernel on crash" covers process death only**
- Claim: BP:182 "Tauri in-process hosting — Keep; a daemon is deferred". BP:133. ICC:237 and ICC:254.
- Attack:
  - (a) Rust aborts the whole process on allocation failure by default [F9]. An OOM while indexing 10M commits (frontier maps, change layers read into heap per BP:129) kills the UI with it.
  - (b) Since 1.81 a panic that unwinds into an `extern "C"` frame aborts [F10], and synchronous Tauri commands run on the main thread [F11]. *Inference, not verified for tao/wry:* a panic in a sync command whose call path crosses a C-ABI event-loop callback aborts the app, and stack overflow is never catchable.
  - (c) A panic in the writer *thread* leaves the `File` that holds the lock owned by a live process. No other process can take over, and indexing silently stops unless something supervises it. Today's code already has this shape: the panic becomes an error at `indexer.rs:509-511`, and a poisoned cache mutex silently disables caching forever (`indexer.rs:1339-1355`) [F15].
  - (d) "Keep" is the wrong word: the repo has **no Tauri dependency** [F15], so there is nothing in-process to keep.
  - (e) The cited precedents are out-of-process relative to their UIs. rust-analyzer is a separate LSP server, and it puts proc macros in yet another process "for isolation" (ICC:243). Watchman is a separate server (ICC:242).
- Revision (a missed alternative that is not a daemon): a **split process**.
  - The UI process keeps an in-process *reader* (files are the API, per frontier D9).
  - A child worker process, launched from the same binary with a `--worker` mode or as a Tauri sidecar, owns `WRITER.lock` and does ingest, derive, compaction and GC.
  - IPC is only progress events and "new manifest gen".
  - OOM or abort in the worker leaves the UI alive, and the kernel releases the lock.
  - If the team stays in-process, at minimum: a supervisor that restarts the writer thread and releases and re-takes the lock; `catch_unwind` at each command boundary; no sync Tauri commands.

**A4. MAJOR: dense IDs, and so user-visible order, are a function of ingest history, not of the DAG**
- Claim: BP:137 "Topological streaming … iterate descending IDs over the visible set". BP:208 "logical content is a pure function of the reachable commit DAG … invariant to ingestion batching". BP:174 auto-tune.
- Attack (concrete):
  - Run 1: only `main` exists, giving R=0, A=1.
  - Run 2: `feature`, with commit B a child of R, appears and gets B=2.
  - A fresh full ingest of the same final DAG, run as a DFS from the tips [feature, main], can give R=0, B=1, A=2.
  - The topological stream (descending ID) and the lane layout therefore differ between "incremental" and "full" for the same DAG.
- The digest passes, because it is defined over OID-sorted records and "does not depend on IDs" (BP:198). So the metamorphic contract never checks user-visible output.
- If numbering is done per wave, auto-tuned wave sizes (driven by RAM) also change order between machines.
- Blue/green renumbers everything, which invalidates UI selections and pagination cursors that hold IDs.
- Revision:
  1. Treat dense IDs as internal.
  2. Define a canonical, ingest-independent output order, e.g. topological with a deterministic tiebreak such as (generation, committer date, OID).
  3. Key cursors by OID plus generation.
  4. Add metamorphic tests on *query outputs* (topo page 1..k), not only on the content digest.

**A5. MAJOR: "OS cache dir" does not guarantee a local filesystem, and flock semantics change on NFS, SMB, 9p and VM mounts**
- Claim: ICC:253 "keep the index on local disk in the OS cache directory". BP:133.
- Attack:
  - Linux `$XDG_CACHE_HOME` defaults to `~/.cache`, which is on NFS wherever home directories are NFS-mounted (common at universities, in HPC and in enterprises).
  - On NFS, flock becomes an fcntl byte-range lock [F7]. Fcntl/POSIX record locks belong to the process (from general knowledge, fcntl(2) not fetched this session). So two `Engine`s in one process may both "hold" the lock, and lock-lease or server-reboot behaviour is outside the design.
  - On SMB before Linux 5.4 the lock is not propagated at all, so two clients can both acquire it [F7].
  - WSL2 tools that open the Windows index via `/mnt/c` go through 9p [F14]. Whether Linux flock there is visible to a Windows `LockFileEx` is **unverified**. The design must assume it is not.
  - LMDB forbids remote filesystems "even between processes on the same host" (ICC:240), and jj says lock files "don't work … in a distributed file system" [F8].
- Revision:
  - At open, detect the filesystem type (`statfs` f_type on Linux and macOS for NFS, SMB/CIFS, FUSE and 9p; drive type on Windows).
  - Refuse writer mode on a remote filesystem, or fall back to a non-shared single-process mode with a clear message. A2's compare-and-swap check is the second line of defence.
  - Allow an override to a known-local directory.

**A6. MAJOR: `MANIFEST.prev` fallback is unspecified and can be invalidated**
- Claim: BP:127 "falls back to `MANIFEST.prev`". ICC:152-154.
- Attack:
  - (a) If `prev` is produced by renaming `MANIFEST → MANIFEST.prev` and then `MANIFEST.lock → MANIFEST`, there is a window with **no MANIFEST**. A crash there, or another process reading then, sees "no index" and may start a full rebuild (inference).
  - (b) GC is defined over "unreferenced" files (BP:104). Unless `prev`'s layers count as live, GC deletes them and the fallback points at missing files.
  - (c) The publish sequence (BP:102, BP:127) puts no directory fsync between the segment rename and the manifest rename. On Linux the durability of a rename needs one (ICC:153 says this; BP omits it). After a power loss the MANIFEST can name a segment whose rename was lost. That is detected, and then it relies on (b).
- Revision:
  - Use generation-numbered immutable manifests (`MANIFEST-<gen>`) plus a tiny `CURRENT` pointer replaced by rename. This is LevelDB-style, from general knowledge, not sourced this session.
  - Keep the last k generations live for GC.
  - fsync the directory after the segment renames and before the pointer swap.

**A7. MAJOR: visibility after a force-push is undefined in cost and scope, and the index grows forever**
- Claim: BP:143 "Force-pushed-away commits keep their per-OID data and drop out of the visible set computed from current tips, so re-pushing the same OIDs costs nothing."
- Attack:
  - (a) Scope: which refs define visibility? BP does not say. Today's code indexes only heads and tags [F15], so a detached HEAD, remote-tracking refs, stash and per-worktree HEADs are invisible.
  - (b) Cost: after any non-fast-forward update, visibility is a reachability recompute. With parent-first IDs it is one descending-ID sweep over a 1.25 MB bitset per 10M commits, an estimated tens of ms (inference, unmeasured). That is fine *if* it runs only on non-fast-forward ref changes and is memoized. BP specifies neither. Sapling gets ancestry in "O(number-of-merges)" with segments (BP:137), which BP defers.
  - (c) Growth: rebase, amend and `refs/pull/*` churn create new OIDs constantly. Dead commits are never removed, because IDs are positions, so removal means renumbering and a full blue/green rebuild. "Re-pushing the same OIDs" is the rare case; ever-growing dead data is the common one.
  - (d) Every query must intersect with the visibility bitset, which is a hidden cost on every call.
- Revision:
  - Specify the ref scope (default: `HEAD` of every worktree, `refs/heads`, `refs/remotes`, `refs/tags`; `refs/pull` opt-in).
  - Check for fast-forwards with `is_ancestor` per changed ref, and recompute only otherwise.
  - Memoize the visibility bitset by ref-snapshot hash.
  - Trigger a renumbering compaction when dead/N exceeds a pre-registered threshold.

**A8. MAJOR: blue/green full reindex: disk, freshness and readers**
- Claim: BP:127 "A full re-index becomes blue/green". ICC:157.
- Attack:
  - BP sizes only topology (about 360 MB at 10M commits). Change layers (`fc@v1`) are not sized and plausibly take several GB (inference).
  - Peak disk is blue + green + the merge temp of the largest layer. A derived-version bump "backfilled while old-version data keeps serving" (ICC:297) adds yet another copy.
  - ENOSPC mid-build: partial green files must be deleted at once, not after a grace window, or the disk stays full.
  - While green holds the single writer lock for minutes, blue receives no incremental updates, so the UI goes stale. BP defines no catch-up from green's ref snapshot.
  - After the swap, GC of the whole blue generation is the A1 race at full scale.
- Revision:
  - Preflight free space (`statvfs` / `GetDiskFreeSpaceExW`) against an estimated green size times a margin.
  - When space is short, accept "delete blue, then rebuild": it is a rebuildable cache, so the "never clear first" rule should yield to disk limits, after warning the user.
  - Record green's ref snapshot and run an incremental catch-up immediately after the swap.
  - Use the A1 reader protocol for the swap.

**A9. MAJOR: "shrink waves when the UI is busy" does not bound query latency**
- Claim: ICC:192 "Priority by construction … shrinks its wave size or sleeps briefly while UI queries are in flight". ICC:190 is the separate interactive pool.
- Attack:
  - Readers never wait on the writer (ArcSwap), so wave size changes *freshness*, not query latency.
  - Latency comes from CPU, memory-bandwidth and IO contention, and wave size controls none of these once a `par_iter` of tree diffs is in flight.
  - Rayon has no priority or preemption, and its pool size is fixed [F12]. Two pools at equal OS priority get only a fair share.
  - A query that calls `interactive.install()` from an indexer-pool thread (or the reverse) "may potentially schedule other tasks to run on the current thread in the meantime" [F12]. Its latency then includes arbitrary diff jobs.
  - Each publish calls `sync_all`, which is `F_FULLFSYNC` on Apple and `FlushFileBuffers` on Windows [F5][F6]. *Inference:* a full flush of the drive cache can stall concurrent reads.
  - *Inference:* the last `Arc` drop of an old snapshot frees hundreds of MB on whichever thread drops it, possibly a query thread.
  - A long-running query pins old layers, so compaction can double resident memory.
  - Worst-case latency: **unbounded by design and unmeasured**.
- Revision:
  - Split derive work into small tasks and check an "interactive pending" flag between tasks.
  - Lower OS priority on indexer threads (macOS QoS works per ICC:180; other OS APIs unverified).
  - Never `install` across pools from worker threads.
  - Drop snapshots on a reclaimer thread.
  - Make p99 query latency during ingest plus compaction a gated metric.

**A10. MAJOR/MINOR: the ref snapshot at scale, and TOCTOU**
- Claim: BP:143 "The manifest stores `{ref -> oid}`". BP:127 "Only the small manifest is ever replaced".
- Attack:
  - Mirror clones with `refs/pull/*`, or Gerrit `refs/changes/*`, can carry 100k+ refs. The manifest is then MBs, rewritten and fsynced on every wave publish, which is O(refs) per wave (inference).
  - TOCTOU: refs read at T0, then a concurrent `git fetch` plus `gc --prune=now` deletes objects of now-unreachable commits mid-walk. The run fails on a missing object, and BP gives no retry rule. The fixture matrix does include "repack mid-ingest" (BP:198).
  - The manifest must store exactly the T0 snapshot the walk used, never a re-read at publish time. BP does not say so.
  - A symbolic `HEAD` duplicates `refs/heads/main`. Per-worktree HEADs are not covered.
  - OID→ID lookups during the frontier walk cost O(new commits × parents × layers × log n). That is acceptable (inference).
- Revision:
  - Store the ref snapshot as a separate content-addressed file referenced by hash, rewritten only when refs change.
  - Detect changes from packed-refs/reftable stat plus a loose-ref scan.
  - On a missing object during the walk, refresh the ODB and refs, then restart from the new snapshot.

**A11. MINOR/MAJOR: a transient failure becomes a permanent `unavailable` in an immutable layer**
- Claim: BP:141 "A commit whose objects are missing … gets an explicit `unavailable` status in the layer"; "pending = indexed IDs minus IDs covered".
- Attack: once written into an immutable layer, an IO error, an antivirus sharing violation [F13] or a pack replaced mid-repack is permanent until the derived-type version is bumped.
- Revision: record `unavailable` only after an ODB refresh and re-lookup confirms absence. Leave transient failures uncovered so the next start retries them.

**A12. MINOR: durability cost and a mis-stated Windows rename**
- BP:127 says the design wants "atomicity and detectability rather than strict durability". Yet `sync_all` on Apple is `F_FULLFSYNC` [F5], twice per wave and about 625 waves at 10M commits. That is full durability paid for a rebuildable cache, which is internally inconsistent. Consider fsync only at checkpoints.
- BP:127 says std's Windows rename "uses POSIX-semantics `FileRenameInfoEx`". In fact `MoveFileExW` comes first and `FileRenameInfoEx` is only the `ACCESS_DENIED` fallback [F6].
- Antivirus can still make the manifest rename fail transiently [F13]. BP has retry only for GC (BP:104). Add bounded retry with backoff on publish.

**A13. MINOR: the DBSP "−1 on force-push" input is not incremental**
- ICC:58 and BP:143. The −1 delta needs old_visible − new_visible, which is the A7 recompute. A commit still reachable from another ref must not get −1, which is a `distinct` over reachability.
- Theorem 3.3 (linear operators) applies only after that step.
- Revision: compute the delta as an XOR of visibility bitsets, then feed it to linear aggregates.

**A14. MINOR: auto-tune makes failures machine-dependent**
- BP:174, ICC:223. If invariance holds, auto-tune cannot change the digest. But:
  - it makes a batch-boundary bug show up only on machines with a given amount of RAM;
  - it changes physical layer boundaries, which breaks the deferred "byte-identical compacted layers" root (BP:208) unless compaction is canonical;
  - it feeds A4.
- Profiles are testable if `tune(HardwareFacts, RepoFacts) -> IngestConfig` is a pure, logged function. Tests should enumerate its boundary outputs and replay logged configs.

**A15. MINOR: lock lifetime and version skew**
- An Engine holds the lock for its whole life (BP:96), so the CLI can never index while the app is open, even when the app is idle.
- An older CLI reading a newer-format index must refuse to read and must never run GC.
- Revision: state "only the lock holder runs GC", and put a format version in the pointer file.

**A16. MINOR: contradictions between notes** [F16]
- The frontier note says many processes share mmap; BP says `read()`.
- The portability note says "local data dir"; BP says "cache dir". A cache dir is semantically deletable, which feeds A2.
- Pick one, data-local, and record it in the ADR.

### Gaps
- The Docker Desktop bind-mount (virtiofs/gRPC FUSE) flock behaviour between host and container was **not found**. The search results covered only visibility and ownership bugs ([docker/for-mac #7274](https://www.github.com/docker/for-mac/issues/7274)).
- WSL2 9p lock interop with Windows `LockFileEx` was **not found** ([microsoft/WSL #4557](https://github.com/microsoft/WSL/issues/4557) turned out to be unrelated).
- fcntl(2) per-process lock semantics, LevelDB's CURRENT/MANIFEST pattern, Apple `F_BARRIERFSYNC`, and git's `reprepare_packed_git` retry were not fetched this session. They are used only inside inferences.
- No measurement exists for the visibility sweep cost, change-layer size at 10M commits, or query p99 under ingest.

---

## Q1. File::try_lock: version, semantics, filesystems, and whether the cache dir is enough

### Takeaway
BP's facts about the API are right: 1.89, `flock` on Unix, `LockFileEx` on Windows, released when all handles close. Its safety argument is incomplete. The lock is advisory on Linux, bound to an inode rather than a path, changes semantics on NFS and SMB, and has unknown semantics across the WSL2 and Docker VM boundaries. The cache dir does not guarantee a local disk.

### Cited Findings
- Stable since 1.89.0. `try_lock -> Result<(), TryLockError>`. Locks "may be advisory or mandatory". They are released when the handle and all duplicates close. No NFS statement. — [std::fs::File](https://doc.rust-lang.org/std/fs/struct.File.html)
- Unix uses `flock(LOCK_EX|LOCK_NB)`. Unsupported targets return `Unsupported`. — [unix.rs](https://github.com/rust-lang/rust/blob/master/library/std/src/sys/fs/unix.rs)
- Windows uses `LockFileEx` over the whole file (`u32::MAX, u32::MAX`), and `ERROR_LOCK_VIOLATION` maps to `WouldBlock`. — [windows.rs](https://github.com/rust-lang/rust/blob/master/library/std/src/sys/fs/windows.rs)
- flock is advisory on Linux and tied to the open file description. On NFS it is emulated as an fcntl whole-file lock (since 2.6.12). On SMB it is not propagated up to 5.4, and since 5.5 it is mandatory, with EACCES for IO from other descriptors. — [flock(2)](https://man7.org/linux/man-pages/man2/flock.2.html)
- WSL2 `/mnt/c` is 9p. — [wsl.dev drvfs](https://wsl.dev/technical-documentation/drvfs/) (snippet)
- jj avoids lock files because they fail on distributed filesystems. — [jj concurrency](https://docs.jj-vcs.dev/latest/technical/concurrency/)

### Inferences
- Advisory locking is acceptable *only* because every writer is the project's own binary. Any tool that writes without the lock, including an old app version with a different lock name, is unprotected. That makes the compare-and-swap manifest check of A2 the real guard.
- On native filesystems, two handles in one process conflict [std docs], so a second `Engine` in the same process is detected. Over NFS emulation that likely stops holding, because fcntl locks are per-process (unverified this session).
- The cache dir is not enough. Add filesystem-type detection, a remote-filesystem refusal, and a lock-identity check (A2, A5).

### Gaps
- No primary source was found for flock propagation in Docker Desktop or WSL2 9p.

---

## Q2. Multi-process readers vs compaction/GC: is a grace window safe? How do git, Tantivy and LMDB do it? Windows?

### Takeaway
No. Each precedent uses something BP lacks. git can tolerate missing layers. Tantivy has a reader-side meta lock. LMDB has a shared reader table. On Windows, deleting a file that a reader holds open works with std's defaults if layers are `read()`, not mmapped.

### Cited Findings
- git: grace defaults to "the current time" (zero). A missing layer gives a warning and a partial chain. — [git-commit-graph](https://git-scm.com/docs/git-commit-graph); [commit-graph.c](https://github.com/git/git/blob/master/commit-graph.c)
- Tantivy: `.tantivy-meta.lock` (blocking) protects segment files during `IndexReader::reload()` from GC "for another process to safely consume our index in-writing". RW semantics were wanted "but it is difficult to achieve on Windows". — [directory_lock.rs](https://github.com/quickwit-oss/tantivy/blob/main/src/directory/directory_lock.rs)
- LMDB: "Stale reader transactions left behind by an aborted program cause further writes to grow the database quickly". The fix is `mdb_reader_check`. That is a shared reader table which pins pages. — [lmdb.h](https://raw.githubusercontent.com/LMDB/lmdb/mdb.master/libraries/liblmdb/lmdb.h) (as cited in ICC:104)
- std on Windows opens with `FILE_SHARE_DELETE` by default, and `unlink` falls back to a POSIX-semantics delete on `ACCESS_DENIED`. — [windows.rs](https://github.com/rust-lang/rust/blob/master/library/std/src/sys/fs/windows.rs)
- Windows refuses to delete a file that is memory-mapped. — as cited in ICC:146 ([OSR thread](https://community.osr.com/t/deleting-an-opened-memory-mapped-file/19414))

### Inferences
- See A1 for the interleaving and the fix. `lock_shared` (Unix `LOCK_SH`, `LockFileEx` without the exclusive flag) now gives Tantivy's wished-for RW semantics on all three OSes.
- The phase-7 mmap option (BP:257) reintroduces "cannot delete while mapped" on Windows. GC must then tolerate reader processes that live for days, so disk leaks become possible.

### Gaps
- Tantivy's Windows retry loop, and Tantivy's behaviour on NFS, were not read.

---

## Q3. Visible set after a force-push: cost at 10M commits, and how jj and Sapling do it

### Takeaway
One recompute is affordable with parent-first IDs (one bitset sweep, estimated, not measured). It becomes a problem only when it is not limited to non-fast-forward updates and memoized. The unsolved parts are ref scope and the unbounded growth of dead commits.

### Cited Findings
- A jj view holds "the set of visible head commits, bookmarks, tags, and the working-copy commit in each workspace". The doc does not say how the visible set is computed. — [jj concurrency](https://docs.jj-vcs.dev/latest/technical/concurrency/)
- Sapling segments give ancestry in "O(number-of-merges) time". — [Meta Engineering](https://engineering.fb.com/2022/11/15/open-source/sapling-source-control-scalable/) (as cited in BP:137)
- Current code indexes only `refs/heads/` and `refs/tags/`. — `git.rs:2099-2107` [F15]

### Inferences
- See A7. With dense parent-first IDs, a sweep from the highest ID down that marks parents of marked nodes is O(N+E) sequential. Recomputing it on every UI refresh is wasteful. Recomputing it on a non-fast-forward event is fine.

### Gaps
- No Sapling primary documentation on `visibleheads` was found by search. jj's internal visibility computation was not documented on the fetched page.

---

## Q4. The ref-snapshot frontier: 100k refs, packed-refs, symbolic refs, worktrees, TOCTOU, lookup cost

### Takeaway
The frontier semantics (`rev-list NEW --not OLD`) are sound and strictly better than the 300 s clock lookback (`indexer.rs:367-369`). The weak points are where the ref snapshot is stored, how often it is rewritten, which refs are in scope, and how a concurrent gc during a walk is handled.

### Cited Findings
- jj gives each command an isolated view: "Because the associated view object completely defines the repo state" it does not see later changes. — [jj concurrency](https://docs.jj-vcs.dev/latest/technical/concurrency/)
- The fixture matrix includes "rebase + `gc --prune=now`, repack mid-ingest". — BP:198

### Inferences
- See A10. The jj precedent supports "record exactly the snapshot you walked". BP should state it explicitly.

### Gaps
- No measurements were found for packed-refs parse time at 100k+ refs in gix.

---

## Q5. Blue/green reindex: disk, disk-full, readers during the swap

### Takeaway
Blue/green is the right default for a format change. BP needs a free-space preflight, an ENOSPC rule, a catch-up step and the A1 reader protocol. When space is short, delete-then-rebuild is acceptable for a rebuildable cache.

### Cited Findings
- Today, a full index clears all data first (`indexer.rs:403-404`), and LMDB map-full after clearing leaves the index half-built (audit item 6, BP:64).
- BP's size estimate covers topology only (about 360 MB at 10M commits; BP:129).

### Inferences
- See A8.

### Gaps
- No size estimate exists for `fc@v1` change layers at 10M commits.

---

## Q6. Cancellation and priority: can shrinking waves bound UI latency? What is the worst case?

### Takeaway
No. Wave size affects freshness only. Rayon has no priorities or preemption, and `install` across pools can run unrelated jobs. The worst case is unbounded and unmeasured. Cancellation tokens checked every N nodes do bound *query* cancellation latency, and that part holds.

### Cited Findings
- Rayon `install` "may potentially schedule other tasks to run on the current thread in the meantime". The docs describe no priority or preemption. — [rayon ThreadPool](https://docs.rs/rayon/latest/rayon/struct.ThreadPool.html)
- rust-analyzer QoS intents map to real QoS only on Apple. — [intent.rs](https://rust-lang.github.io/rust-analyzer/src/stdx/thread/intent.rs.html) (as cited in ICC:180)
- `sync_all` on Apple is `F_FULLFSYNC`, and on Windows `FlushFileBuffers`. — [unix.rs](https://github.com/rust-lang/rust/blob/master/library/std/src/sys/fs/unix.rs); [windows.rs](https://github.com/rust-lang/rust/blob/master/library/std/src/sys/fs/windows.rs)

### Inferences
- See A9.

### Gaps
- Per-OS background-priority APIs (Linux `SCHED_IDLE`, Windows background mode) were not verified.

---

## Q7. Tauri in-process vs sidecar/daemon: panics, poisoning, crash isolation

### Takeaway
The blueprint's deferral rests on "files give multi-process readers". That is true for reading, but it ignores crash isolation. OOM aborts the process, panics across a C ABI abort, and writer-thread death holds the lock. A split process (UI reader plus a worker that owns the writer) gets isolation without a daemon's lifecycle problems.

### Cited Findings
- On allocation failure, std will "print a message to standard error and abort the process". — [handle_alloc_error](https://doc.rust-lang.org/std/alloc/fn.handle_alloc_error.html)
- Rust 1.81: non-unwind ABIs "abort on uncaught unwinds". — [Rust 1.81.0](https://blog.rust-lang.org/2024/09/05/Rust-1.81.0/)
- Sync Tauri commands run on the main thread, and async ones via `async_runtime::spawn`. — [Tauri v2 Calling Rust](https://v2.tauri.app/develop/calling-rust/)
- Tauri sidecars bundle through `bundle.externalBin`. The page does not document lifecycle or kill-on-exit. — [Tauri v2 sidecar](https://v2.tauri.app/develop/sidecar/) (as cited in ICC:244)
- rust-analyzer puts proc macros in a separate process for isolation. — [rust-analyzer architecture](https://rust-analyzer.github.io/book/contributing/architecture.html) (as cited in ICC:243)
- Repo facts: there is no `tauri` dependency, the default panic strategy is unwind, and a poisoned cache mutex is silently ignored (`indexer.rs:1339-1355`). [F15]

### Inferences
- See A3. *Not verified:* whether tao/wry dispatch sync commands through `extern "C"` callbacks on each OS.

### Gaps
- Sidecar kill-on-exit and orphan handling in Tauri v2 remain unverified (also listed in ICC:258).

---

## Q8. Config: does auto-tune break the digest contract? Are profiles testable?

### Takeaway
Auto-tune does not break the content digest if invariance holds. It does make failures and output order machine-dependent, through A4 and through the physical layout. Profiles are testable once tuning is a pure, logged function.

### Cited Findings
- BP:198 lists "every performance profile must equal the default" among its metamorphic relations. BP:208 sets the "physical canonical root over byte-identical compacted layers" as a deferred goal.
- Xu et al.'s knob statistics in ICC:210-215 come from a secondary summary, which ICC says itself. — [Never Work In Theory](https://neverworkintheory.org/2016/06/09/too-many-knobs.html)

### Inferences
- See A14 and A4.

### Gaps
- No study of auto-tuning determinism in embedded storage engines was found (also an ICC gap).

---

## Q9. Claims tested and not broken

### Takeaway
The core shape survives:
- one writer, with immutable content-hashed layers that are never replaced in place;
- in-process `ArcSwap` snapshots;
- frontier diffing with no clocks;
- derived work as an idempotent set difference;
- Rayon for compute, Tokio at the edges;
- std-only locking, sync and rename on all three OSes.

### Cited Findings
- `File::try_lock`: stable in 1.89, `flock`/`LockFileEx`, released when all handles close. **Verified.** — [std::fs::File](https://doc.rust-lang.org/std/fs/struct.File.html)
- `sync_all` is `F_FULLFSYNC` on Apple and `FlushFileBuffers` on Windows. **Verified** (BP:127 is correct). — [unix.rs](https://github.com/rust-lang/rust/blob/master/library/std/src/sys/fs/unix.rs); [windows.rs](https://github.com/rust-lang/rust/blob/master/library/std/src/sys/fs/windows.rs)
- std's Windows default `FILE_SHARE_DELETE`, plus the POSIX-delete fallback, supports "read() not mmap", so deleting a layer that a reader has open works. **Supports BP:129.** — [windows.rs](https://github.com/rust-lang/rust/blob/master/library/std/src/sys/fs/windows.rs)
- ArcSwap avoids the RwLock writer starvation where "an update may be blocked for arbitrary long time by a steady inflow of readers". **Holds in-process.** The caveat is old-snapshot memory pinning (A9). — [arc-swap](https://docs.rs/arc-swap/latest/arc_swap/) (as cited in ICC:99)
- The ref-snapshot frontier vs the 300 s lookback (`indexer.rs:367-369`): the set-difference frontier needs no clock assumption. **Holds.** Caveats are in A10.
- Rayon for CPU work and Tokio only at the edge follow Tokio's own guidance. — [tokio docs](https://docs.rs/tokio/latest/tokio/index.html) (as cited in ICC:177)
- Content-hash segment names make republishing idempotent, and segments are never replaced in place. **Holds**, with the caveat of the antivirus rename retry (A12).

### Inferences
- Pending = indexed − covered is sound and crash-safe, given pure derivation (caveat A11).
- Per-query cancellation tokens checked every N nodes do bound cancellation latency to O(N) node visits. Only cross-workload *priority* is broken (A9).

### Gaps
- No claim in this section was exercised by running code. All of it rests on reading docs, std source and repo code.
