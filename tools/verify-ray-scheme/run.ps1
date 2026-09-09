# Verificación del esquema ray:// en Windows (WebView2) con una ventana real. Desde la raíz del repo:
#   powershell -ExecutionPolicy Bypass -File tools\verify-ray-scheme\run.ps1
# Requiere el WebView2 Runtime (>= 112) y Rust (rustup) instalados.
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..\..")
$kit = Join-Path (Get-Location) "tools\verify-ray-scheme"
$big = Join-Path $kit "big.bin"
if (-not (Test-Path $big)) {
    $bytes = New-Object byte[] 8388608
    (New-Object System.Random).NextBytes($bytes)
    [System.IO.File]::WriteAllBytes($big, $bytes)
}
cargo build --release --quiet
Write-Host "== toolchain: $(& .\target\release\ray.exe version)"
$rt = Get-ItemProperty "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" -ErrorAction SilentlyContinue
Write-Host "== WebView2 runtime: $($rt.pv)"
& .\target\release\ray.exe run "$kit\probe.ray" "$kit"
exit $LASTEXITCODE
