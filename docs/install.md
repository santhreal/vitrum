# Installing, updating, removing

The install command is in the [README](../README.md).

## What the installer does

1. Resolves the latest published release.
2. Downloads the platform archive and the release `SHA256SUMS`.
3. Compares digests. On a mismatch it stops, having installed nothing.
4. Places `vitrum` and `vitrum-server` in the install directory.
5. Adds that directory to `PATH`.
6. Adds a launcher entry: a `.desktop` file on Linux, an app bundle in
   `~/Applications` on macOS, a Start menu shortcut on Windows.
7. Defines `vu` as `vitrum update`.

Steps 5 to 7 are idempotent. Re-running the installer rewrites nothing it
already wrote.

Default install directory:

| Platform | Directory |
|---|---|
| Linux, macOS | `~/.local/bin` |
| Windows | `%LOCALAPPDATA%\vitrum\bin` |

## Options

```sh
sh install.sh --help
```

| Option | Environment | Effect |
|---|---|---|
| `VERSION` | `VITRUM_VERSION` | install that version instead of the latest |
| `--install-dir=PATH` | `VITRUM_INSTALL_DIR` | put the binaries elsewhere |
| `--no-integrate` | `VITRUM_NO_INTEGRATE` | binaries only: skip steps 5 to 7 |

PowerShell takes the same three as `-Version`, `-InstallDir` and
`-NoIntegrate`.

`--no-integrate` is for images, provisioning scripts and headless hosts, where
a `PATH` edit in a home directory and a launcher entry on a machine with no
desktop are both wrong.

## Updating

```sh
vitrum update
```

It verifies the new archive the same way the installer does, and it replaces
both binaries or neither.

The new client runs after you restart the window. The daemon keeps running the
old code until it is restarted, and restarting it ends every session it holds.
Do that when the agents are idle.

`vitrum --version` prints the version the running binary was built at.

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
rm -f ~/.local/bin/vitrum ~/.local/bin/vitrum-server
rm -f ~/.local/share/applications/vitrum.desktop
rm -rf ~/Applications/vitrum.app
```

```powershell
Remove-Item -Recurse "$env:LOCALAPPDATA\vitrum\bin"
Remove-Item (Join-Path ([Environment]::GetFolderPath('Programs')) 'vitrum.lnk')
```

Delete the `# vitrum` block from your shell rc, or the `vu` function from your
PowerShell profile. What remains is config and state, listed in
[configuration.md](configuration.md).

Building from source is in [CONTRIBUTING.md](../CONTRIBUTING.md).
