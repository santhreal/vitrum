//! Does the terminal pane have to be xterm.js inside a webview?
//!
//! This binary answers that with three runnable modes rather than an opinion:
//!
//! - `native` — a GTK 3 window whose pane is a `wgpu` surface on a real X11
//!   drawable, a `vitrum_vt::Vt` fed from a real PTY, and `vitrum_grid`'s
//!   renderer drawing the `CellGrid`. `--webview` adds a real `WebKitWebView`
//!   to the same toplevel, which is the compositing question.
//! - `web` — the same PTY, the same bytes, into the vendored `xterm.js` the app
//!   ships, inside a real `WebKitWebView`. The baseline.
//! - `offscreen` — the native path with no window, on whatever adapter the box
//!   has, so the engine's cost and the X server's cost stay separable.
//!
//! Both windowed modes print one JSON object with a frame-time distribution
//! and a sustained byte rate, and neither arms a repeating timer, so idle CPU
//! is a property the process either has or does not.

mod bench;
mod native;
mod pty;
mod stats;
mod web;

const USAGE: &str = "\
pane-lab native    [--cols N] [--rows N] [--webview] [--vsync] [--seconds N] [--stats FILE] [-- PROG ARGS]
pane-lab web       [--cols N] [--rows N] [--webgl]  [--seconds N] [--stats FILE] [-- PROG ARGS]
pane-lab offscreen [--cols N] [--rows N] [--seconds N] [--stats FILE] [-- PROG ARGS]
";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let result = match args.get(1).map(String::as_str) {
        Some("native") => native::run(&args[2..]),
        Some("web") => web::run(&args[2..]),
        Some("offscreen") => bench::run(&args[2..]),
        _ => {
            eprint!("{USAGE}");
            std::process::exit(2);
        }
    };
    if let Err(err) = result {
        eprintln!("pane-lab: {err:#}");
        std::process::exit(1);
    }
}
