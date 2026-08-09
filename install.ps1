# vitrum installer (Windows, PowerShell 5.1 or newer).
#
# Downloads one published release archive, verifies it against the release
# `SHA256SUMS`, and installs `vitrum.exe` and `vitrum-server.exe` into a
# per-user directory. Nothing is installed unless the checksum matches.
#
#   .\install.ps1                     install the latest release
#   .\install.ps1 -Version 0.1.0      install a specific version
#   .\install.ps1 -InstallDir C:\bin  install somewhere else
#   .\install.ps1 -NoIntegrate        binaries only: no PATH, shortcut or `vu`
#
# Beyond the binaries, the installer puts the install directory on your user
# PATH, adds a Start menu shortcut, and defines `vu` as `vitrum update`. Each
# step is idempotent.
#
# Env overrides:
#   $env:VITRUM_VERSION       same as -Version
#   $env:VITRUM_INSTALL_DIR   same as -InstallDir
#   $env:VITRUM_NO_INTEGRATE  same as -NoIntegrate
#   $env:GITHUB_TOKEN         bearer token for the GitHub API

[CmdletBinding()]
param(
    [string]$Version = $env:VITRUM_VERSION,
    [string]$InstallDir = $(if ($env:VITRUM_INSTALL_DIR) { $env:VITRUM_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'vitrum\bin' }),
    [switch]$NoIntegrate = [bool]$env:VITRUM_NO_INTEGRATE
)

$ErrorActionPreference = 'Stop'
$Repo = 'santhreal/vitrum'

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

# ============================================================
# platform
# ============================================================

# `.github/workflows/release.yml` builds and uploads exactly one Windows
# target. ARM64 Windows has no asset, so it is told to build from source rather
# than sent to a URL that will 404.
# A 32-bit PowerShell on 64-bit Windows reports x86 here and puts the real
# architecture in PROCESSOR_ARCHITEW6432, so the machine is asked, not the host
# process.
$arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
if ($arch -ne 'AMD64') {
    Fail "no published release for Windows $arch" @(
        'Releases carry x86_64 Windows only.',
        'Build from source instead: https://github.com/santhreal/vitrum/blob/main/CONTRIBUTING.md'
    )
}
$Target = 'x86_64-pc-windows-msvc'

