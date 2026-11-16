# Red-team review of the FinalGitGraph blueprint: novelty claim, codebase audit, cross-note consistency, migration plan (as of 2026-11-01)

Scope: an adversarial review of `reports/FinalGitGraph backend architecture blueprint.md` (called "blueprint" below) and its notes `codebase_audit.md` ("audit") and `frontier_novel_architectures.md` ("frontier note"). Method: I re-read the cited code line by line, grepped the scripts and guardrails, read the CI workflow, and fetched primary sources on prior art. Nothing was compiled or run. On this Mac there is no cargo or pwsh. Code citations are local file paths with line numbers. Paths are relative to `/Users/singh.deependra-int/Desktop/git/FinalGitGraph/`. Labels: **Evidence** means I re-read the code or source. **Inference** means my reasoning from that evidence.

## Ranked attack list (the deliverable)

### Takeaway
The headline bug is real and confirmed. The plan built on it has errors. Phase 0 deletes the function that the incremental overlay compaction itself calls. No phase repairs indexes that are already corrupted. Several audit fixes collide with live guardrail string checks, which AGENTS.md rule 6 makes a governance problem. The audit missed a second wrong-answer bug that is at least as serious: in incremental mode, the default walk prunes every commit older than the checkpoint, so it can drop a whole merged branch in an ordinary merge workflow. The novelty claim is weaker than stated. Git's own test suite already runs "commit-graph on vs off must produce identical output". Sapling's dag already ships an isomorphism check between DAG indexes. gitoxide already tests against git CLI baselines.

### Cited Findings
Each attack lists the claim and its location, the attack type, the evidence, the severity and a proposed revision.

**A1 (critical): missed bug. Incremental mode silently skips old-dated commits reachable from new tips. This is not just "clock skew".**
- Claim attacked: blueprint table row 13, "Unsound ordering and checkpoint … Clock skew can misorder commits or miss them", ranked 13th. The audit lists the 300 s lookback only under "left out".
- Evidence:
  - The checkpoint is the *maximum* committer time seen ([indexer.rs:1399-1415](apps/desktop/src-tauri/src/application/indexer.rs)).
  - The incremental `since_epoch` is that value minus 300 s ([indexer.rs:367-369](apps/desktop/src-tauri/src/application/indexer.rs)).
  - The default walk scope is `Frontier` ([git.rs:2233-2241](apps/desktop/src-tauri/src/adapters/git.rs)).
  - In Frontier scope, any commit with `committer_time < since` is neither emitted nor expanded. That holds in the tape walker ([git.rs:1124-1131, 1158-1166](apps/desktop/src-tauri/src/adapters/git.rs)) and in both commit-graph walks ([git.rs:~1284-1293 and ~1373-1383](apps/desktop/src-tauri/src/adapters/git.rs)).
- Failure scenario 1 (Inference): index at time T. Then fetch and `git merge` a colleague's branch whose commits were made yesterday. The incremental walk emits the new merge commit. Its second parent is older than the cutoff, so it is pruned and the whole feature branch is never indexed. The merge's second-parent edge then points to an unindexed SHA, and the CSR drops it (`continue`, [db.rs:2657-2659](apps/desktop/src-tauri/src/adapters/db.rs)). Only a full re-index repairs this.
- Failure scenario 2 (Inference): one future-dated commit (say, committer date 2030) raises the checkpoint, and then every real commit is below the cutoff in every later incremental run.
- Severity: **critical**. It is a wrong answer in the normal workflow, with no error, independent of the wave bug.
- Revision: rank it next to item 1. Add it to the Phase 0 test, for example "merge an old-dated branch, then run incremental". The blueprint's ref-snapshot frontier fixes it structurally. The old path needs either a hotfix (stop only at already-indexed OIDs) or a statement that incremental mode is unsound until Phase 4.

**A2 (critical, plan error): Phase 0 deletes `apply_csr_delta_from_commit_shas` and `build_incremental_csr_snapshot`, but the overlay compaction that Phase 0 relies on calls them.**
- Claim attacked: the Phase 0 row "Deletes `apply_csr_delta_from_commit_shas`, `build_incremental_csr_snapshot`". The blueprint and audit justify this with "incremental mode uses the overlay path, whose compaction resolves all overlay commits in one call (`indexer.rs:742-780`)".
- Evidence:
  - `compact_graph_overlay_into_snapshot` is implemented as `self.apply_csr_delta_from_commit_shas(repo_id, &overlay_shas)` ([db.rs:1813-1830](apps/desktop/src-tauri/src/adapters/db.rs)).
  - That function calls `build_incremental_csr_snapshot` ([db.rs:1615](apps/desktop/src-tauri/src/adapters/db.rs)).
  - The guardrail requires `compact_graph_overlay_into_snapshot` to exist ([enforce-guardrails.ps1:489-499](scripts/enforce-guardrails.ps1)).
  - Deleting the two functions therefore breaks compilation of compaction.
  - The audit's phrase "This deletes the streaming code instead of adding any" is wrong.
- Severity: **critical** for plan correctness. As written, Phase 0 cannot compile.
- Revision: Phase 0 deletes only the per-wave call site in `flush_commit_write_wave` ([indexer.rs:1595-1606](apps/desktop/src-tauri/src/application/indexer.rs)), `initialize_empty_csr_snapshot` in the full path ([indexer.rs:404-407](apps/desktop/src-tauri/src/application/indexer.rs)) and the two env switches. Keep `build_incremental_csr_snapshot` for compaction. It is correct when it receives every new commit in one call, because all new indexes are inserted before parents are resolved ([db.rs:2621-2630](apps/desktop/src-tauri/src/adapters/db.rs)). Alternatively, make compaction call `rebuild_csr_snapshot` and accept O(N) compaction cost.

**A3 (major): no phase repairs indexes that are already corrupted.**
- Claim attacked: Phase 0 exit gate "New test red before the fix, green after".
- Evidence:
  - `build_incremental_csr_snapshot` copies old parent and child rows verbatim. It can only add edges *from* new commits ([db.rs:2700-2760](apps/desktop/src-tauri/src/adapters/db.rs)).
  - Nothing re-links an edge from an existing snapshot node to a later-arriving parent. A grep of every CSR call site found no re-link path ([indexer.rs:756, 772-774, 789, 799, 1597](apps/desktop/src-tauri/src/application/indexer.rs)).
  - After Phase 0, an existing index with dropped edges keeps them through every incremental run and compaction.
- Severity: **major**.
- Revision: bump the snapshot or format version in Phase 0, or force `rebuild_csr_snapshot` on open when `directed_edges < Σ parents of indexed commits`. State it in ADR-0019.

