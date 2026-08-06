//! Exercise every OS integration against the running desktop and print what
//! actually happened.
//!
//! The test suite asserts logic and asserts that absent services are reported
//! as absent. This binary is the other half: it does the visible things (raises
//! real notifications, sets a real badge, puts a real icon in the tray) so a
//! human can confirm they appear.
//!
//! ```text
//! cargo run -p vitrum-os --example live_check
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use vitrum_os::deeplink::{RegistrationPlan, plan_registration};
use vitrum_os::notify::Notification;
use vitrum_os::paths::Platform;
use vitrum_os::tray::TrayCommand;
use vitrum_os::window_state::{Monitor, WindowState, clamp_to_monitors};
use vitrum_os::{AppPaths, badge, notify, theme, time, tray};
use vitrum_proto::SessionId;

fn main() {
    section("capability report");
    print!("{}", vitrum_os::probe(None));

    section("resolved paths");
    match AppPaths::for_current_platform() {
        Ok(p) => {
            println!("config    {}", p.config_dir.display());
            println!("data      {}", p.data_dir.display());
            println!("cache     {}", p.cache_dir.display());
            println!("state     {}", p.state_dir.display());
            println!("runtime   {}", p.runtime_dir.display());
            println!("window    {}", p.window_state_file().display());
            println!("lock      {}", p.instance_lock_file().display());
            println!("socket    {}", p.instance_socket_path().display());
        }
        Err(e) => println!("unavailable: {e}"),
    }
    println!("home      {:?}", vitrum_os::paths::home_dir());
    println!("utc offset {} seconds", time::utc_offset_secs());

    section("theme");
    // Held for the whole run: dropping the watcher stops its listener thread,
    // which is the documented lifetime and the mistake a caller makes first.
    let theme_watcher = theme::theme_watcher();
    match &theme_watcher {
        Ok(watcher) => {
            println!("preference {:?}", watcher.preference());
            println!("current    {:?}", watcher.current());
            let changes = Arc::new(AtomicU32::new(0));
            let counter = Arc::clone(&changes);
            match watcher.subscribe(Arc::new(move |t| {
                counter.fetch_add(1, Ordering::SeqCst);
                println!("theme changed to {t}");
            })) {
                Ok(()) => println!("subscribed to change notifications"),
                Err(e) => println!("subscription failed: {e}"),
            }
        }
        Err(e) => println!("unavailable: {e}"),
    }

    section("notifications");
    // Also held for the whole run, for the same reason.
    let notifier = notify::notifier();
    match &notifier {
        Ok(notifier) => {
            let clicked = Arc::new(AtomicU32::new(0));
            let counter = Arc::clone(&clicked);
            match notifier.set_activation_handler(Arc::new(move |session| {
                counter.fetch_add(1, Ordering::SeqCst);
                println!("activated: session {}", session.0);
            })) {
                Ok(()) => println!("activation handler installed"),
                Err(e) => println!("activation handler failed: {e}"),
            }
            for n in [
                Notification::finished(SessionId(1), "cargo build", "Finished in 12.4s"),
                Notification::needs_approval(
                    SessionId(2),
                    "claude",
                    "Run `rm -rf target`? [y/N]",
                ),
                Notification::failed(SessionId(3), "pytest", "3 failed, 41 passed"),
            ] {
                match notifier.notify(&n) {
                    Ok(handle) => println!("{:<14} -> id {}", n.kind.as_str(), handle.0),
                    Err(e) => println!("{:<14} -> {e}", n.kind.as_str()),
                }
            }
            println!("three notifications raised; click one within 15s to test activation");
        }
        Err(e) => println!("unavailable: {e}"),
    }

    section("badge");
    match badge::badge(None) {
        Ok(b) => match b.set_count(3) {
            Ok(()) => println!("set to 3"),
            Err(e) => println!("set failed: {e}"),
        },
        Err(e) => println!("unavailable: {e}"),
    }

    section("tray");
    let mut tray_handle = match tray::tray(Arc::new(|command: TrayCommand| {
        println!("tray command: {command:?}");
    })) {
        Ok(mut t) => {
            match t.set_count(3) {
                Ok(()) => println!("registered with attention count 3"),
                Err(e) => println!("count update failed: {e}"),
            }
            Some(t)
        }
        Err(e) => {
            println!("unavailable: {e}");
            None
        }
    };

    section("window state clamping");
    let saved = WindowState { x: 2400, y: 200, ..WindowState::default() };
    let monitors = [Monitor::new(0, 0, 1920, 1080)];
    println!("saved   {saved:?}");
    println!("clamped {:?}", clamp_to_monitors(&saved, &monitors));

    section("deep link registration plan");
    let exe = std::env::current_exe().unwrap_or_else(|_| "vitrum".into());
    if let Ok(paths) = AppPaths::for_current_platform() {
        match plan_registration(Platform::current(), &exe, &paths) {
            RegistrationPlan::DesktopEntry { path, contents, post_install } => {
                println!("write {}", path.display());
                println!("{contents}");
                for step in post_install {
                    println!("then: {}", step.join(" "));
                }
            }
            RegistrationPlan::BundleInfoPlist { fragment, note } => {
                println!("{fragment}\n{note}");
            }
            RegistrationPlan::RegistryValues { values } => {
                for v in values {
                    println!("HKCU\\{} [{:?}] = {:?}", v.key, v.name, v.value);
                }
            }
        }
    }

    section("waiting 15s for tray and notification interaction");
    std::thread::sleep(Duration::from_secs(15));

    if let Some(t) = tray_handle.as_mut() {
        t.shutdown();
        println!("tray removed");
    }
    if let Ok(b) = badge::badge(None) {
        let _ = b.clear();
        println!("badge cleared");
    }
    drop(theme_watcher);
    drop(notifier);
}

fn section(title: &str) {
    println!("\n== {title} ==");
}
