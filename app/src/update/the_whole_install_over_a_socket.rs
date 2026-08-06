//// The whole path, over a real socket.
////
//// Every other test here exercises one step with the others stubbed out. This
//// one runs `install` exactly as the terminal command and the window button
//// run it: an HTTP fetch of an archive, an HTTP fetch of the sums, a checksum
//// pass, an unpack, and the rename that replaces the running program. The
//// pieces were each correct in isolation while the whole was wired wrong more
//// than once, which is the only reason to pay for a socket in a unit test.

use super::*;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

/// Serve a fixed set of paths once each, then stop.
fn serve(routes: Vec<(String, Vec<u8>)>) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bound");
    let base = format!("http://{}", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        for _ in 0..routes.len() {
            let Ok((mut sock, _)) = listener.accept() else {
                return;
            };
            let mut reader = BufReader::new(sock.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
            // Drain headers so the client is not left writing into a
            // socket nobody is reading.
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
                None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
            };
            let _ = sock.write_all(&response);
            let _ = sock.flush();
        }
    });
    (base, handle)
}

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

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vitrum-install-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn available(base: &str) -> Available {
    Available {
        version: Version::parse("9.9.9").unwrap(),
        tag: "v9.9.9".to_string(),
        asset_url: Some(format!("{base}/archive")),
        sums_url: Some(format!("{base}/sums")),
    }
}

/// A good release replaces both binaries and reports each step.
#[test]
fn a_verified_release_replaces_both_binaries() {
    let dir = scratch("ok");
    fs::write(dir.join("vitrum"), b"old client").unwrap();
    fs::write(dir.join("vitrum-server"), b"old daemon").unwrap();

    let archive = archive_of(b"new client", b"new daemon");
    let name = archive_name(&Version::parse("9.9.9").unwrap());
    let sums = format!("{}  {name}\n", hex(&Sha256::digest(&archive)));
    let (base, server) = serve(vec![
        ("/archive".to_string(), archive),
        ("/sums".to_string(), sums.into_bytes()),
    ]);

    let mut steps = Vec::new();
    install(&available(&base), &dir, &mut |s| steps.push(s.to_string()))
        .expect("install succeeded");
    server.join().ok();

    assert_eq!(fs::read(dir.join("vitrum")).unwrap(), b"new client");
    assert_eq!(fs::read(dir.join("vitrum-server")).unwrap(), b"new daemon");
    assert!(
        steps.iter().any(|s| s.contains("verifying checksum")),
        "the operator was never told it verified: {steps:?}"
    );
    assert!(
        steps.iter().any(|s| s.contains("updated to 9.9.9")),
        "no completion line: {steps:?}"
    );
    // Nothing left behind to be mistaken for a binary.
    let leftovers: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with('.'))
        .collect();
    assert!(leftovers.is_empty(), "staging files survived: {leftovers:?}");
    fs::remove_dir_all(&dir).ok();
}

/// A tampered download leaves the installed copy untouched.
///
/// This is the property that matters most and the one an isolated
/// checksum test cannot show: that a failed verification happens *before*
/// anything on disk is replaced, so a machine served a bad archive keeps
/// running the version it had.
#[test]
fn a_tampered_download_never_touches_the_installed_copy() {
    let dir = scratch("bad");
    fs::write(dir.join("vitrum"), b"old client").unwrap();
    fs::write(dir.join("vitrum-server"), b"old daemon").unwrap();

    let archive = archive_of(b"malicious", b"malicious");
    let name = archive_name(&Version::parse("9.9.9").unwrap());
    // A digest of something else entirely: what a mirror serving a swapped
    // archive alongside the real sums file looks like.
    let sums = format!("{}  {name}\n", hex(&Sha256::digest(b"the real release")));
    let (base, server) = serve(vec![
        ("/archive".to_string(), archive),
        ("/sums".to_string(), sums.into_bytes()),
    ]);

    let e = install(&available(&base), &dir, &mut |_| {}).unwrap_err();
    server.join().ok();

    assert!(e.to_string().contains("checksum mismatch"), "{e}");
    assert_eq!(fs::read(dir.join("vitrum")).unwrap(), b"old client");
    assert_eq!(fs::read(dir.join("vitrum-server")).unwrap(), b"old daemon");
    fs::remove_dir_all(&dir).ok();
}

/// A release with no checksums is refused before anything is downloaded.
#[test]
fn a_release_without_sums_is_refused() {
    let dir = scratch("nosums");
    fs::write(dir.join("vitrum"), b"old client").unwrap();
    let mut a = available("http://127.0.0.1:1");
    a.sums_url = None;
    let e = install(&a, &dir, &mut |_| {}).unwrap_err();
    assert!(
        e.to_string().contains("refusing to install an unverified binary"),
        "{e}"
    );
    assert_eq!(fs::read(dir.join("vitrum")).unwrap(), b"old client");
    fs::remove_dir_all(&dir).ok();
}

/// A download that 404s is an error, not a silent no-op.
#[test]
fn a_missing_asset_is_an_error() {
    let dir = scratch("404");
    fs::write(dir.join("vitrum"), b"old client").unwrap();
    let (base, server) = serve(vec![("/nothing".to_string(), Vec::new())]);
    let e = install(&available(&base), &dir, &mut |_| {}).unwrap_err();
    server.join().ok();
    assert!(format!("{e:#}").contains("downloading"), "{e:#}");
    assert_eq!(fs::read(dir.join("vitrum")).unwrap(), b"old client");
    fs::remove_dir_all(&dir).ok();
}
