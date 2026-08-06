//! Records the target triple this binary was built for.
//!
//! The updater downloads one archive per platform and has to name the right
//! one. That name has to describe the machine the binary was produced for, not
//! the machine it happens to be running on, so it is captured at compile time
//! from cargo's own `TARGET` rather than assembled at runtime from
//! `std::env::consts`, which cannot distinguish a gnu build from a musl one.

fn main() {
    let target = std::env::var("TARGET").expect("cargo sets TARGET for build scripts");
    println!("cargo:rustc-env=VITRUM_TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");
}
