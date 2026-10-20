# Windows PowerShell 5.1 / PowerShell 7, x64. Build + package + validate + ZIP:
#   powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1
[CmdletBinding()]
param([string]$GStreamerRoot = $env:GSTREAMER_1_0_ROOT_MSVC_X86_64)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'windows/common.ps1')
Assert-WindowsX64
$repo = Split-Path -Parent $PSScriptRoot
$target = 'x86_64-pc-windows-msvc'
$saved = Save-ProcessEnvironment
Push-Location $repo
try {
    # A failed build must not leave an older ZIP looking like this build's result.
    Remove-PortableArtifacts (Join-Path $repo 'dist')
    foreach ($tool in 'cargo','rustc','rustup') {
        if (-not (Get-Command $tool -CommandType Application -ErrorAction SilentlyContinue)) { throw "Install Rust from https://rustup.rs (MSVC toolchain): missing $tool" }
    }
    $rustc = (Invoke-Native rustc @('-vV') -TimeoutSeconds 120).Output
    if ($rustc -notmatch '(?m)^release: ([0-9]+\.[0-9]+\.[0-9]+)' -or [version]$matches[1] -lt [version]'1.85.0') {
        throw 'Rust >= 1.85 (edition 2024) required: rustup update stable'
    }
    $rustVersion = $matches[1]
    if ($rustc -notmatch '(?m)^host: x86_64-pc-windows-msvc\s*$') {
        throw 'Use the x86_64-pc-windows-msvc Rust toolchain: rustup default stable-x86_64-pc-windows-msvc'
    }
    $targets = (Invoke-Native rustup @('target','list','--installed') -TimeoutSeconds 120).Output -split '\r?\n'
    if ($target -notin $targets) { throw "Run: rustup target add $target" }
    $msvc = Initialize-Msvc
    $GStreamerRoot = Find-GStreamer $GStreamerRoot
    $version = Get-GStreamerVersion $GStreamerRoot
    $env:GSTREAMER_1_0_ROOT_MSVC_X86_64 = $GStreamerRoot
    $env:PATH = "$GStreamerRoot\bin;$env:PATH"
    $env:PKG_CONFIG = Join-Path $GStreamerRoot 'bin\pkg-config.exe'
    $env:PKG_CONFIG_PATH = Join-Path $GStreamerRoot 'lib\pkgconfig'
    $env:PKG_CONFIG_LIBDIR = $env:PKG_CONFIG_PATH
    Remove-Item Env:PKG_CONFIG_SYSROOT_DIR -ErrorAction SilentlyContinue
    # Remap this checkout in panic/debug strings; keep the shared CRT ABI that
    # GStreamer uses. /DEPENDENTLOADFLAG limits the client's initial imports to
    # the application directory + System32 (no cwd/PATH).
    $flags = @()
    if ($env:CARGO_ENCODED_RUSTFLAGS) { $flags += $env:CARGO_ENCODED_RUSTFLAGS.Split([char]31) }
    elseif ($env:RUSTFLAGS) { throw 'Unset RUSTFLAGS or use CARGO_ENCODED_RUSTFLAGS; packaging must preserve paths with spaces.' }
    if (($flags -join ' ') -match 'crt-static') { throw 'Remove crt-static overrides: this build uses the GStreamer shared CRT.' }
    $env:CARGO_ENCODED_RUSTFLAGS = ($flags + @('--remap-path-prefix', "$repo=/subdisc", '-C', 'link-arg=/DEPENDENTLOADFLAG:0xA00')) -join [char]31
    Write-Host "Rust $rustVersion, MSVC $($msvc.msvc_tools), Windows SDK $($msvc.windows_sdk), GStreamer $version ($GStreamerRoot)"
    # An explicit target directory avoids picking up a stale Linux/x86 executable.
    & cargo build --release --locked --target $target --target-dir (Join-Path $repo 'target')
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }
    $exe = Join-Path $repo "target\$target\release\subdisc-client.exe"
    $null = Get-PeImports $exe
    $build = [ordered]@{ rust = $rustVersion; target = $target; msvc_tools = $msvc.msvc_tools; windows_sdk = $msvc.windows_sdk; compiler = $msvc.compiler }
    & (Join-Path $PSScriptRoot 'package-windows.ps1') -GStreamerRoot $GStreamerRoot -Executable $exe -BuildInfo $build
} finally {
    Pop-Location
    Restore-ProcessEnvironment $saved
}
