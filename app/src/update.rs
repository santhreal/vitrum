//! Finding, verifying and installing a newer vitrum.
//!
//! One module serves both ways in, because an updater that behaves differently
//! depending on whether you typed the command or clicked the button is two
//! updaters and one of them is wrong. [`check`] answers what is available and
//! [`install`] puts it in place; `vitrum update` and the window's update
//! control both call exactly these.
//!
//! **Releases, not the branch.** The source of truth is the latest published
//! GitHub release, never `main`, because the tip of a branch carries work that
//! has not been released and an updater that installs it is not an updater.
//!
//! **The archive is verified before anything is replaced.** A downloaded
//! binary is remote code that is about to become the program the operator
//! runs. Its SHA-256 must match the `SHA256SUMS` published alongside it, and a
//! mismatch aborts with the two digests rather than continuing.
//!
//! **The swap cannot half-happen.** `vitrum` and `vitrum-server` speak a
//! versioned protocol to each other, so a run that replaced one and failed on
//! the other would leave a pair that refuses to talk. Both are staged next to
//! their targets first, and only then renamed in, which on every platform this
//! runs on is atomic within a filesystem.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use semver::Version;
use sha2::{Digest, Sha256};

/// Where releases are published.
pub const REPO: &str = "santhreal/vitrum";

/// Version this binary was built at.
pub fn current_version() -> Version {
    // A malformed `CARGO_PKG_VERSION` cannot happen: cargo refuses to build a
    // manifest whose version is not semver.
    Version::parse(env!("CARGO_PKG_VERSION")).expect("crate version is semver")
}

/// The platform triple whose asset this build needs.
///
/// Hardcoding one triple per platform rather than reading it at runtime is
/// deliberate: the value must match the machine that produced the binary, and
/// that is a compile-time fact.
pub const TARGET: &str = env!("VITRUM_TARGET");

/// How long any single network operation may take.
///
/// An updater that hangs is worse than one that fails, because the operator
/// cannot tell it apart from a slow download and waits.
const NET_TIMEOUT: Duration = Duration::from_secs(30);

/// What the newest release is, and whether it is worth installing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Available {
    /// Version parsed from the release tag.
    pub version: Version,
    /// Tag exactly as published, needed to build asset URLs.
    pub tag: String,
    /// URL of the archive for this platform, if the release published one.
    pub asset_url: Option<String>,
    /// URL of the checksum file, if the release published one.
    pub sums_url: Option<String>,
}

/// The outcome of asking what is available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// The newest release is the one already running.
    UpToDate { version: Version },
    /// A newer release exists and carries an asset this platform can install.
    Ready(Available),
    /// A newer release exists but published nothing for this platform.
    ///
    /// Reported rather than treated as up to date, because the operator is
    /// behind and the reason is not something they can fix by waiting.
    NoAssetForPlatform { version: Version, target: String },
    /// The project has published no releases at all yet.
    NoReleases,
}

/// Ask GitHub what the newest release is.
///
/// Network errors propagate. This is called from a terminal command that must
/// exit non-zero when it could not answer, and from a window that must say it
/// could not reach the network rather than quietly showing "up to date".
pub fn check() -> Result<Status> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let response = agent()
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .call();

    let body = match response {
        Ok(r) => r.into_string().context("reading the release response")?,
        // A project with no releases answers 404 on this endpoint, and so does
        // a repository that does not exist. They are indistinguishable here
        // and both mean the same thing to the operator: nothing to install.
        Err(ureq::Error::Status(404, _)) => return Ok(Status::NoReleases),
        Err(ureq::Error::Status(code, r)) => {
            let detail = r.into_string().unwrap_or_default();
            let detail = vitrum_proto::error_text(detail.trim());
            bail!("GitHub answered {code} for {REPO}: {detail}");
        }
        Err(e) => return Err(anyhow!(e)).context(format!("asking GitHub about {REPO}")),
    };

    let release: serde_json::Value =
        serde_json::from_str(&body).context("parsing the release response")?;
    parse_release(&release)
}

