//! Which release each channel resolves, and what an unreachable network does.
//!
//! The defect class: a channel that resolves the wrong release. There are four
//! ways to get it wrong and every one of them ships a build nobody chose. A
//! stable install that picks up a prerelease is the worst of them, because the
//! operator asked for the opposite. A nightly install that never sees a newer
//! stable is stuck. A nightly install moved back to an older stable is
//! downgraded. And a resolver that reads a nightly's version from anywhere but
//! its asset name reads it from a tag that is the word `nightly`.
//!
//! The offline case is here too, because a check nobody asked for must not be
//! able to produce an error the operator has to dismiss.

use super::*;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;


fn asset(name: &str, url: &str) -> serde_json::Value {
    serde_json::json!({ "name": name, "browser_download_url": url })
}

/// A stable release object as GitHub returns it.
fn stable_release(version: &str) -> serde_json::Value {
    serde_json::json!({
        "tag_name": format!("v{version}"),
        "prerelease": false,
        "assets": [
            asset(&archive_name(&Version::parse(version).unwrap()), "https://x/a"),
            asset("SHA256SUMS", "https://x/s"),
        ],
    })
}

/// The nightly release object: one moving tag, marked prerelease, carrying an
/// archive whose name is the only place its version is written down.
fn nightly_release(version: &str) -> serde_json::Value {
    serde_json::json!({
        "tag_name": NIGHTLY_TAG,
        "prerelease": true,
        "assets": [
            asset(&archive_name(&Version::parse(version).unwrap()), "https://x/n"),
            asset("SHA256SUMS", "https://x/ns"),
        ],
    })
}

fn ready(status: Status) -> Available {
    match status {
        Status::Ready(a) => a,
        other => panic!("expected a ready release, got {other:?}"),
    }
}

/// The stable channel never resolves the nightly release, even handed one.
///
/// WHY: the nightly tag is published as a prerelease so `/releases/latest`
/// skips it, but "the server will not offer it" is not a property of this
/// program. A stable build handed the nightly object anyway must still ignore
/// it, or a single API change puts every stable install on nightlies.
#[test]
fn stable_ignores_the_nightly_release() {
    let current = Version::parse("0.1.0").unwrap();
    let status = resolve(
        Channel::Stable,
        Some(&stable_release("0.1.1")),
        Some(&nightly_release("0.1.2-nightly.20260808.abc1234")),
        &current,
    )
    .expect("resolved");
    assert_eq!(ready(status).version, Version::parse("0.1.1").unwrap());
}

/// A prerelease tag is not what the stable channel installs.
///
/// WHY: the second half of the same class. If the latest endpoint ever
/// answered with a prerelease, the version comparison alone would happily
/// install it.
#[test]
fn stable_never_offers_a_prerelease_version() {
    let current = Version::parse("0.1.0").unwrap();
    let status = resolve(Channel::Stable, None, None, &current).expect("resolved");
    assert_eq!(status, Status::NoReleases);

    // And the endpoint the stable channel reads is the one GitHub defines as
    // excluding prereleases; the nightly tag is never requested at all. That
    // is asserted over a socket below.
}

/// The nightly channel resolves the moving tag.
///
/// WHY: the nightly tag is the word `nightly`, which is not a version. A
/// resolver that reads the version from the tag either errors or invents one,
/// and the only place the version is actually written is the asset filename.
#[test]
fn nightly_resolves_the_moving_tag() {
    let current = Version::parse("0.1.0").unwrap();
    let status = resolve(
        Channel::Nightly,
        Some(&stable_release("0.1.0")),
        Some(&nightly_release("0.1.1-nightly.20260809.f4f494e")),
        &current,
    )
    .expect("resolved");
    let available = ready(status);
    assert_eq!(available.tag, NIGHTLY_TAG);
    assert_eq!(
        available.version,
        Version::parse("0.1.1-nightly.20260809.f4f494e").unwrap()
    );
    assert_eq!(available.sums_url.as_deref(), Some("https://x/ns"));
}

/// The version comes out of the asset name the workflow publishes.
///
/// WHY: this filename is the only contract between the release workflow and
/// the resolver. It is asserted against the exact string the workflow
/// produces, hyphens in the prerelease and all, because a naive split on `-`
/// parses `0.1.1` out of it and silently offers a downgrade.
#[test]
fn the_nightly_version_is_read_out_of_the_asset_name() {
    let name = format!("vitrum-0.1.1-nightly.20260809.f4f494e-{TARGET}.tar.gz");
    assert_eq!(
        version_from_archive(&name),
        Some(Version::parse("0.1.1-nightly.20260809.f4f494e").unwrap())
    );
    // Round trip with the name the installer looks for.
    let v = Version::parse("0.1.1-nightly.20260809.f4f494e").unwrap();
    assert_eq!(version_from_archive(&archive_name(&v)), Some(v));
    // Another platform's archive is not this platform's version.
    assert_eq!(
        version_from_archive("vitrum-0.1.1-some-other-triple.tar.gz"),
        None
    );
}

