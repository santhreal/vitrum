# Configuration

## File locations

| Platform | `ui.json`, `launch.json` | `windows.json` |
|---|---|---|
| Linux | `~/.config/vitrum/` | `~/.local/state/vitrum/` |
| macOS | `~/Library/Application Support/dev.santhreal.vitrum/` | same |
| Windows | `%APPDATA%\santhreal\vitrum\config\` | `%LOCALAPPDATA%\santhreal\vitrum\state\` |

Linux honours `XDG_CONFIG_HOME` and `XDG_STATE_HOME`. An empty or relative
value is treated as unset rather than resolved against `/` or the working
directory.

| File | Contents |
|---|---|
| `ui.json` | settings, workspaces, sidebar layout |
| `launch.json` | presets: saved commands, directories and bindings |
| `windows.json` | window size and position |

All three are plain JSON. Each is written on its own, so a window resize does
not rewrite the presets. Window geometry does not migrate to another machine.

## Settings

Everything in this section lives in `ui.json` and is editable in Settings. A
key that is absent takes its default, so a hand-edited file may hold only the
keys you changed. A value outside its range is clamped on load rather than
rejected, and the clamped value is what gets written back.

### Sidebar and window

| Key | Effect | Default |
|---|---|---|
| `showBranch` | Draw the git branch on rows that have one | `true` |
| `showPlace` | Draw the working directory on rows where it is not the project's own | `true` |
| `showWorktree` | Draw the linked worktree name on rows that are in one | `true` |
| `showTime` | Draw the row's timestamp | `true` |
| `showStatusWord` | Draw the status word beside the status icon | `true` |
| `showStatusBar` | Draw the status bar under the pane | `true` |
| `alwaysSlim` | Force every row to the slim variant at any width | `false` |
| `density` | `comfortable` or `compact` | `comfortable` |
| `theme` | `system`, `light` or `dark` | `system` |
| `textScalePct` | Interface text scale, 80 to 200 | `100` |
| `reduceMotion` | Take the reduced-motion path whatever the desktop reports | `false` |
| `confirmTerminate` | Ask before terminating a live child | `true` |

### Terminal

Under `terminal`.

| Key | Effect | Default |
|---|---|---|
| `scrollbackLines` | Lines the grid keeps above the viewport | `1000` |
| `fontFamily` | Font for the grid. Empty picks the first installed monospace | `""` |
| `fontSizePx` | Cell font size | `13` |
| `lineHeightPct` | Cell height against the font's own, 80 to 200 | `100` |
| `cellWidthPct` | Cell width against the font's advance, 80 to 140 | `100` |
| `cursorShape` | `block`, `bar` or `underline` | `block` |
| `cursorBlink` | Blink the cursor | `true` |
| `blinkIntervalMs` | Blink period, 100 to 2000 | `530` |
| `wheelLines` | Lines per wheel notch, up to 25 | `3` |
| `bracketedPaste` | Wrap a paste in the bracketed-paste sequences | `true` |
| `presentMode` | `vsync`, `adaptive` or `immediate` | `vsync` |
| `palette` | `inherit` or a named scheme | `inherit` |
| `followHostTerminal` | Import the palette from the terminal's own configuration | `false` |
| `hostPalette` | The imported colours and the file they came from | empty |

`presentMode` decides what a frame waits for. `vsync` presents on the
refresh. `adaptive` presents immediately when a frame misses its deadline and
on the refresh otherwise. `immediate` never waits and tears. The named
schemes and the import are listed in [appearance.md](appearance.md).

### Appearance

Under `appearance`.

| Key | Effect | Default |
|---|---|---|
| `opacityPct` | Window chrome opacity, 20 to 100 | `100` |
| `terminalOpacityPct` | Grid opacity, independent of the chrome | `100` |
| `backdrop` | Absolute path to a backdrop image. Empty means none | `""` |
| `backdropFit` | `cover`, `contain`, `tile` or `center` | `cover` |
| `backdropBlurPx` | Blur over the backdrop, up to 64 | `0` |
| `backdropDimPct` | Scrim between the backdrop and the interface | `0` |

### Notices

Under `notices`. A lifetime of `0` means the strip stays until it is
dismissed. The maximum is 60 seconds.

| Key | Effect | Default |
|---|---|---|
| `flashSeconds` | How long a flash message stays | `6` |
| `noticeSeconds` | How long a notice strip stays | `0` |
| `showHistoryNotice` | Draw the strip saying a pane is showing history | `true` |
| `showStartupErrors` | Draw what a child wrote before it got going | `true` |

### Notifications

Under `notifications`.

| Key | Effect | Default |
|---|---|---|
| `finished` | Notify when a child exits | `false` |
| `needsApproval` | Notify when a session blocks on you | `true` |
| `failed` | Notify when a child exits non-zero | `true` |
| `skipFocusedSession` | Skip the notification for the session on screen | `true` |

### Startup

Under `startup`.

| Key | Effect | Default |
|---|---|---|
| `showSplash` | Draw the boot surface | `true` |
| `splashAfterMs` | Delay before it appears, so a fast start never shows it | `120` |

### Keyboard

Under `keyboard`. `overrides` maps an action name to a chord and replaces the
default for that action. `custom` holds bindings that have no default.
[keys.md](keys.md) lists the actions and the chord syntax.

### Disposition

Under `policy`.

| Key | Effect | Default |
|---|---|---|
| `autoSettleAfterMs` | Inactivity after which an unattended row settles itself. `null` disables it | `604800000` |

### Updates and first run

| Key | Effect | Default |
|---|---|---|
| `updateChannel` | `stable` or `nightly` | `stable` |
| `showRestartToUpdate` | Draw the restart band after an update is staged | `true` |
| `ignoredUpdate` | Newest version dismissed from the titlebar. Empty means none | `""` |
| `seenVersion` | Version whose release notes were last shown | `""` |
| `onboarded` | Whether the first-run sheet has been passed | `false` |
| `daemonUrl` | Daemon to connect to. Empty leaves `--server` authoritative | `""` |

Dismissing an update records that version only. A later release is offered
again.

## The token file

The daemon writes a per-user token at startup: 32 random bytes, hex-encoded,
in `$XDG_RUNTIME_DIR/vitrum/token` when that variable is set, and in the data
directory otherwise. The file is mode 0600 inside a 0700 directory, and a new
token is written on every start.

The client reads it from `VITRUM_TOKEN`, then from `--token-file`, then from
that path. Reaching a daemon on another machine is covered in
[remote.md](remote.md).

## Daemon settings

Each is a flag or an environment variable. The flag wins.

| Flag | Variable | Default |
|---|---|---|
| `--port` | `VITRUM_PORT` | 7737 |
| `--scrollback-bytes` | `VITRUM_SCROLLBACK_BYTES` | 10 MiB per session |

`VITRUM_LOG` sets the log level to `trace`, `debug`, `warn` or `error`. The
default is `info`.

Under systemd, put them in the unit's `Environment=` lines. `vitrum-server
--help` prints the same list.