/// Turn one GitHub release object into a [`Status`].
///
/// Split from [`check`] so the shape of a real release can be tested without a
/// network, which is the only way to test the case that matters: a release
/// that exists but has no asset for the platform asking.
pub fn parse_release(release: &serde_json::Value) -> Result<Status> {
    let tag = release
        .get("tag_name")
        .and_then(|t| t.as_str())
        .ok_or_else(|| anyhow!("release has no tag_name"))?
        .to_string();

    let version = Version::parse(tag.trim_start_matches('v'))
        .with_context(|| format!("release tag {tag} is not a semver version"))?;

    if version <= current_version() {
        return Ok(Status::UpToDate {
            version: current_version(),
        });
    }

    let wanted = archive_name(&version);
    let mut asset_url = None;
    let mut sums_url = None;
    if let Some(assets) = release.get("assets").and_then(|a| a.as_array()) {
        for asset in assets {
            let (Some(name), Some(url)) = (
                asset.get("name").and_then(|n| n.as_str()),
                asset.get("browser_download_url").and_then(|u| u.as_str()),
            ) else {
                continue;
            };
            if name == wanted {
                asset_url = Some(url.to_string());
            } else if name == "SHA256SUMS" {
                sums_url = Some(url.to_string());
            }
        }
    }

    match asset_url {
        Some(_) => Ok(Status::Ready(Available {
            version,
            tag,
            asset_url,
            sums_url,
        })),
        None => Ok(Status::NoAssetForPlatform {
            version,
            target: TARGET.to_string(),
        }),
    }
}

/// Name of the archive a release publishes for this platform.
///
/// Shared by the updater and by `RELEASING.md`, so the thing that builds the
/// asset and the thing that looks for it cannot drift.
pub fn archive_name(version: &Version) -> String {
    format!("vitrum-{version}-{TARGET}.tar.gz")
}

/// Download, verify and swap in a release.
///
/// `into` is the directory holding the binaries to replace, normally the one
/// containing the running executable. Progress lines go to `report` so the
/// terminal can print them and the window can show them without this module
/// knowing which it is talking to.
pub fn install(available: &Available, into: &Path, report: &mut dyn FnMut(&str)) -> Result<()> {
    let url = available
        .asset_url
        .as_deref()
        .ok_or_else(|| anyhow!("release {} has no asset for {TARGET}", available.tag))?;

    // Refused before the download, not after. An unverified archive is remote
    // code about to replace the program the operator runs, so "the release
    // forgot to publish sums" is a reason to stop; pulling twenty megabytes
    // first and then refusing wastes their bandwidth to reach the same answer.
    let sums_url = available.sums_url.as_deref().ok_or_else(|| {
        anyhow!(
            "release {} published no SHA256SUMS; refusing to install an unverified binary",
            available.tag
        )
    })?;

    report(&format!("downloading {}", archive_name(&available.version)));
    let archive = download(url).with_context(|| format!("downloading {url}"))?;

    report("verifying checksum");
    let sums = download(sums_url)
        .with_context(|| format!("downloading {sums_url}"))
        .and_then(|b| String::from_utf8(b).context("SHA256SUMS is not text"))?;
    verify(&archive, &sums, &archive_name(&available.version))?;

    report("unpacking");
    let staged = unpack(&archive, into)?;

    report("replacing binaries");
    for (temp, target) in &staged {
        swap_in(temp, target).with_context(|| format!("replacing {}", target.display()))?;
    }

    report(&format!("updated to {}", available.version));
    Ok(())
}

/// Read a whole response body, refusing one that is implausibly large.
fn download(url: &str) -> Result<Vec<u8>> {
    let response = agent().get(url).call()?;
    let mut body = Vec::new();
    // A release archive for this program is single-digit megabytes. The cap
    // exists so a redirect to something enormous cannot exhaust memory on the
    // machine being updated.
    const MAX: u64 = 256 * 1024 * 1024;
    response
        .into_reader()
        .take(MAX)
        .read_to_end(&mut body)
        .context("reading the response body")?;
    Ok(body)
}

