//! Finding, verifying and staging a newer vitrum.
//!
//! One module serves both ways in, because an updater that behaves differently
//! depending on whether you typed the command or clicked the button is two
//! updaters and one of them is wrong. [`check`] answers what is available and
//! [`install`] puts it in place; `vitrum update` and the window's update
//! control both call exactly these.
//!
//! **Releases, not the branch.** The source of truth is a published GitHub
//! release, never `main`, because the tip of a branch carries work that has
//! not been released and an updater that installs it is not an updater. Which
//! releases count is a [`Channel`]: stable reads the latest non-prerelease,
//! nightly also reads the single moving `nightly` tag, which is published as a
//! prerelease so the stable lookup never returns it.
//!
//! **The archive is verified before anything is written.** A downloaded
//! binary is remote code that is about to become the program the operator
//! runs. Its SHA-256 must match the `SHA256SUMS` published alongside it, and a
//! mismatch aborts with the two digests rather than continuing. This is the
//! same on both channels: a nightly archive is checked against the nightly
//! release's `SHA256SUMS` or it is not installed.
//!
//! **Nothing is swapped under a running client.** vitrum releases many times a
//! day, and an update that replaced the binaries of a live client would make
//! every one of those releases an interruption. [`install`] downloads and
//! verifies into a staging directory beside the binaries and records what is
//! there; [`apply_staged`] renames them in at the start of the next run,
//! before the window opens.
//!
//! **The swap cannot half-happen.** `vitrum` and `vitrum-server` speak a
//! versioned protocol to each other, so a run that replaced one and failed on
//! the other would leave a pair that refuses to talk. The staging record is
//! removed only after every rename has completed, so a crash between the two
//! renames leaves the record and the file that is still waiting, and the next
//! start finishes the job.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use vitrum_proto::exit::{self, Exit};

/// Where releases are published.
pub const REPO: &str = "santhreal/vitrum";

/// Root of the GitHub REST API.
///
/// A parameter of the fetch functions rather than a hardcoded string inside
/// them, so the resolver can be run against a local socket in a test without
/// an environment variable that every other test in the process would see.
pub const API_BASE: &str = "https://api.github.com";

/// The one tag the nightly channel ever resolves.
///
/// A single moving tag, not one tag per night. Retaining a tag per build would
/// make the release list unreadable within a week, and nothing installs an old
/// nightly on purpose.
pub const NIGHTLY_TAG: &str = "nightly";

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

/// How long a running window waits between quiet update checks.
///
/// Four hours. The rate this has to keep up with is the release rate, and this
/// project publishes many times a day, so a daily check leaves an operator
/// several builds behind for most of the day. Checking every few minutes would
/// spend an API budget on an answer nobody asked for and would make polling
/// the most frequent thing the program does. Four hours notices the same day's
/// release without either.
///
/// The first check is not on this schedule: it happens shortly after the
/// window is up, so a launch also answers the question.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(4 * 60 * 60);

/// How often the window re-reads what is staged on disk.
///
/// Separate from [`CHECK_INTERVAL`] and much shorter, because these answer
/// different questions. Whether a newer release exists is a network question
/// asked rarely; whether one is already staged is a single small file read,
/// and it changes when `vitrum update` runs in a terminal beside the window or
/// the About tab finishes a download. A minute is soon enough for a restart
/// prompt and cheap enough to be invisible.
pub const STAGED_POLL: Duration = Duration::from_secs(60);

/// Which stream of releases a build follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Channel {
    /// Published, non-prerelease releases. The default, and what an operator
    /// who has never opened the setting is on.
    #[default]
    Stable,
    /// The moving [`NIGHTLY_TAG`] release, and stable releases when one of
    /// them is newer.
    ///
    /// Nightly is ahead of stable, not instead of it: a nightly build offered
    /// a stable release with a higher version takes it, because that is a
    /// forward move. A stable release older than the running nightly is not
    /// offered, because that is a downgrade nobody asked for.
    Nightly,
}

impl Channel {
    /// The word this channel is written as, in a setting and in a message.
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Nightly => "nightly",
        }
    }
}

/// What the newest release is, and whether it is worth installing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Available {
    /// Version parsed from the release tag, or for a nightly, from the name of
    /// the asset this platform would install.
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

/// What the window may say about an update, in one value.
///
/// The three answers the interface has to distinguish, and no others. An
/// update that is merely available is a different sentence from one that is
/// already on disk: the first asks the operator to spend bandwidth, the second
/// asks them to restart, and only the second is free to act on later.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Standing {
    /// Nothing to say.
    #[default]
    Current,
    /// A newer build exists and has not been downloaded.
    Available { version: Version },
    /// A verified build is staged, and the next start runs it.
    Staged { version: Version },
}

