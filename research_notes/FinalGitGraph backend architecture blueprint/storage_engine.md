# Storage engine and on-disk format for the FinalGitGraph git-history graph indexer (as of 2026-11-01)

Scope: pick the storage engine and file format for gitgraph-lab-core (Rust 1.94; gix 0.80; today: `lmdb` 0.8 crate, 17 named DBs, bincode values, string keys `"{repo_id:020}:{sha}"`, one bincode CSR blob per repo rewritten on every 16k-commit wave, overlay compacted at 20k). Targets: ingest at 100k+ commits/s, scale to 1M–10M+ commits, behave the same on Windows, macOS and Linux (x86_64 and arm64).

Evidence labels: **[measured]** means a number from a primary source or a check run in this session (crates.io API, GitHub API, upstream source grep). **[doc]** means official documentation or source code. **[inference]** means my reasoning; it is not verified.

Local repo facts checked this session:
- `apps/desktop/src-tauri/Cargo.toml` pins `lmdb = "0.8.0"`, `lmdb-sys = "0.8.0"` and `bincode = "1.3.3"`.
- `src/adapters/db.rs:22-23` sets the default map size to 4 GiB and the minimum to 512 MiB.
- `src/adapters/db.rs:94-109` can turn on `WRITE_MAP`, `MAP_ASYNC` and `NO_MEM_INIT`. It sets `NO_READAHEAD` only when not on Windows.

---

## Q1. How do LMDB (`lmdb`, `heed`), redb, fjall, sled, RocksDB, SQLite, DuckDB and canopydb compare for this workload?

### Takeaway
No general-purpose KV store wins on every axis.
- LMDB is still fastest on reads and near-fastest on bulk load. But the `lmdb` crate this repo uses is unmaintained (last release 2018-03-03) and bundles LMDB 0.9.21 from 2017.
- RocksDB and fjall win on batch writes and file size. They pay for it with LSM compaction.
- redb is the strongest pure-Rust, mmap-free B+tree. It is actively maintained but has had serious bugs recently.
- sled and canopydb are not production-grade.
- None of these engines is needed for the immutable CSR graph itself (see Q3).

### Cited Findings

