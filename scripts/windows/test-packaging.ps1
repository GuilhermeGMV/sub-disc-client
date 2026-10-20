# Unit/regression tests of the dependency validator; runs on PowerShell 7/Linux
# as well as Windows PowerShell 5.1. Does not claim Windows runtime validation.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')
$work = Join-Path ([IO.Path]::GetTempPath()) ('subdisc-pe-test-' + [guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $work
function Assert-True([bool]$Value, [string]$Message) { if (-not $Value) { throw $Message } }
function Must-Fail([scriptblock]$Action) {
    $failed = $false
    try { & $Action | Out-Null } catch { $failed = $true }
    Assert-True $failed "Expected rejection did not occur: {$Action}"
}
function Set-U16([byte[]]$Bytes, [int]$Offset, [uint16]$Value) { [BitConverter]::GetBytes($Value).CopyTo($Bytes, $Offset) }
function Set-U32([byte[]]$Bytes, [int]$Offset, [uint32]$Value) { [BitConverter]::GetBytes($Value).CopyTo($Bytes, $Offset) }
function New-Pe([string]$Path, [string]$Import = 'kernel32.dll', [string]$Delay = 'vcruntime140.dll') {
    $b = New-Object byte[] 2048
    Set-U16 $b 0 0x5a4d; Set-U32 $b 0x3c 0x80
    Set-U32 $b 0x80 0x4550; Set-U16 $b 0x84 0x8664; Set-U16 $b 0x86 1
    Set-U16 $b 0x94 240; Set-U16 $b 0x98 0x20b; $b[0x9a] = 14; $b[0x9b] = 44
    Set-U32 $b (0x98+60) 0x200; Set-U32 $b (0x98+108) 16
    Set-U32 $b (0x98+120) 0x1000; Set-U32 $b (0x98+124) 40
    Set-U32 $b (0x98+216) 0x1080; Set-U32 $b (0x98+220) 64
    Set-U32 $b (0x188+8) 0x600; Set-U32 $b (0x188+12) 0x1000
    Set-U32 $b (0x188+16) 0x600; Set-U32 $b (0x188+20) 0x200
    Set-U32 $b (0x200+12) 0x1100
    Set-U32 $b 0x280 1; Set-U32 $b 0x284 0x1140
    [Text.Encoding]::ASCII.GetBytes($Import).CopyTo($b, 0x300)
    [Text.Encoding]::ASCII.GetBytes($Delay).CopyTo($b, 0x340)
    [IO.File]::WriteAllBytes($Path, $b)
}
try {
    $app = Join-Path $work 'subdisc-client.exe'
    New-Pe $app
    $imports = @(Get-PeImports $app)
    Assert-True ($imports.Count -eq 2 -and 'kernel32.dll' -in $imports -and 'vcruntime140.dll' -in $imports) 'Normal/delay import parsing failed'
    Assert-True (Test-SystemDll 'KERNEL32.dll') 'Inbox DLL rejected'
    Assert-True (Test-SystemDll 'api-ms-win-core-file-l1-1-0.dll') 'API set rejected'
    Assert-True (-not (Test-SystemDll 'vcruntime140.dll')) 'VC runtime incorrectly treated as inbox'
    Assert-True (-not (Test-SystemDll 'libcrypto-3-x64.dll')) 'Third-party library incorrectly treated as inbox'
    Must-Fail { Assert-BundleDependencies $work }
    $crt = Join-Path $work 'vcruntime140.dll'
    New-Pe $crt 'kernel32.dll' 'advapi32.dll'
    $edges = @(Assert-BundleDependencies $work)
    Assert-True ($edges.Count -eq 4) 'Transitive closure graph failed'
    New-Pe $crt 'missing-transitive.dll' 'kernel32.dll'
    Must-Fail { Assert-BundleDependencies $work }
    New-Pe $app '../outside.dll'
    Must-Fail { Get-PeImports $app }
    New-Pe $app
    $b = [IO.File]::ReadAllBytes($app); Set-U16 $b 0x84 0x14c; [IO.File]::WriteAllBytes($app,$b)
    Must-Fail { Get-PeImports $app }
    New-Pe $app
    $b = [IO.File]::ReadAllBytes($app); Set-U32 $b 0x284 4294967280; [IO.File]::WriteAllBytes($app,$b)
    Must-Fail { Get-PeImports $app }
    [IO.File]::WriteAllBytes($app, [byte[]]@(0x4d,0x5a))
    Must-Fail { Get-PeImports $app }
    Must-Fail { Get-RelativePath $work (Split-Path -Parent $work) }
    New-Pe $app
    Assert-True ((Get-PeLinkerVersion $app) -eq [version]'14.44') 'Linker version parsing failed'
    # Closure and missing-dependency diagnostics name the DLL and its importer.
    New-Pe $app 'kernel32.dll' 'vcruntime140.dll'
    New-Pe $crt 'kernel32.dll' 'advapi32.dll'
    $closure = @(Get-ImportClosure $work $app)
    Assert-True ($closure.Count -eq 1 -and $closure[0] -eq 'vcruntime140.dll') 'Import closure failed'
    Remove-Item -LiteralPath $crt
    $message = ''
    try { $null = Assert-BundleDependencies $work } catch { $message = $_.Exception.Message }
    Assert-True ($message -match 'vcruntime140\.dll' -and $message -match 'subdisc-client\.exe') "Missing-DLL diagnostic: $message"

    # Windows command-line quoting used for every native call.
    $cases = [ordered]@{ 'plain' = 'plain'; 'a b' = '"a b"'; 'x"y' = '"x\"y"'; 'C:\dir\' = 'C:\dir\'; 'C:\my dir\' = '"C:\my dir\\"'; '' = '""' }
    foreach ($case in $cases.GetEnumerator()) {
        $actual = ConvertTo-ArgumentString @($case.Key)
        Assert-True ($actual -ceq $case.Value) "Quoting of [$($case.Key)]: $actual"
    }
    # Explicit exit-code and timeout policy for native programs.
    $shell = (Get-Process -Id $PID).Path
    Must-Fail { Invoke-Native $shell @('-NoProfile','-Command','exit 3') -TimeoutSeconds 60 }
    Assert-True ((Invoke-Native $shell @('-NoProfile','-Command','exit 3') -AllowedExitCodes @(3) -TimeoutSeconds 60).ExitCode -eq 3) 'Allowed exit code rejected'
    Assert-True ((Invoke-Native $shell @('-NoProfile','-Command','exit 4') -IgnoreExitCode -TimeoutSeconds 60).ExitCode -eq 4) 'IgnoreExitCode failed'
    $echo = Invoke-Native $shell @('-NoProfile','-Command','[Console]::OutputEncoding = New-Object Text.UTF8Encoding $false; [Console]::Out.Write($env:SUBDISC_PROBE + [char]0x00E7)') -Environment @{ SUBDISC_PROBE = 'a b' } -TimeoutSeconds 60
    Assert-True ($echo.Output -ceq ('a b' + [char]0x00E7)) "Child environment/UTF-8 output: $($echo.Output)"
    Must-Fail { Invoke-Native $shell @('-NoProfile','-Command','Start-Sleep 30') -TimeoutSeconds 2 }

    # The caller's environment is restored exactly.
    $env:SUBDISC_KEEP = 'original'
    $path = [Environment]::GetEnvironmentVariable('PATH', 'Process')
    $saved = Save-ProcessEnvironment
    $env:SUBDISC_KEEP = 'changed'; $env:SUBDISC_NEW = 'x'; [Environment]::SetEnvironmentVariable('PATH', $null, 'Process')
    Restore-ProcessEnvironment $saved
    $keep = [Environment]::GetEnvironmentVariable('SUBDISC_KEEP', 'Process')
    $added = [Environment]::GetEnvironmentVariable('SUBDISC_NEW', 'Process')
    $restored = [Environment]::GetEnvironmentVariable('PATH', 'Process')
    Assert-True ($keep -ceq 'original') "Environment restore: SUBDISC_KEEP is '$keep'"
    Assert-True ($null -eq $added) "Environment restore: SUBDISC_NEW still set to '$added'"
    Assert-True ($restored -ceq $path) ("Environment restore: PATH differs (before {0} chars, after {1})" -f "$path".Length, "$restored".Length)
    Remove-Item Env:SUBDISC_KEEP

    # ZIP: one top-level folder, '/' separators, byte-identical extraction, and
    # rejection of archives with entries outside that folder.
    $tree = Join-Path $work 'zip/Subdisc-portable'
    $null = New-Item -ItemType Directory -Force -Path (Join-Path $tree 'lib/gstreamer-1.0')
    [IO.File]::WriteAllBytes((Join-Path $tree 'lib/gstreamer-1.0/gstx.dll'), [byte[]](1..200))
    Write-Utf8 (Join-Path $tree ('README ' + [char]0x00E7 + '.txt')) 'texto'
    $zip = Join-Path $work 'zip/out.zip'
    New-PortableZip $tree $zip
    $archive = [IO.Compression.ZipFile]::OpenRead($zip)
    try { $names = @($archive.Entries | ForEach-Object FullName) } finally { $archive.Dispose() }
    Assert-True ($names.Count -eq 2 -and @($names | Where-Object { $_ -notlike 'Subdisc-portable/*' -or $_.Contains('\') }).Count -eq 0) "ZIP entries: $names"
    $out = Join-Path $work ('zip/extracted ' + [char]0x4E2D)
    Expand-PortableZip $zip $out
    foreach ($file in Get-ChildItem -LiteralPath $tree -Recurse -File) {
        $copy = Join-Path (Join-Path $out 'Subdisc-portable') (Get-RelativePath $tree $file.FullName)
        Assert-True ((Get-FileHash -LiteralPath $copy).Hash -eq (Get-FileHash -LiteralPath $file.FullName).Hash) "ZIP roundtrip: $($file.Name)"
    }
    $evil = Join-Path $work 'zip/evil.zip'
    $stream = [IO.File]::Open($evil, [IO.FileMode]::CreateNew)
    $archive = New-Object IO.Compression.ZipArchive($stream, [IO.Compression.ZipArchiveMode]::Create)
    $entry = $archive.CreateEntry('Subdisc-portable/../evil.dll'); $writer = $entry.Open(); $writer.WriteByte(1); $writer.Dispose()
    $archive.Dispose(); $stream.Dispose()
    Must-Fail { Expand-PortableZip $evil (Join-Path $work 'zip/evil-out') }
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $work 'zip/evil.dll'))) 'Escaping ZIP entry was extracted'
    Write-Host 'PASS: normal/delay imports, x64, malformed PE, linker version, path traversal, missing/transitive DLLs, import closure, inbox classification, native exit codes/timeouts/quoting, environment restore, ZIP layout/roundtrip/escape rejection.'
} finally { Remove-Item -LiteralPath $work -Recurse -Force }
