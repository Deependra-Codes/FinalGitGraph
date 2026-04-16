# platform: windows
param(
    [Parameter(Mandatory = $true)]
    [string]$Image,
    [string]$Command = "",
    [string]$RepoPath = "",
    [string]$SandboxRoot = "D:\sandbox\faculty-server",
    [switch]$AllowNetwork,
    [switch]$ReadWriteRepo,
    [switch]$RelaxedFilesystem
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

if (-not (Test-Path "D:\")) {
    throw "D: drive is required for sandbox execution."
}

$resolvedRepoPath = if ([string]::IsNullOrWhiteSpace($RepoPath)) {
    (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
} else {
    (Resolve-Path $RepoPath).Path
}
$resolvedRepoPath = Normalize-AbsolutePath -PathValue $resolvedRepoPath
$resolvedSandboxRoot = Normalize-AbsolutePath -PathValue $SandboxRoot

Assert-OnDDrive -PathValue $resolvedRepoPath -Label "Repository path"
Assert-OnDDrive -PathValue $resolvedSandboxRoot -Label "Sandbox root"

$homeRoot = Join-Path $resolvedSandboxRoot "home"
$tmpRoot = Join-Path $resolvedSandboxRoot "tmp"
$dataRoot = Join-Path $resolvedSandboxRoot "data"
$logsRoot = Join-Path $resolvedSandboxRoot "logs"

foreach ($dir in @($resolvedSandboxRoot, $homeRoot, $tmpRoot, $dataRoot, $logsRoot)) {
    if (-not (Test-Path $dir)) {
        New-Item -Path $dir -ItemType Directory -Force | Out-Null
    }
}

docker info --format "{{.ServerVersion}}" *> $null
if ($LASTEXITCODE -ne 0) {
    throw "Docker daemon is not running. Start Docker Desktop first."
}

$repoMount = if ($ReadWriteRepo) {
    "${resolvedRepoPath}:/work"
} else {
    "${resolvedRepoPath}:/work:ro"
}

$dockerArgs = @(
    "run",
    "--rm",
    "--name",
    "faculty-server-sandbox",
    "--security-opt",
    "no-new-privileges:true",
    "--cap-drop",
    "ALL",
    "--pids-limit",
    "256",
    "-v",
    $repoMount,
    "-v",
    "${homeRoot}:/sandbox/home",
    "-v",
    "${tmpRoot}:/sandbox/tmp",
    "-v",
    "${dataRoot}:/sandbox/data",
    "-v",
    "${logsRoot}:/sandbox/logs",
    "-e",
    "HOME=/sandbox/home",
    "-e",
    "TMPDIR=/sandbox/tmp",
    "-e",
    "XDG_CACHE_HOME=/sandbox/home/.cache",
    "-w",
    "/work"
)

if (-not $AllowNetwork) {
    $dockerArgs += @("--network", "none")
}

if (-not $RelaxedFilesystem) {
    $dockerArgs += @(
        "--read-only",
        "--tmpfs",
        "/tmp",
        "--tmpfs",
        "/run"
    )
}

$dockerArgs += $Image
if ([string]::IsNullOrWhiteSpace($Command)) {
    $dockerArgs += "bash"
} else {
    $dockerArgs += @("bash", "-lc", $Command)
}

Write-Host ("Sandbox repo mount: " + $repoMount)
Write-Host ("Sandbox writable root: " + $resolvedSandboxRoot)
Write-Host ("Sandbox network enabled: " + ($AllowNetwork.IsPresent))
Write-Host ("Sandbox strict filesystem: " + (-not $RelaxedFilesystem.IsPresent))

& docker @dockerArgs
if ($LASTEXITCODE -ne 0) {
    throw "Faculty server sandbox run failed."
}

Write-Host "Faculty server sandbox run completed."
