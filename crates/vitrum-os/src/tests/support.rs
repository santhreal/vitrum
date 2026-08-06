//! Shared test helpers.

use std::path::{Path, PathBuf};

/// Assert a resolved path equals a `/`-separated expectation.
///
/// Resolution builds paths with `PathBuf::join`, which emits the host's
/// separator. Normalising here lets one assertion pin the exact segment
/// sequence on Linux, macOS and Windows, which is the thing under test; the
/// separator itself is `std`'s job, not ours.
pub fn assert_path(actual: &Path, expected: &str) {
    let normalised = actual.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
    assert_eq!(normalised, expected, "resolved path differs");
}

/// A directory that removes itself, so tests never leave state behind and never
/// collide when the suite runs in parallel.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new(tag: &str) -> Self {
        // Process id plus a per-call counter: unique across concurrent test
        // binaries and across threads within one.
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("vitrum-os-test-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).expect("temp dir must be creatable");
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
