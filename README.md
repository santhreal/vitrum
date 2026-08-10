<p align="center">
  <img src="assets/logo/vitrum.svg" alt="vitrum" width="96" height="96" />
</p>

<h1 align="center">vitrum</h1>

vitrum is one interface for managing agent TUIs: Codex, Claude Code, Gemini CLI
and opencode. Every agent gets a row in a sidebar carrying its branch, its
working directory and its state, grouped by project.

<p align="center">
  <img src="assets/screenshots/hero-sidebar-five-states.png" alt="Three projects in the vitrum sidebar, each holding several agents: one Claude Code session waiting for approval, one Codex session working, one Gemini CLI session ready, one waiting for input and one failed, beside the transcript of the session stopped on an approval prompt" width="900" />
</p>

<p align="center">
  <img src="assets/screenshots/launcher.png" alt="The vitrum launcher over a sidebar where a Claude Code session waits for approval and a Codex session is working: a directory, a name to run, saved entries for Claude Code, Codex, Gemini CLI, opencode and veyyon, and a numbered list of agents to start in each project" width="440" />
  <img src="assets/screenshots/settings-appearance.png" alt="vitrum settings over a sidebar where a Codex session is working and a Claude Code session waits for approval: theme, density, text scale, reduce motion and window opacity" width="440" />
</p>

## Install

Linux and macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/santhreal/vitrum/main/install.sh | sh
```

Windows:

```powershell
irm https://raw.githubusercontent.com/santhreal/vitrum/main/install.ps1 | iex
```

`vitrum update` installs the next release in place. Options, source builds and
uninstall: [docs/install.md](docs/install.md).

## Documentation

| | |
|---|---|
| [install.md](docs/install.md) | install options, source builds, uninstall |
| [keys.md](docs/keys.md) | keyboard shortcuts and presets |
| [states.md](docs/states.md) | how an agent declares approval and input |
| [appearance.md](docs/appearance.md) | opacity, backdrops, compositor blur |
| [remote.md](docs/remote.md) | running the daemon on another machine |
| [configuration.md](docs/configuration.md) | file locations, daemon flags |
| [performance.md](docs/performance.md) | measured memory and idle cost |
| [architecture.md](docs/architecture.md) | crate layout |

[CHANGELOG](CHANGELOG.md) · [CONTRIBUTING](CONTRIBUTING.md) ·
[SECURITY](SECURITY.md) · [RELEASING](RELEASING.md)

## Status

vitrum is at version 0.2.1. Linux, macOS and Windows build and pass the
test suite. Interactive use is exercised on Linux. Collision detection is
Linux only.

Sessions run in a daemon and outlive the window. They do not survive the
daemon.

## License

MIT ([LICENSE-MIT](LICENSE-MIT)) or Apache-2.0 ([LICENSE-APACHE](LICENSE-APACHE)),
at your option. Contributions are dual licensed on the same terms unless you
state otherwise.

`vendor/` and `vendor-pty/` are forks of other projects, under the MIT license
and copyright they arrived with. `NOTICE` names them.
