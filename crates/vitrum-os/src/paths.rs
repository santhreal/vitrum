//! Per-platform config, data, cache, state and runtime directories.
//!
//! Resolution is a pure function of a [`Platform`] and a captured [`PathEnv`],
//! which is the only way to test macOS and Windows layouts from a Linux CI box.
//! [`AppPaths::for_current_platform`] is a thin wrapper that captures the real
//! process environment and calls the same function the tests call.
//!
//! Conventions followed:
//!
//! - **Linux**: the XDG Base Directory Specification. `$XDG_CONFIG_HOME`,
//!   `$XDG_DATA_HOME`, `$XDG_CACHE_HOME`, `$XDG_STATE_HOME`,
//!   `$XDG_RUNTIME_DIR`, each falling back to the specified `$HOME` default.
//!   The spec requires that a relative value be treated as invalid; we do that
//!   rather than resolving it against the cwd, because a cwd-relative config
//!   directory means the app stores state wherever it happened to be launched.
//! - **macOS**: `~/Library/Application Support/<bundle id>` for config, data
//!   and state, `~/Library/Caches/<bundle id>` for cache. Apple does not
//!   separate config from data, and inventing a split would put files where no
//!   macOS tool looks for them.
//! - **Windows**: roaming `%APPDATA%\<org>\<app>\{config,data}` and local
//!   `%LOCALAPPDATA%\<org>\<app>\{cache,state,run}`. Roaming is for settings
//!   that should follow a domain user between machines; caches must not, which
//!   is precisely why Windows has two roots.

use core::fmt;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::branding::{APP_NAME, BUNDLE_ID, ORG_NAME};

/// Which platform's directory convention to apply.
///
/// Explicit rather than implied by `cfg!` so the resolution logic for all three
/// platforms is reachable, and therefore testable, from any one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Platform {
    /// XDG base directories, also applied to the other desktop Unixes.
    Linux,
    /// `~/Library/Application Support`, `~/Library/Caches` and friends, keyed
    /// by bundle identifier rather than by application name.
    MacOs,
    /// `%APPDATA%` and `%LOCALAPPDATA%`, keyed by organisation then
    /// application, as the Windows convention expects.
    Windows,
}

impl Platform {
    /// The platform this binary was compiled for.
    ///
    /// Anything not Linux, macOS or Windows resolves as Linux, because the
    /// remaining desktop Unixes (the BSDs, illumos) follow XDG.
    pub const fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self::MacOs
        }
        #[cfg(target_os = "windows")]
        {
            Self::Windows
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            Self::Linux
        }
    }

    /// Stable machine token, used in reports and tests.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::MacOs => "macos",
            Self::Windows => "windows",
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `pad`, not `write_str`: a report column uses `{:<16}` and
        // `write_str` silently discards the width.
        f.pad(self.as_str())
    }
}

/// The subset of the environment that directory resolution reads.
///
/// Captured up front rather than read through `std::env::var` at each lookup so
/// resolution is deterministic and testable, and so a test never has to mutate
/// process-global environment state that another test thread can observe.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PathEnv {
    vars: BTreeMap<String, String>,
}

impl PathEnv {
    /// Capture the variables this module consults from the live process.
    pub fn from_process() -> Self {
        const KEYS: [&str; 12] = [
            "HOME",
            "USERPROFILE",
            "HOMEDRIVE",
            "HOMEPATH",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "XDG_STATE_HOME",
            "XDG_RUNTIME_DIR",
            "TMPDIR",
            "APPDATA",
            "LOCALAPPDATA",
        ];
        let mut vars = BTreeMap::new();
        for key in KEYS {
            if let Ok(value) = std::env::var(key) {
                vars.insert(key.to_string(), value);
            }
        }
        Self { vars }
    }

    /// Build a synthetic environment.
    pub fn from_pairs<K, V, I>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self {
            vars: pairs.into_iter().map(|(k, v)| (k.into(), v.into())).collect(),
        }
    }

    /// A variable's value, treating an empty value as unset.
    ///
    /// An exported-but-empty variable is the normal shape of "I cleared this",
    /// and reading it as a path yields the filesystem root.
    fn get(&self, key: &str) -> Option<&str> {
        self.vars.get(key).map(String::as_str).filter(|v| !v.is_empty())
    }

    /// A variable that must hold an absolute path, per the XDG requirement that
    /// relative values be ignored.
    fn absolute(&self, key: &str) -> Option<&str> {
        self.get(key).filter(|v| Path::new(v).is_absolute())
    }
}

/// Resolved directories for one platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    /// User settings that should be backed up and, on Windows, roam.
    pub config_dir: PathBuf,
    /// Application-owned data that is not settings and not disposable.
    pub data_dir: PathBuf,
    /// Regenerable data. Safe to delete at any time.
    pub cache_dir: PathBuf,
    /// State that persists across runs but is not user-editable settings:
    /// window geometry, last-open session.
    pub state_dir: PathBuf,
    /// Per-user, per-boot scratch for the single-instance lock and socket.
    pub runtime_dir: PathBuf,
}

impl AppPaths {
    /// Resolve for the platform this binary was compiled for, from the live
    /// environment.
    pub fn for_current_platform() -> Result<Self, PathError> {
        Self::resolve(Platform::current(), &PathEnv::from_process())
    }

    /// Resolve for an explicit platform and environment.
    pub fn resolve(platform: Platform, env: &PathEnv) -> Result<Self, PathError> {
        match platform {
            Platform::Linux => Self::resolve_linux(env),
            Platform::MacOs => Self::resolve_macos(env),
            Platform::Windows => Self::resolve_windows(env),
        }
    }

