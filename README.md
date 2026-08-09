<h1 align="center">vitrum</h1>

<p align="center">
  <a href="https://github.com/santhreal/vitrum/releases/latest"><img src="https://img.shields.io/github/v/release/santhreal/vitrum?style=flat-square&color=7aa2f7&label=release&labelColor=0a0a0a" alt="Latest vitrum release" /></a>&nbsp;
  <a href="https://github.com/santhreal/vitrum/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/santhreal/vitrum/ci.yml?style=flat-square&label=CI&labelColor=0a0a0a" alt="CI" /></a>&nbsp;
  <a href="#license"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-7aa2f7?style=flat-square&labelColor=0a0a0a" alt="License: MIT OR Apache-2.0" /></a>&nbsp;
  <a href="https://github.com/santhreal/vitrum/stargazers"><img src="https://img.shields.io/github/stars/santhreal/vitrum?style=flat-square&color=7aa2f7&labelColor=0a0a0a" alt="Stars" /></a>
</p>

<p align="center">
  <strong><a href="#install">Install</a></strong> ·
  <strong><a href="#what-it-costs-to-run">Performance</a></strong> ·
  <strong><a href="#first-run">First run</a></strong> ·
  <strong><a href="CONTRIBUTING.md">Contributing</a></strong> ·
  <strong><a href="SECURITY.md">Security</a></strong>
</p>

# One interface for every agent TUI you have running

vitrum runs the agent TUIs you already use — Claude Code, Codex, Gemini CLI,
veyyon, or any other command — and puts them in one window with one list down
the side.

Every session gets a row: which tool is running it, which project it is in,
and what it is doing right now.

- **working** — busy, nothing for you to do
- **waiting for approval** — it wants a yes before it edits or runs something
- **waiting for input** — it asked you a question
- **ready** — it finished, possibly while you were reading something else
- **failed** — it died

Agent TUIs block. Four of them in tab bars means clicking through tabs to find
the one stuck waiting on you, because tabs show names and never states. This
shows states. `Ctrl+Shift+Down` jumps straight to the next session that wants
something.

Sessions run in a background daemon, not in the window. Closing the window or
updating the app does not kill them. Reopen it and everything is still there,
scrollback included.

## Install

One command. It resolves the latest release, verifies the archive against the
release `SHA256SUMS`, refuses to install on a mismatch, puts `vitrum` and
`vitrum-server` on your `PATH`, and starts the app.

**Linux and macOS**

```sh
curl -fsSL https://raw.githubusercontent.com/santhreal/vitrum/main/install.sh | sh
```

**Windows**

```powershell
irm https://raw.githubusercontent.com/santhreal/vitrum/main/install.ps1 | iex
```

That is the whole install. Update in place later with `vitrum update`, which
checks the same `SHA256SUMS` and refuses a release that is not covered by it.

One system dependency, a WebKit runtime, and only on Linux. Debian and Ubuntu:
`sudo apt install libwebkit2gtk-4.1`. Fedora: `sudo dnf install webkit2gtk4.1`.
Arch: `sudo pacman -S webkit2gtk-4.1`. macOS and Windows ship one.

If `vitrum` is not found afterwards, the install directory is not on your
`PATH`; on Windows, open a new terminal.

### Read it before you run it

Piping a script into a shell means trusting whatever the host serves, so the
script is worth a look, and it is written to be read:

```sh
curl -fsSLO https://raw.githubusercontent.com/santhreal/vitrum/main/install.sh
less install.sh
sh install.sh
```

It refuses to install anything it cannot verify. If the release has no
`SHA256SUMS`, if that file has no entry for the archive, or if the digest
disagrees, it stops and installs nothing rather than falling back.

Earlier versions of this page offered a shorter paste that skipped all of
that: it resolved a version, downloaded the archive and extracted it straight
onto your `PATH` without checking a digest at all, while telling you elsewhere
that the project verifies its downloads. A one-line install that quietly drops
the verification is worse than a longer one that keeps it.

### Build from source

