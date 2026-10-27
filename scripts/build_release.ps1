[CmdletBinding()]
param(
    [switch]$SkipTests,
    [switch]$SkipInstaller
)

$ErrorActionPreference = "Stop"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$AgentRoot = Split-Path -Parent $ScriptDir

Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host "       Zev Detector - Production Build and Package        " -ForegroundColor Cyan
Write-Host "==========================================================" -ForegroundColor Cyan

Set-Location $AgentRoot

# 1. Run Tests
if (-not $SkipTests) {
    Write-Host "`n[1/5] Running unit tests..." -ForegroundColor Yellow
    cargo test --lib
    if ($LASTEXITCODE -ne 0) {
        Write-Error "Unit tests failed! Build aborted."
        exit $LASTEXITCODE
    }
    Write-Host "All unit tests passed." -ForegroundColor Green
} else {
    Write-Host "`n[1/5] Skipping unit tests (-SkipTests specified)." -ForegroundColor DarkGray
}

# 2. Build Release Binaries
Write-Host "`n[2/5] Compiling release binaries..." -ForegroundColor Yellow
cargo build --release --bin zev-detector --bin zev-detector-ui
if ($LASTEXITCODE -ne 0) {
    Write-Error "Cargo release build failed!"
    exit $LASTEXITCODE
}
Write-Host "Release binaries compiled successfully." -ForegroundColor Green

$ReleaseDir = Join-Path $AgentRoot "target\release"
$DetectorExe = Join-Path $ReleaseDir "zev-detector.exe"
$UiExe = Join-Path $ReleaseDir "zev-detector-ui.exe"

if (-not (Test-Path $DetectorExe) -or -not (Test-Path $UiExe)) {
    Write-Error "Compiled executables not found in $ReleaseDir"
    exit 1
}

# 3. Detect Version & Calculate Checksums
Write-Host "`n[3/5] Inspecting binaries and calculating SHA-256..." -ForegroundColor Yellow
$VersionRaw = (& $DetectorExe --version)
$Version = if ($VersionRaw -match '[\d\.]+') { $Matches[0] } else { "1.3.0" }
Write-Host "Detected Agent Version: $Version" -ForegroundColor Cyan

$HashDetector = (Get-FileHash -Path $DetectorExe -Algorithm SHA256).Hash
$HashUi = (Get-FileHash -Path $UiExe -Algorithm SHA256).Hash

# 4. Prepare Standalone Distribution Bundle
Write-Host "`n[4/5] Preparing standalone distribution package..." -ForegroundColor Yellow
$DistDir = Join-Path $AgentRoot "dist"
$StandaloneDir = Join-Path $DistDir "Zev-Detector-v$Version-standalone"

if (Test-Path $StandaloneDir) {
    Stop-Service -Name "ZevDetector" -Force -ErrorAction SilentlyContinue
    Stop-Process -Name "zev-detector", "zev-detector-ui" -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 800
    try {
        Remove-Item -Path $StandaloneDir -Recurse -Force -ErrorAction Stop
    } catch {
        Write-Warning "Could not clean entire ${StandaloneDir}: $_"
    }
}
New-Item -ItemType Directory -Path $StandaloneDir -Force | Out-Null

try {
    Copy-Item $DetectorExe -Destination $StandaloneDir -Force -ErrorAction Stop
} catch {
    Write-Warning "zev-detector.exe in standalone dir is locked by active service. Proceeding with installer build."
}
try {
    Copy-Item $UiExe -Destination $StandaloneDir -Force -ErrorAction Stop
} catch {
    Write-Warning "zev-detector-ui.exe in standalone dir could not be replaced: $_"
}
Copy-Item (Join-Path $AgentRoot "config.sample.toml") -Destination (Join-Path $StandaloneDir "config.sample.toml") -Force -ErrorAction SilentlyContinue
Copy-Item (Join-Path $AgentRoot "README.md") -Destination (Join-Path $StandaloneDir "README.md") -Force -ErrorAction SilentlyContinue

# Helper batch generator
$ElevateLines = @(
    '@echo off',
    'net session >nul 2>&1',
    'if %errorlevel% neq 0 (',
    '    echo Requesting administrative permissions...',
    '    powershell -NoProfile -ExecutionPolicy Bypass -Command "Start-Process cmd -ArgumentList ''/c \"""%~dp0%~nx0\""'' -Verb RunAs"',
    '    exit /b',
    ')',
    'cd /d "%~dp0"'
)

# install_service.bat
$InstallLines = $ElevateLines + @(
    'echo ========================================================',
    'echo Installing Zev Detector Windows Service...',
    'echo ========================================================',
    'zev-detector.exe install',
    'echo.',
    'echo Service status:',
    'zev-detector.exe status',
    'echo.',
    'pause'
)
$InstallLines -join "`r`n" | Set-Content (Join-Path $StandaloneDir "install_service.bat") -Encoding ASCII