**A4 (major, AGENTS.md rule 6 conflict): many planned deletions break live guardrail string checks. The blueprint checked only the Phase 0 names.**
- Claim attacked: "A grep of the scripts found no guardrail that requires the deleted functions." That is true for Phase 0 only. The Phase 3 deletions, audit fixes 9/13/14/18 and the "fix the 0015 collision" action were not checked.
- Evidence (every line re-read):
  - Guardrail requires `delta_log`, `append_delta_event` and `META_NEXT_DELTA_SEQUENCE` ("append-only delta mutation log contract", [enforce-guardrails.ps1:479-487](scripts/enforce-guardrails.ps1)). This conflicts with Phase 3 "Deletes `delta_log`".
  - Guardrail requires `let store = open_store\(` in indexer.rs ([enforce-guardrails.ps1:~420](scripts/enforce-guardrails.ps1)). This conflicts with Phase 3 / audit 13 "one store per process".
  - Guardrail requires `struct DenseSet` in indexer.rs ([enforce-guardrails.ps1:~433](scripts/enforce-guardrails.ps1)). This conflicts with audit 18 "keep one [bitset] in support/".
  - Guardrail requires `ingest_tx_commit_ms` in perf-report.rs ([enforce-guardrails.ps1:946](scripts/enforce-guardrails.ps1)). This conflicts with audit 9 "Delete `tx_commit`".
  - Guardrail requires `'Patch::from_diff'` OR `PATCH_FROM_DIFF_GUARDRAIL_MARKER` ([enforce-guardrails.ps1:355-360](scripts/enforce-guardrails.ps1)). The only `Patch::from_diff` text in git.rs is the marker constant itself ([git.rs:67](apps/desktop/src-tauri/src/adapters/git.rs)). Deleting the marker (audit 14) therefore fails the guardrail.
  - Guardrail requires the exact file `docs/adr/0015-lmdb-csr-delta-storage.md` ([enforce-guardrails.ps1:632](scripts/enforce-guardrails.ps1)), and the README must reference every ADR file ([:857-869](scripts/enforce-guardrails.ps1)). Renumbering the duplicate 0015 fails the guardrail.
- Severity: **major**. Each fix needs a guardrail edit in the same PR, which rule 6 forbids unless it is shown to be equal or stronger. The blueprint maps rule 6 only for retiring the whole script, at Phase 1 or 5.
- Revision: add a "guardrail delta" column to every phase, naming each removed check and its behavior-test replacement. Leave the duplicate 0015 file names alone and only add a note. Or schedule the renumbering with the xtask port.

**A5 (major): the kill criteria are not pre-registered. Most have no numbers.**
- Claim attacked: "Thresholds are written into the ADR before measuring, so a result cannot move the goalposts."
- Evidence: the blueprint's kill-criteria table uses "the pre-registered throughput", "the UI's cold-start budget", "the pre-registered margin", "the UI latency budget" and "the reference machine". None of these values appears anywhere in the blueprint. The only numeric criteria are segments/N > 0.3 and "≥100k commits/s". The second has no machine, repo, cold/warm-cache or commit-graph-present condition. Phase 4's exit gate, "pre-registered perf criteria met at 1M and 10M", points to the same empty values.
- Severity: **major** (AGENTS rule 8: traceable claims).
- Revision: state concrete numbers now. Examples: the reference machine (CPU, RAM, disk, OS); topology+metadata ≥100k commits/s on synthetic-1M with and without a commit-graph, warm cache, median of 5 runs; cold open ≤ X ms at 1M; tape walker kept only if ≥ Y% faster than gix-pack at equal digest. Otherwise remove "pre-registered" from the text.