/// What is standing, given what is on disk and what a check last found.
///
/// Staged wins over available: once the bytes are down and verified, what is
/// available is no longer the operator's problem.
pub fn standing(into: &Path, offer: Option<&Available>) -> Standing {
    if let Some(version) = staged(into).and_then(|s| s.version()) {
        return Standing::Staged { version };
    }
    match offer {
        Some(available) => Standing::Available {
            version: available.version.clone(),
        },
        None => Standing::Current,
    }
}

/// Whether the sidebar draws its restart affordance, and for which version.
///
/// `show` is [`crate::state::Settings::show_restart_to_update`]. It is read
/// here and nowhere else in the update path, which is the whole point of the
/// setting: it decides what is drawn and never what is done. Checking,
/// staging and applying do not consult it.
///
/// Only [`Standing::Staged`] produces an affordance. An available update is
/// the titlebar chip's business and has its own dismissal.
pub fn restart_offer(standing: &Standing, show: bool) -> Option<&Version> {
    match standing {
        Standing::Staged { version } if show => Some(version),
        _ => None,
    }
}

/// The label on the restart affordance.
pub const RESTART_TO_UPDATE: &str = "Restart to update";

/// The affordance's full line, naming the version waiting.
pub fn restart_line(version: &Version) -> String {
    format!("{RESTART_TO_UPDATE} to vitrum {version}")
}

/// Whether a quiet titlebar check should surface this status.
///
/// Only a ready newer release becomes chrome, and only when the operator has
/// not already dismissed that exact version. Every other answer is silence:
/// up to date is not news, a missing asset is a sentence for the About tab,
/// and a network error must not invent a badge.
pub fn chrome_offer(status: &Status, ignored: &str) -> Option<Available> {
    match status {
        Status::Ready(available) if available.version.to_string() != ignored.trim() => {
            Some(available.clone())
        }
        _ => None,
    }
}

/// Ask what is available for the titlebar chip and the About tab seed.
///
/// Same answer as [`check`], except `VITRUM_UPDATE_OFFER=<semver>` forces a
/// ready status for demos and screenshots without a network round trip. The
/// override is ignored when empty so an exported blank value cannot silence
/// a real check by accident.
pub fn quiet_check() -> Result<Status> {
    if let Ok(raw) = std::env::var("VITRUM_UPDATE_OFFER") {
        let raw = raw.trim();
        if !raw.is_empty() {
            let version =
                Version::parse(raw).context("VITRUM_UPDATE_OFFER must be a semver version")?;
            return Ok(Status::Ready(Available {
                version: version.clone(),
                tag: format!("v{version}"),
                asset_url: Some(format!(
                    "https://example.invalid/vitrum-{version}-{TARGET}.tar.gz"
                )),
                sums_url: Some("https://example.invalid/SHA256SUMS".into()),
            }));
        }
    }
    check()
}

/// The channel this profile follows.
///
/// Read from the saved settings rather than passed in, because `vitrum update`
/// has no window and no signal to read it from, and an operator who chose
/// nightly in the interface means it for the command too.
pub fn configured_channel() -> Channel {
    crate::state::load_prefs().0.settings.update_channel
}

/// Ask GitHub what the newest release is, on the channel this profile follows.
///
/// Network errors propagate. This is called from a terminal command that must
/// exit non-zero when it could not answer, and from a window that must say it
/// could not reach the network rather than quietly showing "up to date".
pub fn check() -> Result<Status> {
    check_on(configured_channel())
}

/// [`check`] on a named channel.
pub fn check_on(channel: Channel) -> Result<Status> {
    check_at(API_BASE, channel)
}

/// [`check_on`] against a named API root.
pub fn check_at(base: &str, channel: Channel) -> Result<Status> {
    let latest = fetch_release(base, "releases/latest")?;
    // Asked for only on nightly. The stable channel never even requests the
    // prerelease tag, which is the strongest form of "stable never resolves a
    // prerelease": there is no code path on which it could.
    let nightly = match channel {
        Channel::Stable => None,
        Channel::Nightly => fetch_release(base, &format!("releases/tags/{NIGHTLY_TAG}"))?,
    };
    resolve(
        channel,
        latest.as_ref(),
        nightly.as_ref(),
        &current_version(),
    )
}

/// [`check_on`], with an unreachable network as silence rather than an error.
///
/// The quiet check is something this program decided to do, not something the
/// operator asked for, so a laptop on a train must not produce an error the
/// operator has to dismiss. A failure is logged at debug and answered with
/// [`None`], which every caller treats as "nothing to say".
pub fn background_check(channel: Channel) -> Option<Status> {
    background_check_at(API_BASE, channel)
}

/// [`background_check`] against a named API root.
pub fn background_check_at(base: &str, channel: Channel) -> Option<Status> {
    match check_at(base, channel) {
        Ok(status) => Some(status),
        Err(e) => {
            tracing::debug!("update check skipped: {e:#}");
            None
        }
    }
}

