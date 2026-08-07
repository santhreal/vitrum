//! The two facts that decide whether this workspace renders on Windows.
//!
//! Both were broken at once and neither was visible from Linux. Direct3D 12
//! was switched off to make the workspace compile, which it did, and then 43
//! renderer tests failed on a real Windows runner because D3D12's WARP device
//! is the only adapter a GPU-less Windows VM has. Turning it back on needs
//! `Cargo.lock` to hand gpu-allocator the same `windows` crate wgpu-hal uses.
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
/// wgpu-hal requires `windows` 0.62 and `tao` requires 0.61, so both are in
/// the graph and neither can move. gpu-allocator accepts `>=0.53, <=0.62`, and
/// cargo would rather reuse a node than add one, so left alone it wires
/// gpu-allocator to 0.61 and `D3D12_RESOURCE_DESC` stops being one type across
/// the call between them. `Cargo.lock` points that edge at 0.62 instead.
///
/// Compared against wgpu-hal's own edge rather than a version written down
/// here, so this keeps holding when wgpu moves to a newer `windows`.
#[test]
fn the_lock_gives_gpu_allocator_the_windows_wgpu_hal_uses() {
    let lock = read_repo_file("Cargo.lock");
    let consumer = windows_edge(&lock, "wgpu-hal");
    let allocator = windows_edge(&lock, "gpu-allocator");
    assert_eq!(
        allocator, consumer,
        "gpu-allocator is built against windows {allocator} while wgpu-hal \
         uses {consumer}, so the Direct3D handles they pass between them are \
         different types and dx12 does not compile"
    );
}

/// The `windows` version a locked package is built against.
fn windows_edge(lock: &str, package: &str) -> String {
    let start = lock
        .find(&format!("[[package]]\nname = \"{package}\"\n"))
        .unwrap_or_else(|| panic!("{package} is not in Cargo.lock"));
    let rest = &lock[start..];
    // A package block ends where the next one begins, or at the end of file.
    let end = rest[1..]
        .find("\n[[package]]")
        .map(|at| at + 1)
        .unwrap_or(rest.len());
    rest[..end]
        .lines()
        .filter_map(|line| line.trim().trim_matches(['"', ','].as_slice()).strip_prefix("windows "))
        .map(str::to_string)
        .next()
        .unwrap_or_else(|| panic!("{package} has no `windows` dependency in Cargo.lock"))
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
