# Build Frontend - React + Vite production bundle

$ErrorActionPreference = "Stop"

Write-Host "=== Building React Frontend ===" -ForegroundColor Cyan

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectRoot = Split-Path -Parent $ScriptDir
$FrontendDir = Join-Path $ProjectRoot "frontend"

Push-Location $FrontendDir

try {
    # Check if node_modules exists
    if (-not (Test-Path "node_modules")) {
        Write-Host "node_modules not found. Installing dependencies..." -ForegroundColor Yellow
        npm ci
    } else {
        Write-Host "Installing dependencies..." -ForegroundColor Yellow
        npm ci
    }

    # Build production bundle
    Write-Host "`nBuilding production bundle..." -ForegroundColor Yellow
    npm run build

    $DistDir = Join-Path $FrontendDir "dist"

    Write-Host "`n=== Frontend Build Complete ===" -ForegroundColor Green
    Write-Host "Build output directory: dist/" -ForegroundColor Cyan

    if (Test-Path $DistDir) {
        $distSize = (Get-ChildItem $DistDir -Recurse | Measure-Object -Property Length -Sum).Sum
        Write-Host ("Total size: {0:N2} MB" -f ($distSize / 1MB)) -ForegroundColor Cyan

        Write-Host "`nBuild contents:" -ForegroundColor Yellow
        Get-ChildItem $DistDir | Format-Table Name, Length, LastWriteTime -AutoSize
    } else {
        Write-Host "[X] dist/ directory not found" -ForegroundColor Red
    }

} finally {
    Pop-Location
}
