#!/usr/bin/env pwsh
# Instalador de raylang para Windows (M165). El gemelo de install.sh: descarga el zip de la
# plataforma desde la GitHub Release, deja ray.exe (+ raylang.exe) en un directorio del usuario y
# lo añade al PATH de usuario (sin permisos de administrador).
#
#   irm https://raylang.dev/install.ps1 | iex
#
# Variables de entorno (opcionales, las mismas que install.sh):
#   RAYLANG_VERSION   tag a instalar (p. ej. v1.5.0). Por defecto: la última release.
#   RAYLANG_BIN_DIR   directorio de instalación. Por defecto: %LOCALAPPDATA%\Programs\raylang\bin
#   RAYLANG_REPO      owner/repo. Por defecto: ray-language/raylang
#   RAYLANG_DRY_RUN   si está definida, imprime el plan y NO descarga (para probar la detección).
#
# Requisitos: Windows 10+ con PowerShell 5.1 o pwsh 7 (Expand-Archive y TLS 1.2 vienen de serie).
$ErrorActionPreference = 'Stop'

$Repo = if ($env:RAYLANG_REPO) { $env:RAYLANG_REPO } else { 'ray-language/raylang' }
$BinDir = if ($env:RAYLANG_BIN_DIR) { $env:RAYLANG_BIN_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\raylang\bin' }

function Info($msg) { Write-Host "-> $msg" -ForegroundColor Blue }
# `throw` y no `exit`: bajo `irm | iex` un `exit` cerraría la sesión entera del usuario.
function Fail($msg) { throw "error: $msg" }

# --- Detectar plataforma -> target triple de Rust ---
# Solo se publica x86_64; Windows 11 en ARM lo ejecuta por emulación, así que se instala con aviso.
$osArch = try { [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString() } catch { $env:PROCESSOR_ARCHITECTURE }
switch -Regex ($osArch) {
    '^(X64|AMD64)$' { $cpu = 'x86_64' }
    '^(Arm64|ARM64)$' {
        $cpu = 'x86_64'
        Write-Host "nota: no hay build nativa para Windows ARM64 todavia; se instala la x86_64 (corre por emulacion)." -ForegroundColor Yellow
    }
    default { Fail "arquitectura no soportada: $osArch" }
}
$target = "$cpu-pc-windows-msvc"
$asset = "raylang-$target.zip"

# --- Resolver la URL de descarga ---
if ($env:RAYLANG_VERSION) {
    $url = "https://github.com/$Repo/releases/download/$($env:RAYLANG_VERSION)/$asset"
    $version = $env:RAYLANG_VERSION
} else {
    $url = "https://github.com/$Repo/releases/latest/download/$asset"
    $version = 'latest'
}

Info "raylang - $target - $version"
Info "asset:   $asset"
Info "destino: $BinDir"

if ($env:RAYLANG_DRY_RUN) {
    Info "DRY RUN - url: $url"
    return
}

# --- Descargar y extraer ---
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("raylang-install-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    # PowerShell 5.1 negocia TLS 1.0 por defecto; GitHub exige 1.2.
    try { [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12 } catch { }
    Info "descargando..."
    $zip = Join-Path $tmp $asset
    try {
        Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
    } catch {
        Fail "no se pudo descargar $url`n       existe una Release con ese asset? Mira https://github.com/$Repo/releases"
    }
    # Quitar la marca de origen web del zip: si no, los .exe extraidos heredan el aviso de SmartScreen.
    Unblock-File -Path $zip -ErrorAction SilentlyContinue
    Info "extrayendo..."
    Expand-Archive -Path $zip -DestinationPath $tmp -Force

    # --- Instalar ---
    New-Item -ItemType Directory -Path $BinDir -Force | Out-Null
    foreach ($bin in @('ray.exe', 'raylang.exe')) {
        $src = Join-Path $tmp $bin
        if (-not (Test-Path $src)) { Fail "el paquete no contiene '$bin'" }
        $dest = Join-Path $BinDir $bin
        # Un .exe en ejecucion no se puede sobrescribir en Windows, pero si RENOMBRAR: se aparta
        # a .old, se coloca el nuevo y el .old se borra (si sigue en uso, en la proxima instalacion).
        $old = "$dest.old"
        Remove-Item -Path $old -Force -ErrorAction SilentlyContinue
        if (Test-Path $dest) { Move-Item -Path $dest -Destination $old -Force }
        Move-Item -Path $src -Destination $dest -Force
        Remove-Item -Path $old -Force -ErrorAction SilentlyContinue
    }
} finally {
    Remove-Item -Path $tmp -Recurse -Force -ErrorAction SilentlyContinue
}

Info "instalado: $BinDir\ray.exe  (+ raylang.exe)"
& (Join-Path $BinDir 'ray.exe') version

# --- PATH de usuario (HKCU, sin administrador) ---
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$entries = if ($userPath) { $userPath -split ';' } else { @() }
if ($entries -notcontains $BinDir) {
    $newPath = if ($userPath) { "$BinDir;$userPath" } else { $BinDir }
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
    $env:Path = "$BinDir;$env:Path"
    Write-Host "nota: $BinDir se anadio a tu PATH de usuario; abre una terminal nueva para que aplique." -ForegroundColor Yellow
}
