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
        $working = Invoke-GitLines -Arguments "diff --name-only -- ."
        $staged = Invoke-GitLines -Arguments "diff --name-only --cached -- ."
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
            Invoke-GitQuiet -Arguments "fetch --no-tags --depth=200 origin $($env:GITHUB_BASE_REF)" | Out-Null
            if ($LASTEXITCODE -eq 0) {
                $pullRequestDiff = Invoke-GitLines -Arguments "diff --name-only origin/$($env:GITHUB_BASE_REF)...HEAD"
                foreach ($entry in $pullRequestDiff) {
                    if (-not [string]::IsNullOrWhiteSpace($entry)) {
                        $changed.Add((Normalize-RepoPath $entry))
                    }
                }
            }
        }

        if ($changed.Count -eq 0) {
            Invoke-GitQuiet -Arguments "rev-parse --verify HEAD~1" | Out-Null
            if ($LASTEXITCODE -eq 0) {
                $headDiff = Invoke-GitLines -Arguments "diff --name-only HEAD~1..HEAD"
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

function Invoke-GitLines {
    param([string]$Arguments)

    $isWindowsPlatform = $env:OS -eq "Windows_NT"
    if ($isWindowsPlatform) {
        $output = cmd /c "git $Arguments 2>nul"
    } else {
        $output = & /bin/bash -lc "git $Arguments 2>/dev/null"
    }
    if ($LASTEXITCODE -ne 0 -or $null -eq $output) {
        return @()
    }
    return @(
        $output |
        ForEach-Object { $_.Trim() } |
        Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    )
}

function Invoke-GitQuiet {
    param([string]$Arguments)

    $isWindowsPlatform = $env:OS -eq "Windows_NT"
    if ($isWindowsPlatform) {
        cmd /c "git $Arguments 1>nul 2>nul" | Out-Null
    } else {
        & /bin/bash -lc "git $Arguments >/dev/null 2>/dev/null" | Out-Null
    }
    return $LASTEXITCODE
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

    $graphQueriesPath = Join-Path $srcRoot "application/graph_queries.rs"
    if (Test-Path $graphQueriesPath) {
        $stringHeavyPatterns = @(
            'HashSet<String>',
            'HashMap<String,\s*String>',
            'VecDeque<String>'
        )
        $stringHeavyMatches = @()
        foreach ($pattern in $stringHeavyPatterns) {
            $hits = Select-String -Path $graphQueriesPath -Pattern $pattern
            if ($hits) {
                $stringHeavyMatches += $hits
            }
        }

        if ($stringHeavyMatches.Count -eq 0) {
            Add-Pass "Graph query engine avoids String-heavy frontier state."
        } else {
            $sample = $stringHeavyMatches | Select-Object -First 10
            $formatted = $sample | ForEach-Object { "$($_.Path):$($_.LineNumber)" }
            Add-Failure "Graph query engine must use integer frontier state. Found String-heavy patterns at: $($formatted -join ", ")"
        }
    }
}

$requiredDocs = @(
    "docs/engineering-system.md",
    "docs/from-scratch-system.md",
    "docs/adr/README.md",
    "docs/performance-engineering.md"
)
foreach ($docPath in $requiredDocs) {
    $fullPath = Join-Path $repoRoot $docPath
    if (Test-Path $fullPath) {
        Add-Pass "Required governance doc present: $docPath"
    } else {
        Add-Failure "Required governance doc missing: $docPath"
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
            $filePath -match '^apps/desktop/src-tauri/migrations/.+\.sql$' -or
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

$workflowPath = Join-Path $repoRoot ".github/workflows/rust-ci.yml"
if (Test-Path $workflowPath) {
    if (Select-String -Path $workflowPath -Pattern "Performance Budget" -Quiet) {
        Add-Pass "Rust CI workflow includes Performance Budget step."
    } else {
        Add-Failure "Rust CI workflow must include a Performance Budget step."
    }
}

$packageJsonPath = Join-Path $repoRoot "package.json"
if (Test-Path $packageJsonPath) {
    if (Select-String -Path $packageJsonPath -Pattern '"quality:perf"' -Quiet) {
        Add-Pass "package.json exposes quality:perf script."
    } else {
        Add-Failure "package.json must include a quality:perf script."
    }
}

if ($failures.Count -gt 0) {
    Write-Host ""
    Write-Host "Guardrail check failed with $($failures.Count) issue(s)." -ForegroundColor Red
    exit 1
}

Write-Host ""
Write-Host "Guardrail check passed." -ForegroundColor Green