/// Check `archive` against the digest `sums` publishes for `name`.
pub fn verify(archive: &[u8], sums: &str, name: &str) -> Result<()> {
    let expected = sums
        .lines()
        .find_map(|line| {
            let mut parts = line.split_whitespace();
            let digest = parts.next()?;
            let file = parts.next()?.trim_start_matches('*');
            (file == name).then_some(digest)
        })
        .ok_or_else(|| anyhow!("SHA256SUMS does not list {name}"))?;

    let actual = hex(&Sha256::digest(archive));
    if !actual.eq_ignore_ascii_case(expected) {
        bail!("checksum mismatch for {name}\n  published: {expected}\n  downloaded: {actual}");
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Unpack every binary in the archive beside its target, returning the pairs
/// still to be renamed.
///
/// Nothing is renamed here. The caller does that in one pass so a failure
/// while unpacking the second binary cannot leave the first one already
/// swapped, which would pair a new client with an old daemon.
fn unpack(archive: &[u8], into: &Path) -> Result<Vec<(PathBuf, PathBuf)>> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let mut staged = Vec::new();
    for entry in tar.entries().context("reading the archive")? {
        let mut entry = entry.context("reading an archive entry")?;
        let path = entry.path().context("an archive entry has no path")?;
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Only the two binaries are taken, by name. An archive is untrusted
        // input, and honouring its paths is how an archive writes outside the
        // directory it was unpacked into.
        if name != "vitrum"
            && name != "vitrum-server"
            && name != "vitrum.exe"
            && name != "vitrum-server.exe"
        {
            continue;
        }
        let target = into.join(name);
        let temp = into.join(format!(".{name}.incoming"));
        let mut out =
            fs::File::create(&temp).with_context(|| format!("staging {}", temp.display()))?;
        std::io::copy(&mut entry, &mut out)
            .with_context(|| format!("writing {}", temp.display()))?;
        drop(out);
        set_executable(&temp)?;
        staged.push((temp, target));
    }
    if staged.is_empty() {
        bail!("the archive contained no vitrum binaries");
    }
    Ok(staged)
}

/// Move a staged binary onto the one it replaces.
///
/// On Unix this is one `rename`, which is atomic and works while the target is
/// executing: the running process keeps the old inode and the name points at
/// the new file. Nothing else is needed and nothing is left behind.
///
/// Windows refuses to replace a running image, and `vitrum.exe` updating
/// itself is exactly that case. A running image CAN be renamed though, so the
/// old one is moved aside first and the new one takes the freed name. Deleting
/// the displaced file fails while it is still executing, which is expected and
/// ignored; [`sweep_displaced`] removes it on the next launch.
fn swap_in(temp: &Path, target: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        if target.exists() {
            let displaced = target.with_extension("old");
            let _ = fs::remove_file(&displaced);
            fs::rename(target, &displaced)
                .with_context(|| format!("moving the running {} aside", target.display()))?;
            fs::rename(temp, target).inspect_err(|_| {
                // Put it back rather than leave the operator with no binary
                // under the name their shortcut and PATH point at.
                let _ = fs::rename(&displaced, target);
            })?;
            let _ = fs::remove_file(&displaced);
            return Ok(());
        }
    }
    fs::rename(temp, target)?;
    Ok(())
}

