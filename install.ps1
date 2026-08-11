# vitrum installer (Windows, PowerShell 5.1 or newer).
#
# Downloads one published release archive, verifies it against the release
# `SHA256SUMS`, and installs `vitrum.exe` and `vitrum-server.exe` into a
# per-user directory. Nothing is installed unless the checksum matches.
#
#   .\install.ps1                     install the latest release
#   .\install.ps1 -Version 0.1.0      install a specific version
#   .\install.ps1 -Channel nightly    install the current nightly build
#   .\install.ps1 -InstallDir C:\bin  install somewhere else
#   .\install.ps1 -NoIntegrate        binaries only: no PATH, shortcut or `vu`
#   .\install.ps1 -NoDeps             do not install the WebView2 runtime
#   .\install.ps1 -Uninstall          remove everything the installer wrote
#
# Beyond the binaries, the installer puts the install directory on your user
# PATH, adds a Start menu shortcut, and defines `vu` as `vitrum update`. Each
# step is idempotent.
#
# WebView2 is the one system dependency on Windows. A machine without it gets
# the Evergreen bootstrapper, downloaded from Microsoft and run unattended.
# `-NoDeps` turns that off and names the command to run by hand instead.
#
# Everything written is recorded in an install manifest, so -Uninstall removes
# exactly that and nothing else. A machine that has a proxy, no write
# permission, a running vitrum, a truncated download, an unsupported
# architecture or no WebView2 runtime it can install is told which of those it
# is, and the installer exits non-zero without installing half of anything.
#
# Env overrides:
#   $env:VITRUM_VERSION       same as -Version
#   $env:VITRUM_CHANNEL       same as -Channel
#   $env:VITRUM_INSTALL_DIR   same as -InstallDir
#   $env:VITRUM_BASE_URL      same as -BaseUrl
#   $env:VITRUM_NO_INTEGRATE  same as -NoIntegrate
#   $env:VITRUM_NO_DEPS       same as -NoDeps
#   $env:GITHUB_TOKEN         bearer token for the GitHub API

[CmdletBinding()]
param(
    [string]$Version = $env:VITRUM_VERSION,
    [ValidateSet('stable', 'nightly')]
    [string]$Channel = $(if ($env:VITRUM_CHANNEL) { $env:VITRUM_CHANNEL } else { 'stable' }),
    [string]$InstallDir = $(if ($env:VITRUM_INSTALL_DIR) { $env:VITRUM_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'vitrum\bin' }),
    [string]$BaseUrl = $env:VITRUM_BASE_URL,
    [switch]$NoIntegrate = [bool]$env:VITRUM_NO_INTEGRATE,
    [switch]$NoRuntimeCheck = [bool]$env:VITRUM_NO_RUNTIME_CHECK,
    [switch]$NoDeps = [bool]$env:VITRUM_NO_DEPS,
    [switch]$Uninstall
)

$ErrorActionPreference = 'Stop'
$Repo = 'santhreal/vitrum'
$DataRoot = if ($env:LOCALAPPDATA) { Join-Path $env:LOCALAPPDATA 'vitrum' } else { Join-Path $HOME '.vitrum' }
$Manifest = Join-Path $DataRoot 'install-manifest'
$BlockBegin = '# >>> vitrum >>>'
$BlockEnd = '# <<< vitrum <<<'
$Written = New-Object System.Collections.Generic.List[string]
# The one tag the nightly channel ever resolves. It moves to the commit each
# nightly was built from, holds one build at a time, and is marked prerelease
# so the latest-release lookup walks past it.
$NightlyTag = 'nightly'

# ============================================================
# output
# ============================================================

function Say { param([string]$Text) Write-Host $Text }
function Warn { param([string]$Text) Write-Host "warning: $Text" -ForegroundColor Yellow }

# Every failure leaves through here, so every failure names what to do next.
function Fail {
    param([string]$Message, [string[]]$Actions = @())
    Write-Host "error: $Message" -ForegroundColor Red
    foreach ($line in $Actions) { Write-Host "  $line" -ForegroundColor Red }
    exit 1
}

# A failure of the network path, which also names the proxy when there is one.
# A proxy is the commonest reason a download arrives empty, truncated, or as a
# sign-in page, and an operator who is not told it was in force goes looking at
# the release instead.
function FailNet {
    param([string]$Message, [string[]]$Actions = @())
    $extra = @()
    if ($script:Proxy) {
        $extra = @(
            "A proxy is in force: $script:Proxy",
            'It has to allow HTTPS to the download host. Clear it, or fetch the archive',
            'on a machine that can reach the host and install from a local copy with',
            '-BaseUrl file:///C:/path/to/assets'
        )
    }
    Fail $Message ($Actions + $extra)
}

# ============================================================
# install manifest
# ============================================================
#
# Uninstalling is not a list of paths in a document for you to retype. Every
# file the installer creates is recorded as it is created, including the icon
# files, whose names come from the binary rather than from this script.

function Record {
    param([string]$Kind, [string]$Path)
    $script:Written.Add("$Kind $Path")
}

