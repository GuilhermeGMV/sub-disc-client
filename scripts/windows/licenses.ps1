param([string]$Bundle, [string]$GStreamerRoot, [hashtable]$Origins)
$ErrorActionPreference = 'Stop'
# Windows PowerShell 5.1 downloads are very slow with the progress bar.
$ProgressPreference = 'SilentlyContinue'
$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$licenseRoot = Join-Path $GStreamerRoot 'share/licenses'
if (-not (Test-Path -LiteralPath $licenseRoot -PathType Container)) { throw 'GStreamer share/licenses missing: install the complete official Runtime/Development.' }
$destination = Join-Path $Bundle 'licenses'
$null = New-Item -ItemType Directory -Force -Path $destination
# Retain upstream copyright notices, alternative-license explanations and the
# licenses of statically embedded dependencies (e.g. usrsctp and WebRTC DSP).
Copy-Item -LiteralPath $licenseRoot -Destination (Join-Path $destination 'gstreamer') -Recurse
$spec = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'runtime.json') -Raw -Encoding UTF8 | ConvertFrom-Json
$components = @($spec.plugins.component | Sort-Object -Unique)
# Some codec/DSP and header-only dependencies are embedded in the plugin DLLs.
$components += @('libvpx','webrtc-audio-processing','opus','libsrtp','openssl','orc')
# Map dynamically linked libraries to the recipe carrying the upstream notices.
# Unknown new dependencies stop packaging instead of silently inventing a license.
$rules = [ordered]@{
    '^(lib)?gst(webrtc|webrtcnice|sctp|badaudio|codecparsers|codecs|d3d11|d3dshader|dxva|winrt)-.*\.dll$' = 'gst-plugins-bad-1.0'
    '^(lib)?gst(app|audio|video|pbutils|rtp|sdp|tag|allocators|riff)-.*\.dll$' = 'gst-plugins-base-1.0'
    '^(lib)?gst.*\.dll$' = 'gstreamer-1.0'
    '^gst-plugin-scanner\.exe$' = 'gstreamer-1.0'
    '^(lib)?(glib|gobject|gio|gmodule|gthread)-2\.0.*\.dll$' = 'glib'
    '^(lib)?(ssl|crypto)-.*\.dll$|^(legacy|default|fips)\.dll$' = 'openssl'
    '^(lib)?nice.*\.dll$' = 'libnice'
    '^(lib)?opus.*\.dll$' = 'opus'
    '^(lib)?vpx.*\.dll$' = 'libvpx'
    '^(lib)?srtp.*\.dll$' = 'libsrtp'
    '^(lib)?(webrtc.*audio.*processing|webrtc.*audio.*coding).*\.dll$' = 'webrtc-audio-processing'
    '^(lib)?orc-.*\.dll$' = 'orc'
    '^(lib)?ffi.*\.dll$' = 'libffi'
    '^(lib)?intl.*\.dll$' = 'proxy-libintl'
    '^(lib)?iconv.*\.dll$' = 'libiconv'
    '^(lib)?pcre2.*\.dll$' = 'pcre2'
    '^(lib)?z(lib)?[0-9-]*\.dll$' = 'zlib'
    '^gio(gnutls|openssl|schannel).*\.dll$' = 'glib-networking'
    '^gio(libproxy|environmentproxy).*\.dll$' = 'glib-networking'
    '^(lib)?gnutls.*\.dll$' = 'gnutls'
    '^(lib)?(nettle|hogweed).*\.dll$' = 'nettle'
    '^(lib)?gmp.*\.dll$' = 'gmp'
    '^(lib)?tasn1.*\.dll$' = 'libtasn1'
    '^(lib)?p11-kit.*\.dll$' = 'p11-kit'
    '^(lib)?proxy.*\.dll$' = 'libproxy'
    '^(lib)?bz2.*\.dll$' = 'bzip2'
    '^(lib)?lzma.*\.dll$' = 'xz'
    '^(lib)?xml2.*\.dll$' = 'libxml2'
    '^(lib)?usrsctp.*\.dll$' = 'usrsctp'
}
$inventory = @()
foreach ($entry in $Origins.GetEnumerator() | Sort-Object Key) {
    if ($entry.Value -ne 'gstreamer') { continue }
    $name = Split-Path -Leaf $entry.Key
    $component = $null
    if ($entry.Key -eq 'etc/ssl/certs/ca-certificates.crt') {
        $component = 'ca-certificates'
    } elseif ($entry.Key -like 'lib/gstreamer-1.0/*') {
        $component = ($spec.plugins | Where-Object { "gst$($_.name).dll" -eq $name }).component
    } else {
        foreach ($rule in $rules.GetEnumerator()) {
            if ($name -match $rule.Key) { $component = $rule.Value; break }
        }
    }
    if (-not $component) { throw "No reviewed license/source mapping for $($entry.Key). Add its upstream recipe to windows/licenses.ps1." }
    $components += $component
    $inventory += [ordered]@{file=$entry.Key; notices="licenses/gstreamer/$component"; component=$component}
}
$version = (Select-String -LiteralPath (Join-Path $GStreamerRoot 'lib/pkgconfig/gstreamer-1.0.pc') -Pattern '^Version:\s*(\S+)').Matches.Groups[1].Value
if ($version -notmatch '^(1\.[0-9]+\.[0-9]+)(\.[0-9]+)?$') { throw 'Unrecognized GStreamer version; corresponding source references cannot be generated.' }
$sdkVersion = $version
# Upstream tags carry major.minor.micro only. A nano (development/prerelease)
# SDK is not exactly the tagged release; the notice files record that.
$version = $matches[1]
$exactRelease = ($sdkVersion -eq $version -or $sdkVersion -eq "$version.0")
$extraNotices = @()
foreach ($component in $components | Sort-Object -Unique) {
    $directory = Join-Path $destination "gstreamer/$component"
    # Official 1.26.10 Development omits the good module's license directory.
    # Recover its actual upstream texts, not an inferred license from another DLL.
    if (-not (Test-Path -LiteralPath $directory) -and $component -eq 'gst-plugins-good-1.0') {
        $null = New-Item -ItemType Directory -Force -Path $directory
        # COPYING (the license text) is required; upstream dropped AUTHORS in 1.28.
        foreach ($name in 'COPYING','AUTHORS') {
            $url = "https://raw.githubusercontent.com/GStreamer/gstreamer/$version/subprojects/gst-plugins-good/$name"
            $out = Join-Path $directory $name
            [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
            try {
                Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $out
            } catch {
                $response = $_.Exception.Response
                if ($name -eq 'AUTHORS' -and $response -and [int]$response.StatusCode -eq 404) {
                    Remove-Item -LiteralPath $out -Force -ErrorAction SilentlyContinue
                    continue
                }
                throw "Cannot download upstream notice $url : $($_.Exception.Message)"
            }
            if ((Get-Item -LiteralPath $out).Length -lt 100) { throw "Incomplete upstream notice: $url" }
            $extraNotices += [ordered]@{component=$component;url=$url;sha256=(Get-FileHash -LiteralPath $out -Algorithm SHA256).Hash}
        }
    }
    if (-not (Test-Path -LiteralPath $directory) -and $component -eq 'ca-certificates') {
        $null = New-Item -ItemType Directory -Force -Path $directory
        $url = "https://raw.githubusercontent.com/GStreamer/cerbero/$version/data/licenses/MPL-2.0.txt"
        $out = Join-Path $directory 'MPL-2.0.txt'
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $out
        if ((Get-Item -LiteralPath $out).Length -lt 100) { throw "Incomplete upstream notice: $url" }
        $extraNotices += [ordered]@{component=$component;url=$url;sha256=(Get-FileHash -LiteralPath $out -Algorithm SHA256).Hash}
    }
    if (-not (Test-Path -LiteralPath $directory) -or @(Get-ChildItem -LiteralPath $directory -Recurse -File).Count -eq 0) {
        throw "Missing upstream license materials: share/licenses/$component"
    }
}
Write-Utf8 (Join-Path $destination 'upstream-notices.json') (ConvertTo-Json -InputObject $extraNotices -Depth 4)
$sources = @($components | Sort-Object -Unique | ForEach-Object {
    [ordered]@{component=$_; recipe="https://gitlab.freedesktop.org/gstreamer/cerbero/-/blob/$version/recipes/$_.recipe"; sdk_version=$sdkVersion; exact_release=$exactRelease}
})
Write-Utf8 (Join-Path $destination 'native-components.json') (ConvertTo-Json -InputObject $inventory -Depth 5)
Write-Utf8 (Join-Path $destination 'source-recipes.json') (ConvertTo-Json -InputObject $sources -Depth 5)
Copy-Item -LiteralPath (Join-Path $repo 'docs/windows-licenses.md') -Destination (Join-Path $destination 'NOTICE.md')
# Record the exact source locator of every locked Rust crate and retain its legal
# files, including notices nested in bundled native sources and fonts.
$cargo = New-Object Diagnostics.ProcessStartInfo
$cargo.FileName = (Get-Command cargo -ErrorAction Stop).Source
$cargo.Arguments = 'metadata --locked --format-version 1 --filter-platform x86_64-pc-windows-msvc'
$cargo.WorkingDirectory = $repo
$cargo.UseShellExecute = $false
$cargo.RedirectStandardOutput = $true
$cargo.StandardOutputEncoding = New-Object Text.UTF8Encoding $false
$process = New-Object Diagnostics.Process
$process.StartInfo = $cargo
try {
    if (-not $process.Start()) { throw 'Cannot start cargo metadata.' }
    $metadata = $process.StandardOutput.ReadToEnd()
    $process.WaitForExit()
    if ($process.ExitCode -ne 0) { throw 'cargo metadata failed while collecting Rust licenses.' }
    $metadata = $metadata | ConvertFrom-Json
} finally { $process.Dispose() }
# A few crates omit workspace-level license texts from crates.io. Retrieve those
# exact texts at the published .cargo_vcs_info commit, never from a mutable branch.
$upstreamFiles = @{
    'AccessKit/accesskit' = @('LICENSE-MIT','LICENSE-APACHE')
    'emilk/egui' = @('LICENSE-MIT','LICENSE-APACHE')
    'DoumanAsh/clipboard-win' = @('LICENSE')
    'brendanzab/gl-rs' = @('LICENSE')
    'aclysma/profiling' = @('LICENSE-MIT','LICENSE-APACHE')
    'gfx-rs/rspirv' = @('LICENSE')
}
$downloaded = @{}
$rust = @()
foreach ($package in $metadata.packages) {
    if ($package.name -eq 'subdisc-client') { continue }
    $base = Split-Path -Parent $package.manifest_path
    $legal = @(Get-ChildItem -LiteralPath $base -Recurse -File | Where-Object {
        $_.Name -match '(?i)licen[cs]e|^(COPYING|NOTICE|COPYRIGHT|AUTHORS)([.\-_]|$)' -and $_.Extension -notin '.rs','.c','.h'
    })
    if ($package.license_file) {
        $legal += Get-Item -LiteralPath (Join-Path $base $package.license_file)
    }
    # Fonts carry additional notices with font-specific filenames.
    if ($package.name -eq 'epaint_default_fonts') {
        $legal += Get-ChildItem -LiteralPath (Join-Path $base 'fonts') -Filter '*.txt' -File
    }
    $outputBase = Join-Path $destination "rust/$($package.name)-$($package.version)"
    $null = New-Item -ItemType Directory -Force -Path $outputBase
    $extraSources = @()
    if ($package.repository -match '^https://github.com/([^/]+/[^/]+)') {
        $repository = $matches[1].TrimEnd('/').Replace('.git','')
        if ($upstreamFiles.ContainsKey($repository)) {
            $vcsFile = Join-Path $base '.cargo_vcs_info.json'
            Assert-File $vcsFile
            $commit = (Get-Content -LiteralPath $vcsFile -Raw -Encoding UTF8 | ConvertFrom-Json).git.sha1
            if ($commit -notmatch '^[a-f0-9]{40}$') { throw "Invalid source commit for $($package.name)" }
            foreach ($name in $upstreamFiles[$repository]) {
                $url = "https://raw.githubusercontent.com/$repository/$commit/$name"
                $target = Join-Path $outputBase "workspace-$name"
                if ($downloaded.ContainsKey($url)) {
                    Copy-Item -LiteralPath $downloaded[$url] -Destination $target
                } else {
                    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
                    Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $target
                    $downloaded[$url] = $target
                }
                if ((Get-Item -LiteralPath $target).Length -lt 100) { throw "Empty/incomplete upstream notice: $url" }
                $extraSources += $url
            }
        }
    }
    if ($legal.Count -eq 0 -and $extraSources.Count -eq 0) {
        throw "Crate $($package.name) $($package.version) has no bundled license text. Add an exact upstream source mapping before distributing."
    }
    foreach ($file in $legal | Sort-Object FullName -Unique) {
        $relative = Get-RelativePath $base $file.FullName
        $out = Join-Path $outputBase $relative
        $null = New-Item -ItemType Directory -Force -Path (Split-Path -Parent $out)
        Copy-Item -LiteralPath $file.FullName -Destination $out
    }
    $rust += [ordered]@{name=$package.name;version=$package.version;license=$package.license;source=$package.source;additional_notices=$extraSources}
}
Write-Utf8 (Join-Path $destination 'rust-components.json') (ConvertTo-Json -InputObject $rust -Depth 5)
