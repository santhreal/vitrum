//! Render a sidebar row from live values: the real clock, the real `$HOME`, the
//! real working directory, and a real child process's exit status.
//!
//! The unit tests pin every function against fixtures. This exercises the same
//! functions against whatever the operating system actually hands back, which
//! is the only way to catch a signature that is unusable in practice or an
//! assumption about input that only fixtures satisfy.
//!
//! ```text
//! cargo run -p vitrum-fmt --example sidebar_row
//! ```
//!
//! `VITRUM_UTC_OFFSET_SECS` sets the display zone; resolving it from the OS is
//! the host application's job, not this crate's.

use vitrum_fmt::{TimeFormat, Timestamp, bytes, count, duration, exit, git, path, text};
use std::time::{Duration, SystemTime};

const COLUMN: usize = 28;

fn main() {
    let now = Timestamp::from_system_time(SystemTime::now());
    let offset = std::env::var("VITRUM_UTC_OFFSET_SECS")
        .ok()
        .and_then(|raw| raw.parse::<i32>().ok())
        .unwrap_or(0);
    let clock = TimeFormat::new(now, offset);

    let home = std::env::var("HOME").unwrap_or_default();
    let cwd = std::env::current_dir()
        .map(|dir| dir.display().to_string())
        .unwrap_or_default();

    println!("now                {}", clock.absolute_datetime(now));
    println!("utc offset         {offset}s");
    println!();

    println!("== relative timestamps ==");
    for ago in [0u64, 3, 12, 90, 4_000, 100_000, 700_000, 40_000_000] {
        let then = Timestamp::from_millis(now.as_millis() - (ago as i64) * 1_000);
        println!("  {:>10}s ago -> {}", ago, clock.relative_ago(then));
    }
    let skewed = Timestamp::from_millis(now.as_millis() + 2_500);
    println!("  daemon 2.5s ahead -> {}", clock.relative(skewed));
    println!();

    println!("== working durations ==");
    for secs in [0u64, 7, 252, 3_601, 7_530, 273_600] {
        let elapsed = Duration::from_secs(secs);
        println!(
            "  {:>7}s -> compact {:<10} terse {:<6} clock {}",
            secs,
            duration::compact(elapsed),
            duration::terse(elapsed),
            duration::clock(elapsed)
        );
    }
    println!();

    println!("== live paths (column budget {COLUMN}) ==");
    println!("  cwd     {cwd}");
    println!("  home    {home}");
    println!("  tilde   {}", path::home_relative(&cwd, &home));
    println!("  fitted  {}", path::shorten_home_relative(&cwd, &home, COLUMN));
    println!("  label   {}", path::base_name(&cwd));
    let fitted = path::shorten_home_relative(&cwd, &home, COLUMN);
    assert!(
        text::display_width(&fitted) <= COLUMN,
        "a live path overran its budget: {fitted:?}"
    );
    println!();

    println!("== titles, sizes, counts ==");
    let hostile = "  vitrum\u{1b}[31m \u{2014} セッション一覧\tbuild  ";
    println!("  title   [{}]", text::pad_end(&text::title(hostile, COLUMN), COLUMN));
    for size in [0u64, 900, 1_229, 9_961_472, 3 << 30] {
        println!("  {:>12} bytes -> {}", size, bytes::binary(size));
    }
    println!("  {}", count::count_s(1, "session"));
    println!("  {}", count::count_s(20, "agent"));
    println!("  {}", count::count_or_none(0, "session", "sessions"));
    println!("  {}", count::count_s(128_480, "line"));
    println!();

    println!("== git heads (column budget 20) ==");
    for head in [
        git::Head::Branch("refs/heads/main"),
        git::Head::Branch("refs/heads/feature/renovate/bump-serde"),
        git::Head::Detached("1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b"),
        git::Head::Unborn,
    ] {
        println!("  {:<20} <- {head:?}", git::head(head, 20));
    }
    println!();

    println!("== real child processes ==");
    report_child("exit 0", &["-c", "exit 0"]);
    report_child("exit 101", &["-c", "exit 101"]);
    report_killed();
}

fn report_child(label: &str, args: &[&str]) {
    match std::process::Command::new("/bin/sh").args(args).status() {
        Ok(status) => println!(
            "  {label:<10} -> {}",
            exit::describe(exit::Termination::from_status(&status))
        ),
        Err(err) => println!("  {label:<10} -> could not spawn: {err}"),
    }
}

fn report_killed() {
    let spawned = std::process::Command::new("/bin/sh")
        .args(["-c", "sleep 30"])
        .spawn();
    let Ok(mut child) = spawned else {
        println!("  killed     -> could not spawn");
        return;
    };
    if child.kill().is_err() {
        println!("  killed     -> could not signal");
        return;
    }
    match child.wait() {
        Ok(status) => println!(
            "  {:<10} -> {}",
            "killed",
            exit::describe(exit::Termination::from_status(&status))
        ),
        Err(err) => println!("  killed     -> could not reap: {err}"),
    }
}
