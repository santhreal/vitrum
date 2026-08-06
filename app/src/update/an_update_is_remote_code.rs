use super::*;
use std::io::Write;

fn release_json(tag: &str, assets: &[(&str, &str)]) -> serde_json::Value {
    serde_json::json!({
        "tag_name": tag,
        "assets": assets.iter().map(|(n, u)| serde_json::json!({
            "name": n, "browser_download_url": u
        })).collect::<Vec<_>>(),
    })
}

fn targz(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut tar = tar::Builder::new(Vec::new());
    for (name, body) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, name, *body).unwrap();
    }
    let raw = tar.into_inner().unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&raw).unwrap();
    gz.finish().unwrap()
}

/// A release older than the running binary is not an update.
///
/// The comparison is semver, not string equality, because `0.10.0` sorts
/// before `0.9.0` as text and an updater that downgrades on every check is
/// worse than no updater.
#[test]
fn an_older_or_equal_release_is_up_to_date() {
    for tag in ["v0.0.1", "v0.1.0", "0.1.0"] {
        let s = parse_release(&release_json(tag, &[])).expect("parsed");
        assert!(
            matches!(s, Status::UpToDate { .. }),
            "{tag} was treated as an update: {s:?}"
        );
    }
}

/// Ten sorts after nine.
#[test]
fn version_order_is_semver_not_alphabetical() {
    let s = parse_release(&release_json("v0.10.0", &[])).expect("parsed");
    assert!(
        !matches!(s, Status::UpToDate { .. }),
        "0.10.0 was treated as older than 0.1.0"
    );
}

/// A newer release with no build for this platform must say so.
///
/// Silently reporting "up to date" would leave the operator believing they
/// are current when they are behind, which is the one answer an updater
/// must never give wrongly.
#[test]
fn a_release_without_this_platforms_asset_is_not_up_to_date() {
    let s = parse_release(&release_json(
        "v9.9.9",
        &[("vitrum-9.9.9-some-other-triple.tar.gz", "https://x/a")],
    ))
    .expect("parsed");
    match s {
        Status::NoAssetForPlatform { version, target } => {
            assert_eq!(version.to_string(), "9.9.9");
            assert_eq!(target, TARGET);
        }
        other => panic!("expected NoAssetForPlatform, got {other:?}"),
    }
}

/// A newer release with the right asset is installable.
#[test]
fn a_release_with_this_platforms_asset_is_ready() {
    let name = archive_name(&Version::parse("9.9.9").unwrap());
    let s = parse_release(&release_json(
        "v9.9.9",
        &[
            (&name, "https://x/archive"),
            ("SHA256SUMS", "https://x/sums"),
        ],
    ))
    .expect("parsed");
    match s {
        Status::Ready(a) => {
            assert_eq!(a.asset_url.as_deref(), Some("https://x/archive"));
            assert_eq!(a.sums_url.as_deref(), Some("https://x/sums"));
            assert!(a.version > current_version(), "Ready must mean newer");
        }
        other => panic!("expected Ready, got {other:?}"),
    }
}

/// A tag that is not a version is an error, not a silent skip.
#[test]
fn a_nonsense_tag_is_reported() {
    let e = parse_release(&release_json("nightly", &[])).unwrap_err();
    assert!(
        e.to_string().contains("not a semver version"),
        "unhelpful: {e}"
    );
}

/// A tampered archive must never be unpacked.
///
/// This is the property the whole updater rests on: the bytes arriving
/// over the network become the program the operator runs on their machine.
/// The message carries both digests, because "checksum mismatch" alone
/// tells nobody whether they hit a corrupt mirror or an attack.
#[test]
fn a_tampered_archive_is_refused_with_both_digests() {
    let good = b"the real release";
    let sums = format!("{}  release.tar.gz\n", hex(&Sha256::digest(good)));
    verify(good, &sums, "release.tar.gz").expect("the real archive verifies");

    let tampered = b"the real releasf";
    let e = verify(tampered, &sums, "release.tar.gz").unwrap_err();
    let m = e.to_string();
    assert!(m.contains("checksum mismatch"), "{m}");
    assert!(
        m.contains(&hex(&Sha256::digest(good))),
        "no published digest: {m}"
    );
    assert!(
        m.contains(&hex(&Sha256::digest(tampered))),
        "no actual digest: {m}"
    );
}

/// A sums file that does not mention the archive is refused.
///
/// Not "no line, no check". An absent entry is exactly what an attacker
/// serving their own archive alongside a real sums file would produce.
#[test]
fn an_unlisted_archive_is_refused() {
    let e = verify(b"x", "abc  something-else.tar.gz\n", "release.tar.gz").unwrap_err();
    assert!(e.to_string().contains("does not list"), "{e}");
}

