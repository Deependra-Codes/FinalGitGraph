# Research Foundations (2026-03-07)

This note captures the source-backed decisions used for the Phase 0 reset.

## Key Findings and Decisions

1. Cargo workspace should own a single lockfile and shared output model.
   - Decision: keep root `Cargo.lock` and workspace-level quality gates.
2. Workspace lint inheritance is first-class.
   - Decision: keep strict root lint policy and crate-level inheritance.
3. Rust module boundaries are the right first step before multi-crate split.
   - Decision: adopt layered internal modules (`application`, `adapters`, `domain`, `support`).
4. Tauri runtime prerequisites are platform-specific on Windows.
   - Decision: keep desktop runtime/build validation in Windows lane.
5. Security hardening for GitHub Actions recommends tight workflow controls.
   - Decision: keep dedicated CI lanes and explicit quality checks before merge.
6. Rust dependency advisory monitoring should run continuously.
   - Decision: keep `Dependency Audit` workflow enabled.
7. pnpm/Corepack pinning keeps Node package manager behavior deterministic.
   - Decision: keep pinned `packageManager` and scripted developer entrypoints.
8. Direction-optimizing BFS should switch between sparse (push) and dense (pull) traversal by frontier/work ratios, not fixed mode.
   - Decision: keep adaptive BFS mode with explicit thresholds and perf-budget verification.
9. Frontier representations and vertex identity should stay compact/integer-based in hot loops.
   - Decision: keep integer rowid/NodeId state and dense visited/frontier structures.
10. Reused prepared statements are a baseline optimization for repeated SQL hot paths.
   - Decision: enable SQLite prepared-statement caching and favor cached preparation in graph-query adapters.
11. Repository indexing hot paths should avoid full-buffer subprocess output collection.
   - Decision: migrate indexing hot path to libgit2 streaming APIs and bounded in-memory batches.
12. SQLite write-path scaling should use bounded parallel compute with single-writer ownership.
   - Decision: parallelize file-change compute via bounded Rayon chunks in Git adapter while keeping DB writes transaction-owned in indexer/db adapters.
13. Graph traversal hot paths should store edges with integer commit IDs, not SHA text pairs.
   - Decision: migrate `commit_edges` to `(repo_id, child_commit_id, parent_commit_id)` and remove SHA edge joins from DB hot-path queries.

## Sources

1. Cargo workspaces:
   - https://doc.rust-lang.org/cargo/reference/workspaces.html
2. Workspace lints:
   - https://doc.rust-lang.org/cargo/reference/workspaces.html#the-lints-table
3. Rust modules:
   - https://doc.rust-lang.org/book/ch07-02-defining-modules-to-control-scope-and-privacy.html
4. Tauri prerequisites:
   - https://v2.tauri.app/start/prerequisites/
5. Tauri project structure:
   - https://v2.tauri.app/start/project-structure/
6. Tauri capabilities security:
   - https://v2.tauri.app/security/capabilities-for-windows-and-webviews/
7. GitHub Actions security hardening:
   - https://docs.github.com/en/actions/security-for-github-actions/security-guides/security-hardening-for-github-actions
8. RustSec:
   - https://rustsec.org/
9. Node Corepack docs:
   - https://nodejs.org/api/corepack.html
10. pnpm installation/Corepack:
   - https://pnpm.io/installation
11. Direction-Optimizing BFS (SC 2012):
   - https://www.scottbeamer.net/pubs/beamer-sc2012.pdf
12. Ligra (sparse/dense push-pull operators):
   - https://jshun.csail.mit.edu/ligra.pdf
13. GraphIt push/pull scheduling docs:
   - https://graphit-lang.org/getting-started
14. GAP Benchmark Suite:
   - https://arxiv.org/abs/1508.03619
15. SQLite prepared statements intro:
   - https://www.sqlite.org/cintro.html
16. SQLite prepare flags (`SQLITE_PREPARE_PERSISTENT`):
   - https://www.sqlite.org/capi3ref.html#prepare_flags
17. Rust process output collection:
   - https://doc.rust-lang.org/std/process/struct.Command.html
18. git2 `Revwalk`:
   - https://docs.rs/git2/latest/git2/struct.Revwalk.html
19. libgit2 diff iteration:
   - https://libgit2.org/docs/reference/main/diff/git_diff_foreach.html
20. SQLite transactions (single writer model):
   - https://www.sqlite.org/lang_transaction.html
21. Rayon parallel iterators:
   - https://docs.rs/rayon/latest/rayon/iter/index.html
22. Git commit-graph format:
   - https://git-scm.com/docs/gitformat-commit-graph
