# vitrum

A terminal for running many coding agents at once.

It is a real terminal. You point it at whatever you already use, whether that is
Claude Code, Codex, Gemini CLI, opencode, veyyon or a plain shell, and it gives
you a sidebar to move between them, one window that can be many windows, and a
warning when two of your agents start writing the same file.

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

Measured on a headless Ubuntu 24.04 host with software rendering, twenty
windows open and nothing running in them:

| | |
|---|---|
| WebKit web processes | **1**, for all twenty |
| Total memory | **~325 MB** PSS across every vitrum and WebKit process (323.9 and 326.5 on two runs) |
| Idle CPU | **0.22%** of one core, averaged over 60 seconds |

Reproduce it with the numbers in `harness/`. Expect a lower idle figure with
hardware rendering and a higher one while agents are actually printing.

Cold start, measured separately on the same host with a single window: the web
process exists **0.20 s** after exec, and the window is painted and back to
doing no work at **1.31 s** (1.26, 1.31, and 1.36 on three runs). Idle after
that is 4 CPU ticks per 30 seconds across the client and the web process
together, which is the noise floor of the measurement rather than a number
worth quoting.

**Saved commands with your own shortcuts.** Nobody runs a bare `claude`. Save
the invocation you actually use, with the directory it belongs in, and bind a key
to it. The key works from anywhere in the app, not just inside a dialog.

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

## Build

Build a **release**, not `main`. `main` carries whatever is in flight; a tag is
a state that was tested as a whole.

```sh
curl -L https://github.com/santhreal/vitrum/archive/refs/tags/v0.1.0.tar.gz | tar xz
cd vitrum-0.1.0
cargo build --release --locked
```

Or with git, if you would rather have the history:

```sh
git clone --depth 1 --branch v0.1.0 https://github.com/santhreal/vitrum
cd vitrum
cargo build --release --locked
```

`--locked` builds the exact dependency versions the tag was tested against.
Drop it only if you deliberately want Cargo to resolve fresher ones.

Check what you built with `vitrum --version`; it reports the crate version the
tag was cut at, so an installed copy can always be told from a rebuild.

Every release is listed at
[github.com/santhreal/vitrum/releases](https://github.com/santhreal/vitrum/releases).
Each one carries the source tag GitHub generates for it, plus a per-platform
archive and the `SHA256SUMS` that `vitrum update` checks the archive against.
Building from source is the documented path for a first install; the archive is
what an installed copy updates itself from.

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

## Install

One paste. Pick your platform.

### Linux

```sh
mkdir -p ~/.local/bin && v=$(curl -fsSL https://api.github.com/repos/santhreal/vitrum/releases/latest | sed -n 's/.*"tag_name": *"v\([^"]*\)".*/\1/p') && curl -fsSL "https://github.com/santhreal/vitrum/releases/download/v$v/vitrum-$v-x86_64-unknown-linux-gnu.tar.gz" | tar xz -C ~/.local/bin && vitrum
```

### macOS

```sh
mkdir -p ~/.local/bin && v=$(curl -fsSL https://api.github.com/repos/santhreal/vitrum/releases/latest | sed -n 's/.*"tag_name": *"v\([^"]*\)".*/\1/p') && curl -fsSL "https://github.com/santhreal/vitrum/releases/download/v$v/vitrum-$v-$(uname -m | sed s/arm64/aarch64/)-apple-darwin.tar.gz" | tar xz -C ~/.local/bin && vitrum
```

### Windows (PowerShell)

```powershell
$b="$env:LOCALAPPDATA\Programs\vitrum"; mkdir -Force $b >$null; $v=(irm https://api.github.com/repos/santhreal/vitrum/releases/latest).tag_name.TrimStart('v'); iwr "https://github.com/santhreal/vitrum/releases/download/v$v/vitrum-$v-x86_64-pc-windows-msvc.tar.gz" -OutFile "$b\v.tgz"; tar xzf "$b\v.tgz" -C $b; del "$b\v.tgz"; [Environment]::SetEnvironmentVariable('Path',"$([Environment]::GetEnvironmentVariable('Path','User'));$b",'User'); & "$b\vitrum.exe"
```

If `vitrum` is not found afterwards, `~/.local/bin` is not on your `PATH` (or on
Windows, open a new terminal).

Requires a WebKit runtime, which is the only system dependency. Debian and
Ubuntu: `sudo apt install libwebkit2gtk-4.1`. Fedora: `sudo dnf install
webkit2gtk4.1`. Arch: `sudo pacman -S webkit2gtk-4.1`. macOS and Windows ship
one.

Update in place with `vitrum update`.

### From crates.io

```sh
cargo install vitrum vitrum-server
```

Builds from source, so it needs the development packages listed under
[Requirements](#requirements). Both names in one command: `vitrum` looks for the
daemon beside itself, so a client without `vitrum-server` has nothing to talk
to.

### Desktop entry and icon

The pastes above install the command. These add a launcher entry, an icon, and a
`vu` shortcut for `vitrum update`. Run from a repository checkout.

### Linux

```sh
bin=$(cargo metadata --format-version 1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')/release
mkdir -p ~/.local/bin ~/.local/share/applications
install -m755 "$bin/vitrum" "$bin/vitrum-server" ~/.local/bin/
for s in 16 24 32 48 64 128 256 512; do
  install -Dm644 assets/logo/vitrum-$s.png \
    ~/.local/share/icons/hicolor/${s}x${s}/apps/vitrum.png
done
gtk-update-icon-cache -qtf ~/.local/share/icons/hicolor 2>/dev/null || true
cat > ~/.local/share/applications/vitrum.desktop <<EOF
[Desktop Entry]
Type=Application
Name=vitrum
Comment=A terminal for running many coding agents at once
Exec=$HOME/.local/bin/vitrum
Icon=vitrum
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

### macOS

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
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
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

### Windows (PowerShell)

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
$s.Description = "A terminal for running many coding agents at once"
Copy-Item assets\logo\vitrum.ico $bin -Force
$s.IconLocation = "$bin\vitrum.ico"
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
rm -f ~/.local/share/icons/hicolor/*/apps/vitrum.png
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
cut. `SPEC.md` is an internal requirements ledger, not user documentation: read
it for what is still owed.

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

MIT.
