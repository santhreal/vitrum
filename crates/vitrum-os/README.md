# vitrum-os

The only place in vitrum where a platform API is called. Eight things separate a
window with a terminal in it from an application the operating system treats as
a citizen, and this crate is all eight on Linux, macOS, and Windows:
notifications, a badge, a tray icon, single-instance behaviour, theme following,
window state that survives a restart, a URL scheme, and per-platform
directories.

**A capability that is not available says so.** There is no silent no-op
anywhere. Every backend either does the thing or returns an `Unavailable`
carrying an `UnavailableKind` and a sentence naming the missing piece.
"macOS has no taskbar overlay" and "your desktop has no notification daemon
running" call for different UI, and a `bool` gives the caller neither.

```rust
use vitrum_os::{AppPaths, PathEnv, Platform};

let env = PathEnv::new(Platform::Linux)
    .with("HOME", "/home/mk")
    .with("XDG_CONFIG_HOME", "/home/mk/.config");
let paths = AppPaths::resolve(&env).unwrap();
assert!(paths.config_dir().ends_with("vitrum"));
```

Nothing here polls. The theme watcher parks a thread on a D-Bus signal, a
distributed notification, or `RegNotifyChangeKeyValue`; the single-instance
listener parks in `accept`. There is no timer in this crate.

Part of [vitrum](https://github.com/santhreal/vitrum). MIT licensed.