function Save-Manifest {
    try {
        New-Item -ItemType Directory -Path (Split-Path -Parent $Manifest) -Force | Out-Null
        $lines = New-Object System.Collections.Generic.List[string]
        foreach ($l in $script:Written) { $lines.Add($l) }
        if (Test-Path $Manifest) {
            foreach ($old in (Get-Content -Path $Manifest)) {
                if (-not $old) { continue }
                if ($lines -contains $old) { continue }
                $p = $old.Substring($old.IndexOf(' ') + 1)
                if (Test-Path -LiteralPath $p) { $lines.Add($old) }
            }
        }
        Set-Content -Path $Manifest -Value $lines
    } catch {
        Warn "could not write $Manifest, so -Uninstall will fall back to the default layout"
    }
}

function Remove-Recorded {
    param([string]$Kind, [string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) { return }
    try {
        if ($Kind -eq 'tree') {
            Remove-Item -LiteralPath $Path -Recurse -Force
        } else {
            Remove-Item -LiteralPath $Path -Force
        }
        Say "  $Path"
        $script:Removed = $true
    } catch {
        Warn "could not remove ${Path}: $($_.Exception.Message)"
    }
}

# The block in the PowerShell profile, taken back whole. Everything outside the
# markers is written straight back, so a profile keeps its own contents.
function Remove-ProfileBlock {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) { return $false }
    $lines = @(Get-Content -LiteralPath $Path)
    if ($lines -notcontains $BlockBegin) { return $false }
    $kept = New-Object System.Collections.Generic.List[string]
    $skip = $false
    foreach ($line in $lines) {
        if ($line -eq $BlockBegin) { $skip = $true; continue }
        if ($line -eq $BlockEnd) { $skip = $false; continue }
        if (-not $skip) { $kept.Add($line) }
    }
    while ($kept.Count -gt 0 -and [string]::IsNullOrWhiteSpace($kept[$kept.Count - 1])) {
        $kept.RemoveAt($kept.Count - 1)
    }
    Set-Content -LiteralPath $Path -Value $kept
    return $true
}

# A profile this installer created holds nothing but the block it was created
# for, so once the block is gone the file is gone too. A profile that turned
# out to have something else in it is kept: someone put it there after the
# install.
function Remove-CreatedProfile {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) { return }
    if ((Get-Content -LiteralPath $Path -Raw) -match '\S') { return }
    Remove-Recorded 'file' $Path
}

