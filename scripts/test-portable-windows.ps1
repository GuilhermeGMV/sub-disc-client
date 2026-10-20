# Validates a portable bundle (folder or extracted ZIP). Needs no build tools or
# GStreamer: copy scripts\ (with windows\) next to the ZIP on the test machine.
#   powershell -ExecutionPolicy Bypass -File scripts\test-portable-windows.ps1 [-BundlePath <dir>] [-Gui]
# Every child runs with a sanitized PATH, no GStreamer variables, a private
# Unicode LOCALAPPDATA/APPDATA and a fresh registry; the caller's environment,
# error mode, ACLs and profile caches are left untouched.
[CmdletBinding()]
param(
    [string]$BundlePath = (Join-Path (Split-Path -Parent $PSScriptRoot) 'dist/Subdisc-portable'),
    [string]$ReportPath = (Join-Path (Split-Path -Parent $PSScriptRoot) 'dist/portable-validation.json'),
    # Optional global SDK used as hostile PATH/plugin path: proves it is never used.
    [string]$GStreamerRoot = $env:GSTREAMER_1_0_ROOT_MSVC_X86_64,
    [int]$TimeoutSeconds = 120,
    [switch]$Gui
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'windows/common.ps1')
Assert-WindowsX64
$BundlePath = (Resolve-Path -LiteralPath $BundlePath).Path
$ReportPath = [IO.Path]::GetFullPath($ReportPath)
if ($GStreamerRoot -and -not (Test-Path -LiteralPath (Join-Path $GStreamerRoot 'bin\gstreamer-1.0-0.dll'))) { $GStreamerRoot = $null }
$unicode = [string][char]0x00FC + ' ' + [char]0x4E2D + ' ' + [char]0x00E7
$work = Join-Path ([IO.Path]::GetTempPath()) ("Subdisc test $unicode " + [guid]::NewGuid().ToString('N').Substring(0, 8))
$null = New-Item -ItemType Directory -Path $work
$profileRoot = Join-Path $work "perfil $unicode"
$localAppData = Join-Path $profileRoot 'Local'
$appData = Join-Path $profileRoot 'Roaming'
$null = New-Item -ItemType Directory -Force -Path $localAppData, $appData
$checks = New-Object 'Collections.Generic.List[string]'
$results = [ordered]@{}
$success = $false
$failure = $null
$denied = $null
$sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$previousErrorMode = [SubdiscErrorMode]::SetErrorMode([SubdiscErrorMode]::Quiet)