/// The `*` binary marker in a sums file is understood.
#[test]
fn the_binary_marker_in_a_sums_line_is_tolerated() {
    let body = b"payload";
    let sums = format!("{} *release.tar.gz\n", hex(&Sha256::digest(body)));
    verify(body, &sums, "release.tar.gz").expect("binary-marked line should verify");
}

/// Build a tar entry byte by byte, bypassing the builder's own checks.
///
/// `tar::Builder` refuses to write a path containing `..`, which is the
/// right default and also means it cannot express the archive this code
/// has to defend against. An attacker writes the header bytes directly, so
/// the test does too.
fn raw_tar_entry(name: &str, body: &[u8]) -> Vec<u8> {
    let mut header = [0u8; 512];
    header[..name.len()].copy_from_slice(name.as_bytes());
    header[100..107].copy_from_slice(b"0000755");
    header[108..115].copy_from_slice(b"0000000");
    header[116..123].copy_from_slice(b"0000000");
    let size = format!("{:011o}\0", body.len());
    header[124..136].copy_from_slice(size.as_bytes());
    header[136..148].copy_from_slice(b"00000000000\0");
    header[148..156].copy_from_slice(b"        ");
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let sum: u32 = header.iter().map(|b| *b as u32).sum();
    let checksum = format!("{sum:06o}\0 ");
    header[148..156].copy_from_slice(checksum.as_bytes());

    let mut out = header.to_vec();
    out.extend_from_slice(body);
    out.resize(out.len().div_ceil(512) * 512, 0);
    out
}

fn raw_targz(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut raw = Vec::new();
    for (name, body) in entries {
        raw.extend_from_slice(&raw_tar_entry(name, body));
    }
    raw.extend_from_slice(&[0u8; 1024]);
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&raw).unwrap();
    gz.finish().unwrap()
}