If you would rather build it, [Build from source](#build-from-source) is the
full path, and it is the one to use while you are changing the code.

---

## It is a real terminal

You point it at whatever you already use, whether that is Claude Code, Codex,
Gemini CLI, opencode, veyyon or a plain shell, and it gives you a sidebar to move
between them, one window that can be many windows, and a warning when two of your
agents start writing the same file.

It is not a wrapper. It spawns your agent in a PTY and stays out of the way.
There is no per-agent integration to write, because there is no integration at
all.

---

## What you get

**A sidebar that tells you who is doing what.** Every session shows which agent
is running it as a drawn mark, its status, and how long since it last spoke.
Grouped by filesystem directory, or by folders you name yourself, per workspace,
your choice in Settings.

**Same-file collision detection.** Ten agents in a big checkout usually do not
conflict, so a warning that fires because two of them share a repository is a
warning you mute on day one. This fires on one condition: two live sessions have
both written the same file. When they have, both rows say so. Whichever agent
writes last wins and the other's work is gone, with nothing else in your toolchain
reporting an error. Linux only today; on other platforms it says so rather than
telling you nothing is wrong.

**Sessions that outlive the window.** The daemon owns the PTYs. Close every
window and your agents keep running; open a new one and they are all still
there, scrollback included.

To be exact about the promise: **your agents survive losing the client. They do
not survive losing the daemon**, because the PTYs are its children. Connecting
from elsewhere over SSH is sketched in `DESIGN-REMOTE.md` and is not built.

**One process, many windows.** Every window shares a single WebKit web
process, which is the difference between this and an Electron-shaped app that
spends a renderer per window.

**Saved commands with your own shortcuts.** Nobody runs a bare `claude`. Save
the invocation you actually use, with the directory it belongs in, and bind a key
to it. The key works from anywhere in the app, not just inside a dialog.

---

## What it costs to run

Every number in this section is measured by `harness/` on a real host and
written here by `make readme-perf`. Nothing in it is typed by hand, and CI fails
if a figure here drifts from the snapshot it came from.

### Memory

<!-- BENCH:footprint:start -->
Measured on **13th Gen Intel(R) Core(TM) i9-13900K**, 32 logical cores, WebKitGTK 2.52.3-0ubuntu0.24.04.1, `Linux 6.8.0-136-generic x86_64`. Every window holds a live shell against one `vitrum-server`. The figure is PSS, which charges shared pages once, so the totals below add up across processes instead of counting the same engine twice. `vitrum 0.1.0` at `18df8cb`.

| Windows | Client tree | Client processes | Daemon tree | Daemon processes |
|---:|---:|---:|---:|---:|
| 1 | 247.8 MB | 3 | 5.5 MB | 2 |
| 20 | 460.1 MB | 3 | 40.6 MB | 21 |

The 20-window client tree is still 3 processes, not 60: every window is a view onto one shared web process and one network process. Going from 1 to 20 windows costs **11.2 MB per extra window**.

Where the 20-window client tree goes:

| Process | Count | PSS |
|---|---:|---:|
| `WebKitWebProcess` | 1 | 298.0 MB |
| `vitrum` | 1 | 140.8 MB |
| `WebKitNetworkProcess` | 1 | 21.3 MB |

The daemon side of the same run is 40.6 MB across 21 processes, and the shells the operator asked for are most of it:

| Process | Count | PSS |
|---|---:|---:|
| `bash` | 20 | 35.0 MB |
| `vitrum-server` | 1 | 5.6 MB |

Reproduce: `harness/run.sh memory 1` and `harness/run.sh memory 20`, then `make readme-perf`.
<!-- BENCH:footprint:end -->

### Idle cost

<!-- BENCH:idle:start -->
An idle terminal should cost nothing. Measured over 60 s with 20 windows open, every one holding a live shell, on **13th Gen Intel(R) Core(TM) i9-13900K**, 32 logical cores, WebKitGTK 2.52.3-0ubuntu0.24.04.1, `Linux 6.8.0-136-generic x86_64`.

| Tree | CPU | PSS before | PSS after | Drift |
|---|---:|---:|---:|---:|
| Client | 0.1000% of one core | 447.4 MB | 447.4 MB | +0.0 MB |
| Daemon | 0.0000% of one core | 40.7 MB | 40.7 MB | +0.0 MB |

That is 6 scheduler ticks in 60 seconds across 20 windows. Nothing polls, so nothing accumulates: the drift is the point of the last column.

Reproduce: `harness/run.sh idle-cpu 60 20`, then `make readme-perf`.
<!-- BENCH:idle:end -->

Expect a lower idle figure with hardware rendering, and a higher one while
agents are actually printing. Cold start, measured separately on the same host
with a single window: the web process exists **0.20 s** after exec, and the
window is painted and back to doing no work at **1.31 s** (1.26, 1.31, and 1.36
on three runs).

---

## Requirements

- **Rust nightly**, pinned by `rust-toolchain.toml`. `rustup` reads it and
  installs the right toolchain by itself; you do not pick a version.
- **A WebKit runtime**, which is the only system dependency:

| | |
|---|---|
| Debian / Ubuntu | `sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev` |
| Fedora | `sudo dnf install webkit2gtk4.1-devel gtk3-devel` |
| Arch | `sudo pacman -S webkit2gtk-4.1 gtk3` |
| macOS | nothing; WebKit ships with the system |
| Windows | nothing; WebView2 ships with Windows 11 and current 10 |

---

## Build from source

One command, and it needs the [Requirements](#requirements) above:

```sh
git clone https://github.com/santhreal/vitrum && cd vitrum && cargo build --release --locked
```

`--locked` builds the exact dependency versions this tree was tested against.
Drop it only if you deliberately want Cargo to resolve fresher ones.

Check what you built with `vitrum --version`, which reports the crate version
it was built at, so an installed copy can always be told from a rebuild.

That produces three binaries in `target/release` (or wherever your
`CARGO_TARGET_DIR` points, if you set one):

- `vitrum`, the window
- `vitrum-server`, the session daemon
- `vitrum-replay`, a tool for reading a captured session back

The first two are the product, and they are what the release archive and the
install blocks below carry. `vitrum-replay` is a build-tree tool for now: run it
from the build directory, and see Contributing for what it does.

You never start the daemon yourself. `vitrum` starts one if nothing is
listening, reuses one that is already running, and never kills it on exit.

Run it in place to check the build before installing anything:

```sh
./target/release/vitrum
```

---

### Desktop entry

The install above places the command. These add a launcher entry and a `vu`
shortcut for `vitrum update`. Run from a repository checkout.

#### Linux

```sh
bin=$(cargo metadata --format-version 1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')/release
mkdir -p ~/.local/bin ~/.local/share/applications
install -m755 "$bin/vitrum" "$bin/vitrum-server" ~/.local/bin/
cat > ~/.local/share/applications/vitrum.desktop <<EOF
[Desktop Entry]
Type=Application
Name=vitrum
Comment=One interface for every agent TUI you have running
Exec=$HOME/.local/bin/vitrum
Terminal=false
Categories=Development;TerminalEmulator;
StartupWMClass=vitrum
EOF
update-desktop-database ~/.local/share/applications 2>/dev/null || true
case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *)
  echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.profile
  echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
  echo "added ~/.local/bin to PATH in ~/.profile and ~/.bashrc"
esac
grep -qs 'alias vu=' ~/.bashrc || {
  printf '\n# vitrum\nalias vu="vitrum update"\n' >> ~/.bashrc
  echo "added the vu alias to ~/.bashrc"
}
echo "installed. run: vitrum   (or find it in your app launcher)"
echo "update with: vitrum update   (or vu, in a new shell)"
```

`~/.local/bin` is already on `PATH` on most distributions, so the `PATH` lines
usually do not run. Log out and back in if they did.

`vu` is the only alias, and it is a convenience rather than the interface:
`vitrum update` is the command, and it works whether or not the alias exists.

#### macOS

```sh
bin=$(cargo metadata --format-version 1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')/release
mkdir -p ~/.local/bin "$HOME/Applications/vitrum.app/Contents/MacOS"
install -m755 "$bin/vitrum" "$bin/vitrum-server" ~/.local/bin/
cat > "$HOME/Applications/vitrum.app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>vitrum</string>
  <key>CFBundleIdentifier</key><string>dev.santhreal.vitrum</string>
  <key>CFBundleExecutable</key><string>vitrum</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$("$bin/vitrum" --version | awk '{print $NF}')</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
EOF
ln -sf ~/.local/bin/vitrum "$HOME/Applications/vitrum.app/Contents/MacOS/vitrum"
ln -sf ~/.local/bin/vitrum-server "$HOME/Applications/vitrum.app/Contents/MacOS/vitrum-server"
SHELLRC=~/.zshrc; [ "$(basename "$SHELL")" = bash ] && SHELLRC=~/.bash_profile
case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *)
  echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$SHELLRC"
  echo "added ~/.local/bin to PATH in $SHELLRC"
esac
grep -qs 'alias vu=' "$SHELLRC" || {
  printf '\n# vitrum\nalias vu="vitrum update"\n' >> "$SHELLRC"
  echo "added the vu alias to $SHELLRC"
}
echo "installed. run: vitrum   (or open vitrum from ~/Applications)"
echo "update with: vitrum update   (or vu, in a new shell)"
```

Both binaries are symlinked into the bundle, so rebuilding and re-running
`install` updates the app too. The daemon is linked in as well and that is not
redundant: an app launched from Finder does not inherit your shell `PATH`, so
`vitrum` has to find the daemon beside itself rather than on a `PATH` that will
not contain `~/.local/bin`.

The bundle is unsigned. The first launch needs right-click then Open, once.

#### Windows (PowerShell)

```powershell
$bin = "$env:LOCALAPPDATA\Programs\vitrum"
New-Item -ItemType Directory -Force -Path $bin | Out-Null
$rel = (cargo metadata --format-version 1 --no-deps | ConvertFrom-Json).target_directory + "\release"
Copy-Item "$rel\vitrum.exe", "$rel\vitrum-server.exe" $bin -Force
$user = [Environment]::GetEnvironmentVariable('Path','User')
if ($user -notlike "*$bin*") {
  [Environment]::SetEnvironmentVariable('Path', "$user;$bin", 'User')
  Write-Host "added $bin to your user PATH (restart your terminal)"
}
$menu = [Environment]::GetFolderPath('Programs')
$s = (New-Object -ComObject WScript.Shell).CreateShortcut("$menu\vitrum.lnk")
$s.TargetPath = "$bin\vitrum.exe"
$s.WorkingDirectory = $bin
$s.Description = "One interface for every agent TUI you have running"
$s.Save()
if (-not (Test-Path $PROFILE)) { New-Item -ItemType File -Force -Path $PROFILE | Out-Null }
if (-not (Select-String -Path $PROFILE -Pattern 'function vu' -Quiet)) {
  Add-Content $PROFILE "`nfunction vu { vitrum update @args }"
  Write-Host "added the vu function to $PROFILE"
}
Write-Host "installed. run: vitrum   (or find vitrum in the Start menu)"
Write-Host "update with: vitrum update   (or vu, in a new shell)"
```

A function rather than an alias: a PowerShell alias names a command and cannot
carry the `update` argument with it.

### Uninstall

```sh
rm -f ~/.local/bin/vitrum ~/.local/bin/vitrum-server
rm -f ~/.local/share/applications/vitrum.desktop
```

Then delete the `# vitrum` alias line from your shell rc. vitrum writes nothing
else outside the config and state directories listed below, and removing those
removes every trace of it.

---

## First run

`vitrum` opens an empty window and starts the daemon. Press **Ctrl+Shift+N**,
type a command, press Enter.

The dialog asks two things and nothing else:

```
in   ~/src/vitrum      the directory, autofilled from where you have worked
run  claude --resume   the command, with your saved ones listed below
```

**Ctrl+S** saves what is in those two fields as a preset. It appears in this
list every time from then on. Give it a keyboard shortcut in
**Settings › Presets**, and that shortcut starts it from anywhere in the app,
with no dialog.

A few keys worth knowing; the full list is **F1**.

| | |
|---|---|
| `Ctrl+Shift+N` | new session |
| `Ctrl+K` | filter the sidebar |
| `Ctrl+Shift+F` | search scrollback across every session |
| `Ctrl+Tab` | next session |
| `Ctrl+Shift+X` | stop the focused session |

---

## Translucency and backdrops

**Settings › Appearance.** Two opacity controls, and a backdrop image.

| | |
|---|---|
| Window opacity | the whole window, chrome included |
| Terminal opacity | the grid alone, so the shell can stay solid |
| Backdrop | an image drawn inside the window, with fit, blur and dim |

Both default to fully opaque and emit no CSS at all, so an install that never
opens this tab composites nothing.

The backdrop is drawn **inside** the window, so it looks the same on every
platform and needs nothing from your desktop. Point it at an absolute path to
a PNG, JPEG, GIF or WEBP. The file is checked by signature and not by
extension, so anything that is not really an image is refused. SVG is refused
too: it is a scripted document, and it would render inside the application
page.

### Blur belongs to your compositor

No application can blur what is behind its own window. The compositor owns
that, and on Wayland there is deliberately no protocol for an application to
ask. So vitrum makes the window see-through and your compositor frosts it.

Turn the window opacity down, then add one rule:

```sh
# Hyprland, in hyprland.conf
windowrule = opacity 1.0 override, class:^(vitrum)$
blur = yes                      # under decoration { }
```

```sh
# picom, in picom.conf
blur-background-exclude = [ "class_g != 'vitrum'" ];
```

KWin frosts translucent windows through **System Settings › Desktop Effects ›
Blur**, with no per-application rule needed.

Without a compositor running, a see-through window has nothing to blend with
and will look wrong. Use the backdrop image instead: it does not depend on one.

Native frosting that needs no configuration, using Mica and Acrylic on Windows
and `NSVisualEffectView` on macOS, is not in this release.

---

## Telling the sidebar what an agent is doing

Five statuses can appear on a row. Three of them, Working, Ready and Failed,
vitrum works out for itself by watching the process: what its foreground
program is blocked in, whether it is still printing, how it exited. It needs
nothing from the agent and works with every one, including agents that have
never heard of vitrum.

The other two, **Approval** and **Input**, cannot be observed. An agent asking
"may I force-push?" and a shell sitting at a prompt are both blocked in the
same `read`, and a terminal that guessed between them would be wrong often and
confidently. So an agent has to say so, and until something does, every row
falls back to the observed status and those two never appear.

Saying so is one escape sequence, **OSC 7373**:

```text
ESC ] 7373 ; <state> [ ; <label> ] ESC \
```

`<state>` is `approval`, `input`, `working` or `ready`. `<label>` is optional
short text shown beside the row, such as the question being asked. Any terminal
that does not know the sequence ignores it, so it is safe to emit anywhere.

`vitrum hint` writes it for you:

```sh
vitrum hint approval 'run `rm -rf build/`?'
vitrum hint input 'which file? a, b or c'
vitrum hint ready 'tests pass'
vitrum hint --clear
```

`--clear` hands the row back to the observed status. It declares `working`,
which is the one state vitrum retires by itself once the session goes quiet.

It writes to stdout whether or not stdout is a terminal, so it works inside a
pipeline. It exits 0 when the sequence was written and 2 on a state that does
not exist.

### From a shell prompt

`PROMPT_COMMAND` runs after every command, which is exactly when the shell has
gone back to waiting for you:

```sh
PROMPT_COMMAND='vitrum hint ready "$(basename "$PWD")"'
```

In zsh, the same thing with the hook that runs before each prompt:

```sh
precmd() { vitrum hint ready "${PWD:t}" }
```

### From an agent wrapper

Wrap the agent in a script and declare around it. The `trap` matters: an agent
that is killed mid-run must not leave a stale `working` badge behind.

```sh
#!/bin/sh
# ~/.local/bin/claude-vitrum
trap 'vitrum hint --clear' EXIT INT TERM
vitrum hint working "$*"
claude "$@"
vitrum hint ready 'done'
```

An agent that can run a shell command can call `vitrum hint approval` from its
own permission prompt, which is where the sequence earns its keep: the row
turns Approval the moment the agent asks, and back the moment you answer.

### From Claude Code

That last paragraph is a whole integration once you try it, because a Claude
Code hook has nowhere to write: its stdout belongs to Claude Code, and it runs
with no controlling terminal, so the sequence has to be delivered to the pty by
hand.

[`integrations/claude-code`](integrations/claude-code) is that, ready to
install: a hook, the three lines of `settings.json` that call it, and what each
event maps to.

---

## Running it on another machine

The daemon binds loopback and nothing else, on purpose: it spawns processes you
ask it to, so a listener on the network would be a remote shell for whoever
found it. You reach a remote daemon over **SSH**, which already handles the
authentication and the encryption, and already knows your keys, jump hosts and
`~/.ssh/config` aliases.

**On the remote machine**, install as above, then keep the daemon alive across
logouts:

```sh
mkdir -p ~/.config/systemd/user
cp packaging/vitrum-server.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now vitrum-server
loginctl enable-linger "$USER"
```

`enable-linger` is the important line. Without it your user's services are torn
down when your last session ends, which means closing the SSH connection kills
every agent. With it the daemon belongs to the machine rather than to your
connection.

The daemon takes two settings, each as a flag or an environment variable, with
the flag winning:

| Flag | Variable | Default |
|---|---|---|
| `--port` | `VITRUM_PORT` | 7737 |
| `--scrollback-bytes` | `VITRUM_SCROLLBACK_BYTES` | 10 MiB per session |

`VITRUM_LOG` sets the log level to `trace`, `debug`, `warn` or `error`; the
default is `info`. Under systemd, put them in the unit's `Environment=` lines.
Run `vitrum-server --help` for the same list.

**From your laptop:**

```sh
ssh -N -L 7737:127.0.0.1:7737 user@host &
vitrum --server ws://127.0.0.1:7737
```

Close the lid, lose the wifi, move to another network. The window reconnects on
its own, doubling from a quarter second to a thirty second ceiling, and resumes
each session's output at the exact byte it stopped at.

The schedule is finite: 25 attempts, which is roughly ten minutes of trying once
it reaches the ceiling. After that the window says the connection failed and
waits for Retry, rather than dialling an absent daemon forever. Either way your
agents are untouched, because they belong to the daemon and not to the window.

### What survives what

| | |
|---|---|
| Close a window | sessions keep running |
| Close every window | sessions keep running |
| Lose the network, or the client | sessions keep running; the window reconnects and resumes |
| Log out of the remote host | sessions keep running, **if** `enable-linger` is set |
| The daemon crashes or is upgraded | **every session dies.** The PTYs are its children |

That last row is the honest limit and there is no flag that changes it. Making
sessions outlive the daemon needs the PTYs reparented away from it, which is a
real change and is not built. `DESIGN-REMOTE.md` has the argument.

---

## Where it keeps things

| | `ui.json`, `launch.json` | `windows.json` |
|---|---|---|
| Linux | `~/.config/vitrum/` | `~/.local/state/vitrum/` |
| macOS | `~/Library/Application Support/dev.santhreal.vitrum/` | same |
| Windows | `%APPDATA%\santhreal\vitrum\config\` | `%LOCALAPPDATA%\santhreal\vitrum\state\` |

Linux honours `XDG_CONFIG_HOME` and `XDG_STATE_HOME`, so a relocated config is
respected; an empty or relative value is treated as unset rather than resolved
against `/` or the working directory.

`ui.json` holds your settings, workspaces and sidebar layout. `launch.json`
holds your saved commands, kept separate so a window resize does not rewrite
your presets. Both are plain JSON you can read and edit. Window size and
position live apart from both, in `windows.json` in the state directory, because
where a window sits is not a preference you would ever migrate to another
machine.

---

## Status

Pre-release, version 0.1.0. It runs, and it is used daily on Linux.

Known gaps, stated plainly:

- **Collision detection is Linux only.** On macOS and Windows it reports that
  this build has no watcher for the platform rather than reporting that nothing
  is wrong.
- **Attribution needs a file held open for longer than an instant.** A write
  that opens, appends and closes within microseconds is counted as
  unattributed rather than guessed at. The count is shown; it is never folded
  into a confident "nothing is colliding".
- **Only Linux is exercised end to end.** macOS and Windows compile and the
  platform code exists, but the release is not tested there yet.

---

## Layout

```
app/               the window: sidebar, terminal panes, dialogs, settings
crates/
  vitrum-proto     the wire protocol, shared by the client and the daemon
  vitrum-core      PTY sessions, scrollback, the process registry
  vitrum-server    the daemon: sessions, search, collision detection
  vitrum-model     sidebar ordering, dispositions, time
  vitrum-fmt       formatting that must not differ between surfaces
  vitrum-os        notifications, paths, single instance, theme, badge
  vitrum-search    scrollback search
  vitrum-grid      terminal cells: the grid model, and a wgpu renderer
  vitrum-replay    seekable replay over captured bytes, and the replay binary
vendor/            a patched dioxus-desktop; see Cargo.toml [patch.crates-io]
```

`vendor/` is why twenty windows share one web process. It exposes WebKit's
`webkit_web_view_new_with_related_view`, which upstream wry has but
dioxus-desktop did not surface.

`vitrum-grid` is in the shipped build through `vitrum-replay`, which uses its
cell grid to reconstruct a screen. The wgpu renderer in the same crate is not
reachable from any surface yet: the window still draws terminals with xterm.js,
and the renderer is there for a later move to Dioxus Native, which paints
through Blitz and so cannot carry JavaScript along.

---

## Contributing

```sh
cargo test  --workspace
cargo build --release --workspace
```

Both profiles compile. Release is what a tag is cut against, and it is the
profile every measurement in this file was taken on.

CI builds Linux, macOS and Windows with `RUSTFLAGS: -D warnings`, which turns
an unused import or an ungated test helper into a failed build on a platform
you are not sitting at. You can catch that here before pushing, because
checking never links:

```sh
rustup target add x86_64-pc-windows-gnu
RUSTFLAGS="-D warnings" cargo check --profile release \
  --target x86_64-pc-windows-gnu --workspace --all-targets
```

That covers the whole workspace. macOS needs the Apple SDK for `objc2`'s build
script, so only the crates that do not depend on it can be checked:

```sh
rustup target add aarch64-apple-darwin
RUSTFLAGS="-D warnings" cargo check --profile release \
  --target aarch64-apple-darwin --all-targets \
  -p vitrum-core -p vitrum-os -p vitrum-server -p vitrum-proto \
  -p vitrum-fmt -p vitrum-grid -p vitrum-model -p vitrum-replay
```

The `vitrum` binary itself is still first compiled for macOS by CI.

`CHANGELOG.md` is what changed per release; `RELEASING.md` is how a release is
cut.

`vitrum-replay` reads a raw scrollback capture or an asciicast v2 recording and
answers what the screen held at a given position. Four subcommands: `info` for
size, geometry and chapter counts, `screen` to print the screen as it stood at
one byte or time offset, `markers` to list the OSC 7373 chapters, and `export`
to write the input back out as an asciicast recording. It exits 0 on success, 1
when the file cannot be read or replayed, and 2 on a bad command line.

`vitrum --fixture` renders an in-memory fixture instead of connecting to a
daemon. It ships in the release binary on purpose, as a diagnostic you can reach
only by typing the flag: it opens no socket, it forces `--standalone` so it
cannot attach to a running instance, and it says FIXTURE DATA in both the
sidebar and the titlebar.

Two things this codebase is strict about, because it has been bitten by both:

1. **A test asserts a real value.** Not `!is_empty()`, not that a file exists,
   not that a string appears somewhere. Every test carries a doc comment naming
   the bug it locks out.
2. **A feature is not done until it renders.** This repository has shipped a
   status dot with four colours and no box, a notification button that could not
   be clicked, and a whole search path the client threw away, all with a green
   suite. If you add a surface, add a test that builds it and looks at the
   output.

---

## License

Dual licensed under either of

- MIT license ([LICENSE-MIT](LICENSE-MIT))
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))

at your option. Unless you say otherwise, any contribution you submit is dual
licensed on those same terms.

The two forks under `vendor/` and `vendor-pty/` are somebody else's work, kept
under the MIT license and copyright they arrived with. `NOTICE` names them.