function Assert-Inventory([string]$Root) {
    foreach ($file in 'subdisc-client.exe','gst-plugin-scanner.exe','portable-runtime.json','files.json',
        'licenses/NOTICE.md','licenses/native-components.json','licenses/rust-components.json','licenses/source-recipes.json',
        'vcruntime140.dll','share/openssl/openssl.cnf','README.txt','etc/ssl/certs/ca-certificates.crt') {
        Assert-File (Join-Path $Root $file)
    }
    $inventory = Read-JsonArray (Join-Path $Root 'files.json')
    $actual = @(Get-ChildItem -LiteralPath $Root -Recurse -File)
    if ($actual.Count -ne $inventory.Count + 1) { throw "Unexpected/missing files in the bundle: $($actual.Count) present, $($inventory.Count + 1) inventoried (a shipped registry or a write into the application folder?)." }
    $seen = @{}
    foreach ($file in $inventory) {
        if ($file.path -match '(^[/\\]|^[A-Za-z]:|(^|[/\\])\.\.([/\\]|$))') { throw "Absolute/escaping manifest path: $($file.path)" }
        if ($seen.ContainsKey($file.path)) { throw "Duplicate inventory path: $($file.path)" }
        $seen[$file.path] = $true
        $path = Join-Path $Root $file.path
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Missing file: $($file.path). Re-extract the complete ZIP." }
        if ((Get-Item -LiteralPath $path).Length -ne $file.bytes -or (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $file.sha256) {
            throw "Integrity check failed: $($file.path)"
        }
        if ($file.origin -notin 'subdisc','gstreamer','msvc-redist','license-notice','generated') { throw "Unknown origin for $($file.path)" }
    }
    if (@($actual | Where-Object { $_.Name -match '^registry.*\.bin$' -or $_.Extension -in '.pc','.lib','.pdb','.a','.h','.exp','.ilk' }).Count) {
        throw 'Development files or a prebuilt GStreamer registry leaked into the bundle.'
    }
    $manifest = Read-Json (Join-Path $Root 'portable-runtime.json')
    $expected = Read-Json (Join-Path $PSScriptRoot 'windows/runtime.json')
    if (($manifest.plugins | ConvertTo-Json -Depth 6 -Compress) -ne ($expected.plugins | ConvertTo-Json -Depth 6 -Compress)) {
        throw 'Bundle plugin manifest differs from the checked-in runtime specification.'
    }
    foreach ($plugin in $expected.plugins) { Assert-File (Join-Path $Root "lib/gstreamer-1.0/gst$($plugin.name).dll") }
    # x64 PE32+ for every binary, normal + delay imports, transitive closure.
    $dependencies = @(Assert-BundleDependencies $Root)
    if ($dependencies.Count -ne @($manifest.dependencies).Count) { throw 'PE dependency graph differs from the manifest.' }
    $crt = [Diagnostics.FileVersionInfo]::GetVersionInfo((Join-Path $Root 'vcruntime140.dll'))
    if ("$($crt.FileMajorPart).$($crt.FileMinorPart).$($crt.FileBuildPart).$($crt.FilePrivatePart)" -ne $manifest.crt_version) { throw 'App-local MSVC runtime differs from the manifest.' }
    # No machine-specific paths in runtime configuration. Native upstream binaries
    # can contain inert compilation/debug strings; those are not used for discovery.
    foreach ($path in 'portable-runtime.json','share/openssl/openssl.cnf') {
        $text = Get-Content -LiteralPath (Join-Path $Root $path) -Raw -Encoding UTF8
        if ($text -match '(?i)\b[a-z]:[\\/]|\\\\[^\\]+\\') { throw "Absolute build-machine path in $path" }
    }
    return $manifest
}
# Child environment: inherited variables minus anything that could leak a global
# GStreamer/GLib/OpenSSL setup, PATH limited to Windows, private profile dirs.
function Get-ChildEnvironment([switch]$Hostile) {
    $environment = @{}
    foreach ($key in [Environment]::GetEnvironmentVariables('Process').Keys) {
        if ($key -match '^(GST|GSTREAMER|GIO|GI_TYPELIB|PKG_CONFIG|OPENSSL|SSL_CERT|SUBDISC_)') { $environment[$key] = $null }
    }
    $environment['PATH'] = "$env:SystemRoot\System32;$env:SystemRoot"
    $environment['LOCALAPPDATA'] = $localAppData
    $environment['APPDATA'] = $appData
    if ($Hostile) {
        # configure() must override every one of these.
        $global = if ($GStreamerRoot) { $GStreamerRoot } else { Join-Path $work 'absent global sdk' }
        $environment['PATH'] = "$global\bin;$($environment['PATH'])"
        $environment['GST_PLUGIN_PATH_1_0'] = "$global\lib\gstreamer-1.0"
        $environment['GST_PLUGIN_PATH'] = "$global\lib\gstreamer-1.0"
        $environment['GST_PLUGIN_SYSTEM_PATH'] = "$global\lib\gstreamer-1.0"
        $environment['GST_PLUGIN_SCANNER'] = "$global\libexec\gstreamer-1.0\gst-plugin-scanner.exe"
        $environment['GST_REGISTRY_UPDATE'] = 'no'
        $environment['GST_REGISTRY_FORK'] = 'no'
        $environment['GSTREAMER_1_0_ROOT_MSVC_X86_64'] = $global
    }
    return $environment
}
# Runs --portable-check (output in $Name.log, not a pipe) and returns
# [exit code, parsed report or $null].
function Invoke-CheckRaw([string]$Root, [string]$Name, [hashtable]$Environment, [string[]]$Extra = @()) {
    $output = Join-Path $work "$Name.json"
    $result = Invoke-Isolated (Join-Path $Root 'subdisc-client.exe') (@('--portable-check', $output) + $Extra) -WorkingDirectory $work -LogPath (Join-Path $work "$Name.log") -Environment $Environment -TimeoutSeconds $TimeoutSeconds
    $report = if (Test-Path -LiteralPath $output) { Read-Json $output } else { $null }
    return @($result.ExitCode, $report)
}
function Invoke-Check([string]$Root, [string]$Name, [hashtable]$Environment, [string[]]$Extra = @()) {
    $code, $report = Invoke-CheckRaw $Root $Name $Environment $Extra
    if ($code -ne 0 -or -not $report) {
        throw ("Portable media check '{0}' failed (exit {1}): {2}" -f $Name, $code, $(if ($report) { $report.error } else { 'no report written' }))
    }
    if (-not $report.ok -or -not $report.portable -or $report.audio_buffers -le 0 -or $report.video_frames -le 0) {
        throw "Media/portable check '$Name' returned an unexpected report."
    }
    # Compare normalized paths: separators may be mixed in the reported one.
    $prefix = [IO.Path]::GetFullPath((Join-Path $Environment['LOCALAPPDATA'] 'Subdisc\cache\gstreamer')).TrimEnd('\') + '\'
    if (-not [IO.Path]::GetFullPath($report.registry).StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) { throw "Registry is outside the per-user cache: $($report.registry)" }
    Assert-File $report.registry
    foreach ($plugin in $report.plugins) {
        if ($plugin.file -notlike 'gst*.dll') { throw "Unexpected plugin file: $($plugin.file)" }
    }
    if (@($report.modules.bundle).Count -lt @($report.plugins).Count) { throw "Module audit did not see the loaded plugins ($Name)." }
    $results[$Name] = $report
    return $report
}
# Negative test helper: must fail with a non-zero exit and an explanatory report.
function Assert-CheckFails([string]$Root, [string]$Name, [hashtable]$Environment, [string]$Pattern) {
    $code, $report = Invoke-CheckRaw $Root $Name $Environment
    if ($code -eq 0 -or ($report -and $report.ok)) { throw "Negative test '$Name' unexpectedly passed." }
    if (-not $report -or $report.error -notmatch $Pattern) {
        throw "Negative test '$Name' failed without the expected diagnostic /$Pattern/: $(if ($report) { $report.error } else { 'no report' })"
    }
    $results[$Name] = [ordered]@{ exit_code = $code; error = $report.error }
}
function New-TestCopy([string]$Name) {
    $copy = Join-Path $work $Name
    Copy-Item -LiteralPath $BundlePath -Destination $copy -Recurse
    return $copy
}
function Remove-InventoryEntry([string]$Root, [string]$Relative) {
    $all = Read-JsonArray (Join-Path $Root 'files.json')
    $inventory = @($all | Where-Object { $_.path -ne $Relative })
    if ($inventory.Count -ne $all.Count - 1) { throw "Inventory entry not found: $Relative" }
    Write-Utf8 (Join-Path $Root 'files.json') (ConvertTo-Json -InputObject $inventory -Depth 4)
}

try {
    $manifest = Assert-Inventory $BundlePath
    $checks.Add('inventory: relative paths, SHA-256, sizes, origins, no registry/dev files, plugin list, licenses, CRT version, configuration paths')
    $checks.Add('PE: every EXE/DLL is AMD64 PE32+; normal + delay imports resolved transitively inside the bundle or as explicit inbox Windows DLLs')

    # The scanner is a separate process: start it alone with a Windows-only PATH.
    # gst-plugin-scanner exits with 1 when run without the loader handshake.
    $scanner = Invoke-Isolated (Join-Path $BundlePath 'gst-plugin-scanner.exe') @() -WorkingDirectory $work -LogPath (Join-Path $work 'scanner.log') -Environment (Get-ChildEnvironment) -TimeoutSeconds 30
    if ($scanner.ExitCode -ne 1) { throw ("gst-plugin-scanner.exe exited with {0} (0x{0:X8}) instead of 1: {1}" -f $scanner.ExitCode, $scanner.Output) }
    $checks.Add('scanner: gst-plugin-scanner.exe starts from the bundle with a Windows-only PATH (exit 1, imports resolved)')

    $first = New-TestCopy 'first location'
    $environment = Get-ChildEnvironment
    $logs = Join-Path $work 'gst-logs'
    $null = New-Item -ItemType Directory -Path $logs
    $environment['GST_DEBUG'] = 'GST_PLUGIN_LOADING:6,GST_REGISTRY:4'
    # %p: one file per process; the scanner child inherits these variables.
    $environment['GST_DEBUG_FILE'] = Join-Path $logs 'gst-%p.log'
    $environment['GST_DEBUG_NO_COLOR'] = '1'
    $fresh = Invoke-Check $first 'fresh' $environment
    # GStreamer silently falls back to in-process scanning when the helper fails;
    # the debug log proves the bundled scanner process did the scan.
    # GST debug files (one per process) plus the client's stderr: the
    # "External plugin loader failed" fallback is a g_warning, not a GST log.
    $log = ((Get-ChildItem -LiteralPath $logs -File | ForEach-Object { Read-SharedText $_.FullName }) + (Read-SharedText (Join-Path $work 'fresh.log'))) -join "`n"
    $spawned = [regex]::Matches($log, 'Trying to spawn gst-plugin-scanner helper at (.+?), command') | ForEach-Object { $_.Groups[1].Value }
    if (-not $spawned -or @($spawned | Where-Object { -not $_.EndsWith('\first location\gst-plugin-scanner.exe', [StringComparison]::OrdinalIgnoreCase) }).Count) {
        throw "The bundled gst-plugin-scanner.exe was not the helper used for the fresh registry: $($spawned -join '; ')"
    }
    if ($log -match 'Failed starting plugin scanner|External plugin loader failed|Scanning in-process|Couldn.t create helper process|Unexpected child process exit state') {
        throw 'gst-plugin-scanner failed; GStreamer fell back to in-process scanning.'
    }
    $checks.Add('fresh registry under the private LOCALAPPDATA built by the bundled scanner process (debug log), no in-process fallback')
    $cached = Invoke-Check $first 'cached' (Get-ChildEnvironment)
    if ($cached.registry -ne $fresh.registry) { throw 'Registry path was not stable between launches.' }
    $hostile = Invoke-Check $first 'hostile' (Get-ChildEnvironment -Hostile)
    if ($hostile.registry -ne $fresh.registry) { throw 'Inherited GStreamer variables changed the registry location.' }
    $checks.Add("cached restart; hostile inherited PATH/GST_PLUGIN_*/scanner/registry variables$(if ($GStreamerRoot) { " pointing to the installed SDK $GStreamerRoot" } else { ' (no SDK installed: absent paths)' }) are ignored")
    $checks.Add('media: every plugin loaded from lib/gstreamer-1.0 with matching version and reviewed license, every element instantiated, Opus+RTP and VP8+RTP encode/decode, webrtcdsp, audiomixer+webrtcechoprobe, audio device providers')
    $checks.Add('module audit: every loaded GStreamer/GLib/CRT DLL comes from the bundle, none from System32 or a global installation')

    $moved = Join-Path $work "moved folder $unicode"
    Move-Item -LiteralPath $first -Destination $moved
    # Deny writes to the application folder (inherited by its files).
    $null = Invoke-Native "$env:SystemRoot\System32\icacls.exe" @($moved, '/deny', "*${sid}:(OI)(CI)(W)") -TimeoutSeconds 60
    $denied = $moved
    $relocated = Invoke-Check $moved 'moved-read-only' (Get-ChildEnvironment)
    if ($relocated.registry -eq $fresh.registry) { throw 'Moving the bundle reused the old registry.' }
    $checks.Add('relocation to a new path with spaces and Unicode, fresh registry, unrelated working directory, write-denied application folder')
    if ($Gui) {
        Invoke-Check $moved 'capture' (Get-ChildEnvironment) @('--portable-capture') | Out-Null
        if ($results['capture'].capture_frames -le 0) { throw 'D3D11/WGC capture produced no frames.' }
        foreach ($mode in '--demo','--dev-ui') {
            $output = Join-Path $work ($mode.TrimStart('-') + '.json')
            $ui = Invoke-Isolated (Join-Path $moved 'subdisc-client.exe') @($mode, '--portable-ui-smoke', $output) -WorkingDirectory $work -LogPath "$output.log" -Environment (Get-ChildEnvironment) -TimeoutSeconds $TimeoutSeconds
            if ($ui.ExitCode -ne 0) { throw ("GUI check {0} exited with {1}: {2}" -f $mode, $ui.ExitCode, $ui.Output) }
            Assert-File $output
            $ui = Read-Json $output
            if (-not $ui.ok -or $ui.demo_frames -lt 10) { throw "GUI check failed: $mode" }
        }
        $checks.Add('GUI session: D3D11/WGC monitor capture frames; --demo and --dev-ui render 10 frames and exit cleanly offline')
    }
    $null = Invoke-Native "$env:SystemRoot\System32\icacls.exe" @($moved, '/remove:d', "*$sid") -TimeoutSeconds 60
    $denied = $null
    $null = Assert-Inventory $moved
    $checks.Add('the application folder is unchanged after all runs')

    # Negative tests on separate copies, with the hostile (global SDK) environment.
    $missingPlugin = New-TestCopy 'missing plugin'
    Remove-Item -LiteralPath (Join-Path $missingPlugin 'lib/gstreamer-1.0/gstwebrtc.dll')
    Assert-CheckFails $missingPlugin 'missing-plugin-startup' (Get-ChildEnvironment -Hostile) 'gstwebrtc\.dll.*ZIP'
    Remove-InventoryEntry $missingPlugin 'lib/gstreamer-1.0/gstwebrtc.dll'
    Assert-CheckFails $missingPlugin 'missing-plugin-registry' (Get-ChildEnvironment -Hostile) 'webrtc'
    $checks.Add('negative: deleted gstwebrtc.dll is named at startup; with the inventory also edited, the registry check fails; no global fallback')

    # A DLL needed only by plugins (not by the client's own imports).
    $clientClosure = Get-ImportClosure $BundlePath (Join-Path $BundlePath 'subdisc-client.exe')
    $pluginOnly = @($manifest.dependencies | Where-Object { $_.source -eq 'app-local' -and $_.file -like 'lib/gstreamer-1.0/*' -and $_.dependency -notin $clientClosure -and $_.dependency -notmatch '^(vcruntime|msvcp)' } |
        ForEach-Object { $_.dependency } | Sort-Object -Unique)
    if (-not $pluginOnly) { throw 'No plugin-only transitive dependency found for the negative test.' }
    $transitive = if ($pluginOnly -match '^(lib)?nice') { @($pluginOnly -match '^(lib)?nice')[0] } else { $pluginOnly[0] }
    $missingTransitive = New-TestCopy 'missing transitive'
    Remove-Item -LiteralPath (Join-Path $missingTransitive $transitive)
    $static = $false
    try { $null = Assert-BundleDependencies $missingTransitive } catch { $static = $_.Exception.Message -match [regex]::Escape($transitive) }
    if (-not $static) { throw "Static validation did not report the missing transitive dependency $transitive." }
    Assert-CheckFails $missingTransitive 'missing-transitive-startup' (Get-ChildEnvironment -Hostile) ([regex]::Escape($transitive))
    Remove-InventoryEntry $missingTransitive $transitive
    Assert-CheckFails $missingTransitive 'missing-transitive-registry' (Get-ChildEnvironment -Hostile) 'ausente|Plugin|Elemento'
    $checks.Add("negative: deleted transitive dependency $transitive is reported by the PE validation, named at startup, and makes plugin loading fail even with the global SDK in PATH")

    # A DLL imported by the client itself. Either the loader stops before main()
    # (application dir + System32 only), or - if Windows resolved it elsewhere,
    # e.g. from the hostile PATH - the startup inventory check rejects the bundle
    # before GStreamer is used. Running with a foreign core DLL is a failure.
    $missingCore = New-TestCopy 'missing core'
    Remove-Item -LiteralPath (Join-Path $missingCore 'gstreamer-1.0-0.dll')
    $code, $report = Invoke-CheckRaw $missingCore 'missing-core' (Get-ChildEnvironment -Hostile)
    if ($code -eq 0 -or ($report -and ($report.ok -or $report.error -notmatch 'gstreamer-1\.0-0\.dll.*ZIP'))) {
        throw "The client ran without its gstreamer-1.0-0.dll: $(if ($report) { $report | ConvertTo-Json -Compress -Depth 3 } else { "exit $code" })"
    }
    $stage = if ($report) { 'startup inventory check (the loader found the DLL outside the bundle)' } else { 'Windows loader, before main()' }
    $results['missing-core'] = [ordered]@{ exit_code = $code; exit_code_hex = ('0x{0:X8}' -f $code); stopped_by = $stage; error = $(if ($report) { $report.error } else { $null }) }
    $checks.Add(('negative: without gstreamer-1.0-0.dll the client stops (exit 0x{0:X8}, {1}), even with the global SDK in PATH' -f $code, $stage))
    $success = $true
} catch {
    $failure = $_.Exception.Message
    throw
} finally {
    if ($denied) { $null = & "$env:SystemRoot\System32\icacls.exe" $denied /remove:d "*$sid" 2>&1 }
    $null = [SubdiscErrorMode]::SetErrorMode($previousErrorMode)
    $null = New-Item -ItemType Directory -Force -Path (Split-Path -Parent $ReportPath)
    $bundleManifest = Join-Path $BundlePath 'portable-runtime.json'
    Write-Utf8 $ReportPath ([ordered]@{
        ok = $success; utc = [DateTime]::UtcNow.ToString('o'); os = [Environment]::OSVersion.VersionString
        powershell = $PSVersionTable.PSVersion.ToString(); bundle = $BundlePath
        gstreamer = $(if (Test-Path -LiteralPath $bundleManifest) { (Read-Json $bundleManifest).gstreamer_version } else { $null })
        global_sdk_used_as_hostile_environment = $GStreamerRoot
        checks = @($checks.ToArray()); results = $results; error = $failure
        gui = $(if ($Gui) { 'requested; see checks' } else { 'not run; use -Gui on an interactive desktop' })
        manual = 'NOT VERIFIED here: clean Windows 10/11 VM, Explorer double-click, real server calls between two users, physical microphone/speakers, acoustic AEC, window capture, reconnection'
    } | ConvertTo-Json -Depth 12)
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
Write-Host "Portable automated checks passed. Report: $ReportPath"
Write-Host 'Real devices, server calls and Explorer double-click still require the documented Windows release tests.'
