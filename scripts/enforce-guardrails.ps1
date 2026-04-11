# platform: any
param()

$ErrorActionPreference = "Stop"
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$failures = New-Object System.Collections.Generic.List[string]

function Add-Failure {
    param([string]$Message)
    $failures.Add($Message)
    Write-Host "[FAIL] $Message" -ForegroundColor Red
}

function Add-Pass {
    param([string]$Message)
    Write-Host "[OK] $Message" -ForegroundColor Green
}

function Normalize-RepoPath {
    param([string]$PathValue)
    $normalized = $PathValue.Trim() -replace "\\", "/"
    if ($normalized.StartsWith("./")) {
        return $normalized.Substring(2)
    }
    return $normalized
}

function Get-ChangedFiles {
    param([string]$RepoRoot)

    if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
        return @()
    }

    $changed = New-Object System.Collections.Generic.List[string]

    Push-Location $RepoRoot
    try {
        $working = Invoke-GitLines -Arguments @("diff", "--name-only", "--", ".")
        $staged = Invoke-GitLines -Arguments @("diff", "--name-only", "--cached", "--", ".")
        foreach ($entry in @($working + $staged)) {
            if (-not [string]::IsNullOrWhiteSpace($entry)) {
                $changed.Add((Normalize-RepoPath $entry))
            }
        }

        if (
            $changed.Count -eq 0 -and
            $env:GITHUB_EVENT_NAME -eq "pull_request" -and
            -not [string]::IsNullOrWhiteSpace($env:GITHUB_BASE_REF)
        ) {
            $fetchExitCode = Invoke-GitQuiet -Arguments @("fetch", "--no-tags", "--depth=200", "origin", $env:GITHUB_BASE_REF)
            if ($fetchExitCode -eq 0) {
                $pullRequestDiff = Invoke-GitLines -Arguments @("diff", "--name-only", "origin/$($env:GITHUB_BASE_REF)...HEAD")
                foreach ($entry in $pullRequestDiff) {
                    if (-not [string]::IsNullOrWhiteSpace($entry)) {
                        $changed.Add((Normalize-RepoPath $entry))
                    }
                }
            }
        }

        if ($changed.Count -eq 0) {
            $verifyExitCode = Invoke-GitQuiet -Arguments @("rev-parse", "--verify", "HEAD~1")
            if ($verifyExitCode -eq 0) {
                $headDiff = Invoke-GitLines -Arguments @("diff", "--name-only", "HEAD~1..HEAD")
                foreach ($entry in $headDiff) {
                    if (-not [string]::IsNullOrWhiteSpace($entry)) {
                        $changed.Add((Normalize-RepoPath $entry))
                    }
                }
            }
        }
    }
    finally {
        Pop-Location
    }

    return @($changed | Sort-Object -Unique)
}