# The install directory is taken off the user PATH only when it is the entry
# this installer put there. Every other entry is left exactly as it is.
function Remove-FromUserPath {
    param([string]$Dir)
    try {
        $user = [Environment]::GetEnvironmentVariable('Path', 'User')
        if (-not $user) { return $false }
        $entries = @($user -split ';' | Where-Object {
            -not [string]::Equals($_.TrimEnd('\'), $Dir.TrimEnd('\'), [StringComparison]::OrdinalIgnoreCase)
        })
        $joined = ($entries -join ';')
        if ($joined -eq $user) { return $false }
        [Environment]::SetEnvironmentVariable('Path', $joined, 'User')
        return $true
    } catch {
        Warn "could not edit your user PATH: $($_.Exception.Message)"
        return $false
    }
}

# ============================================================
# what is already running
# ============================================================

# The pid of a process running exactly the binary at $Path, or $null. The path
# is compared, not the name: an unrelated vitrum.exe from a source build is
# none of this installer's business.
function Running-Pid {
    param([string]$Path)
    try {
        $leaf = [IO.Path]::GetFileNameWithoutExtension($Path)
        foreach ($p in @(Get-Process -Name $leaf -ErrorAction SilentlyContinue)) {
            if ($p.Path -and [string]::Equals($p.Path, $Path, [StringComparison]::OrdinalIgnoreCase)) {
                return $p.Id
            }
        }
    } catch { }
    return $null
}

# The client is refused, the daemon is not.
#
# Windows will not let a running image be overwritten at all, and replacing the
# client under an open window would leave that window on the old build anyway.
# Quitting it costs nothing: the sessions belong to vitrum-server, which keeps
# running. Refusing while the daemon runs would mean no install could complete
# without ending every session on the machine, so its file is renamed aside
# instead and it is told plainly that it stays on the old code.
function Refuse-IfClientRunning {
    $client = Join-Path $InstallDir 'vitrum.exe'
    $running = Running-Pid $client
    if ($running) {
        Fail "vitrum is running from $client (pid $running)" @(
            'Quit the vitrum window, then run this again.',
            'Your sessions are not affected: they belong to vitrum-server, which this',
            'installer never stops.',
            'To leave the running copy alone, install elsewhere with -InstallDir PATH.'
        )
    }
}

# ============================================================
# uninstall
# ============================================================

if ($Uninstall) {
    Refuse-IfClientRunning
    $script:Removed = $false
    Say 'Removing vitrum.'
    if (Test-Path $Manifest) {
        foreach ($line in (Get-Content -Path $Manifest)) {
            if (-not $line) { continue }
            $i = $line.IndexOf(' ')
            if ($i -lt 1) { Warn "ignoring an unreadable manifest line: $line"; continue }
            $kind = $line.Substring(0, $i)
            $path = $line.Substring($i + 1)
            switch ($kind) {
                'file' { Remove-Recorded 'file' $path }
                'tree' { Remove-Recorded 'tree' $path }
                'profile' {
                    if (Remove-ProfileBlock $path) {
                        Say "  $path (vitrum block)"
                        $script:Removed = $true
                    }
                }
                'profile-created' {
                    if (Remove-ProfileBlock $path) {
                        Say "  $path (vitrum block)"
                        $script:Removed = $true
                    }
                    Remove-CreatedProfile $path
                }
                'path' {
                    if (Remove-FromUserPath $path) {
                        Say '  user PATH'
                        $script:Removed = $true
                    }
                }
                default { Warn "ignoring an unreadable manifest line: $line" }
            }
        }
        Remove-Item -LiteralPath $Manifest -Force -ErrorAction SilentlyContinue
    } else {
        Say "  no manifest at $Manifest, so this removes the default layout"
        foreach ($name in @('vitrum.exe', 'vitrum-server.exe', 'vitrum.exe.old', 'vitrum-server.exe.old')) {
            Remove-Recorded 'file' (Join-Path $InstallDir $name)
        }
        # The whole icon directory, not one file in it. `vitrum icons` writes
        # a theme tree beside the .ico, the names in it come from the binary
        # rather than from this script, and nothing else puts anything under
        # the vitrum data directory.
        Remove-Recorded 'tree' (Join-Path $DataRoot 'icons')
        try {
            $lnk = Join-Path ([Environment]::GetFolderPath('Programs')) 'vitrum.lnk'
            Remove-Recorded 'file' $lnk
        } catch { }
        if (Remove-ProfileBlock $PROFILE) { Say "  $PROFILE (vitrum block)"; $script:Removed = $true }
        if (Remove-FromUserPath $InstallDir) { Say '  user PATH'; $script:Removed = $true }
    }
    foreach ($dir in @((Join-Path $DataRoot 'icons'), $DataRoot, $InstallDir)) {
        try {
            if ((Test-Path -LiteralPath $dir) -and -not (Get-ChildItem -LiteralPath $dir -Force)) {
                Remove-Item -LiteralPath $dir -Force
            }
        } catch { }
    }
    if (-not $script:Removed) {
        Fail 'no vitrum install was found, so nothing was removed' @(
            "Looked for the manifest at $Manifest and for binaries in $InstallDir.",
            'If it is installed somewhere else, name it: -Uninstall -InstallDir PATH'
        )
    }
    $server = Running-Pid (Join-Path $InstallDir 'vitrum-server.exe')
    if ($server) {
        Say ''
        Warn 'vitrum-server is still running from the copy that was just removed.'
        Say '  It keeps its sessions until you stop it, and stopping it ends them.'
    }
    Say ''
    Say 'Config and state were left alone; they are listed in docs/configuration.md.'
    exit 0
}

# ============================================================
# platform
# ============================================================

# `.github/workflows/release.yml` builds and uploads exactly one Windows
# target. ARM64 Windows has no asset, so it is told what it is rather than sent
# to a URL that answers 404.
# A 32-bit PowerShell on 64-bit Windows reports x86 here and puts the real
# architecture in PROCESSOR_ARCHITEW6432, so the machine is asked, not the host
# process.
$arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
if ($arch -ne 'AMD64') {
    Fail "there is no published build for Windows on $arch" @(
        'Releases carry x86_64 Windows only, so no archive exists to download.',
        'On ARM64 Windows, build from source instead:',
        "https://github.com/$Repo/blob/main/CONTRIBUTING.md"
    )
}
$Target = 'x86_64-pc-windows-msvc'

if (-not (Get-Command tar.exe -ErrorAction SilentlyContinue)) {
    Fail 'tar.exe was not found, so the release archive cannot be unpacked' @(
        'It ships with Windows 10 1803 and later.',
        "On an older Windows, unpack the archive by hand from https://github.com/$Repo/releases"
    )
}

# ============================================================
# proxy
# ============================================================
#
# A proxy is not an error. Being behind one and not knowing it is.

$script:Proxy = $null
foreach ($name in @('HTTPS_PROXY', 'https_proxy', 'ALL_PROXY', 'HTTP_PROXY', 'http_proxy')) {
    $value = [Environment]::GetEnvironmentVariable($name)
    if (-not $value) { continue }
    if ($value -notmatch '^(https?|socks[45]h?)://') {
        Fail "$name is set to '$value', which is not a URL a proxy can be reached at" @(
            'It is read as scheme://host:port, so a bare host:port is treated as a',
            'hostname and every download fails with a name lookup.',
            "Set it as http://$value, or clear $name."
        )
    }
    $script:Proxy = "$name=$value"
    break
}
if (-not $script:Proxy) {
    try {
        $system = [System.Net.WebRequest]::GetSystemWebProxy().GetProxy('https://github.com')
        if ($system -and $system.AbsoluteUri -notlike 'https://github.com*') {
            $script:Proxy = "Windows proxy settings ($($system.AbsoluteUri))"
        }
    } catch { }
}

# ============================================================
# preflight
# ============================================================
#
# Everything that can be known before a byte is downloaded is checked before a
# byte is downloaded.

# A directory this installer can really write into, rather than one the ACL
# merely suggests it can.
function Assert-Writable {
    param([string]$Dir)
    if ((Test-Path -LiteralPath $Dir) -and -not (Test-Path -LiteralPath $Dir -PathType Container)) {
        Fail "$Dir exists and is not a directory" @(
            'Move it aside, or install somewhere else with -InstallDir PATH.'
        )
    }
    if (-not (Test-Path -LiteralPath $Dir)) {
        try {
            New-Item -ItemType Directory -Path $Dir -Force | Out-Null
        } catch {
            Fail "could not create $Dir" @(
                "Windows refused it: $($_.Exception.Message)",
                'Pick a writable directory with -InstallDir PATH.'
            )
        }
    }
    $probe = Join-Path $Dir ('.vitrum-write-test-' + [Guid]::NewGuid().ToString('N'))
    try {
        Set-Content -LiteralPath $probe -Value 'x'
        Remove-Item -LiteralPath $probe -Force
    } catch {
        Fail "$Dir cannot be written to" @(
            'The directory exists, and creating a file in it was refused: it belongs to',
            'another user, it is on a read-only volume, or a policy denies writing there.',
            'Pick a writable directory with -InstallDir PATH, or install for this user',
            "only into $(Join-Path $env:LOCALAPPDATA 'vitrum\bin')."
        )
    }
}

# WebView2 is vitrum's only system dependency on Windows. Windows 11 ships it
# and most Windows 10 machines have it through Edge, but a fresh image, a
# server SKU and an LTSC build do not, and without it the binary installs and
# then fails to open a window.
function Have-WebView2 {
    $clsid = '{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
    foreach ($key in @(
            "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\$clsid",
            "HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\$clsid",
            "HKCU:\Software\Microsoft\EdgeUpdate\Clients\$clsid")) {
        try {
            $pv = (Get-ItemProperty -Path $key -Name pv -ErrorAction Stop).pv
            if ($pv -and $pv -ne '0.0.0.0') { return $true }
        } catch { }
    }
    foreach ($root in @(${env:ProgramFiles(x86)}, $env:ProgramFiles)) {
        if (-not $root) { continue }
        $dir = Join-Path $root 'Microsoft\EdgeWebView\Application'
        if (Test-Path -LiteralPath $dir) { return $true }
    }
    return $false
}

# PowerShell 5.1 defaults to TLS 1.0 against microsoft.com and github.com,
# which fails the handshake rather than the download.
function Use-Tls12 {
    try {
        [Net.ServicePointManager]::SecurityProtocol =
            [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    } catch {
        # Newer runtimes negotiate this themselves and may not expose the enum.
    }
}

# Installs the WebView2 runtime with Microsoft's Evergreen bootstrapper, the
# same 2 MB installer `winget install Microsoft.EdgeWebView2Runtime` runs.
# Returns nothing on success, or a sentence saying what stopped it.
#
# It installs per user when this session is not elevated and machine wide when
# it is, and it asks for elevation itself if it wants it. Both register the
# runtime where Have-WebView2 looks, which is why the answer is read from the
# registry afterwards rather than taken from an exit status.
function Install-WebView2 {
    $url = 'https://go.microsoft.com/fwlink/p/?LinkId=2124703'
    $exe = Join-Path ([IO.Path]::GetTempPath()) `
        ('MicrosoftEdgeWebview2Setup-' + [Guid]::NewGuid().ToString('N') + '.exe')
    Say "  downloading $url"
    try {
        Invoke-WebRequest -Uri $url -OutFile $exe -UseBasicParsing -TimeoutSec 300
    } catch {
        return "it could not be downloaded: $($_.Exception.Message)"
    }
    try {
        Say '  MicrosoftEdgeWebview2Setup.exe /silent /install'
        $p = Start-Process -FilePath $exe -ArgumentList '/silent', '/install' -Wait -PassThru
        if ($p.ExitCode -ne 0) {
            return "the bootstrapper exited $($p.ExitCode)"
        }
    } catch {
        return "the bootstrapper could not be run: $($_.Exception.Message)"
    } finally {
        Remove-Item -LiteralPath $exe -Force -ErrorAction SilentlyContinue
    }
    return $null
}

Assert-Writable $InstallDir
Refuse-IfClientRunning
Use-Tls12

if (-not $NoRuntimeCheck -and -not (Have-WebView2)) {
    if ($NoDeps) {
        Fail 'vitrum needs the WebView2 runtime and this machine has none' @(
            'It is vitrum''s only system dependency on Windows, and without it the',
            'binary installs and then fails to open a window.',
            'Install it first:',
            '  winget install Microsoft.EdgeWebView2Runtime',
            'or download the Evergreen Runtime from',
            '  https://go.microsoft.com/fwlink/p/?LinkId=2124703',
            'Then run this installer again.',
            'To install anyway, for an image that adds the runtime separately, pass',
            '-NoRuntimeCheck.'
        )
    }
    Say 'vitrum needs the WebView2 runtime, and this machine has none.'
    $problem = Install-WebView2
    if ($problem) {
        Fail "the WebView2 runtime could not be installed: $problem" @(
            'Nothing of vitrum was installed.',
            'Install the runtime yourself, then run this installer again:',
            '  winget install Microsoft.EdgeWebView2Runtime',
            'or download the Evergreen Runtime from',
            '  https://go.microsoft.com/fwlink/p/?LinkId=2124703',
            'To install anyway, for an image that adds the runtime separately, pass',
            '-NoRuntimeCheck.'
        )
    }
    if (-not (Have-WebView2)) {
        Fail 'the WebView2 bootstrapper exited zero and the runtime is not registered' @(
            'It reported success and left nothing behind that vitrum can use, so a',
            'policy on this machine removed it again or the install went somewhere',
            'this installer does not look. Nothing of vitrum was installed.',
            'Install the runtime yourself, then run this installer again:',
            '  winget install Microsoft.EdgeWebView2Runtime',
            'To install anyway, pass -NoRuntimeCheck.'
        )
    }
    Say '  the WebView2 runtime is installed'
    Say ''
}

# A re-install is normal and is stated, so the operator knows the version they
# are leaving as well as the one they are getting.
$Previous = ''
$clientPath = Join-Path $InstallDir 'vitrum.exe'
if (Test-Path -LiteralPath $clientPath) {
    try {
        $Previous = (& $clientPath --version 2>$null | Select-Object -First 1)
    } catch { }
    if (-not $Previous) { $Previous = 'an unreadable build' }
}

# ============================================================
# version
# ============================================================

function Invoke-GitHubApi {
    param([string]$Uri)
    $headers = @{}
    if ($env:GITHUB_TOKEN) {
        $headers['Authorization'] = "Bearer $env:GITHUB_TOKEN"
        $headers['X-GitHub-Api-Version'] = '2022-11-28'
    }
    if ($headers.Count -gt 0) {
        return Invoke-RestMethod -Uri $Uri -Headers $headers -UseBasicParsing
    }
    return Invoke-RestMethod -Uri $Uri -UseBasicParsing
}

$BaseUrl = $BaseUrl.TrimEnd('/')

# A nightly is whatever the moving `nightly` tag holds right now. Naming a
# version or a mirror alongside it asks for two different builds at once, and
# the installer would have to pick one silently.
if ($Channel -eq 'nightly') {
    if ($Version) {
        Fail "-Channel nightly and version $Version name two different builds" @(
            'A nightly has no version to ask for: it is whatever the nightly tag',
            'holds now. Install the nightly with ".\install.ps1 -Channel nightly",',
            "or that exact version with `".\install.ps1 -Version $Version`"."
        )
    }
    if ($BaseUrl) {
        Fail '-Channel nightly and -BaseUrl name two different builds' @(
            'A mirror holds the archives that were copied into it, so install from',
            "it by version: `".\install.ps1 -Version X.Y.Z -BaseUrl $BaseUrl`"."
        )
    }
}

# The tag carries a leading `v`; the version inside the asset name does not.
# Accepting either spelling is what keeps `-Version v0.1.0` and
# `-Version 0.1.0` from building two different URLs.
if ($Channel -eq 'nightly') {
    # Resolved from the release's own checksum file, below.
    $Version = $null
} elseif ($Version) {
    $Version = $Version.TrimStart('v')
} elseif ($BaseUrl) {
    Fail '-BaseUrl needs an explicit version' @(
        'A mirror has no releases API to ask, so the version cannot be resolved.',
        "Pass it: .\install.ps1 -Version 0.1.0 -BaseUrl $BaseUrl"
    )
} else {
    Say "Resolving the latest release of $Repo."
    $Version = $null
    try {
        $release = Invoke-GitHubApi -Uri "https://api.github.com/repos/$Repo/releases/latest"
        if ($release.tag_name) { $Version = ([string]$release.tag_name).TrimStart('v') }
    } catch {
        Say "  the releases API did not answer: $($_.Exception.Message)"
    }
    # The website's redirect, when the API did not answer. Its anonymous rate
    # limit is per address and is spent by everything behind that address, so
    # on a shared one — a CI runner, an office, a carrier NAT — a working
    # network still gets HTTP 403 here. `releases/latest` on github.com
    # redirects to the tag it resolves to and is not the resource that ran
    # out, and the tag is the whole answer this needs.
    if (-not $Version) {
        try {
            $head = Invoke-WebRequest -Uri "https://github.com/$Repo/releases/latest" `
                -UseBasicParsing -MaximumRedirection 5 -Method Head
            $url = $head.BaseResponse.RequestMessage.RequestUri.AbsoluteUri
            if ($url -match '/releases/tag/v?(.+)$') { $Version = $Matches[1] }
        } catch {
            Say "  the releases page did not answer either: $($_.Exception.Message)"
        }
    }
    if (-not $Version) {
        FailNet 'could not resolve the latest release' @(
            'Neither the releases API nor the redirect on the releases page',
            'answered. Check your network, or pass an explicit version:',
            '  .\install.ps1 -Version 0.1.0',
            "Published versions are listed at https://github.com/$Repo/releases"
        )
    }
}

$Base = $null
$Archive = $null
if ($Channel -eq 'nightly') {
    $Base = "https://github.com/$Repo/releases/download/$NightlyTag"
} else {
    $Archive = "vitrum-$Version-$Target.tar.gz"
    $Base = if ($BaseUrl) { $BaseUrl } else { "https://github.com/$Repo/releases/download/v$Version" }
}

# ============================================================
# download and verify
# ============================================================

# `file://` is copied rather than fetched: a mirror on a local disk or a share
# is the only way an air-gapped host installs.
function Get-Asset {
    param([string]$Uri, [string]$OutFile)
    if ($Uri -match '^file://') {
        $local = [Uri]::new($Uri).LocalPath
        Copy-Item -LiteralPath $local -Destination $OutFile -Force
        return
    }
    Invoke-WebRequest -Uri $Uri -OutFile $OutFile -UseBasicParsing -TimeoutSec 120
}

# Why a shape check when there is a digest below: because "checksum mismatch"
# is the wrong answer to "the transfer stopped half way" and to "a captive
# portal sent you its sign-in page". Both are common, neither is a bad release,
# and each has a different thing for the operator to do.
function Archive-Shape {
    param([string]$Path)
    $len = (Get-Item -LiteralPath $Path).Length
    if ($len -eq 0) { return 'it is empty (0 bytes)' }
    $head = New-Object byte[] 512
    $read = 0
    $fs = [IO.File]::OpenRead($Path)
    try { $read = $fs.Read($head, 0, [Math]::Min(512, [int][Math]::Min($len, 512))) } finally { $fs.Dispose() }
    if ($read -lt 2 -or $head[0] -ne 0x1f -or $head[1] -ne 0x8b) {
        $text = [Text.Encoding]::ASCII.GetString($head, 0, $read)
        if ($text -match '(?i)<html|<!doctype|<title') {
            return "it is a web page, not an archive ($len bytes)"
        }
        return ("it is not a gzip archive ({0} bytes, first bytes {1:x2}{2:x2})" -f $len, $head[0], $head[1])
    }
    & tar.exe -tzf $Path *> $null
    if ($LASTEXITCODE -ne 0) {
        return "it is truncated: the archive ends part way through ($len bytes)"
    }
    return $null
}

$work = Join-Path ([System.IO.Path]::GetTempPath()) ("vitrum-install-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work -Force | Out-Null
$sumsPath = Join-Path $work 'SHA256SUMS'

# The release's checksum file, refused unless it is one. Both channels need
# it, and the nightly channel needs it before it knows what to download.
function Get-Sums {
    Say 'Downloading SHA256SUMS.'
    try {
        Get-Asset -Uri "$Base/SHA256SUMS" -OutFile $sumsPath
    } catch {
        FailNet "could not download $Base/SHA256SUMS" @(
            'Every vitrum release publishes it, so a release without one is',
            'incomplete and must not be installed. Report it at',
            "https://github.com/$Repo/issues"
        )
    }
    $lines = @(Get-Content -Path $sumsPath)
    if ($lines.Count -eq 0 -or $lines[0] -notmatch '^[0-9a-fA-F]{64}[ *]') {
        FailNet 'what came back for SHA256SUMS is not a checksum file' @(
            'Its first line is not a digest and a filename, so something answered on the',
            "release's behalf: a proxy, a captive portal, or a sign-in page.",
            'Nothing was installed.'
        )
    }
    return $lines
}

# The nightly tag names no version, so the version is read out of the release:
# its SHA256SUMS carries one line per platform, and the line for this target
# names the archive and with it the build. Reading it from the checksum file
# means the name that is downloaded and the name that is verified come from
# one source.
function Resolve-Nightly {
    $lines = Get-Sums
    foreach ($line in $lines) {
        if ($line -match "^[0-9a-fA-F]{64}\s+\*?(vitrum-.+-$([regex]::Escape($Target))\.tar\.gz)\s*$") {
            $script:Archive = $matches[1]
            $script:Version = $script:Archive.Substring(7, $script:Archive.Length - 7 - ($Target.Length + 8))
            return $lines
        }
    }
    Fail "the nightly release has no archive for $Target" @(
        'Its SHA256SUMS lists: ' + (($lines | ForEach-Object { ($_ -split '\s+')[-1] }) -join ' '),
        "Report it at https://github.com/$Repo/issues, or install a stable",
        'release instead: .\install.ps1'
    )
}

$sums = $null
try {
    if ($Channel -eq 'nightly') {
        Say "Resolving the current nightly build of $Repo."
        $sums = Resolve-Nightly
    }

    Say ''
    Say "  channel      $Channel"
    Say "  version      v$Version"
    Say "  target       $Target"
    Say "  archive      $Archive"
    Say "  install to   $InstallDir"
    Say '  binaries     vitrum.exe, vitrum-server.exe'
    if ($Previous) { Say "  replacing    $Previous" }
    if ($script:Proxy) { Say "  proxy        $script:Proxy" }
    Say ''

    $archivePath = Join-Path $work $Archive

    Say "Downloading $Archive."
    try {
        Get-Asset -Uri "$Base/$Archive" -OutFile $archivePath
    } catch {
        # One re-resolve, on the nightly channel only, because that is the one
        # channel whose release is replaced while people are installing from
        # it. Every build has its own archive name, so a nightly published
        # between the checksum file and the archive takes the name that was
        # just resolved with it.
        if ($Channel -eq 'nightly') {
            Say 'That archive is gone; a new nightly landed while this was running.'
            $sums = Resolve-Nightly
            $archivePath = Join-Path $work $Archive
            Say "Downloading $Archive."
            try {
                Get-Asset -Uri "$Base/$Archive" -OutFile $archivePath
            } catch {
                FailNet "could not download $Base/$Archive" @(
                    "Check that the release is published and carries an asset for ${Target}:",
                    "  https://github.com/$Repo/releases"
                )
            }
        } else {
            FailNet "could not download $Base/$Archive" @(
                "Check that v$Version is published and carries an asset for ${Target}:",
                "  https://github.com/$Repo/releases/tag/v$Version"
            )
        }
    }

    $shape = Archive-Shape $archivePath
    if ($shape) {
        FailNet "the download of $Archive did not arrive intact: $shape" @(
            'Nothing was installed.',
            'This is the transfer, not the release: retry, and if it keeps stopping at',
            'the same size, something between you and the download host is cutting the',
            'connection.'
        )
    }

    # Already in hand on the nightly channel, which had to read it to learn
    # what to download. Fetching it twice would also let the two halves come
    # from different builds of a tag that moves.
    if (-not $sums) { $sums = Get-Sums }
    $expected = $null
    foreach ($line in $sums) {
        if ($line -match "^([0-9a-fA-F]{64})\s+\*?$([regex]::Escape($Archive))\s*$") {
            $expected = $matches[1].ToLowerInvariant()
            break
        }
    }
    if (-not $expected) {
        Fail "SHA256SUMS has no entry for $Archive" @(
            'The release is inconsistent with its own checksum file and this',
            'installer will not install an unverified archive. Nothing was installed.',
            "Report it at https://github.com/$Repo/issues, or install a version whose",
            'checksum file lists its own archive: .\install.ps1 -Version X.Y.Z'
        )
    }

    $actual = (Get-FileHash -Path $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        Fail "checksum mismatch for $Archive; nothing was installed" @(
            "expected $expected",
            "actual   $actual",
            'The archive is intact but is not the file this release published, so it was',
            'changed on the way here. Do not use this download. Retry, and if it fails',
            "again report it at https://github.com/$Repo/issues"
        )
    }
    Say 'Checksum verified.'

    # ============================================================
    # install
    # ============================================================

    & tar.exe -xzf $archivePath -C $work
    if ($LASTEXITCODE -ne 0) {
        Fail "could not unpack $Archive" @(
            'The archive verified, so this is a tar problem, not a corrupt download.'
        )
    }

    $binaries = @('vitrum.exe', 'vitrum-server.exe')
    foreach ($name in $binaries) {
        if (-not (Test-Path -PathType Leaf (Join-Path $work $name))) {
            Fail "$Archive does not contain $name" @(
                'The release archive is incomplete; both binaries ship together.',
                "Report it at https://github.com/$Repo/issues"
            )
        }
    }

    # Both binaries move in one pass. The client and the daemon speak a
    # versioned protocol, so a half-finished install is a pair that refuses to
    # talk. A running vitrum-server.exe holds its own image open, so an
    # existing one is renamed aside rather than overwritten, and the file it is
    # renamed to is recorded so the uninstaller takes it away too.
    foreach ($name in $binaries) {
        $target = Join-Path $InstallDir $name
        if (Test-Path -PathType Leaf $target) {
            $displaced = "$target.old"
            Remove-Item -Force $displaced -ErrorAction SilentlyContinue
            try {
                Move-Item -Force -Path $target -Destination $displaced
                Record 'file' $displaced
            } catch {
                Fail "could not replace $target" @(
                    'It is running, or another process is holding it open.',
                    'Close any running vitrum, then run the installer again.',
                    'Or install elsewhere with -InstallDir PATH.'
                )
            }
        }
        try {
            Move-Item -Force -Path (Join-Path $work $name) -Destination $target
            Record 'file' $target
        } catch {
            Fail "could not install $name into $InstallDir" @(
                "Windows refused it: $($_.Exception.Message)",
                "Check permissions on $InstallDir, or use -InstallDir PATH."
            )
        }
    }

    if ($Previous) {
        Say "Replaced $Previous with vitrum $Version in $InstallDir."
    } else {
        Say "Installed vitrum.exe and vitrum-server.exe into $InstallDir."
    }
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

$serverPid = Running-Pid (Join-Path $InstallDir 'vitrum-server.exe')
if ($serverPid) {
    Warn "vitrum-server (pid $serverPid) is still running the previous build."
    Say '  Its sessions are unaffected. It takes the new build when it is next'
    Say '  restarted, and restarting it ends every session it holds, so do that'
    Say '  when the agents are idle.'
}

# ============================================================
# desktop integration
# ============================================================
#
# The installer finishes the job: PATH, a Start menu shortcut, and `vu` as
# `vitrum update`. Every step is idempotent and each is skipped by
# -NoIntegrate.

$onPath = $false
foreach ($entry in ($env:Path -split ';')) {
    if ([string]::IsNullOrWhiteSpace($entry)) { continue }
    if ([string]::Equals($entry.TrimEnd('\'), $InstallDir.TrimEnd('\'), [StringComparison]::OrdinalIgnoreCase)) {
        $onPath = $true
        break
    }
}

if ($NoIntegrate) {
    if (-not $onPath) {
        Warn "$InstallDir is not on your PATH, so 'vitrum' will not be found."
    }
} else {
    Say ''
    Say 'Setting up.'

    try {
        $user = [Environment]::GetEnvironmentVariable('Path', 'User')
        if ($null -eq $user) { $user = '' }
        $known = $user -split ';' | Where-Object {
            [string]::Equals($_.TrimEnd('\'), $InstallDir.TrimEnd('\'), [StringComparison]::OrdinalIgnoreCase)
        }
        if (-not $known) {
            $joined = if ($user.Length -gt 0) { "$user;$InstallDir" } else { $InstallDir }
            [Environment]::SetEnvironmentVariable('Path', $joined, 'User')
            Say '  user PATH'
        }
        Record 'path' $InstallDir
    } catch {
        Warn "could not edit your user PATH: $($_.Exception.Message)"
    }
    if (-not $onPath) {
        # This process, so `vitrum --version` below works without a new shell.
        $env:Path = "$env:Path;$InstallDir"
    }

    # The icon is drawn by the binary rather than shipped beside it: the
    # release archive carries vitrum.exe and vitrum-server.exe and nothing
    # else, and there is no converter on the machine. `vitrum icons` writes a
    # multi-size .ico from the mark's geometry, so the Start menu entry and
    # every shortcut pinned from it stop showing the generic placeholder.
    #
    # Idempotent: the same paths are overwritten on every install. The paths it
    # prints are the ones recorded, so the uninstaller removes the set this
    # build wrote rather than a list of names copied into this script.
    $ico = $null
    try {
        $iconRoot = $DataRoot
        $emitted = @(& (Join-Path $InstallDir 'vitrum.exe') icons $iconRoot)
        foreach ($path in $emitted) {
            if ($path -and (Test-Path -LiteralPath $path)) { Record 'file' $path }
        }
        $candidate = Join-Path $iconRoot 'icons\vitrum.ico'
        if (Test-Path $candidate) {
            $ico = $candidate
            Say "  $candidate"
        }
    } catch {
        Warn "could not write the icon set: $($_.Exception.Message)"
    }

    try {
        $menu = [Environment]::GetFolderPath('Programs')
        $lnk = Join-Path $menu 'vitrum.lnk'
        $s = (New-Object -ComObject WScript.Shell).CreateShortcut($lnk)
        $s.TargetPath = Join-Path $InstallDir 'vitrum.exe'
        $s.WorkingDirectory = $InstallDir
        $s.Description = 'One interface for every agent TUI you have running'
        # Only when the file is really there. A shortcut whose IconLocation
        # names a missing file shows a broken-document glyph, which is worse
        # than the generic executable icon Windows would have used.
        if ($ico) { $s.IconLocation = "$ico,0" }
        $s.Save()
        Record 'file' $lnk
        Say "  $lnk"
    } catch {
        Warn "could not write the Start menu shortcut: $($_.Exception.Message)"
    }

    # A function, not an alias: a PowerShell alias names a command and cannot
    # carry the `update` argument with it. Written inside a marked block, so
    # -Uninstall can take back these lines and no others.
    try {
        # A profile that did not exist is recorded as created rather than
        # edited, so uninstalling takes it away instead of leaving an empty
        # file behind that nobody put there.
        $profileExisted = Test-Path $PROFILE
        if (-not $profileExisted) {
            New-Item -ItemType File -Force -Path $PROFILE | Out-Null
        }
        Remove-ProfileBlock $PROFILE | Out-Null
        Add-Content $PROFILE "`n$BlockBegin`nfunction vu { vitrum update @args }`n$BlockEnd"
        if ($profileExisted) {
            Record 'profile' $PROFILE
        } else {
            Record 'profile-created' $PROFILE
        }
        Say "  $PROFILE"
    } catch {
        Warn "could not write $PROFILE : $($_.Exception.Message)"
    }
}

Save-Manifest

if ($NoRuntimeCheck -and -not (Have-WebView2)) {
    Warn 'the WebView2 runtime is still missing, so vitrum will not open a window.'
    Say '  winget install Microsoft.EdgeWebView2Runtime'
}

Say ''
Say "Run 'vitrum', or open it from the Start menu."
Say "Update with 'vitrum update', or 'vu' in a new terminal."
Say "Remove it with '.\install.ps1 -Uninstall', or without a copy of this file:"
Say "  & ([scriptblock]::Create((irm https://raw.githubusercontent.com/$Repo/main/install.ps1))) -Uninstall"
