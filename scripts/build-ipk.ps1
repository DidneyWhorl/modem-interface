# Build .ipk locally - Uses WSL to run the shell-based .ipk builder
param(
    [string]$Version = ""
)

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectRoot = Split-Path -Parent $ScriptDir

# Auto-detect version from package.json if not provided
if (-not $Version) {
    $packageJson = Get-Content (Join-Path $ProjectRoot "frontend\package.json") | ConvertFrom-Json
    $Version = $packageJson.version
}

Write-Host "=== Building .ipk Package (local) ===" -ForegroundColor Cyan
Write-Host "Version: $Version" -ForegroundColor Cyan

# Verify prerequisites
$BackendBinary = Join-Path $ProjectRoot "backend\target\aarch64-unknown-linux-musl\release\modem-interface"
$FrontendDist = Join-Path $ProjectRoot "frontend\dist"

if (-not (Test-Path $BackendBinary)) {
    Write-Host "[X] Backend binary not found: $BackendBinary" -ForegroundColor Red
    Write-Host "Run build-backend.ps1 first." -ForegroundColor Yellow
    exit 1
}
Write-Host "[OK] Backend binary found" -ForegroundColor Green

if (-not (Test-Path $FrontendDist)) {
    Write-Host "[X] Frontend dist not found: $FrontendDist" -ForegroundColor Red
    Write-Host "Run build-frontend.ps1 first." -ForegroundColor Yellow
    exit 1
}
Write-Host "[OK] Frontend dist found" -ForegroundColor Green

# Check for WSL
if (-not (Get-Command wsl -ErrorAction SilentlyContinue)) {
    Write-Host "[X] WSL not found. WSL is required to build .ipk on Windows." -ForegroundColor Red
    Write-Host "Install WSL: wsl --install" -ForegroundColor Yellow
    exit 1
}
Write-Host "[OK] WSL available" -ForegroundColor Green

# Ensure binutils is installed in WSL (provides ar)
Write-Host "`nEnsuring WSL has required tools (binutils)..." -ForegroundColor Yellow
wsl -- sh -c "which ar >/dev/null 2>&1 || (sudo apt-get update && sudo apt-get install -y binutils)"

# Convert Windows paths to WSL paths
$WslProjectRoot = wsl -- wslpath -u ($ProjectRoot -replace '\\', '/')
$WslBinary = wsl -- wslpath -u ($BackendBinary -replace '\\', '/')
$WslFrontendDist = wsl -- wslpath -u ($FrontendDist -replace '\\', '/')

Write-Host "`nRunning build-ipk.sh via WSL..." -ForegroundColor Yellow
wsl -- sh -c "cd '$WslProjectRoot' && chmod +x scripts/build-ipk.sh && ./scripts/build-ipk.sh '$Version' '$WslBinary' '$WslFrontendDist' '.'"

if ($LASTEXITCODE -ne 0) {
    Write-Host "[X] .ipk build failed" -ForegroundColor Red
    exit 1
}

# Find the generated .ipk
$IpkPattern = "modem-interface_${Version}-1_aarch64_cortex-a53.ipk"
$IpkPath = Join-Path $ProjectRoot $IpkPattern

if (Test-Path $IpkPath) {
    $ipkInfo = Get-Item $IpkPath
    Write-Host "`n=== .ipk Build Complete ===" -ForegroundColor Green
    Write-Host "Package: $IpkPattern" -ForegroundColor Cyan
    Write-Host ("Size: {0:N2} MB" -f ($ipkInfo.Length / 1MB)) -ForegroundColor Cyan
    Write-Host "`nTo deploy via opkg feed:" -ForegroundColor Yellow
    Write-Host "  .\scripts\deploy-to-router.ps1 -UseOpkg" -ForegroundColor Gray
    Write-Host "`nTo install directly on router:" -ForegroundColor Yellow
    Write-Host "  scp $IpkPattern root@192.168.1.1:/tmp/" -ForegroundColor Gray
    Write-Host "  ssh root@192.168.1.1 `"opkg install /tmp/$IpkPattern`"" -ForegroundColor Gray
} else {
    Write-Host "[X] Expected .ipk not found at: $IpkPath" -ForegroundColor Red
    exit 1
}
