//! The two facts that decide whether this workspace renders on Windows.
//!
//! Both were broken at once and neither was visible from Linux. Direct3D 12
//! was switched off to make the workspace compile, which it did, and then 43
//! renderer tests failed on a real Windows runner because D3D12's WARP device
//! is the only adapter a GPU-less Windows VM has. Turning it back on needs
//! `Cargo.lock` to hand gpu-allocator the same `windows` crate wgpu-hal uses,
//! which stays true only while the graph locks exactly one of them.
//!
//! The Windows cross check in CI catches a regression of either within about a
//! minute, and these say out loud what it would be failing about.

/// The renderer asks for every backend the default feature set has.
///
/// `default-features = false` is how this crate reaches `webgpu`, so the
/// backend list is written out by hand and a backend can go missing by
/// deleting one word. Direct3D 12 is the one that matters most: it is the
/// native Windows backend, and its software adapter is what a machine with no
/// GPU driver falls back to.
#[test]
fn the_renderer_keeps_every_backend_the_default_set_has() {
    let manifest = read_repo_file("crates/vitrum-grid/Cargo.toml");
    let line = manifest
        .lines()
        .find(|line| line.starts_with("wgpu = "))
        .expect("vitrum-grid declares wgpu on one line");
    for backend in ["dx12", "metal", "vulkan", "gles", "webgpu"] {
        assert!(
            line.contains(&format!("\"{backend}\"")),
            "vitrum-grid no longer builds the {backend} backend. Dropping one \
             is a supported choice, but it costs every machine that has only \
             that backend: without dx12 a Windows box with no GPU driver has \
             no adapter at all, which is how 43 renderer tests failed there."
        );
    }
}

/// gpu-allocator and wgpu-hal name Direct3D types through the same `windows`.
///
/// wgpu-hal requires `windows` 0.62 and gpu-allocator accepts
/// `>=0.53, <=0.62`. While a second consumer pinned an older one, cargo would
/// rather reuse a node than add one, so it wired gpu-allocator to that older
/// node and `D3D12_RESOURCE_DESC` stopped being one type across the call
/// between them. The webview brought that second consumer in; nothing does
/// now, so the graph locks one `windows` and the two agree by construction.
///
/// Asserted as a count rather than as a version, because the defect is a
/// SECOND node existing at all. A lock with one writes the edge unversioned,
/// so a version comparison cannot see the difference between agreement and a
/// split that has not happened yet.
#[test]
fn the_lock_holds_one_windows_crate_for_every_direct3d_consumer() {
    let lock = read_repo_file("Cargo.lock");
    let versions = locked_versions(&lock, "windows");
    assert_eq!(
        versions.len(),
        1,
        "Cargo.lock holds {} `windows` packages ({versions:?}); cargo wires \
         gpu-allocator to whichever node it can reuse, so the Direct3D handles \
         it passes wgpu-hal become a different type and dx12 does not compile. \
         Pin gpu-allocator's edge at the one wgpu-hal uses.",
        versions.len()
    );
    for consumer in ["wgpu-hal", "gpu-allocator"] {
        assert!(
            package_block(&lock, consumer).contains("\n \"windows\""),
            "{consumer} no longer depends on `windows` at all, so this guard \
             has stopped covering the edge it was written for"
        );
    }
}

/// Every locked version of `package`, in lock order.
fn locked_versions(lock: &str, package: &str) -> Vec<String> {
    lock.split("[[package]]\n")
        .filter_map(|block| block.strip_prefix(&format!("name = \"{package}\"\nversion = \"")))
        .filter_map(|rest| rest.split_once('"'))
        .map(|(version, _)| version.to_string())
        .collect()
}

/// The `[[package]]` block for `package`, up to the next one.
fn package_block<'a>(lock: &'a str, package: &str) -> &'a str {
    let start = lock
        .find(&format!("[[package]]\nname = \"{package}\"\n"))
        .unwrap_or_else(|| panic!("{package} is not in Cargo.lock"));
    let rest = &lock[start..];
    let end = rest[1..]
        .find("\n[[package]]")
        .map_or(rest.len(), |at| at + 1);
    &rest[..end]
}

/// A file read from the repository root.
fn read_repo_file(relative: &str) -> String {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the app crate has a parent directory")
        .join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|why| panic!("cannot read {}: {why}", path.display()))
}
