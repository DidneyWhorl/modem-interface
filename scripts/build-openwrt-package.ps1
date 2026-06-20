# Build OpenWRT Package - Create tarball with all components
param(
    [string]$Version = "0.1.0"
)

$ErrorActionPreference = "Stop"

Write-Host "=== Creating OpenWRT Package Structure ===" -ForegroundColor Cyan
Write-Host "Version: $Version" -ForegroundColor Cyan

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectRoot = Split-Path -Parent $ScriptDir
$BuildDir = Join-Path $ProjectRoot "build-pkg"
$BackendBinary = Join-Path $ProjectRoot "backend\target\aarch64-unknown-linux-musl\release\modem-interface"
$FrontendDist = Join-Path $ProjectRoot "frontend\dist"

# Verify prerequisites
Write-Host "`nVerifying prerequisites..." -ForegroundColor Yellow

if (-not (Test-Path $BackendBinary)) {
    Write-Host "[X] Backend binary not found at: $BackendBinary" -ForegroundColor Red
    Write-Host "Run build-backend.ps1 first." -ForegroundColor Yellow
    exit 1
}
Write-Host "[OK] Backend binary found" -ForegroundColor Green

if (-not (Test-Path $FrontendDist)) {
    Write-Host "[X] Frontend build not found at: $FrontendDist" -ForegroundColor Red
    Write-Host "Run build-frontend.ps1 first." -ForegroundColor Yellow
    exit 1
}
Write-Host "[OK] Frontend build found" -ForegroundColor Green

# Clean and create build directory structure
Write-Host "`nCreating directory structure..." -ForegroundColor Yellow
if (Test-Path $BuildDir) {
    Remove-Item $BuildDir -Recurse -Force
}

$null = New-Item -ItemType Directory -Force -Path (Join-Path $BuildDir "usr\bin")
$null = New-Item -ItemType Directory -Force -Path (Join-Path $BuildDir "etc\init.d")
$null = New-Item -ItemType Directory -Force -Path (Join-Path $BuildDir "etc\config")
$null = New-Item -ItemType Directory -Force -Path (Join-Path $BuildDir "etc\modem-interface")
$null = New-Item -ItemType Directory -Force -Path (Join-Path $BuildDir "www\modem-interface")

# Copy binary
Write-Host "Copying backend binary..." -ForegroundColor Yellow
Copy-Item $BackendBinary (Join-Path $BuildDir "usr\bin\modem-interface")

# Copy init script
Write-Host "Copying init script..." -ForegroundColor Yellow
$InitScript = Join-Path $ProjectRoot "openwrt\files\etc\init.d\modem-interface"
if (Test-Path $InitScript) {
    Copy-Item $InitScript (Join-Path $BuildDir "etc\init.d\modem-interface")
} else {
    Write-Host "  Warning: Init script not found" -ForegroundColor Yellow
}

# Copy config files
Write-Host "Copying configuration files..." -ForegroundColor Yellow
$UciConfig = Join-Path $ProjectRoot "openwrt\files\etc\config\modem-interface"
if (Test-Path $UciConfig) {
    Copy-Item $UciConfig (Join-Path $BuildDir "etc\config\modem-interface")
}

$AppConfig = Join-Path $ProjectRoot "openwrt\files\etc\modem-interface\config.toml"
if (Test-Path $AppConfig) {
    Copy-Item $AppConfig (Join-Path $BuildDir "etc\modem-interface\config.toml")
}

# Copy frontend
Write-Host "Copying frontend..." -ForegroundColor Yellow
Copy-Item (Join-Path $FrontendDist "*") (Join-Path $BuildDir "www\modem-interface") -Recurse -Force

# Create tarball (requires tar.exe from Windows 10+ or Git for Windows)
Write-Host "`nCreating tarball..." -ForegroundColor Yellow
$TarballName = "modem-interface-$Version.tar.gz"
$TarballPath = Join-Path $ProjectRoot $TarballName

Push-Location $BuildDir
try {
    if (Get-Command tar -ErrorAction SilentlyContinue) {
        tar -czf $TarballPath *

        if (Test-Path $TarballPath) {
            Write-Host "`n=== Package Creation Complete ===" -ForegroundColor Green
            Write-Host "Package: $TarballName" -ForegroundColor Cyan
            $tarInfo = Get-Item $TarballPath
            Write-Host ("Size: {0:N2} MB" -f ($tarInfo.Length / 1MB)) -ForegroundColor Cyan

            Write-Host "`nPackage contents preview:" -ForegroundColor Yellow
            tar -tzf $TarballPath | Select-Object -First 20 | ForEach-Object { Write-Host "  $_" -ForegroundColor Gray }
            Write-Host "  ..." -ForegroundColor Gray
            Write-Host "(use 'tar -tzf $TarballName' to see full contents)" -ForegroundColor Gray

            Write-Host "`nNote: Windows-created tarballs don't preserve Unix permissions." -ForegroundColor Yellow
            Write-Host "After extracting on router, run:" -ForegroundColor Yellow
            Write-Host "  chmod +x /usr/bin/modem-interface" -ForegroundColor Gray
            Write-Host "  chmod +x /etc/init.d/modem-interface" -ForegroundColor Gray
        } else {
            Write-Host "[X] Failed to create tarball" -ForegroundColor Red
        }
    } else {
        Write-Host "[X] tar command not found" -ForegroundColor Red
        Write-Host "Package directory created at: $BuildDir" -ForegroundColor Yellow
        Write-Host "You can manually create the tarball or install Git for Windows which includes tar." -ForegroundColor Yellow
    }
} finally {
    Pop-Location
}