/// Fetch one release object. `Ok(None)` when the endpoint answers 404, which
/// is "no releases yet" for the latest endpoint and "no nightly published" for
/// the tag endpoint. Both mean the same thing here: nothing from this source.
fn fetch_release(base: &str, path: &str) -> Result<Option<serde_json::Value>> {
    let url = format!("{}/repos/{REPO}/{path}", base.trim_end_matches('/'));
    let response = agent()
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .call();

    let body = match response {
        Ok(r) => r.into_string().context("reading the release response")?,
        // A project with no releases answers 404 on this endpoint, and so does
        // a repository that does not exist. They are indistinguishable here
        // and both mean the same thing to the operator: nothing to install.
        Err(ureq::Error::Status(404, _)) => return Ok(None),
        Err(ureq::Error::Status(code, r)) => {
            let detail = r.into_string().unwrap_or_default();
            let detail = vitrum_proto::error_text(detail.trim());
            bail!("GitHub answered {code} for {REPO}: {detail}");
        }
        // A transport failure is the operator's network, not GitHub's answer,
        // and it is the one update failure that is worth retrying unchanged.
        Err(e) => {
            return Err(anyhow!(UpdateFault::Unreachable {
                what: format!("the {REPO} release list on GitHub"),
                cause: e.to_string(),
            }));
        }
    };

    serde_json::from_str(&body)
        .context("parsing the release response")
        .map(Some)
}

/// Decide what a channel offers, given the release objects it may consider.
///
/// Pure, and separate from the fetching, because every rule worth getting
/// right here is a comparison between two versions and none of them needs a
/// socket to test.
pub fn resolve(
    channel: Channel,
    latest: Option<&serde_json::Value>,
    nightly: Option<&serde_json::Value>,
    current: &Version,
) -> Result<Status> {
    let stable = latest.map(candidate_stable).transpose()?;
    let nightly = match channel {
        // A stable build ignores the nightly release even if it is handed one.
        Channel::Stable => None,
        Channel::Nightly => nightly.and_then(candidate_nightly),
    };

    let best = match (stable, nightly) {
        // The higher version wins outright, which is both halves of the rule:
        // a nightly build takes a newer stable, and it is never moved back to
        // an older one.
        (Some(s), Some(n)) => Some(if n.version >= s.version { n } else { s }),
        (s, n) => s.or(n),
    };
    Ok(status_of(best, current))
}

/// What a candidate means to a build running `current`.
fn status_of(best: Option<Available>, current: &Version) -> Status {
    match best {
        None => Status::NoReleases,
        Some(a) if a.version <= *current => Status::UpToDate {
            version: current.clone(),
        },
        Some(a) if a.asset_url.is_none() => Status::NoAssetForPlatform {
            version: a.version,
            target: TARGET.to_string(),
        },
        Some(a) => Status::Ready(a),
    }
}

/// A stable release reduced to what the updater needs.
///
/// The version comes from the tag, which for a stable release is the whole
/// point of the tag. A tag that is not a version is an error and not a shrug:
/// silently skipping it would leave the operator on an old build with no
/// explanation.
fn candidate_stable(release: &serde_json::Value) -> Result<Available> {
    let tag = release
        .get("tag_name")
        .and_then(|t| t.as_str())
        .ok_or_else(|| anyhow!("release has no tag_name"))?
        .to_string();

    let version = Version::parse(tag.trim_start_matches('v'))
        .with_context(|| format!("release tag {tag} is not a semver version"))?;

    let wanted = archive_name(&version);
    let mut asset_url = None;
    let mut sums_url = None;
    for (name, url) in assets_of(release) {
        if name == wanted {
            asset_url = Some(url);
        } else if name == "SHA256SUMS" {
            sums_url = Some(url);
        }
    }

    Ok(Available {
        version,
        tag,
        asset_url,
        sums_url,
    })
}

/// The nightly release reduced to what the updater needs.
///
/// Its tag is the word `nightly` and carries no version, so the version is
/// read from the name of the asset this platform would install. That filename
/// is the one string the workflow and the updater both have to agree on, and
/// it already had to be right for the download to work.
///
/// A nightly release with no asset for this platform is [`None`] rather than
/// an error: with no asset there is no version to compare, nothing to install
/// and nothing to say, and the stable release the caller also fetched is the
/// right answer.
fn candidate_nightly(release: &serde_json::Value) -> Option<Available> {
    let tag = release
        .get("tag_name")
        .and_then(|t| t.as_str())
        .unwrap_or(NIGHTLY_TAG)
        .to_string();

    let mut best: Option<(Version, String)> = None;
    let mut sums_url = None;
    for (name, url) in assets_of(release) {
        if name == "SHA256SUMS" {
            sums_url = Some(url);
        } else if let Some(version) = version_from_archive(&name) {
            // A nightly release should carry exactly one archive per platform.
            // If a failed replacement left two, the newer one is the one the
            // notes describe.
            if best.as_ref().map(|(v, _)| version > *v).unwrap_or(true) {
                best = Some((version, url));
            }
        }
    }

    let (version, asset_url) = best?;
    Some(Available {
        version,
        tag,
        asset_url: Some(asset_url),
        sums_url,
    })
}

