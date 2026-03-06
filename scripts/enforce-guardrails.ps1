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

$perfBudgetPath = Join-Path $repoRoot "scripts/perf-budget.ps1"
if (Test-Path $perfBudgetPath) {
    $hasProbeMode = Select-String -Path $perfBudgetPath -Pattern '\[switch\]\$ProbeOnly' -Quiet
    $hasDockerFallback = Select-String -Path $perfBudgetPath -Pattern 'function\s+Invoke-PerfBudgetDocker' -Quiet
    $hasPolicyDetection = Select-String -Path $perfBudgetPath -Pattern 'Application Control policy has blocked this file|os error 4551' -Quiet

    if ($hasProbeMode -and $hasDockerFallback -and $hasPolicyDetection) {
        Add-Pass "Performance budget script contract present (probe mode + resilient fallback)."
    } else {
        Add-Failure "Performance budget script must keep probe mode and resilient fallback contract."
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
}

if ($failures.Count -gt 0) {
    Write-Host ""
    Write-Host "Guardrail check failed with $($failures.Count) issue(s)." -ForegroundColor Red
    exit 1
}

Write-Host ""
Write-Host "Guardrail check passed." -ForegroundColor Green
