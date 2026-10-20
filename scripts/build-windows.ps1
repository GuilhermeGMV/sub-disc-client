# Compila o cliente no Windows (x64) e copia o executável para dist\.
# Requisitos: Rust (rustup, toolchain MSVC) e Visual Studio Build Tools com a
# carga "Desenvolvimento para desktop com C++" (compilador MSVC + Windows SDK).
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw 'cargo não encontrado. Instale o Rust em https://rustup.rs (toolchain MSVC).'
}
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (-not (Test-Path $vswhere) -or -not (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)) {
    throw 'Compilador MSVC não encontrado. Instale o Visual Studio Build Tools com "Desenvolvimento para desktop com C++".'
}
# Instale os SDKs runtime e development do GStreamer MSVC x86_64 (Complete).
if (-not $env:GSTREAMER_1_0_ROOT_MSVC_X86_64) {
    throw 'Defina GSTREAMER_1_0_ROOT_MSVC_X86_64 para a instalação GStreamer MSVC x86_64.'
}
$env:Path = "$env:GSTREAMER_1_0_ROOT_MSVC_X86_64\bin;$env:Path"
$env:PKG_CONFIG_PATH = "$env:GSTREAMER_1_0_ROOT_MSVC_X86_64\lib\pkgconfig"

cargo build --release --locked
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item target\release\subdisc-client.exe dist\subdisc-client.exe -Force
Write-Host 'Pronto: dist\subdisc-client.exe. A instalação GStreamer também é necessária na máquina de destino.'