/// Name and download URL of every asset on a release.
fn assets_of(release: &serde_json::Value) -> Vec<(String, String)> {
    let Some(assets) = release.get("assets").and_then(|a| a.as_array()) else {
        return Vec::new();
    };
    assets
        .iter()
        .filter_map(|asset| {
            let name = asset.get("name")?.as_str()?.to_string();
            let url = asset.get("browser_download_url")?.as_str()?.to_string();
            Some((name, url))
        })
        .collect()
}

/// Name of the archive a release publishes for this platform.
///
/// Shared by the updater and by `RELEASING.md`, so the thing that builds the
/// asset and the thing that looks for it cannot drift.
pub fn archive_name(version: &Version) -> String {
    format!("vitrum-{version}-{TARGET}.tar.gz")
}

/// The version an archive filename carries, if it is this platform's archive.
///
/// The exact inverse of [`archive_name`], and deliberately strict about the
/// target suffix. A version may itself contain hyphens — every nightly's does,
/// because it is a semver prerelease — so the only unambiguous way to read one
/// out of a filename is to require the rest of the name to match exactly.
pub fn version_from_archive(name: &str) -> Option<Version> {
    let rest = name.strip_prefix("vitrum-")?;
    let rest = rest.strip_suffix(&format!("-{TARGET}.tar.gz"))?;
    Version::parse(rest).ok()
}

// ---------------------------------------------------------------------------
// What can go wrong on the way to a new build
// ---------------------------------------------------------------------------

/// Why an update did not happen, in the terms the operator has to act on.
///
/// One type for the whole updater boundary: the resolver, the download, the
/// checksum pass and the apply on the next start all report through it, and
/// every variant carries both what was wrong and what to do about it. Before
/// it, all four came out as prose in an `anyhow` chain and `vitrum update`
/// answered every one of them with exit code 1, so a cron entry could not tell
/// a train tunnel from a tampered archive.
///
/// Not every update failure is here, and that is deliberate. A directory that
/// cannot be written or an archive with no member for this platform is
/// reported where it is found; these are the four whose exit code a caller
/// genuinely branches on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateFault {
    /// A published endpoint did not answer.
    Unreachable {
        /// What was being asked for, named for a person rather than by URL.
        what: String,
        /// What the transport said.
        cause: String,
    },
    /// The release's `SHA256SUMS` has no line for this platform's archive.
    Unlisted {
        /// Archive name that was looked for.
        name: String,
    },
    /// Downloaded bytes are not what the release published.
    Mismatch {
        name: String,
        published: String,
        found: String,
    },
    /// Staged bytes are not what they hashed to when they were verified.
    ///
    /// The gap between staging and the next start may be days, and the disk
    /// they waited on may have failed in between.
    StaleStage {
        name: String,
        recorded: String,
        found: String,
    },
}

impl UpdateFault {
    /// The exit code `vitrum update` returns for this.
    pub fn exit(&self) -> Exit {
        match self {
            UpdateFault::Unreachable { .. } => Exit::Offline,
            // A release that published no sums, or bytes that do not match the
            // ones it did publish, are the same class to a caller: what is on
            // the server cannot be trusted, and the right move is to look
            // rather than to retry in a loop.
            UpdateFault::Unlisted { .. }
            | UpdateFault::Mismatch { .. }
            | UpdateFault::StaleStage { .. } => Exit::Corrupt,
        }
    }

    /// The code for `error`, if this type produced it anywhere in its chain.
    ///
    /// Walks the chain rather than looking only at the outermost error,
    /// because `install` adds context on the way out and the code has to
    /// survive that.
    pub fn exit_for(error: &anyhow::Error) -> Exit {
        error
            .chain()
            .find_map(|cause| cause.downcast_ref::<UpdateFault>())
            .map_or(Exit::Failed, UpdateFault::exit)
    }
}

