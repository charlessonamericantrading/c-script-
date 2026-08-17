# Script de instalación universal para c-script en Windows (PowerShell)
# Uso: irm https://raw.githubusercontent.com/charlessonamericantrading/c-script-/master/install.ps1 | iex

$ErrorActionPreference = "Continue"

$Repo = "charlessonamericantrading/c-script-"
$InstallDir = Join-Path $env:USERPROFILE ".c-script\bin"

if (!(Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
}

$BinaryTarget = Join-Path $InstallDir "linkc.exe"
$Asset = "linkc-x86_64-pc-windows-msvc.zip"
$Url = "https://github.com/$Repo/releases/latest/download/$Asset"
$Installed = $false

Write-Host "⚡ Instalando c-script (linkc) para Windows x64..." -ForegroundColor Cyan

# 1. Intentar descargar release precompilado desde GitHub
try {
    $TempZip = Join-Path ([System.IO.Path]::GetTempPath()) "linkc_download.zip"
    $TempExtract = Join-Path ([System.IO.Path]::GetTempPath()) "linkc_extracted"
    
    Write-Host "==> Descargando binario oficial desde GitHub Releases..." -ForegroundColor Yellow
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest -Uri $Url -OutFile $TempZip -UseBasicParsing -TimeoutSec 30
    
    if (Test-Path $TempZip) {
        if (Test-Path $TempExtract) { Remove-Item -Path $TempExtract -Recurse -Force }
        Expand-Archive -Path $TempZip -DestinationPath $TempExtract -Force
        $DownloadedExe = Join-Path $TempExtract "linkc.exe"
        if (Test-Path $DownloadedExe) {
            Copy-Item -Path $DownloadedExe -Destination $BinaryTarget -Force
            $Installed = $true
        }
        Remove-Item -Path $TempZip -Force -ErrorAction SilentlyContinue
        Remove-Item -Path $TempExtract -Recurse -Force -ErrorAction SilentlyContinue
    }
} catch {
    Write-Host "==> No se pudo descargar el release precompilado ($($_.Exception.Message))." -ForegroundColor Gray
}

# 2. Si falló la descarga o se ejecuta desde el repo local, verificar compilación local
if (-not $Installed) {
    if (Test-Path "$PSScriptRoot\compiler\target\release\linkc.exe") {
        Copy-Item -Path "$PSScriptRoot\compiler\target\release\linkc.exe" -Destination $BinaryTarget -Force
        $Installed = $true
    } elseif (Get-Command cargo -ErrorAction SilentlyContinue) {
        Write-Host "==> Compilando desde el código fuente con cargo..." -ForegroundColor Cyan
        if (Test-Path "$PSScriptRoot\compiler") {
            Push-Location "$PSScriptRoot\compiler"
            cargo build --release
            Pop-Location
            if (Test-Path "$PSScriptRoot\compiler\target\release\linkc.exe") {
                Copy-Item -Path "$PSScriptRoot\compiler\target\release\linkc.exe" -Destination $BinaryTarget -Force
                $Installed = $true
            }
        }
    }
}

if (-not $Installed -and -not (Test-Path $BinaryTarget)) {
    Write-Host "❌ Error: no se pudo instalar linkc. Asegúrate de tener conexión a Internet o Rust/Cargo instalado." -ForegroundColor Red
    exit 1
}

# Agregar al PATH de usuario permanente si no está presente
$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($UserPath -notlike "*$InstallDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$UserPath;$InstallDir", "User")
    Write-Host "==> Se añadió $InstallDir al PATH permanente del usuario." -ForegroundColor Yellow
}

# Agregar a la sesión actual
if ($env:PATH -notlike "*$InstallDir*") {
    $env:PATH = "$InstallDir;$env:PATH"
}

Write-Host ""
Write-Host "=========================================================" -ForegroundColor Green
Write-Host " 🎉 ¡c-script instalado con éxito en $BinaryTarget!" -ForegroundColor Green
Write-Host " Ejecuta 'linkc' o 'linkc --help' para comenzar." -ForegroundColor Green
Write-Host "=========================================================" -ForegroundColor Green