**Maintenance status (crates.io / GitHub API, queried 2026-11-01) [measured]**
- `lmdb` (danburkert): latest 0.8.0, released 2018-03-03. The repo was last pushed 2023-04-24 and is not archived, but has no releases since 2018. **Unmaintained for 8+ years.** — [crates.io lmdb](https://crates.io/api/v1/crates/lmdb), [GitHub danburkert/lmdb-rs](https://github.com/danburkert/lmdb-rs)
- `lmdb-sys` 0.8.0 pins its LMDB submodule at commit `60d5002`, which is the "Release 0.9.21" commit dated 2017-06-01. Upstream LMDB 0.9.36 shipped 2026-08-06, and 0.9.37 is in engineering. **The repo's LMDB C code is 15 point releases behind.** — [lmdb-sys submodule](https://github.com/LMDB/lmdb/tree/60d500206a108b2c64ca7e36b0113b2cd3711b98), [LMDB 0.9 CHANGES](https://raw.githubusercontent.com/LMDB/lmdb/mdb.RE/0.9/libraries/liblmdb/CHANGES)
- `lmdb-rkv` (Mozilla fork): last release 0.14.0 on 2020-03-10. **The repo is archived.** — [crates.io lmdb-rkv](https://crates.io/crates/lmdb-rkv), [GitHub mozilla/lmdb-rs](https://github.com/mozilla/lmdb-rs)
- `heed` / `heed3` (Meilisearch): 0.23.1, released 2026-10-26, about 7.4M downloads. It is active. It builds on `lmdb-master-sys` 0.2.6 (2026-04-07), which bundles the LMDB `mdb.master` (1.0-dev) branch. — [crates.io heed](https://crates.io/crates/heed), [lmdb-master-sys](https://github.com/meilisearch/heed/tree/main/lmdb-master-sys)
- `redb`: 4.4.0, released 2026-10-13, about 14.9M downloads. Repo pushed 2026-11-01. Major versions: 1.0 on 2023-06-16, 3.0 on 2025-08-09, 4.0 on 2026-04-02. — [crates.io redb](https://crates.io/crates/redb), [redb CHANGELOG](https://github.com/cberner/redb/blob/master/CHANGELOG.md)
- `fjall`: 3.1.13, released 2026-10-25. 3.0.0 shipped 2026-01-02 and 2.0.0 on 2024-09-20. — [crates.io fjall](https://crates.io/crates/fjall). The fjall-3 blog page rendered a date of "September 15, 2026", which conflicts with the crates.io release date of 2026-01-02. Trust crates.io. — [fjall 3 post](https://fjall-rs.github.io/post/fjall-3/)
- `sled`: latest stable 0.34.7. The newest prerelease is 1.0.0-alpha.124, from 2024-10-11. The last commit on main is 2026-04-04 (a benchmark). The README says "This README is out of sync with the main branch which contains a large in-progress rewrite", "if reliability is your primary constraint, use SQLite. sled is beta", and that the on-disk format "is going to change in ways that require manual migrations" before 1.0. **Not production-suitable.** — [sled README](https://github.com/spacejam/sled)
- RocksDB bindings come in two crates:
  - `rocksdb` 0.25.0, released 2026-08-16. It releases about once a year (0.23 on 2024-12-24, 0.24 on 2025-08-10).
  - The fork `rust-rocksdb` 0.54.0, released 2026-10-26, releases monthly.
  - Both wrap C++ RocksDB. — [crates.io rocksdb](https://crates.io/crates/rocksdb), [crates.io rust-rocksdb](https://crates.io/crates/rust-rocksdb)
- `rusqlite`: 0.41.1, released 2026-10-28, about 141M downloads. — [crates.io rusqlite](https://crates.io/crates/rusqlite)
- `duckdb` (duckdb-rs): 1.20000.1, released 2026-10-28. — [crates.io duckdb](https://crates.io/crates/duckdb)
- `canopydb`: 0.2.5, released 2025-11-22. Releases have slowed (0.2.4 was 2025-04-15). It has 162 stars. The README says "should be considered early stage software", "Do not trust it with production data", and that it has been rewritten "a few times". — [crates.io canopydb](https://crates.io/crates/canopydb), [GitHub canopydb](https://github.com/arthurprs/canopydb)
- Other pure-Rust stores: `nebari` 0.5.5 (2023-02-27) and `okaywal` 0.3.1 (2023-11-26) have had no releases for about 3 years and are effectively dormant. `persy` 1.8.1 (2026-06-30) is active but niche. — crates.io API (see the queries above).

**Benchmarks (redb README, Ryzen 9950X3D + Samsung 9100 PRO NVMe) [measured by the redb author; the item count and value sizes are not stated on the page]** — [redb README](https://github.com/cberner/redb)

| test | redb | lmdb | rocksdb | fjall | sqlite |
|---|---|---|---|---|---|
| bulk load | 17063ms | **9232ms** | 13969ms | 18619ms | 15341ms |
| individual writes | **920ms** | 1598ms | 2432ms | 3488ms | 7040ms |
| batch writes | 1595ms | 942ms | 451ms | **353ms** | 2625ms |
| random reads | 1138ms | **637ms** | 2911ms | 2177ms | 4283ms |
| random range reads | 1174ms | **565ms** | 2734ms | 2564ms | 8431ms |
| random reads, 32 threads | 410ms | **125ms** | 1100ms | 576ms | 26536ms |
| uncompacted size | 4.00 GiB | 2.61 GiB | **893 MiB** | 1000.95 MiB | 1.09 GiB |
| compacted size | 1.69 GiB | 1.26 GiB | **454.71 MiB** | 1000.95 MiB | 556.85 MiB |

- In the fjall 3 benchmarks (Ryzen 9950X3D, RAM capped at 8 GB, ext4):
  - redb ran with `Durability::None`, and heed ran with `MDB_NOSYNC`, which the post says "may not be safe to use". SQLite ran with `synchronous=NORMAL`.
  - sled was excluded from several runs because it "does not reliably fsync".
  - SQLite "basically stopp[ed] to work completely" on the 50M-user feed workload.
  - The post publishes charts but gives no numbers in its text.
  - Source: [fjall 3 post](https://fjall-rs.github.io/post/fjall-3/)

**Design and semantics [doc]**
- **redb** stores data in "copy-on-write B+trees". It offers "MVCC support for concurrent readers & writer, without blocking", is "Crash-safe by default", and its file format is "stable". — [redb README](https://github.com/cberner/redb)
  - All data is checksummed "using a non-cryptographic Merkle tree with XXH3_128". The default commit is 1PC+C: one fsync, with checksums and transaction ids used to detect torn writes. 2PC is optional. — [redb design.md](https://github.com/cberner/redb/blob/master/docs/design.md)
  - Freed pages are "only reused after all read transactions that could reference it have completed". This is the same long-reader growth hazard LMDB has. — [redb design.md](https://github.com/cberner/redb/blob/master/docs/design.md)
- **redb has no mmap.** In 0.14.0 (2023-03-26), "The mmap backend has been removed because it was infeasible to prove that it was sound. This makes the redb API entirely safe, and the remaining `File` based backed is within a factor of ~2x on all workloads". — [redb CHANGELOG](https://github.com/cberner/redb/blob/master/CHANGELOG.md)
- **redb has recent serious bugs.**
  - 4.0.0 (2026-04-02) fixed "a critical bug where the accessor could outlive the `Table` … This could cause data loss".
  - 4.4.0 (2026-10-13) fixed "`Durability::None` commits being lost on a clean close", "unbounded memory growth in the page cache under write workloads", and `compact()` hangs.
  - Source: [redb CHANGELOG](https://github.com/cberner/redb/blob/master/CHANGELOG.md)
- **fjall** is an LSM-tree, "100% safe & stable Rust", with keyspaces that have "cross-keyspace atomic semantics". It offers optional `SingleWriterTxDatabase` and `OptimisticTxDatabase` transactions, and optional KV separation for blobs. By default, writes go to OS buffers only ("matches RocksDB's default durability"). "A single database may not be opened from separate processes in parallel." — [fjall GitHub](https://github.com/fjall-rs/fjall)
  - v3 changed the block format and added 128-bit xxh3 checksums on by default. Blob GC now runs during compaction. v3 uses "Rust's file locking API" to exclusively lock the database. — [fjall 3 post](https://fjall-rs.github.io/post/fjall-3/)
- **canopydb** offers B+tree MVCC, multiple writers with OCC/SI, an "Optional WAL" with background fsync, and periodic checkpoints. — [GitHub canopydb](https://github.com/arthurprs/canopydb)
- **SQLite WAL** gives one writer and many readers, and "readers do not block writers and a writer does not block readers".
  - With `synchronous=NORMAL`, commits skip the fsync, so "transactions are no longer durable and might rollback following a power failure".
  - "if … there is always at least one active reader, then no checkpoints will be able to complete and hence the WAL file will grow without bound".
  - "WAL does not work over a network filesystem".
  - A WAL-reset corruption bug affects SQLite 3.7.0–3.51.2. It is fixed in 3.51.3 and later, with backports to 3.44.6 and 3.50.7.
  - Source: [SQLite WAL](https://www.sqlite.org/wal.html)
- **DuckDB** allows "one process can both read and write", or "multiple processes can read … but no processes can write" in `READ_ONLY` mode. "Appends will never conflict". Multi-process writes need the Quack client-server protocol or DuckLake. — [DuckDB concurrency](https://duckdb.org/docs/2.0/connect/concurrency.html)

### Inferences
- **Native dependencies [inference]:** RocksDB (C++), SQLite (C, via `rusqlite` `bundled`), DuckDB (C++) and LMDB (C) all need a C/C++ toolchain to cross-compile for each target. redb, fjall and canopydb are pure Rust, so they cross-compile like the rest of the crate.
- **Bulk load [inference]:** LMDB leads in the redb bulk-load row. For this workload, though, the cost lies in key design, not in engine choice. The keys are 61-byte strings (`020-digit id` + `:` + 40-hex sha), across 17 tables, with bincode values. Moving to dense u32 commit ordinals (as git does) would shrink keys about 15x in any engine.
- **LSM engines [inference]:** fjall and RocksDB write about 2–3x less data in the batch-writes row and produce the smallest files. That pays off for the random-insert-heavy diff/file-change tables. It does not help a graph that is append-only by construction.
- **Tier [inference]:** sled (beta, does not reliably fsync, format unstable) and canopydb ("Do not trust it with production data") are out. nebari and okaywal are dormant.

### Gaps
- The redb benchmark page does not state its item count, key size or value size, so its milliseconds cannot be turned into commits per second.
- The fjall 3 numbers exist only as charts. No text values were extracted.
- No independent 2025–2026 benchmark covers canopydb.
- I did not verify whether `heed` prevents opening the same environment twice in one process, for example through a global registry. Check this in the heed source before relying on it.

---

## Q2. Which documented LMDB caveats apply here?

### Takeaway
Upstream lmdb.h names every reported pain point:
- Opening the same environment twice in one process is forbidden.
- Long-lived readers make the file grow.
- `WRITEMAP` and `MAPASYNC` trade crash safety for speed.
- The map size is a hard cap.
- On the LMDB 0.9 branch this repo bundles, Windows pre-extends the data file to the full map size. Incremental growth exists only on the 1.0-dev `mdb.master` branch, which heed uses, and upstream says it has "a large performance impact".

### Cited Findings (verbatim from lmdb.h / mdb.c [doc])
- "Do not have open an LMDB database twice in the same process at the same time. Not even from a plain open() call - close()ing it breaks fcntl() advisory locking." — [lmdb.h (mdb.master)](https://raw.githubusercontent.com/LMDB/lmdb/mdb.master/libraries/liblmdb/lmdb.h)
- "Avoid long-lived transactions. Read transactions prevent reuse of pages freed by newer write transactions, thus the database can grow quickly. Write transactions prevent other write transactions, since writes are serialized." — [lmdb.h](https://raw.githubusercontent.com/LMDB/lmdb/mdb.master/libraries/liblmdb/lmdb.h)
- "Do not use LMDB databases on remote filesystems, even between processes on the same host." — [lmdb.h](https://raw.githubusercontent.com/LMDB/lmdb/mdb.master/libraries/liblmdb/lmdb.h)
- `MDB_WRITEMAP`: "uses fewer mallocs but loses protection from application bugs like wild pointer writes and other bad updates into the database. This may be slightly faster for DBs that fit entirely in RAM, but is slower for DBs larger than RAM. Incompatible with nested transactions. Do not mix processes with and without MDB_WRITEMAP on the same environment. This can defeat durability". — [lmdb.h](https://raw.githubusercontent.com/LMDB/lmdb/mdb.master/libraries/liblmdb/lmdb.h)
- `MDB_MAPASYNC`: "When using MDB_WRITEMAP, use asynchronous flushes to disk. As with MDB_NOSYNC, a system crash can then corrupt the database or lose the last transactions." — [lmdb.h](https://raw.githubusercontent.com/LMDB/lmdb/mdb.master/libraries/liblmdb/lmdb.h)
- `MDB_NOLOCK`: "Don't do any locking. If concurrent access is anticipated, the caller must manage all concurrency itself." `MDB_NORDAHEAD`: "The option is not implemented on Windows." — [lmdb.h](https://raw.githubusercontent.com/LMDB/lmdb/mdb.master/libraries/liblmdb/lmdb.h)
- `mdb_env_set_mapsize` rules:
  - The default is 10485760 bytes.
  - "The size of the memory map is also the maximum size of the database."
  - "only mapsize increases are persisted into the environment."
  - A new size "will not be persisted to any others until a write transaction has been committed".
  - Source: [lmdb.h](https://raw.githubusercontent.com/LMDB/lmdb/mdb.master/libraries/liblmdb/lmdb.h)
- The default `MDB_MAXKEYSIZE` is 511 bytes. — [lmdb.h](https://raw.githubusercontent.com/LMDB/lmdb/mdb.master/libraries/liblmdb/lmdb.h)
- The caveats say stale reader slots after a crash are cleared automatically on "Windows - automatic" and on Linux with robust POSIX mutexes. Otherwise, the caveats say to "make all programs using the database close it". — [lmdb.h](https://raw.githubusercontent.com/LMDB/lmdb/mdb.master/libraries/liblmdb/lmdb.h)
- **Windows file size on the 0.9 branch.** The comment in `mdb.c` reads: "Windows won't create mappings for zero length files. and won't map more than the file size. Just set the maxsize right now." When `MDB_WRITEMAP` is not set, the code calls `SetEndOfFile` at the map size. — [mdb.c (mdb.RE/0.9)](https://raw.githubusercontent.com/LMDB/lmdb/mdb.RE/0.9/libraries/liblmdb/mdb.c)
- **Windows file size on `mdb.master`** (1.0-dev, heed): "Some users are afraid of seeing their disk space getting used all at once, so the default is now to do incremental file growth. But that has a large performance impact, so give the option of allocating the file up front." It uses `NtCreateSection(... SEC_RESERVE ...)`, and `MDB_FIXEDSIZE` restores preallocation. — [mdb.c (mdb.master)](https://raw.githubusercontent.com/LMDB/lmdb/mdb.master/libraries/liblmdb/mdb.c)
- **Mailing-list reports (2014–2018), not official docs:**
  - On Windows a 1 GiB map size produced a 1 GiB file, while on Linux the file tracked the data.
  - One user reported about 30% slowdown with sparse files.
  - The maintainer rejected "set large and shrink at shutdown" as "No. That would be stupid."
  - Source: [openldap-technical thread](https://lists.openldap.org/hyperkitty/list/openldap-technical@openldap.org/thread/NLJCBSEOCNTJ6URQD2SSC2NYVIVHD4Q7)

### Inferences
- **Windows disk use [inference]:** With the bundled LMDB 0.9.21 and a 4 GiB map, Windows users should see a 4 GiB data file per environment from first open. With `WRITE_MAP`, `CreateFileMapping` at the map size likely extends the file in the same way. That source path was not traced. macOS (APFS) and Linux grow the file sparsely. This is a real OS-parity break.
- **Reopen per command [inference]:** Reopening the environment on every command works around the double-open rule. A single process-wide `Env` handle (OnceLock) would remove both the reopen cost and the hazard. Moving the hot graph out of LMDB also shrinks how much this matters.
- **Optional flags [inference]:** `db.rs` can enable `WRITE_MAP | MAP_ASYNC`. That combination is documented to let a system crash "corrupt the database". It must never be the default for user data.

### Gaps
- No primary source says whether LMDB 0.9.x on Windows with `MDB_WRITEMAP` grows the file incrementally. The 0.9 code skips `SetEndOfFile` under `WRITEMAP`, but `CreateFileMapping` is given the full size.
- I did not check which 0.9.22–0.9.36 fixes affect this repo's usage.

---

## Q3. Would a custom immutable, versioned segment format beat a KV store for the CSR graph? What do the precedent formats do, and what are the per-OS atomic-rename and fsync semantics?

### Takeaway
Yes. Git's commit-graph is a production-proven design for exactly this data:
- a chunked, checksummed file,
- dense u32 positions,
- an incremental "split chain" of layers merged at a size multiple of 2.

Copying that design, with little-endian 64-byte-aligned chunks instead of network byte order, replaces the O(N²/batch) whole-blob rewrite with O(N log N) total writes. It also needs no KV engine for the graph. Atomic publish works the same way on all three OSes:
1. write a temp file,
2. call `File::sync_all`,
3. rename to a new content-addressed name,
4. replace a small manifest file.

Rust std already maps `sync_all` to `F_FULLFSYNC` on Apple, `FlushFileBuffers` on Windows and `fsync` on Linux. It maps `rename` to POSIX-semantics rename on Windows.

### Cited Findings

**Git commit-graph (gitformat-commit-graph) [doc]** — [gitformat-commit-graph](https://git-scm.com/docs/gitformat-commit-graph)
- **Byte order and limits:** "All multi-byte numbers are in network byte order." Positions are u32, with a maximum of about 1.8 billion commits.
- **Header:** `CGPH`, version 1, hash version (1 = SHA-1, 2 = SHA-256; on mismatch "the commit-graph file should be ignored with a warning"), chunk count, and base graph count.
- **Chunk lookup table:** "(C + 1) * 12 bytes", made of a 4-byte ID plus an 8-byte offset.
- **Chunks:**
  - `OIDF` is a 256×u32 fanout.
  - `OIDL` holds the sorted OIDs.
  - `CDAT` holds N×(H+16): tree OID, 2 parent positions with `0x70000000` meaning none, and generation plus commit time.
  - `EDGE` holds octopus parents.
  - `GDA2`/`GDO2` hold corrected commit dates.
  - `BIDX`/`BDAT` hold changed-path Bloom filters.
  - `BASE` holds the chain.
- **Trailer:** "H-byte HASH-checksum of all of the above."
- **Alignment:** the format defines none.
- **Versioning lesson:** `GDA2`/`GDO2` carry a "2" because "a previous version of Git wrote possibly erroneous data in these chunks with the IDs "GDAT" and "GDOV"". Chunk IDs work as an in-place versioning mechanism.

**Split commit-graph chains (technical/commit-graph) [doc]** — [git commit-graph design](https://git-scm.com/docs/commit-graph)
- Layer files are named `graph-{hash}.graph` ("hash of the file's contents before that hash"). A text file `commit-graph-chain` lists them from lowest to highest.
- Global position = i + the sum of commit counts in the lower layers.
- **Write protocol:** "write the contents to a temporary file, all while holding a `commit-graph-chain.lock` lock-file. When the file is flushed, we rename it to `graph-{hash3}` … Finally, we write the new chain data to `commit-graph-chain.lock`".
- **Merge rules:** a layer is merged down if "the number of commits in level N is less than X times the number of commits in level N + 1" (default X = 2), or if level N+1 exceeds `--max-commits` (default 64,000). "This decision cascades down the levels", which keeps the layer count logarithmic.
- **Deletion:** removal of unreferenced layers is delayed by an expiry window, because "another process could read the `commit-graph-chain` file before it is rewritten, but then look for the `graph-{hash}` files after they are deleted."

**Evidence that a positional graph file pays off [measured by git developers]**
- Parsing from the commit-graph instead of commit objects gave a "5-10x speedup for certain Git commands". — [Microsoft DevBlogs, Supercharging the Git Commit Graph II (2018)](https://devblogs.microsoft.com/devops/?p=44805)
- On Linux, `git log --graph` returning ten results took about 6.1 s without the commit-graph and about 9 ms with it (a 650x speedup). — [Stolee, git list draft 2019](https://lore.kernel.org/r/20191021204253.10196-1-dstolee@microsoft.com)
- Changed-path Bloom filters gave 10x–30x speedups on `git log -- path`. — [GitGitGadget PR #497](https://github.com/gitgitgadget/git/pull/497)

**Apache Arrow IPC [doc]** — [Arrow columnar format](https://arrow.apache.org/docs/format/Columnar.html)
- "Implementations are recommended to allocate memory on aligned addresses (multiple of 8- or 64-bytes)"; "prefer using 64-byte alignment and padding". The IPC minimum is 8 bytes.
- "The Arrow IPC format is little-endian by default." The Schema carries an endianness field. On mismatch, some readers byte-swap, and "other implementations may simply refuse reading such data".
- Metadata uses FlatBuffers. Messages have a `0xFFFFFFFF` continuation marker, a little-endian length prefix, and a total length that is a multiple of 8.
- The file format is bracketed by `ARROW1` and has a footer that indexes record batches for random access. It is "Relocatable without 'pointer swizzling', allowing for true zero-copy access".
- `arrow-ipc` 60.1.0 was released 2026-10-28 and is active. — [crates.io arrow-ipc](https://crates.io/crates/arrow-ipc)

**rkyv [doc]**
- rkyv guarantees "no data is copied during deserialization", and access is "a pointer offset and a cast". Alignment matters: some types can't be borrowed because "our buffer might not be properly aligned". — [rkyv book](https://rkyv.org/zero-copy-deserialization.html)
- rkyv 0.8.18 (2026-08-05). Its `Cargo.toml` features include `little_endian`, `big_endian`, `aligned` and `unaligned`, and `bytecheck` validation is on by default. — [rkyv Cargo.toml](https://github.com/rkyv/rkyv/blob/main/rkyv/Cargo.toml), [crates.io rkyv](https://crates.io/crates/rkyv)

**Atomic rename and fsync per OS [doc]**
- **Rust std on Apple:** `sync_all` and `sync_data` both call `libc::fcntl(fd, libc::F_FULLFSYNC)` (`#[cfg(target_vendor = "apple")]`). Other Unix targets use `fsync`, or `fdatasync` on Linux. — [rust std sys/fs/unix.rs](https://github.com/rust-lang/rust/blob/master/library/std/src/sys/fs/unix.rs)
- **macOS fsync(2)** (local man page): plain `fsync()` moves data to the drive, but "the drive itself may not physically write the data … and it may be written in an out-of-order sequence … This is not a theoretical edge case."
  - fcntl(2) `F_FULLFSYNC` "asks the drive to flush all buffered data to the permanent storage device … implemented on HFS, MS-DOS (FAT), Universal Disk Format (UDF) and APFS".
  - `F_BARRIERFSYNC` is a cheaper ordering-only barrier.
  - Source: [Apple fsync(2) archive](https://developer.apple.com/library/ios/documentation/system/conceptual/manpages_iphoneos/man2/fsync.2.html) (and `man 2 fcntl` on Darwin 24.6, checked locally).
- **Rust std on Windows:**
  - `fsync` and `datasync` both call `FlushFileBuffers`.
  - `rename` calls `rename_inner(..., MOVEFILE_REPLACE_EXISTING, FILE_RENAME_FLAG_REPLACE_IF_EXISTS | FILE_RENAME_FLAG_POSIX_SEMANTICS)`. This is a POSIX-semantics `FileRenameInfoEx` rename, with a fallback to `MoveFileExW`.
  - Default `share_mode` is `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE`.
  - Source: [rust std sys/fs/windows.rs](https://github.com/rust-lang/rust/blob/master/library/std/src/sys/fs/windows.rs)
- **MoveFileExW:**
  - `MOVEFILE_REPLACE_EXISTING` replaces the destination, "provided that security requirements regarding access control lists (ACLs) are met".
  - `MOVEFILE_WRITE_THROUGH`: "The function does not return until the file is actually moved on the disk". This matters only for copy-and-delete moves across volumes.
  - The page makes no explicit atomicity guarantee. It points to `MoveFileTransacted` for transacted moves.
  - Source: [Microsoft Learn MoveFileExW](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw)

### Inferences
**Write amplification today vs a layered design [inference, arithmetic]**
- At 10M commits, the CSR is about 20 B sha + about 8 B parent offset/neighbor + about 8 B child offset/neighbor per commit, roughly 360 MB.
- Rewriting it whole every 16k commits means about 625 rewrites averaging about 180 MB, roughly 110 GB written.
- A git-style chain with a size multiple of 2 rewrites each commit about log2(625) ≈ 9–10 times, roughly 3.5 GB in total. The design point is a 30x reduction.
- The raw graph payload at 100k commits/s is only about 4–6 MB/s. The bottleneck is per-key KV overhead, not bytes.

**Recommended segment layout [inference, combining git and Arrow]**
- Use a header `magic + format_version + hash_kind + endianness marker + chunk_count`.
- Use a chunk table of `[u32 id, u64 offset]`, as in git. Readers skip unknown chunk IDs, which gives forward compatibility. A changed meaning gets a new chunk ID, following the GDA2 lesson.
- Use fixed little-endian. All targets (x86_64, arm64 on all three OSes) are LE, so reads are zero-cost. Reject files with the wrong endianness marker, which is the Arrow "refuse" option.
- Align every chunk start to 64 bytes, per Arrow, so `&[u32]` casts via `bytemuck`/`zerocopy` are always valid.
- Store a trailer checksum. Optionally store per-chunk XXH3-128, as redb and fjall do.
- **Chunks to carry:**
  - sorted SHA plus a 256-entry fanout, for O(log n) sha→position lookup,
  - parent CSR (u32 positions; a sentinel for "none"; an octopus EDGE list),
  - generation / corrected date,
  - a commit-time column,
  - optionally changed-path Bloom filters fed by the async diff extractor.
- **Children:** git does not store them. A child edge always lives in a newer layer than its parent. Each layer can carry a reverse-edge chunk keyed by global parent position, and `children(c)` is the union over layers ≥ layer(c). Merges collapse the union. Total reads stay O(layers) = O(log N).
- **Prefer plain `repr(C)` / `bytemuck` arrays over rkyv for the CSR.** The data is flat u32/u64 arrays. rkyv's relative pointers add nothing here, and its format couples to rkyv's version. Keep rkyv or bincode only for small, irregular metadata, if at all.

**Publish protocol [inference]**
1. Write `seg-<gen>.tmp` in the same directory.
2. Call `sync_all` (F_FULLFSYNC, FlushFileBuffers or fsync, depending on the OS).
3. Rename to `seg-<hash>.seg`. The name is new, so nothing mapped is ever replaced.
4. Write `manifest.tmp`, call `sync_all`, then `rename` over `manifest`.
5. On Unix, fsync the directory. Linux fsync(2) says a directory fsync is needed for the entry to be durable. That page was not fetched this session, so verify it.

Only the small manifest is ever replaced. Readers `read()` it and close it, never mmap it. This avoids the Windows rule that mapped files cannot be renamed over (see Q4).

### Gaps
- I did not fetch the Lance format, Parquet, FlatBuffers or `gitformat-chunk(5)` / multi-pack-index docs this session.
  - I believe Parquet's compressed, encoded pages are not zero-copy mmappable and that Lance targets random access on Arrow. These are unverified here.
- No primary Microsoft statement guarantees atomicity of `FileRenameInfoEx` with POSIX semantics against power loss.
- On Windows, std's directory fsync equivalent is not available. Windows durability of the directory entry after rename relies on NTFS metadata journaling. This is not verified from a primary source.

---

## Q4. What is the safety story of mmap in Rust, and how do production systems mitigate it?

### Takeaway
File-backed mmap is inherently `unsafe` in Rust:
- Any external modification or truncation is UB.
- On Unix, a read past the new end-of-file raises SIGBUS.
- On Windows, a mapped file cannot be truncated, deleted or renamed over.

redb removed its mmap backend for this reason and lost at most about 2x performance. The robust portable pattern:
- Immutable, never-rewritten segment files.
- Verify checksums at open.
- Default to a `read()` into an aligned heap buffer, which is fully safe.
- Make mmap an opt-in for larger-than-RAM repos only.

### Cited Findings
- memmap2 0.9.12 (2026-10-21) docs: "All file-backed memory map constructors are marked `unsafe` because of the potential for Undefined Behavior (UB) using the map if the underlying file is subsequently modified, in or out of process. Applications must consider the risk and take appropriate precautions". Also: "Resizing the memory mapping beyond the end of the mapped file will result in UB". — [memmap2 Mmap docs](https://docs.rs/memmap2/latest/memmap2/struct.Mmap.html), [crates.io memmap2](https://crates.io/crates/memmap2)
- Linux mmap(2):
  - SIGBUS: "Attempted access to a page of the buffer that lies beyond the end of the mapped file."
  - MAP_PRIVATE: "It is unspecified whether changes made to the file after the mmap() call are visible in the mapped region."
  - "The effect of changing the size of the underlying file of a mapping on the pages that correspond to added or removed regions of the file is unspecified."
  - Source: [man7 mmap(2)](https://man7.org/linux/man-pages/man2/mmap.2.html)
- redb 0.14.0: "The mmap backend has been removed because it was infeasible to prove that it was sound … the remaining `File` based backed is within a factor of ~2x on all workloads that I've benchmarked". — [redb CHANGELOG](https://github.com/cberner/redb/blob/master/CHANGELOG.md)
- Windows `ERROR_USER_MAPPED_FILE` (1224): "The requested operation cannot be performed on a file with a user-mapped section open."
  - A JDK report says: "Once a file has been mapped a number of operations on that file will fail until the mapping has been released (e.g. delete, truncating to a size less than the mapped area)".
  - Another JDK report says: "Memory-Mapped files cannot be renamed on Windows", and `Files.move(..., REPLACE_EXISTING)` throws. — [JDK-8028683](https://bugs.openjdk.org/browse/JDK-8028683), [JDK-4724038](https://bugs.openjdk.org/browse/JDK-4724038)
  - SQLite's Windows VFS makes truncate a no-op while mmap pages are referenced, because "truncating the file means temporarily unmapping the file, and that might delete memory out from under existing cursors". — [SQLite check-in](https://www.sqlite.org/src/info/75a85a1c6ac59f2d)
  - Chromium found "file truncation silently fails when the file is memory mapped" on Windows. — [Chromium review 2830263002](https://codereview.chromium.org/2830263002)
  - These are secondary sources (JDK, SQLite, Chromium) aggregated by search. I found no single Microsoft API page that states all of these rules.
- SQLite itself uses mmap only for the WAL index: "The wal-index is implemented using an ordinary file that is mmapped for robustness … rarely exceeds 32 KiB in size and is never synced." — [SQLite WAL](https://www.sqlite.org/wal.html)

### Inferences
- **Risk sources [inference]:** The app writes segments only once, before publishing them. The main risk is external tampering: users, antivirus, or OneDrive/Dropbox/iCloud sync clients rewriting files in the app-data directory. A checksum at open detects corruption but cannot stop a modification made after open.
- **Reading strategy [inference]:**
  - A 10M-commit CSR of about 360 MB is a single sequential read, about 0.1–0.3 s on NVMe (estimate, not measured).
  - Reading it into an aligned `Vec`/`Box<[u8]>` and casting with `bytemuck`/`zerocopy` is fully safe, zero-parse and identical on every OS.
  - The process-global cache already holds the whole CSR, so mmap's lazy-paging advantage is small.
  - Keep mmap behind a size threshold and advisory locks. Never map a file that will be replaced or truncated.
- **Windows deletes [inference]:** Garbage collection must tolerate deletes that fail while old segments are mapped or open. Use git's approach: mark the segment unreferenced, delete it later or at next startup, and retry on error.

### Gaps
- No Rust-ecosystem study measures `read()` vs `mmap` load time for 100 MB–1 GB immutable graph files across all three OSes. This should be benchmarked in-repo with `quality:perf`.

---

## Q5. What is the smallest robust store for mutable state (retry queues, checkpoints, metadata)?

### Takeaway
Either redb or SQLite works. redb is the smallest fit for a pure-Rust, OS-uniform build:
- one file,
- ACID with single-writer MVCC,
- no mmap, no C toolchain,
- XXH3 Merkle checksums.

Its 2026 data-loss fixes mean pinning to 4.4.0 or later and running integrity checks in tests. SQLite (`rusqlite` with `bundled`, WAL mode) has the more battle-tested crash record. Pick it if a C toolchain is acceptable or ad-hoc SQL over the queue is useful. Either way, the volume is small (queue rows plus checkpoints), so engine throughput does not matter.

### Cited Findings
- redb: "Crash-safe by default", "Fully ACID-compliant transactions", MVCC for concurrent readers and one writer, and savepoints and rollbacks. The file format is stable. — [redb README](https://github.com/cberner/redb)
  - The default 1PC+C commit uses a single fsync. 2PC is available against checksum-collision attacks. — [redb design.md](https://github.com/cberner/redb/blob/master/docs/design.md)
  - v3 shrank the minimum file "from ~2.5MiB to ~50KiB" and made bulk load about 15% faster. — [redb CHANGELOG](https://github.com/cberner/redb/blob/master/CHANGELOG.md)
  - Its 2026 fixes: a critical data-loss accessor bug (4.0.0, 2026-04-02), lost `Durability::None` commits, and page-cache memory growth (4.4.0, 2026-10-13). — [redb CHANGELOG](https://github.com/cberner/redb/blob/master/CHANGELOG.md)
- SQLite WAL: one writer and many readers, with checkpoint starvation under constant readers. `synchronous=NORMAL` is not durable on power loss. A WAL-reset bug needs SQLite ≥ 3.51.3. — [SQLite WAL](https://www.sqlite.org/wal.html)
- fjall's default durability writes to OS buffers only, and it needs an explicit `persist`. — [fjall GitHub](https://github.com/fjall-rs/fjall)

### Inferences
- **Fit [inference]:** The retry queue and checkpoints have a classic B+tree access pattern: small read-modify-write ops, point lookups and short scans. redb or SQLite fits well. An LSM (fjall, RocksDB) adds compaction machinery with no benefit at this size.
- **Lifecycle [inference]:** Open the redb `Database` once per process and keep it for the process's lifetime. This removes the current reopen-per-command pattern. redb does support multi-process access with a single writer.
- **Per-commit metadata [inference]:**
  - Author and subject should move out of per-key KV tables into columnar chunks in the immutable segments, keyed by position, with a string heap for subjects.
  - The KV store then holds only O(repos + queue) records, not O(commits). This eliminates most of the 17 tables, the string keys and bincode on hot paths.
  - The file-change/diff table may stay in KV if it is sparse and keyed by (position u32, path id u32). It could also become its own append-only segment family.

### Gaps
- No public crash-consistency torture test (for example, ALICE-style) covers redb 4.x or fjall 3.x. SQLite's testing is far more extensive, but I did not fetch its testing page this session.

---

## Q6. Is there 2022–2026 evidence on embedded KV vs custom columnar files for graph or analytics workloads? And what design should FinalGitGraph adopt?

### Takeaway
Direct head-to-head benchmarks (KV vs custom CSR file for a commit DAG) were not found. Strong indirect evidence favors purpose-built columnar/CSR files:
- git's commit-graph speedups (5–10x generally, 650x for `log --graph` top 10),
- Kuzu's design, which stores edges as columnar, disk-based CSR join indices (Kuzu itself was archived in October 2025),
- the KV benchmarks above, which show 2–4 GiB files and per-key overhead for data that is naturally dense arrays.

Recommended design: a hybrid. Immutable, checksummed, git-style layered segment files hold the graph and per-commit columns. A small redb (or SQLite) file holds mutable state.

### Cited Findings
- Kuzu is described as "single node (multi-core), disk-based, transactional but read-optimized". Its edges are "double indexed and stored in CSR-based adjacency list indices", and node properties are in "vanilla column files". — [Kuzu technical report](https://cs.uwaterloo.ca/~ssalihog/papers/kuzu-tr.pdf), [Kuzu docs](https://docs.kuzudb.com)
- The GitHub repo `kuzudb/kuzu` is archived (archived=true, last push 2025-10-10) [measured via GitHub API]. Its design ideas remain valid, but it is not a dependency candidate.
- Git commit-graph speedups: [Microsoft DevBlogs](https://devblogs.microsoft.com/devops/?p=44805), [Stolee 2019](https://lore.kernel.org/r/20191021204253.10196-1-dstolee@microsoft.com), [Bloom filters PR #497](https://github.com/gitgitgadget/git/pull/497).
- Lance 14.0.2 (2026-10-29) and arrow-ipc 60.1.0 (2026-10-28) are actively maintained columnar options. — [crates.io lance](https://crates.io/crates/lance), [crates.io arrow-ipc](https://crates.io/crates/arrow-ipc)
- On a fixed workload, the redb benchmark shows LMDB at 2.61 GiB uncompacted and redb at 4.00 GiB, against RocksDB at 893 MiB. B+tree KV stores carry large space overhead. — [redb README](https://github.com/cberner/redb)

### Inferences (recommended design, all [inference])
1. **Graph store: custom segment family. No KV engine for the graph.**
   - Layout: `graph/<repo>/seg-<hash>.seg` plus `graph/<repo>/MANIFEST`.
   - Segment format as in Q3: chunked, LE, 64-byte aligned, XXH3 or SHA trailer, dense u32 positions.
   - Merge policy: git's size-multiple-2 merge plus a max-commits cap.
   - Ingest writes one new top layer per wave, about 16k–64k commits. A background merge runs off the writer thread.
   - Readers load the manifest, then read (or optionally map) the segments. The process-global cache becomes a `Vec<Arc<Segment>>`, so it is cheap to keep warm across waves.
   - This removes the O(N²/batch) rewrite and the overlay-compaction special case, since the overlay is just the top layer.
2. **Commit identity:** dense u32 global positions are the only key on hot paths. SHA→position lookup uses each layer's fanout plus binary search. String keys `"{repo_id:020}:{sha}"` disappear from hot paths.
3. **Per-commit metadata:** columnar chunks in the same segments:
   - commit time, author id (interned), subject offset into a string heap,
   - plus generation numbers for fast topo and ancestry queries,
   - plus optional changed-path Bloom filters written when the async diff pass completes, as a later layer or a sidecar.
4. **Mutable state** (retry queue, ingest checkpoints such as last indexed tip, repo registry, schema version): one redb database per app data dir, opened once per process. SQLite WAL is the fallback if C deps are acceptable.
   - The checkpoint record must reference the manifest generation, so crash recovery is: read the manifest, then resume from the checkpoint. Orphan `.tmp` files and unreferenced segments get deleted at startup.
5. **Durability** uses only std:
   - `File::sync_all` before every rename (Apple F_FULLFSYNC, Windows FlushFileBuffers, Linux fsync),
   - `std::fs::rename` for the manifest (POSIX semantics on Windows),
   - directory fsync on Unix.
   - Never replace or truncate a file that may be mapped.
6. **Deprecations:** drop the `lmdb` 0.8 crate (unmaintained since 2018; bundles LMDB 0.9.21 from 2017). If LMDB must stay during migration, switch to `heed` 0.23.x (active, LMDB 1.0-dev with incremental Windows growth).
7. **Validation before adopting:**
   - an in-repo `quality:perf` comparison of the current LMDB path vs segment append at 1M and 10M commits,
   - Windows CI for rename-while-open and delete-while-mapped behavior,
   - a crash-injection test (kill during publish) to prove recovery always lands on the old or new manifest.
   - Per `AGENTS.md`, this boundary change needs an ADR in `docs/adr`.

### Gaps
- I found no published benchmark of an embedded KV store vs a custom CSR/columnar file for git-history ingest specifically. The 30x write-amplification estimate and the "per-key overhead dominates" claim are arithmetic and inference, and need in-repo measurement.
- I fetched no academic LSM vs B+tree write-amplification paper this session (for example WiscKey, or the RocksDB leveled-compaction wiki). Engine-level WA claims above rest on the redb and fjall benchmarks only.
- I did not verify that gix 0.80 can emit commit-graph positions or generation numbers directly for reuse. If it can, the segment writer could reuse git's own `.graph` layers instead of re-deriving them.
