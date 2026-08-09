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
| `launch.json` | saved commands |
| `windows.json` | window size and position |

All three are plain JSON. Presets live apart from settings so a window resize
does not rewrite them, and window geometry lives in state rather than config
because it does not migrate to another machine.

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
