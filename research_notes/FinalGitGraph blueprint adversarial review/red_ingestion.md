# Red-team review: git ingestion, derived data and throughput claims in the FinalGitGraph blueprint (as of 2026-11-01)

Targets: `research_notes/FinalGitGraph backend architecture blueprint/git_ingestion_and_derived_data.md` (the "note"), and these blueprint sections in `reports/FinalGitGraph backend architecture blueprint.md`: "Derived data and incrementality", "Throughput: topology clears 100k commits/s; tree diffs set the ceiling", and the ledger rows for gix, the commit-graph fast path, the pack tape walker and the Bloom reader. Current code: `apps/desktop/src-tauri/src/adapters/git.rs` (4,948 lines) with `gix = "0.80.0"` and `gix-pack = "0.67.0"` (`apps/desktop/src-tauri/Cargo.toml:13-14`).

Evidence labels:
- **[Primary]**: gitoxide source, commits, changelogs, crates.io, docs.rs; git source docs and release notes; Apple DTS.
- **[Code]**: a line range in this repo, read this session.
- **[Measured-local]**: run this session on an Apple M1 Pro (10 cores: 8 performance, 2 efficiency). Repo: a bare, blobless (`--filter=blob:none`), shallow (`--shallow-since=2025-09-01`) clone of `torvalds/linux` master, called the "linux slice". It has 115,295 commits, 9,181 merges, 549 shallow-boundary commits and 97,345 files at HEAD (HEAD `cbbddb6b08cc`, 2026-10-31), in a 182 MB pack. Tools: Apple Git 2.39.5 (`/usr/bin/git`) and Homebrew Git 2.49.0. Each timing is one wall-clock run after one warm-up, with output sent to `/dev/null` and `-c log.mailmap=false -c log.showRoot=false GIT_NO_LAZY_FETCH=1` unless stated. Shallow clones disable git's commit-graph, so none of these runs used one. The clones were deleted before finishing. Treat the numbers as order-of-magnitude.
- **[Inference]**: my reasoning from the cited evidence.
- **[Background]**: knowledge not re-verified this session.

---

## Q0. Ranked attack list (summary)

### Takeaway
The weakest claim is that derived data keyed by `(commit, fact_version)` is "never invalidated". Replace refs, grafts and shallow/unshallow transitions change a commit's effective parents and tree without changing its OID. Unshallowing can also give an already-numbered commit new parents, which breaks the parent-first dense-ID invariant. The second weakest is the throughput extrapolation. On linux-shaped history, merges produce 87% of first-parent change records and run about 13x slower per commit than non-merges. A single core reaches about 10k commits/s with merges included, so the "6+ cores" estimate is about 2x too optimistic. Several gix facts are stale or mis-stated. The pinned gix 0.80 had replace-ref handling with inverted polarity, so replace refs were off by default. gix 0.90 already implements the commit-graph guards the blueprint plans to add. The reftable fallback fails on stock macOS, because `/usr/bin/git` predates reftable and is blocked inside the App Sandbox.

### Cited Findings
Ranked attacks follow. Details and sources are in Q1–Q7, and claims that survived testing are in Q8.