/// Semver order, not filename order.
///
/// WHY: `sort -V` puts `0.1.1` before `0.1.1-nightly.1`, which is the opposite
/// of semver, and a resolver that agreed with it would treat every nightly as
/// newer than the stable it precedes.
#[test]
fn a_nightly_sorts_below_the_stable_it_precedes() {
    let nightly = Version::parse("0.1.1-nightly.20260809.f4f494e").unwrap();
    let stable = Version::parse("0.1.1").unwrap();
    assert!(nightly < stable);
}

/// A nightly build takes a stable release that is newer.
///
/// WHY: nightly is ahead of stable, not a separate product. Once stable passes
/// the running nightly, staying on the nightly stream means staying behind.
#[test]
fn a_nightly_takes_a_newer_stable() {
    let current = Version::parse("0.1.1-nightly.20260801.aaaaaaa").unwrap();
    let status = resolve(
        Channel::Nightly,
        Some(&stable_release("0.1.1")),
        Some(&nightly_release("0.1.1-nightly.20260801.aaaaaaa")),
        &current,
    )
    .expect("resolved");
    let available = ready(status);
    assert_eq!(available.version, Version::parse("0.1.1").unwrap());
    assert_eq!(available.tag, "v0.1.1", "the nightly was taken over a newer stable");
}

/// A nightly is never moved back to an older stable.
///
/// WHY: the mirror image of the rule above, and the one that produces a
/// downgrade rather than a stall. A build running `0.1.2-nightly.x` with
/// `0.1.1` as the newest stable is ahead of stable and must be left alone.
#[test]
fn a_nightly_is_never_downgraded_to_an_older_stable() {
    let current = Version::parse("0.1.2-nightly.20260809.f4f494e").unwrap();
    let status = resolve(
        Channel::Nightly,
        Some(&stable_release("0.1.1")),
        Some(&nightly_release("0.1.2-nightly.20260809.f4f494e")),
        &current,
    )
    .expect("resolved");
    assert_eq!(
        status,
        Status::UpToDate {
            version: current.clone()
        },
        "an older stable was offered to a newer nightly"
    );

    // Even with no nightly release published at all, the older stable is not
    // an update.
    let status = resolve(Channel::Nightly, Some(&stable_release("0.1.1")), None, &current)
        .expect("resolved");
    assert_eq!(status, Status::UpToDate { version: current });
}

/// A nightly release with no build for this platform falls back to stable.
///
/// WHY: with no asset there is no version to compare and nothing to install.
/// Reporting "no asset for your platform" would hide a perfectly good stable
/// release behind a nightly runner that failed.
#[test]
fn a_nightly_without_this_platforms_asset_falls_back_to_stable() {
    let current = Version::parse("0.1.0").unwrap();
    let broken = serde_json::json!({
        "tag_name": NIGHTLY_TAG,
        "prerelease": true,
        "assets": [asset("vitrum-0.1.1-nightly.1-some-other-triple.tar.gz", "https://x/o")],
    });
    let status = resolve(
        Channel::Nightly,
        Some(&stable_release("0.1.1")),
        Some(&broken),
        &current,
    )
    .expect("resolved");
    assert_eq!(ready(status).version, Version::parse("0.1.1").unwrap());
}

