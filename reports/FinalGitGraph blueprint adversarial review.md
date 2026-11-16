# Red team spares the core, breaks the edges

The adversarial review keeps the blueprint's central bet and breaks much of what was built around it. Parent-first dense IDs survived every attack. A Python replay on the real git.git DAG found **zero parent-after-child violations, even across 200 incremental refreshes**, and the three O(1) ancestry rules gave **zero wrong answers**. Re-reading the code line by line confirms the edge-loss bug that motivates the rebuild. But **six critical breaks** would cause silent wrong answers, lost data or a plan that cannot run if an ADR adopted the blueprint as written:

1. Background compaction and ingest both publish manifests, with no compare-and-swap between them. A newly published layer can vanish while every checksum still passes.
2. Garbage collection protects only readers in the writer's own process. The writer lock is tied to a file path that can be deleted, so a second writer can start.
3. `git fetch --unshallow` and replace-ref changes give an already-numbered commit a parent that would need a lower ID. The O(1) ancestry rule then answers wrongly, and the claim that "derived facts are never invalidated" is false.
4. Limiting shortest path to an ID interval gives **a wrong answer for 112 of 120 random git.git pairs** under `direction=both`, the mode the API and perf harness use.
5. The same `git fast-import` stream produces **different commit OIDs on macOS and Linux**, because `git init` writes `core.ignorecase=true` on macOS.
6. Phase 0 cannot compile. The overlay compaction it relies on calls the two functions it deletes.

The review also found a critical bug the audit missed. Incremental mode skips every commit dated before its checkpoint, so merging a colleague's day-old branch can drop that whole branch from the index.

The headline numbers do not survive either:

- The 110 GB versus 3.5 GB write comparison prices a path that Phase 0 removes. Against the Phase 0 path, a cold layered index writes **4–12x more**.
- The "360 MB" open cost is really **0.5–1.2 GB** once the blueprint's own columns are counted.
- Change sets, including merges, run near **10k commits/s per core** on Linux history, so 100k/s needs at least 10 cores.
- Git's own test suite already checks that commit-graph on and off give the same output. That refutes the "first" in the novelty claim.

Before an ADR can rely on the blueprint, it must:

- specify a single publisher with compare-and-swap and a reader lock that works across processes;
- name the events that force a renumber;
- define ingestion over raw objects under a stated ref policy;
- fix shortest path and the output order that depends on IDs;
- pin fixtures completely and assert their expected (golden) OIDs;
- put numbers in the kill criteria;
- correct the Phase 0 deletion list.

Nothing was compiled or run in the repo. The evidence is code reading, primary sources, git CLI timings, Python simulations, and constructed failure scenarios that nobody has reproduced.

## Every area has a break, but the core ideas survive

Six red-team passes attacked the blueprint:

1. storage and file format;
2. graph representation, identity and queries;
3. git ingestion, derived data and throughput;
4. incremental updates, concurrency and crash consistency;
5. portability, automation, testing, the oracle and perf gates;
6. the novelty claim, the codebase audit and the migration plan.

Each pass re-read the cited code and fetched primary sources. Several also ran git or Python experiments on this Mac.

Severity uses one scale throughout:

- **Critical**: silent wrong answers, lost data, two concurrent writers, or a plan step that cannot run.
- **Major**: a failure users will hit, or a materially wrong claim.
- **Minor**: cost, precision or a missing detail.

The Basis column in every table says what kind of evidence backs each verdict:

- **Code**: a repo line read during this review.
- **Source**: a primary document or upstream source fetched during this review.
- **Measured**: a git CLI or Python run on this Mac.
- **Sim**: a Python model of a stated rule.
- **Inference**: reasoning or a constructed scenario that nobody observed.

Paths are relative to `apps/desktop/src-tauri/src/` unless stated.

| Blueprint area | Verdict | Decisive evidence | Worst severity |
|---|---|---|---|
| Storage and file format | Weakened | Immutable layers and checksums hold. "Append-only by construction" fails on unshallow. The 110 GB vs 3.5 GB comparison prices a deleted path. Open cost is 0.5–1.2 GB, not 360 MB | Critical |
| "No database in v1" | Breaks as specified | Ref map, registry, attempt counts and lazy line stats have no home. The redb fallback cannot serve two processes | Major |
| Graph representation | Weakened | No children chunk. Dead IDs need a visible-set bitset the query kernel never mentions | Major |
| Identity (parent-first IDs) | Holds for appends, breaks on deepen | 0 violations on git.git over 200 refreshes. Unshallow needs a parent ID below its child's | Critical |
| Query algorithms | Breaks for `direction=both` | 112/120 wrong shortest paths. 92% of positive ancestry queries fall to the walk. ID order leaks ingest history | Critical |
| Git ingestion | Weakened | gix capability inventory is right. Replace-ref polarity flips at gix 0.90. The reftable fallback fails on stock macOS | Major |
| Derived data | Breaks | `git replace --graft` changed the diff of an unchanged OID. Shallow roots seal phantom "added everything" commits | Critical |
| Throughput | Weakened | Topology and metadata hold marginally (105–113k/s). Change sets with merges run about 10.2k commits/s per core | Major |
| Incremental updates | Weakened | The ref-snapshot frontier is sound. Ref scope, dead-ID growth, 100k-ref manifests and TOCTOU races are unspecified | Major |
| Concurrency and crash consistency | Breaks | No manifest compare-and-swap. Cross-process GC with no reader registration. A deletable lock path | Critical |
| Portability | Breaks | fast-import OIDs differ between macOS and Linux. A C toolchain and git ≥2.32 are required | Critical |
| Automation | Weakened | Crate layering holds. Clippy bans cannot be scoped without attributes the repo bans. The proposed lints already exist | Major |
| Testing and artifact contract | Weakened | The pinned env covers about half of what gix-testtools pins. The leak gate has false positives and blind spots | Critical |
| Verification oracle | Weakened | Catches the edge loss. Misses metadata, query semantics, and ref and replace-ref semantics | Major |
| Perf gates | Weakened | The per-PR gate is Linux-only and blind to fsync and I/O | Major |
| Novelty claim | Breaks | git's `graph_git_two_modes`, Sapling's `check_isomorphic_graph`, gitoxide's git baselines | Major |
| Codebase audit | Holds, with misses | Every top finding confirmed. Missed incremental pruning of old-dated commits | Critical (missed bug) |
| Phased migration plan | Breaks at Phase 0 | Compaction calls the functions Phase 0 deletes. No repair for indexes that are already corrupted | Critical |

What survived intact is worth stating, because the breaks are not a rejection of the architecture. Every one of these ideas was attacked, and none broke:

- a single writer feeding bounded Rayon compute;
- immutable, checksummed graph layers in place of a key-value engine for an append-only DAG;
- in-process `ArcSwap` snapshots;
- a ref-snapshot frontier in place of the 300 s clock lookback;
- derived work computed as an idempotent set difference;
- metadata by default, with line stats computed lazily;
- a git oracle compared as sets, never counts;
- metamorphic tests that ingest must not depend on batch size, thread count or ingest path;
- validating output before timing it;
- `cargo xtask` over `just` or Nix;
- crates as the layering boundary.

The breaks cluster in two places. In one, the blueprint names an idea but leaves out the protocol around it. In the other, it quotes a number derived for a different object than the one it specifies.

## Storage numbers price the wrong objects, and "no database" leaves state homeless

