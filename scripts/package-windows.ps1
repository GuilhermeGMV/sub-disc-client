# Assemble in a temporary directory; promote only after dependency/media checks
# on both the staging tree and the extracted ZIP. Any failure leaves no artifact.
[CmdletBinding()]
param(
    [string]$GStreamerRoot = $env:GSTREAMER_1_0_ROOT_MSVC_X86_64,
    [string]$Executable = '',
    [string]$CrtDirectory = '',
    [Collections.IDictionary]$BuildInfo = $null
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'windows/common.ps1')
Assert-WindowsX64
$repo = Split-Path -Parent $PSScriptRoot
$dist = Join-Path $repo 'dist'
$null = New-Item -ItemType Directory -Force -Path $dist
$failureReport = Join-Path $dist 'portable-failure.json'
$saved = Save-ProcessEnvironment
$stage = Join-Path $dist ('.portable-' + [guid]::NewGuid().ToString('N'))
$crtPattern = '^(msvcp140(_1|_2|_atomic_wait|_codecvt_ids)?|vcruntime140(_1|_threads)?|concrt140|vccorlib140)\.dll$'
try {
    Remove-PortableArtifacts $dist
    if (Test-Path -LiteralPath $failureReport) { Remove-Item -LiteralPath $failureReport -Force }
    if (-not $Executable) { $Executable = Join-Path $repo 'target/x86_64-pc-windows-msvc/release/subdisc-client.exe' }
    Assert-File $Executable
    $GStreamerRoot = Find-GStreamer $GStreamerRoot
    $version = Get-GStreamerVersion $GStreamerRoot
    $spec = Read-Json (Join-Path $PSScriptRoot 'windows/runtime.json')
    if (-not $CrtDirectory -or -not $BuildInfo) {
        $msvc = Initialize-Msvc
        if (-not $BuildInfo) { $BuildInfo = [ordered]@{ msvc_tools = $msvc.msvc_tools; windows_sdk = $msvc.windows_sdk; compiler = $msvc.compiler } }
    }
    if (-not $CrtDirectory) {
        $CrtDirectory = Get-ChildItem -LiteralPath (Join-Path $env:VCToolsRedistDir 'x64') -Directory |
            Where-Object Name -Match '^Microsoft\.VC14[0-9]\.CRT$' | Sort-Object Name -Descending | Select-Object -First 1 -ExpandProperty FullName
    }
    if (-not $CrtDirectory) { throw 'MSVC x64 release redistributable directory (VC\Redist\MSVC\<version>\x64\Microsoft.VC14x.CRT) missing.' }
    # Only the reviewed release redistributables, and only when actually imported.
    $crt = @{}
    foreach ($file in Get-ChildItem -LiteralPath $CrtDirectory -Filter '*.dll' -File) {
        if ($file.Name -match $crtPattern) { $crt[$file.Name.ToLowerInvariant()] = $file.FullName }
    }
    if (-not $crt.ContainsKey('vcruntime140.dll')) { throw "vcruntime140.dll missing in $CrtDirectory" }

    $bundle = Join-Path $stage 'Subdisc-portable'
    $zipTemp = Join-Path $stage 'Subdisc-Windows-x64-portable.zip'
    $null = New-Item -ItemType Directory -Force -Path (Join-Path $bundle 'lib/gstreamer-1.0')
    $origins = @{}
    $queue = New-Object 'Collections.Generic.Queue[string]'
    function Copy-RuntimeFile([string]$Source, [string]$Relative, [string]$Origin) {
        Assert-File $Source
        $dest = Join-Path $bundle $Relative
        if (Test-Path -LiteralPath $dest) { return }
        $null = New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dest)
        Copy-Item -LiteralPath $Source -Destination $dest
        $origins[$Relative.Replace('\','/')] = $Origin
        if ([IO.Path]::GetExtension($dest) -in '.dll','.exe') { $queue.Enqueue($dest) }
    }

    Copy-RuntimeFile $Executable 'subdisc-client.exe' 'subdisc'
    # The scanner runs as a separate process; beside the client its application
    # directory is the bundle root, so its own imports resolve without PATH.
    Copy-RuntimeFile (Join-Path $GStreamerRoot 'libexec/gstreamer-1.0/gst-plugin-scanner.exe') 'gst-plugin-scanner.exe' 'gstreamer'
    foreach ($plugin in $spec.plugins) {
        Copy-RuntimeFile (Join-Path $GStreamerRoot "lib/gstreamer-1.0/gst$($plugin.name).dll") "lib/gstreamer-1.0/gst$($plugin.name).dll" 'gstreamer'
    }
    # Loaded with LoadLibrary at runtime, so PE imports alone cannot find them.
    # Keep their layout and scan their imports too; never copy SDK files.
    # GIO: only the TLS backend (GTlsConnection, e.g. for libnice TURN over TLS).
    # Proxy-resolver modules (giolibproxy -> libproxy -> pxbackend) are unused:
    # HTTP/WebSocket are Rust code and libnice has its own proxy settings.
    $modules = [ordered]@{ 'lib/gio/modules' = '^(lib)?gio(openssl|gnutls)\.dll$'; 'lib/ossl-modules' = '\.dll$' }
    foreach ($module in $modules.GetEnumerator()) {
        $source = Join-Path $GStreamerRoot $module.Key
        if (Test-Path -LiteralPath $source) {
            foreach ($file in Get-ChildItem -LiteralPath $source -Filter '*.dll' -File | Where-Object Name -Match $module.Value) {
                Copy-RuntimeFile $file.FullName "$($module.Key)/$($file.Name)" 'gstreamer'
            }
        }
    }
    Copy-RuntimeFile (Join-Path $GStreamerRoot 'etc/ssl/certs/ca-certificates.crt') 'etc/ssl/certs/ca-certificates.crt' 'gstreamer'
    while ($queue.Count -gt 0) {
        $binary = $queue.Dequeue()
        foreach ($dependency in Get-PeImports $binary) {
            if ((Test-SystemDll $dependency) -or (Test-Path -LiteralPath (Join-Path $bundle $dependency))) { continue }
            $requiredBy = Get-RelativePath $bundle $binary
            if ($dependency -match '^(msvcp|vcruntime|concrt|vccorlib)') {
                if (-not $crt.ContainsKey($dependency)) {
                    throw "MSVC redistributable $dependency (required by $requiredBy) is not in $CrtDirectory. Never copied from System32."
                }
                Copy-RuntimeFile $crt[$dependency] $dependency 'msvc-redist'
                continue
            }
            $source = Join-Path $GStreamerRoot "bin/$dependency"
            if (-not (Test-Path -LiteralPath $source)) { throw "Unresolved dependency: $dependency (required by $requiredBy). Not searched in PATH/System32." }
            Copy-RuntimeFile $source $dependency 'gstreamer'
        }
    }
    $edges = @(Assert-BundleDependencies $bundle)

    # The app-local CRT must be at least as new as the newest MSVC toolset that
    # linked any bundled component (e.g. GStreamer built with a newer VS).
    $crtFiles = @($origins.GetEnumerator() | Where-Object Value -eq 'msvc-redist' | ForEach-Object { Join-Path $bundle $_.Key })
    $crtVersions = @($crtFiles | ForEach-Object { $info = [Diagnostics.FileVersionInfo]::GetVersionInfo($_); New-Object Version($info.FileMajorPart, $info.FileMinorPart, $info.FileBuildPart, $info.FilePrivatePart) } | Sort-Object -Unique)
    if ($crtVersions.Count -ne 1) { throw "Mixed MSVC redistributable versions: $($crtVersions -join ', ')" }
    $crtVersion = $crtVersions[0]
    $newest = $null
    foreach ($file in Get-ChildItem -LiteralPath $bundle -Recurse -File | Where-Object { $_.Extension -in '.dll','.exe' -and $_.Name -notmatch $crtPattern }) {
        $linker = Get-PeLinkerVersion $file.FullName
        if ($linker.Major -eq 14 -and (-not $newest -or $linker -gt $newest.Version)) { $newest = [pscustomobject]@{ Version = $linker; File = $file.Name } }
    }
    if ($newest -and (New-Object Version($crtVersion.Major, $crtVersion.Minor)) -lt $newest.Version) {
        throw "MSVC redistributable $crtVersion is older than toolset $($newest.Version) used by $($newest.File). Update Visual Studio Build Tools."
    }

    # The selected pipelines need no schemas/fonts/typelibs. OpenSSL uses an
    # explicit relocatable default-provider config; no host openssl.cnf is read.
    $null = New-Item -ItemType Directory -Force -Path (Join-Path $bundle 'share/openssl')
    Write-Utf8 (Join-Path $bundle 'share/openssl/openssl.cnf') "# Subdisc uses the OpenSSL built-in default provider.`n"
    & (Join-Path $PSScriptRoot 'windows/licenses.ps1') -Bundle $bundle -GStreamerRoot $GStreamerRoot -Origins $origins

    $appVersion = (Select-String -LiteralPath (Join-Path $repo 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
    $commit = 'unknown'; $dirty = $null
    if (Get-Command git -CommandType Application -ErrorAction SilentlyContinue) {
        try {
            $commit = (Invoke-Native git @('rev-parse','HEAD') -WorkingDirectory $repo -TimeoutSeconds 60).Output.Trim()
            $dirty = [bool](Invoke-Native git @('status','--porcelain','--untracked-files=no') -WorkingDirectory $repo -TimeoutSeconds 60).Output.Trim()
        } catch { $commit = 'unknown' }
    }
    $build = [ordered]@{ app_version = $appVersion; git_commit = $commit; git_dirty = $dirty; generated_utc = [DateTime]::UtcNow.ToString('o') }
    foreach ($entry in $BuildInfo.GetEnumerator()) { $build[$entry.Key] = $entry.Value }
    $spec | Add-Member -NotePropertyName gstreamer_version -NotePropertyValue $version
    $spec | Add-Member -NotePropertyName crt_version -NotePropertyValue $crtVersion.ToString()
    $spec | Add-Member -NotePropertyName newest_linker -NotePropertyValue $(if ($newest) { "$($newest.Version) ($($newest.File))" } else { $null })
    $spec | Add-Member -NotePropertyName build -NotePropertyValue $build
    $spec | Add-Member -NotePropertyName dependencies -NotePropertyValue $edges
    Write-Utf8 (Join-Path $bundle 'portable-runtime.json') ($spec | ConvertTo-Json -Depth 12)
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'windows/README-portable.txt') -Destination (Join-Path $bundle 'README.txt')
    $files = @(Get-ChildItem -LiteralPath $bundle -Recurse -File | Sort-Object { Get-RelativePath $bundle $_.FullName } | ForEach-Object {
        $relative = Get-RelativePath $bundle $_.FullName
        $fileVersion = $null
        if ($_.Extension -in '.dll','.exe') {
            $info = [Diagnostics.FileVersionInfo]::GetVersionInfo($_.FullName)
            if ($info.FileVersion) { $fileVersion = $info.FileVersion.Trim() }
        }
        $origin = if ($origins.ContainsKey($relative)) { $origins[$relative] } elseif ($relative -like 'licenses/*') { 'license-notice' } else { 'generated' }
        [ordered]@{path=$relative; bytes=$_.Length; sha256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash; version=$fileVersion; origin=$origin}
    })
    Write-Utf8 (Join-Path $bundle 'files.json') (ConvertTo-Json -InputObject $files -Depth 4)

    # Headless tests are mandatory; GUI tests are explicit (-Gui) because build
    # machines may have no interactive desktop.
    & (Join-Path $PSScriptRoot 'test-portable-windows.ps1') -BundlePath $bundle -ReportPath (Join-Path $stage 'validation.json') -GStreamerRoot $GStreamerRoot
    New-PortableZip $bundle $zipTemp
    # Validate the extracted archive, not only the staging tree.
    $extracted = Join-Path $stage 'zip extracted'
    Expand-PortableZip $zipTemp $extracted
    & (Join-Path $PSScriptRoot 'test-portable-windows.ps1') -BundlePath (Join-Path $extracted 'Subdisc-portable') -ReportPath (Join-Path $stage 'zip-validation.json') -GStreamerRoot $GStreamerRoot
    $final = Join-Path $dist 'Subdisc-portable'
    $zip = Join-Path $dist 'Subdisc-Windows-x64-portable.zip'
    Move-Item -LiteralPath $bundle -Destination $final
    Move-Item -LiteralPath (Join-Path $stage 'zip-validation.json') -Destination (Join-Path $dist 'portable-validation.json')
    # The ZIP is promoted last: its presence implies every step above passed.
    Move-Item -LiteralPath $zipTemp -Destination $zip
    $bytes = (Get-ChildItem -LiteralPath $final -Recurse -File | Measure-Object Length -Sum).Sum
    Write-Host ("Portable folder: {0} ({1:N2} MiB, {2} files)" -f $final, ($bytes / 1MB), $files.Count + 1)
    Write-Host ("ZIP: {0} ({1:N2} MiB)" -f $zip, ((Get-Item -LiteralPath $zip).Length / 1MB))
    Write-Host "Executable: $final\subdisc-client.exe"
} catch {
    $message = $_.Exception.Message
    Remove-PortableArtifacts $dist
    Write-Utf8 $failureReport ([ordered]@{ok=$false; utc=[DateTime]::UtcNow.ToString('o'); error=$message} | ConvertTo-Json -Depth 4)
    throw
} finally {
    if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
    Restore-ProcessEnvironment $saved
}
