# Installing, updating, removing

## One command

Linux and macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/santhreal/vitrum/main/install.sh | sh
```

Windows, in PowerShell:

```powershell
irm https://raw.githubusercontent.com/santhreal/vitrum/main/install.ps1 | iex
```

Nothing has to be installed first. The installer adds what the build needs on
the machine it is running on, and refuses only when it cannot.

On a container image that ships no downloader, fetch one in the same line:

```sh
command -v curl >/dev/null || { apt-get update && apt-get install -y curl; } || dnf install -y curl || apk add curl; curl -fsSL https://raw.githubusercontent.com/santhreal/vitrum/main/install.sh | sh
```

A checkout needs no downloader at all. `sh install.sh` installs curl for
itself when the machine has neither curl nor wget.

## Published builds

| Platform | Target |
|---|---|
| Linux, 64-bit x86 | `x86_64-unknown-linux-gnu` |
| Linux, 64-bit ARM | `aarch64-unknown-linux-gnu` |
| macOS, Apple silicon | `aarch64-apple-darwin` |
| macOS, Intel | `x86_64-apple-darwin` |
| Windows, 64-bit x86 | `x86_64-pc-windows-msvc` |

Anything else is told there is no build for it and pointed at a source build.
`tools/release/targets.sh check` fails if this table, the release matrix and
the two installers stop agreeing.

## System dependencies

vitrum draws its windows with a WebView, and the installer installs the one
this machine is missing rather than naming it and stopping.

| Platform | What it installs | Command |
|---|---|---|
| Debian, Ubuntu, Mint, Pop, Raspbian | `libwebkit2gtk-4.1-0` | `apt-get install -y` |
| Fedora, RHEL, Rocky, Alma | `webkit2gtk4.1` | `dnf install -y` |
| Arch, Manjaro, EndeavourOS | `webkit2gtk-4.1` | `pacman -S --noconfirm --needed` |
| openSUSE, SLES | `libwebkit2gtk-4_1-0` | `zypper -n in` |
| Alpine | `webkit2gtk-4.1` | `apk add` |
| Void | `webkit2gtk` | `xbps-install -Sy` |
| Gentoo | `net-libs/webkit-gtk:4.1` | `emerge` |
| NixOS | `nixpkgs.webkitgtk_4_1` | `nix-env -iA` |
| Windows | WebView2 Evergreen runtime | the Microsoft bootstrapper, `/silent /install` |
| macOS | nothing | |

Each command is printed before it runs. `sudo` is prefixed only when the
installer is not already root, and sudo reads its password from the terminal,
so it works inside `curl ... | sh`. NixOS installs into a user profile and
uses no root.

Afterwards the library is looked for again. A package manager that exits zero
and leaves nothing behind is a failed install, not a finished one.

The refusals that remain:

| Condition | What it says |
|---|---|
| not root, and no sudo | `This is not root and there is no sudo on this machine, so no package can be installed from here.` |
| a distribution with no entry above | `No package on this distribution is known to provide it.` |
| the package manager exits non-zero | `the package manager could not install <package>`, with its status |
| the package installs and the library is still absent | `<package> installed and libwebkit2gtk-4.1.so.0 is still not here` |

Pass `--no-deps` (`-NoDeps`) to install nothing. The installer then prints the
command that installs the runtime and exits non-zero, which is what a
provisioning script that owns its own packages wants.

The published Linux build links other shared libraries as well, and needs a C
library new enough for the symbol versions it references. No version is
written down here, because the installer reads both from the archive it just
downloaded: it resolves every library the build links against this machine's
loader, installs the packages carrying the ones that are missing, and reads
the loader again. A build that stops linking something stops needing it, and
a build that starts linking something new is handled the first time anyone
installs it.

A C library older than the build needs cannot be fixed by installing
anything: it comes with the distribution release. That failure names the
version the build requires, the version this machine has, and the two things
that resolve it.

Published x86-64 builds target the base instruction set, so they run on any
x86-64 processor. They do not use AVX2 or AVX-512 even where the processor
has them. A build you make yourself may target your own processor and fail
with `SIGILL` on an older one.

Published Linux builds link glibc. A musl host, such as Alpine or Void, is
told so and pointed at a source build, rather than being handed an archive
whose loader it does not have.

## The display server

The terminal pane is a GPU surface created on the pane widget's own X window,
so on Linux vitrum runs under X11. Under a Wayland compositor, run it through
Xwayland. There is no Wayland path yet.

The surface is created through wgpu, which uses Vulkan on Linux, Metal on
macOS and Direct3D 12 on Windows. A machine with no GPU driver falls back to
that platform's software adapter. The installer does not install a driver: a
machine that cannot create a surface reports which backend it tried rather
than opening a window with no pane in it.

## macOS

The archive is fetched with curl and unpacked with tar, and neither marks
what it writes with `com.apple.quarantine`. An archive that arrives some
other way carries the mark and passes it to everything unpacked out of it, so
the installer reads the mark rather than assuming it, clears it when it is
there, and says so.

It then runs the installed binary once. macOS refuses a binary it will not
run by killing it rather than by printing anything, so asking for
`--version` is the only way to find out. When that fails the installer names
the exact commands that clear the mark by hand and try again.

## What the installer does

1. Checks this machine before downloading anything: the architecture and libc
   have a published build, the install directory can really be written to, no
   `vitrum` is running from it, there is something to download with, and the
   WebKit or WebView2 runtime is present or can be installed.
2. Resolves the latest published release.
3. Downloads the platform archive and the release `SHA256SUMS`.
4. Checks the archive arrived whole, then compares digests. On a mismatch it
   stops, having installed nothing.
5. Checks the downloaded build against this machine: every shared library it
   links can be resolved, and the C library is new enough for the symbol
   versions it references. Missing libraries are installed and the check is
   repeated. This runs before anything is written, so a machine that cannot
   run the build keeps the copy it already had.
6. Places `vitrum` and `vitrum-server` in the install directory.
7. Adds that directory to `PATH`: in `~/.profile`, in `~/.bash_profile` when
   bash has one, and in the rc of every shell you have among bash, zsh and
   fish. On Windows, in the user `Path` environment variable.
8. Adds a launcher entry: a `.desktop` file on Linux, an app bundle in
   `~/Applications` on macOS, a Start menu shortcut on Windows.
9. Defines `vu` as `vitrum update`.
10. Records every file it wrote, so `--uninstall` can remove exactly those.

Steps 7 to 9 are idempotent. Re-running the installer rewrites nothing it
already wrote: the shell edits live in one marked block per file, and a
re-install replaces that block rather than adding a second one.

Each shell gets its own syntax. bash and zsh get a guarded `export PATH`,
fish gets `set -gx PATH` inside `if not contains`, and neither grows your
`PATH` by an entry per nested shell.

An rc file that refuses the edit is a warning, not a failed install. bash
reads one login file and stops, so when `~/.profile` refuses the write and
there is no `~/.bash_profile`, the installer writes `~/.bash_profile`
instead. That file sources `~/.profile` first, so shadowing it changes
nothing, and it is recorded as created, so `--uninstall` takes it away. When
no login file takes the entry at all, the installer says so at the end and
prints the line to add.

## Verifying where a build came from

The digest check says an archive is the file the release published. It says
nothing about who published it. Every release archive is also signed with the
identity of the workflow that built it, and GitHub stores the attestation:

```sh
gh attestation verify vitrum-0.3.0-x86_64-unknown-linux-gnu.tar.gz \
  --repo santhreal/vitrum
