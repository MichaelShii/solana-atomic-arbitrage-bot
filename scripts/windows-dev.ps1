# MEVBot Windows local build environment initialization script
# Usage (in PowerShell):
#   . .\scripts\windows-dev.ps1
#   cargo check
#   cargo test -- --nocapture
#
# Prerequisites (installed via winget):
#   - Microsoft.VisualStudio.2022.BuildTools (with VC++ toolchain)
#   - ShiningLight.OpenSSL.Dev
#   - Google.Protobuf (protoc)

$ErrorActionPreference = 'Stop'

# --- OpenSSL (Shining Light, MSVC dynamic libraries) ---
$opensslRoot = 'C:\Program Files\OpenSSL-Win64'
if (-not (Test-Path $opensslRoot)) {
    Write-Warning "OpenSSL not found: $opensslRoot — run: winget install -e --id ShiningLight.OpenSSL.Dev"
} else {
    $env:OPENSSL_DIR = $opensslRoot
    $env:OPENSSL_LIB_DIR = Join-Path $opensslRoot 'lib\VC\x64\MD'
    Write-Host "[OK] OPENSSL_DIR=$env:OPENSSL_DIR"
}

# --- protoc ---
$protocCandidates = @(
    (Get-Command protoc -ErrorAction SilentlyContinue)?.Source
    "$env:LOCALAPPDATA\Microsoft\WinGet\Packages\Google.Protobuf_Microsoft.Winget.Source_8wekyb3d8bbwe\bin\protoc.exe"
) | Where-Object { $_ -and (Test-Path $_) }

if ($protocCandidates.Count -eq 0) {
    Write-Warning "protoc not found — run: winget install -e --id Google.Protobuf"
} else {
    $env:PROTOC = $protocCandidates[0]
    Write-Host "[OK] PROTOC=$env:PROTOC"
}

# --- MSVC toolchain (link.exe / cl.exe) ---
$vcvars = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat'
if (-not (Test-Path $vcvars)) {
    Write-Warning "vcvars64.bat not found — run: winget install -e --id Microsoft.VisualStudio.2022.BuildTools"
    Write-Warning "Select the 'Desktop development with C++' workload during install"
} else {
    # Inject MSVC toolchain into current PowerShell session
    $envDump = cmd.exe /c "`"$vcvars`" >nul && set" 2>$null
    foreach ($line in $envDump) {
        if ($line -match '^(.*?)=(.*)$') {
            Set-Item -Path "env:$($matches[1])" -Value $matches[2]
        }
    }
    Write-Host "[OK] MSVC dev environment loaded (vcvars64)"
}

Write-Host ""
Write-Host "Environment variables set. Notes:"
Write-Host "  - This script only affects the current PowerShell session"
Write-Host "  - yellowstone-grpc-client may not compile on native Windows (depends on UnixStream)"
Write-Host "  - For full builds, use WSL2/Linux, or use WebSocket-only mode with conditional gRPC compilation"
Write-Host ""