/// Delete an image displaced by a previous update, once it is no longer running.
///
/// Only Windows leaves one. Called at startup, where the process that was
/// holding it open has certainly exited, because it is the one that was
/// replaced.
pub fn sweep_displaced(dir: &Path) {
    if !cfg!(windows) {
        return;
    }
    for name in ["vitrum.exe", "vitrum-server.exe"] {
        let _ = fs::remove_file(dir.join(name).with_extension("old"));
    }
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .with_context(|| format!("making {} executable", path.display()))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(NET_TIMEOUT)
        .timeout_read(NET_TIMEOUT)
        // GitHub rejects requests with no user agent.
        .user_agent(concat!("vitrum/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// What to say once the new binaries are on disk.
///
/// "Restart vitrum" is not the whole truth and the missing half is the part
/// that costs something. This product's defining behaviour is that the daemon
/// owns the PTYs and outlives every window, so restarting the window swaps the
/// client and leaves the OLD daemon running: it is a live process, and
/// replacing its file on disk does not touch it. It keeps serving until it is
/// restarted, and restarting it ends every session it is holding.
///
/// So the operator has to be told two separate things, because they have very
/// different costs and only one of them is free.
pub const AFTER_INSTALL: &str = "\
restart vitrum to run the new client.\n\
The daemon keeps running the old version until it is restarted, and restarting \
it ends every session it is holding. Do that when your agents are idle.";

/// Directory whose binaries an update should replace.
pub fn install_dir() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("finding the running executable")?;
    let dir = exe
        .parent()
        .ok_or_else(|| anyhow!("the running executable has no parent directory"))?;
    Ok(dir.to_path_buf())
}

/// Whether the install directory can be written without elevation.
///
/// Checked before downloading anything. A package-managed or system-wide
/// install must be updated by whatever installed it, and finding that out
/// after a 20 MB download and a checksum pass is a waste of the operator's
/// time and bandwidth.
pub fn writable(dir: &Path) -> bool {
    let probe = dir.join(".vitrum-write-probe");
    match fs::File::create(&probe) {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// `vitrum update` — check for a newer release and install it.
///
/// Returns the process exit code. Codes are load-bearing here because this is
/// the command people put in a shell alias or a cron entry: `0` means the
/// installed version is now the newest one, `1` means it is not and why is on
/// stderr. "Already up to date" is success, because nothing is wrong.
pub fn run_update(args: &[String]) -> i32 {
    let mut check_only = false;
    for arg in args {
        match arg.as_str() {
            "--check" => check_only = true,
            "-h" | "--help" => {
                println!("{}", update_usage());
                return 0;
            }
            other => {
                eprintln!("unknown argument {other}\n\n{}", update_usage());
                return 2;
            }
        }
    }

    let status = match check() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("could not check for updates: {e:#}");
            return 1;
        }
    };

    match status {
        Status::UpToDate { version } => {
            println!("vitrum {version} is the newest release");
            0
        }
        Status::NoReleases => {
            println!(
                "no releases published for {} yet; you are on {}",
                REPO,
                current_version()
            );
            0
        }
        Status::NoAssetForPlatform { version, target } => {
            eprintln!(
                "vitrum {version} is available but published no build for {target}.\n\
                 Build it from source: https://github.com/{}/releases/tag/v{version}",
                REPO
            );
            1
        }
        Status::Ready(available) => {
            println!(
                "vitrum {} is available (you have {})",
                available.version,
                current_version()
            );
            if check_only {
                return 0;
            }
            let dir = match install_dir() {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("could not find where vitrum is installed: {e:#}");
                    return 1;
                }
            };
            // Checked before the download rather than after, so a
            // package-managed install fails in a second instead of after
            // fetching and verifying an archive it can never write.
            if !writable(&dir) {
                eprintln!(
                    "cannot write to {}.\n\
                     This copy was installed by something else; update it the same way.",
                    dir.display()
                );
                return 1;
            }
            match install(&available, &dir, &mut |line| println!("{line}")) {
                Ok(()) => {
                    println!("{}", AFTER_INSTALL);
                    0
                }
                Err(e) => {
                    eprintln!("update failed: {e:#}");
                    1
                }
            }
        }
    }
}

fn update_usage() -> String {
    format!(
        "vitrum update - install the newest published release\n\n\
         usage: vitrum update [--check]\n\n\
         Reads the latest release of {}, never the branch, because the tip of\n\
         a branch carries work that has not been released.\n\n\
         The archive's SHA-256 must match the SHA256SUMS published beside it.\n\
         A release with no sums is refused rather than trusted.\n\n\
         options:\n  \
         --check              report what is available and install nothing\n  \
         -h, --help           show this message\n\n\
         exit status:\n  \
         0                    already newest, or updated successfully\n  \
         1                    could not check, or could not install\n",
        REPO
    )
}

#[cfg(test)]
mod an_update_is_remote_code;

/// The whole path, over a real socket.
///
/// Every other test here exercises one step with the others stubbed out. This
/// one runs `install` exactly as the terminal command and the window button
/// run it: an HTTP fetch of an archive, an HTTP fetch of the sums, a checksum
/// pass, an unpack, and the rename that replaces the running program. The
/// pieces were each correct in isolation while the whole was wired wrong more
/// than once, which is the only reason to pay for a socket in a unit test.
#[cfg(test)]
mod the_whole_install_over_a_socket;

#[cfg(test)]
mod what_an_update_actually_leaves_running;

/// The mark belongs on the launcher, not in the window.
///
/// Documented in `assets/logo/README.md`; enforced here, because a rule that
/// lives only in a document is a rule that survives exactly until somebody
/// wants a splash of brand in the titlebar.
#[cfg(test)]
mod where_the_mark_may_appear;