    fn resolve_linux(env: &PathEnv) -> Result<Self, PathError> {
        let home = env
            .get("HOME")
            .ok_or(PathError::MissingEnv { platform: Platform::Linux, var: "HOME" })?;
        let home = Path::new(home);

        let base = |var: &str, default: &str| -> PathBuf {
            match env.absolute(var) {
                Some(v) => Path::new(v).join(APP_NAME),
                None => home.join(default).join(APP_NAME),
            }
        };

        let config_dir = base("XDG_CONFIG_HOME", ".config");
        let data_dir = base("XDG_DATA_HOME", ".local/share");
        let cache_dir = base("XDG_CACHE_HOME", ".cache");
        let state_dir = base("XDG_STATE_HOME", ".local/state");
        // No XDG_RUNTIME_DIR means no logind session. Falling back to /tmp
        // would put a socket in a world-writable directory; nesting under the
        // already per-user cache directory keeps the ownership guarantee that
        // makes the single-instance lock meaningful.
        let runtime_dir = match env.absolute("XDG_RUNTIME_DIR") {
            Some(v) => Path::new(v).join(APP_NAME),
            None => cache_dir.join("run"),
        };

        Ok(Self { config_dir, data_dir, cache_dir, state_dir, runtime_dir })
    }

    fn resolve_macos(env: &PathEnv) -> Result<Self, PathError> {
        let home = env
            .get("HOME")
            .ok_or(PathError::MissingEnv { platform: Platform::MacOs, var: "HOME" })?;
        let home = Path::new(home);

        let support = home.join("Library/Application Support").join(BUNDLE_ID);
        let cache_dir = home.join("Library/Caches").join(BUNDLE_ID);
        // $TMPDIR on macOS is a per-user directory under /var/folders that the
        // system prunes, which is exactly the runtime-dir contract.
        let runtime_dir = match env.absolute("TMPDIR") {
            Some(v) => Path::new(v).join(BUNDLE_ID),
            None => support.join("run"),
        };

        Ok(Self {
            config_dir: support.clone(),
            data_dir: support.clone(),
            cache_dir,
            state_dir: support,
            runtime_dir,
        })
    }

    fn resolve_windows(env: &PathEnv) -> Result<Self, PathError> {
        let roaming = env
            .get("APPDATA")
            .ok_or(PathError::MissingEnv { platform: Platform::Windows, var: "APPDATA" })?;
        let local = env
            .get("LOCALAPPDATA")
            .ok_or(PathError::MissingEnv { platform: Platform::Windows, var: "LOCALAPPDATA" })?;

        let roaming = Path::new(roaming).join(ORG_NAME).join(APP_NAME);
        let local = Path::new(local).join(ORG_NAME).join(APP_NAME);

        Ok(Self {
            config_dir: roaming.join("config"),
            data_dir: roaming.join("data"),
            cache_dir: local.join("cache"),
            state_dir: local.join("state"),
            runtime_dir: local.join("run"),
        })
    }

    /// Where window geometry is persisted.
    ///
    /// Plural: the file holds one slot per window ordinal, not one window.
    /// It said `window.json` for a while and the client wrote `windows.json`
    /// through a path it built itself, so this returned a name nothing on
    /// disk ever had.
    pub fn window_state_file(&self) -> PathBuf {
        self.state_dir.join("windows.json")
    }

    /// Advisory lock proving one instance owns this user's session.
    pub fn instance_lock_file(&self) -> PathBuf {
        self.runtime_dir.join("instance.lock")
    }

    /// Where a second launch hands its activation to the first.
    ///
    /// On Unix this is a filesystem socket path and is length-limited; see
    /// [`crate::single_instance`].
    pub fn instance_socket_path(&self) -> PathBuf {
        self.runtime_dir.join("instance.sock")
    }

    /// Create every directory. Called once at startup.
    pub fn create_all(&self) -> std::io::Result<()> {
        for dir in [
            &self.config_dir,
            &self.data_dir,
            &self.cache_dir,
            &self.state_dir,
            &self.runtime_dir,
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

/// The user's home directory for the running platform.
///
/// Exposed because callers shorten paths for display (`~/src/foo`) and reading
/// `$HOME` directly gets Windows wrong: there is no `HOME` there, and
/// `%USERPROFILE%` is absent in some service contexts where `%HOMEDRIVE%` plus
/// `%HOMEPATH%` still resolves.
pub fn home_dir() -> Option<PathBuf> {
    home_dir_from(Platform::current(), &PathEnv::from_process())
}

/// [`home_dir`] as a pure function, for a specific platform and environment.
pub(crate) fn home_dir_from(platform: Platform, env: &PathEnv) -> Option<PathBuf> {
    match platform {
        Platform::Linux | Platform::MacOs => env.get("HOME").map(PathBuf::from),
        Platform::Windows => {
            if let Some(profile) = env.get("USERPROFILE") {
                return Some(PathBuf::from(profile));
            }
            let drive = env.get("HOMEDRIVE")?;
            let path = env.get("HOMEPATH")?;
            Some(PathBuf::from(format!("{drive}{path}")))
        }
    }
}

/// Why directory resolution failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    /// A variable with no defensible default was unset or empty.
    MissingEnv {
        /// Which platform's convention was being applied when the lookup failed.
        platform: Platform,
        /// Name of the variable, without the leading `$` or `%`.
        var: &'static str,
    },
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingEnv { platform, var } => write!(
                f,
                "cannot resolve {platform} application directories: ${var} is unset or empty"
            ),
        }
    }
}

impl core::error::Error for PathError {}
