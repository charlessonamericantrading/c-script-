# Script de instalación para c-script en Windows (PowerShell)

$ErrorActionPreference = "Stop"

Write-Host "==> Compilando linkc en modo Release..." -ForegroundColor Cyan
Push-Location "$PSScriptRoot\compiler"
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) {
        throw "La compilación con cargo build --release falló con código $LASTEXITCODE."
    }
} finally {
    Pop-Location
}

$BinarySource = Join-Path $PSScriptRoot "compiler\target\release\linkc.exe"
if (!(Test-Path $BinarySource)) {
    throw "El ejecutable $BinarySource no existe tras la compilación."
}

$InstallDir = Join-Path $env:USERPROFILE ".c-script\bin"
if (!(Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
}

$BinaryTarget = Join-Path $InstallDir "linkc.exe"


Write-Host "==> Instalando linkc.exe en $InstallDir..." -ForegroundColor Cyan
Copy-Item -Path $BinarySource -Destination $BinaryTarget -Force

# Agregar al PATH de la sesión actual si no está presente
if ($env:PATH -notlike "*$InstallDir*") {
    $env:PATH = "$InstallDir;$env:PATH"
    Write-Host "==> Agregado $InstallDir al PATH de esta sesión." -ForegroundColor Yellow
}

Write-Host ""
Write-Host "=========================================================" -ForegroundColor Green
Write-Host " ¡c-script instalado con éxito!" -ForegroundColor Green
Write-Host " Ejecuta 'linkc' o 'linkc --help' para comenzar." -ForegroundColor Green
Write-Host "=========================================================" -ForegroundColor Green