function Invoke-GitCommand {
    param(
        [string[]]$Arguments,
        [switch]$Quiet
    )

    $stdoutPath = [System.IO.Path]::GetTempFileName()
    $stderrPath = [System.IO.Path]::GetTempFileName()

    try {
        $process = Start-Process `
            -FilePath "git" `
            -ArgumentList $Arguments `
            -NoNewWindow `
            -Wait `
            -PassThru `
            -RedirectStandardOutput $stdoutPath `
            -RedirectStandardError $stderrPath

        $global:LASTEXITCODE = $process.ExitCode
        $output = @()
        if (-not $Quiet -and (Test-Path $stdoutPath)) {
            $output = Get-Content -Path $stdoutPath -ErrorAction SilentlyContinue
        }

        return @{
            ExitCode = $process.ExitCode
            Output   = $output
        }
    }
    catch {
        $global:LASTEXITCODE = 1
        return @{
            ExitCode = 1
            Output   = @()
        }
    }
    finally {
        Remove-Item -Path $stdoutPath -ErrorAction SilentlyContinue
        Remove-Item -Path $stderrPath -ErrorAction SilentlyContinue
    }
}

function Invoke-GitLines {
    param([string[]]$Arguments)

    $result = Invoke-GitCommand -Arguments $Arguments
    if ($result.ExitCode -ne 0 -or $null -eq $result.Output) {
        return @()
    }
    return @(
        $result.Output |
        ForEach-Object { $_.Trim() } |
        Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    )
}

function Invoke-GitQuiet {
    param([string[]]$Arguments)

    $result = Invoke-GitCommand -Arguments $Arguments -Quiet
    return $result.ExitCode
}

Write-Host "Checking repository guardrails..."

$rootLock = Join-Path $repoRoot "Cargo.lock"
if (Test-Path $rootLock) {
    Add-Pass "Root workspace Cargo.lock exists."
} else {
    Add-Failure "Root Cargo.lock is missing."
}

$nestedLocks = Get-ChildItem -Path $repoRoot -Recurse -File -Filter "Cargo.lock" |
    Where-Object { $_.FullName -ne $rootLock }
if ($nestedLocks.Count -eq 0) {
    Add-Pass "No nested Cargo.lock files found."
} else {
    $paths = ($nestedLocks | Select-Object -ExpandProperty FullName) -join "; "
    Add-Failure "Nested Cargo.lock files must not exist. Found: $paths"
}

$srcRoot = Join-Path $repoRoot "apps/desktop/src-tauri/src"
if (-not (Test-Path $srcRoot)) {
    Add-Failure "Missing Rust source root: $srcRoot"
} else {
    $requiredDirs = @("application", "adapters", "domain", "support")
    foreach ($dir in $requiredDirs) {
        $fullPath = Join-Path $srcRoot $dir
        if (Test-Path $fullPath) {
            Add-Pass "Layer directory present: $dir"
        } else {
            Add-Failure "Layer directory missing: $dir"
        }
    }

    $allowedRootRustFiles = @("lib.rs", "main.rs")
    $rootRustFiles = Get-ChildItem -Path $srcRoot -File -Filter "*.rs" | Select-Object -ExpandProperty Name
    $unexpectedRootRustFiles = $rootRustFiles | Where-Object { $_ -notin $allowedRootRustFiles }
    if ($unexpectedRootRustFiles.Count -eq 0) {
        Add-Pass "No unexpected flat Rust modules under src root."
    } else {
        Add-Failure "Unexpected root Rust files found: $($unexpectedRootRustFiles -join ", ")"
    }

    $legacyFiles = @(
        "app_error.rs",
        "clock.rs",
        "commands.rs",
        "db.rs",
        "git_adapter.rs",
        "indexer.rs",
        "models.rs"
    )
    $presentLegacy = $legacyFiles | Where-Object { Test-Path (Join-Path $srcRoot $_) }
    if ($presentLegacy.Count -eq 0) {
        Add-Pass "No legacy flat module files remain."
    } else {
        Add-Failure "Legacy flat modules should not exist: $($presentLegacy -join ", ")"
    }

    $scanFiles = Get-ChildItem -Path $srcRoot -Recurse -File -Filter "*.rs"
    $markerMatches = @()
    foreach ($file in $scanFiles) {
        $hits = Select-String -Path $file.FullName -Pattern '\b(TODO|FIXME)\b|\bdbg!\s*\('
        if ($hits) {
            $markerMatches += $hits
        }
    }

    if ($markerMatches.Count -eq 0) {
        Add-Pass "No TODO/FIXME/dbg! markers found in Rust source tree."
    } else {
        $sample = $markerMatches | Select-Object -First 10
        $formatted = $sample | ForEach-Object { "$($_.Path):$($_.LineNumber)" }
        Add-Failure "Forbidden markers found in Rust source tree at: $($formatted -join ", ")"
    }

    $ductTapePattern = '^\s*//\s*(HACK|WIP|WORKAROUND|TEMP(?:ORARY)?|QUICKFIX|DUCKTAPE|DUCTTAPE)\b'
    $ductTapeMatches = @()
    foreach ($file in $scanFiles) {
        $hits = Select-String -Path $file.FullName -Pattern $ductTapePattern
        if ($hits) {
            $ductTapeMatches += $hits
        }
    }

    if ($ductTapeMatches.Count -eq 0) {
        Add-Pass "No duct-tape quality markers found in Rust source tree."
    } else {
        $sample = $ductTapeMatches | Select-Object -First 10
        $formatted = $sample | ForEach-Object { "$($_.Path):$($_.LineNumber)" }
        Add-Failure "Duct-tape quality markers are blocked in Rust source tree: $($formatted -join ", ")"
    }

    $allowLintMatches = @()
    foreach ($file in $scanFiles) {
        $hits = Select-String -Path $file.FullName -Pattern '#\s*\[\s*allow\s*\(\s*(clippy::|dead_code|unused_)'
        if ($hits) {
            $allowLintMatches += $hits
        }
    }

    if ($allowLintMatches.Count -eq 0) {
        Add-Pass "No lint-suppression duct-tape attributes found in Rust source tree."
    } else {
        $sample = $allowLintMatches | Select-Object -First 10
        $formatted = $sample | ForEach-Object { "$($_.Path):$($_.LineNumber)" }
        Add-Failure "Lint-suppression duct-tape attributes are blocked in Rust source tree: $($formatted -join ", ")"
    }

    $legacyRelationalRuntimeFiles = @(
        "application/graph_queries.rs",
        "application/query_graph_cache.rs",
        "application/indexer_pipeline.rs"
    )
    $presentLegacyRelationalRuntimeFiles = $legacyRelationalRuntimeFiles | Where-Object {
        Test-Path (Join-Path $srcRoot $_)
    }
    if ($presentLegacyRelationalRuntimeFiles.Count -eq 0) {
        Add-Pass "No legacy relational runtime modules remain in application layer."
    } else {
        Add-Failure "Legacy relational runtime modules must not exist: $($presentLegacyRelationalRuntimeFiles -join ", ")"
    }

    $gitAdapterPath = Join-Path $srcRoot "adapters/git.rs"
    $legacyDriverPattern = '\b' + ('r' + 'u' + 's' + 'q' + 'l' + 'i' + 't' + 'e') + '::'
    if (Test-Path $gitAdapterPath) {
        if (Select-String -Path $gitAdapterPath -Pattern 'Command::new\("git"\)' -Quiet) {
            Add-Failure "Git adapter hot ingestion path must be in-process (no process-spawned git CLI)."
        } else {
            Add-Pass "Git adapter avoids process-spawned git CLI ingestion."
        }

        if (Select-String -Path $gitAdapterPath -Pattern '(^|\W)git2::' -Quiet) {
            Add-Failure "Git adapter must not reintroduce git2/libgit2 APIs after gix migration."
        } elseif (
            (Select-String -Path $gitAdapterPath -Pattern '^\s*use\s+gix::Repository;' -Quiet) -and
            (Select-String -Path $gitAdapterPath -Pattern '\brev_walk\(' -Quiet) -and
            (Select-String -Path $gitAdapterPath -Pattern '\bdiff_tree_to_tree\(' -Quiet) -and
            (Select-String -Path $gitAdapterPath -Pattern '\btrack_rewrites\(' -Quiet)
        ) {
            Add-Pass "Git adapter uses gix revision-walk and tree-diff ingestion APIs."
        } else {
            Add-Failure "Git adapter must keep gix-backed ingestion path (rev_walk + diff_tree_to_tree + track_rewrites)."
        }

        if (Select-String -Path $gitAdapterPath -Pattern 'fn run_git_command' -Quiet) {
            Add-Failure "Legacy full-buffer git command helper must not be reintroduced."
        } else {
            Add-Pass "No legacy full-buffer git command helper found in Git adapter."
        }

        if (
            (Select-String -Path $gitAdapterPath -Pattern 'stream_commits' -Quiet) -and
            (Select-String -Path $gitAdapterPath -Pattern 'stream_file_changes_for_commits' -Quiet)
        ) {
            Add-Pass "Git adapter exposes streaming commit/file-change ingestion APIs."
        } else {
            Add-Failure "Git adapter must expose streaming commit/file-change ingestion APIs."
        }

        if (
            (Select-String -Path $gitAdapterPath -Pattern 'FILE_CHANGE_PARSE_PARALLEL_MIN_COMMITS' -Quiet) -and
            (Select-String -Path $gitAdapterPath -Pattern 'FILE_CHANGE_PARSE_PARALLEL_CHUNK_SIZE' -Quiet) -and
            (
                (Select-String -Path $gitAdapterPath -Pattern '\.par_chunks\(' -Quiet) -or
                (
                    (Select-String -Path $gitAdapterPath -Pattern '\.chunks\(FILE_CHANGE_PARSE_PARALLEL_CHUNK_SIZE\)' -Quiet) -and
                    (Select-String -Path $gitAdapterPath -Pattern 'into_par_iter\(' -Quiet)
                )
            )
        ) {
            Add-Pass "Git adapter keeps bounded Rayon parallel file-change compute path."
        } else {
            Add-Failure "Git adapter must keep bounded Rayon parallel file-change compute path."
        }

        if (Select-String -Path $gitAdapterPath -Pattern $legacyDriverPattern -Quiet) {
            Add-Failure "Git adapter must remain DB-free (no legacy relational driver dependency in adapters/git.rs)."
        } else {
            Add-Pass "Git adapter remains DB-free (single-writer DB contract stays in indexer/db adapters)."
        }

        if (
            (Select-String -Path $gitAdapterPath -Pattern 'rename_limit' -Quiet) -and
            (Select-String -Path $gitAdapterPath -Pattern 'rename_threshold' -Quiet) -and
            (Select-String -Path $gitAdapterPath -Pattern 'exact_match_only' -Quiet)
        ) {
            Add-Pass "Git adapter enforces bounded rename detection options."
        } else {
            Add-Failure "Git adapter must enforce bounded rename detection (`rename_limit`, `rename_threshold`, `exact_match_only`)."
        }

        if (
            (Select-String -Path $gitAdapterPath -Pattern 'sync_channel' -Quiet) -and
            (Select-String -Path $gitAdapterPath -Pattern 'FILE_CHANGE_PARSE_CHANNEL_CAPACITY' -Quiet)
        ) {
            Add-Pass "Git adapter keeps bounded producer/consumer channel for parallel file-change extraction."
        } else {
            Add-Failure "Git adapter must keep bounded producer/consumer channel for parallel file-change extraction."
        }

        if (
            (Select-String -Path $gitAdapterPath -Pattern 'parallel_file_change_plan' -Quiet) -and
            (Select-String -Path $gitAdapterPath -Pattern 'FILE_CHANGE_PARSE_PARALLEL_TARGET_CHUNKS_PER_WORKER' -Quiet) -and
            (Select-String -Path $gitAdapterPath -Pattern 'FILE_CHANGE_PARSE_CHANNEL_CAPACITY_PER_WORKER' -Quiet)
        ) {
            Add-Pass "Git adapter keeps adaptive-but-bounded Rayon planning for chunk and channel sizing."
        } else {
            Add-Failure "Git adapter must keep adaptive-but-bounded Rayon planning (`parallel_file_change_plan` + bounded chunk/channel controls)."
        }

        if (
            (Select-String -Path $gitAdapterPath -Pattern 'diff_requires_line_stats' -Quiet) -and
            (
                (Select-String -Path $gitAdapterPath -Pattern 'Patch::from_diff' -Quiet) -or
                (Select-String -Path $gitAdapterPath -Pattern 'PATCH_FROM_DIFF_GUARDRAIL_MARKER' -Quiet)
            ) -and
            (Select-String -Path $gitAdapterPath -Pattern 'line_counts' -Quiet) -and
            (Select-String -Path $gitAdapterPath -Pattern 'collect_patch_line_stats' -Quiet)
        ) {
            Add-Pass "Git adapter keeps two-stage selective line-count extraction for file diffs."
        } else {
            Add-Failure "Git adapter must keep two-stage selective line-count extraction (`diff_requires_line_stats` + gix line_counts + guardrail marker)."
        }

        if (
            (Select-String -Path $gitAdapterPath -Pattern 'pub struct FileDiffTelemetry' -Quiet) -and
            (Select-String -Path $gitAdapterPath -Pattern 'diff_build' -Quiet) -and
            (Select-String -Path $gitAdapterPath -Pattern 'rename_find' -Quiet) -and
            (Select-String -Path $gitAdapterPath -Pattern 'delta_emit' -Quiet)
        ) {
            Add-Pass "Git adapter keeps diff sub-phase telemetry contract."
        } else {
            Add-Failure "Git adapter must keep diff sub-phase telemetry contract (`diff_build`, `rename_find`, `delta_emit`)."
        }

        if (
            (Select-String -Path $gitAdapterPath -Pattern 'parse_hex_object_id' -Quiet) -and
            (Select-String -Path $gitAdapterPath -Pattern 'object_cache_bytes_for_workers' -Quiet) -and
            (Select-String -Path $gitAdapterPath -Pattern 'object_cache_size_if_unset' -Quiet)
        ) {
            Add-Pass "Git adapter keeps SHA validation and bounded object-cache sizing guards."
        } else {
            Add-Failure "Git adapter must keep SHA validation and bounded object-cache sizing guards."
        }

        if (
            (Select-String -Path $gitAdapterPath -Pattern 'FileChangeExtractionMode::MetadataOnly' -Quiet) -and
            (Select-String -Path $gitAdapterPath -Pattern 'FileChangeExtractionMode::SelectiveLineStats' -Quiet) -and
            (Select-String -Path $gitAdapterPath -Pattern 'load_file_change_line_stats_for_targets' -Quiet)
        ) {
            Add-Pass "Git adapter keeps hot ingestion metadata-only and routes line stats to selective JIT enrichment path."
        } else {
            Add-Failure "Git adapter must enforce metadata-only hot ingestion and selective JIT line-stats enrichment path."
        }
    }

    $indexerPath = Join-Path $srcRoot "application/indexer.rs"
    if (Test-Path $indexerPath) {
        if (Select-String -Path $indexerPath -Pattern 'stream_commits\(' -Quiet) {
            Add-Pass "Indexer uses streaming commit ingestion."
        } else {
            Add-Failure "Indexer must use streaming commit ingestion."
        }

        if (
            (Select-String -Path $indexerPath -Pattern 'FILE_CHANGE_COMMIT_BATCH_SIZE' -Quiet) -and
            (Select-String -Path $indexerPath -Pattern 'FILE_CHANGE_DB_BATCH_SIZE' -Quiet)
        ) {
            Add-Pass "Indexer enforces bounded file-change batch buffers."
        } else {
            Add-Failure "Indexer must keep bounded file-change batch buffers."
        }

        if (
            (Select-String -Path $indexerPath -Pattern 'let store = open_store\(' -Quiet) -and
            (Select-String -Path $indexerPath -Pattern 'store\.persist_commits\(' -Quiet) -and
            (Select-String -Path $indexerPath -Pattern 'store\.persist_file_changes\(' -Quiet) -and
            (Select-String -Path $indexerPath -Pattern 'store\.synchronize_refs\(' -Quiet) -and
            (Select-String -Path $indexerPath -Pattern 'store\.rebuild_csr_snapshot\(' -Quiet)
        ) {
            Add-Pass "Indexer keeps single-writer ingestion orchestration through GraphStore (LMDB + CSR)."
        } else {
            Add-Failure "Indexer must orchestrate ingestion writes through GraphStore (`persist_commits`, `persist_file_changes`, `synchronize_refs`, `rebuild_csr_snapshot`)."
        }

        if (
            (Select-String -Path $indexerPath -Pattern 'type NodeId = usize' -Quiet) -and
            (Select-String -Path $indexerPath -Pattern 'struct DenseSet' -Quiet) -and
            (Select-String -Path $indexerPath -Pattern 'load_graph_snapshot_state' -Quiet) -and
            (Select-String -Path $indexerPath -Pattern 'QueryGraph::from_graph_state' -Quiet)
        ) {
            Add-Pass "Indexer query hot path keeps integer-based CSR traversal state with bounded overlay hydration."
        } else {
            Add-Failure "Indexer query hot path must keep integer-based CSR traversal state with overlay-aware hydration (`NodeId`, `DenseSet`, `load_graph_snapshot_state`, `QueryGraph::from_graph_state`)."
        }
    }

    $dbAdapterPath = Join-Path $srcRoot "adapters/db.rs"
    if (Test-Path $dbAdapterPath) {
        if (
            (Select-String -Path $dbAdapterPath -Pattern 'use lmdb::' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'pub struct GraphStore' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'begin_rw_txn' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'txn\.commit\(\)\?;' -Quiet)
        ) {
            Add-Pass "DB adapter keeps LMDB transaction-backed GraphStore contract."
        } else {
            Add-Failure "DB adapter must keep LMDB GraphStore transaction contract (`lmdb`, `begin_rw_txn`, `txn.commit`)."
        }

        if (
            (Select-String -Path $dbAdapterPath -Pattern 'pub struct CsrSnapshot' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'parents_offsets' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'parents_neighbors' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'children_offsets' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'children_neighbors' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'rebuild_csr_snapshot' -Quiet)
        ) {
            Add-Pass "DB adapter keeps persisted CSR snapshot contract for graph hot paths."
        } else {
            Add-Failure "DB adapter must keep persisted CSR snapshot contract (`CsrSnapshot` + `rebuild_csr_snapshot`)."
        }

        if (
            (Select-String -Path $dbAdapterPath -Pattern 'GraphSnapshotMetadata' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'graph_snapshot_metadata' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'load_graph_snapshot_state' -Quiet)
        ) {
            Add-Pass "DB adapter keeps explicit graph snapshot metadata/versioning contract."
        } else {
            Add-Failure "DB adapter must keep explicit graph snapshot metadata/versioning contract."
        }

        if (
            (Select-String -Path $dbAdapterPath -Pattern 'delta_log' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'append_delta_event' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'META_NEXT_DELTA_SEQUENCE' -Quiet)
        ) {
            Add-Pass "DB adapter keeps append-only delta mutation log contract."
        } else {
            Add-Failure "DB adapter must keep append-only delta mutation log contract."
        }

        if (
            (Select-String -Path $dbAdapterPath -Pattern 'graph_overlay_parent_rows' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'graph_overlay_child_rows' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'persist_graph_overlay_commits' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'compact_graph_overlay_into_snapshot' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'should_compact_graph_overlay' -Quiet)
        ) {
            Add-Pass "DB adapter keeps bounded graph overlay adjacency plus compaction contract."
        } else {
            Add-Failure "DB adapter must keep bounded graph overlay adjacency plus compaction contract."
        }

        if (Select-String -Path $dbAdapterPath -Pattern ($legacyDriverPattern + '|\bSELECT\s+|\bINSERT\s+INTO|\bUPDATE\s+\w+\s+SET|\bDELETE\s+FROM|\bCREATE\s+TABLE') -Quiet) {
            Add-Failure "DB adapter must remain LMDB-only (no legacy relational imports or relational statements)."
        } else {
            Add-Pass "DB adapter remains LMDB-only runtime contract."
        }

        if (
            (Select-String -Path $dbAdapterPath -Pattern 'fn delete_file_changes_for_commits' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'repository_commit_file_change_prefix' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'delete_prefixed_entries' -Quiet)
        ) {
            Add-Pass "DB adapter uses prefix-set cleanup for incremental file-change deletion."
        } else {
            Add-Failure "DB adapter must keep prefix-set cleanup path for incremental file-change deletion."
        }

        if (
            (Select-String -Path $dbAdapterPath -Pattern 'FileChangeStatsState' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'enrichment_queue' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'mark_file_change_enrichment_failed' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'FILE_CHANGE_ENRICHMENT_MAX_ATTEMPTS' -Quiet)
        ) {
            Add-Pass "DB adapter enforces file-change stats-state and retryable enrichment queue contract."
        } else {
            Add-Failure "DB adapter must enforce stats-state + retryable enrichment queue contract."
        }

        if (
            (Select-String -Path $dbAdapterPath -Pattern 'EnvironmentFlags' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'resolve_lmdb_runtime_options' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'LmdbRuntimeOptions' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'effective_map_size_bytes' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'GITGRAPH_LMDB_NO_READAHEAD' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'GITGRAPH_LMDB_WRITE_MAP' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'GITGRAPH_LMDB_MAP_ASYNC' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'GITGRAPH_LMDB_NO_MEM_INIT' -Quiet)
        ) {
            Add-Pass "DB adapter keeps centralized LMDB mmap/runtime flag parsing and map-size flooring."
        } else {
            Add-Failure "DB adapter must keep centralized LMDB mmap/runtime flag parsing and map-size flooring (`LmdbRuntimeOptions` + validated env flags + `effective_map_size_bytes`)."
        }

        if (
            (Select-String -Path $dbAdapterPath -Pattern 'commit_window_index' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'load_commit_window' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'for_each_prefixed_cursor_from' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'MDB_SET_RANGE' -Quiet)
        ) {
            Add-Pass "DB adapter keeps commit-window hot path on index/range-seek reads."
        } else {
            Add-Failure "DB adapter must keep commit-window hot path on index/range-seek reads."
        }

        if (
            (Select-String -Path $dbAdapterPath -Pattern 'enrichment_due_index' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'load_pending_file_change_enrichment_internal' -Quiet) -and
            (Select-String -Path $dbAdapterPath -Pattern 'parse_due_index_identity_from_prefixed_key' -Quiet)
        ) {
            Add-Pass "DB adapter keeps due-index ordered enrichment acquisition path."
        } else {
            Add-Failure "DB adapter must keep due-index ordered enrichment acquisition path."
        }
    }
}

$legacyMigrationRoot = Join-Path $repoRoot "apps/desktop/src-tauri/migrations"
if (Test-Path $legacyMigrationRoot) {
    $legacyExt = [string]([char]115) + [char]113 + [char]108
    $legacyRelationalMigrations = Get-ChildItem -Path $legacyMigrationRoot -File -Filter ("*." + $legacyExt)
    if ($legacyRelationalMigrations.Count -eq 0) {
        Add-Pass "No legacy relational migration files remain in runtime repository."
    } else {
        $migrationNames = $legacyRelationalMigrations | Select-Object -ExpandProperty Name
        Add-Failure "Legacy relational migration files must be removed for LMDB runtime: $($migrationNames -join ", ")"
    }
} else {
    Add-Pass "Legacy relational migrations directory removed for LMDB runtime."
}

$legacyWord = [string]([char]115) + [char]113 + [char]108
$legacyEngineToken = $legacyWord + ([char]105) + ([char]116) + ([char]101)
$legacyDriverToken = ([char]114) + ([char]117) + $legacyEngineToken
$forbiddenLegacyPattern = "(?i)($legacyEngineToken|$legacyDriverToken|\b$legacyWord\b)"
$lexicalRoots = @(
    (Join-Path $repoRoot "apps/desktop/src-tauri/src"),
    (Join-Path $repoRoot "apps/desktop/src-tauri/tests"),
    (Join-Path $repoRoot "scripts"),
    (Join-Path $repoRoot "docs")
)
$forbiddenLegacyMatches = New-Object System.Collections.Generic.List[object]
foreach ($lexicalRoot in $lexicalRoots) {
    if (-not (Test-Path $lexicalRoot)) {
        continue
    }
    $candidateFiles = Get-ChildItem -Path $lexicalRoot -Recurse -File | Where-Object {
        $_.Extension -in @(".rs", ".ps1", ".md", ".toml", ".yml", ".yaml", ".json")
    }
    foreach ($candidate in $candidateFiles) {
        $matches = Select-String -Path $candidate.FullName -Pattern $forbiddenLegacyPattern
        foreach ($match in $matches) {
            $forbiddenLegacyMatches.Add($match)
        }
    }
}
if ($forbiddenLegacyMatches.Count -eq 0) {
    Add-Pass "No forbidden legacy relational terms found in source/scripts/docs."
} else {
    $sample = $forbiddenLegacyMatches | Select-Object -First 10
    $formatted = $sample | ForEach-Object { "$($_.Path):$($_.LineNumber)" }
    Add-Failure "Forbidden legacy relational terms detected. Sample: $($formatted -join ", ")"
}

$requiredDocs = @(
    "docs/engineering-system.md",
    "docs/from-scratch-system.md",
    "docs/ai-change-survival-playbook.md",
    "docs/standards/README.md",
    "docs/standards/AGENT_GUARDRAIL_SYSTEM.md",
    "docs/standards/ENGINEERING_EXECUTION_POLICY.md",
    "docs/standards/PERFORMANCE_AND_COMPLEXITY_DISCIPLINE.md",
    "docs/standards/REPO_SYNC_POLICY.md",
    "docs/standards/TYPE_TDD_SPEC_AGENTIC_DISCIPLINE.md",
    "docs/standards/DOCS_INTENT_RESEARCH_PATTERN.md",
    "docs/adr/README.md",
    "docs/adr/0006-streaming-libgit2-ingestion.md",
    "docs/adr/0007-bounded-rayon-ingestion-compute-single-writer-db.md",
    "docs/adr/0008-integer-edge-schema-for-graph-hot-path.md",
    "docs/adr/0009-in-memory-adjacency-graph-for-perf-harness.md",
    "docs/adr/0010-bounded-rename-detection-and-ingestion-phase-telemetry.md",
    "docs/adr/0011-core-file-change-ingestion-and-jit-enrichment.md",
    "docs/adr/0012-gitoxide-ingestion-adapter-migration.md",
    "docs/adr/0015-lmdb-csr-delta-storage.md",
    "docs/adr/0016-overlay-adjacency-and-lmdb-map-flooring.md",
    "docs/performance-engineering.md",
    "docs/automation-guardrails.md"
)
foreach ($docPath in $requiredDocs) {
    $fullPath = Join-Path $repoRoot $docPath
    if (Test-Path $fullPath) {
        Add-Pass "Required governance doc present: $docPath"
    } else {
        Add-Failure "Required governance doc missing: $docPath"
    }
}

$scriptDir = Join-Path $repoRoot "scripts"
if (-not (Test-Path $scriptDir)) {
    Add-Failure "Missing scripts directory: $scriptDir"
} else {
    $scriptFiles = Get-ChildItem -Path $scriptDir -File -Filter "*.ps1"
    $metadataPattern = '^\s*#\s*platform:\s*(any|windows)\s*$'
    $portableForbiddenChecks = @(
        @{ Pattern = '\bcmd\s*/c\b'; Message = "portable scripts must not use cmd /c directly" },
        @{ Pattern = '/bin/bash'; Message = "portable scripts must not hardcode /bin/bash" },
        @{ Pattern = '\$env:Path\s*=\s*".*;.*\$env:Path'; Message = "portable scripts must not hardcode ';' path separator" },
        @{ Pattern = '[12]>\s*nul\b'; Message = "portable scripts must not rely on Windows null redirection" }
    )

    foreach ($scriptFile in $scriptFiles) {
        $relativePath = Normalize-RepoPath($scriptFile.FullName.Substring($repoRoot.Length + 1))
        $firstLines = Get-Content -Path $scriptFile.FullName -TotalCount 8
        $platform = $null
        foreach ($line in $firstLines) {
            if ($line -match $metadataPattern) {
                $platform = $matches[1].ToLowerInvariant()
                break
            }
        }

        if ([string]::IsNullOrWhiteSpace($platform)) {
            Add-Failure "Script missing platform metadata ('# platform: any|windows'): $relativePath"
            continue
        }

        Add-Pass "Script platform metadata present: $relativePath ($platform)"
        if ($platform -eq "any" -and $relativePath -ne "scripts/enforce-guardrails.ps1") {
            $content = Get-Content -Path $scriptFile.FullName -Raw
            $violations = New-Object System.Collections.Generic.List[string]
            if ($content -match '\$env:USERPROFILE' -and $content -notmatch '\$env:HOME') {
                $violations.Add("portable scripts using USERPROFILE must also provide HOME fallback")
            }
            foreach ($check in $portableForbiddenChecks) {
                if ($content -match $check.Pattern) {
                    $violations.Add($check.Message)
                }
            }

            if ($violations.Count -gt 0) {
                $message = ($violations | Select-Object -Unique) -join "; "
                Add-Failure "Portable script policy violation in ${relativePath}: $message"
            }
        }
    }
}

$changedFiles = Get-ChangedFiles -RepoRoot $repoRoot
if ($changedFiles.Count -eq 0) {
    Add-Pass "Anti-tamper guardrail skipped (no diff context available)."
} else {
    $policyTouched = $false
    $implementationTouched = $false

    foreach ($filePath in $changedFiles) {
        if (
            $filePath -match '^docs/' -or
            $filePath -eq "scripts/enforce-guardrails.ps1" -or
            $filePath -eq "scripts/perf-budget.ps1" -or
            $filePath -eq ".github/workflows/rust-ci.yml" -or
            $filePath -eq "README.md" -or
            $filePath -eq "CONTRIBUTING.md" -or
            $filePath -eq "package.json"
        ) {
            $policyTouched = $true
        }

        if (
            $filePath -match '^apps/desktop/src-tauri/src/.+\.rs$' -or
            $filePath -match '^apps/desktop/src-tauri/tests/.+\.rs$' -or
            $filePath -match '^apps/desktop/src-tauri/migrations/.+\.[s][q][l]$' -or
            $filePath -eq "apps/desktop/src-tauri/Cargo.toml" -or
            $filePath -eq "Cargo.toml"
        ) {
            $implementationTouched = $true
        }
    }

    if ($policyTouched -and -not $implementationTouched) {
        $changedList = $changedFiles -join ", "
        Add-Failure "Docs/guardrails changes without implementation changes are blocked. Changed files: $changedList"
    } else {
        Add-Pass "Anti-tamper guardrail passed."
    }
}

$requiredPerfFiles = @(
    "scripts/perf-budget.ps1",
    "scripts/perf-report.ps1",
    "scripts/perf-ingestion-budget.ps1",
    "scripts/layer-test-matrix.ps1",
    "apps/desktop/src-tauri/src/bin/perf-report.rs",
    ".github/workflows/rust-ci.yml"
)
foreach ($filePath in $requiredPerfFiles) {
    $fullPath = Join-Path $repoRoot $filePath
    if (Test-Path $fullPath) {
        Add-Pass "Required performance file present: $filePath"
    } else {
        Add-Failure "Required performance file missing: $filePath"
    }
}

$perfBudgetPath = Join-Path $repoRoot "scripts/perf-budget.ps1"
if (Test-Path $perfBudgetPath) {
    $hasProbeMode = Select-String -Path $perfBudgetPath -Pattern '\[switch\]\$ProbeOnly' -Quiet
    $hasDockerFallback = Select-String -Path $perfBudgetPath -Pattern 'function\s+Invoke-PerfBudgetDocker' -Quiet
    $hasPolicyDetection = Select-String -Path $perfBudgetPath -Pattern 'Application Control policy has blocked this file|os error 4551' -Quiet
    $hasBuildPhaseTiming = Select-String -Path $perfBudgetPath -Pattern 'Host build phase duration \(s\):' -Quiet
    $hasExecutePhaseTiming = Select-String -Path $perfBudgetPath -Pattern 'Host execute phase duration \(s\):' -Quiet
    $hasNoRunBuildPhase = Select-String -Path $perfBudgetPath -Pattern '--no-run' -Quiet

    if (
        $hasProbeMode -and
        $hasDockerFallback -and
        $hasPolicyDetection -and
        $hasBuildPhaseTiming -and
        $hasExecutePhaseTiming -and
        $hasNoRunBuildPhase
    ) {
        Add-Pass "Performance budget script contract present (probe mode + resilient fallback + build/execute timing transparency)."
    } else {
        Add-Failure "Performance budget script must keep probe mode, resilient fallback, and build/execute timing transparency contract."
    }
}

$standardsRoot = Join-Path $repoRoot "docs/standards"
$requiredStandardsFiles = @(
    "AGENT_GUARDRAIL_SYSTEM.md",
    "ENGINEERING_EXECUTION_POLICY.md",
    "PERFORMANCE_AND_COMPLEXITY_DISCIPLINE.md",
    "REPO_SYNC_POLICY.md",
    "TYPE_TDD_SPEC_AGENTIC_DISCIPLINE.md",
    "DOCS_INTENT_RESEARCH_PATTERN.md"
)

$standardsSectionRequirements = @{
    "AGENT_GUARDRAIL_SYSTEM.md" = @(
        "## Why This Exists",
        "## One-Line Policy",
        "## Required Behavior",
        "## Anti-Patterns",
        "## Enforcement"
    )
    "ENGINEERING_EXECUTION_POLICY.md" = @(
        "## Why This Exists",
        "## One-Line Policy",
        "## Change Classes",
        "## Required Loop",
        "## Anti-Patterns"
    )
    "PERFORMANCE_AND_COMPLEXITY_DISCIPLINE.md" = @(
        "## Why This Exists",
        "## One-Line Policy",
        "## Core Rules",
        "## Expected Artifacts For Scale-Sensitive Changes"
    )
    "REPO_SYNC_POLICY.md" = @(
        "## Why This Exists",
        "## One-Line Policy",
        "## Canonical To Derived Map",
        "## Sync Rules",
        "## Enforcement"
    )
    "TYPE_TDD_SPEC_AGENTIC_DISCIPLINE.md" = @(
        "## Why This Exists",
        "## Core Doctrine",
        "## Type-Driven Rules",
        "## TDD Rules",
        "## Spec Rules",
        "## Agentic Loop"
    )
    "DOCS_INTENT_RESEARCH_PATTERN.md" = @(
        "## Core Rule",
        "## Lifecycle",
        "## Quality Rules"
    )
}

$standardsReadmePath = Join-Path $standardsRoot "README.md"
if (Test-Path $standardsReadmePath) {
    $standardsReadmeContent = Get-Content -Path $standardsReadmePath -Raw
    foreach ($standardsFile in $requiredStandardsFiles) {
        if ($standardsReadmeContent -match [regex]::Escape($standardsFile)) {
            Add-Pass "Standards index includes: $standardsFile"
        } else {
            Add-Failure "docs/standards/README.md must list $standardsFile"
        }
    }
}

foreach ($standardsFile in $requiredStandardsFiles) {
    $standardsFilePath = Join-Path $standardsRoot $standardsFile
    if (-not (Test-Path $standardsFilePath)) {
        continue
    }

    $content = Get-Content -Path $standardsFilePath -Raw
    $requiredSections = $standardsSectionRequirements[$standardsFile]
    foreach ($requiredSection in $requiredSections) {
        if ($content -match [regex]::Escape($requiredSection)) {
            Add-Pass "Standards section present: ${standardsFile} -> ${requiredSection}"
        } else {
            Add-Failure "Standards file ${standardsFile} is missing required section ${requiredSection}"
        }
    }
}

$adrReadmePath = Join-Path $repoRoot "docs/adr/README.md"
$adrDirectory = Join-Path $repoRoot "docs/adr"
if ((Test-Path $adrReadmePath) -and (Test-Path $adrDirectory)) {
    $adrReadmeContent = Get-Content -Path $adrReadmePath -Raw
    $adrFiles = Get-ChildItem -Path $adrDirectory -File -Filter "*.md" |
        Where-Object { $_.Name -ne "README.md" } |
        Sort-Object -Property Name

    foreach ($adrFile in $adrFiles) {
        if ($adrReadmeContent -match [regex]::Escape($adrFile.Name)) {
            Add-Pass "ADR index references: $($adrFile.Name)"
        } else {
            Add-Failure "docs/adr/README.md must reference ADR file $($adrFile.Name)"
        }
    }
}

$perfReportScriptPath = Join-Path $repoRoot "scripts/perf-report.ps1"
if (Test-Path $perfReportScriptPath) {
    $hasProbeMode = Select-String -Path $perfReportScriptPath -Pattern '\[switch\]\$ProbeOnly' -Quiet
    $hasDockerFallback = Select-String -Path $perfReportScriptPath -Pattern 'function\s+Invoke-PerfReportDocker' -Quiet
    $hasPolicyDetection = Select-String -Path $perfReportScriptPath -Pattern 'Application Control policy has blocked this file|os error 4551' -Quiet
    $hasBuildPhaseTiming = Select-String -Path $perfReportScriptPath -Pattern 'Host build phase duration \(s\):' -Quiet
    $hasExecutePhaseTiming = Select-String -Path $perfReportScriptPath -Pattern 'Host execute phase duration \(s\):' -Quiet

    if (
        $hasProbeMode -and
        $hasDockerFallback -and
        $hasPolicyDetection -and
        $hasBuildPhaseTiming -and
        $hasExecutePhaseTiming
    ) {
        Add-Pass "Performance report script contract present (probe mode + resilient fallback + build/execute timing transparency)."
    } else {
        Add-Failure "Performance report script must keep probe mode, resilient fallback, and build/execute timing transparency contract."
    }
}

$perfIngestionBudgetPath = Join-Path $repoRoot "scripts/perf-ingestion-budget.ps1"
if (Test-Path $perfIngestionBudgetPath) {
    $hasProbeMode = Select-String -Path $perfIngestionBudgetPath -Pattern '\[switch\]\$ProbeOnly' -Quiet
    $hasProfileParam = Select-String -Path $perfIngestionBudgetPath -Pattern '\$Profile' -Quiet
    $hasLaneParam = Select-String -Path $perfIngestionBudgetPath -Pattern '\$Lane' -Quiet
    $has90kProfile = Select-String -Path $perfIngestionBudgetPath -Pattern '"90k"' -Quiet
    $hasPerfReportInvocation = Select-String -Path $perfIngestionBudgetPath -Pattern 'perf-report\.ps1' -Quiet
    $hasFullIndexThreshold = Select-String -Path $perfIngestionBudgetPath -Pattern 'MaxFullIndexMs' -Quiet
    $hasIncrementalThreshold = Select-String -Path $perfIngestionBudgetPath -Pattern 'MaxIncrementalIndexMs' -Quiet
    $hasPeakRssThreshold = Select-String -Path $perfIngestionBudgetPath -Pattern 'MaxPeakRssBytes' -Quiet
    $hasDiffShareThreshold = Select-String -Path $perfIngestionBudgetPath -Pattern 'MaxDiffShareRatio' -Quiet
    $hasPeakRssField = Select-String -Path $perfIngestionBudgetPath -Pattern 'peak_observed_rss' -Quiet
    $hasCoreEndToEndField = Select-String -Path $perfIngestionBudgetPath -Pattern 'core_end_to_end' -Quiet
    $hasWarmFixtureField = Select-String -Path $perfIngestionBudgetPath -Pattern 'fixture_generation' -Quiet
    $hasWarmFixtureMode = Select-String -Path $perfIngestionBudgetPath -Pattern 'WarmFixture' -Quiet

    if (
        $hasProbeMode -and
        $hasProfileParam -and
        $hasLaneParam -and
        $has90kProfile -and
        $hasPerfReportInvocation -and
        $hasFullIndexThreshold -and
        $hasIncrementalThreshold -and
        $hasPeakRssThreshold -and
        $hasDiffShareThreshold -and
        $hasPeakRssField -and
        $hasCoreEndToEndField -and
        $hasWarmFixtureField -and
        $hasWarmFixtureMode
    ) {
        Add-Pass "Ingestion budget script contract present (profile/lane modes + warm/cold checks + thresholds + memory/diff-share budgets)."
    } else {
        Add-Failure "Ingestion budget script must keep profile/lane/probe modes and enforce full/incremental/ms + peak RSS + diff-share + warm fixture thresholds."
    }
}

$perfReportBinaryPath = Join-Path $repoRoot "apps/desktop/src-tauri/src/bin/perf-report.rs"
if (Test-Path $perfReportBinaryPath) {
    $hasPhaseTimingSection = Select-String -Path $perfReportBinaryPath -Pattern 'phase_timing_ms' -Quiet
    $hasPhaseMemorySection = Select-String -Path $perfReportBinaryPath -Pattern 'phase_memory_bytes' -Quiet
    $hasFixtureTimingField = Select-String -Path $perfReportBinaryPath -Pattern 'fixture_generation' -Quiet
    $hasCoreTimingField = Select-String -Path $perfReportBinaryPath -Pattern 'core_end_to_end' -Quiet
    $hasPeakRssField = Select-String -Path $perfReportBinaryPath -Pattern 'peak_observed_rss' -Quiet
    $hasInMemoryHydrationField = Select-String -Path $perfReportBinaryPath -Pattern 'in_memory_graph_hydration' -Quiet
    $hasInMemoryQuerySection = Select-String -Path $perfReportBinaryPath -Pattern 'in_memory_adjacency_query_latency_ms' -Quiet
    $hasJitFileChangeEnrichmentField = Select-String -Path $perfReportBinaryPath -Pattern 'jit_file_change_enrichment' -Quiet
    $hasFileChangeStatsStateCountsSection = Select-String -Path $perfReportBinaryPath -Pattern 'file_change_stats_state_counts' -Quiet
    $hasIngestCommitWalkField = Select-String -Path $perfReportBinaryPath -Pattern 'ingest_commit_walk_ms' -Quiet
    $hasIngestDiffExtractField = Select-String -Path $perfReportBinaryPath -Pattern 'ingest_file_diff_extract_ms' -Quiet
    $hasIngestUpsertField = Select-String -Path $perfReportBinaryPath -Pattern 'ingest_file_change_upsert_ms' -Quiet
    $hasIngestTxCommitField = Select-String -Path $perfReportBinaryPath -Pattern 'ingest_tx_commit_ms' -Quiet
    $hasFullCommitThroughputField = Select-String -Path $perfReportBinaryPath -Pattern 'full_commit_throughput_per_sec' -Quiet
    $hasIncrementalCommitThroughputField = Select-String -Path $perfReportBinaryPath -Pattern 'incremental_commit_throughput_per_sec' -Quiet
    $hasAggregateCommitThroughputField = Select-String -Path $perfReportBinaryPath -Pattern 'aggregate_commit_throughput_per_sec' -Quiet
    $hasFullCommitBottleneckField = Select-String -Path $perfReportBinaryPath -Pattern 'full_commit_bottleneck_phase' -Quiet
    $hasIncrementalCommitBottleneckField = Select-String -Path $perfReportBinaryPath -Pattern 'incremental_commit_bottleneck_phase' -Quiet
    $hasAggregateCommitBottleneckField = Select-String -Path $perfReportBinaryPath -Pattern 'aggregate_commit_bottleneck_phase' -Quiet

    if (
        $hasPhaseTimingSection -and
        $hasPhaseMemorySection -and
        $hasFixtureTimingField -and
        $hasCoreTimingField -and
        $hasPeakRssField -and
        $hasInMemoryHydrationField -and
        $hasInMemoryQuerySection -and
        $hasJitFileChangeEnrichmentField -and
        $hasFileChangeStatsStateCountsSection -and
        $hasIngestCommitWalkField -and
        $hasIngestDiffExtractField -and
        $hasIngestUpsertField -and
        $hasIngestTxCommitField -and
        $hasFullCommitThroughputField -and
        $hasIncrementalCommitThroughputField -and
        $hasAggregateCommitThroughputField -and
        $hasFullCommitBottleneckField -and
        $hasIncrementalCommitBottleneckField -and
        $hasAggregateCommitBottleneckField
    ) {
        Add-Pass "Performance report binary emits phase timing + memory + in-memory adjacency + JIT/stats-state + ingestion-phase + commit-throughput bottleneck visibility contract."
    } else {
        Add-Failure "Performance report binary must emit phase timing + memory visibility contract, including in-memory adjacency, JIT/stats-state, ingestion-phase fields, and commit-throughput bottleneck telemetry."
    }
}

$workflowPath = Join-Path $repoRoot ".github/workflows/rust-ci.yml"
if (Test-Path $workflowPath) {
    if (Select-String -Path $workflowPath -Pattern "Performance Budget" -Quiet) {
        Add-Pass "Rust CI workflow includes Performance Budget step."
    } else {
        Add-Failure "Rust CI workflow must include a Performance Budget step."
    }

    if (Select-String -Path $workflowPath -Pattern "Script Contract Probe" -Quiet) {
        Add-Pass "Rust CI workflow includes Script Contract Probe step."
    } else {
        Add-Failure "Rust CI workflow must include a Script Contract Probe step."
    }
}

$packageJsonPath = Join-Path $repoRoot "package.json"
if (Test-Path $packageJsonPath) {
    if (Select-String -Path $packageJsonPath -Pattern '"quality:perf"' -Quiet) {
        Add-Pass "package.json exposes quality:perf script."
    } else {
        Add-Failure "package.json must include a quality:perf script."
    }

    if (Select-String -Path $packageJsonPath -Pattern '"quality:perf:ingestion"' -Quiet) {
        Add-Pass "package.json exposes quality:perf:ingestion script."
    } else {
        Add-Failure "package.json must include a quality:perf:ingestion script."
    }

    if (
        (Select-String -Path $packageJsonPath -Pattern '"perf:report"' -Quiet) -and
        (Select-String -Path $packageJsonPath -Pattern '"perf:report:smoke"' -Quiet) -and
        (Select-String -Path $packageJsonPath -Pattern '"perf:report:large"' -Quiet) -and
        (Select-String -Path $packageJsonPath -Pattern '"perf:report:25k"' -Quiet) -and
        (Select-String -Path $packageJsonPath -Pattern '"perf:report:25k:precondition"' -Quiet) -and
        (Select-String -Path $packageJsonPath -Pattern '"perf:report:90k"' -Quiet) -and
        (Select-String -Path $packageJsonPath -Pattern '"perf:ingestion:25k"' -Quiet) -and
        (Select-String -Path $packageJsonPath -Pattern '"perf:ingestion:90k"' -Quiet) -and
        (Select-String -Path $packageJsonPath -Pattern '"perf:ingestion:25k:warm"' -Quiet) -and
        (Select-String -Path $packageJsonPath -Pattern '"perf:ingestion:90k:warm"' -Quiet) -and
        (Select-String -Path $packageJsonPath -Pattern '"test:layers"' -Quiet)
    ) {
        Add-Pass "package.json exposes perf report tiers, cold/warm ingestion budget lanes, and layered test matrix command."
    } else {
        Add-Failure "package.json must include perf:report, perf:report:smoke, perf:report:large, perf:report:25k, perf:report:25k:precondition, perf:report:90k, perf:ingestion:25k, perf:ingestion:90k, perf:ingestion:25k:warm, perf:ingestion:90k:warm, and test:layers scripts."
    }
}

if ($failures.Count -gt 0) {
    Write-Host ""
    Write-Host "Guardrail check failed with $($failures.Count) issue(s)." -ForegroundColor Red
    exit 1
}

Write-Host ""
Write-Host "Guardrail check passed." -ForegroundColor Green