```

It reports the repository, the workflow and the commit the archive was built
from. An archive that was not built by that workflow fails, whatever its
digest says.

## What gets written, and where

| Path | What |
|---|---|
| `~/.local/bin/vitrum`, `~/.local/bin/vitrum-server` | the binaries, Linux and macOS |
| `%LOCALAPPDATA%\vitrum\bin\vitrum.exe`, `vitrum-server.exe` | the binaries, Windows |
| `~/.local/share/icons/hicolor/*/apps/vitrum.png` | the icon set, Linux |
| `~/.local/share/applications/vitrum.desktop` | the launcher entry, Linux |
| `~/Applications/vitrum.app` | the app bundle, macOS |
| `%LOCALAPPDATA%\vitrum\icons\` | the icon set, Windows |
| Start menu `vitrum.lnk` | the launcher entry, Windows |
| `~/.profile`, `~/.bashrc`, `~/.zshrc`, `config.fish` | one marked block each, `PATH` and `vu` |
| `$PROFILE` | one marked block, `vu`, Windows |
| user `Path` | the install directory, Windows |
| `~/.local/share/vitrum/install-manifest` | every path above, as it is written |
| `%LOCALAPPDATA%\vitrum\install-manifest` | the same, Windows |

The install directory is `--install-dir=PATH` or `VITRUM_INSTALL_DIR`.

The system packages the installer installs are not recorded and are not
removed by `--uninstall`. A WebKit runtime and the WebView2 runtime are
shared with everything else on the machine.

## Options

```sh
sh install.sh --help
```

| Option | Environment | Effect |
|---|---|---|
| `VERSION` | `VITRUM_VERSION` | install that version instead of the latest |
| `--install-dir=PATH` | `VITRUM_INSTALL_DIR` | put the binaries elsewhere |
| `--base-url=URL` | `VITRUM_BASE_URL` | take the archive and `SHA256SUMS` from a mirror or a local directory |
| `--no-integrate` | `VITRUM_NO_INTEGRATE` | binaries only: skip steps 7 to 9 |
| `--no-deps` | `VITRUM_NO_DEPS` | install no system packages; print the command and stop |
| `--no-runtime-check` | `VITRUM_NO_RUNTIME_CHECK` | install without checking the runtime this machine has |
| `--uninstall` | | remove everything the installer wrote |

PowerShell takes the same as `-Version`, `-InstallDir`, `-BaseUrl`,
`-NoIntegrate`, `-NoDeps`, `-NoRuntimeCheck` and `-Uninstall`.

`--no-integrate` is for images, provisioning scripts and headless hosts, where
a `PATH` edit in a home directory and a launcher entry on a machine with no
desktop are both wrong.

`--base-url` is for a host that cannot reach GitHub. Copy the release archive
and its `SHA256SUMS` into one directory, then name it. The digest is still
checked, so a mirror is trusted no further than the release is:

```sh
sh install.sh 1.2.3 --base-url=file:///srv/vitrum
```

With a `file://` base the installer needs no curl and no wget, which is what
makes an air-gapped install possible.

## When the install fails

Every failure names what failed and what to do next, and exits non-zero
having installed nothing.

| What happened | What it says |
|---|---|
| no downloader, and none can be installed | `neither curl nor wget is available, so nothing can be downloaded` |
| a proxy variable that is not a URL | `https_proxy is set to 'proxy.corp:8080', which is not a URL a proxy can be reached at` |
| a proxy that blocks the download | the download error, followed by `A proxy is in force: https_proxy=...` |
| the transfer stopped early | `the download of ... did not arrive intact: it is truncated: the gzip stream ends part way through (N bytes)` |
| a portal answered instead | `... did not arrive intact: it is a web page, not an archive (N bytes)` |
| `SHA256SUMS` is a sign-in page | `what came back for SHA256SUMS is not a checksum file` |
| `SHA256SUMS` does not list the archive | `SHA256SUMS has no entry for vitrum-<version>-<target>.tar.gz` |
| the digest disagrees | `checksum mismatch for ...; nothing was installed` |
| the install directory refuses a write | `<dir> cannot be written to` |
| `vitrum` is running from there | `vitrum is running from <dir>/vitrum (pid N)` |
| the WebKit runtime cannot be installed | `vitrum needs a WebKit runtime and this installer cannot install one`, with the reason |
| the WebView2 runtime cannot be installed | `the WebView2 runtime could not be installed: ...` |
| an architecture with no build | `there is no published build for Linux on riscv64` |
| a libc with no build | `there is no published build for Linux with musl libc` |
| a shared library has no package here | `the published build needs shared libraries this distribution does not package` |
| the C library is older than the build needs | `the published build needs a newer C library than this machine has`, with both versions |
| macOS will not run the installed binary | `the installed vitrum does not run on this machine (exit N)`, with the commands that clear the download mark |

A running `vitrum-server` never blocks an install. Its file is replaced by
rename, the running daemon keeps the image it started with, and the installer
says so: the new build is taken when it is next restarted, and restarting it
ends the sessions it holds.

## Updating

```sh
vitrum update
```

It verifies the new archive the same way the installer does, and it stages
both binaries or neither.

A staged update is applied the next time `vitrum` starts. The running client
is never replaced underneath itself, so an update cannot break the window you
are working in. The sidebar shows that a restart will take the new build;
Settings hides that mark for anyone who would rather not see it, and hiding it
does not stop updates being checked for, staged or applied.

The daemon keeps running the old code until it is restarted, and restarting it
ends every session it holds. Do that when the agents are idle.

`vitrum --version` prints the version the running binary was built at.

## Channels

| Channel | Gets |
|---|---|
| stable | published releases, in order |
| nightly | a build of `main`, replaced whenever `main` changes |

Stable is the default. A nightly carries the next patch version with a date
and a commit, such as `0.1.1-nightly.20260809.f4f494e`, so `vitrum --version`
names the build rather than repeating the last release.

Nightly moves forward onto a stable release once that release is newer than
the nightly you are running. It is never rolled back to an older stable.

To leave nightly for a specific stable build, install that version directly.

## Pinning a version, and rolling back

`vitrum update` only moves forward. To install a specific version, in either
direction, run the installer with it:

```sh
curl -fsSL https://raw.githubusercontent.com/santhreal/vitrum/main/install.sh | sh -s -- 1.2.3
```

```powershell
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/santhreal/vitrum/main/install.ps1))) -Version 1.2.3
```

Both binaries are replaced together, so the client and daemon never end up on
different protocol versions on disk. A daemon that is already running is not
replaced. Restart it, and the sessions it holds, to complete a rollback.

## Removing

```sh
curl -fsSL https://raw.githubusercontent.com/santhreal/vitrum/main/install.sh | sh -s -- --uninstall
```

```powershell
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/santhreal/vitrum/main/install.ps1))) -Uninstall
```

With a copy of the script on disk, `sh install.sh --uninstall` and
`.\install.ps1 -Uninstall` do the same thing.

This removes what the install wrote and nothing else. Every file was recorded
as it was created, including the icon files, whose names come from the binary
rather than from a list in the script, so the uninstaller takes away the set
this build produced. Your shell rc keeps everything outside the `# >>> vitrum`
block, and a directory that still holds anything is left alone.

An rc file the installer created is recorded as created and is deleted once
its block is taken out, so a machine that had no `~/.profile`, no `~/.zshrc`
or no `config.fish` before the install has none after the uninstall. An rc
file that already existed keeps everything outside the block, and one that
has picked up other content since is kept whole.

It refuses while `vitrum` is running, for the same reason installing does, and
it says so if `vitrum-server` is still holding sessions from the copy it just
removed.

If the install predates the manifest, or was made somewhere else, name the
directory: `sh install.sh --uninstall --install-dir=PATH`. Uninstalling
something that is not there is an error rather than a silent success.

What remains is config and state, listed in
[configuration.md](configuration.md), and the WebKit or WebView2 runtime,
which is shared with the rest of the machine.

Building from source is in [CONTRIBUTING.md](../CONTRIBUTING.md).
