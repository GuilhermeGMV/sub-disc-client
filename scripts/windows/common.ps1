# Shared by build, packaging and validation; Windows PowerShell 5.1 compatible.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (-not ('SubdiscPe' -as [type])) {
    Add-Type -Path (Join-Path $PSScriptRoot 'PeDependencies.cs')
} elseif (-not ('SubdiscErrorMode' -as [type]) -or -not ('SubdiscProcess' -as [type]) -or -not [SubdiscPe].GetMethod('LinkerVersion')) {
    throw 'An older SubdiscPe type is loaded in this session. Start a new PowerShell process.'
}
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
if ($PSVersionTable.PSEdition -ne 'Core') {
    # Windows PowerShell 5.1 gives every array an ETS 'Count' alias, and its
    # ConvertTo-Json then writes a PSObject-wrapped array as {"value":[..],"Count":n}
    # instead of [..]. The intrinsic .Count keeps working without this type data.
    Remove-TypeData -TypeName System.Array -ErrorAction SilentlyContinue
}

$script:ArtifactNames = @('Subdisc-portable', 'Subdisc-Windows-x64-portable.zip', 'portable-validation.json', 'portable-test-logs')

function Assert-WindowsX64 {
    if ($env:OS -ne 'Windows_NT' -or -not [Environment]::Is64BitProcess) {
        throw 'Use Windows PowerShell x64 (Windows 10/11), not x86 PowerShell.'
    }
}
function Assert-File([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "Required file missing: $Path" }
}
function Write-Utf8([string]$Path, [string]$Text) {
    [IO.File]::WriteAllText($Path, $Text, (New-Object Text.UTF8Encoding $false))
}
# Reads a log that another process may still hold open for writing.
function Read-SharedText([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return '' }
    $stream = New-Object IO.FileStream($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, ([IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete))
    try {
        $reader = New-Object IO.StreamReader($stream, (New-Object Text.UTF8Encoding $false), $true)
        try { return $reader.ReadToEnd().Trim() } finally { $reader.Dispose() }
    } finally { $stream.Dispose() }
}
function Read-Json([string]$Path) {
    Get-Content -LiteralPath $Path -Raw -Encoding UTF8 | ConvertFrom-Json
}
# Windows PowerShell 5.1's ConvertFrom-Json emits a JSON array as ONE object
# (so @(...) around it has a single element); PowerShell 7 enumerates it.
# Returns a real array in both, including for [] and one-element arrays.
function Read-JsonArray([string]$Path) {
    $value = Read-Json $Path
    return ,@(foreach ($item in $value) { $item })
}
function Get-RelativePath([string]$Root, [string]$Path) {
    $prefix = [IO.Path]::GetFullPath($Root).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    $full = [IO.Path]::GetFullPath($Path)
    if (-not $full.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) { throw "Path outside bundle: $Path" }
    $full.Substring($prefix.Length).Replace('\', '/')
}

# The caller's process environment is restored after build/package/test, also
# when the scripts are dot-sourced or run with & from an existing session.
function Save-ProcessEnvironment {
    # Case-insensitive like Windows itself. Names starting with '=' (per-drive
    # current directories) cannot be set through .NET and are left alone.
    $saved = New-Object 'Collections.Generic.Dictionary[string,string]' ([StringComparer]::OrdinalIgnoreCase)
    foreach ($entry in [Environment]::GetEnvironmentVariables('Process').GetEnumerator()) {
        if (-not ([string]$entry.Key).StartsWith('=')) { $saved[[string]$entry.Key] = [string]$entry.Value }
    }
    return ,$saved
}
function Restore-ProcessEnvironment($Saved) {
    foreach ($key in @([Environment]::GetEnvironmentVariables('Process').Keys)) {
        if (-not ([string]$key).StartsWith('=') -and -not $Saved.ContainsKey([string]$key)) {
            # [NullString]::Value: PowerShell turns $null into '' for .NET string
            # parameters, and since .NET 9 (PowerShell 7.5+) '' creates an empty
            # variable instead of deleting it.
            [Environment]::SetEnvironmentVariable([string]$key, [NullString]::Value, 'Process')
        }
    }
    foreach ($entry in $Saved.GetEnumerator()) {
        if ([Environment]::GetEnvironmentVariable($entry.Key, 'Process') -cne $entry.Value) {
            [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value, 'Process')
        }
    }
}

# Windows command-line quoting (CommandLineToArgvW rules) for ProcessStartInfo.
function ConvertTo-ArgumentString([string[]]$Arguments) {
    $quoted = foreach ($argument in $Arguments) {
        if ($argument -ne '' -and $argument -notmatch '[\s"]') { $argument }
        else { '"' + (($argument -replace '(\\*)"', '$1$1\"') -replace '(\\+)$', '$1$1') + '"' }
    }
    return ($quoted -join ' ')
}
# Runs a native program with an explicit exit-code and timeout policy. Output is
# captured (both streams, read asynchronously) and returned for parsing/errors.
function Invoke-Native {
    param(
        [Parameter(Mandatory)][string]$FilePath,
        [string[]]$Arguments = @(),
        [string]$WorkingDirectory = (Get-Location).ProviderPath,
        [int]$TimeoutSeconds = 3600,
        [int[]]$AllowedExitCodes = @(0),
        # The caller inspects ExitCode itself (negative tests); timeouts still throw.
        [switch]$IgnoreExitCode,
        [hashtable]$Environment = $null,
        # cmd.exe does not follow the CommandLineToArgvW quoting rules.
        [string]$RawArguments,
        [Text.Encoding]$OutputEncoding = (New-Object Text.UTF8Encoding $false)
    )
    $start = New-Object Diagnostics.ProcessStartInfo
    $start.FileName = $FilePath
    $start.Arguments = if ($PSBoundParameters.ContainsKey('RawArguments')) { $RawArguments } else { ConvertTo-ArgumentString $Arguments }
    $start.WorkingDirectory = $WorkingDirectory
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.StandardOutputEncoding = $OutputEncoding
    $start.StandardErrorEncoding = New-Object Text.UTF8Encoding $false
    if ($Environment) {
        foreach ($entry in $Environment.GetEnumerator()) {
            if ($null -eq $entry.Value) { $start.EnvironmentVariables.Remove($entry.Key) }
            else { $start.EnvironmentVariables[$entry.Key] = [string]$entry.Value }
        }
    }
    $process = New-Object Diagnostics.Process
    $process.StartInfo = $start
    try {
        if (-not $process.Start()) { throw "Cannot start $FilePath" }
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
            try { $process.Kill() } catch { }
            $null = $process.WaitForExit(10000)
            # The pipes close with the process: show what it printed before hanging.
            $partial = ''
            foreach ($task in $stderr, $stdout) {
                if ($task.Wait(5000)) { $partial += $task.Result }
            }
            $partial = $partial.Trim()
            if ($partial.Length -gt 4000) { $partial = $partial.Substring($partial.Length - 4000) }
            throw "Timed out after ${TimeoutSeconds}s: $FilePath $($start.Arguments)`nOutput before the timeout: $(if ($partial) { $partial } else { '(none)' })"
        }
        $process.WaitForExit()
        $result = [pscustomobject]@{ ExitCode = $process.ExitCode; Output = $stdout.Result; Error = $stderr.Result }
    } finally { $process.Dispose() }
    if (-not $IgnoreExitCode -and $result.ExitCode -notin $AllowedExitCodes) {
        $detail = ($result.Error + "`n" + $result.Output).Trim()
        if ($detail.Length -gt 4000) { $detail = $detail.Substring($detail.Length - 4000) }
        throw ("{0} exited with {1} (0x{2:X8}): {3}`n{4}" -f $FilePath, $result.ExitCode, $result.ExitCode, $start.Arguments, $detail)
    }
    return $result
}

# Runs a program with stdout+stderr in $LogPath instead of a pipe (see
# SubdiscProcess) and a timeout that ends its whole process tree. The child gets
# this process's environment with $Environment applied ($null removes a name).
# Never throws for a non-zero exit code: callers check ExitCode themselves.
function Invoke-Isolated {
    param(
        [Parameter(Mandatory)][string]$FilePath,
        [string[]]$Arguments = @(),
        [Parameter(Mandatory)][string]$WorkingDirectory,
        [Parameter(Mandatory)][string]$LogPath,
        [hashtable]$Environment = @{},
        [int]$TimeoutSeconds = 120
    )
    $block = @{}
    foreach ($entry in [Environment]::GetEnvironmentVariables('Process').GetEnumerator()) {
        if (-not ([string]$entry.Key).StartsWith('=')) { $block[[string]$entry.Key] = [string]$entry.Value }
    }
    foreach ($entry in $Environment.GetEnumerator()) {
        if ($null -eq $entry.Value) { $block.Remove($entry.Key) } else { $block[$entry.Key] = [string]$entry.Value }
    }
    $argumentString = ConvertTo-ArgumentString $Arguments
    $processId = 0
    try {
        $code = [SubdiscProcess]::Run($FilePath, $argumentString, $WorkingDirectory, $block, $LogPath, $TimeoutSeconds * 1000, [ref]$processId)
    } catch {
        $inner = $_.Exception
        while ($inner.InnerException) { $inner = $inner.InnerException }
        $output = Read-SharedText $LogPath
        if ($inner -is [TimeoutException]) {
            if ($output.Length -gt 4000) { $output = $output.Substring($output.Length - 4000) }
            throw "$($inner.Message)`nOutput before the timeout: $(if ($output) { $output } else { '(none)' })"
        }
        if ($inner -is [ComponentModel.Win32Exception] -and $inner.Message -like 'CreateProcess failed*') {
            if (-not (Test-Path -LiteralPath $FilePath -PathType Leaf)) { throw "Program not found: $FilePath" }
            # Not found, access denied, invalid/blocked image: the program itself
            # cannot start, a pipe-based start would fail the same way.
            if ($inner.NativeErrorCode -in 2, 3, 5, 32, 193, 216, 225, 1260) {
                throw "Cannot start ${FilePath}: $($inner.Message) (Win32 error $($inner.NativeErrorCode))"
            }
            # Safety net for the native runner itself; the pipe-based runner
            # yields the same exit code, only with pipes for the output.
            Write-Warning "Native runner could not start $FilePath ($($inner.Message)); using pipes instead."
            $result = Invoke-Native $FilePath $Arguments -WorkingDirectory $WorkingDirectory -Environment $Environment -TimeoutSeconds $TimeoutSeconds -IgnoreExitCode
            Write-Utf8 $LogPath (($result.Error + "`n" + $result.Output).Trim())
            return [pscustomobject]@{ ExitCode = $result.ExitCode; Output = (($result.Error + "`n" + $result.Output).Trim()); ProcessId = 0 }
        }
        throw
    }
    return [pscustomobject]@{ ExitCode = $code; Output = (Read-SharedText $LogPath); ProcessId = $processId }
}

function Test-SystemDll([string]$Name) {
    # Explicit inbox Windows 10/11 contracts, NOT 'anything found in System32'.
    # VC runtimes, vendor codecs/drivers and SDK redistributables are not inbox.
    if ($Name -match '^(api-ms-win-|ext-ms-win-)[a-z0-9-]+\.dll$') { return $true }
    return $Name.ToLowerInvariant() -in @(
        'kernel32.dll','kernelbase.dll','ntdll.dll','user32.dll','gdi32.dll','gdi32full.dll',
        'advapi32.dll','secur32.dll','sspicli.dll','crypt32.dll','cryptbase.dll','bcrypt.dll',
        'bcryptprimitives.dll','ncrypt.dll','cryptsp.dll','wintrust.dll','msasn1.dll',
        'ole32.dll','oleaut32.dll','combase.dll','rpcrt4.dll','shell32.dll','shlwapi.dll',
        'shcore.dll','propsys.dll','comctl32.dll','comdlg32.dll','imm32.dll','uxtheme.dll',
        'dwmapi.dll','version.dll','ws2_32.dll','iphlpapi.dll','dnsapi.dll','mswsock.dll',
        'winhttp.dll','wininet.dll','normaliz.dll','userenv.dll','netapi32.dll','wtsapi32.dll',
        'winmm.dll','msacm32.dll','avrt.dll','mmdevapi.dll','audioses.dll','ksuser.dll',
        'setupapi.dll','cfgmgr32.dll','powrprof.dll','hid.dll','winspool.drv','winusb.dll',
        'd3d11.dll','d3d12.dll','dxgi.dll','d3d9.dll','d3dcompiler_47.dll',
        'dwrite.dll','d2d1.dll','dcomp.dll','opengl32.dll','glu32.dll',
        'mf.dll','mfplat.dll','mfreadwrite.dll','evr.dll',
        'windowscodecs.dll','runtimeobject.dll','winmmbase.dll','win32u.dll',
        'uiautomationcore.dll','oleacc.dll','gdiplus.dll','msimg32.dll',
        'ucrtbase.dll','msvcrt.dll','msvcp_win.dll','dbghelp.dll','dbgcore.dll',
        'psapi.dll','imagehlp.dll','cabinet.dll'
    )
}
function Get-PeImports([string]$Path) { [SubdiscPe]::Imports($Path) }
function Get-PeLinkerVersion([string]$Path) { [SubdiscPe]::LinkerVersion($Path) }
# All bundled PE files load into a process whose application directory is the
# bundle root (the client, or the scanner placed beside it), so every non-inbox
# import of every PE - plugins and dynamic modules included - must be in the root.
function Assert-BundleDependencies([string]$Root) {
    $edges = @()
    foreach ($file in Get-ChildItem -LiteralPath $Root -Recurse -File | Where-Object { $_.Extension -in '.dll','.exe' } | Sort-Object FullName) {
        $relative = Get-RelativePath $Root $file.FullName
        foreach ($dll in Get-PeImports $file.FullName) {
            if (Test-SystemDll $dll) {
                $edges += [ordered]@{file=$relative; dependency=$dll; source='windows'}
            } else {
                if (-not (Test-Path -LiteralPath (Join-Path $Root $dll) -PathType Leaf)) {
                    throw "Missing dependency: $dll (imported by $relative). Re-extract the complete ZIP / repackage."
                }
                $edges += [ordered]@{file=$relative; dependency=$dll; source='app-local'}
            }
        }
    }
    return $edges
}
# DLLs reachable from $Start through static imports inside $Root (lower-case names).
function Get-ImportClosure([string]$Root, [string]$Start) {
    $seen = @{}
    $queue = New-Object 'Collections.Generic.Queue[string]'
    $queue.Enqueue($Start)
    while ($queue.Count -gt 0) {
        $path = $queue.Dequeue()
        foreach ($dll in Get-PeImports $path) {
            $candidate = Join-Path $Root $dll
            if (-not $seen.ContainsKey($dll) -and (Test-Path -LiteralPath $candidate -PathType Leaf)) {
                $seen[$dll] = $true
                $queue.Enqueue($candidate)
            }
        }
    }
    return @($seen.Keys)
}

function Initialize-Msvc {
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    Assert-File $vswhere
    $vs = (Invoke-Native $vswhere @('-latest','-products','*','-requires','Microsoft.VisualStudio.Component.VC.Tools.x86.x64','-property','installationPath') -TimeoutSeconds 120).Output.Trim()
    if (-not $vs) { throw 'Install Visual Studio Build Tools: Desktop development with C++ (MSVC x64) and a Windows 10/11 SDK.' }
    $dev = Join-Path $vs 'Common7\Tools\VsDevCmd.bat'
    Assert-File $dev
    # /u: the 'set' output is UTF-16, so non-ASCII profile paths survive.
    $result = Invoke-Native $env:ComSpec -RawArguments "/d /u /s /c `"`"$dev`" -no_logo -arch=x64 -host_arch=x64 >nul && set`"" -OutputEncoding ([Text.Encoding]::Unicode) -TimeoutSeconds 300
    foreach ($line in $result.Output.Split([char]10)) {
        if ($line.TrimEnd([char]13) -match '^([^=]+)=(.*)$') {
            [Environment]::SetEnvironmentVariable($matches[1], $matches[2], 'Process')
        }
    }
    foreach ($name in 'VCToolsInstallDir','VCToolsVersion','VCToolsRedistDir','WindowsSdkDir','WindowsSDKVersion') {
        if (-not [Environment]::GetEnvironmentVariable($name, 'Process')) { throw "VsDevCmd did not define ${name}: incomplete MSVC/Windows SDK installation." }
    }
    # The tools must be the x64-hosted, x64-targeting MSVC ones, not e.g. Git's link.exe.
    $bin = Join-Path $env:VCToolsInstallDir 'bin\Hostx64\x64'
    foreach ($tool in 'cl.exe','link.exe','lib.exe') {
        $found = Get-Command $tool -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        if (-not $found -or -not $found.Source.StartsWith($bin, [StringComparison]::OrdinalIgnoreCase)) {
            throw "MSVC x64 $tool not first in PATH (expected $bin)."
        }
    }
    $cl = Invoke-Native (Join-Path $bin 'cl.exe') @() -AllowedExitCodes @(0, 2) -TimeoutSeconds 60
    $banner = ($cl.Error + "`n" + $cl.Output).Trim()
    if ($banner -notmatch 'Compiler Version [0-9.]+ for x64') { throw "cl.exe does not target x64: $banner" }
    $sdkVersion = $env:WindowsSDKVersion.TrimEnd('\')
    if ([version]$sdkVersion -lt [version]'10.0.17763.0') { throw "Windows SDK $sdkVersion is too old; install a Windows 10/11 SDK." }
    foreach ($lib in "um\x64\kernel32.lib", "ucrt\x64\ucrt.lib") {
        Assert-File (Join-Path $env:WindowsSdkDir "Lib\$sdkVersion\$lib")
    }
    $rc = Get-Command rc.exe -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $rc) { throw 'Windows SDK rc.exe missing.' }
    return [ordered]@{
        visual_studio = $vs; msvc_tools = $env:VCToolsVersion; windows_sdk = $sdkVersion
        compiler = ($banner -split "`n")[0].Trim()
    }
}
function Find-GStreamer([string]$Root) {
    if (-not $Root) {
        $candidates = @($env:GSTREAMER_1_0_ROOT_MSVC_X86_64,
            "$env:LOCALAPPDATA\Programs\gstreamer\1.0\msvc_x86_64",
            "$env:ProgramFiles\gstreamer\1.0\msvc_x86_64", 'C:\gstreamer\1.0\msvc_x86_64')
        $Root = $candidates | Where-Object { $_ -and (Test-Path -LiteralPath (Join-Path $_ 'bin\gstreamer-1.0-0.dll')) } | Select-Object -First 1
    }
    if (-not $Root) { throw 'Install GStreamer MSVC x86_64 Runtime + Development (Complete); set GSTREAMER_1_0_ROOT_MSVC_X86_64.' }
    $Root = (Resolve-Path -LiteralPath $Root).Path
    # Runtime: core DLL, tools, scanner, CA data. Development: headers, import libs, .pc files.
    foreach ($file in 'bin/gstreamer-1.0-0.dll','bin/gst-inspect-1.0.exe','bin/pkg-config.exe','etc/ssl/certs/ca-certificates.crt',
        'libexec/gstreamer-1.0/gst-plugin-scanner.exe','include/gstreamer-1.0/gst/gst.h',
        'include/gstreamer-1.0/gst/webrtc/webrtc.h','include/glib-2.0/glib.h',
        'lib/gstreamer-1.0.lib','lib/gstwebrtc-1.0.lib','lib/glib-2.0.lib',
        'lib/pkgconfig/gstreamer-1.0.pc','lib/pkgconfig/gstreamer-app-1.0.pc','lib/pkgconfig/gstreamer-video-1.0.pc',
        'lib/pkgconfig/gstreamer-webrtc-1.0.pc','lib/pkgconfig/gstreamer-sdp-1.0.pc','share/licenses') {
        if (-not (Test-Path -LiteralPath (Join-Path $Root $file))) {
            throw "GStreamer Runtime/Development incomplete or not MSVC x86_64: $file missing in $Root"
        }
    }
    $null = Get-PeImports (Join-Path $Root 'bin/gstreamer-1.0-0.dll')
    $null = Get-PeImports (Join-Path $Root 'libexec/gstreamer-1.0/gst-plugin-scanner.exe')
    return $Root
}
# pkg-config describes the Development package, gst-inspect the Runtime DLLs it
# loads; both must report the same version or the headers/libs do not match.
function Get-GStreamerVersion([string]$Root) {
    $pkgConfig = Join-Path $Root 'bin\pkg-config.exe'
    # Throwaway registry and an empty plugin directory: no plugin scan, and the
    # developer's cache is untouched.
    $scratch = Join-Path ([IO.Path]::GetTempPath()) ('subdisc-gst-version-' + [guid]::NewGuid().ToString('N'))
    $null = New-Item -ItemType Directory -Force -Path (Join-Path $scratch 'no-plugins')
    $environment = @{ PKG_CONFIG_PATH = (Join-Path $Root 'lib\pkgconfig'); PKG_CONFIG_LIBDIR = (Join-Path $Root 'lib\pkgconfig')
        PATH = "$Root\bin;$env:SystemRoot\System32"; GST_REGISTRY_1_0 = (Join-Path $scratch 'registry.bin')
        GST_PLUGIN_SYSTEM_PATH_1_0 = (Join-Path $scratch 'no-plugins')
        GST_PLUGIN_PATH_1_0 = $null; GST_PLUGIN_PATH = $null; GST_PLUGIN_SYSTEM_PATH = $null; GST_DEBUG = $null }
    try {
        $version = (Invoke-Native $pkgConfig @('--modversion','gstreamer-1.0') -Environment $environment -TimeoutSeconds 60).Output.Trim()
        # major.minor.micro[.nano]: nano 1 = development (GIT) build, >= 2 = prerelease.
        if ($version -notmatch '^1\.[0-9]+\.[0-9]+(\.[0-9]+)?$' -or [version]$version -lt [version]'1.22') { throw "GStreamer >= 1.22 required (Development reports '$version')." }
        $parts = $version.Split('.')
        $release = $parts[0..2] -join '.'
        if ($parts.Count -eq 4 -and $parts[3] -ne '0') {
            Write-Warning "GStreamer $version is a development/prerelease build (nano $($parts[3])). Prefer an official release SDK for distribution: the generated source references point to the $release tag."
        }
        foreach ($module in 'gstreamer-base-1.0','gstreamer-app-1.0','gstreamer-video-1.0','gstreamer-webrtc-1.0','gstreamer-sdp-1.0') {
            $other = (Invoke-Native $pkgConfig @('--modversion',$module) -Environment $environment -TimeoutSeconds 60).Output.Trim()
            if ($other -ne $version) { throw "Mismatched GStreamer Development modules: $module $other vs gstreamer-1.0 $version" }
        }
        # gst-inspect 1.28.8 was seen hanging with its output on a pipe while it
        # finished at once on a console, so its output goes to a file instead.
        $inspect = Join-Path $Root 'bin\gst-inspect-1.0.exe'
        $output = Join-Path $scratch 'version.txt'
        $null = Invoke-Native $env:ComSpec -RawArguments "/d /s /c `"`"$inspect`" --version > `"$output`" 2>&1`"" -Environment $environment -TimeoutSeconds 120
        $text = [IO.File]::ReadAllText($output)
    } finally { Remove-Item -LiteralPath $scratch -Recurse -Force -ErrorAction SilentlyContinue }
    # gst-inspect prints the tools' major.minor.micro and the core library's
    # version string ("GStreamer 1.28.8", "... (GIT)", "... (prerelease)").
    if ($text -notmatch "(?m)version $([regex]::Escape($release))(\.[0-9]+)?\s*$" -or
        $text -notmatch "(?m)^GStreamer $([regex]::Escape($release))( \((GIT|prerelease)\))?\s*$") {
        throw "GStreamer Runtime and Development versions differ: Development $version, Runtime: $($text.Trim())"
    }
    return $version
}

# -KeepDiagnostics: after a failure, keep portable-test-logs for inspection.
function Remove-PortableArtifacts([string]$Dist, [switch]$KeepDiagnostics) {
    foreach ($name in $script:ArtifactNames) {
        if ($KeepDiagnostics -and $name -eq 'portable-test-logs') { continue }
        $path = Join-Path $Dist $name
        if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Recurse -Force }
    }
}
# Deterministic entry order, '/' separators and one top-level folder.
function New-PortableZip([string]$Source, [string]$Destination) {
    $base = Split-Path -Leaf $Source
    $stream = [IO.File]::Open($Destination, [IO.FileMode]::CreateNew)
    try {
        # Exactly [Text.Encoding]::UTF8: .NET Framework (PowerShell 5.1) only sets
        # the ZIP UTF-8 name flag for that instance, not for new UTF8Encoding().
        $zip = New-Object IO.Compression.ZipArchive($stream, [IO.Compression.ZipArchiveMode]::Create, $false, [Text.Encoding]::UTF8)
        try {
            foreach ($file in Get-ChildItem -LiteralPath $Source -Recurse -File | Sort-Object { Get-RelativePath $Source $_.FullName }) {
                $entry = $zip.CreateEntry("$base/" + (Get-RelativePath $Source $file.FullName), [IO.Compression.CompressionLevel]::Optimal)
                # ZIP (DOS) timestamps cover 1980-2107; e.g. crate files may carry 1970.
                $time = $file.LastWriteTime
                if ($time.Year -lt 1980) { $time = New-Object DateTime(1980, 1, 2) }
                if ($time.Year -gt 2107) { $time = New-Object DateTime(2107, 12, 30) }
                $entry.LastWriteTime = New-Object DateTimeOffset($time)
                $reader = [IO.File]::OpenRead($file.FullName)
                $writer = $entry.Open()
                try { $reader.CopyTo($writer) } finally { $writer.Dispose(); $reader.Dispose() }
            }
        } finally { $zip.Dispose() }
    } finally { $stream.Dispose() }
}
function Expand-PortableZip([string]$Zip, [string]$Destination) {
    # Names are UTF-8 (flagged); never fall back to the ANSI/OEM code page.
    $archive = [IO.Compression.ZipFile]::Open($Zip, [IO.Compression.ZipArchiveMode]::Read, [Text.Encoding]::UTF8)
    try {
        foreach ($entry in $archive.Entries) {
            if ($entry.FullName -notmatch '^Subdisc-portable/' -or $entry.FullName -match '(^|/)\.\.(/|$)|\\|^/|:') {
                throw "Unexpected ZIP entry: $($entry.FullName)"
            }
        }
    } finally { $archive.Dispose() }
    [IO.Compression.ZipFile]::ExtractToDirectory($Zip, $Destination, [Text.Encoding]::UTF8)
}
