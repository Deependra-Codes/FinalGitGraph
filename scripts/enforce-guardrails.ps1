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
}

$requiredDocs = @(
    "docs/engineering-system.md",
    "docs/from-scratch-system.md",
    "docs/adr/README.md"
)
foreach ($docPath in $requiredDocs) {
    $fullPath = Join-Path $repoRoot $docPath
    if (Test-Path $fullPath) {
        Add-Pass "Required governance doc present: $docPath"
    } else {
        Add-Failure "Required governance doc missing: $docPath"
    }
}

if ($failures.Count -gt 0) {
    Write-Host ""
    Write-Host "Guardrail check failed with $($failures.Count) issue(s)." -ForegroundColor Red
    exit 1
}

Write-Host ""
Write-Host "Guardrail check passed." -ForegroundColor Green
