# Installing, updating, removing

The install command is in the [README](../README.md).

## What the installer does

1. Checks this machine before downloading anything: the architecture and libc
   have a published build, the install directory can really be written to,
   no `vitrum` is running from it, and the WebKit or WebView2 runtime is
   present.
2. Resolves the latest published release.
3. Downloads the platform archive and the release `SHA256SUMS`.
4. Checks the archive arrived whole, then compares digests. On a mismatch it
   stops, having installed nothing.
5. Places `vitrum` and `vitrum-server` in the install directory.
6. Adds that directory to `PATH`: in `~/.profile`, in `~/.bash_profile` when
   bash has one, and in the rc of every shell you have among bash, zsh and
   fish.
7. Adds a launcher entry: a `.desktop` file on Linux, an app bundle in
   `~/Applications` on macOS, a Start menu shortcut on Windows.
8. Defines `vu` as `vitrum update`.
9. Records every file it wrote, so `--uninstall` can remove exactly those.

Steps 6 to 8 are idempotent. Re-running the installer rewrites nothing it
already wrote: the shell edits live in one marked block per file, and a
re-install replaces that block rather than adding a second one.

Each shell gets its own syntax. bash and zsh get a guarded `export PATH`,
fish gets `set -gx PATH` inside `if not contains`, and neither grows your
`PATH` by an entry per nested shell.

Default install directory:

| Platform | Directory |
|---|---|
| Linux, macOS | `~/.local/bin` |
| Windows | `%LOCALAPPDATA%\vitrum\bin` |

## Requirements

vitrum needs one system library, and the installer refuses to install without
it rather than leaving you a binary that opens no window. It names the package
for the distribution it is running on:

| Distribution | Package |
|---|---|
| Debian, Ubuntu, Mint, Pop, Raspbian | `sudo apt install libwebkit2gtk-4.1-0` |
| Fedora, RHEL, Rocky, Alma | `sudo dnf install webkit2gtk4.1` |
| Arch, Manjaro, EndeavourOS | `sudo pacman -S webkit2gtk-4.1` |
| openSUSE, SLES | `sudo zypper install libwebkit2gtk-4_1-0` |
| Alpine | `sudo apk add webkit2gtk-4.1` |
| Void | `sudo xbps-install -S webkit2gtk` |
| Gentoo | `sudo emerge net-libs/webkit-gtk:4.1` |
| NixOS | `nix-env -iA nixpkgs.webkitgtk_4_1` |
| Windows | `winget install Microsoft.EdgeWebView2Runtime` |
| macOS | nothing extra |

Pass `--no-runtime-check` (`-NoRuntimeCheck`) to install anyway, for an image
that installs the runtime separately. The installer then says the runtime is
still missing instead of pretending the install is complete.

Published x86-64 builds target the base instruction set, so they run on any
x86-64 processor. They do not use AVX2 or AVX-512 even where the processor
has them. A build you make yourself may target your own processor and fail
with `SIGILL` on an older one.

Published Linux builds link glibc. A musl host, such as Alpine or Void, is
told so and pointed at a source build, rather than being handed an archive
whose loader it does not have.

## Options

```sh
sh install.sh --help
```

| Option | Environment | Effect |
|---|---|---|
| `VERSION` | `VITRUM_VERSION` | install that version instead of the latest |
| `--install-dir=PATH` | `VITRUM_INSTALL_DIR` | put the binaries elsewhere |
| `--base-url=URL` | `VITRUM_BASE_URL` | take the archive and `SHA256SUMS` from a mirror or a local directory |
| `--no-integrate` | `VITRUM_NO_INTEGRATE` | binaries only: skip steps 6 to 8 |
| `--no-runtime-check` | `VITRUM_NO_RUNTIME_CHECK` | install without the WebKit or WebView2 runtime |
| `--uninstall` | | remove everything the installer wrote |

PowerShell takes the same as `-Version`, `-InstallDir`, `-BaseUrl`,
`-NoIntegrate`, `-NoRuntimeCheck` and `-Uninstall`.

`--no-integrate` is for images, provisioning scripts and headless hosts, where
a `PATH` edit in a home directory and a launcher entry on a machine with no
desktop are both wrong.

`--base-url` is for a host that cannot reach GitHub. Copy the release archive
and its `SHA256SUMS` into one directory, then name it. The digest is still
checked, so a mirror is trusted no further than the release is:

```sh
sh install.sh 1.2.3 --base-url=file:///srv/vitrum
```

With a `file://` base the installer needs no `curl` and no `wget`, which is
what makes an air-gapped install possible.

## When the install fails

Every failure names what failed and what to do next, and exits non-zero
having installed nothing.

| What happened | What it says |
|---|---|
| no `curl` and no `wget` | `neither curl nor wget is available, so nothing can be downloaded` |
| a proxy variable that is not a URL | `https_proxy is set to 'proxy.corp:8080', which is not a URL a proxy can be reached at` |
| a proxy that blocks the download | the download error, followed by `A proxy is in force: https_proxy=...` |
| the transfer stopped early | `the download of ... did not arrive intact: it is truncated: the gzip stream ends part way through (N bytes)` |
| a portal answered instead | `... did not arrive intact: it is a web page, not an archive (N bytes)` |
| `SHA256SUMS` is a sign-in page | `what came back for SHA256SUMS is not a checksum file` |
| `SHA256SUMS` does not list the archive | `SHA256SUMS has no entry for vitrum-<version>-<target>.tar.gz` |
| the digest disagrees | `checksum mismatch for ...; nothing was installed` |
| the install directory refuses a write | `<dir> cannot be written to` |
| `vitrum` is running from there | `vitrum is running from <dir>/vitrum (pid N)` |
| no WebKit or WebView2 runtime | `vitrum needs a WebKit runtime and this machine has none`, with the package for your distribution |
| an architecture with no build | `there is no published build for Linux on aarch64` |
| a libc with no build | `there is no published build for Linux with musl libc` |

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
curl -fsSLO https://raw.githubusercontent.com/santhreal/vitrum/main/install.sh
sh install.sh 1.2.3
```

```powershell
irm https://raw.githubusercontent.com/santhreal/vitrum/main/install.ps1 -OutFile install.ps1
.\install.ps1 -Version 1.2.3
```

Both binaries are replaced together, so the client and daemon never end up on
different protocol versions on disk. A daemon that is already running is not
replaced. Restart it, and the sessions it holds, to complete a rollback.

## Removing

```sh
sh install.sh --uninstall
```

```powershell
.\install.ps1 -Uninstall
```

This removes what the install wrote and nothing else. Every file was recorded
as it was created, including the icon files, whose names come from the binary
rather than from a list in the script, so the uninstaller takes away the set
this build produced. Your shell rc keeps everything outside the `# >>> vitrum`
block, and a directory that still holds anything is left alone.

It refuses while `vitrum` is running, for the same reason installing does, and
it says so if `vitrum-server` is still holding sessions from the copy it just
removed.

If the install predates the manifest, or was made somewhere else, name the
directory: `sh install.sh --uninstall --install-dir=PATH`. Uninstalling
something that is not there is an error rather than a silent success.

What remains is config and state, listed in
[configuration.md](configuration.md).

Building from source is in [CONTRIBUTING.md](../CONTRIBUTING.md).
