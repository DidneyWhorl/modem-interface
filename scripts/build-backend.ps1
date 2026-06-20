# Build Backend - Cross-compile Rust for aarch64
param(
    [string]$Target = "aarch64-unknown-linux-musl"
)

$ErrorActionPreference = "Stop"

Write-Host "=== Building Rust Backend for $Target ===" -ForegroundColor Cyan

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectRoot = Split-Path -Parent $ScriptDir
$BackendDir = Join-Path $ProjectRoot "backend"

Push-Location $BackendDir

try {
    # Check if cargo-zigbuild is installed
    Write-Host "Checking for cargo-zigbuild..." -ForegroundColor Yellow
    $zigbuildInstalled = $null
    try {
        $zigbuildInstalled = cargo zigbuild --version 2>$null
    } catch {}

    if (-not $zigbuildInstalled) {
        Write-Host "cargo-zigbuild not found. Installing..." -ForegroundColor Yellow
        cargo install cargo-zigbuild
    } else {
        Write-Host "[OK] cargo-zigbuild found" -ForegroundColor Green
    }

    # Cross-compile with real-hardware feature
    Write-Host "`nCross-compiling with cargo-zigbuild..." -ForegroundColor Yellow
    cargo zigbuild --release --target $Target --no-default-features --features real-hardware,tls,embedded-frontend

    $BinaryPath = Join-Path $BackendDir "target\$Target\release\modem-interface"
    if (-not (Test-Path $BinaryPath)) {
        $BinaryPath = Join-Path $BackendDir "target\$Target\release\modem-interface.exe"
    }

    Write-Host "`n=== Backend Build Complete ===" -ForegroundColor Green
    Write-Host "Binary location: $BinaryPath" -ForegroundColor Cyan

    if (Test-Path $BinaryPath) {
        $fileInfo = Get-Item $BinaryPath
        Write-Host ("Binary size: {0:N2} MB" -f ($fileInfo.Length / 1MB)) -ForegroundColor Cyan

        # Verify architecture (requires 'file' command from Git for Windows or WSL)
        if (Get-Command file -ErrorAction SilentlyContinue) {
            Write-Host "`nArchitecture verification:" -ForegroundColor Yellow
            $archInfo = file $BinaryPath
            if ($archInfo -like "*aarch64*") {
                Write-Host "[OK] Correct architecture (aarch64)" -ForegroundColor Green
            } else {
                Write-Host "[!] Warning: Architecture mismatch" -ForegroundColor Red
                Write-Host $archInfo -ForegroundColor Gray
            }
        }
    } else {
        Write-Host "[X] Binary not found at expected location" -ForegroundColor Red
    }

} finally {
    Pop-Location
}