/// Verification does not vary by channel.
///
/// WHY: a nightly is remote code on exactly the same terms as a release. The
/// digest check is the same call on both paths, and a mismatch has to install
/// nothing on both.
#[test]
fn a_digest_mismatch_installs_nothing_on_either_channel() {
    for (channel, tag) in [(Channel::Stable, "v9.9.9"), (Channel::Nightly, NIGHTLY_TAG)] {
        let dir = std::env::temp_dir().join(format!(
            "vitrum-mismatch-{}-{}-{:?}",
            channel.as_str(),
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("vitrum"), b"old client").unwrap();
        fs::write(dir.join("vitrum-server"), b"old daemon").unwrap();

        let version = Version::parse("9.9.9").unwrap();
        let archive = archive_of(b"malicious", b"malicious");
        let sums = format!(
            "{}  {}\n",
            hex(&Sha256::digest(b"the real release")),
            archive_name(&version)
        );
        let (base, server) = serve(vec![
            ("/archive".to_string(), archive),
            ("/sums".to_string(), sums.into_bytes()),
        ]);
        let available = Available {
            version,
            tag: tag.to_string(),
            asset_url: Some(format!("{base}/archive")),
            sums_url: Some(format!("{base}/sums")),
        };

        let e = install(&available, &dir, &mut |_| {}).unwrap_err();
        server.join().ok();
        assert!(
            e.to_string().contains("checksum mismatch"),
            "{}: {e}",
            channel.as_str()
        );
        assert_eq!(fs::read(dir.join("vitrum")).unwrap(), b"old client");
        assert_eq!(fs::read(dir.join("vitrum-server")).unwrap(), b"old daemon");
        assert!(
            staged(&dir).is_none(),
            "{}: a release that failed verification was staged anyway",
            channel.as_str()
        );
        fs::remove_dir_all(&dir).ok();
    }
}

/// A tar.gz of the two binaries.
fn archive_of(client: &[u8], daemon: &[u8]) -> Vec<u8> {
    let mut tar = tar::Builder::new(Vec::new());
    for (name, body) in [("vitrum", client), ("vitrum-server", daemon)] {
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, name, body).unwrap();
    }
    let raw = tar.into_inner().unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&raw).unwrap();
    gz.finish().unwrap()
}

/// Serve a fixed set of paths, answering `calls` requests and handing back
/// every path that was asked for.
///
/// The record comes out of the thread's return value rather than a shared
/// mutex: there is exactly one writer, one reader, and a join between them.
///
/// `{BASE}` anywhere in a body is replaced with the served root, so a release
/// object can name asset URLs on a port that is not known until it is bound.
fn serve_api(
    routes: Vec<(String, Vec<u8>)>,
    calls: usize,
) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bound");
    let base = format!("http://{}", listener.local_addr().unwrap());
    let routes: Vec<(String, Vec<u8>)> = routes
        .into_iter()
        .map(|(path, body)| {
            let body = match String::from_utf8(body) {
                Ok(text) => text.replace("{BASE}", &base).into_bytes(),
                Err(e) => e.into_bytes(),
            };
            (path, body)
        })
        .collect();
    let handle = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for _ in 0..calls {
            let Ok((mut sock, _)) = listener.accept() else {
                return seen;
            };
            let mut reader = BufReader::new(sock.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
            seen.push(path.clone());
            loop {
                let mut h = String::new();
                match reader.read_line(&mut h) {
                    Ok(0) | Err(_) => break,
                    Ok(_) if h.trim().is_empty() => break,
                    Ok(_) => {}
                }
            }
            let body = routes
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, b)| b.clone());
            let response = match body {
                Some(b) => {
                    let mut r = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        b.len()
                    )
                    .into_bytes();
                    r.extend_from_slice(&b);
                    r
                }
                None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_vec(),
            };
            let _ = sock.write_all(&response);
            let _ = sock.flush();
        }
        seen
    });
    (base, handle)
}

/// Reuse the one-shot server for the digest test above.
fn serve(routes: Vec<(String, Vec<u8>)>) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let calls = routes.len();
    serve_api(routes, calls)
}

/// The stable channel never asks for the nightly tag.
///
/// WHY: the cheapest and most durable guarantee that stable cannot resolve a
/// prerelease is that it never requests one. Asserted over a real socket
/// because it is a statement about the requests this program makes.
#[test]
fn the_stable_channel_only_ever_asks_for_the_latest_release() {
    let body = serde_json::to_vec(&stable_release("0.0.1")).unwrap();
    let (base, server) = serve_api(vec![(format!("/repos/{REPO}/releases/latest"), body)], 1);
    check_at(&base, Channel::Stable).expect("resolved");
    let seen = server.join().expect("server thread");
    assert_eq!(seen, vec![format!("/repos/{REPO}/releases/latest")]);
}

/// The nightly channel asks for both, so a newer stable can win.
#[test]
fn the_nightly_channel_asks_for_the_moving_tag_too() {
    let latest = serde_json::to_vec(&stable_release("0.0.1")).unwrap();
    let nightly = serde_json::to_vec(&nightly_release("0.0.2-nightly.1")).unwrap();
    let (base, server) = serve_api(
        vec![
            (format!("/repos/{REPO}/releases/latest"), latest),
            (format!("/repos/{REPO}/releases/tags/{NIGHTLY_TAG}"), nightly),
        ],
        2,
    );
    check_at(&base, Channel::Nightly).expect("resolved");
    let seen = server.join().expect("server thread");
    assert!(
        seen.contains(&format!("/repos/{REPO}/releases/tags/{NIGHTLY_TAG}")),
        "the nightly tag was never requested: {seen:?}"
    );
}

