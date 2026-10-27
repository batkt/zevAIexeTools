<#
.SYNOPSIS
    Deploys the newly compiled release binaries to the local machine's service installation.
.DESCRIPTION
    Stops ZevDetector service, backs up current exes, copies target\release binaries to
    C:\Program Files\Zev Detector, restarts the service, and verifies status.
#>

$ErrorActionPreference = "Stop"

# Auto-elevate
$IsAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $IsAdmin) {
    try {
        Write-Host "Requesting administrative privileges..." -ForegroundColor Yellow
        Start-Process powershell -ArgumentList "-NoProfile -ExecutionPolicy Bypass -File `"$PSCommandPath`"" -Verb RunAs
        exit
    } catch {
        Write-Warning "Deploying to 'C:\Program Files\Zev Detector' requires Administrator rights. Please double-click deploy_local.bat or run as Administrator."
        exit 1
    }
}

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$AgentRoot = Split-Path -Parent $ScriptDir
$ReleaseDir = Join-Path $AgentRoot "target\release"
$InstallDir = "C:\Program Files\Zev Detector"

Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host "       Deploy Latest Release to Local Machine             " -ForegroundColor Cyan
Write-Host "==========================================================" -ForegroundColor Cyan

$SrcDetector = Join-Path $ReleaseDir "zev-detector.exe"
$SrcUi = Join-Path $ReleaseDir "zev-detector-ui.exe"

if (-not (Test-Path $SrcDetector) -or -not (Test-Path $SrcUi)) {
    Write-Host "Binaries not found in $ReleaseDir. Running build first..." -ForegroundColor Yellow
    & (Join-Path $ScriptDir "build_release.ps1") -SkipTests
}

if (-not (Test-Path $InstallDir)) {
    Write-Host "Creating installation directory: $InstallDir" -ForegroundColor Yellow
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

$Svc = Get-Service -Name "ZevDetector" -ErrorAction SilentlyContinue
if ($Svc) {
    if ($Svc.Status -eq "Running") {
        Write-Host "Stopping ZevDetector service..." -ForegroundColor Yellow
        Stop-Service -Name "ZevDetector" -Force
        Start-Sleep -Seconds 2
    }
}

# Backup old binaries
$BackupDir = Join-Path $InstallDir ("backup_" + (Get-Date -Format "yyyyMMdd_HHmmss"))
if (Test-Path (Join-Path $InstallDir "zev-detector.exe")) {
    New-Item -ItemType Directory -Path $BackupDir -Force | Out-Null
    Copy-Item (Join-Path $InstallDir "zev-detector*.exe") -Destination $BackupDir -Force -ErrorAction SilentlyContinue
    Write-Host "Backed up existing exes to: $BackupDir" -ForegroundColor DarkGray
}

Write-Host "Copying fresh release binaries..." -ForegroundColor Yellow
Copy-Item $SrcDetector -Destination $InstallDir -Force
Copy-Item $SrcUi -Destination $InstallDir -Force

if ($Svc) {
    Write-Host "Starting ZevDetector service..." -ForegroundColor Yellow
    Start-Service -Name "ZevDetector"
    Start-Sleep -Seconds 2
    $Svc.Refresh()
    Write-Host "Service Status: $($Svc.Status)" -ForegroundColor Green
} else {
    Write-Host "ZevDetector service is not installed yet. You can run install_service.bat to register it." -ForegroundColor Yellow
}

$ActiveVer = (& (Join-Path $InstallDir "zev-detector.exe") --version)
Write-Host ""
Write-Host "Successfully updated! Active version: $ActiveVer" -ForegroundColor Green
Write-Host "==========================================================" -ForegroundColor Cyan
