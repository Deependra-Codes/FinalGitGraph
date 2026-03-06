# platform: windows
param(
    [ValidateSet("x64", "x86")]
    [string]$Arch = "x64",
    [switch]$SkipVsDevShell
)

$ErrorActionPreference = "Stop"
$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")

if (-not $SkipVsDevShell) {
    . "$PSScriptRoot\enter-vs-dev-shell.ps1" -Arch $Arch
}

$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if (Test-Path $cargoBin) {
    $env:Path = "$cargoBin;$env:Path"
}

$checks = New-Object System.Collections.Generic.List[pscustomobject]

function Add-Check {
    param(
        [string]$Name,
        [bool]$Ok,
        [string]$Details
    )

    $status = if ($Ok) { "OK" } else { "FAIL" }
    $checks.Add([pscustomobject]@{
            Check   = $Name
            Status  = $status
            Details = $Details
        })
}

function Try-Run {
    param(
        [string]$Exe,
        [string[]]$CmdArgs = @()
    )

    try {
        $output = & $Exe @CmdArgs 2>$null
        if ($LASTEXITCODE -eq 0 -or $null -eq $LASTEXITCODE) {
            return @{ ok = $true; value = ($output | Out-String).Trim() }
        }
        return @{ ok = $false; value = "" }
    }
    catch {
        return @{ ok = $false; value = "" }
    }
}

$git = Try-Run -Exe "git" -CmdArgs @("--version")
Add-Check -Name "Git" -Ok $git.ok -Details $git.value

$rustupCmd = Get-Command "rustup" -ErrorAction SilentlyContinue
$rustup = Try-Run -Exe "rustup" -CmdArgs @("--version")
$rustupOk = $null -ne $rustupCmd
$rustupDetails = if ($rustup.ok) { ($rustup.value -split "`n")[0] } else { $rustupCmd.Source }
Add-Check -Name "rustup" -Ok $rustupOk -Details $rustupDetails

$rustc = Try-Run -Exe "rustc" -CmdArgs @("--version")
Add-Check -Name "rustc" -Ok $rustc.ok -Details $rustc.value

$cargo = Try-Run -Exe "cargo" -CmdArgs @("--version")
Add-Check -Name "cargo" -Ok $cargo.ok -Details $cargo.value

$node = Try-Run -Exe "node" -CmdArgs @("--version")
Add-Check -Name "node" -Ok $node.ok -Details $node.value

$pnpm = Try-Run -Exe "pnpm" -CmdArgs @("--version")
Add-Check -Name "pnpm" -Ok $pnpm.ok -Details $pnpm.value

$clPath = Try-Run -Exe "where.exe" -CmdArgs @("cl")
Add-Check -Name "MSVC cl.exe" -Ok $clPath.ok -Details (($clPath.value -split "`n")[0])

$tauri = Try-Run -Exe "cargo" -CmdArgs @("tauri", "--version")
Add-Check -Name "cargo-tauri" -Ok $tauri.ok -Details $tauri.value

$cargoTargetDir = $env:CARGO_TARGET_DIR
$cargoTargetOk = -not [string]::IsNullOrWhiteSpace($cargoTargetDir)
if ($cargoTargetOk -and -not (Test-Path $cargoTargetDir)) {
    New-Item -Path $cargoTargetDir -ItemType Directory -Force | Out-Null
}
Add-Check -Name "CARGO_TARGET_DIR" -Ok $cargoTargetOk -Details $cargoTargetDir

$workspaceLockPath = Join-Path $repoRoot "Cargo.lock"
$workspaceLockOk = Test-Path $workspaceLockPath
Add-Check -Name "Workspace Cargo.lock" -Ok $workspaceLockOk -Details $workspaceLockPath

$webViewRegPath = "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"
$webViewVersion = ""
$webViewOk = $false
try {
    $webViewVersion = (Get-ItemProperty -Path $webViewRegPath -Name pv -ErrorAction Stop).pv
    if ($webViewVersion) {
        $webViewOk = $true
    }
}
catch {
    $webViewOk = $false
}
Add-Check -Name "WebView2 Runtime" -Ok $webViewOk -Details $webViewVersion

$checks | Format-Table -AutoSize

$failed = @($checks | Where-Object { $_.Status -eq "FAIL" })
if ($failed.Count -gt 0) {
    Write-Host ""
    Write-Host "Preflight failed. Fix the FAIL checks first."
    exit 1
}

Write-Host ""
Write-Host "Preflight passed. Environment is ready."