# uninstall_service.bat
$UninstallLines = $ElevateLines + @(
    'echo ========================================================',
    'echo Removing Zev Detector Windows Service...',
    'echo ========================================================',
    'zev-detector.exe uninstall',
    'echo.',
    'pause'
)
$UninstallLines -join "`r`n" | Set-Content (Join-Path $StandaloneDir "uninstall_service.bat") -Encoding ASCII

# start_service.bat
$StartLines = $ElevateLines + @(
    'zev-detector.exe start',
    'zev-detector.exe status',
    'pause'
)
$StartLines -join "`r`n" | Set-Content (Join-Path $StandaloneDir "start_service.bat") -Encoding ASCII

# stop_service.bat
$StopLines = $ElevateLines + @(
    'zev-detector.exe stop',
    'zev-detector.exe status',
    'pause'
)
$StopLines -join "`r`n" | Set-Content (Join-Path $StandaloneDir "stop_service.bat") -Encoding ASCII

# status_service.bat
$StatusLines = @(
    '@echo off',
    'cd /d "%~dp0"',
    'zev-detector.exe status',
    'echo.',
    'pause'
)
$StatusLines -join "`r`n" | Set-Content (Join-Path $StandaloneDir "status_service.bat") -Encoding ASCII

# test_collection.bat
$TestLines = @(
    '@echo off',
    'cd /d "%~dp0"',
    'echo Running one collection test pass (nothing uploaded)...',
    'echo.',
    'zev-detector.exe test',
    'echo.',
    'pause'
)
$TestLines -join "`r`n" | Set-Content (Join-Path $StandaloneDir "test_collection.bat") -Encoding ASCII

# open_ui.bat
$UiLines = @(
    '@echo off',
    'cd /d "%~dp0"',
    'start "" "zev-detector-ui.exe"'
)
$UiLines -join "`r`n" | Set-Content (Join-Path $StandaloneDir "open_ui.bat") -Encoding ASCII

# Checksums
$Checksums = @(
    "$HashDetector *zev-detector.exe",
    "$HashUi *zev-detector-ui.exe"
)
$Checksums -join "`r`n" | Set-Content (Join-Path $StandaloneDir "SHA256SUMS.txt") -Encoding ASCII
$Checksums -join "`r`n" | Set-Content (Join-Path $DistDir "SHA256SUMS.txt") -Encoding ASCII

# Create ZIP archive
$ZipPath = Join-Path $DistDir "Zev-Detector-v$Version-standalone.zip"
if (Test-Path $ZipPath) { Remove-Item $ZipPath -Force }
Compress-Archive -Path "$StandaloneDir\*" -DestinationPath $ZipPath -Force
Write-Host "Standalone ZIP package created: $ZipPath" -ForegroundColor Green

# 5. Inno Setup Compiler check
Write-Host "`n[5/5] Checking Inno Setup compiler for installer generation..." -ForegroundColor Yellow
$IsccPaths = @(
    "ISCC.exe",
    "${env:LOCALAPPDATA}\Programs\Inno Setup 6\ISCC.exe",
    "${env:LOCALAPPDATA}\Programs\Inno Setup 5\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles}\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 5\ISCC.exe",
    "${env:ProgramFiles}\Inno Setup 5\ISCC.exe"
)

$Iscc = $null
foreach ($p in $IsccPaths) {
    if (Get-Command $p -ErrorAction SilentlyContinue) {
        $Iscc = $p
        break
    } elseif (Test-Path $p) {
        $Iscc = $p
        break
    }
}

if ($Iscc -and (-not $SkipInstaller)) {
    Write-Host "Found Inno Setup Compiler at: $Iscc" -ForegroundColor Cyan
    $IssPath = Join-Path $AgentRoot "installer\setup.iss"
    & $Iscc $IssPath
    if ($LASTEXITCODE -eq 0) {
        Write-Host "Windows Installer generated successfully in dist\" -ForegroundColor Green
    } else {
        Write-Warning "Inno Setup compilation exited with code $LASTEXITCODE"
    }
} else {
    Write-Host "Inno Setup Compiler (ISCC.exe) not found on system. Standalone package & pre-existing installers available in dist\" -ForegroundColor DarkGray
}

Write-Host "`n==========================================================" -ForegroundColor Cyan
Write-Host "                   BUILD SUMMARY                         " -ForegroundColor Cyan
Write-Host "==========================================================" -ForegroundColor Cyan
[PSCustomObject]@{
    Artifact = "zev-detector.exe"
    SizeMB = [math]::Round((Get-Item $DetectorExe).Length / 1MB, 2)
    SHA256 = $HashDetector
} | Format-List

[PSCustomObject]@{
    Artifact = "zev-detector-ui.exe"
    SizeMB = [math]::Round((Get-Item $UiExe).Length / 1MB, 2)
    SHA256 = $HashUi
} | Format-List

Write-Host "Standalone Bundle : $StandaloneDir" -ForegroundColor Green
Write-Host "Distribution ZIP  : $ZipPath" -ForegroundColor Green
Write-Host "Checksums File    : $(Join-Path $DistDir 'SHA256SUMS.txt')" -ForegroundColor Green
Write-Host "Done!`n" -ForegroundColor Cyan