if (-not (Get-Command tar.exe -ErrorAction SilentlyContinue)) {
    Fail 'tar.exe was not found' @(
        'It ships with Windows 10 1803 and later.',
        "On an older Windows, unpack the archive by hand from https://github.com/$Repo/releases"
    )
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

# The tag carries a leading `v`; the version inside the asset name does not.
# Accepting either spelling is what keeps `-Version v0.1.0` and
# `-Version 0.1.0` from building two different URLs.
if ($Version) {
    $Version = $Version.TrimStart('v')
} else {
    Say "Resolving the latest release of $Repo."
    try {
        $release = Invoke-GitHubApi -Uri "https://api.github.com/repos/$Repo/releases/latest"
    } catch {
        Fail "could not reach the GitHub releases API: $($_.Exception.Message)" @(
            'Check your network, or pass an explicit version:',
            '  .\install.ps1 -Version 0.1.0',
            "Published versions are listed at https://github.com/$Repo/releases"
        )
    }
    if (-not $release.tag_name) {
        Fail 'the releases API returned no tag_name' @(
            '  .\install.ps1 -Version 0.1.0',
            "Published versions are listed at https://github.com/$Repo/releases"
        )
    }
    $Version = ([string]$release.tag_name).TrimStart('v')
}

$Archive = "vitrum-$Version-$Target.tar.gz"
$Base = "https://github.com/$Repo/releases/download/v$Version"

Say ''
Say "  version      v$Version"
Say "  target       $Target"
Say "  archive      $Archive"
Say "  install to   $InstallDir"
Say '  binaries     vitrum.exe, vitrum-server.exe'
Say ''

# ============================================================
# download and verify
# ============================================================

$work = Join-Path ([System.IO.Path]::GetTempPath()) ("vitrum-install-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work -Force | Out-Null

try {
    $archivePath = Join-Path $work $Archive
    $sumsPath = Join-Path $work 'SHA256SUMS'

    # PowerShell 5.1 defaults to TLS 1.0 against github.com, which fails the
    # handshake rather than the download.
    try {
        [Net.ServicePointManager]::SecurityProtocol =
            [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    } catch {
        # Newer runtimes negotiate this themselves and may not expose the enum.
    }

    Say "Downloading $Archive."
    try {
        Invoke-WebRequest -Uri "$Base/$Archive" -OutFile $archivePath -UseBasicParsing
    } catch {
        Fail "could not download $Base/$Archive" @(
            "Check that v$Version is published and carries an asset for ${Target}:",
            "  https://github.com/$Repo/releases/tag/v$Version"
        )
    }

    Say 'Downloading SHA256SUMS.'
    try {
        Invoke-WebRequest -Uri "$Base/SHA256SUMS" -OutFile $sumsPath -UseBasicParsing
    } catch {
        Fail "could not download $Base/SHA256SUMS" @(
            'Every vitrum release publishes it, so a release without one is',
            'incomplete and must not be installed. Report it at',
            "https://github.com/$Repo/issues"
        )
    }

    $expected = $null
    foreach ($line in (Get-Content -Path $sumsPath)) {
        if ($line -match "^([0-9a-fA-F]{64})\s+\*?$([regex]::Escape($Archive))\s*$") {
            $expected = $matches[1].ToLowerInvariant()
            break
        }
    }
    if (-not $expected) {
        Fail "SHA256SUMS has no entry for $Archive" @(
            'The release is inconsistent with its own checksum file and this',
            'installer will not install an unverified archive. Report it at',
            "https://github.com/$Repo/issues"
        )
    }

    $actual = (Get-FileHash -Path $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        Fail "checksum mismatch for $Archive; nothing was installed" @(
            "expected $expected",
            "actual   $actual",
            'Do not use this download. Retry, and if it fails again report it at',
            "https://github.com/$Repo/issues"
        )
    }
    Say 'Checksum verified.'

    # ============================================================
    # install
    # ============================================================

    try {
        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    } catch {
        Fail "could not create $InstallDir" @(
            'Pick a writable directory with -InstallDir PATH.'
        )
    }

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
    # talk. A running vitrum.exe holds its own image open, so an existing one is
    # renamed aside rather than overwritten.
    foreach ($name in $binaries) {
        $target = Join-Path $InstallDir $name
        if (Test-Path -PathType Leaf $target) {
            $displaced = "$target.old"
            Remove-Item -Force $displaced -ErrorAction SilentlyContinue
            try {
                Move-Item -Force -Path $target -Destination $displaced
            } catch {
                Fail "could not replace $target" @(
                    "Close any running vitrum, then run the installer again.",
                    'Or install elsewhere with -InstallDir PATH.'
                )
            }
        }
        try {
            Move-Item -Force -Path (Join-Path $work $name) -Destination $target
        } catch {
            Fail "could not install $name into $InstallDir" @(
                "Check permissions on $InstallDir, or use -InstallDir PATH."
            )
        }
    }

    Say "Installed vitrum.exe and vitrum-server.exe into $InstallDir."
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
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

    if (-not $onPath) {
        $user = [Environment]::GetEnvironmentVariable('Path', 'User')
        if ($null -eq $user) { $user = '' }
        $known = $user -split ';' | Where-Object {
            [string]::Equals($_.TrimEnd('\'), $InstallDir.TrimEnd('\'), [StringComparison]::OrdinalIgnoreCase)
        }
        if (-not $known) {
            $joined = if ($user.Length -gt 0) { "$user;$InstallDir" } else { $InstallDir }
            [Environment]::SetEnvironmentVariable('Path', $joined, 'User')
            Say "  user PATH"
        }
        # This process, so `vitrum --version` below works without a new shell.
        $env:Path = "$env:Path;$InstallDir"
    }

    try {
        $menu = [Environment]::GetFolderPath('Programs')
        $lnk = Join-Path $menu 'vitrum.lnk'
        $s = (New-Object -ComObject WScript.Shell).CreateShortcut($lnk)
        $s.TargetPath = Join-Path $InstallDir 'vitrum.exe'
        $s.WorkingDirectory = $InstallDir
        $s.Description = 'One interface for every agent TUI you have running'
        $s.Save()
        Say "  $lnk"
    } catch {
        Warn "could not write the Start menu shortcut: $($_.Exception.Message)"
    }

    # A function, not an alias: a PowerShell alias names a command and cannot
    # carry the `update` argument with it.
    try {
        if (-not (Test-Path $PROFILE)) {
            New-Item -ItemType File -Force -Path $PROFILE | Out-Null
        }
        if (-not (Select-String -Path $PROFILE -Pattern 'function vu' -Quiet)) {
            Add-Content $PROFILE "`n# vitrum`nfunction vu { vitrum update @args }"
            Say "  $PROFILE"
        }
    } catch {
        Warn "could not write $PROFILE : $($_.Exception.Message)"
    }
}

Say ''
Say "Run 'vitrum', or open it from the Start menu."
Say "Update with 'vitrum update', or 'vu' in a new terminal."
