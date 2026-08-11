<p align="center">
  <img src="assets/logo/vitrum.svg" alt="vitrum" width="96" height="96" />
</p>

<h1 align="center">vitrum</h1>

<p align="center">
One clean interface for every coding agent you have running.
</p>

vitrum manages agent TUIs: Codex, Claude Code, Gemini CLI and opencode. Each
agent gets a row carrying its worktree, its branch, its working directory and
its state, grouped by project, beside a terminal pane drawn on the GPU.

<p align="center">
  <img src="assets/screenshots/hero-sidebar-five-states.png" alt="Three projects in the vitrum sidebar, each holding several agents: a Claude Code session stopped on an approval prompt, two working, one waiting for input, one failed and two ready, each row carrying its working directory and its branch, beside the transcript of the session waiting for approval" width="900" />
</p>

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/santhreal/vitrum/main/install.sh | sh
```

Nothing has to be installed first. The installer resolves the newest stable
release, verifies it against the checksums published beside it, adds the
GTK or WebView2 runtime this machine is missing, and writes a launcher
entry. On Windows the same install runs from PowerShell:
`irm https://raw.githubusercontent.com/santhreal/vitrum/main/install.ps1 | iex`.

`vitrum update` installs the next release in place. Options, source builds and
uninstall: [docs/install.md](docs/install.md).

<p align="center">
  <img src="assets/screenshots/launcher.png" alt="The vitrum launcher over a sidebar where a Claude Code session waits for approval, two sessions are working, one waits for input and one has failed: a directory to start in, a name to run, saved entries for Claude Code, Codex, Gemini CLI, opencode and veyyon, and a numbered list of running agents to switch to" width="900" />
</p>

<p align="center">
  <img src="assets/screenshots/settings-appearance.png" alt="vitrum settings on the Appearance tab over a sidebar where a Codex session is working, a Claude Code session waits for approval and one has failed: theme, density, text scale, reduce motion and window opacity" width="900" />
</p>

## Documentation

| | |
|---|---|
| [install.md](docs/install.md) | install options, source builds, uninstall |
| [keys.md](docs/keys.md) | keyboard shortcuts and presets |
| [states.md](docs/states.md) | how an agent declares approval and input |
| [appearance.md](docs/appearance.md) | palette, opacity, backdrops, compositor blur |
| [configuration.md](docs/configuration.md) | every setting, its default and its file |
| [remote.md](docs/remote.md) | running the daemon on another machine |
| [performance.md](docs/performance.md) | measured latency, memory and idle cost |
| [architecture.md](docs/architecture.md) | how the pieces fit together |

[CHANGELOG](CHANGELOG.md) · [CONTRIBUTING](CONTRIBUTING.md) ·
[SECURITY](SECURITY.md) · [RELEASING](RELEASING.md)

## Status

vitrum is at version 0.3.1. Linux, macOS and Windows build and pass the test
suite. Interactive use is exercised on Linux. Collision detection is
Linux only, and the terminal pane needs X11.

Sessions run in a daemon and outlive the window. They do not survive the
daemon.

## License

MIT ([LICENSE-MIT](LICENSE-MIT)) or Apache-2.0 ([LICENSE-APACHE](LICENSE-APACHE)),
at your option. Contributions are dual licensed on the same terms unless you
state otherwise.

`vendor-pty/` and `vendor-ghostty-vt-sys/` are forks of other
projects, under the license and copyright they arrived with. `NOTICE` names
them.