/// A nightly published exactly as the release workflow publishes one is
/// resolved, verified, staged and applied.
///
/// WHY: the workflow and this resolver agree on three strings and nothing
/// else — the moving tag, the archive filename, and the `sha256sum` line that
/// covers it. Every other test here builds those strings the way this module
/// would like them; this one builds them the way `release.yml` actually
/// writes them: the tag literally `nightly`, the version a prerelease of the
/// NEXT patch from `tools/release/versions.sh`, and `SHA256SUMS` as
/// `sha256sum *.tar.gz` writes it, two spaces and a plain filename.
#[test]
fn a_nightly_shaped_like_the_workflow_publishes_it_installs() {
    // `versions.sh nightly` derives this from the workspace version, so it is
    // derived here the same way rather than pinned.
    let current = current_version();
    let version = Version::parse(&format!(
        "{}.{}.{}-nightly.20260809.f4f494e",
        current.major,
        current.minor,
        current.patch + 1
    ))
    .expect("the workflow's version is semver");
    let name = archive_name(&version);

    let archive = archive_of(b"nightly client", b"nightly daemon");
    // `sha256sum` output: digest, two spaces, plain name, no `./`.
    let sums = format!("{}  {name}\n", hex(&Sha256::digest(&archive)));

    let dir = std::env::temp_dir().join(format!(
        "vitrum-nightly-e2e-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("vitrum"), b"old client").unwrap();
    fs::write(dir.join("vitrum-server"), b"old daemon").unwrap();

    // `{BASE}` is replaced with the served root once the port is known, so
    // the release JSON can point at the same socket that serves it.
    let release = serde_json::json!({
        "tag_name": NIGHTLY_TAG,
        "name": "nightly f4f494e",
        "prerelease": true,
        "assets": [
            asset(&name, "{BASE}/archive"),
            asset("SHA256SUMS", "{BASE}/sums"),
        ],
    });

    // The stable side is the last release, which a nightly must not be moved
    // back to.
    let latest = serde_json::to_vec(&stable_release(&current.to_string())).unwrap();
    let (base, server) = serve_api(
        vec![
            (format!("/repos/{REPO}/releases/latest"), latest),
            (
                format!("/repos/{REPO}/releases/tags/{NIGHTLY_TAG}"),
                serde_json::to_vec(&release).unwrap(),
            ),
            ("/archive".to_string(), archive),
            ("/sums".to_string(), sums.into_bytes()),
        ],
        4,
    );

    let status = check_at(&base, Channel::Nightly).expect("resolved");
    let available = ready(status);
    assert_eq!(available.version, version);
    assert_eq!(available.tag, NIGHTLY_TAG);

    install(&available, &dir, &mut |_| {}).expect("staged");
    server.join().ok();
    assert_eq!(
        staged(&dir).and_then(|s| s.version()),
        Some(version.clone())
    );
    assert_eq!(
        staged(&dir).map(|s| s.channel),
        Some(Channel::Nightly),
        "a nightly was recorded as a stable staging"
    );

    assert_eq!(apply_staged(&dir).expect("applied"), Some(version));
    assert_eq!(fs::read(dir.join("vitrum")).unwrap(), b"nightly client");
    assert_eq!(
        fs::read(dir.join("vitrum-server")).unwrap(),
        b"nightly daemon"
    );
    fs::remove_dir_all(&dir).ok();
}

/// An unreachable network is silence, not an error.
///
/// WHY: the quiet check is something this program decided to do. On a train,
/// on a locked-down network, or behind a proxy that drops GitHub, it must
/// produce nothing at all: no badge, no flash, no log line the operator has to
/// read. It must also end rather than hang, which is why this asserts a bound
/// as well as a value.
#[test]
fn an_unreachable_network_is_a_silent_no_op() {
    // Port 1 on loopback: refused immediately, so this asserts the offline
    // path and not the connect timeout.
    let started = std::time::Instant::now();
    for channel in [Channel::Stable, Channel::Nightly] {
        assert_eq!(
            background_check_at("http://127.0.0.1:1", channel),
            None,
            "{} produced an answer with no network",
            channel.as_str()
        );
        // The loud form of the same call still reports, because a command the
        // operator typed must exit non-zero when it could not answer.
        assert!(
            check_at("http://127.0.0.1:1", channel).is_err(),
            "{}: the terminal command swallowed a failure",
            channel.as_str()
        );
    }
    assert!(
        started.elapsed() < NET_TIMEOUT,
        "the offline path waited on a timeout instead of failing fast"
    );
}