| Blueprint claim | Verdict | Evidence | Basis |
|---|---|---|---|
| Immutable CSR layers beat a KV engine for an append-only DAG | Holds | The per-key overhead argument was attacked and not broken. The other attacks hit the protocol and the numbers, not the idea | Inference |
| xxh3 checksums are cheap to verify at open | Holds | XXH3 runs at 31.5–59.4 GB/s, about 40 ms per GB ([xxHash README](https://github.com/Cyan4973/xxHash/blob/dev/README.md)) | Source |
| Dated facts: lmdb 0.8.0 (2018-03-03), LMDB 0.9.36 (2026-08-06), Windows map pre-extension, redb mmap removal (2023) | Holds | ([crates.io lmdb](https://crates.io/api/v1/crates/lmdb), [LMDB CHANGES](https://raw.githubusercontent.com/LMDB/lmdb/mdb.RE/0.9/libraries/liblmdb/CHANGES), [mdb.c](https://raw.githubusercontent.com/LMDB/lmdb/mdb.RE/0.9/libraries/liblmdb/mdb.c), [redb CHANGELOG](https://github.com/cberner/redb/blob/master/CHANGELOG.md)) | Source |
| "Append-only by construction" | Breaks | Unshallow or deepen adds a parent after its child already holds an ID | Inference, consistent with [git commit-graph](https://git-scm.com/docs/commit-graph) |
| Geometric merge "with a max-commits cap" keeps layers logarithmic | Breaks under either reading | git's code default is `max_commits = 0`. At C = 64,000, amplification is 62.5x. Read as a hard cap, about 156 layers remain | Source + Sim |
| 110 GB written today vs 3.5 GB layered | Weakened (strawman baseline) | After Phase 0, a single rebuild writes 0.36–0.65 GB. Layering writes 2.5–8.2 GB | Sim |
| 10M-commit topology is about 360 MB, read in one sequential pass | Weakened | The blueprint's own columns come to about 520–650 MB, and about 1 GB with subjects. Every CLI call reads all of it | Inference (arithmetic) |
| About 24 B/commit for OID lookup, about 4x less memory | Weakened, and understates the win | Both lookup directions need 28 B (SHA-1) or 40 B (SHA-256). Today's cost is about 158–196 B/commit, so the gain is 6–8x | Code + Inference |
| `read()` costs at most about 2x vs mmap (redb precedent) | Weakened | redb reads pages on demand. It does not read the whole file at open | Source |
| v1 needs no embedded database | Breaks as specified | Refs, the repo registry, attempt counts, `unavailable` overrides and lazy line stats are all unassigned | Code + Inference |
| redb ≥4.4.0 is the fallback for shared mutable state | Breaks | Every released redb locks the whole file for a writer. SingleWriter mode is in the unreleased 4.5.0 | Source |
| heed 0.23 is a clean fallback | Weakened | `EnvOpenOptions::open` is `unsafe`, and heed builds on LMDB's 1.0-dev branch | Source |
| A custom chunk format is the right container | Weakened | Arrow IPC was never weighed. git's own format needed meaning-level revisions | Source + Inference |

### The max-commits cap breaks one of the two headline claims

The worst numeric error is the "max-commits cap". In git, the merge loop keeps merging while `g->num_commits <= st_mult(size_mult, num_commits) || (max_commits && num_commits > max_commits)`, and `max_commits` is 0 unless set ([git commit-graph.c](https://github.com/git/git/blob/master/commit-graph.c)). The technical doc describes 64,000 as a value that "could be extracted into config settings", bounding the size of each split file ([git technical/commit-graph](https://github.com/git/git/blob/master/Documentation/technical/commit-graph.adoc)). The blueprint says a layer merges "when it exceeds a max-commits cap", and the storage note gives a "default 64,000". Both misread git.

A simulation of git's exact loop at 10M commits with 16,384-commit waves gives:

- **without the cap: 6.9x** write amplification, at most seven layers and 14 full base rewrites;
- **with C = 64,000: 62.5x** write amplification and 125 full base rewrites.

That puts the "cheap" design within 2–5x of today's quadratic path. Read the other way, a true cap that never merges above C leaves about 156 layers at 10M commits. OID lookup then costs about 156 binary searches, which breaks "the number of layers stays logarithmic". Either reading voids one of the two headline claims.

### The 30x write saving compares against a deleted path

The 110 GB figure prices the streaming whole-blob path that Phase 0 itself removes. After Phase 0, a cold full index writes the CSR once: **0.36–0.65 GB**, depending on per-commit width. Today's bincode blob is about 65 B/commit, because bincode 1 stores each SHA as a u64 length plus 40 hex bytes (`db.rs:208-216`). At git's real merge rule, the layered design writes **2.5–8.2 GB** for the same history, 4–12x more on a cold index.

Layering is still the right choice. It makes partial results queryable during a long first index, and it keeps incremental runs at O(wave). The ADR must argue those points rather than the 30x headline. The earlier notes also contradicted each other: the storage note estimated about 9–10 rewrites and 3.5 GB, and the frontier note about 7 rewrites and 1.2 GB. Neither used the blueprint's actual layer width.

### Open cost and memory figures describe a smaller layer than the one specified

The 360 MB figure prices a 36-byte record per commit (a 20 B SHA plus two 8-byte neighbour slots). The blueprint's own chunk list costs much more at 10M commits:

- sorted OIDs: 200 MB;
- permutation and inverse permutation: 40 MB each;
- parents: about 80 MB;
- level, corrected date, commit time and author ID: 40 MB each.

That totals about 520 MB. git.git's mean subject length of 45.9 bytes pushes it to about 1 GB. Because the design reads everything at open, a CLI call such as `is-ancestor A B` reads and verifies the whole index before answering an O(1) question. Each process also holds its own private copy. Keeping mmap as an opt-in while verifying checksums at open defeats the point of mmap.

The xxh3 cost is not the problem. The I/O is. The cited redb precedent is "within a factor of ~2x on all workloads that I've benchmarked", and it describes page reads on demand, not reading the whole file at open ([redb CHANGELOG](https://github.com/cberner/redb/blob/master/CHANGELOG.md)).

The memory comparison runs the other way: it understates the gain. `QueryGraph` keeps both `sha_to_id: HashMap<String, NodeId>` and `id_to_sha: Vec<String>` (`application/indexer.rs:1872-1874`), so each SHA is allocated twice. The red team estimates **158–196 B/commit** today (inference from hashbrown layout constants it did not re-fetch), not 85–100. Against 28 B for lookup in both directions, the gain is 6–8x, not 4x.

### "No database" leaves mutable state with nowhere to live

Atomic rename gives atomic visibility. It does not give isolation, so two processes doing read-modify-write can still overwrite each other. The blueprint's inventory of mutable state leaves out several things the current code keeps for good reason:

- **The ref map.** On a mirror with 100k `refs/pull/*` refs, it is about 7 MB. Rewriting and flushing it on every one of about 611 wave publishes writes about 4.3 GB of manifest (a constructed scenario).
- **The repo registry.** The app and the CLI can each read-modify-write it, and the last rename wins silently.
- **Derivation attempt counts.** Today they are capped at five (`adapters/db.rs:31`). Without them, a commit that fails deterministically is retried at every startup forever.
- **`unavailable` facts.** A later fetch makes them wrong, but they are immutable.
- **Lazy line stats and metadata hydration.** Today the walker emits commits as `Pending`, with empty author and subject (`adapters/git.rs:481-499`).

The registry cannot be rebuilt from git, yet the blueprint puts it in an OS cache directory, which users and cleanup tools routinely wipe.

The blueprint also contradicts itself. It rejects SQLite for "a C build step" while saying the C toolchain is present wherever Rust links.

Its fallbacks fail too:

- **redb.** Every released version, up to 4.4.0, locks the whole file for a writer. Multi-process `SingleWriter` mode appears only in the unreleased 4.5.0 entry, after an experimental flag in 4.3.0 ([redb CHANGELOG](https://github.com/cberner/redb/blob/master/CHANGELOG.md)). The storage note's "redb does support multi-process access with a single writer" is therefore false for every shipped release.
- **heed.** `EnvOpenOptions::open` is `pub unsafe fn` ([heed env_open_options.rs](https://github.com/meilisearch/heed/blob/main/heed/src/envs/env_open_options.rs)), so AGENTS.md rule 2 requires an ADR before using it.

The missed alternative is **immutable layer files plus a SQLite WAL catalog**. The catalog holds the layer list, tips, registry, derived coverage, attempt counts and status overrides. Publishing becomes: fsync a uniquely named file, then commit one transaction that checks the generation number (compare-and-swap). That removes `MANIFEST.lock`, `MANIFEST.prev`, both lost-update risks and the full manifest rewrite for large ref sets.

### The custom container skipped a specified alternative

The layer container also skipped a specified alternative. Arrow IPC is little-endian, 64-byte aligned and has a footer index, and Phase 7 already adds "Arrow export". git's own carefully reviewed chunk format still needed meaning-level revisions: `GDA2`/`GDO2` replaced erroneous data, and Bloom v1 had a signed-`char` bug ([gitformat-commit-graph](https://git-scm.com/docs/gitformat-commit-graph)). Whichever container is chosen, its parser should be fuzzed.

## Three concurrency holes lose data while every checksum passes

| Blueprint claim | Verdict | Evidence | Basis |
|---|---|---|---|
| Background compaction is "published the same way" as ingest | Breaks (critical) | Compaction can publish a layer list it read before ingest's publish, which drops the new layer. git holds `commit-graph-chain.lock` across the whole merge | Inference + Source |
| GC runs after the "last in-process Arc drops + grace window" | Breaks (critical) | This says nothing about other processes. git's default grace is zero, and git can fall back to parsing objects | Source + Inference |
| `WRITER.lock` via `File::try_lock`, "released by the kernel on crash" | Breaks (critical) | flock binds to the open file, not the path. Deleting and recreating the lock file admits a second writer | Source + Inference |
| Publish sequence: tmp, `sync_all`, rename, `MANIFEST.lock`, rename | Weakened | No directory fsync. No step writes `MANIFEST.prev`. GC can delete `.prev`'s layers | Source + Inference |
| "A segment name is never reused" | Breaks | Content-hash names plus deterministic layers do reuse names. That collides with GC and with Windows delete-pending files | Source + Inference |
| std's Windows `rename` uses POSIX-semantics `FileRenameInfoEx` | Breaks (factual) | Rust 1.94 calls `MoveFileExW` first, and takes the POSIX path only after `ACCESS_DENIED` | Source |
| `sync_all` maps to `F_FULLFSYNC` / `FlushFileBuffers` | Holds | Rust 1.94 std source | Source |
| The goal is atomicity, not durability | Weakened (inconsistent) | Two device-wide flushes per wave on macOS, never budgeted | Source + Inference |
| Keep Tauri in-process hosting; defer a daemon | Weakened | OOM aborts the process. The repo has no `tauri` dependency. A writer-thread panic keeps holding the lock | Source + Code |
| Shrinking waves keeps the UI responsive | Breaks | Wave size changes freshness, not latency. Rayon has no priorities | Source + Inference |
| Single writer, `ArcSwap` snapshots, std-only locks | Holds | `try_lock` is stable since Rust 1.89. ArcSwap avoids writers starved by readers | Source |
| Index lives in the OS cache dir | Weakened | Cache dirs can be on NFS and are deletable. The registry cannot be rebuilt | Source + Inference |

### Compaction and ingest race to publish, and a layer vanishes

Three holes produce valid-looking corruption. The first is a **lost update between two publishers**:

1. The manifest is at generation N = [A, B].
2. The background compactor starts merging A+B into C, which takes minutes at 10M commits.
3. Meanwhile, ingest seals wave D and publishes [A, B, D].
4. The compactor finishes and publishes [C], built from the list it read in step 1.

D's commits are now gone. Every file still checksums correctly, so neither "verify on open" nor the `MANIFEST.prev` fallback fires. Worse, the ingest run moved the manifest's tips forward, so the ref-snapshot frontier treats D's commits as indexed and never re-ingests them. This is the same class of silent loss as today's wave-boundary bug.

git avoids this by holding `commit-graph-chain.lock` for the whole write, merge decision included ([git technical/commit-graph](https://github.com/git/git/blob/master/Documentation/technical/commit-graph.adoc)). The fix has three parts:

- a single publisher;
- a `generation` field in the manifest;
- compaction that may replace [A, B] only if they are still a contiguous run in the *current* manifest, so D is kept.

### GC deletes layers under readers in other processes

The second hole is **garbage collection across processes**. "After the last in-process Arc drops" protects only the writer's own process. The blueprint explicitly supports a CLI or second window reading the same files, and that process can stall for many reasons: laptop sleep, an antivirus scan, or simply reading a gigabyte of layers. When it resumes, it finds a deleted layer.

The precedent cited for the grace window does not hold. git's `--expire-time` defaults to "the current time", which means zero grace ([git-commit-graph](https://git-scm.com/docs/git-commit-graph)). git survives anyway: when a chain names a missing layer, it warns, uses the partial chain, and parses the missing commits from objects ([git commit-graph.c](https://github.com/git/git/blob/master/commit-graph.c)). FinalGitGraph's layers are the only copy of its index, so it has no such fallback.

Tantivy solved this exact problem. A second lock file exists "to protect the segment files being opened by `IndexReader::reload()` from being garbage collected", so "another process [can] safely consume our index in-writing" ([tantivy directory_lock.rs](https://github.com/quickwit-oss/tantivy/blob/main/src/directory/directory_lock.rs)). Rust's `File::lock_shared` now provides the read/write semantics Tantivy wished for, on all three OSes ([std::fs::File](https://doc.rust-lang.org/std/fs/struct.File.html)). The protocol becomes:

1. A reader takes a shared lock on `META.lock`, reads the manifest, opens every layer handle, then unlocks.
2. GC takes the exclusive lock before deleting anything.
3. Once a reader's handles are open, deletion is harmless. On Unix, an unlinked file stays readable. On Windows, std opens files with `FILE_SHARE_DELETE` and falls back to a POSIX-semantics delete ([rust 1.94 windows.rs](https://github.com/rust-lang/rust/blob/1.94.0/library/std/src/sys/fs/windows.rs)). This holds only while layers are read with `read()`, not mmapped.

### A deletable lock path admits a second writer

The third hole is a **writer lock bound to a path that can be deleted**. flock state belongs to the open file description, not the path ([flock(2)](https://man7.org/linux/man-pages/man2/flock.2.html)). Suppose a cleaner, an in-app "clear cache" button, or a blue/green step deletes the cache directory while the app holds the lock. The CLI then recreates `WRITER.lock` as a new inode, and `try_lock` succeeds. Two writers now assign dense IDs from different views, and every layer passes its own checksum.

The fix has four parts:

- After locking, compare the handle's device and inode (or Windows file ID) with the path.
- Keep the lock file at a stable path that nothing ever deletes.
- Put `writer_epoch` and `parent_manifest_hash` in the manifest.
- Refuse to publish unless the current manifest is the one this writer published last.

The same lock changes meaning on shared filesystems. On NFS, Linux emulates flock as an fcntl byte-range lock. On SMB up to Linux 5.4, it "is not propagated" at all ([flock(2)](https://man7.org/linux/man-pages/man2/flock.2.html)). jj avoids lock files because "they don't work when the clone is in a distributed file system" ([jj concurrency](https://docs.jj-vcs.dev/latest/technical/concurrency/)). Home directories, and with them `~/.cache`, are often NFS-mounted. The engine should detect the filesystem type at open and refuse writer mode on NFS, SMB, FUSE or 9p.

### The crash-recovery path leaks at every step

The blueprint's crash-consistency story borrows git's protocol and drops parts of it:

- **No directory fsync.** The blueprint's sequence has none, although the storage note's own step 5 had one. Linux fsync(2) says that for the directory entry, "an explicit fsync() on a file descriptor for the directory is also needed" ([man7 fsync(2)](https://man7.org/linux/man-pages/man2/fsync.2.html)).
- **Riskier pedigree.** ALICE counted unique crash vulnerabilities in **Git: 9, LMDB: 1, SQLite-WAL: 0** ([Pillai et al., OSDI'14](https://www.usenix.org/system/files/conference/osdi14/osdi14-paper-pillai.pdf)). The blueprint replaces the engine with one vulnerability by a copy of the protocol with nine. ALICE tested 2014 versions, and checksum-plus-rebuild turns most crash failures into rebuilds. It cannot catch the logical loss in the first hole above.
- **`MANIFEST.prev` is never written.** Renaming `MANIFEST` to `.prev` before the swap leaves a moment with no manifest. GC defined over "unreferenced files" can delete `.prev`'s layers, which kills the fallback.
- **`MANIFEST.lock` can go stale.** Taken literally as git's `O_CREAT|O_EXCL` lockfile ([git lockfile.h](https://github.com/git/git/blob/master/lockfile.h)), a SIGKILL leaves it behind, and every later publish fails.
- **Content-hash names are reused.** A crash between the segment rename and the manifest rename leaves an orphan, and the restart re-seals the same name. On Windows, a delete-pending file blocks re-creating its own name: "Subsequent calls to CreateFile to open the file fail with ERROR_ACCESS_DENIED" ([DeleteFileW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-deletefilew)).
- **The startup sweep can delete live files.** If a non-lock-holder runs it between another process's segment rename and manifest rename, it deletes that just-published segment.
- **Windows renames need retries.** git-for-windows retries rename and unlink at 0, 1, 10, 20 and 40 ms delays, because "some other process had the file open at the wrong moment" ([git-for-windows mingw.c](https://github.com/git-for-windows/git/blob/main/compat/mingw.c)). The blueprint retries only during GC.
- **Disk space is never checked.** A base merge needs room for the merged output while the inputs, `.prev`'s layers and the snapshots still pinned by readers stay on disk. Blue/green re-indexing doubles that, and nothing checks free space first.

Generation-numbered manifests with a tiny `CURRENT` pointer close most of these gaps, together with:

- segment names of the form `seg-<gen>-<hash>`;
- GC run only by the lock holder, under the publish mutex, with a grace period in minutes;
- a reader rule of "reload the manifest on ENOENT, never rebuild";
- a free-space check before any merge.

Two smaller errors sit in the same area. The Windows rename claim is wrong for the pinned toolchain. Rust 1.94 calls `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` first, and tries `FileRenameInfoEx` with POSIX semantics only "if `MoveFileExW` fails with ERROR_ACCESS_DENIED" ([rust 1.94 windows.rs](https://github.com/rust-lang/rust/blob/1.94.0/library/std/src/sys/fs/windows.rs)). A sharing violation from antivirus or a search indexer therefore gets no fallback. The design also says it wants atomicity rather than durability, yet `sync_all` on Apple is `F_FULLFSYNC` ([rust 1.94 unix.rs](https://github.com/rust-lang/rust/blob/1.94.0/library/std/src/sys/fs/unix.rs)). Two full flushes per wave, over about 611 waves at 10M commits, is full durability paid for a rebuildable cache, and the throughput budget leaves it out.

### In-process hosting gives no crash isolation

Running in-process gives no crash isolation:

- On allocation failure, std will "print a message to standard error and abort the process" ([handle_alloc_error](https://doc.rust-lang.org/std/alloc/fn.handle_alloc_error.html)), so an OOM during a 10M-commit ingest kills the UI too.
- A panic that unwinds into an `extern "C"` frame aborts the process since Rust 1.81 ([Rust 1.81.0](https://blog.rust-lang.org/2024/09/05/Rust-1.81.0/)).
- A panicked writer thread leaves the lock file owned by a live process, so no other process can take over.

The ledger row "Tauri in-process hosting: Keep" also has nothing to keep: the crate has **no `tauri` dependency**. A split process gets isolation without a daemon. The UI keeps an in-process reader, and a worker launched from the same binary owns the writer lock and does ingest, derivation, compaction and GC.

"Shrink waves when the UI is busy" does not bound query latency. Readers never wait on the writer, so wave size changes only freshness. Rayon's `install` "may potentially schedule other tasks to run on the current thread in the meantime", and the docs describe no priorities ([rayon ThreadPool](https://docs.rs/rayon/latest/rayon/struct.ThreadPool.html)). Worst-case query latency during ingest plus compaction is unbounded by design and was never measured.

## Parent-first numbering holds on git.git; shortest path and ordering do not

| Blueprint claim | Verdict | Evidence | Basis |
|---|---|---|---|
| Every parent ID is lower than its child's | Holds for appends | 0 violations on git.git, both for a full ingest and after 200 refreshes | Measured |
| The ID, level and same-run rules are O(1) and exact | Holds | 0 wrong answers in about 9,000 sampled queries | Measured |
| "Most parent hops become i → i−1" | Holds | 82.5% of first-parent hops. master's first-parent chain is 96% contiguous | Measured |
| Shortest path restricted to `[id(B), id(A)]` and generation bounds | Breaks for `both` | 11/120 pairs found no path, and 101/120 found a longer path | Measured |
| First-parent-last DFS order, "Sapling style" | Weakened | Sapling uses first-parent-first for incremental builds. Only 16 of 24,442 master commits keep ancestors = [0, id] | Source + Measured |
| Most ancestry queries are O(1) | Breaks | 92% of positive pairs fall to the walk. The walk peaks at 73,516 of 85,936 nodes | Measured |
| Topological stream = iterate descending IDs, "no heap" | Weakened | k = 100 scans up to 77,495 IDs. The truncation set changes the API contract | Measured + Code |
| Query outputs do not depend on ingest batching | Breaks | Topo stream with limit 100: only 92/100 overlap between full and incremental ingest | Measured |
| The layer chunk list is enough | Weakened | It has no children chunk, yet `children`, `both` and descendants need one | Inference |
| Kill segments if segments/N > 0.3 | Weakened (arbitrary) | git.git measures 0.41. The criterion measures storage, not latency | Measured + Source |
| Memory: 85–100 B today, 24 B new, 360 MB per layer set | Weakened | 158–196 B today. 28 B (SHA-1) or 40 B (SHA-256) new. At least 520 MB per layer set | Code + Inference |
| Git's 2.64 s → 0.02 s backs the fallback walk | Weakened | That result uses corrected commit dates. The cascade specifies topological level | Source |
| Software Heritage's renumbering supports topological IDs | Breaks | SWH orders nodes by BFS, then Layered Label Propagation (LLP) on a symmetrized graph | Source |

### How the replay was run, and what held

The graph pass replayed the blueprint's numbering in Python on a bare clone of [git.git](https://github.com/git/git). The clone has 85,936 commits, 25.2% merges, 37 octopus merges and 9 roots. The incremental model cut history into 200 committer-time slices, each closed under parents.

The invariant held: zero parent ≥ child violations after a full ingest and after all 200 refreshes. The three O(1) rules are logically sound and gave zero wrong answers:

- if `id(A) > id(B)`, B cannot reach A;
- if `level(A) ≥ level(B)` and A ≠ B, B cannot reach A;
- if A and B lie on the same contiguous first-parent run, B reaches A.

These are the parts of the design an ADR can cite now.

### Shortest path fails for `direction=both`

Shortest path breaks. Along `parents`, each hop strictly lowers the ID, so restricting the search to `[id(B), id(A)]` is sound. It is also sound for `children` if written as [min, max]. Copied literally, the blueprint's `[id(B), id(A)]` gives an empty interval when `from` is the ancestor.

For `direction=both`, the restriction is simply wrong. Take root R with two child tips X and Y, numbered R=0, X=1 and Y=2. The true path X–R–Y leaves the interval [1, 2], and R's generation is below both endpoints. The restricted search reports `found=false` for two connected commits.

On git.git, the restricted search **found no path for 11 of 120 random pairs and a longer path for 101**. The current API exposes `direction=both`, and the benchmark and `bin/perf-report.rs:306` measure exactly that mode. The fix:

- apply the interval and generation bounds only for `parents` and `children`;
- run an unrestricted, budgeted bidirectional BFS for `both`;
- add a third result state, "budget exhausted".

Today's `found: false` cannot tell "no path" from "gave up". That makes this an API change, contrary to the ledger's "keep the API".

### The chosen visit order gives up exact main-line ancestry

The blueprint numbers with an iterative DFS that visits the first parent last, and credits Sapling for it. That order buys ID adjacency: 82.5% of first-parent hops are i → i−1, against 60.3% for first-parent-first. But it gives up something more valuable.

Under first-parent-last, a merge's second-parent subtree is numbered first and takes the shared old history. On git.git, **only 16 of 24,442 master-chain commits** keep the property "ancestors(c) = [0, id(c)]". Under first-parent-first, **all 24,442** keep it, so "is X in main?" becomes one integer comparison. Ancestry queries with B on master fell to the walk 42% of the time under the blueprint's order, and 0% under first-parent-first.

Sapling's own source treats this as a choice that depends on the situation. It says "the above visit order (first parent last) is not optimal for incremental build case with pushrebase branches". It uses `VisitOrder::FirstFirst` for incremental updates, and reserves ID ranges for mainline growth ([sapling idmap.rs](https://github.com/facebook/sapling/blob/main/eden/scm/lib/dag/src/idmap.rs)). The blueprint copies neither choice.

Two 4-byte labels recover what is lost without changing IDs (a design inference with no precedent cited):

- a `main_rank` column, which makes main-line containment exact in O(1);
- a DFS-interval `low(v)`, which gives an exact positive test whenever `low(B) ≤ id(A) ≤ id(B)`.

### "Most queries O(1)" does not hold on commit DAGs

The claim that most ancestry queries are O(1) comes from O'Reach's rates on generic benchmark graphs ([O'Reach](https://ar5iv.labs.arxiv.org/html/2008.10932)), not from commit DAGs. On git.git, **92% of positive pairs reached the walk**, and the walk peaked at 73,516 of 85,936 nodes, the same worst case git has without bitmaps.

The cited 2.64 s → 0.02 s for `--is-ancestor` relies on generation v2 (corrected commit dates) and first-parent DFS ([GitHub blog: Git's database internals II](https://github.blog/open-source/git/gits-database-internals-ii-commit-history-queries/)). The cascade instead specifies topological level, which git itself treats as the weaker key for ordering walks.

### Query outputs depend on ingest history

IDs depend on ingest history: the tip order, the refresh cadence and the ingest path. Every output ordered or truncated by ID inherits that dependence. A six-commit hand example shows it:

- Ingest main R←a←b and feature R←x, then add merge m and child c incrementally.
- "Topo stream from c, limit 3" then returns [c, m, x].
- A full ingest of the same final DAG returns [c, m, b].

On git.git, the limit-100 stream from master **shared only 92 of 100 commits** between full and incremental ingest.

The content digest hides this, because it is defined over OID-sorted records and never looks at query outputs. The oracle cannot fix it either, because git gives no single expected value for such outputs.

The ADR must choose one of two fixes:

- define output order by a content-only tie-break, such as generation, then corrected date, then OID, and key cursors by OID;
- or exclude outputs ordered by ID from the invariance contract and test them only for topological validity.

### The topological stream and the layer format need more parts

The "no heap" topological stream costs the length of the ID interval, not k. Its median was 156 IDs scanned for k = 100, but its maximum was **77,495**, about 90% of the repo, from an old tag numbered last. A max-heap over frontier IDs gives the same descending order at O(k log w). It also changes the truncation set: today the code returns the BFS ball of up to `max_nodes` nearest commits, ordered by Kahn depth (`application/indexer.rs:2245-2338`). That is a contract change, not "same contracts".

The layer chunk list has no children index, yet `children`, `both`, descendants and the backward side of bidirectional BFS all need one. Old commits gain children in every new layer, so either a children chunk with a delta chunk merged at compaction, or an inversion at open of about 84 MB at 10M, must be specified.

Force-pushed commits leave dead IDs that every query must AND away with a per-snapshot visible bitset. The query section never mentions that bitset, and nothing ever reclaims dead IDs.

### Smaller fixes for the graph ADR

- **Segments kill criterion.** "Segments/N > 0.3" would kill segments for git.git, which measures **0.41**, and keep them for Linux, which has about 7–9% merges (secondary summaries of the [kernel docs](https://docs.kernel.org/maintainer/rebasing-and-merging.html) and [Destroy All Software](https://www.destroyallsoftware.com/blog/2017/the-biggest-and-weirdest-commits-in-linux-kernel-git-history); neither page was fetched). The criterion measures storage compression, while the blueprint wants segments for query latency. A p99 latency criterion fits that goal.
- **Sapling citations.** Sapling's `is_ancestor` builds the full ancestor set with no early exit. "O(flat segments), or O(merges)" is documented for `range` and `descendants`, not for ancestry ([sapling-dag iddag.rs](https://docs.rs/sapling-dag/latest/src/dag/iddag.rs.html)).
- **Software Heritage.** SWH renumbers by BFS and then Layered Label Propagation (LLP) on "a simplified (loopless, symmetric) graph", for compression ([swh-graph compression](https://raw.githubusercontent.com/SoftwareHeritage/swh-graph/master/docs/compression.rst)). That gives no support for parent-first IDs.
- **ID width.** With jj's overflow-bit convention, usable IDs are 2^31, not 4.29B ([jj-lib readonly.rs](https://docs.rs/jj-lib/latest/src/jj_lib/default_index/readonly.rs.html)). That is still ample for a single desktop repo.
- **Parent order.** The current overlay path sorts parents by descending ID (`application/indexer.rs:1980`), which loses first-parent identity. The oracle must pin git's parent order.

## Unshallow, replace refs and merges break "never invalidated" and "6+ cores"

| Blueprint claim | Verdict | Evidence | Basis |
|---|---|---|---|
| Facts keyed by `(commit, fact_version)` are never invalidated | Breaks | `git replace --graft B` kept C's OID but changed `diff-tree B` | Measured + Source |
| First-parent `fc@v1` is the canonical change set | Weakened | It cannot reproduce git's default `log -- path`. Merges hold 86.8% of change records | Source + Measured |
| About 6+ cores reach 100k change sets/s | Weakened | True for non-merges (106k/s on 8 processes). With merges, about 10.2k/s per core, so at least 10 cores | Measured |
| Topology + metadata ≥100k commits/s on one core | Holds marginally | 105–113k/s on the linux slice. Mailmap resolution pushes it lower | Measured |
| Line counts come from `line_counts()` with Histogram | Weakened | gix takes the algorithm from `diff.algorithm` or diff drivers | Source |
| Delete the Bloom reader because v1 is not portable | Right decision, wrong reason | The current reader re-checks every negative exactly, so the v1 bug cannot cause a wrong answer | Code |
| Exact change layers answer `log -- path` | Weakened | That is a scan of about 15 records per commit, unavailable until derivation completes | Measured + Inference |
| Commit-graph guards: shallow, replace, grafts, hash kind | Weakened | Omits `core.commitGraph=false`, corruption and stale caches. gix 0.90 already implements most guards | Source + Code |
| Upgrading gix 0.80 → 0.9x brings gains | Weakened | Replace-ref polarity was inverted up to 0.89. The upgrade breaks an API. "Parallel gix-pack" means delta-tree resolution | Source |
| Reftable repos handled by one `git for-each-ref` | Breaks on macOS | Apple Git 2.39.5 fails on reftable repos. The App Sandbox blocks the binary | Measured + Source |
| The ref-snapshot frontier replaces the 300 s lookback | Holds | It makes no clock assumption | Source + Code |
| "Only the ref layer is non-monotonic" | Breaks | The shallow set, the replace set and ODB contents also change | Source + Inference |
| Force-pushed commits drop out of the visible set for free | Weakened | Ref scope is undefined. Dead IDs grow without bound | Code + Inference |
| Pending = indexed − covered is crash-safe | Holds, with caveats | `unavailable` and `boundary` must not count as covered | Inference |

### Derived facts depend on more than the commit OID

The blueprint's most important false claim is that facts keyed by `(commit, fact_version)` are "never invalidated". git's own design doc lists the exceptions:

> "Commit grafts and replace objects can change the shape of the commit history". When shallow commits "are made unshallow, those generation numbers become invalid" ([git commit-graph](https://git-scm.com/docs/commit-graph)).

The ingestion note itself proposed keying facts by `(commit_oid, parents_fingerprint)`, and the blueprint dropped that.

A local demo with Git 2.49.0 made the failure concrete. In a chain A ← B ← C, `git replace --graft B` left C's OID unchanged, yet `diff-tree --root B` changed from listing `A f2` to listing `A f1` and `A f2`. Shallow clones are worse:

- A boundary commit's first-parent diff, computed as a root, lists every file at that commit.
- On the linux slice (described below), that is about 97,345 additions per boundary commit, across 549 boundary commits.
- Sealed as `fc@v1` and "never invalidated", these become permanent phantom "added everything" commits after `git fetch --unshallow`.
- The same unshallow breaks parent-first numbering, as the graph section showed.

Configuration and repo files are further inputs that are not part of the OID:

- gix picks the line-diff algorithm from "`diff.algorithm` configuration, or individual diff drivers" ([gix Platform docs](https://docs.rs/gix/latest/gix/object/blob/diff/struct.Platform.html)).
- gix honors textconv and `diff.*.command` ([gix crate-status](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)). A background indexer could therefore run user-configured external programs.
- On the linux slice, 246 commits touched `.mailmap` in about 14 months, so author IDs resolved through mailmap and sealed in topology layers would go stale.
- Similarity-rename output changed between gix versions, when 0.86 made the rename tracker independent of input order ([gix CHANGELOG](https://github.com/GitoxideLabs/gitoxide/blob/main/gix/CHANGELOG.md)).

The cheapest fix is to define ingestion over raw objects:

- open gix with replacements ignored;
- pin the diff algorithm and disable textconv and external diff drivers;
- store raw identities, and apply mailmap at query time;
- keep `fc@v1` to exact renames;
- record digests of the replace set, shallow set, mailmap blob and diff policy in the manifest, so a change triggers targeted re-derivation or a blue/green rebuild.

Boundary and `unavailable` commits must never count as "covered". An `unavailable` status should be written only after an ODB refresh confirms the object is truly missing, so that an antivirus lock or a mid-repack read error does not become permanent.

### Merges set the throughput ceiling

The throughput claim survives for topology and non-merge diffs, and fails for merges. The ingestion pass measured a bare, blobless, shallow clone of [torvalds/linux](https://github.com/torvalds/linux) on an M1 Pro, called the "linux slice". It has 115,295 commits, 9,181 merges and 97,345 files at HEAD. Shallow clones disable git's commit-graph, so none of these runs used one. Each figure is a single run after one warm-up.

| git command (Apple Git 2.39.5) | Time | Throughput |
|---|---|---|
| `rev-list --parents HEAD` | 1.02 s | ≈113k commits/s |
| `log --format='%H %P %T %an %ae %at %ct %s'` | 1.10 s | ≈105k commits/s |
| `log --no-merges --raw --no-renames` (106,114 commits) | 5.33 s | ≈19.9k diffs/s |
| `log --merges --raw --diff-merges=first-parent` (9,181 merges) | 5.99 s | ≈1.5k merges/s |
| Both together, a first-parent change set for every commit | 11.32 s | ≈10.2k commits/s |
| `diff-tree --stdin` non-merges, 8 processes (Git 2.49.0) | 1.00 s | ≈106k/s (5.7x) |

Five things follow from these runs:

- **Non-merges hold.** Diffs ran at the same rate as on git.git, so the larger Linux trees did not slow them. The "6+ cores" estimate holds here: eight processes reached about 106k diffs/s.
- **Merges break the estimate.** Each costs about 13x a non-merge and carries **86.8% of all change records** (1,489,512 of 1,715,595). Merge sizes are heavy-tailed: p99 is 2,329 changes and the maximum 36,879.
- **Including merges, 100k/s needs at least 10 cores** at perfect scaling. The M1 Pro scaled only 5.7x on eight uniform processes.
- **Metadata at "100k/s on one core" is marginal.** It reached 105–111k/s with mailmap off. With mailmap on, the 2.49.0 non-merge run took 6.25 s instead of 5.49 s.
- **The git version barely matters** (5.33 s vs 5.49 s), so the "old Apple Git" objection does not move the numbers.

No gix tree-diff measurement exists, published or local.

The fix is two derived types with separate targets: non-merge change sets and merge change sets. Merge work should be bounded (a capped or flagged "large merge", or derived lazily), and the work queue ordered by expected cost so one huge merge does not stall a worker.

### First-parent change sets lose git's default file history

First-parent change sets also lose a user-visible behaviour. git's default `log -- path` hides a merge that is TREESAME to any parent and follows only that parent ([git-log](https://git-scm.com/docs/git-log)). A scan of first-parent change layers instead returns both the side-branch commit and every merge that carries the change. Per-path churn then double-counts.

Storing all-parent diffs costs **14.6x** the first-parent volume on the linux slice (21.7M vs 1.49M change lines). A cheaper fix: store, per merge and per path, one bit saying whether the path is TREESAME to parent 2. That reproduces git's simplification and also flags conflict resolutions.

### The Bloom reader can go, but not for the stated reason

Deleting the Bloom reader is right, for a different reason than the blueprint gives. The current code returns `Touched` on a Bloom "maybe" without checking it, and re-checks every "definitely not" with an exact two-tree lookup (`adapters/git.rs:2840-2877`, `:3463-3517`). It therefore never skips the expensive case, and the v1 signed-`char` quirk cannot produce a wrong answer here.

git also still writes v1 filters by default when none exist ([git config commitgraph](https://github.com/git/git/blob/master/Documentation/config/commitgraph.adoc)). Its own sources describe the bug from opposite sides: the format doc says "when char is signed" ([gitformat-commit-graph](https://git-scm.com/docs/gitformat-commit-graph)), while the 2.46 release notes say "broken on systems whose 'char' is unsigned" ([RelNotes 2.46.0](https://github.com/git/git/blob/master/Documentation/RelNotes/2.46.0.adoc)).

The real gap is what replaces Bloom filters. Exact change layers mean a path query scans every change record, about 15 per commit on the linux slice, and cannot run at all before derivation completes. git's Bloom filters gave `log -- path` "2x to 5x" ([gitgitgadget PR #497](https://github.com/gitgitgadget/git/pull/497)). gitoxide lists them as needed for gix-blame to be "competitive with Git" ([gix crate-status](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)). The plan needs a per-path posting list from the start, plus a rule for partial answers on a cold start.

### gix facts mostly hold, but the upgrade is a semantic migration

The gix capability inventory checks out against source:

- gix 0.90.0 is current (2026-10-22) ([crates.io gix](https://crates.io/crates/gix/versions)).
- gix-commitgraph reads no Bloom or GDA2 chunks and cannot write graphs.
- Reftable, promisor and bitmap support are absent.
- `line_counts()` exists and returns `None` for binary input.

Five of the blueprint's gix claims are wrong or out of date:

- **Replace refs had inverted polarity.** Up to gix 0.89, an unset config meant replacements were *off*. Commit 1b6d8da fixed this in 0.90, and gix now also skips commit-graphs "while object replacements are active" ([gitoxide 1b6d8da](https://github.com/GitoxideLabs/gitoxide/commit/1b6d8da7a4)). After the upgrade, `find_commit` returns replacement objects, while FinalGitGraph's hand-rolled commit-graph path still returns original parents. That is a mixed view unless replacements are pinned off.
- **The upgrade breaks an API.** Path-based `Graph` constructors "now require `object_hash`" ([gitoxide 8722752](https://github.com/GitoxideLabs/gitoxide/commit/8722752744)).
- **"Parallel gix-pack" is mis-cited.** It is lazy delta-tree resolution for whole-pack decoding ([PR #2852](https://github.com/GitoxideLabs/gitoxide/pull/2852)). It does not speed up random object lookups for per-commit tree diffs.
- **The guard list is incomplete.** It omits `core.commitGraph=false`, corrupt graphs, and a cache that goes stale while the process runs. gix 0.90's `rev_walk` already disables the graph for replacements and shallow repos and honors `core.commitGraph` ([gix walk.rs](https://github.com/GitoxideLabs/gitoxide/blob/main/gix/src/revision/walk.rs)). Today's loader has none of these checks (`adapters/git.rs:3000-3019`).
- **gix gained topological sorting on main** on 2026-10-27 (not yet in 0.90). Its hidden-ancestry pruning is "not exact under arbitrary date skew" ([gitoxide 56a9867](https://github.com/GitoxideLabs/gitoxide/commit/56a9867a36)). The frontier walk should keep its own exact stop condition rather than gix's `hide()`.

The reftable fallback fails where it matters most. On this Mac, `/usr/bin/git` (Apple Git 2.39.5) printed `fatal: unknown repository extension found: refstorage` on a reftable repo. Reftable arrived only in Git 2.45 ([RelNotes 2.45.0](https://github.com/git/git/blob/master/Documentation/RelNotes/2.45.0.adoc)). Apple says `xcrun` "contains an explicitly check for it being run from within a sandbox. There's no way to bypass this" ([Apple DTS](https://developer.apple.com/forums/thread/655664)). Git 3.0 plans reftable as the default for new repositories ([git BreakingChanges](https://git-scm.com/docs/BreakingChanges)), so this "fallback" will become the main path.

### Incremental updates: the frontier is sound, its scope and growth are not

The ref-snapshot frontier, `rev-list NEW --not OLD`, is strictly better than the 300 s clock lookback. Parent-first numbering stayed valid across 200 refreshes, including refs that entered scope late.

What the blueprint leaves unspecified:

- **Which refs define "visible".** Today `load_refs` reads only `refs/heads/` and `refs/tags/` (`adapters/git.rs:2099-2107`). The default walk scope is local branches plus a detached HEAD (`adapters/git.rs:1887-1960`).
- **When visibility is recomputed.** It should be recomputed only on non-fast-forward ref changes, and memoized by the hash of the ref snapshot.
- **Dead IDs.** Rebase, amend and pull-ref churn add them, and only a renumbering rebuild removes them.
- **Which ref snapshot the manifest stores.** It must be exactly the snapshot the walk used, never a re-read at publish time.
- **Recovery from a concurrent gc.** A `gc --prune=now` that deletes objects mid-walk needs a defined restart.

The DBSP "−1 on force-push" delta needs a distinct over reachability, because a commit still reachable from another ref must not be subtracted. It is not linear, so it is not free.

## Fixtures differ by OS, and the oracle misses whole bug classes

| Blueprint claim | Verdict | Evidence | Basis |
|---|---|---|---|
| Seeded fast-import fixtures, and Windows = macOS = Linux | Breaks | The same stream gives 3 tree entries on a default macOS init and 4 with ignorecase off | Measured |
| The core path needs only cargo and git | Breaks | Every OS needs a C linker. `lmdb-sys` and `libmimalloc-sys` build with `cc`. The Visual Studio licence applies on Windows | Code + Source |
| xtask checks the git version | Weakened | No minimum given. `GIT_CONFIG_GLOBAL` needs git ≥2.32. Git 3.0 flips the hash, ref-storage and branch defaults | Source |
| The gates cannot run on macOS or Linux | Overstated | CI already runs the scripts under `pwsh` on Linux. Only the pnpm aliases call `powershell`. CI has no macOS leg | Code |
| The pinned env (`NOSYSTEM`, null global config, fixed dates) isolates fixtures | Weakened | gix-testtools also clears all `GIT_*` vars and sets idents, the hash and gc off. Without idents, commits fail | Source + Measured |
| The leak gate guarantees nothing lands outside `target/` | Weakened | False positives on `.idea/`, `*.log`, proptest, insta and mutants output. Blind to `.git/`, HOME and app data | Code + Source |
| `CARGO_TARGET_TMPDIR` plus `--artifact-root` everywhere | Weakened | `CARGO_TARGET_TMPDIR` is set at compile time, for integration tests and benches only. libtest has no channel for `--artifact-root` | Source |
| A clippy ban on `env::var` outside the config module | Weakened | Scoping needs attributes the repo bans. Dependencies compile with `--cap-lints allow` | Source + Code |
| Add workspace lints such as `unsafe_code = "forbid"` | Already done | The root `Cargo.toml` already has them | Code |
| Extra tools installed under `target/tools` | Weakened | `cargo install` builds in the OS temp dir. Valgrind is a system package | Source |
| nextest is no longer needed for correctness | Weakened | Two runners mean two test semantics. libtest has no per-test timeout | Source + Inference |
| Oracle: `rev-list --all --parents` and `log --raw` | Weakened | `--all` adds notes commits, honors replace refs and includes refs the product skips. The `log` command walks HEAD only | Measured + Code |
| proptest DAGs with persisted failing seeds | Weakened | The seed is random per run. Persisted seeds break after strategy edits. About 5,900 git spawns per property | Source + Inference |
| Digest-checked perf, gungraun per PR on Linux | Weakened | Instruction counts miss fsync, syscalls and I/O. Windows and macOS go ungated | Source + Inference |
| Validate output before timing | Holds | This directly fixes the `let _ =` results | Code |

### The same fixture stream gives different repos on macOS and Linux

The most damaging portability result came from a local experiment with Apple Git 2.39.5:

1. A plain `git init` on macOS wrote `ignorecase = true` and `precomposeunicode = true` into the repo's own `.git/config`.
2. A fast-import stream containing both `Makefile` and `makefile` then produced a tree with **3 entries** (commit `452154be…`). `makefile` was dropped, and `Makefile` got the second blob's content.
3. The same stream with `core.ignorecase=false` produced **4 entries** (commit `23bc481b…`).

git's docs confirm that init "will probe and set core.ignoreCase true if appropriate", and list APFS and NTFS as case-insensitive ([git-config](https://git-scm.com/docs/git-config)).

Each OS stays green on its own, because the oracle and the product read the same collapsed repo. But the "case-only renames" fixture tests nothing on macOS (and almost certainly on Windows, which is inferred and not observed), and any cross-OS digest comparison either goes red or compares two different repositories.

The fix:

- create fixtures with `-c core.ignorecase=false -c core.precomposeunicode=false`, or better, write fixture objects with gix and use the git CLI only on the read side;
- commit each fixture's expected HEAD OID and ref list, and assert them on every OS before any oracle runs.

### The pinned environment covers half of what gitoxide pins

The cited gitoxide tooling pins far more than the blueprint does. gix-testtools:

- removes every inherited variable starting with `GIT_`;
- sets `XDG_CONFIG_HOME`, fixed author and committer names and emails, and `GIT_DEFAULT_HASH`;
- turns off gpgsign, sets `init.defaultBranch`, `protocol.file.allow`, `gc.auto=0` and `maintenance.auto=false`.

([gix-testtools](https://raw.githubusercontent.com/GitoxideLabs/gitoxide/main/tests/tools/src/lib.rs))

Without these settings, four things go wrong:

- **Hooks leak the project repo.** A hook exports `GIT_DIR` ([githooks](https://git-scm.com/docs/githooks)), so fixture commands run from a pre-push hook write into FinalGitGraph's own `.git`, which `git status` never shows.
- **Inherited hash settings change OIDs.** An exported `GIT_DEFAULT_HASH` changes every fixture OID.
- **Commits fail with no identity.** A host without an identity fails outright: locally, `git notes add` failed with `fatal: unable to auto-detect email address`.
- **Background gc interferes.** Detached auto-gc repacks while the product ingests.

`GIT_CONFIG_GLOBAL` exists only since Git 2.32 ([RelNotes 2.32.0](https://raw.githubusercontent.com/git/git/master/Documentation/RelNotes/2.32.0.adoc)), and older git ignores it silently. Git 3.0 will flip the defaults to SHA-256, reftable and `main` ([git BreakingChanges](https://git-scm.com/docs/BreakingChanges)), so fixtures must pass `--object-format`, the ref format and the initial branch explicitly.

### Toolchain and automation claims overreach

"Only cargo and git" is false today. rustup says Windows "requires a linker, libraries and Windows API import libraries" from Visual Studio, with licence limits on Build Tools ([rustup Windows MSVC](https://rust-lang.github.io/rustup/installation/windows-msvc.html)). The repo also compiles C through `lmdb-sys` and `libmimalloc-sys` until LMDB leaves.

The audit's "gates cannot run on macOS or Linux" is overstated. CI already runs `enforce-guardrails.ps1` and `perf-budget.ps1` under `pwsh` on `ubuntu-latest` and `windows-latest`. Only the pnpm aliases hardcode `powershell`. macOS is missing from CI, and two perf scripts throw "D: drive is required on Windows".

Clippy's `disallowed-methods` cannot be scoped to "outside the config module" by configuration. Scoping needs `#[expect]` or `#[allow]` attributes, which AGENTS.md and `docs/automation-guardrails.md:95` ban. Cargo also passes "`--cap-lints allow` when compiling your dependencies" ([rustc lint levels](https://doc.rust-lang.org/rustc/lints/levels.html)), so gix's own environment reads cannot be banned at all.

The workspace lints the blueprint proposes already exist in the root `Cargo.toml`. With `forbid` in force, a Rust 2024 edition bump cannot compile the tests' `set_var` calls even with a local allow, so that blocker is harder than the blueprint states. The workspace is still `edition = "2021"`, so `set_var` is not `unsafe` today.

### The artifact contract is too strict and too weak at once

The artifact contract is both too strict and too weak.

It is too strict: `git status --porcelain --ignored` flags `.idea/`, `.vscode/`, `*.log` and `tmp/`, which this repo's `.gitignore` already lists. It also flags tool defaults that land outside `target/`:

- proptest writes `proptest-regressions/` ([proptest Config](https://docs.rs/proptest/latest/proptest/test_runner/struct.Config.html));
- insta writes `.snap.new` files ([insta](https://insta.rs/docs/advanced/));
- cargo-mutants writes `mutants.out` in the source dir ([mutants.rs](https://mutants.rs/mutants-out.html)).

It is too weak: it cannot see writes inside `.git/`, HOME or app-data folders, and on Windows, known folders ignore environment redirection ([dirs-sys](https://raw.githubusercontent.com/dirs-dev/dirs-sys-rs/main/src/lib.rs)). Moving fixtures under `target/` also creates a new hazard. A failed fast-import or a "not a repo" test can discover FinalGitGraph's own repository upward from the fixture directory, unless `GIT_CEILING_DIRECTORIES` is set.

The cleanup and path rules have their own gaps:

- **The sweep can delete a live run.** Its rule, "not owned by a live process, or older than N hours", deletes a live perf run that has run longer than N hours.
- **The fixture root does not reach unit tests.** `CARGO_TARGET_TMPDIR` is "only set when building integration test or benchmark code" ([Cargo env vars](https://doc.rust-lang.org/cargo/reference/environment-variables.html)).
- **Windows paths run long.** Git for Windows keeps long-path support off by default ([Git for Windows](https://gitforwindows.org/git-cannot-create-a-file-or-directory-with-a-long-path)). A typical clone root plus `target\gitgraph-artifacts\run-<uuid>\` plus a SHA-256 pack path comes to about 240 characters, close to the 260-character limit.

### The oracle catches the edge loss and misses everything around it

The oracle catches the edge-loss class, and comparing sets rather than counts is right. It misses:

- commit metadata, tree OIDs and generation numbers;
- whether a topological order is valid at all;
- BFS depth, truncation flags and pagination;
- shortest-path minimality;
- diffs against second parents;
- which refs and which object view count.

Two local experiments showed the oracle's own semantics shifting under it. After `git notes add`, `rev-list --all --parents` printed an extra commit from `refs/notes/commits`. After `git replace c2 <parentless>`, `rev-list --parents main` printed c2 with no parent, and c1 disappeared.

The product's default scope indexes only `refs/heads/*` plus a detached HEAD (`adapters/git.rs:1887-1960`), and a test asserts this on purpose (`tests/indexer_smoke.rs:347`). `--all` therefore fails on any repo with a tag-only commit, for reasons unrelated to the bug. The change-set command `git log --raw -z --no-abbrev --no-renames --diff-merges=first-parent` has no `--all` and no explicit refs, so it diffs only HEAD's history.

proptest picks a random seed per run unless `PROPTEST_RNG_SEED` is set, and persisted seeds "may or may not produce the problematic value" after strategy changes ([proptest failure persistence](https://proptest-rs.github.io/proptest/proptest/failure-persistence.html)). The three OSes therefore run different DAGs. With 256 cases and about 23 git spawns per case, one property costs about 5,900 spawns, plus up to about 23,500 more during shrinking (inference).

### Per-PR perf gates are Linux-only and blind to I/O

Gungraun "cannot be run on Windows" and needs a system Valgrind ([gungraun guide](https://gungraun.github.io/gungraun/latest/html/)). Instruction counts also miss fsync, syscalls and I/O, which are exactly the costs the layered write-sync-rename design adds, and they differ most between OSes.

GitHub's hosted runners give private repos 2 vCPU on Linux and Windows ([GitHub-hosted runners](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)), and the "fixed hardware" for the wall-time lane does not exist.

The fix is deterministic counters, gated on all three OSes on every PR:

- bytes read and written;
- fsync calls;
- files opened and renamed;
- objects decoded;
- allocations, counted by a counting allocator.

## Git's test suite refutes "first", and Phase 0 cannot compile

### Novelty: every element of the contract has shipped prior art

The novelty claim breaks on "first". git's test library has `graph_git_two_modes`, which runs a command with `core.commitGraph=true` and `=false` and `test_cmp`s the outputs for `log --topo-order`, `log --graph`, `merge-base -a` and more ([git t/lib-commit-graph.sh](https://raw.githubusercontent.com/git/git/master/t/lib-commit-graph.sh)). That is the blueprint's "ingest path invariance", verbatim. Other prior art covers the rest:

- `GIT_TEST_COMMIT_GRAPH` runs git's whole suite with a commit-graph written after every commit ([git t/README](https://raw.githubusercontent.com/git/git/master/t/README)).
- `git fsck` runs `commit-graph verify` against the object database ([git-fsck](https://git-scm.com/docs/git-fsck)).
- Sapling ships `check_isomorphic_graph`, which checks that "the subset of the current graph (ancestors of heads) is isomorphic with the subset in the other graph" ([sapling-dag CheckIntegrity](https://docs.rs/sapling-dag/latest/dag/ops/trait.CheckIntegrity.html)).
- gitoxide checks gix-diff output against `git diff` baselines ([gitoxide PR 2197](https://github.com/GitoxideLabs/gitoxide/pull/2197)).
- CodeLore 0.30.0 (2026-09-09) lists "differential `GixRepo` vs `GitCliRepo` cross-walker parity" in its tests ([codelore-lib](https://docs.rs/codelore-lib/latest/codelore_lib/repo/git_cli_repo/index.html)).
- Reproducible builds already define environment-invariant, bit-identical artifacts ([reproducible-builds.org](https://reproducible-builds.org/docs/definition/)).

The claim is also imprecise in two ways. "Reachable commit DAG" is undefined without a ref policy. And IDs make user-visible output depend on ingest history, so only the digest, not the logical content users see, is a pure function of the DAG.

The blueprint dropped the Merkle-root and consistency-proof parts that earned the frontier note its novelty rating, but kept the word "first". Honest wording: "adopts git's two-mode equivalence tests, Sapling's isomorphism check and gitoxide-style git baselines as a release gate, extended to batch size, thread count and OS, with a published content digest".

One citation also needs fixing. InfoQ reports that Scion's 15 October update credited "Mononoke and its Bonsai data model", but does not say the credit followed criticism ([InfoQ](https://infoq.com/news/2026/10/anthropic-source-control)). That framing comes from a secondary blog ([braindetox](https://braindetox.kr/en/posts/anthropic_scion_mononoke_derived_data_critique_2026.html)) that the blueprint does not cite.

### Codebase audit: confirmed at the top, with one critical miss

| Audit finding | Verdict | Evidence (code read, not run) |
|---|---|---|
| Parent links dropped across waves in the default full and incremental modes | Confirmed | `application/indexer.rs:372`, `:404-407`, `:742-743`, `:797`, `:1453-1476`, `:1595-1606`. `adapters/db.rs:2657-2659`. No re-link path at any CSR call site |
| Tautological edge assertion, 3-commit fixtures, `let _ =` results | Confirmed | `tests/indexer_smoke.rs:85` vs `adapters/db.rs:1878-1890`. `bin/perf-report.rs:266-301` |
| "The perf gate times nothing" | Overstated | CI's required "Ingestion Budget Fast (25k cold)" throws on `full_index_ms > 45000`, `incremental_index_ms > 15000` and RSS > 60 MB. Its 20,001-commit fixture crosses a wave boundary on the buggy path, but its results are discarded |
| "Gates cannot run on macOS/Linux" | Overstated | CI runs them under `pwsh` on Linux. Only the pnpm aliases use `powershell` |
| The "bounded" channel can hold about 536M commits | Arithmetic confirmed, consequence overstated | The tape walker, the commit-graph caches and `streamed_commit_shas` are already O(N) memory |
| 8 unread env vars, 12 dead functions, `tx_commit` never set, a double count | Confirmed, with nuances | Some unread vars are forwarded into Docker. ADR 0014 still documents one. The dead functions are `pub` items of a lib crate |
| The drain deletes rows before replacing them | Confirmed, and worse | One failure wipes the rows of a whole batch of up to 1,024 commits |
| LMDB double-open; risky `WRITE_MAP \| MAP_ASYNC` | Latent; flags are opt-in | Opened sequentially today. The flags are off by default |
| Missed: incremental mode prunes commits older than the checkpoint | New, critical | `application/indexer.rs:367-369`, `:1399-1415`. `adapters/git.rs:1124-1131`, `:1158-1166`, `:2233-2241` |
| Missed: no shallow, replace or graft guards in either walker | New, verified absent | `adapters/git.rs:3000-3022`, `:1543-1580`, `:481-490` |
| Missed: global commit-graph and Bloom caches never invalidated | New | `adapters/git.rs:3000-3022`. It caches `None` too |
| Missed: commit-graph walk aborts, then re-emits through the tape walker without dedupe; errors swallowed | New | `adapters/git.rs:~904-962`, `:~1568-1575`. `application/indexer.rs:~994-997` |
| Missed: wave boundaries depend on timing | New | The adaptive controller at `application/indexer.rs:181-194` |

The missed pruning bug matters as much as the headline bug. The incremental checkpoint is the *maximum* committer time seen, minus 300 s. In the default Frontier scope, any commit older than that is neither emitted nor expanded, in the tape walker and in both commit-graph walks.

Merging a colleague's branch whose commits were made yesterday emits the merge, prunes its second parent, and never indexes the branch. The dangling edge is then dropped by the same `continue`. A single commit with a future date (2030, say) raises the checkpoint and blinds every later incremental run.

This failure is inferred from reading the code, not reproduced. It belongs next to the edge loss in the ranking and in the Phase 0 test. Streaming CSR has been the default since commit `80d4ed1` (2026-04-09). The committed `docs/perf-report-25k-final.md` is dated 2026-03-07, so those committed numbers predate the bug.

### Migration plan: Phase 0 needs a corrected deletion list and a repair step

| Plan claim | Verdict | Evidence and correction |
|---|---|---|
| Phase 0 "fix is a deletion" of `apply_csr_delta_from_commit_shas` and `build_incremental_csr_snapshot` | Breaks: does not compile | `compact_graph_overlay_into_snapshot` is implemented by calling `apply_csr_delta_from_commit_shas` (`adapters/db.rs:1813-1830`), which calls `build_incremental_csr_snapshot` (`:1615`). The guardrail requires the compaction function (`scripts/enforce-guardrails.ps1:489-499`). Delete only the per-wave call site (`application/indexer.rs:1595-1606`), the empty-snapshot init in full mode (`:404-407`) and the two env switches. Keep the builder, which is correct when it receives all new commits in one call (`adapters/db.rs:2621-2630`) |
| Phase 0 exit gate: red before the fix, green after | Weakened | Nothing repairs existing indexes: the delta builder copies old rows verbatim (`adapters/db.rs:2700-2760`). Add a format bump, or a rebuild on open when stored edges < Σ parents |
| Phase 0 test: "default settings and no environment variables" | Weakened | Scrub env with a CLI subprocess and `env_clear()`, not `remove_var`. Use strictly increasing dates, because equal timestamps break ties by OID. Waves depend on timing, so assert sets. Add an incremental case of more than 8,192 commits and an old-dated merge |
| "No guardrail requires the deleted functions" | Holds for Phase 0 only | Later phases collide with `delta_log`/`append_delta_event`, `let store = open_store\(`, `struct DenseSet`, `ingest_tx_commit_ms`, the `Patch::from_diff` marker and the exact ADR file name `0015-lmdb-csr-delta-storage.md` |
| Phase 1: `cargo xtask` mirrors every gate | Weakened | Mirroring means porting about 1,038 lines of name-existence greps the blueprint calls harmful. Port only behavioral gates |
| Scripts deleted in Phase 1 | Contradictory | Phase 2 deletes `perf-budget.ps1` again, and Phase 5 deletes the grep guardrails again |
| Phase 3: harden LMDB, one store per process | Mostly wasted | Phase 5 deletes LMDB. No Tauri UI exists yet. Keep only drain atomicity |
| Phase 4: old engine = new engine = git | Ill-posed | The old engine's file-change semantics depend on thresholds and env (`adapters/git.rs:60-66`). Make git the only oracle |
| Kill criteria "written into the ADR before measuring" | Breaks | "Pre-registered throughput", "cold-start budget" and "reference machine" have no values anywhere |
| Phase 0 side effects | Unstated | `perf-ablation.ps1:443`, `:459` still set the streaming mode, creating a ninth dead variable. Default incremental becomes overlay plus compaction at 20,000 commits / 80,000 edges, which the hardware lane never exercises |

Credit where due: the blueprint's choice of a separate test binary and a 20,000-commit test with default settings is the right detector. The cheapest first step is even smaller: assert on the query results that `bin/perf-report.rs` already computes and discards, using its existing fast-import fixture generator (`bin/perf-report.rs:609-626`).

## Seven ADRs must change before they cite the blueprint

### The blocking changes

Eight changes block any ADR from citing the blueprint, because without them it would ship silent wrong answers or lost data:

1. **A single publisher.** Use a compare-and-swap on generation, plus a writer-identity check, so compaction cannot drop a layer and a second writer cannot appear.
2. **A cross-process reader protocol.** A shared lock while opening handles, an exclusive lock for GC, and readers that never trigger a rebuild.
3. **Named renumber triggers.** A shallow set that shrinks, or a change in replace refs or grafts, forces a blue/green renumber. Derived facts are keyed so they can be remapped.
4. **Raw-object ingestion.** Ingest under a stated ref policy, and record digests of every input that is not part of the OID.
5. **A shortest-path fix.** For `both`, plus an explicit "budget exhausted" result.
6. **An ID-independent output order,** or the explicit exclusion of ID-ordered outputs from the invariance contract.
7. **Fully pinned fixtures** with golden OIDs asserted on every OS.
8. **A corrected Phase 0** with a repair step and the incremental-pruning test.

### Changes by ADR

The table below assigns every required change to the ADR numbers the blueprint proposed.

| ADR (blueprint numbering) | Must change before it can cite the blueprint | Closes |
|---|---|---|
| **0019 Remove streaming CSR (Phase 0)** | Delete only the per-wave call site, the full-mode empty init and the env switches, and keep the builder for compaction. Repair existing indexes with a format bump or a rebuild on open. Add old-dated-merge and >8,192-commit incremental cases. Either hotfix the walk to stop only at indexed OIDs, or declare incremental unsound until Phase 4. Run the test as a subprocess with `env_clear()` and strictly increasing dates. Assert on perf-report results. Remove the streaming mode from `perf-ablation.ps1`/`perf-report.ps1` | Plan cannot compile; corrupted indexes persist; missed pruning bug |
| **0020 Portable automation, crates, lints** | State prerequisites as "Rust + platform C toolchain + git ≥2.32", with `cargo xtask doctor`. Add a guardrail-delta column per phase, mapping each removed string check to a behavior test (AGENTS rule 6). Port only behavioral gates, and delete each script exactly once. Choose a scoping mechanism for env bans (a config crate, or an explicit `#[expect(reason)]` carve-out). Ban `var`/`var_os`/`vars`/`vars_os`, both `canonicalize` forms and tempfile constructors without `_in`. Route git spawns through one helper. Make nextest the single runner, pinned. Credit the existing lints | Unscoped bans; rule-6 collisions; two test runners |
| **0021 Typed config and artifact contract** | Use `env_clear()` plus an allowlist plus the full gix-testtools set, plus `GIT_CEILING_DIRECTORIES`, `core.ignorecase=false`, explicit object format, ref format and branch, and `core.longpaths` on Windows. Commit golden fixture OIDs and assert them. The leak gate diffs before and after locally and checks absolutely only in fresh-checkout CI. Set `INSTA_UPDATE=no`, `CARGO_MUTANTS_OUTPUT` and disabled proptest persistence. Hash-check the project's `.git` refs before and after. Lock each run dir, and sweep only dirs that are unlocked AND old. Keep disk fixtures out of unit tests. Add `.gitattributes -text` for streams and goldens | Cross-OS fixture drift; hook and env leaks; gate false positives and false negatives |
| **0022 Layer format, manifest, LMDB removal** | Single publisher with a generation CAS. Tantivy-style shared and exclusive meta lock. GC only by the lock holder, with an mtime grace in minutes. Unique `seg-<gen>-<hash>` names. `MANIFEST-<gen>` plus a `CURRENT` pointer, with the last k generations kept live. Directory fsync on Unix. Bounded retry on every Windows rename and delete. Lock-identity check plus `writer_epoch`. Refuse writer mode on NFS, SMB or 9p. Free-space preflight, an ENOSPC abort rule and a blue/green catch-up. Re-inventory mutable state, and prefer a SQLite WAL catalog. Correct the fallbacks (redb only at ≥4.5 SingleWriter; heed needs an `unsafe` ADR). Re-derive write amplification against the Phase 0 baseline. Drop or define "max-commits". Price the open from the real chunk list, with lazy per-chunk verified reads. Weigh Arrow IPC. Fuzz the parser. Measure publish fsync cost per OS | Lost layers; cross-process GC; two writers; wrong numbers |
| **0023 Parent-first IDs and query kernel** | Name the renumber triggers. Limit the shortest-path interval to `parents`/`children` as [min, max], with an unrestricted bounded BFS for `both` and a tri-state result. Choose first-parent-first, or add `main_rank` plus a DFS `low` label. Make output order independent of IDs, with OID-keyed cursors. Use a max-heap topo stream and document the new truncation semantics. Add a children chunk plus a delta chunk. Add a visible bitset and a dead-ID compaction trigger. Use corrected commit date to order walks. Replace the segments/N kill criterion with a p99 latency criterion, and consider adopting `sapling-dag`. Correct the memory figures. Reserve the high ID bit | Wrong shortest paths; unstable outputs; missing indexes |
| **0024 Derived data and incremental model** | Ingest raw objects with replacements off. Record replace-set, shallow-set, mailmap and diff-policy digests. `fc@v1` holds exact renames only, with similarity renames as their own versioned type. `boundary` and `unavailable` never count as covered, and `unavailable` is written only after an ODB refresh. Pin `diff.algorithm` and disable textconv. Split merge and non-merge change sets, with separate targets. Store a per-merge TREESAME bit. Plan a path posting list and a cold-start partial-answer rule. Specify ref scope and store the exact snapshot walked. Use a content-addressed refs sidecar. Restart on a missing object. Treat the gix upgrade as a migration (replace polarity, graph constructor, `commit_graph_if_enabled` plus a shallow check, grafts refusal, cache invalidation, a linked-worktree fixture). Resolve git explicitly, require ≥2.45 for the reftable fallback, and return a typed error otherwise | "Never invalidated"; throughput; file history; reftable |
| **0025 Verification contract and perf gates** | Define digest = F(ref policy, format version, derived versions, DAG), computed with BLAKE3 or SHA-256 over a versioned encoding. Add a cross-OS fan-in job or committed golden digests. Match oracle refs to the product's policy, and fix the replace policy and the change-set command. Add a metadata oracle, an in-process reference model for queries and metamorphic tests on query outputs. Use small DAGs with the real wave parameter, a committed seed corpus, and failures saved as fast-import streams. Gate portable counters on all three OSes, and use gungraun only for single-thread kernels. Write numeric kill criteria now: reference machine, N cores, with and without a commit-graph, warm cache, median of five, cold-open ms, tape-walker margin, and p99 query latency during ingest. Drop "first" and fix the Scion citation | Oracle blind spots; ungated OSes; empty kill criteria; novelty overclaim |

The process model needs a decision in one of these ADRs as well. Either use a split process, with a worker that owns the writer lock, or keep in-process hosting with three safeguards: a supervisor that restarts the writer, `catch_unwind` at every command boundary, and no synchronous Tauri commands. The priority mechanism also needs one: small derive tasks with an "interactive pending" check between them, lower OS priority for indexer threads, never calling `install` across pools, and dropping old snapshots on a reclaimer thread.

## What was verified, measured, inferred and never run

| Evidence class | What it supports | Limits |
|---|---|---|
| Code read line by line | The edge-loss chain; the Phase 0 compile break; the guardrail collisions; incremental pruning; missing shallow/replace/graft guards; stale global caches; abort-then-re-emit; the audit confirmations; the absence of a Tauri dependency | No line was executed. The commit-window cursor and the 1,553-line perf formatter were not read line by line |
| Primary sources fetched | Rust 1.94 std rename/unlink/lock/fsync behavior; redb lock modes and bug history; heed's `unsafe` open; git's merge loop and expiry defaults; Tantivy's meta lock; flock semantics; gix 0.90 commits; Sapling idmap and iddag; git's two-mode tests; ALICE counts | The XDG spec, fcntl(2), Apple's `F_BARRIERFSYNC`, LevelDB's CURRENT/MANIFEST pattern, the PostgreSQL fsync-retry issue and gungraun's threading guidance were not fetched. They appear only inside inferences |
| Measured locally (git CLI) | Linux-slice and git.git throughput; the replace-graft diff change; notes and replace-ref effects on `rev-list`; the fast-import case collapse; Apple Git's reftable failure; the missing-identity failure | Single runs on one M1 Pro. A shallow, blobless 14-month Linux window, not full linux.git or chromium. No Windows or Linux host |
| Simulated (Python) | git.git numbering, the cascade, shortest path, topo stream, visit-order and segment statistics; the write-amplification table | Node counts, not Rust timings. One merge-heavy repository. The storage simulation models only the merge rule, not I/O or timing |
| Inference only | The lost-layer interleaving; the second writer after cache deletion; the cross-process GC race; OOM and C-ABI aborts in Tauri; NFS, SMB, 9p and Docker lock behavior; the incremental pruning failure scenario; Windows `MoveFileExW` error codes; linked-worktree commit-graph misses; disk-speed and change-layer size estimates; the HashMap memory arithmetic; the cost of the 100k-ref manifest; proptest spawn costs | Each needs a test or measurement before an ADR cites it as fact |

Several things were never run or measured at all:

- **Building and testing:** nothing in the repo was compiled, tested or benchmarked, because this Mac has no cargo and no pwsh.
- **gix itself:** no gix tree-diff speed, no gix replace-ref behavior on 0.80, no heap profile of `QueryGraph`.
- **Query and crash behavior:** no query latency under ingest, no crash-injection or ALICE-style test of the proposed publish protocol, no fsync latency on any OS.
- **Size and cost:** no change-layer size at 10M commits, no cost for the visibility sweep, no merge statistics for full linux.git or chromium.
- **Platforms:** no Windows run of the fixture or rename semantics.

The edge-loss bug and the incremental pruning bug are both confirmed only by reading code. The first executable proof should be the corrected Phase 0 test.

## Conclusion

The blueprint's breaks have the same shape as the bug it was written to fix. Today an edge falls between two write waves, and each wave looks correct on its own. In the blueprint:

- a layer falls between two publishers;
- a reader falls between two GC passes;
- a parent falls between a shallow clone and its unshallow;
- a query result falls between a full ingest and an incremental one.

In every case, two actors are individually correct, and the system has no rule that makes their combination correct. The lesson for the ADRs is narrower and more useful than "be careful". Every invariant needs a stated list of events that violate it, plus a defined response (compare-and-swap, renumber or re-derive). The verification contract must cover what users see, the query outputs, not only a digest that was designed to ignore IDs.

The red team also changed what should happen first. Phase 0 is still the right first move, but in corrected form: delete the per-wave call site rather than the builder, repair existing indexes, and add the old-dated-merge case that exposes the second wrong-answer bug. The cheapest detector already exists in the repo, in the query results `perf-report` computes and throws away. Everything else in the blueprint, including storage, IDs, the oracle and the novelty wording, is sound enough to keep and too loosely specified to cite until the changes above are written into ADRs 0019–0025 with numbers attached.