impl std::fmt::Display for UpdateFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateFault::Unreachable { what, cause } => write!(
                f,
                "could not reach {what}: {cause}\n\
                 Nothing was downloaded and nothing on disk changed. Try again \
                 when the network is back, or install by hand from \
                 https://github.com/{REPO}/releases."
            ),
            UpdateFault::Unlisted { name } => write!(
                f,
                "SHA256SUMS does not list {name}\n\
                 The release published an archive it did not publish a digest \
                 for, so there is nothing to verify it against and it was not \
                 installed. Report it at https://github.com/{REPO}/issues."
            ),
            UpdateFault::Mismatch {
                name,
                published,
                found,
            } => write!(
                f,
                "checksum mismatch for {name}\n  \
                 published: {published}\n  \
                 downloaded: {found}\n\
                 Nothing was installed. Run `vitrum update` again; a second \
                 mismatch means the archive on the server does not match its \
                 own SHA256SUMS, and installing it would run code nobody \
                 published."
            ),
            UpdateFault::StaleStage {
                name,
                recorded,
                found,
            } => write!(
                f,
                "staged {name} no longer matches the digest recorded for it; \
                 nothing was applied\n  \
                 recorded: {recorded}\n  \
                 on disk: {found}\n\
                 The staged copy has been discarded and the running install is \
                 untouched. Run `vitrum update` to fetch it again."
            ),
        }
    }
}

impl std::error::Error for UpdateFault {}

/// Download, verify and stage a release for the next start.
///
/// `into` is the directory holding the binaries to replace, normally the one
/// containing the running executable. Nothing in `into` is replaced here: the
/// verified binaries land in [`staging_dir`] and [`apply_staged`] renames them
/// in when vitrum next starts. Progress lines go to `report` so the terminal
/// can print them and the window can show them without this module knowing
/// which it is talking to.
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

    report("staging");
    // Whatever was staged before is discarded whole rather than written over.
    // Two half-overlapping releases in one directory is a pair that has never
    // been tested together, and the record would name only one of them.
    discard_staged(into);
    let staged = unpack(&archive, into)?;

    let mut files = Vec::new();
    for (temp, target) in &staged {
        let name = target
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow!("staged {} has no file name", target.display()))?
            .to_string();
        let body =
            fs::read(temp).with_context(|| format!("reading back {}", temp.display()))?;
        files.push(StagedFile {
            name,
            sha256: hex(&Sha256::digest(&body)),
        });
    }

    let record = Staged {
        version: available.version.to_string(),
        tag: available.tag.clone(),
        // Derived from the tag rather than carried alongside it, so the record
        // cannot disagree with the release it came from.
        channel: if available.tag == NIGHTLY_TAG {
            Channel::Nightly
        } else {
            Channel::Stable
        },
        files,
    };
    write_record(into, &record)?;

    report(&format!(
        "staged {}; it is applied the next time vitrum starts",
        available.version
    ));
    Ok(())
}

/// Read a whole response body, refusing one that is implausibly large.
///
/// A transport failure becomes [`UpdateFault::Unreachable`] rather than a bare
/// `ureq` error, so a laptop off the network exits with a code that says
/// "retry later" instead of the same 1 a corrupt archive gets. An HTTP status
/// is left alone: the endpoint answered, and what it said is the diagnosis.
fn download(url: &str) -> Result<Vec<u8>> {
    let response = agent().get(url).call().map_err(|e| match e {
        ureq::Error::Status(code, _) => anyhow!("{url} answered {code}"),
        ureq::Error::Transport(t) => anyhow!(UpdateFault::Unreachable {
            what: url.to_string(),
            cause: t.to_string(),
        }),
    })?;
    let mut body = Vec::new();
    // A release archive for this program is single-digit megabytes. The cap
    // exists so a redirect to something enormous cannot exhaust memory on the
    // machine being updated.
    const MAX: u64 = 256 * 1024 * 1024;
    response
        .into_reader()
        .take(MAX)
        .read_to_end(&mut body)
        .map_err(|e| {
            anyhow!(UpdateFault::Unreachable {
                what: url.to_string(),
                cause: e.to_string(),
            })
        })?;
    Ok(body)
}

/// Check `archive` against the digest `sums` publishes for `name`.
///
/// Returns [`UpdateFault`] rather than an opaque error because this is the one
/// gate between remote bytes and the program the operator runs: its two
/// failures are the ones a caller must be able to tell apart from a flaky
/// download without reading prose.
pub fn verify(archive: &[u8], sums: &str, name: &str) -> std::result::Result<(), UpdateFault> {
    let expected = sums
        .lines()
        .find_map(|line| {
            let mut parts = line.split_whitespace();
            let digest = parts.next()?;
            let file = parts.next()?.trim_start_matches('*');
            (file == name).then_some(digest)
        })
        .ok_or_else(|| UpdateFault::Unlisted {
            name: name.to_string(),
        })?;

    let actual = hex(&Sha256::digest(archive));
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(UpdateFault::Mismatch {
            name: name.to_string(),
            published: expected.to_string(),
            found: actual,
        });
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

/// Name of the directory holding a verified update that has not been applied.
///
/// Inside the install directory and not a sibling of it, because the rename
/// that applies an update is only atomic within one filesystem, and the one
/// place guaranteed to be on the same filesystem as the binaries is the
/// directory holding them. The leading dot keeps it out of the way of a
/// listing.
pub const STAGING_DIR: &str = ".vitrum-staged";

/// Name of the record describing what is staged.
const STAGED_RECORD: &str = "staged.json";

/// Where a verified update waits for the next start.
pub fn staging_dir(into: &Path) -> PathBuf {
    into.join(STAGING_DIR)
}

/// One binary waiting to be renamed into place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StagedFile {
    /// File name, identical in the staging directory and at its destination.
    pub name: String,
    /// SHA-256 of the staged bytes, as lowercase hex.
    ///
    /// Recorded so applying can check that what it is about to rename over a
    /// working binary is still what was verified against the release. The
    /// archive's digest was checked at download time; this catches the gap
    /// between then and the restart, which may be days and may include a
    /// crashed disk.
    pub sha256: String,
}

