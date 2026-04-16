# platform: windows
param(
    [string]$RepoRoot = "",
    [string]$CargoTargetRoot = "D:\cargo-target\gitgraph-lab",
    [string]$TempRoot = "D:\temp\gitgraph-lab",
    [string]$PnpmStoreRoot = "D:\pnpm-store\gitgraph-lab",
    [string]$PnpmHomeRoot = "D:\pnpm-home",
    [string]$DockerDataRoot = "D:\docker-data\gitgraph-lab",
    [string]$SandboxRoot = "D:\sandbox\faculty-server",
    [switch]$SkipPnpmInstall,
    [switch]$SkipCargoFetch,
    [switch]$RunGuardrails
)

$ErrorActionPreference = "Stop"

function Normalize-AbsolutePath {
    param([string]$PathValue)

    return [System.IO.Path]::GetFullPath($PathValue).TrimEnd('\', '/')
}

function Assert-OnDDrive {
    param(
        [string]$PathValue,
        [string]$Label
    )

    $normalized = Normalize-AbsolutePath -PathValue $PathValue
    if ($normalized -notmatch '^[dD]:\\') {
        throw ("{0} must be on D: drive. Received: {1}" -f $Label, $normalized)
    }
}

function Ensure-Directory {
    param([string]$PathValue)

    if (-not (Test-Path $PathValue)) {
        New-Item -Path $PathValue -ItemType Directory -Force | Out-Null
    }
}

function Ensure-Command {
    param([string]$CommandName)

    $cmd = Get-Command $CommandName -ErrorAction SilentlyContinue
    if ($null -eq $cmd) {
        throw ("Required command not found: " + $CommandName)
    }
}

function Invoke-Step {
    param(
        [string]$Title,
        [scriptblock]$Action
    )

    Write-Host ""
    Write-Host $Title
    & $Action
    if ($LASTEXITCODE -ne 0) {
        throw ("Step failed: " + $Title)
    }
}

function Try-Invoke-Step {
    param(
        [string]$Title,
        [scriptblock]$Action
    )

    Write-Host ""
    Write-Host $Title
    try {
        & $Action
        if ($LASTEXITCODE -eq 0) {
            return $true
        }
    }
    catch {
    }

    return $false
}

if (-not (Test-Path "D:\")) {
    throw "D: drive is required for safe installation."
}

$resolvedRepoRoot = if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
} else {
    (Resolve-Path $RepoRoot).Path
}
$resolvedRepoRoot = Normalize-AbsolutePath -PathValue $resolvedRepoRoot

Assert-OnDDrive -PathValue $resolvedRepoRoot -Label "Repository root"
Assert-OnDDrive -PathValue $CargoTargetRoot -Label "Cargo target root"
Assert-OnDDrive -PathValue $TempRoot -Label "Temp root"
Assert-OnDDrive -PathValue $PnpmStoreRoot -Label "PNPM store root"
Assert-OnDDrive -PathValue $PnpmHomeRoot -Label "PNPM home root"
Assert-OnDDrive -PathValue $DockerDataRoot -Label "Docker data root"
Assert-OnDDrive -PathValue $SandboxRoot -Label "Sandbox root"

$cargoTargetRootAbs = Normalize-AbsolutePath -PathValue $CargoTargetRoot
$tempRootAbs = Normalize-AbsolutePath -PathValue $TempRoot
$pnpmStoreRootAbs = Normalize-AbsolutePath -PathValue $PnpmStoreRoot
$pnpmHomeRootAbs = Normalize-AbsolutePath -PathValue $PnpmHomeRoot
$dockerDataRootAbs = Normalize-AbsolutePath -PathValue $DockerDataRoot
$sandboxRootAbs = Normalize-AbsolutePath -PathValue $SandboxRoot

foreach ($dir in @(
        $cargoTargetRootAbs,
        $tempRootAbs,
        $pnpmStoreRootAbs,
        $pnpmHomeRootAbs,
        $dockerDataRootAbs,
        $sandboxRootAbs
    )) {
    Ensure-Directory -PathValue $dir
}

$env:TEMP = $tempRootAbs
$env:TMP = $tempRootAbs
$env:CARGO_TARGET_DIR = $cargoTargetRootAbs
$env:PNPM_STORE_DIR = $pnpmStoreRootAbs
$env:PNPM_HOME = $pnpmHomeRootAbs
$env:GITGRAPH_WINDOWS_DATA_ROOT = $dockerDataRootAbs
$env:GITGRAPH_FACULTY_SANDBOX_ROOT = $sandboxRootAbs

if ((Test-Path $pnpmHomeRootAbs) -and ($env:Path -notlike "*$pnpmHomeRootAbs*")) {
    $env:Path = "$pnpmHomeRootAbs;$env:Path"
}

$userHome = if (-not [string]::IsNullOrWhiteSpace($env:USERPROFILE)) {
    $env:USERPROFILE
} else {
    (Resolve-Path "~").Path
}
$cargoBin = Join-Path $userHome ".cargo\bin"
if ((Test-Path $cargoBin) -and ($env:Path -notlike "*$cargoBin*")) {
    $env:Path = "$cargoBin;$env:Path"
}

Ensure-Command -CommandName "node"
Ensure-Command -CommandName "corepack"
Ensure-Command -CommandName "cargo"
Ensure-Command -CommandName "rustup"

Push-Location $resolvedRepoRoot
try {
    if (-not (Test-Path "package.json")) {
        throw ("package.json not found in repo root: " + $resolvedRepoRoot)
    }

    $packageJson = Get-Content -Path "package.json" -Raw | ConvertFrom-Json
    $packageManager = $packageJson.packageManager
    if ([string]::IsNullOrWhiteSpace($packageManager) -or $packageManager -notmatch '^pnpm@') {
        throw ("packageManager must be pnpm@<version>. Found: " + $packageManager)
    }

    $corepackEnabled = Try-Invoke-Step -Title "Enabling Corepack..." -Action { corepack enable }
    if (-not $corepackEnabled) {
        Write-Host "Corepack enable skipped (no admin write access or existing policy block)."
    }

    $corepackPrepared = Try-Invoke-Step -Title ("Preparing " + $packageManager + "...") -Action {
        corepack prepare $packageManager --activate
    }
    if (-not $corepackPrepared) {
        Write-Host ("Corepack prepare skipped for " + $packageManager + ".")
    }

    $pnpmCommand = Get-Command "pnpm" -ErrorAction SilentlyContinue
    if ($null -eq $pnpmCommand) {
        throw "pnpm command is not available. Run PowerShell as Administrator once and execute: corepack enable"
    }

    $pnpmVersion = (& pnpm --version | Out-String).Trim()
    if (-not [string]::IsNullOrWhiteSpace($pnpmVersion)) {
        Write-Host ("Using pnpm version: " + $pnpmVersion)
    }

    if (-not $SkipPnpmInstall) {
        $hasLockfile = Test-Path "pnpm-lock.yaml"
        if ($hasLockfile) {
            Invoke-Step -Title "Installing JavaScript dependencies (frozen lockfile)..." -Action { pnpm install --frozen-lockfile }
        } else {
            Invoke-Step -Title "Installing JavaScript dependencies..." -Action { pnpm install }
        }
    }

    if (-not $SkipCargoFetch) {
        Invoke-Step -Title "Fetching Rust dependencies..." -Action { cargo fetch --locked }
    }

    if ($RunGuardrails) {
        Invoke-Step -Title "Running guardrails..." -Action { pnpm run guardrails }
    }
}
finally {
    Pop-Location
}

Write-Host ""
Write-Host "Safe installation completed."
Write-Host ("Repo root: " + $resolvedRepoRoot)
Write-Host ("CARGO_TARGET_DIR: " + $env:CARGO_TARGET_DIR)
Write-Host ("PNPM_STORE_DIR: " + $env:PNPM_STORE_DIR)
Write-Host ("TEMP/TMP: " + $env:TEMP)
Write-Host ("Docker data root: " + $env:GITGRAPH_WINDOWS_DATA_ROOT)
Write-Host ("Faculty sandbox root: " + $env:GITGRAPH_FACULTY_SANDBOX_ROOT)