| # | Severity | Claim attacked (location) | Attack type | One-line evidence |
|---|---|---|---|---|
| 1 | Critical | "any fact about a commit's history can be keyed by `(commit, fact_version)` and never invalidated" (blueprint, Derived data) | Correctness hole; contradiction between notes | `git replace --graft B` left the OID unchanged but changed `diff-tree B` from `A f2` to `A f1, A f2` [Measured-local]. git docs say grafts/replace "change the shape of the commit history" and unshallowing invalidates generations ([git commit-graph](https://git-scm.com/docs/commit-graph)). The note itself proposed `(commit_oid, parents_fingerprint)`. See Q4. |
| 2 | Major | "need about 6 or more cores at git-like efficiency … to reach 100k commits/s" of first-parent change sets (blueprint, Throughput; note Q2) | Unsound extrapolation from one small repo | Linux slice: non-merges 19.9k diffs/s, merges 1.5k/s. All commits together run at about 10.2k commits/s on one core. Merges hold 1,489,512 of 1,715,595 change records (86.8%) [Measured-local]. See Q3. |
| 3 | Major | first-parent `fc@v1` is the canonical change set; "store parent-2+ diffs lazily if ever needed" (blueprint; note Q4) | Missed product semantics | git's default `log -- path` hides merges that are TREESAME to any parent ([git-log](https://git-scm.com/docs/git-log)). First-parent change sets cannot reproduce that. All-parent merge diffs are 14.6x the first-parent volume (21.7M vs 1.49M lines) [Measured-local]. See Q5. |
| 4 | Major | "The custom Bloom/murmur3 reader is deleted. Exact change layers answer `log -- path` with no false positives" (blueprint) | Performance regression risk; missed alternative | A path query becomes a scan of about 15 records per commit unless an inverted index exists, and it cannot run before derivation finishes. gitoxide lists commit-graph Bloom filters as needed for gix-blame to compete with Git ([crate-status](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)). See Q2. |
| 5 | Major | reftable "handled with one `git for-each-ref` per refresh" (blueprint; note Q5) | OS-portability hole | Apple Git 2.39.5 on a reftable repo prints `fatal: unknown repository extension found: refstorage` [Measured-local]. Inside the App Sandbox, `xcrun` refuses to run, with "no way to bypass this" ([Apple DTS](https://developer.apple.com/forums/thread/655664)). See Q7. |
| 6 | Major | "Reuse from gix: replace-ref handling"; "upgrade 0.80 → 0.9x" listed only as gains (note Q1; ledger) | Factual error / hidden semantic change | Up to and including 0.89, gix disabled replacements by default because of an inverted check. 0.90 fixed it and now skips the commit-graph while replacements are active ([commit 1b6d8da](https://github.com/GitoxideLabs/gitoxide/commit/1b6d8da7a4)). See Q1/Q6. |
| 7 | Major | line counts via `line_counts()` "using imara-diff's Histogram"; rename/line caches keyed by blob pairs; author IDs sealed in topo layers (blueprint; note Q4) | Correctness hole (purity) | gix 0.90: "The diffing algorithm is determined by the `diff.algorithm` configuration, or individual diff drivers" ([docs.rs](https://docs.rs/gix/latest/gix/object/blob/diff/struct.Platform.html)). gix also honors textconv and external diff commands ([crate-status](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)). In the linux slice, 246 commits touched `.mailmap` in about 14 months [Measured-local]. See Q4. |
| 8 | Major/minor | Commit-graph used "only if: no shallow/replace/grafts, hash kind matches" (blueprint) | Incomplete guard list; missed alternative | The list omits `core.commitGraph=false` and corrupt graphs. gix 0.90's `rev_walk` already disables the graph for replacements and shallow and honors `core.commitGraph` ([walk.rs](https://github.com/GitoxideLabs/gitoxide/blob/main/gix/src/revision/walk.rs)). `commit_graph_if_enabled()` does not check shallow. See Q6. |
| 9 | Minor/major | "the upgrade brings parallel pack decoding" as a throughput lever (blueprint, Throughput and ledger) | Mis-cited source | The 0.87 entry "Make sure `gix-pack` uses the parallel feature" belongs to PR #2852, "resolve delta trees lazily in a lock-free pool". That is index-pack-style whole-pack resolution, not random object lookups for per-commit tree diffs ([PR #2852](https://github.com/GitoxideLabs/gitoxide/pull/2852)). See Q1/Q3. |
| 10 | Minor | "There is no topological-order variant" in gix (note Q1) | Outdated within days | gix main gained `Sorting::TopoOrder`/`DateOrder` on 2026-10-27 (not in 0.90.0). Hidden-ancestry pruning now uses Git's date-based lookahead, which is not exact under clock skew ([3a05954](https://github.com/GitoxideLabs/gitoxide/commit/3a059543f3), [56a9867](https://github.com/GitoxideLabs/gitoxide/commit/56a9867a36)). See Q1. |
| 11 | Minor | Bloom removal justified because "v1 filters 'have a bug that appears when char is signed'" (blueprint; ledger "v1 is not portable") | Right decision, wrong rationale | The current reader emulates signed-char v1 and checks every Bloom negative exactly, so the v1 bug cannot produce a wrong answer here ([Code] git.rs:2840-2877, 3418-3460). Git's own sources describe the bug from opposite sides (format doc: "char is signed"; 2.46 notes: "broken on systems whose char is unsigned"). See Q2. |
| 12 | Minor | Quoted gix text "uses the first-found candidate rather than Git's up-to-four candidates" (blueprint) | Paraphrase presented as a quote | The actual text: "git keeps up to four candidates whereas we use the first-found candidate that matches the similarity percentage … we also don't consider the filename at all" ([crate-status](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)). See Q1. |
| 13 | Minor | gix upgrade presented as cheap and beneficial (ledger) | Understated cost and risk | 10 minor releases from 0.80.0 (2026-02-22) to 0.90.0 (2026-10-22), with breaking API changes such as path-based `Graph` constructors that now require `object_hash` ([8722752](https://github.com/GitoxideLabs/gitoxide/commit/8722752744)). Behavior changed too (replace polarity). See Q1. |
| 14 | Minor | Oracle commands "cheap enough for CI" (note Q6); partial-clone handling via "unavailable" status (blueprint) | Oracle hazard | In a blobless clone, `git log --format=%H` tries to read the `HEAD:.mailmap` blob. With lazy fetch disabled, Apple Git aborts with `fatal: could not fetch … from promisor remote` [Measured-local]. Oracles must pin `log.mailmap=false` and `GIT_NO_LAZY_FETCH=1`. See Q4/Q7. |

### Inferences
- Attacks 1, 3 and 7 share one root cause. The blueprint treats "a pure function of the commit OID" as a free property. In fact it holds only for the raw object graph under a pinned diff configuration with no repo-local inputs: no replace refs, no shallow file, no mailmap, no attributes, no user `diff.*` config. The cheapest fix is to define ingestion explicitly over raw objects. Open gix with replacements ignored, ignore the user's diff/attribute config, and store raw identities. Then record digests of every non-raw input (replace set, shallow set, mailmap blob OID, rename/diff policy, gix-diff version for similarity renames) in the manifest, and treat a digest change as targeted re-derivation or a blue/green rebuild.
- Attacks 2 and 3 suggest splitting `fc@v1` into two derived types: non-merge change sets, which are cheap and well distributed, and merge change sets, which are expensive, heavy-tailed and dominate storage. Set separate throughput targets for each.

### Gaps
- No gix (as opposed to git) tree-diff measurement exists, published or local; cargo is unavailable on this machine (project memory note).
- chromium and full linux.git were not measured. The linux slice is a recent 14-month window with full trees, so merge-window behavior is represented but not decades of history.

---

## Q1. gix facts as of November 2026: what was mis-stated

### Takeaway
The headline facts hold. gix 0.90.0 is current. gix-commitgraph reads no Bloom (BIDX/BDAT) and no GDA2 chunks and cannot write graphs. Reftable, promisor and bitmap support are absent, and `line_counts()` exists. The note is wrong on replace refs: the pinned 0.80 effectively ignored them by default. It also mis-reads the "parallel gix-pack" entry and misses three changes from October 2026: built-in commit-graph guards, corruption checks with a breaking constructor change, and topological sorting landing on main.

### Cited Findings
- **Current versions.** crates.io lists gix 0.90.0 (2026-10-22), 0.89.0 (2026-10-08), 0.88.0 (2026-09-25), 0.87.0 (2026-08-22), 0.86.0 (2026-07-23), 0.85.0 (2026-06-22), 0.81.0 (2026-03-22) and 0.80.0 (2026-02-22). gix-commitgraph's newest is 0.42.0 (2026-10-22). — [crates.io gix](https://crates.io/crates/gix/versions); [crates.io gix-commitgraph](https://crates.io/crates/gix-commitgraph)
- **Bloom and GDA2.** gix-commitgraph's file reader defines only the chunk IDs `BASE`, `CDAT`, `EDGE`, `OIDF` and `OIDL`. `Commit::generation()` documents topological levels: "Commits without parents have generation number 1 … max of their parents' generation numbers + 1." — [gix-commitgraph src/file/mod.rs](https://github.com/GitoxideLabs/gitoxide/blob/main/gix-commitgraph/src/file/mod.rs); [src/file/commit.rs](https://github.com/GitoxideLabs/gitoxide/blob/main/gix-commitgraph/src/file/commit.rs)
- crate-status still lists "[ ] Corrected generation dates", "[ ] Bloom filter index", "[ ] Bloom filter data" and "[ ] create and update graphs and graph files". — [crate-status](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)
- **A misleading changelog entry.** The gix-commitgraph CHANGELOG for 0.17.0 (2023-06-22) says "support for 'generation v2' format for overlow correction". The underlying commit changes only tests, fixtures and Cargo files, with no `src/` change. So gix does not read GDA2, and the note's claim stands. — [gix-commitgraph CHANGELOG](https://github.com/GitoxideLabs/gitoxide/blob/main/gix-commitgraph/CHANGELOG.md); [commit 101dec0](https://github.com/GitoxideLabs/gitoxide/commit/101dec0adb2def4016f01a102de19a47da6752cc)
- **Replace refs had inverted polarity until 0.90.** Commit 1b6d8da (2026-10-10), "fix(gix): honor replacement refs and bypass stale commit graphs", says: "Enable standard replacement refs by default and honor `core.useReplaceRefs` with the same polarity as Git." Its patch changes `let is_disabled = is_replace_refs_enabled(...).unwrap_or(true); if is_disabled { return Ok(None) }` to `let is_enabled = …; if !is_enabled { return Ok(None) }`. Before the fix, an unset config meant replacements were off. The same commit adds: "Commit graphs contain original parent links, so skip them while object replacements are active, including when a caller forces or supplies a cache." The GitHub compare API shows the commit inside `gix-v0.90.0` (ahead_by 98, behind_by 0). — [commit 1b6d8da](https://github.com/GitoxideLabs/gitoxide/commit/1b6d8da7a4); [compare](https://github.com/GitoxideLabs/gitoxide/compare/1b6d8da7a4...gix-v0.90.0)
- **Commit-graph corruption checks and an API break, in 0.90.** Commit 8722752 (2026-10-18), "fix(gix-commitgraph)!: require hash kind and allocation limit for graph loading":
  - "path-based `Graph` constructors now require `object_hash`"; "`TryFrom<&Path> for Graph` is removed".
  - "Corruption checks now protect ordinary traversal and graph loading, without requiring a prior call to `Graph::verify_integrity()`", including "Validate parent positions before yielding them".
  - "Fall back to a split chain only when the monolithic graph is missing, preserving errors from existing corrupt graphs."
  - The compare API shows it in `gix-v0.90.0`. — [commit 8722752](https://github.com/GitoxideLabs/gitoxide/commit/8722752744)
- **Topological walks are on main but not released.** Commit 3a05954 (2026-10-27), "feat(gix)!: expose topological walks through `Repository`", adds `Sorting::TopoOrder` and `Sorting::DateOrder` to `Repository::rev_walk`. It notes that callers previously "had to use the plumbing `Topo` builder … bypassing repository handling of replacements and shallow boundaries". The compare API shows it is not in `gix-v0.90.0` (behind_by 62). — [commit 3a05954](https://github.com/GitoxideLabs/gitoxide/commit/3a059543f3)
- **Hidden-traversal fix (main, 2026-10-27).** "Without commit-graph generation numbers, `@~10..@` in Linux could spend minutes proving hidden ancestry". The fix uses "Git's five-commit date-based lookahead … Byron explicitly chose Git's fast pruning over exact results under arbitrary date skew." — [commit 56a9867](https://github.com/GitoxideLabs/gitoxide/commit/56a9867a36)
- **"Parallel gix-pack" means delta-tree resolution.** The 0.87.0 entry "Make sure `gix-pack` uses the parallel feature" (commit 79e3478) adds `"gix-pack/parallel"` to gix's `parallel` feature. It is part of PR #2852, "fix: resolve delta trees lazily in a lock-free pool", whose benchmarks compare against `git index-pack` thread selection. — [commit 79e3478](https://github.com/GitoxideLabs/gitoxide/commit/79e34780825b79ff5a2e7e6be3e30cd89d3b0469); [PR #2852](https://github.com/GitoxideLabs/gitoxide/pull/2852)
- **Rename-tracking wording.** "**deviation** - git keeps up to four candidates whereas we use the first-found candidate that matches the similarity percentage. This can lead to different sources being found. As such, we also don't consider the filename at all." Other items: "[ ] heuristics to find best candidate"; "[ ] find by basename … Not having it can lead to issues when files with the same or similar content are part of a move as files can be lost that way"; directory tracking "by identity" only. — [crate-status](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)
- **Diff configuration inputs.** Under tree-with-tree diffs, crate-status lists "respect case-sensitivity of host filesystem", "a way to access various diff related settings", and "respect `diff.*.textconv`, `diff.*.cachetextconv` and external diff viewers with `diff.*.command`, along with support for reading `diff` gitattributes". — [crate-status](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)
- **`line_counts()` (docs.rs, gix 0.90.0).** "Count the amount of removed and inserted lines efficiently. Note that nothing will happen if one of the inputs is binary, and `None` will be returned." For `lines()`: "The diffing algorithm is determined by the `diff.algorithm` configuration, or individual diff drivers." — [docs.rs Platform](https://docs.rs/gix/latest/gix/object/blob/diff/struct.Platform.html)
- **Reftable, promisor, bitmaps.** crate-status lists as not done: "Git 3.0 compatibility (`SHA-256`, `reftable`)", gix-reftable "read and write reftable stacks", every promisor item, and the gix-pack "'bitmap' file". — [crate-status](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)
- **Checkbox reliability.** The gix-traverse section still shows "[ ] `commitgraph` support", while gix's `revision/walk.rs` passes a commit-graph into traversal (`.commit_graph(commit_graph)`). The crate-status preamble says it was "Generated by Codex … Reconcile on demand". — [crate-status](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md); [walk.rs](https://github.com/GitoxideLabs/gitoxide/blob/main/gix/src/revision/walk.rs)
- **Agent-written upstream.** Recent gix commits carry "Assisted-by: GPT 6.0 Astra" and "Co-authored-by: GPT 6.1 Sol". The 3a05954 message says "implementation is rubber-stamped, knowing that regression-protection exists for performance". — [commit 1b6d8da](https://github.com/GitoxideLabs/gitoxide/commit/1b6d8da7a4); [commit 3a05954](https://github.com/GitoxideLabs/gitoxide/commit/3a059543f3)
- **[Code]** The current code opens the commit-graph with `gix::commitgraph::Graph::from_info_dir(repo.path().join("objects").join("info"))` and no replacement, shallow or `core.commitGraph` check (git.rs:3000-3019). The Bloom loader uses the same `repo.path()` base (git.rs:3021-3022).

### Inferences
- **Replace refs (attack 6, major).** On gix 0.80, FinalGitGraph sees the raw object graph even when the user's git honors `refs/replace/*`, so it silently disagrees with `git log`. Upgrading to 0.90 flips this: `find_commit(X)` returns the replacement object, while the current hand-rolled commit-graph and Bloom paths keep returning original parents and trees. Examples are git.rs:3000-3019 and the empty-filter skip at git.rs:3569-3580. The upgrade therefore creates a mixed view unless those paths move to `commit_graph_if_enabled()` or replacements are pinned off. *Revision:* state the chosen semantics (raw objects recommended) and set them explicitly at open. Add `git replace` and `GIT_NO_REPLACE_OBJECTS` fixtures to the oracle, and list the polarity change in the upgrade ADR.
- **Upgrade cost (attack 13, minor).** The 0.90 commit-graph constructor change means `Graph::from_info_dir(path)` must change at upgrade [Inference from the 8722752 text; not compiled]. The upgrade is a semantic migration, not a version bump. Gate it on the differential oracle, and include the gix-diff version in any derived type that stores similarity renames.
- **Parallel gix-pack (attack 9).** Delta-tree parallelism helps a "decode the whole pack once" cold ingest, which is the tape walker's niche. It does not help the per-commit tree-diff ceiling, which is random object access through per-thread caches. The ledger's upgrade rationale overstates the throughput gain.
- **Topological sort (attack 10).** With `Sorting::TopoOrder` on main, the note's "no topological variant" will be stale at the next release. More important: if incremental frontier walks use gix's hide/exclude machinery, they inherit date-based pruning, which is "not exact under arbitrary date skew". The blueprint's own stop condition (stop at already-indexed OIDs) is exact only if the indexed set is closed under ancestry. Keep that condition and do not route frontier discovery through gix `hide()`.
- **Quote fidelity (attack 12).** The substance of the rename deviation is right. The blueprint should quote the actual text, which also says gix ignores filenames and lacks basename matching. That matters for move-heavy commits where "files can be lost".

### Gaps
- No gix 0.80 binary was run to show the replace-ref polarity bug end to end. The evidence is the 0.90 fix's patch and commit message.
- Whether "respect case-sensitivity of host filesystem" changes tree-diff or rename output between macOS/Windows (`core.ignoreCase=true`) and Linux was not tested. That could break "same behavior on every OS" for case-only renames, and needs a fixture.

---

## Q2. Is deleting the Bloom reader a performance regression for path queries, and is the v1 rationale right?

### Takeaway
Deleting the *current* Bloom code costs little, because it never skips the expensive case: every Bloom negative still runs an exact check. The blueprint's reasons are weak, though, and its replacement is unspecified. A change-layer scan has no index, is unavailable until derivation completes, and costs O(change records) per query. The v1 portability rationale is accurate in general but cannot cause wrong answers in this code, and git still writes v1 filters by default.

### Cited Findings
- **[Code] The current Bloom logic is inverted for usefulness.** In `evaluate_changed_path_decision_for_oid`, a Bloom "maybe" returns `Touched` immediately with no verification, which accepts false positives. A Bloom "definitely not" falls through to `commit_touches_target_paths_exact`, a two-tree path lookup (git.rs:2840-2877, 3463-3517). Negatives are the only case where a Bloom filter saves work, and they are re-checked exactly. The only real skip is `commit_might_have_first_parent_changes`, which uses the length-one all-zero filter for commits with no changes (git.rs:3569-3580).
- **[Code] v1 emulation.** `bloom_murmur3_seeded_v1` sign-extends each byte (`chunk[i] as i8 as i32`), which reproduces filters written on signed-char platforms. `bloom_murmur3_seeded_v2` uses unsigned bytes (git.rs:3418-3460).
- **The git sources describe the bug from opposite sides.** The format doc: "Version 1 Bloom filters have a bug that appears when char is signed and the repository has path names that have characters >= 0x80; Git supports reading and writing them, but this ability will be removed in a future version of Git." — [gitformat-commit-graph](https://git-scm.com/docs/gitformat-commit-graph). Git 2.46.0 release notes: "The Bloom filter used for path limited history traversal was broken on systems whose 'char' is unsigned; update the implementation and bump the format version to 2." — [RelNotes 2.46.0](https://github.com/git/git/blob/master/Documentation/RelNotes/2.46.0.adoc)
- **git still writes v1 by default.** `commitGraph.changedPathsVersion` "Defaults to -1. If -1, Git will use the version of the changed-path Bloom filters in the repository, defaulting to 1 if there are none." Also: "values greater than 1 may be incompatible with older versions of Git". `commitGraph.changedPaths` defaults to unset, so filters are written only if they already exist. — [git config/commitgraph.adoc (master, 2.56 era)](https://github.com/git/git/blob/master/Documentation/config/commitgraph.adoc)
- **Upstream needs path filters.** gix-blame's "Performance-Improvements" section says "Without the following the performance isn't competitive with Git", and lists "commit-graph with bloom filter, used to quickly check if a commit has a path" and "custom graph walk which won't run down parents that don't have the path in question". — [crate-status](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)
- **Bloom speedups in git.** `git log -- <path>` gained "2x to 5x" on the git and Linux repos. — [gitgitgadget PR #497](https://github.com/gitgitgadget/git/pull/497). The note measured 3.3x on git.git (0.102 s vs 0.341 s). — [note Q2, Measured-local]
- **[Measured-local] Change-record volume (linux slice, first-parent, `--no-renames`).**
  - Non-merges: 226,083 records over 106,114 commits (median 1, p90 4, p99 12, max 8,016).
  - Merges: 1,489,512 records over 9,181 commits (median 6, p90 80, p99 2,329, max 36,879).
  - In total, about 14.9 records per commit. 384 merges and 4 non-merges exceed 512 changes, the point where git stores an all-ones (useless) Bloom filter.

### Inferences
- **Attack 4 (major): time-to-availability.** Exact change layers exist only after `fc@v1` derivation has covered the history. At about 10k commits/s per core with merges (Q3), a 1.4M-commit linux needs roughly 140 core-seconds, and chromium-class repos more. Until then, file history must block or fall back to on-the-fly tree diffs with no skip filter. git answers `log -- path` immediately when filters exist. The blueprint gives no fallback plan.
- **Attack 4: query cost.** Without an inverted index, `log -- path` scans every change record, and a directory query must match prefixes. On the slice's rate of about 15 records per commit, full linux would be on the order of 20M records. That figure assumes the 14-month mix holds over all history, which is unverified. Records that carry two OIDs (about 40+ bytes each) put the layer near 1 GB. Bloom filters cost about 10 bits per entry ([Background] on git's default bits-per-entry) and answer negatives without reading change data.
- **Revision.** Delete the current reader: it is ineffective, which is a stronger reason than v1 portability. Plan a path index over change layers from the start, for example a posting list (roaring bitmap of dense IDs) per interned path ID, directory prefixes included. Add a cold-start rule: while `fc@v1` is incomplete, answer path queries by bounded on-the-fly diffs and mark them partial. Kill criterion: on linux, file-history p95 must stay within git's Bloom-backed `git log -- <path>`.
- **Attack 11 (minor).** "v1 is not portable" is true: signed- and unsigned-char writers disagree for bytes ≥ 0x80. The current reader emulates only signed-char writers, so filters from Linux aarch64 and other unsigned-char hosts would mis-hash non-ASCII paths. Because negatives are re-verified, that mismatch cannot yield a wrong answer in this code, only lost skips. The blueprint should not cite it as a correctness reason. Also, git does not default to v2, so a reader limited to v2 would ignore nearly all real-world filters.

### Gaps
- An exact-scan versus Bloom-backed `log -- path` timing was not measured on linux, because shallow clones disable the commit-graph and a full clone was out of budget.
- The change-layer record size is an estimate. The blueprint does not specify the change-layer schema.

---

## Q3. Throughput evidence: is one Apple Git run on git.git enough, and is "6+ cores" sound?

### Takeaway
For non-merge commits the blueprint's numbers survive. On linux, with 20x larger trees, non-merge diffs run at the same rate as git.git, and 8 processes reach about 106k diffs/s. Git version barely matters (2.39.5 vs 2.49.0). The extrapolation fails on merges. git.git under-represents them, and on linux-shaped history they dominate both time and storage. With merges included, one core does about 10k commits/s, so 100k/s needs at least 10 perfectly scaling cores, not 6.

### Cited Findings
- **[Measured-local] linux slice (115,295 commits, 97,345 files at HEAD; no commit-graph because shallow):**

  | Command | Apple Git 2.39.5 | Homebrew Git 2.49.0 | Throughput (2.39.5) |
  |---|---|---|---|
  | `log --no-merges --raw --no-renames --format=%H` (106,114 commits) | 5.33 s | 5.49 s | ≈19.9k diffs/s |
  | `log --merges --raw --no-renames --diff-merges=first-parent` (9,181 merges) | 5.99 s | 6.04 s | ≈1.5k merges/s |
  | Both, i.e. a full first-parent `fc` for every commit | 11.32 s | 11.53 s | ≈10.2k commits/s |
  | `rev-list --parents HEAD` | 1.02 s | 1.00 s | ≈113k commits/s |
  | `log --format='%H %P %T %an %ae %at %ct %s'` | 1.10 s | 1.04 s | ≈105k commits/s |
  | `diff-tree --stdin -r --no-renames`, non-merges, 1 process | n/a | 5.68 s | ≈18.7k/s |
  | Same, 8 processes with 13,265 commits each | n/a | 1.00 s | ≈106k/s (5.7x) |

- **[Measured-local]** With `log.mailmap` left at its default (true), the 2.49.0 non-merge run took 6.25 s instead of 5.49 s. In a bare repo, git tried to read `HEAD:.mailmap` and printed `error: unable to read mailmap object at HEAD:.mailmap`.
- **[Measured-local]** `diff-tree --stdin -r -m` on the merges ignores `--first-parent` and diffs against every parent. It emitted 21,715,317 change lines, versus 1,489,512 for first-parent only (14.6x). The single-process run took 623 s and the 8-process run 7.3 s. That superlinear gap is unexplained (memory growth in one long-lived process is a guess) and should not be relied on.
- **Comparison with the note's git.git baseline:** non-merges 3.402 s for 64,322 commits (≈18.9k/s). Including merges as first-parent diffs added about 37% (4.673 s). — [note Q2, Measured-local, Apple Git 2.39.5]. On the linux slice, merges add 112% (5.99 s on top of 5.33 s).
- **No published gix tree-diff benchmark was found.** The nearest numbers come from gitoxide's 2021 `hours` example on 1,002,691 linux commits: "Commit discovery: 6.616651791 s (151,541 commits/s)", with traversal not parallelized and no tree diffs. — [gitoxide discussion #78](https://github.com/Byron/gitoxide/discussions/78). The onefetch-on-gitoxide change measured a whole tool (linux 19.6 s → 8.1 s), not tree diffs. — [onefetch PR #635](https://github.com/o2sh/onefetch/pull/635)

### Inferences
- **The note's guess that "linux.git trees are larger and deeper, so the per-commit cost will be higher there" is wrong for non-merges.** Equal-subtree pruning keeps cost proportional to the changed paths: median 1 change per commit on linux, 2.2 on git.git. The blueprint's caution is harmless, but the real risk was elsewhere.
- **Attack 2 (major).** Merges cost about 13x a non-merge per commit (1.5k/s vs 19.9k/s) and carry 86.8% of change records. Their size is heavy-tailed (p99 2,329, max 36,879), so a rayon chunk that draws a merge-window merge stalls its worker. Sustaining 100k commits/s on all commits would need at least 10 cores at perfect scaling, and in practice more, because of the skew and because the M1 Pro's 8-process scaling was already only 5.7x on uniform non-merge work. The figures are git-equivalent: gix could be faster or slower, unmeasured.
- **Revision.**
  1. Report two derived-throughput metrics: non-merge `fc` (the claim holds at about 8 cores) and merge `fc`.
  2. Bound merge work. Either store merge first-parent diffs only when under a cap, as a flagged "large merge" otherwise, or derive them lazily on demand.
  3. Order the work queue by expected cost (merges by parent-tree distance) to avoid stragglers.
  4. Re-measure on full linux and chromium with gix before writing the target into an ADR.
- **Attack 9 (methodology).** Single runs on one repo with one git build cannot carry an architecture target. The version effect turned out small, so the real error is repo shape: git.git has about 25% merges, and their first-parent diffs are small. A benchmark matrix (git.git, linux, a monorepo with wide merges) plus a merge/non-merge split would have caught this.
- **Metadata at "≥100k/s on one core" holds only marginally:** 105–111k/s on linux on an M1 Pro, 107k/s on git.git, with mailmap off. Mailmap resolution, encoding conversion or slower CPUs will drop below 100k/s. Phrase the target as "≥100k commits/s on reference hardware with N cores", not "one core".

### Gaps
- No gix-based measurement, and no line-stat (`--numstat`) measurement on linux (the clone was blobless).
- Full-history linux and chromium merge statistics may differ from the 14-month window.
- The tree-pair cache hit rate, which the blueprint offers as an alternative to more cores, is still unmeasured.

---

## Q4. Is derived data keyed by commit OID truly "never invalidated"?

### Takeaway
No. The blueprint's own source note lists replace refs, grafts and shallow boundaries as exceptions, and the blueprint drops them. Each changes a commit's effective parents or tree without changing its OID. Unshallowing also breaks the parent-first dense ID numbering. Mailmap, diff algorithm, attributes, textconv and gix version are further non-OID inputs to the facts the blueprint plans to store.

### Cited Findings
- **git's own reasoning.** "Commit grafts and replace objects can change the shape of the commit history. The latter can also be enabled/disabled on the fly using `--no-replace-objects`. This leads to difficulty storing both possible interpretations of a commit id … The commit-graph will not be read or written when replace-objects or grafts are present." "Shallow clones create grafts of commits by dropping their parents … If and when those commits are made unshallow, those generation numbers become invalid." — [git technical/commit-graph](https://git-scm.com/docs/commit-graph)
- **gix 0.90's version.** "Commit graphs contain original parent links, so skip them while object replacements are active." — [commit 1b6d8da](https://github.com/GitoxideLabs/gitoxide/commit/1b6d8da7a4). gix's walker: "Shallow boundary commits have no traversal parents, even though [`Self::object()`] retains their stored parents." — [walk.rs](https://github.com/GitoxideLabs/gitoxide/blob/main/gix/src/revision/walk.rs)
- **[Measured-local] Replace demo (Git 2.49.0).** Three commits A ← B ← C were created, then a commit-graph with changed paths was written.
  - Before: `diff-tree C` = `A f3`; `diff-tree B` = `A f2`.
  - After `git replace --graft B` (B becomes a root), `rev-parse main` still gives the same C OID.
  - `diff-tree --root B` now lists `A f1` and `A f2`.
  - `rev-list --parents` shows B with no parent, while `--no-replace-objects rev-list --parents` shows the original parent.
- **The contradiction between notes.** The ingestion note says derived facts are pure "Exceptions: replace refs, grafts and shallow boundaries change effective parents", and proposes keying "as `(commit_oid, parents_fingerprint)`" (note Q3 Lesson 1; Q5 "Effective parents"). The blueprint says facts "can be keyed by `(commit, fact_version)` and never invalidated" (blueprint, Derived data).
- **Line counts depend on config.** gix 0.90 picks the algorithm from "`diff.algorithm` configuration, or individual diff drivers" ([docs.rs](https://docs.rs/gix/latest/gix/object/blob/diff/struct.Platform.html)). gix honors `diff.*.textconv`, `diff.*.command` and `diff` gitattributes ([crate-status](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)). The note itself says "histogram vs myers counts can differ" (note Q6).
- **Rename output changes across gix versions.** gix 0.86.0 merged "fix/1832-rename-tracker-order-independent", so earlier rename output depended on input order. — [gix CHANGELOG](https://github.com/GitoxideLabs/gitoxide/blob/main/gix/CHANGELOG.md)
- **[Measured-local] Mailmap churn.** In the linux slice, `git log HEAD -- .mailmap` lists 246 commits between September 2025 and 2026-10-31. In a bare repo, `git log` reads `HEAD:.mailmap` by default (observed through the error above).

### Inferences
- **Attack 1 (critical): replace refs.** With gix 0.90 defaults, a full `git replace X Y` changes X's tree and parents, and so the first-parent change sets of X *and of every child of X*. Neither `(commit, version)` nor `(commit, parents_fingerprint)` captures that, because the child's parent OID is unchanged while the parent's content differs. `git replace --graft` changes the grafted commit's own change set, as the demo shows. *Revision:* ingest the raw graph (replacements ignored at open). Store a replace-set digest in the manifest. If the product must mirror `git log`, model replacements as an overlay re-derived for affected commits and their children.
- **Attack 1 (critical): shallow and unshallow.** A shallow-boundary commit's first-parent diff, computed as a root, lists every file. That is 97,345 additions per boundary commit on linux, and the slice has 549 such commits. If that result is sealed as `fc@v1` and "never invalidated", `git fetch --unshallow` leaves permanent phantom "added everything" commits in file history. Worse for the identity scheme: after deepening, a boundary commit already holding dense ID *k* gains parents that must have IDs below *k*, but only IDs above the current maximum can be issued. The append-only "parent < child" invariant cannot absorb this without renumbering. *Revision:* give boundary commits `status=boundary`, which never counts as `fc@v1` coverage. Treat any shrink of the shallow set as a blue/green rebuild (the blueprint already has blue/green for full re-index), and test the case with a fixture.
- **Grafts.** `info/grafts` is deprecated. gix does not implement it (crate-status never mentions grafts), so gix and git already disagree on graft repos. Detect the file and refuse, or rebuild.
- **Partial clones.** "unavailable" must likewise not count as coverage, or a later fetch never triggers re-derivation. The blueprint's pending-set rule ("indexed IDs minus IDs covered by `fc@v1`") must exclude `unavailable` and `boundary` from "covered".
- **Attack 7 (major): line stats.** A cache keyed by `(blob_old, blob_new)` is pure only if the algorithm is pinned, textconv and external drivers are off, and attributes are fixed (or taken from the commit's tree rather than the user's worktree or `core.attributesFile`). The blueprint asserts Histogram but calls an API that reads `diff.algorithm`. Git's default is Myers [Background], so line counts can differ by machine and from a default `git log --numstat` oracle. Textconv also means a background indexer could run user-configured external programs. *Revision:* pin `diff.algorithm` and the conversion mode, disable textconv and `diff.*.command`, and put these in the `fact_version`.
- **Attack 7: mailmap.** The topo layer seals "author ids". If those ids are mailmap-resolved, linux's mailmap churn (about every 2 days in this window) invalidates sealed layers. *Revision:* store raw identities in layers, and apply mailmap as a query-time view keyed by the mailmap blob OID.
- **Similarity renames.** Their output depends on the gix version (PR #2687) and on the deviation from git. Keep `fc@v1` to exact renames only, which are deterministic from blob OIDs. Put similarity renames in a separate derived type whose version includes the gix-diff version, threshold and limit.

### Gaps
- Whether gix's tree diff output (renames off) differs between case-insensitive and case-sensitive hosts ("respect case-sensitivity of host filesystem") was not tested.
- How submodule `.gitmodules` changes or `diff.ignoreSubmodules` interact with gitlink records was not examined. The gitlink plan itself ("submodule pointer moved") looks sound.

---

## Q5. First-parent-only change sets: what product features lose information for merges?

### Takeaway
First-parent `fc` matches git's Bloom definition and `--diff-merges=first-parent`. It does not match git's *default* path history, which hides merges that are TREESAME to any parent and follows only that parent. Without parent-2 information the product cannot reproduce `git log -- path`, cannot tell carried changes from conflict resolutions, and double-counts per-path churn. On linux, merges are where most change volume lives.

### Cited Findings
- git-log default history simplification: "Commits are included if they are not TREESAME to any parent … If the commit was a merge, and it was TREESAME to one parent, follow only that parent. (Even if there are several TREESAME parents, follow only one of them.) Otherwise, follow all parents." — [git-log, History Simplification](https://git-scm.com/docs/git-log)
- `--first-parent` "also changes default diff format for merge commits to `first-parent`". — [git-log](https://git-scm.com/docs/git-log)
- **[Measured-local] (linux slice):** first-parent merge diffs total 1,489,512 records against 226,083 for all non-merges. Diffs against all merge parents total 21,715,317 records.
- The note's oracle table uses `--diff-merges=first-parent` for change sets and no default `log -- path` oracle (note Q6).

### Inferences
- **Attack 3 (major).**
  - **File history.** Scanning change layers for path P returns the side-branch commit that changed P *and* every merge whose first-parent diff carries P. That is a merge-window merge on linux nearly every time. git's default shows the side commit and hides the merge when it is TREESAME to its second parent.
  - **Churn.** Per-path churn and per-author aggregates (the DBSP views) count the same change once per merge layer that carries it.
  - **Evil merges.** Conflict resolutions and "evil merges" are invisible, because a first-parent diff mixes them with carried changes. git exposes them through `--cc` and `--remerge-diff`.
- **Revision.** Keep first-parent `fc` as the storage primitive, since full parent-2 diffs cost 14.6x the volume. Add a cheap derived bit set per merge: for each path in the first-parent diff, whether the path is TREESAME to parent 2 (one path lookup in parent 2's tree). That reproduces git's default simplification, and paths changed against *all* parents flag conflict or evil-merge candidates. Make the oracle check default `git log -- <path>` output on sampled paths, not only `--diff-merges=first-parent`.
- `--diff-merges=first-parent` is a reasonable oracle for the stored primitive. It is the wrong oracle for the user-facing feature.

### Gaps
- The cost of the per-merge TREESAME bit was not measured. By reasoning it is one tree lookup per carried path, below a full parent-2 diff, but unverified.

---

## Q6. Are the commit-graph guards complete, and are they needed at all?

### Takeaway
The guard list (shallow, replace, grafts, hash match, commits newer than the file) matches git's documented rules but omits `core.commitGraph=false` and corrupt or partial chains. It also misses that gix 0.90 already implements most of it. The current code (git.rs:3000-3019) has none of these guards, and probably misses the graph entirely in linked worktrees.

### Cited Findings
- **git's rules.** The commit-graph is not read or written with replace objects, grafts or shallow commits ([technical/commit-graph](https://git-scm.com/docs/commit-graph)). On a header hash-version mismatch, "the commit-graph file should be ignored with a warning" ([gitformat-commit-graph](https://git-scm.com/docs/gitformat-commit-graph)). In mixed generation chains, "Git inspects the topmost layer to see if the layer is missing corrected commit dates. In such a case Git only uses topological level" ([technical/commit-graph](https://git-scm.com/docs/commit-graph)).
- **gix 0.90 `rev_walk`.** `commit_graph = if repo.has_object_replacements() || objects.shallow.is_some() { None } else if use_commit_graph.map_or_else(|| repo.config.may_use_commit_graph(), Ok)? { … }`, documented as "Active object replacements and shallow boundaries disable even an explicitly supplied graph." — [walk.rs](https://github.com/GitoxideLabs/gitoxide/blob/main/gix/src/revision/walk.rs)
- **gix 0.90 `commit_graph_if_enabled()`.** It returns a graph when `!self.has_object_replacements() && self.config.may_use_commit_graph()?`. It has no shallow check, and it opens `self.objects.store_ref().path().join("info")`, the object store's directory. — [graph.rs](https://github.com/GitoxideLabs/gitoxide/blob/main/gix/src/repository/graph.rs)
- **gix 0.90 corruption handling.** Parent positions are validated in ordinary traversal, and a split chain is used "only when the monolithic graph is missing, preserving errors from existing corrupt graphs". — [commit 8722752](https://github.com/GitoxideLabs/gitoxide/commit/8722752744)
- **[Code]** `load_commit_graph_cached` caches `Graph::from_info_dir(repo.path().join("objects").join("info")).ok()` per git dir for the process lifetime (git.rs:3000-3019). It has no replacement, shallow, `core.commitGraph` or staleness check. `walk_commits_from_commit_graph` (git.rs:1196) consumes it.

### Inferences
- **Attack 8.**
  - **Missing conditions.** `core.commitGraph=false` is a user's explicit "do not trust this file", usually set after corruption. The list also misses graphs that become stale *while the process runs*: the cache never refreshes after `git commit-graph write` or `gc`, so new layers and rewritten chains are ignored or mismatched until restart. Corrupt files are a further case (pre-0.90 gix did not validate parents during traversal).
  - **What gix already covers.** Using `repo.rev_walk(...)` or `commit_graph_if_enabled()` plus an explicit `repo.shallow_commits()` check gives replacements, `core.commitGraph` and corruption handling for free.
  - **What remains.** Grafts, which gix does not implement at all, and the shallow check for direct `commit_graph_if_enabled()` users.
- **Linked worktrees.** For a linked worktree, `repo.path()` is the per-worktree git dir (`.git/worktrees/<name>`), not the common dir that holds `objects/` ([Background] on gix `Repository::path` and `common_dir`). git.rs:3011 and 3022 would then find no commit-graph or Bloom data, which is a silent performance cliff. gix's own accessor uses the object store path. This needs a test.
- "Commits newer than the file" is a coverage rule, not a validity rule. A per-lookup fallback to the ODB, which the note already states, handles it.
- **Revision.** Delete the hand-rolled loader. Use `commit_graph_if_enabled()` plus a shallow check plus a grafts-file refusal, invalidate the cached graph when the `info/commit-graph*` directory changes, and add fixtures for `core.commitGraph=false`, a corrupt `CDAT`, a chain with a missing layer, a mixed-GDA2 chain and a linked worktree.

### Gaps
- Linked-worktree behavior of git.rs:3011 was not run, because cargo is unavailable here.
- Whether alternates' commit-graphs (`objects/info/alternates`) matter for FinalGitGraph users was not examined.

---

## Q7. Is a `git for-each-ref` shell-out compatible with "run anywhere" and sandboxed desktop apps?

### Takeaway
No, not as stated. It needs git ≥ 2.45 at runtime. A Finder-launched app finds `/usr/bin/git`, which is Apple Git 2.39.5 and fails on reftable repos, and the App Sandbox blocks that binary outright. Git 3.0 plans reftable as the default for new repositories, so the "fallback" will become the main path for new repos.

### Cited Findings
- The reftable backend arrived in Git 2.45.0: "Integrate the reftable code into the refs framework as a backend. With 'git init --ref-format=reftable' …" — [RelNotes 2.45.0](https://github.com/git/git/blob/master/Documentation/RelNotes/2.45.0.adoc)
- Git 3.0 plan: "The default storage format for references in newly created repositories will be changed from 'files' to 'reftable'" and "A prerequisite for this change is that the ecosystem is ready … alternative implementations of Git like JGit, libgit2 and Gitoxide need to support it." The default hash for new repos will also move to SHA-256. — [BreakingChanges.adoc](https://github.com/git/git/blob/master/Documentation/BreakingChanges.adoc)
- The latest git tag is v2.56.0; Git 3.0 has not shipped as of 2026-11-01. — [git/git tags](https://github.com/git/git/tags)
- **[Measured-local]** On a repo created by Homebrew Git 2.49.0 with `--ref-format=reftable`, `/usr/bin/git for-each-ref` (Apple Git 2.39.5) prints `fatal: unknown repository extension found: refstorage`. Homebrew git reads it fine.
- Apple DTS on `xcrun` in the App Sandbox: "xcrun contains an explicitly check for it being run from within a sandbox. There's no way to bypass this." The error is `xcrun: error: cannot be used within an App Sandbox.` — [Apple Developer Forums 655664](https://developer.apple.com/forums/thread/655664). The SciTE sandbox notes give the same error and "no known workaround". — [SciTE Sandbox](https://www.scintilla.org/Sandbox.html)
- Finder-launched GUI apps get a minimal PATH, typically `/usr/bin:/bin:/usr/sbin:/sbin`. Tauri ships `fix-path-env-rs` for this, and Tauri projects hit the problem in practice. — [tauri-apps/fix-path-env-rs](https://github.com/tauri-apps/fix-path-env-rs); [Burrow PR #279](https://github.com/caezium/Burrow/pull/279)
- gix has no reftable support; 0.88.0 only improved the error (PR #2970 "better-reftable-error"). — [gix CHANGELOG](https://github.com/GitoxideLabs/gitoxide/blob/main/gix/CHANGELOG.md)

### Inferences
- **Attack 5 (major).**
  - **Stock macOS.** The default `git` resolves to Apple Git 2.39.5, which cannot read reftable. On machines without the Command Line Tools, `/usr/bin/git` is a stub that prompts for an install [Background].
  - **Mac App Store builds.** The required App Sandbox blocks the `xcrun` trampoline, so the fallback cannot work.
  - **Windows.** Store/MSIX users may have no git at all [Inference]. The fallback adds a runtime dependency on a system git of a specific version, which contradicts the ledger's reason for gix ("pure Rust, same behavior on every OS").
- **Revision.**
  1. Resolve git explicitly. Probe PATH plus known locations (`/opt/homebrew/bin`, `/usr/local/bin`, Git for Windows) and require `git --version` ≥ 2.45 before using the fallback.
  2. Surface a typed `RefBackendUnsupported` error, never an empty ref set.
  3. Treat reftable as a planned feature with a trigger. When Git 3.0 ships, or gix-reftable lands, implement read-only reftable parsing. The format is specified, and only `{refname → oid}` for `refs/heads/*`, tags and HEAD is needed.
  4. Document that sandboxed distributions cannot use the fallback.
- The same PATH and version problem applies to the blueprint's other shell-outs (`git commit-graph write` as a maintenance hint, oracle commands). Oracles run in CI, so they are fine there. Runtime use is the problem.

### Gaps
- gix 0.80's behavior on a reftable repo was not run. Its `.git/refs/heads` is a stub file and `HEAD` points at `refs/heads/.invalid` [Background], so a files-backend reader could see an empty repository rather than error. That needs a fixture.
- The git version Apple ships with the newest Xcode (2026) was not checked. This machine has 2.39.5 (Apple Git-154).

---

## Q8. Claims tested and not broken

### Takeaway
The core gix capability inventory, git's commit-graph rules, the enumeration and metadata rates, and the non-merge tree-diff rate all survived. The non-merge "6+ cores" estimate held on 8 processes.

### Cited Findings
- gix 0.90.0 is the current release (2026-10-22), and 0.80.0 is about 8 months old (2026-02-22). — [crates.io](https://crates.io/crates/gix/versions)
- gix-commitgraph reads no Bloom (BIDX/BDAT) and no GDA2 chunks and cannot write graphs, confirmed in source, not just in crate-status. — [gix-commitgraph src/file/mod.rs](https://github.com/GitoxideLabs/gitoxide/blob/main/gix-commitgraph/src/file/mod.rs)
- No reftable, promisor/partial-clone or bitmap-file support in gix. — [crate-status](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)
- `line_counts()` exists in 0.90.0 and returns `None` for binary input. — [docs.rs](https://docs.rs/gix/latest/gix/object/blob/diff/struct.Platform.html)
- Changelog attributions check out by section: PR #2687 is in 0.86.0, `commit::Info::generation` in 0.87.0, PR #2970 in 0.88.0. — [gix CHANGELOG](https://github.com/GitoxideLabs/gitoxide/blob/main/gix/CHANGELOG.md)
- The rename-tracking deviation (first-found candidate versus git's four, filenames ignored) is real. — [crate-status](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)
- git's commit-graph is ignored with replace objects, grafts or shallow commits, and hash mismatches are ignored with a warning. — [technical/commit-graph](https://git-scm.com/docs/commit-graph); [gitformat-commit-graph](https://git-scm.com/docs/gitformat-commit-graph)
- v1 Bloom filters are non-portable for bytes ≥ 0x80, and v2 exists (Git 2.46.0). The claim "git ≥ 2.46 writes v2 by default" is false, but the blueprint never made it: the default is still v1 when none exist. — [RelNotes 2.46.0](https://github.com/git/git/blob/master/Documentation/RelNotes/2.46.0.adoc); [config/commitgraph.adoc](https://github.com/git/git/blob/master/Documentation/config/commitgraph.adoc)
- [Measured-local] Enumeration without a commit-graph runs at about 113k commits/s on linux, consistent with 122k/s on git.git. Metadata extraction runs at 105–111k commits/s on linux, consistent with 107k/s on git.git.
- [Measured-local] Non-merge first-parent tree diffs run at about 19.9k/s on linux, close to 18.9k/s on git.git, so the 20x larger tree did not slow them down. 8 processes reach about 106k/s, so "6+ cores" holds for *non-merge* change sets on this hardware.
- [Measured-local] Git version barely changes the result: 2.39.5 took 5.33 s and 2.49.0 took 5.49 s for non-merges. The "old Apple Git" objection does not move the numbers.
- "Bitmaps do not help this ingestion pipeline": not contradicted by anything found. Reachability bitmaps answer object reachability, not commit-graph walks or tree diffs. — [GitHub Blog](https://github.blog/2021-04-29-scaling-monorepo-maintenance/)

### Inferences
- The blueprint's "metadata-only by default, line stats lazy" policy and "commit-graph is an accelerator, never required" stance both survive. The attacks above refine *what* is derived and *how it is keyed*, not the overall shape.
- Keeping the pack tape walker only on a "digest-checked benchmark win" is a sound rule. PR #2852's delta-tree parallelism is the gix primitive to benchmark it against.

### Gaps
- No claim about gix's own tree-diff speed could be tested, because no gix build was available on this machine.