/// What is staged, as written beside the staged binaries.
///
/// Written last and deleted first. Its presence is the only thing that means
/// "there is an update to apply", so a stage that died halfway leaves files
/// nobody will act on, and an apply that died halfway leaves a record that
/// names the work still to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Staged {
    /// Version staged, as it was published.
    pub version: String,
    /// Tag it came from.
    pub tag: String,
    /// Channel it came from.
    #[serde(default)]
    pub channel: Channel,
    /// Every binary waiting, in the order they are applied.
    pub files: Vec<StagedFile>,
}

impl Staged {
    /// The staged version, when it parses.
    pub fn version(&self) -> Option<Version> {
        Version::parse(self.version.trim()).ok()
    }
}

/// What is staged in `into`, if anything.
pub fn staged(into: &Path) -> Option<Staged> {
    let text = fs::read_to_string(staging_dir(into).join(STAGED_RECORD)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Throw away anything staged, leaving the installed binaries alone.
pub fn discard_staged(into: &Path) {
    let _ = fs::remove_dir_all(staging_dir(into));
}

/// Write the record, atomically, after every staged file is on disk.
fn write_record(into: &Path, record: &Staged) -> Result<()> {
    let dir = staging_dir(into);
    let path = dir.join(STAGED_RECORD);
    let tmp = dir.join(".staged.json.tmp");
    let text = serde_json::to_string_pretty(record).context("encoding the staging record")?;
    fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    // Rename rather than write in place: a truncated record read back on the
    // next start would either name a file that is not there or fail to parse,
    // and both discard an update the operator already downloaded.
    fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Apply whatever is staged in `into`, returning the version applied.
///
/// Called at the start of a run, before the window and before the daemon is
/// dialled, because that is the only moment at which nothing yet depends on
/// which build is on disk.
///
/// Crash recovery is the reason this is shaped the way it is. The record
/// survives until every rename has completed, so an interrupted apply leaves
/// the record plus exactly the files that were not renamed yet. This function
/// renames whatever is still there and is therefore safe to run again after a
/// crash at any point, including between the two renames — which is the state
/// that matters, since a new client paired with an old daemon refuses to talk.
///
/// The daemon is not restarted. It is a live process holding every session,
/// and replacing its file does not touch it.
pub fn apply_staged(into: &Path) -> Result<Option<Version>> {
    let dir = staging_dir(into);
    let Some(record) = staged(into) else {
        // Files with no record are the remains of a stage that died before it
        // finished. Nothing verified them as a set, so they are swept, not
        // applied.
        if dir.exists() {
            discard_staged(into);
        }
        return Ok(None);
    };

    // Everything is checked before anything is renamed. Failing halfway
    // through the renames is the one outcome this whole design exists to
    // avoid, so the digests are confirmed while a refusal still costs nothing.
    let mut pending = Vec::new();
    for file in &record.files {
        let path = dir.join(&file.name);
        let body = match fs::read(&path) {
            Ok(body) => body,
            // Absent means an earlier run already renamed it in and died
            // before finishing the rest. That is the crash this recovers from:
            // the work left is exactly the files still here.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let found = hex(&Sha256::digest(&body));
        if !found.eq_ignore_ascii_case(&file.sha256) {
            // Untouched so far, so the safe move is to keep the working
            // install and drop the staged copy.
            discard_staged(into);
            return Err(anyhow!(UpdateFault::StaleStage {
                name: file.name.clone(),
                recorded: file.sha256.clone(),
                found,
            }));
        }
        pending.push(file.name.clone());
    }

    for name in &pending {
        let temp = dir.join(name);
        let target = into.join(name);
        swap_in(&temp, &target).with_context(|| format!("applying {}", target.display()))?;
    }

    let version = record.version();
    // Last, and only now: while this directory exists with a record in it,
    // there is still work to redo after a crash.
    discard_staged(into);
    Ok(version)
}

/// Apply a staged update at startup and continue as the build that was staged.
///
/// Renaming the new client into place does not change the image this process
/// is already running: on Unix the running program keeps the inode it started
/// with. Without the re-exec the operator would have to restart twice to get
/// the build they staged once, and in between they would be running a new
/// daemon against an old client. So the process replaces itself with the
/// binary it just applied, once, guarded by an environment variable so a
/// pathological loop cannot form.
///
/// Nothing here restarts the daemon.
pub fn apply_on_start() {
    /// Set on the re-executed process so it cannot re-exec again.
    const GUARD: &str = "VITRUM_UPDATE_APPLIED";

    let Ok(dir) = install_dir() else {
        return;
    };

    // Read this BEFORE the staged image is moved into place. `apply_staged`
    // renames the new binary over the running one, which unlinks the inode
    // this process is executing, and from that moment `/proc/self/exe` reads
    // `<path> (deleted)`. Rust returns that literal string, so the exec below
    // failed with ENOENT on every successful update: the new build was on
    // disk and correct, and the process still printed an error and carried on
    // as the old one. Captured first the path names the file, and after the
    // rename the same path names the new image, which is the one to exec.
    let exe = std::env::current_exe().ok();
    // A previous update on Windows could not delete the image it replaced,
    // because that image was the process doing the replacing. It has exited by
    // now, so this is the first moment the file can go.
    sweep_displaced(&dir);

    let applied = match apply_staged(&dir) {
        Ok(Some(version)) => version,
        Ok(None) => return,
        Err(e) => {
            eprintln!("could not apply the staged update: {e:#}");
            return;
        }
    };

    if std::env::var_os(GUARD).is_some() {
        return;
    }
    let Some(exe) = exe else {
        return;
    };
    let mut command = std::process::Command::new(exe);
    command.args(std::env::args_os().skip(1)).env(GUARD, "1");

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Only returns on failure, in which case this process carries on as
        // the old build rather than refusing to start.
        let e = command.exec();
        eprintln!("updated to {applied}, but could not restart into it: {e}");
    }
    #[cfg(not(unix))]
    {
        match command.spawn() {
            Ok(_) => std::process::exit(0),
            Err(e) => eprintln!("updated to {applied}, but could not restart into it: {e}"),
        }
    }
}

/// Unpack every binary in the archive into the staging directory, returning
/// the pairs still to be renamed.
///
/// Nothing is renamed here, and nothing in the install directory is touched.
/// The caller records the set and the renames happen at the next start.
fn unpack(archive: &[u8], into: &Path) -> Result<Vec<(PathBuf, PathBuf)>> {
    let dir = staging_dir(into);
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
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
        let temp = dir.join(name);
        let mut out =
            fs::File::create(&temp).with_context(|| format!("staging {}", temp.display()))?;
        std::io::copy(&mut entry, &mut out)
            .with_context(|| format!("writing {}", temp.display()))?;
        drop(out);
        set_executable(&temp)?;
        staged.push((temp, target));
    }
    if staged.is_empty() {
        discard_staged(into);
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

/// What to say once the new binaries are staged.
///
/// Nothing has been replaced when this is printed, and saying otherwise would
/// be the same lie in the other direction: the live client is still the old
/// one and stays that way until it is restarted. The update is on disk,
/// verified, and applied by the next start.
///
/// The daemon half is unchanged and is the part that costs something. This
/// product's defining behaviour is that the daemon owns the PTYs and outlives
/// every window, so restarting the window swaps the client and leaves the OLD
/// daemon running: it is a live process, and replacing its file on disk does
/// not touch it. It keeps serving until it is restarted, and restarting it
/// ends every session it is holding.
///
/// So the operator has to be told two separate things, because they have very
/// different costs and only one of them is free.
pub const AFTER_INSTALL: &str = "\
the update is staged; restart vitrum to run the new client.\n\
Nothing on disk changes under the running client, and no session is disturbed \
until you restart.\n\
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

/// Every code `vitrum update` can exit with.
///
/// Wider than every other subcommand's, because this is the one that talks to
/// the network and to a directory it may not own, and because it is the one
/// people put in a cron entry: a retry loop must be able to tell "the link was
/// down" from "the archive did not match its digest" without parsing prose.
pub const EXIT_CODES: &[Exit] = &[
    Exit::Ok,
    Exit::Failed,
    Exit::Usage,
    Exit::Unavailable,
    Exit::Offline,
    Exit::Corrupt,
];

/// `vitrum update` — check for a newer release and stage it.
///
/// Returns a code from the one table in [`vitrum_proto::exit`]. "Already up to
/// date" is [`Exit::Ok`], because nothing is wrong.
pub fn run_update(args: &[String]) -> i32 {
    let mut check_only = false;
    let mut channel = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => check_only = true,
            "--channel" => match args.next().map(String::as_str) {
                Some("stable") => channel = Some(Channel::Stable),
                Some("nightly") => channel = Some(Channel::Nightly),
                Some(other) => {
                    eprintln!("unknown channel {other}\n\n{}", update_usage());
                    return Exit::Usage.code();
                }
                None => {
                    eprintln!("--channel needs stable or nightly\n\n{}", update_usage());
                    return Exit::Usage.code();
                }
            },
            "-h" | "--help" => {
                println!("{}", update_usage());
                return Exit::Ok.code();
            }
            other => {
                eprintln!("unknown argument {other}\n\n{}", update_usage());
                return Exit::Usage.code();
            }
        }
    }

    let channel = channel.unwrap_or_else(configured_channel);
    let status = match check_on(channel) {
        Ok(s) => s,
        Err(e) => {
            // The fault carries the corrective action and the code; the prefix
            // only says which of the updater's steps was being taken. An
            // unreachable endpoint exits 4 so a cron entry can retry it
            // unchanged, which is exactly the wrong response to a 5.
            eprintln!("could not check for updates: {e:#}");
            return UpdateFault::exit_for(&e).code();
        }
    };

    match status {
        Status::UpToDate { version } => {
            println!(
                "vitrum {version} is the newest {} release",
                channel.as_str()
            );
            Exit::Ok.code()
        }
        Status::NoReleases => {
            println!(
                "no releases published for {} yet; you are on {}",
                REPO,
                current_version()
            );
            Exit::Ok.code()
        }
        Status::NoAssetForPlatform { version, target } => {
            eprintln!(
                "vitrum {version} is available but published no build for {target}.\n\
                 Build it from source: https://github.com/{}/releases/tag/v{version}",
                REPO
            );
            // Nothing is wrong with the command or with this machine's link;
            // the build simply does not exist yet, and a later release may
            // carry it. That is Unavailable, not a flat failure.
            Exit::Unavailable.code()
        }
        Status::Ready(available) => {
            println!(
                "vitrum {} is available (you have {})",
                available.version,
                current_version()
            );
            if check_only {
                return Exit::Ok.code();
            }
            let dir = match install_dir() {
                Ok(d) => d,
                Err(e) => {
                    eprintln!(
                        "could not find where vitrum is installed: {e:#}\n\
                         Run the copy you want updated by its full path, or \
                         reinstall with the installer for your platform."
                    );
                    return Exit::Unavailable.code();
                }
            };
            // Checked before the download rather than after, so a
            // package-managed install fails in a second instead of after
            // fetching and verifying an archive it can never write.
            if !writable(&dir) {
                eprintln!(
                    "cannot write to {}.\n\
                     This copy was installed by something else; update it the \
                     same way you installed it, or reinstall into a directory \
                     you own.",
                    dir.display()
                );
                return Exit::Unavailable.code();
            }
            match install(&available, &dir, &mut |line| println!("{line}")) {
                Ok(()) => {
                    println!("{}", AFTER_INSTALL);
                    Exit::Ok.code()
                }
                Err(e) => {
                    eprintln!("update failed: {e:#}");
                    UpdateFault::exit_for(&e).code()
                }
            }
        }
    }
}

pub(crate) fn update_usage() -> String {
    format!(
        "vitrum update - stage the newest published release\n\n\
         usage: vitrum update [--check] [--channel stable|nightly]\n\n\
         Reads a published release of {}, never the branch, because the tip of\n\
         a branch carries work that has not been released.\n\n\
         The archive's SHA-256 must match the SHA256SUMS published beside it.\n\
         A release with no sums is refused rather than trusted.\n\n\
         The update is staged, not swapped in: the running client and every\n\
         session it holds are left alone, and the next start applies it.\n\n\
         options:\n  \
         --check              report what is available and stage nothing\n  \
         --channel <name>     stable or nightly, overriding the setting\n  \
         -h, --help           show this message\n\n\
         exit status:\n\
         {}",
        REPO,
        exit::status_lines(EXIT_CODES)
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

/// What the titlebar is allowed to say about an update, and when it says
/// nothing at all.
#[cfg(test)]
mod chrome_offer;

/// The mark belongs on the launcher, not in the window.
///
/// Enforced here rather than written down, because the document it used to
/// live in was `assets/logo/README.md` and that directory no longer exists.
#[cfg(test)]
mod where_the_mark_may_appear;

/// Staging, applying, and surviving a crash between the two renames.
#[cfg(test)]
mod an_update_applies_on_restart;

/// Which release each channel resolves, and what an unreachable network does.
#[cfg(test)]
mod which_release_a_channel_resolves;

/// The setting hides the affordance and changes nothing else.
#[cfg(test)]
mod hiding_the_affordance_is_not_disabling_updates;