/// An archive may not write outside the directory it unpacks into.
///
/// A release archive is untrusted input. Honouring its stored paths is how
/// `../../.bashrc` ends up overwritten, so only the two known binary names
/// are taken and only by their final path component.
#[test]
fn an_archive_cannot_escape_the_install_directory() {
    let dir = std::env::temp_dir().join(format!("vitrum-esc-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    // The entry that matters is named so its FINAL component is a binary
    // this code accepts. A filter on the name alone waves it through, and
    // only taking `file_name()` rather than the stored path keeps it
    // inside `dir`.
    let evil = raw_targz(&[
        ("../../../../../../tmp/vitrum", b"pwned".as_slice()),
        ("/tmp/vitrum-server", b"pwned".as_slice()),
        ("../../../../../../tmp/vitrum-pwned", b"no".as_slice()),
    ]);
    let before_vitrum = Path::new("/tmp/vitrum").exists();
    let staged = unpack(&evil, &dir).expect("unpacked");

    // Two entries carry an accepted binary name; the third is ignored
    // outright. All of them must land inside `dir` and nowhere else.
    assert_eq!(staged.len(), 2, "wrong entries taken: {staged:?}");
    for (temp, target) in &staged {
        assert_eq!(
            target.parent(),
            Some(dir.as_path()),
            "an entry targeted {} outside {}",
            target.display(),
            dir.display()
        );
        assert_eq!(temp.parent(), Some(dir.as_path()));
    }
    // Staged, not yet renamed: the bytes are inside `dir` under a
    // temporary name, which is the whole point.
    assert_eq!(fs::read(dir.join(".vitrum.incoming")).unwrap(), b"pwned");
    assert!(!Path::new("/tmp/vitrum-pwned").exists(), "archive escaped");
    assert!(
        !Path::new("/tmp/vitrum-server").exists(),
        "an absolute path in the archive was honoured"
    );
    assert_eq!(
        Path::new("/tmp/vitrum").exists(),
        before_vitrum,
        "the archive wrote to /tmp/vitrum"
    );
    fs::remove_dir_all(&dir).ok();
}

/// An archive with no vitrum binaries is an error, not a silent success.
#[test]
fn an_empty_archive_is_refused() {
    let dir = std::env::temp_dir().join(format!("vitrum-empty-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let e = unpack(&targz(&[("README", b"hi".as_slice())]), &dir).unwrap_err();
    assert!(e.to_string().contains("no vitrum binaries"), "{e}");
    fs::remove_dir_all(&dir).ok();
}

/// Nothing is replaced until every binary has been staged.
///
/// The pair speak a versioned protocol, so a run that swapped the client
/// and then failed on the daemon would leave two halves that refuse each
/// other. Staging happens for all of them first; the originals are still
/// untouched when `unpack` returns.
#[test]
fn the_originals_survive_until_every_binary_is_staged() {
    let dir = std::env::temp_dir().join(format!("vitrum-stage-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("vitrum"), b"old client").unwrap();
    fs::write(dir.join("vitrum-server"), b"old daemon").unwrap();

    let archive = targz(&[
        ("vitrum", b"new client".as_slice()),
        ("vitrum-server", b"new daemon".as_slice()),
    ]);
    let staged = unpack(&archive, &dir).expect("unpacked");
    assert_eq!(staged.len(), 2);
    assert_eq!(fs::read(dir.join("vitrum")).unwrap(), b"old client");
    assert_eq!(fs::read(dir.join("vitrum-server")).unwrap(), b"old daemon");

    for (temp, target) in &staged {
        fs::rename(temp, target).unwrap();
    }
    assert_eq!(fs::read(dir.join("vitrum")).unwrap(), b"new client");
    assert_eq!(fs::read(dir.join("vitrum-server")).unwrap(), b"new daemon");
    fs::remove_dir_all(&dir).ok();
}

/// A staged binary is executable before it is renamed into place.
///
/// A tar entry carries a mode and this code does not trust it. Without the
/// explicit chmod the update completes and the next launch fails with
/// permission denied, having already replaced the working copy.
#[cfg(unix)]
#[test]
fn a_staged_binary_is_executable() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("vitrum-mode-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let mut header = tar::Header::new_gnu();
    header.set_size(4);
    header.set_mode(0o644);
    header.set_cksum();
    let mut tar = tar::Builder::new(Vec::new());
    tar.append_data(&mut header, "vitrum", b"body".as_slice())
        .unwrap();
    let raw = tar.into_inner().unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&raw).unwrap();
    let staged = unpack(&gz.finish().unwrap(), &dir).expect("unpacked");
    let mode = fs::metadata(&staged[0].0).unwrap().permissions().mode();
    assert_eq!(
        mode & 0o111,
        0o111,
        "staged binary is not executable: {mode:o}"
    );
    fs::remove_dir_all(&dir).ok();
}

/// The archive name the updater looks for is the one a release publishes.
#[test]
fn the_archive_name_carries_version_and_target() {
    let n = archive_name(&Version::parse("1.2.3").unwrap());
    assert_eq!(n, format!("vitrum-1.2.3-{TARGET}.tar.gz"));
    assert!(
        !TARGET.is_empty(),
        "the build script did not record a target"
    );
}

/// The script that builds the asset and the code that looks for it agree.
///
/// These are the two halves of a release and nothing connects them at
/// compile time: the script writes a filename with shell interpolation and
/// this module reconstructs the same filename in Rust. If either shape
/// changes alone, `gh release create` uploads an asset that every client
/// scans past, and the only symptom is that nobody ever updates. Cheap to
/// assert here, invisible until a release is already published.
#[test]
fn the_release_script_builds_the_name_the_updater_looks_for() {
    let script = include_str!("../../../packaging/build-release-asset.sh");
    assert!(
        script.contains(r#"name="vitrum-${version}-${target}.tar.gz""#),
        "the asset name in build-release-asset.sh no longer matches archive_name()"
    );
    assert!(
        script.contains("sha256sum"),
        "the release script stopped producing a checksum, which the updater requires"
    );
    assert!(
        script.contains("SHA256SUMS"),
        "the updater looks for a file called SHA256SUMS and the script writes another name"
    );
    assert!(
        script.contains("vitrum vitrum-server"),
        "the archive must carry both binaries; they speak a versioned protocol to each other"
    );
}

/// The release doc tells the maintainer to upload what the updater needs.
///
/// A release published without `SHA256SUMS` is refused by every client, so
/// the upload line is part of the contract rather than a suggestion.
#[test]
fn the_release_doc_uploads_the_assets() {
    let doc = include_str!("../../../RELEASING.md");
    assert!(
        doc.contains("packaging/build-release-asset.sh"),
        "RELEASING.md no longer builds the update asset"
    );
    assert!(
        doc.contains("dist/SHA256SUMS"),
        "RELEASING.md no longer uploads the checksums the updater requires"
    );
}

/// A directory that cannot be written is detected before any download.
#[test]
fn an_unwritable_directory_is_detected() {
    assert!(writable(&std::env::temp_dir()));
    assert!(!writable(Path::new("/proc/self/nonexistent-dir")));
}