**A6 (major, novelty overclaim): each element of the claimed "verification contract" has shipped prior art.**
- Claim attacked: "To our knowledge, FinalGitGraph is the first local git-history index whose logical content is a pure function of the reachable commit DAG … invariant to ingestion batching, parallelism, ingest path (commit-graph vs object walk) and host OS … anchored … by differential tests against the git CLI."
- Evidence:
  - Ingest-path invariance is literally a git test helper. `graph_git_two_modes` runs the same command with `core.commitGraph=true` and `=false` and `test_cmp`s the outputs, applied to `log --topo-order`, `log --graph`, `merge-base -a` and more ([git t/lib-commit-graph.sh](https://raw.githubusercontent.com/git/git/master/t/lib-commit-graph.sh)).
  - `GIT_TEST_COMMIT_GRAPH` "forces the commit-graph to be written after every 'git commit'". `GIT_TEST_SPLIT_INDEX` "forces split-index mode on the whole test suite". So git runs its full suite under alternative internal structures ([git t/README](https://raw.githubusercontent.com/git/git/master/t/README)).
  - `git commit-graph verify` checks the file "against the object database" ([git-commit-graph](https://git-scm.com/docs/git-commit-graph)), and `git fsck` runs it when `core.commitGraph` is true ([git-fsck](https://git-scm.com/docs/git-fsck)).
  - Sapling's dag ships `CheckIntegrity::check_isomorphic_graph`: "Check that the subset of the current graph (ancestors of heads) is isomorphic with the subset in the other graph". It also ships `check_segments` ("no cycles, no overlaps, no gaps") ([sapling-dag CheckIntegrity](https://docs.rs/sapling-dag/latest/dag/ops/trait.CheckIntegrity.html)).
  - gitoxide checks crates against git-generated baselines. A gix-diff PR compares its output with `git diff` baselines ([gitoxide PR 2197](https://github.com/GitoxideLabs/gitoxide/pull/2197)), and gix-testtools supplies the scripted-fixture plumbing ([gix-testtools](https://docs.rs/crate/gix-testtools/latest/source/src/lib.rs)).
  - A 2026 Rust git-history analyzer lists "differential `GixRepo` vs `GitCliRepo` cross-walker parity" in its test suite ([codelore-lib 0.30.0, 2026-09-09](https://docs.rs/codelore-lib/latest/codelore_lib/repo/git_cli_repo/index.html)).
  - Environment-invariant, bit-identical outputs are the definition of reproducible builds: "any party can recreate bit-by-bit identical copies of all specified artifacts" ([reproducible-builds.org](https://reproducible-builds.org/docs/definition/)).
- Severity: **major**. "First" would be refuted by citing git's own test helper.
- Revision: drop "first". Say instead: "adopts git's two-mode equivalence tests, Sapling's isomorphism check and gitoxide's git baselines as a release gate, extended to batch size, thread count and OS". If any novelty is claimed, limit it to "a published, cross-OS content digest as a CI artifact". Even that is weak.

**A7 (major): the novelty statement is false for the current default semantics. "Reachable commit DAG" is undefined without a ref policy. The proposed oracle also mismatches the default scope.**
- Claims attacked: the novelty wording "pure function of the reachable commit DAG", and the oracle "stored edge set equals the set parsed from `git rev-list --all --parents`" (Phase 0 test, Phase 2 oracle, audit fix 4).
- Evidence:
  - The default scope indexes only `refs/heads/*` plus a detached HEAD. It falls back to all refs only if there are no heads ([git.rs:1887-1960](apps/desktop/src-tauri/src/adapters/git.rs)).
  - A test asserts this deliberately: `commit_walk_scope_frontier_excludes_tag_and_remote_only_refs_until_all_refs_override` ([indexer_smoke.rs:347](apps/desktop/src-tauri/tests/indexer_smoke.rs)).
  - `git rev-list --all` includes tags and remote-tracking refs ([git-rev-list](https://git-scm.com/docs/git-rev-list)). On any repo with a tag-only or remote-only commit, the oracle fails for a reason unrelated to the edge bug.
- Severity: **major**. It produces false failures and makes the novelty claim imprecise.
- Revision: define the digest as F(ref policy P, format version V, derived-type versions, DAG reachable from P). Make the oracle `git rev-list --parents --branches HEAD` for the frontier policy and `--all` only for the all-refs policy.

**A8 (major, design flaw): restricting shortest path to the ID interval `[id(B), id(A)]` is wrong for `GraphDirection::Both`, which the API keeps.**
- Claim attacked: "Shortest path runs as a bidirectional BFS restricted to the ID interval `[id(B), id(A)]`", together with "Keep the API, replace the implementations".
- Evidence: the current API takes a `direction` ([indexer.rs:1209-1226](apps/desktop/src-tauri/src/application/indexer.rs)). The perf harness calls `shortest_path(head, pre_merge_main_sha, GraphDirection::Both)` ([indexer.rs:~1268](apps/desktop/src-tauri/src/application/indexer.rs)).
- Counterexample (Inference): A and B are siblings with parent P. The undirected path is A–P–B, and `id(P) < min(id(A), id(B))`. The interval excludes P, so the search answers "not found".
- Severity: **major**. It is a wrong answer by design.
- Revision: apply the interval prune only for Parents and Children direction. For Both, use unrestricted bidirectional BFS with budgets.

**A9 (major, plan coherence): the phase table contradicts itself, and Phase 3 is mostly spent on code that Phase 5 deletes.**
- Evidence:
  - Phase 1 deletes "13 `.ps1` scripts (4,191 lines)". That count is every script, including `enforce-guardrails.ps1` and `perf-budget.ps1` (`wc -l scripts/*.ps1` totals 4,191). But Phase 2 deletes `perf-budget.ps1` again, and Phase 5 deletes "grep guardrails" again.
  - "Cargo xtask mirroring every gate" in Phase 1 means porting about 1,038 lines of name-existence greps that the blueprint itself calls harmful.
  - Phase 3 hardens LMDB (drain atomicity, channel bound, due-index scan, one store per process, map-full), and Phase 5 deletes LMDB. "One store per process" protects a Tauri UI that does not exist: there is no `tauri` dependency in the crate ([Cargo.toml](apps/desktop/src-tauri/Cargo.toml)), and `commands.rs` contains no `#[tauri::command]` (grep).
  - Phase 4's gate "Old engine, new engine and git produce equal … digests" is ill-posed. The old engine's file-change semantics depend on size thresholds and env (`FILE_CHANGE_EXACT_MATCH_ONLY_MIN_DELTAS = 512`, rename limits, [git.rs:60-66](apps/desktop/src-tauri/src/adapters/git.rs)). `fc@v1` excludes similarity renames from the gate, so the digests differ by design.
- Severity: **major**.
- Revision: make Phase 1 port only the gates that check behavior (fmt, clippy, tests, the comment-marker scan). Give each script its deletion phase exactly once. Cut Phase 3 to what changes oracle results or loses user data before cutover, which is drain atomicity. Make git the only oracle in Phase 4.

**A10 (major): missed bug, now verified. No shallow, replace or graft guards in either walker.**
- Claim attacked: the blueprint says "The research found no such guard in the fast path, but this was inferred, not verified line by line."
- Evidence:
  - A grep of git.rs finds no handling of "shallow", grafts or replace refs. The only `replace` hits are `str::replace`/`mem::replace`.
  - The commit-graph is loaded unconditionally from `objects/info` ([git.rs:3000-3022](apps/desktop/src-tauri/src/adapters/git.rs)).
  - The tape walker decodes raw pack objects ([git.rs:1543-1580](apps/desktop/src-tauri/src/adapters/git.rs)), so it ignores replace refs and grafts in the object walk as well.
  - For shallow repos, `to_pending_parsed_commit` keeps the parent SHAs of missing parents ([git.rs:481-490](apps/desktop/src-tauri/src/adapters/git.rs)), while git hides them. Git disables the commit-graph when replace objects or grafts exist ([git-commit-graph](https://git-scm.com/docs/git-commit-graph)).
- Severity: **major**. FinalGitGraph's parents diverge from git's on repos with grafts, replace refs or shallow boundaries.
- Revision: upgrade the blueprint's row from "inferred" to "verified absent in both walkers". Add a Phase 0/1 guard (detect `.git/shallow`, `refs/replace/`, `info/grafts`, then fall back or report).

**A11 (major for the Tauri step): missed bug. A process-global commit-graph cache is never invalidated.**
- Evidence: `static GRAPH_CACHE: OnceLock<Mutex<CommitGraphMetadataCache>>` maps a git dir to `Option<Arc<Graph>>`. It caches `None` too and is never evicted ([git.rs:3000-3022](apps/desktop/src-tauri/src/adapters/git.rs)). A `CommitGraphBloomCache` follows the same pattern ([git.rs:~395-398](apps/desktop/src-tauri/src/adapters/git.rs)).
- Inference: in a long-lived process, a commit-graph written after the first load is never used. A stale graph object stays held, possibly with open or mapped handles to files that git's `--expire-time` cleanup wants to delete. That would be a Windows deletion conflict. Whether `gix-commitgraph` maps the files is unverified.
- Revision: key the cache on the chain file's or graph file's (path, len, mtime), the way `PackFileFingerprint` already does for packs ([git.rs:~421-436](apps/desktop/src-tauri/src/adapters/git.rs)).

**A12 (minor to major, overstated): "the bounded channel is unbounded" is correct arithmetic, but the stated consequence is wrong.**
- Evidence that the arithmetic holds:
  - `bounded::<Vec<ParsedCommit>>(resolve_commit_ingest_batch_queue_capacity(32_768, _))`, and the function returns `commit_capacity.max(1)` ([indexer.rs:426-432, 1516-1525](apps/desktop/src-tauri/src/application/indexer.rs)).
  - The walker sends Vecs of `batch_size` = 16,384 ([git.rs:747-777](apps/desktop/src-tauri/src/adapters/git.rs)), so 32,768 × 16,384 ≈ 536.9M commits.
- Evidence that the consequence ("memory grows with repo size") is not cured by the fix:
  - The tape walker builds the whole tape and emit list, and sorts it, *before* the first emit ([git.rs:1022-1068](apps/desktop/src-tauri/src/adapters/git.rs)).
  - The commit-graph walk allocates `sha_cache` and `commit_time_cache` of `num_commits` ([git.rs:~1216, ~1342](apps/desktop/src-tauri/src/adapters/git.rs)).
  - The indexer keeps every SHA in `streamed_commit_shas` ([indexer.rs:467](apps/desktop/src-tauri/src/application/indexer.rs)).
  - Memory is therefore O(N) by design. The bug only adds about one `ParsedCommit` (several Strings, roughly 200+ B, Inference) per queued commit. The practical bound is N, not 536M.
- Revision: rename it "no backpressure; peak memory adds O(N) ParsedCommits". Do not promise bounded memory from this fix.

**A13 (minor, overstated): "the perf gate times nothing" and "no test crosses a wave boundary".**
- Evidence:
  - `perf-budget.ps1` indeed has no thresholds. It only prints durations ([perf-budget.ps1:228-283](scripts/perf-budget.ps1)).
  - But CI's required job "Ingestion Budget Fast (25k cold)" runs `perf-ingestion-budget.ps1` ([rust-ci.yml:69-100](.github/workflows/rust-ci.yml)). That script throws on `full_index_ms > 45000`, `incremental_index_ms > 15000` and `peak_observed_rss_bytes > 60,000,000` ([perf-ingestion-budget.ps1:9-11, 392-400](scripts/perf-ingestion-budget.ps1)).
  - The 25k fixture has `1 + 2500×(1+6+1) = 20,001` commits ([perf-report.rs:641-644](apps/desktop/src-tauri/src/bin/perf-report.rs)), and that lane does not disable streaming ([perf-ingestion-budget.ps1:225-256](scripts/perf-ingestion-budget.ps1)).
  - So CI *does* cross a wave boundary on every run, on the buggy path. It cannot notice because results are discarded with `let _ =` ([perf-report.rs:266-301](apps/desktop/src-tauri/src/bin/perf-report.rs)).
  - `shortest_path` returns `Ok(found=false)` rather than an error ([indexer.rs:~2117-2125](apps/desktop/src-tauri/src/application/indexer.rs)).
- Revision: correct the text. Note that the cheapest Phase 0 detector is to assert on the existing perf-report query results. It already has a fast-import fixture generator ([perf-report.rs:609-626](apps/desktop/src-tauri/src/bin/perf-report.rs)).
- Dating (Evidence): streaming CSR became default-on in commit `80d4ed1` (2026-04-09). The committed `docs/perf-report-25k-final.md` is dated 2026-03-07 (25,001 nodes), so those committed numbers predate the bug. Lane outputs produced after 2026-04-09 with streaming on would measure traversal on a truncated graph (Inference).

**A14 (minor, overstated): "the gates cannot run on macOS or Linux".**
- Evidence: CI runs `./scripts/enforce-guardrails.ps1` and `perf-budget.ps1` with `shell: pwsh` on `ubuntu-latest` and `windows-latest` ([rust-ci.yml:13-17, 27-29, 64-67](.github/workflows/rust-ci.yml)). Scripts carry `# platform: any` ([dev-loop.ps1:1](scripts/dev-loop.ps1)). Only the pnpm aliases hardcode `powershell` ([package.json:9-35](package.json)). macOS is missing from the CI matrix. Two more Windows-only constraints go unmentioned: `perf-budget.ps1` and `perf-ingestion-budget.ps1` throw "D: drive is required on Windows" ([perf-budget.ps1:26](scripts/perf-budget.ps1), [perf-ingestion-budget.ps1:37-39](scripts/perf-ingestion-budget.ps1)).
- Revision: say "the pnpm aliases fail off Windows, and macOS is untested in CI. The scripts themselves run under pwsh on Linux."

**A15 (minor, factual error): the workspace lints already exist.**
- Claims attacked: "Workspace `[lints]` set `unsafe_code = "forbid"`, `clippy.dbg_macro`, `todo` … to deny", and the rules-table action "enforce `unsafe_code = "forbid"` in workspace lints".
- Evidence: the root `Cargo.toml` already has `unsafe_code = "forbid"`, `dbg_macro`, `todo`, `unwrap_used` and `expect_used = "deny"`, and the crate has `[lints] workspace = true` ([Cargo.toml:13-22](Cargo.toml), [src-tauri/Cargo.toml](apps/desktop/src-tauri/Cargo.toml)).
- Revision: credit what exists. Note that with `forbid` in force, a Rust 2024 edition bump cannot compile the tests' `set_var` calls at all, because `forbid` cannot be locally allowed. That makes the blocker in audit item 10 harder than stated.

**A16 (minor, citation mismatch): the Scion and Mononoke claim.**
- Claim attacked: "Anthropic's Scion, which added Mononoke credit after public criticism ([InfoQ, Oct 2026])".
- Evidence: the InfoQ article exists. It is "How Anthropic Redesigned Source Control for AI-Driven Development" by Eran Stiller, dated Oct 27, 2026. It says a 15 October update explicitly credited "Mononoke and its Bonsai data model". According to my fetch, it does not say the credit followed criticism ([InfoQ](https://infoq.com/news/2026/10/anthropic-source-control)). The "after criticism" framing comes from the secondary source used in the frontier note ([braindetox](https://braindetox.kr/en/posts/anthropic_scion_mononoke_derived_data_critique_2026.html)), which the blueprint does not cite.
- Severity: minor, but this is a causal claim about a real organization.
- Revision: "Scion's 15 Oct 2026 update credits Mononoke and Bonsai (InfoQ); a secondary critique links this to attribution criticism."

**A17 (minor): missed bug. The commit-graph walk can abort midway and re-emit without dedupe. Errors are also swallowed.**
- Evidence:
  - `walk_commits_from_commit_graph` can `return Ok(None)` after it has already called `on_commit`, for example on a parent-position error or out-of-range position ([git.rs:~1265-1275, ~1360-1370, 1461-1477](apps/desktop/src-tauri/src/adapters/git.rs)).
  - `walk_commits_preferred` then calls `walk_commits_tape(repo, tips, context, on_commit)` with the raw, undeduped callback ([git.rs:~904-962](apps/desktop/src-tauri/src/adapters/git.rs)), so commits are emitted twice.
  - `repo.find_commit(*oid)`'s `Err(_)` (any error, including I/O) is recorded as "missing" and silently drops the commit ([git.rs:~1568-1575](apps/desktop/src-tauri/src/adapters/git.rs)).
  - The drain does `reorder_commits_topologically(...).unwrap_or_else(|_| pending.clone())` ([indexer.rs:~994-997](apps/desktop/src-tauri/src/application/indexer.rs)).
- Revision: include these in the "typed boundary, never `continue`" contract and in fault-injection fixtures (a corrupt commit-graph).

**A18 (minor): the drain loss is worse than the audit states. It works per batch, not per commit.**
- Evidence:
  - The drain deletes rows for the whole `extraction_chunk` ([indexer.rs:1001-1003](apps/desktop/src-tauri/src/application/indexer.rs)), then extracts the batch.
  - On any `Err`, it marks *all* `pending_commits` failed and continues ([indexer.rs:1015-1029](apps/desktop/src-tauri/src/application/indexer.rs)).
  - The batch holds up to 1,024 commits by default. After 5 attempts a commit is terminal ([db.rs:35, 1015](apps/desktop/src-tauri/src/adapters/db.rs)).
  - One bad commit therefore wipes the rows of up to 1,024 commits.
  - Caveat: the rows are a regenerable cache, and a full re-index restores them. "Data loss" means "silent loss until re-index".

**A19 (minor): Phase 0 test design gaps.**
- Gaps:
  - "Default settings and no environment variables" needs active scrubbing of inherited `GITGRAPH_*` (perf scripts set `$env:` in the caller's session). In-process scrubbing needs `remove_var`, which becomes `unsafe` in Rust 2024 and is forbidden by the workspace lints. Run the CLI as a subprocess with `env_clear()` instead.
  - Equal fixture timestamps make the order follow OID, not commit index, because of the tie-breaks ([git.rs:1045-1057](apps/desktop/src-tauri/src/adapters/git.rs), [db.rs:1485-1490](apps/desktop/src-tauri/src/adapters/db.rs)). Then "forks before 16,384" is meaningless. Use strictly increasing committer dates.
  - Wave boundaries are timing-dependent. The adaptive controller doubles or halves the target based on ms/commit ([indexer.rs:181-194](apps/desktop/src-tauri/src/application/indexer.rs)), so the set of lost edges varies run to run. Assert sets, never counts.
  - Also add an incremental case of more than 8,192 new commits. Incremental waves are 8,192 ([indexer.rs:47](apps/desktop/src-tauri/src/application/indexer.rs)), and streaming is on for incremental mode by default.

**A20 (minor): "no DB in v1" leaves the mutable state of lazy line stats unassigned.**
- Evidence: the blueprint says line counts come "lazily" and that layers are immutable, and it states no store for computed line stats or JIT-hydrated commit metadata. Today metadata is emitted `Pending` with empty author and subject fields ([git.rs:481-499](apps/desktop/src-tauri/src/adapters/git.rs)) and hydrated later.
- Revision: list lazy line stats and metadata hydration in the "inventory of mutable state at cutover". This is likely the first trigger for redb.

### Inferences
- The blueprint's core thesis survives: the default path loses edges, the tests compared the system with itself, and parent-first IDs plus a git oracle are the right fix. Its *plan* and *novelty* sections need revision before an ADR cites them.
- A1 (incremental pruning) and A2 (Phase 0 cannot compile) should be treated as must-fix edits to the blueprint. A3 and A4 are must-add steps.

### Gaps
- Nothing was compiled or run. A1's failure is inferred from reading the code, not reproduced.
- Not re-verified: audit items 7/8 (due-index rescans), 12 (map-full; I found no `MapFull` handling by grep, but ADR 0016 "map flooring" was not read), 14 (the Bloom murmur3 signed-`char` quirk), 15/16 (telemetry fan-out counts).
- I did not verify whether `gix-commitgraph` keeps files memory-mapped, which affects the Windows half of A11.

## Q1. Re-verification of the audit's top findings (confirmed / overstated / wrong)

### Takeaway
Of the seven named checks, the edge loss, the dead env vars, the dead functions, `tx_commit`, the drain non-atomicity and the latent LMDB double-open are confirmed. The "unbounded channel" arithmetic is right, but its memory consequence is overstated. The audit's proposed repair for edge loss (delete the streaming functions) is wrong as worded (see A2).

### Cited Findings
- **(1) Streaming CSR cross-wave edge loss: CONFIRMED.** The default chain:
  - `parse_env_bool_with_default(LEVEL3_STREAMING_CSR_ENV, true)` ([indexer.rs:372](apps/desktop/src-tauri/src/application/indexer.rs)).
  - An unset mode gives `FullPlusIncremental` ([indexer.rs:1453-1462](apps/desktop/src-tauri/src/application/indexer.rs)), and both modes are enabled ([indexer.rs:1464-1476](apps/desktop/src-tauri/src/application/indexer.rs)).
  - Full mode initializes an empty snapshot ([indexer.rs:404-407](apps/desktop/src-tauri/src/application/indexer.rs)).
  - Each wave calls `apply_csr_delta_from_commit_shas` and sets `streaming_csr_materialized_any` ([indexer.rs:1595-1606](apps/desktop/src-tauri/src/application/indexer.rs)).
  - The final rebuild is skipped ([indexer.rs:797](apps/desktop/src-tauri/src/application/indexer.rs)), and the incremental overlay is skipped ([indexer.rs:742-743](apps/desktop/src-tauri/src/application/indexer.rs)).
  - Missing parents are dropped ([db.rs:2657-2659](apps/desktop/src-tauri/src/adapters/db.rs)), and old rows are copied verbatim with no re-link ([db.rs:2700-2760](apps/desktop/src-tauri/src/adapters/db.rs)).
  - Emission is committer-time-descending in both walkers ([git.rs:1045-1057](apps/desktop/src-tauri/src/adapters/git.rs) for the tape; the heap at [git.rs:~1340-1400](apps/desktop/src-tauri/src/adapters/git.rs), including line 1363, for the commit-graph). BFS mode also goes children-first.
  - A grep of every CSR call site found **no re-link path**.
  - The bug is unreachable only when `GITGRAPH_LEVEL3_STREAMING_CSR` is set to 0/false/no/off, when a full run has ≤1 wave (≈16,384 commits, timing-dependent through the adaptive controller), or when an incremental run has ≤8,192 new commits.
  - Nuance: "BFS stops after about 16,384 commits" holds for merge-heavy histories too, because almost every edge into a later wave is lost. Survivors are clock-skew edges whose parent appeared in an earlier wave.
- **(2) "Bounded channel is unbounded": arithmetic CONFIRMED, consequence OVERSTATED** (see A12).
- **(3) 8 never-read env vars: CONFIRMED for Rust, with nuances.**
  - A grep of `src/` and `tests/` for each name, plus `env::vars`, `vars_os`, `format!("GITGRAPH`, `concat!` and prefix scans, found zero reads.
  - `GITGRAPH_WINDOWS_DATA_ROOT` and `GITGRAPH_FACULTY_SANDBOX_ROOT` are set and echoed by [safe-install.ps1:128-129, 211-212](scripts/safe-install.ps1).
  - `GITGRAPH_PERF_REPORT_DOCKER_ONLY` is only set and restored ([perf-ingestion-budget.ps1:227-291](scripts/perf-ingestion-budget.ps1)).
  - `GITGRAPH_QUERY_RUNTIME_CACHE` and `GITGRAPH_FILE_CHANGE_PIPELINE_COMMIT_BATCH_*` are forwarded into Docker ([perf-report.ps1:251-266](scripts/perf-report.ps1)).
  - ADR 0014 still documents `GITGRAPH_QUERY_RUNTIME_CACHE` as a rollback switch ([0014-runtime-query-graph-cache.md:17](docs/adr/0014-runtime-query-graph-cache.md)). That is extra doc drift the audit missed.
  - `GITGRAPH_INGEST_PRIMARY=rocksdb` is set only under `-EnableLsmRocksdb` ([perf-report.ps1:460-462](scripts/perf-report.ps1)), not by default.
- **(4) 12 dead public functions: CONFIRMED.**
  - Each name has zero non-definition references in `src/` and `tests/` (`grep -rnw`).
  - The workspace has one member, `gitgraph-lab-core` ([Cargo.toml:3-5](Cargo.toml)), which has no `tauri` dependency, so no external or Tauri consumer exists.
  - Caveat: they are `pub` items of a lib crate (`pub mod adapters; …`, [lib.rs](apps/desktop/src-tauri/src/lib.rs)), so deleting them is an API decision for the future app crate.
  - `CommitTimeCutoff ≡ CommitTimeNewest` is confirmed ([git.rs:654](apps/desktop/src-tauri/src/adapters/git.rs), and the commit-graph branch on `BreadthFirst` only).
- **(5) `tx_commit` never assigned: CONFIRMED.**
  - The field is declared at [indexer.rs:123](apps/desktop/src-tauri/src/application/indexer.rs) and only read at :312, :1683 and :1782-1785. There are no writes.
  - `enrichment_enqueue` duplicates `file_change_merge_upsert` ([indexer.rs:1036-1039](apps/desktop/src-tauri/src/application/indexer.rs)), and `stage_insert = telemetry.total()` ([indexer.rs:1033](apps/desktop/src-tauri/src/application/indexer.rs)). Both confirmed.
  - The guardrail requires the field name ([enforce-guardrails.ps1:946](scripts/enforce-guardrails.ps1)), and the ingestion budget parses and prints it, so a 0 throughput is printed ([perf-ingestion-budget.ps1:331-386](scripts/perf-ingestion-budget.ps1)).
- **(6) Drain non-atomicity: CONFIRMED, and worse per batch** (A18).
- **(7) LMDB double-open: CONFIRMED as latent.**
  - `open_store` builds a new environment on each call ([db.rs:2082-2084](apps/desktop/src-tauri/src/adapters/db.rs)), and each `Indexer` method calls it, for example `shortest_path` ([indexer.rs:1217](apps/desktop/src-tauri/src/application/indexer.rs)).
  - Today, use within one process is sequential. The writer thread shares the same `&store` ([indexer.rs:440-444](apps/desktop/src-tauri/src/application/indexer.rs)).
  - The hazard only appears with concurrent commands, which is the audit's own framing.
  - `WRITE_MAP | MAP_ASYNC` are opt-in through env and off by default ([db.rs:26-27, 95-103](apps/desktop/src-tauri/src/adapters/db.rs)). The blueprint's "risky flags" row should say "opt-in".
- **The tautology and the 3-commit fixtures:** the assertion is [indexer_smoke.rs:85](apps/desktop/src-tauri/tests/indexer_smoke.rs), and `counts.1` is read from the same metadata ([db.rs:1878-1890](apps/desktop/src-tauri/src/adapters/db.rs)). Confirmed.
- **The env-mutation race:** tests at :20, :89 and :222 take no `env_lock`, while the later tests do ([indexer_smoke.rs:249-602, 692-715](apps/desktop/src-tauri/tests/indexer_smoke.rs)). Confirmed.
- **The ADR collision:** two `0015-*.md` files exist ([docs/adr](docs/adr)), and both are indexed as items 15 and 16 in [README.md:31-32](docs/adr/README.md). Confirmed. It is a known, indexed state, not an unnoticed accident.

### Inferences
- The audit's case behind "every gate misses this" is right, but the reason is more specific than "no test crosses a wave boundary". One CI lane *does* cross the boundary, and its harness discards results that report `found=false` without an error (A13).

### Gaps
- Bloom v1 sign-extension behavior and the due-index scan were not re-read.

## Q2. Bugs the audit missed in git.rs, db.rs, indexer.rs and perf-report.rs

### Takeaway
I found six defects that neither the audit nor the blueprint ranks: (A1) incremental pruning of old-dated reachable commits, which also covers the single future-dated commit that poisons the checkpoint; (A10) no shallow, replace or graft guards in either walker; (A11) the never-invalidated global commit-graph and Bloom caches; (A17) undeduped re-emission after a commit-graph abort, plus swallowed errors; (A18) batch-level drain wipe; (A19) timing-dependent wave boundaries.

### Cited Findings
- Incremental pruning: [git.rs:1124-1131, 1158-1166](apps/desktop/src-tauri/src/adapters/git.rs); [indexer.rs:367-369, 1399-1415](apps/desktop/src-tauri/src/application/indexer.rs); the default scope [git.rs:2233-2241](apps/desktop/src-tauri/src/adapters/git.rs).
- Walk ignoring shallow, replace and grafts: [git.rs:3000-3022, 1543-1580, 481-490](apps/desktop/src-tauri/src/adapters/git.rs). Git's rule: "The existence of replace objects or commit grafts turns off reading or writing to the commit-graph" ([git-commit-graph](https://git-scm.com/docs/git-commit-graph)).
- Stale global caches: [git.rs:3000-3022](apps/desktop/src-tauri/src/adapters/git.rs). Compare the pack fingerprinting that does exist ([git.rs:~421-436](apps/desktop/src-tauri/src/adapters/git.rs)).
- Abort then re-emit: [git.rs:~904-962](apps/desktop/src-tauri/src/adapters/git.rs). Only the *missing-tips* path is deduped and tested (`commit_graph_partial_fallback_dedupes_missing_tip_history`, [indexer_smoke.rs:514](apps/desktop/src-tauri/tests/indexer_smoke.rs)).
- Overlay compaction preserves earlier losses, because it uses the same append-only delta builder ([db.rs:1813-1830, 2600-2760](apps/desktop/src-tauri/src/adapters/db.rs)).
- The default scope silently excludes tags and remote-tracking branches ([git.rs:1925-1960](apps/desktop/src-tauri/src/adapters/git.rs)). After a fresh clone (one local branch), `origin/*` history is not indexed. That is user-visible semantics the blueprint never mentions.

### Inferences
- A1 interacts with item 1: dropped parents from pruning are never re-linked, even after compaction.
- The audit's "commit window cursor" and "overlay compaction correctness" areas held up. Compaction is internally correct for a single call (all new indexes are assigned before parents resolve). Its flaw is only the inherited append-only limit.

### Gaps
- The commit-window cursor code ([indexer.rs:~1100-1200](apps/desktop/src-tauri/src/application/indexer.rs)) and the perf-report formatter (about 1,500 lines) were not read line by line.

## Q3. Novelty: is the "verification contract" novel? Prior art and the Scion claim

### Takeaway
No, not as worded. Each property already exists in shipped practice: ingest-path invariance (git's `graph_git_two_modes`, `GIT_TEST_COMMIT_GRAPH`), verification of the index against the object database (`commit-graph verify`, run by `fsck`), DAG-index isomorphism checks (Sapling `check_isomorphic_graph`), differential baselines against git (gitoxide; CodeLore, 2026), environment-invariant bit-identical artifacts (reproducible builds), and history independence (Dolt). The only unclaimed part is applying all of them, plus batch size and OS, to one local analytics index as a CI gate. That is an engineering practice, not a research contribution. The blueprint also dropped the Merkle-root and consistency-proof parts that gave the frontier note its "Medium–High" novelty rating, yet kept the word "first".

### Cited Findings
- `graph_git_two_modes`: `git -c core.commitGraph=true $1 >output && git -c core.commitGraph=false $1 >expect && test_cmp expect output`, applied to `log --topo-order`, `log --graph`, `merge-base -a` and others ([git t/lib-commit-graph.sh](https://raw.githubusercontent.com/git/git/master/t/lib-commit-graph.sh)).
- `GIT_TEST_COMMIT_GRAPH` "forces the commit-graph to be written after every 'git commit' command". `GIT_TEST_SPLIT_INDEX` "forces split-index mode on the whole test suite" ([git t/README](https://raw.githubusercontent.com/git/git/master/t/README)).
- `verify`: "Read the commit-graph file and verify its contents against the object database" ([git-commit-graph](https://git-scm.com/docs/git-commit-graph)). "If core.commitGraph is true, the commit-graph file will also be inspected using git commit-graph verify" ([git-fsck](https://git-scm.com/docs/git-fsck)).
- `check_isomorphic_graph`: "Check that the subset of the current graph (ancestors of heads) is isomorphic with the subset in the other graph" ([sapling-dag](https://docs.rs/sapling-dag/latest/dag/ops/trait.CheckIntegrity.html)).
- gix-diff output is checked against `git diff` baselines over a corpus ([gitoxide PR 2197](https://github.com/GitoxideLabs/gitoxide/pull/2197)), with fixture plumbing from gix-testtools ([gix-testtools](https://docs.rs/crate/gix-testtools/latest/source/src/lib.rs)).
- CodeLore 0.30.0 (2026-09-09): its test suite includes "differential `GixRepo` vs `GitCliRepo` cross-walker parity" ([docs.rs codelore-lib](https://docs.rs/codelore-lib/latest/codelore_lib/repo/git_cli_repo/index.html)).
- "any party can recreate bit-by-bit identical copies of all specified artifacts" ([reproducible-builds.org](https://reproducible-builds.org/docs/definition/)).
- Mononoke derived data is verified primary: "Derived data computation is not on this critical path"; "can always be regenerated"; "Blame → Blame V2" ([Mononoke 2.3-derived-data.md](https://github.com/facebook/sapling/blob/main/eden/mononoke/docs/2.3-derived-data.md)). The page has no section on derived-data verification. This is weak negative evidence that Mononoke does not publish this kind of contract.
- InfoQ, Oct 27 2026 (Eran Stiller): the 15 Oct update credited "Mononoke and its Bonsai data model". It does not attribute the update to criticism ([InfoQ](https://infoq.com/news/2026/10/anthropic-source-control)). The frontier note's own sources place the criticism framing in a secondary blog ([braindetox](https://braindetox.kr/en/posts/anthropic_scion_mononoke_derived_data_critique_2026.html)).

### Inferences
- A content digest over OID-sorted `(oid, ordered parents)` is reproducible from `git rev-list --parents | sort | hash`. For topology, the "contract" is a test oracle, not a property unique to the system. Only the derived-fact part (`fc@v1`) adds content, and git computes that too (`log --raw`).
- Recommended wording: "FinalGitGraph's release gate combines git's two-mode equivalence tests, Sapling's isomorphism check and gitoxide-style git baselines, extended to batch size, thread count and OS, with a published content digest." No "first".

### Gaps
- Not searched in this pass: Kythe, SCIP and Zoekt determinism practices; Bazel remote-cache digest semantics; Software Heritage reproducible exports; jj index tests; PVLDB, MSR and ICSE venues. A dated search log is still required before any public claim.

## Q4. Contradictions between the notes and the blueprint, and whether each override is justified

### Takeaway
Most overrides are justified by stated reasoning: little-endian over big-endian, `read()` over mmap, no DB over redb. Two are not. The novelty wording keeps "first" after dropping the novel parts. The no-DB choice ignores lazily computed mutable state. Internal contradictions remain in the blueprint itself.

### Cited Findings
Each contradiction is followed by its judgment.
- **Endianness.** The frontier note says "explicit big-endian chunk layout, like git's"; the blueprint picks little-endian ("Either order is equally deterministic"). *Justified*, and acknowledged in the text. Caveat (Inference): "aligned heap buffer" plus `bytemuck` casts must avoid hand-rolled `std::alloc`, because `unsafe_code = "forbid"` is already on ([Cargo.toml:14](Cargo.toml)). Read into `Vec<u64>` or use a crate.
- **DB.** The frontier note keeps "a small KV only for UI and app state" and the storage note recommends redb; the blueprint says "v1 needs no embedded database". *Partly justified* (redb's 2026 bug record is cited). It is unjustified for lazy line stats and metadata hydration (A20).
- **Roots.** The frontier note's core contribution was a canonical root plus an RFC 9162 chain root ("Medium–High" novelty). The blueprint keeps only an OID-sorted content digest and defers proofs to Phase 7. *The design choice is justified*: proofs are only useful for shared indexes. *The novelty wording that remains is not* (A6).
- **mmap.** The frontier note says readers mmap layers, with O(1) startup and multi-process sharing; the blueprint uses `read()`. *Justified* by the unsafe ban. But the frontier note's memory figures ("~100 MB hot + 200 MB mmapped lazily") and the zero-copy sharing across UI, CLI and tools no longer apply. Each process now holds its own copy, and the blueprint does not restate the memory budget.
- **ID order.** The frontier note uses "deterministic Kahn … ties broken by (generation, OID)"; the blueprint uses "iterative DFS … first parent last". These are compatible only because the digest is defined independently of IDs. Under DFS, IDs depend on the tip set at ingest time. This is fine, but it should be stated.
- **Audit vs blueprint test.** Audit fix 4 sets `GITGRAPH_FILE_CHANGE_COMMIT_BATCH_SIZE=2048` in a test, which is the env-mutation pattern audit item 10 condemns. The blueprint's Phase 0 test uses defaults in a separate test binary. *The blueprint correctly overrides the audit*, but the env must still be scrubbed (A19).
- **Internal to the blueprint.** It deletes the `.ps1` scripts in Phase 1 and again (perf-budget, grep guardrails) in Phases 2 and 5 (A9). The edge-loss "fix is a deletion" while the same functions are needed for compaction (A2). The rules table proposes lints that already exist (A15).

### Inferences
- The blueprint is honest about most overrides. The pattern to fix is that it marks some changes as new or as deletions without checking what currently depends on the code.

### Gaps
- The other notes (storage, graph, portability, incremental) were read only where the blueprint cites them.

## Q5. Plan coherence, guardrails, ADRs and kill criteria

### Takeaway
Phase 0 needs a corrected deletion list (A2) and a repair step (A3). It is otherwise safe with respect to guardrails: no guardrail names the deleted streaming functions or env names, and the required `store.rebuild_csr_snapshot(` and overlay names survive. Phase 3 is mostly wasted effort. The guardrail collisions in Phases 1, 3 and 5 under rule 6 are not mapped (A4). The kill criteria lack numbers (A5).

### Cited Findings
- No guardrail mentions `apply_csr_delta`, `build_incremental_csr` or `STREAMING_CSR` ([enforce-guardrails.ps1](scripts/enforce-guardrails.ps1), grep). The indexer guardrails require `stream_commits\(`, `let store = open_store\(`, `store\.rebuild_csr_snapshot\(`, `struct DenseSet`, `load_graph_snapshot_state` and `QueryGraph::from_graph_state` ([enforce-guardrails.ps1:~404-436](scripts/enforce-guardrails.ps1)). Phase 0 keeps all of these.
- Side effect of Phase 0: `perf-ablation.ps1` still sets `GITGRAPH_LEVEL3_STREAMING_CSR_MODE` ([perf-ablation.ps1:443, 459](scripts/perf-ablation.ps1)), and `perf-report.ps1` forwards it ([perf-report.ps1:259-260](scripts/perf-report.ps1)). Phase 0 would create a ninth "set but never read" variable, the defect class of audit item 8.
- After Phase 0, the default incremental path changes from per-wave delta to overlay plus compaction at 20,000 commits / 80,000 edges ([db.rs:38-39](apps/desktop/src-tauri/src/adapters/db.rs)). The claim "hardware lane numbers should not regress" covers only the full path that lane exercises.
- ADR numbering: confirmed duplicates. The next free number, 0019, is correct. Renaming a 0015 file trips [enforce-guardrails.ps1:632](scripts/enforce-guardrails.ps1).
- Rule 6 and retiring `enforce-guardrails.ps1`: the blueprint's mapping ("map every removed check to an equal-or-stronger replacement") is the right principle. AGENTS.md "Enforcement 2" names the script ([AGENTS.md](AGENTS.md)), so AGENTS.md must change in the same PR. Rule 7 (no docs-only guardrail changes) allows this only alongside the xtask code.

### Inferences
- Reordered plan (proposal): Phase 0′ = A2-corrected deletion + forced rebuild + A1 guard (or documented unsoundness) + result assertions in perf-report. Phase 1′ = portable runner for *behavioral* gates + an explicit guardrail-delta ledger. Phase 2 unchanged. Phase 3′ = drain atomicity only. Phases 4-5 = git as the sole oracle, with numeric kill criteria written into ADR-0025 before any measurement.

### Gaps
- Not checked: whether CI branch protection marks "Ingestion Budget Fast" as a required check.

## Claims verified as correct (for the blue team's benefit)

### Takeaway
The audit and blueprint got these claims right. I re-read the code for each.

### Cited Findings
- The edge loss in the default full and incremental modes, and the absence of any re-link path ([indexer.rs:372, 404-407, 742-743, 797, 1453-1476, 1595-1606](apps/desktop/src-tauri/src/application/indexer.rs); [db.rs:2600-2760](apps/desktop/src-tauri/src/adapters/db.rs)).
- `shortest_path` reports `found=false` silently ([indexer.rs:~2117-2125](apps/desktop/src-tauri/src/application/indexer.rs)).
- The tautological edge assertion ([indexer_smoke.rs:85](apps/desktop/src-tauri/tests/indexer_smoke.rs)), and the perf harness discarding traversal results ([perf-report.rs:266-301](apps/desktop/src-tauri/src/bin/perf-report.rs)).
- `perf-budget.ps1` has no thresholds ([perf-budget.ps1:228-283](scripts/perf-budget.ps1)).
- The hardware lane disables streaming and forces breadth-first ([perf-report.ps1:462-470](scripts/perf-report.ps1)).
- The pnpm aliases use `powershell` ([package.json:9-35](package.json)), and the 13 scripts total 4,191 lines (`wc -l`).
- The guardrail requires `walk_commits_tape`, `rebuild_csr_snapshot` and the marker ([enforce-guardrails.ps1:280-287, 355-360, 424, 462](scripts/enforce-guardrails.ps1)).
- The 8 unread env vars, the 12 dead functions, `tx_commit` never assigned, the `enrichment_enqueue` double count, the drain non-atomicity, the latent double-open and the two 0015 ADR files (see Q1).
- `rust-toolchain.toml` pins `targets = ["x86_64-pc-windows-msvc"]` ([rust-toolchain.toml](rust-toolchain.toml)).
- The dependency pins (`gix 0.80.0`, `gix-pack 0.67.0`, `lmdb 0.8.0`, `bincode 1.3.3`) ([src-tauri/Cargo.toml](apps/desktop/src-tauri/Cargo.toml)).
- The Mononoke derived-data quotes and the existence of the InfoQ article (Q3).

### Inferences
- Credit where due: the blueprint's choice of a separate test binary and a 20,000-commit default-settings test is the right detector, and its novelty section is already more modest than the frontier note's. The required fixes are plan corrections and wording changes, not a rejection of the architecture.

### Gaps
- None beyond those listed in the earlier sections.
