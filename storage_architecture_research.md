# Storage Architecture Pivot — Deep Research

## The Problem

Current storage is a bloated three-tier mess:

| Engine | File | Size | Role |
|---|---|---|---|
| SQLite | [db.rs](file:///d:/dev/finalProject/apps/desktop/src-tauri/src/adapters/db.rs) | **125KB** | Everything: graph edges, commits, refs, file changes, queues, migrations |
| RocksDB | [ingest_store.rs](file:///d:/dev/finalProject/apps/desktop/src-tauri/src/application/ingest_store.rs) | **79KB** | Optional LSM shadow writes |
| In-memory cache | [query_graph_cache.rs](file:///d:/dev/finalProject/apps/desktop/src-tauri/src/application/query_graph_cache.rs) | **32KB** | `Vec<Vec<NodeId>>` adjacency for fast graph queries |
| **Total** | | **236KB** | Three engines with three invalidation models |

Goal: **one storage engine**, lowest memory, highest performance, maximum low-level gain.

---

## Candidate Architectures Evaluated

### 1. Custom mmap'd CSR + Delta Layer + Arrow *(User's proposal)*

**Graph (CSR + mmap):**
- Edges stored as contiguous `u32[]` array, offsets as `u32[]` per node
- mmap'd via `memmap2` crate — zero hydration cost, OS page cache handles memory
- **Memory: ~12MB for 1.5M nodes** (4B × 1.5M offsets + 4B × 1.6M edges)
- **Traversal: 3x faster** than adjacency lists (sequential cache-line reads)

**Writes (Delta Layer):**
- New commits go into an in-memory delta buffer
- Delta is a small adjacency list of uncommitted edges
- Queries merge CSR + delta at read time (like LSM read path)
- Periodically compact: rebuild CSR from CSR + delta → new CSR file
- Atomic swap: write new file → rename over old one

**Metadata (Arrow IPC):**
- Commit metadata (author, message, timestamp) in immutable Arrow columnar files
- Column-selective reads — only load what you need
- Arrow IPC format is mmap-friendly

**Write-Ahead (WAL):**
- Append-only binary log for crash recovery of the delta layer
- On startup: replay WAL into delta, then merge into CSR if needed

| Aspect | Rating | Notes |
|---|---|---|
| **Read perf** | ⭐⭐⭐⭐⭐ | mmap + contiguous arrays = fastest possible |
| **Write perf** | ⭐⭐⭐ | Delta buffer is fast, compaction is O(N) |
| **Memory** | ⭐⭐⭐⭐⭐ | ~12MB mapped (page cache, not RSS) |
| **Crash safety** | ⭐⭐ | Must build from scratch (WAL + atomic rename) |
| **Implementation cost** | ⭐ | **Massive** — custom file formats, WAL, compaction, all from scratch |
| **Thesis impressiveness** | ⭐⭐⭐⭐⭐ | Extremely impressive if pulled off |

> [!WARNING]
> This is building a custom database. The queue system (`enrichment_queue`, [extraction_queue](file:///d:/dev/finalProject/apps/desktop/src-tauri/src/application/commands.rs#66-76) with retry/backoff) would need its own WAL-backed store. Estimated effort: **4-6 weeks** for a solid implementation.

---

### 2. LMDB via `heed` *(Single engine replacement)*

LMDB is a **memory-mapped B+ tree** key-value store. `heed` is the Rust wrapper. This is what **Meilisearch** uses in production.

**How it replaces all three engines:**

```
Current SQLite tables → LMDB named databases (B-trees)
Current RocksDB LSM  → eliminated (LMDB handles writes natively)
Current in-memory cache → eliminated (LMDB IS mmap'd — reads go direct to page cache)
```

**Graph storage in LMDB:**
- Key: [(repo_id: u32, node_id: u32)](file:///d:/dev/finalProject/apps/desktop/src-tauri/src/domain/models.rs#114-117) → Value: `[neighbor_id_1, neighbor_id_2, ...]` as packed `u32[]`
- Prefix scan on [(repo_id, *)](file:///d:/dev/finalProject/apps/desktop/src-tauri/src/domain/models.rs#114-117) gives all nodes — sequential, cache-friendly
- Point lookup for neighbors: **sub-microsecond**
- Zero-copy reads — `heed` returns references into mmap'd memory

**Commit metadata in LMDB:**
- Separate named database: [commits](file:///d:/dev/finalProject/apps/desktop/src-tauri/src/adapters/git.rs#203-256)
- Key: [(repo_id, sha_bytes)](file:///d:/dev/finalProject/apps/desktop/src-tauri/src/domain/models.rs#114-117) → Value: serialized commit struct
- Or split into columnar databases: `commit_authors`, `commit_times`, etc.

**Queues in LMDB:**
- Key: [(repo_id, enqueued_epoch, sha)](file:///d:/dev/finalProject/apps/desktop/src-tauri/src/domain/models.rs#114-117) → Value: queue entry
- Range scan = ordered queue retrieval
- ACID transactions = safe retry/backoff

| Aspect | Rating | Notes |
|---|---|---|
| **Read perf** | ⭐⭐⭐⭐ | mmap + B+ tree, sub-μs point lookups |
| **Write perf** | ⭐⭐⭐⭐ | Single-writer, sequential writes, no compaction needed |
| **Memory** | ⭐⭐⭐⭐⭐ | mmap — OS page cache, not heap. Near-zero RSS |
| **Crash safety** | ⭐⭐⭐⭐⭐ | ACID transactions built-in, copy-on-write B+ tree |
| **Implementation cost** | ⭐⭐⭐⭐ | **Moderate** — replace SQL with KV API, keep same logic |
| **Thesis impressiveness** | ⭐⭐⭐⭐ | "mmap'd zero-copy B+ tree storage" sounds great |

> [!TIP]
> **Key advantage**: LMDB's mmap gives you the same memory benefit as custom CSR, BUT with built-in ACID, crash recovery, and concurrent readers — for free. No WAL to build, no compaction to implement, no file format to design.

**Crate**: [heed](https://crates.io/crates/heed) (LMDB wrapper) or [heed2](https://crates.io/crates/heed2)  
**Dep**: C library `lmdb-sys` (bundled, like your current `rusqlite` bundles SQLite)

---

### 3. `redb` *(Pure Rust alternative)*

Pure Rust embedded KV store inspired by LMDB. No C dependencies.

| Aspect | Rating | Notes |
|---|---|---|
| **Read perf** | ⭐⭐⭐⭐ | B-tree, slightly slower than LMDB for bulk reads |
| **Write perf** | ⭐⭐⭐⭐ | Faster than LMDB for individual writes in benchmarks |
| **Memory** | ⭐⭐⭐⭐ | Not mmap-based, but efficient |
| **Crash safety** | ⭐⭐⭐⭐⭐ | ACID, MVCC, crash-safe |
| **Implementation cost** | ⭐⭐⭐⭐ | Similar to LMDB approach |
| **Thesis impressiveness** | ⭐⭐⭐ | "Pure Rust" is nice but less exotic |

> [!NOTE]
> `redb` doesn't use mmap — so you don't get the "OS page cache instead of RSS" benefit. It manages its own buffer. Still very good, but **not** the zero-copy mmap story you want.

---

### 4. Sanakirja *(Pijul's engine — built for VCS)*

Sanakirja is a Rust mmap'd B-tree built specifically for **Pijul** (a version control system). It benchmarks **20-50% faster than LMDB** for graph storage.

| Aspect | Rating | Notes |
|---|---|---|
| **Read perf** | ⭐⭐⭐⭐⭐ | Beats LMDB by 20-50% for graph workloads (Pijul benchmarks) |
| **Write perf** | ⭐⭐⭐⭐ | Copy-on-write, efficient database forks |
| **Memory** | ⭐⭐⭐⭐⭐ | mmap-based, same page cache benefits as LMDB |
| **Crash safety** | ⭐⭐⭐⭐ | Transactional, copy-on-write |
| **Implementation cost** | ⭐⭐⭐ | Less documentation than LMDB, smaller community |
| **Thesis impressiveness** | ⭐⭐⭐⭐⭐ | "Same engine used by Pijul VCS" — perfect for a Git tool thesis |

> [!IMPORTANT]
> Sanakirja was **purpose-built for storing version control graphs**. It's literally the closest thing to what you need. The downside: smaller community, less documentation, rougher edges.

---

### 5. Hybrid: LMDB for writes + mmap'd CSR for graph reads

Best of both worlds:

- **LMDB** handles: commits, refs, file changes, queues, checkpoints (structured data with ACID)
- **CSR file** handles: graph edges only (rebuilt from LMDB edge data after each index)
- **No in-memory cache needed** — CSR file IS the cache, mmap'd
- **No RocksDB needed** — LMDB replaces it

| Aspect | Rating | Notes |
|---|---|---|
| **Read perf** | ⭐⭐⭐⭐⭐ | CSR for graph, LMDB for metadata — optimal for each |
| **Write perf** | ⭐⭐⭐⭐ | LMDB handles all writes |
| **Memory** | ⭐⭐⭐⭐⭐ | Both mmap'd — near-zero RSS |
| **Crash safety** | ⭐⭐⭐⭐⭐ | LMDB provides ACID; CSR is derived (rebuilt if stale) |
| **Implementation cost** | ⭐⭐⭐ | Two formats but clear separation |
| **Thesis impressiveness** | ⭐⭐⭐⭐⭐ | "mmap'd CSR graph + zero-copy KV store" = peak engineering |

---

## My Recommendation

### If you want maximum performance + thesis wow factor:

**→ Option 5: LMDB (heed) + mmap'd CSR**

- LMDB replaces both SQLite AND RocksDB AND in-memory cache for everything except graph traversal  
- CSR file replaces [query_graph_cache.rs](file:///d:/dev/finalProject/apps/desktop/src-tauri/src/application/query_graph_cache.rs) for graph traversal  
- Total RSS for 1.5M commits: **~15-20MB** (12MB CSR + LMDB page cache)
- Eliminates ~200KB of storage code  
- ACID for free, crash-safe for free, mmap for free

### If you want minimum implementation risk:

**→ Option 2: LMDB (heed) only**

- ONE engine replaces all three  
- Graph stored as packed integer arrays in LMDB keys  
- Still mmap'd, still zero-copy reads, still near-zero RSS  
- Fastest to implement (~2-3 weeks)  
- Slightly slower graph traversal than CSR (B-tree lookup vs contiguous scan)

### If you want the full custom approach (your original proposal):

**→ Option 1: CSR + Delta + Arrow**

- Maximum performance, maximum thesis points  
- Maximum implementation risk and time  
- You're building a custom database engine  
- Estimated: 4-6 weeks minimum

---

## Code Impact Estimate

| Current File | Fate | Replacement |
|---|---|---|
| [db.rs](file:///d:/dev/finalProject/apps/desktop/src-tauri/src/adapters/db.rs) (125KB) | **Rewrite** | `storage.rs` — LMDB or custom format APIs |
| [ingest_store.rs](file:///d:/dev/finalProject/apps/desktop/src-tauri/src/application/ingest_store.rs) (79KB) | **Delete** | Not needed — LMDB or delta layer handles writes |
| [query_graph_cache.rs](file:///d:/dev/finalProject/apps/desktop/src-tauri/src/application/query_graph_cache.rs) (32KB) | **Delete or simplify** | mmap'd CSR or LMDB graph database |
| [graph_queries.rs](file:///d:/dev/finalProject/apps/desktop/src-tauri/src/application/graph_queries.rs) (33KB) | **Refactor** | CSR-native traversal or LMDB-native queries |
| [indexer.rs](file:///d:/dev/finalProject/apps/desktop/src-tauri/src/application/indexer.rs) (45KB) | **Refactor write paths** | New storage API calls |
| [indexer_pipeline.rs](file:///d:/dev/finalProject/apps/desktop/src-tauri/src/application/indexer_pipeline.rs) (27KB) | **Minor changes** | Same pipeline, different persistence target |
| 6 SQL migrations | **Delete** | No SQL |
| [perf-report.rs](file:///d:/dev/finalProject/apps/desktop/src-tauri/src/bin/perf-report.rs) (91KB) | **Update metrics** | New storage telemetry |

**Net code reduction estimate: ~150-200KB**

---

## Rust Crate Dependencies

| Approach | Add | Remove |
|---|---|---|
| LMDB only | `heed` | `rusqlite`, `rocksdb` |
| LMDB + CSR | `heed`, `memmap2` | `rusqlite`, `rocksdb` |
| Full custom | `memmap2`, `arrow-ipc` | `rusqlite`, `rocksdb` |
