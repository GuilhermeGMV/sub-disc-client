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
# Os headers do libwebrtc ficam em caminhos profundos dentro de target\.
if ((Get-Location).Path.Length -gt 40) {
    Write-Warning 'Caminho do projeto longo: se o build falhar com "path too long", mova-o para algo como C:\src\subdisc-client ou habilite caminhos longos no Windows.'
}

cargo build --release --locked
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item target\release\subdisc-client.exe dist\subdisc-client.exe -Force
Write-Host 'Pronto: dist\subdisc-client.exe'
