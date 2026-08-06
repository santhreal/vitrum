//! The About tab: what is installed, and the update control.
//!
//! Separate from the rest of the sheet because it edits no preference at all.
//! Every other panel reads and writes [`crate::state::Settings`]; this one
//! runs [`crate::update`] and reports what it found, which is a different
//! job with a different failure mode.

use dioxus::prelude::*;

use crate::state::UiState;

/// What the update control is doing right now.
///
/// One value rather than a set of booleans, because the states are mutually
/// exclusive and a pair of flags is how a control ends up saying "checking"
/// and "up to date" at once.
#[derive(Debug, Clone, PartialEq, Eq)]
enum UpdateUi {
    /// Nothing asked for yet.
    Idle,
    /// A check or an install is in flight; the string is the current step.
    Busy(String),
    /// The answer to the last check.
    Answer(crate::update::Status),
    /// The update finished and the new binaries are on disk.
    Installed(String),
    /// The last attempt failed, and why.
    Failed(String),
}

#[component]
pub(super) fn AboutPanel(state: Signal<UiState>) -> Element {
    let mut ui = use_signal(|| UpdateUi::Idle);
    let current = crate::update::current_version();

    // The daemon is a separate process that outlives every window, so its
    // version is not this binary's version and after an update it will not be.
    // Read from the Welcome frame rather than assumed.
    let daemon_version = match &state.read().daemon.conn {
        crate::state::ConnState::Live { server_version } => Some(server_version.clone()),
        _ => None,
    };
    let daemon_is_stale = daemon_version
        .as_deref()
        .is_some_and(|v| v != current.to_string());

    // Held so the Install button knows what it is installing without asking
    // the network a second time and risking a different answer than the one
    // the operator is looking at.
    let mut ready = use_signal(|| None::<crate::update::Available>);

    rsx! {
        div { class: "rg-field",
            span { class: "rg-field__label", "Version" }
            span { class: "rg-field__hint", "vitrum {current} ({crate::update::TARGET})" }
            span { class: "rg-field__hint",
                match &daemon_version {
                    Some(v) if daemon_is_stale => format!(
                        "The daemon holding your sessions is still running {v}. Restarting it \
                         picks up {current} and ends every session it is holding."
                    ),
                    Some(v) => format!("Daemon {v}, running your sessions."),
                    None => "Not connected to a daemon.".to_string(),
                }
            }
        }

        div { class: "rg-field",
            span { class: "rg-field__label", "Updates" }
            span { class: "rg-field__control",
                button {
                    class: "rg-btn rg-btn--primary",
                    r#type: "button",
                    disabled: matches!(ui(), UpdateUi::Busy(_)),
                    onclick: move |_| {
                        ui.set(UpdateUi::Busy("checking".to_string()));
                        ready.set(None);
                        spawn(async move {
                            // The check is a blocking HTTP round trip. On the
                            // UI thread it would freeze every window in this
                            // process, since they share one event loop.
                            let got = crate::off_thread(crate::update::check).await;
                            match got {
                                Ok(status) => {
                                    if let crate::update::Status::Ready(a) = &status {
                                        ready.set(Some(a.clone()));
                                    }
                                    ui.set(UpdateUi::Answer(status));
                                }
                                Err(e) => ui.set(UpdateUi::Failed(format!("{e:#}"))),
                            }
                        });
                    },
                    "Check for updates"
                }
                if let Some(available) = ready() {
                    button {
                        class: "rg-btn rg-btn--primary",
                        r#type: "button",
                        disabled: matches!(ui(), UpdateUi::Busy(_)),
                        onclick: move |_| {
                            let available = available.clone();
                            ui.set(UpdateUi::Busy("starting".to_string()));
                            spawn(async move {
                                let done = crate::off_thread(move || {
                                    let dir = crate::update::install_dir()?;
                                    if !crate::update::writable(&dir) {
                                        anyhow::bail!(
                                            "cannot write to {}. This copy was installed by \
                                             something else; update it the same way.",
                                            dir.display()
                                        );
                                    }
                                    // Progress is discarded on this path on
                                    // purpose: a signal cannot be written from
                                    // the worker thread, and the steps take a
                                    // few seconds in total. The button says
                                    // what is happening; a per-step readout
                                    // that flickers past is not worth a
                                    // channel.
                                    crate::update::install(&available, &dir, &mut |_| {})?;
                                    Ok::<_, anyhow::Error>(available.version.to_string())
                                })
                                .await;
                                match done {
                                    Ok(v) => ui.set(UpdateUi::Installed(v)),
                                    Err(e) => ui.set(UpdateUi::Failed(format!("{e:#}"))),
                                }
                            });
                        },
                        "Install {available.version}"
                    }
                }
            }
            span { class: "rg-field__hint",
                match ui() {
                    UpdateUi::Idle => format!(
                        "Checks the latest release of {}, never the branch. \
                         The download's checksum must match the one published beside it.",
                        crate::update::REPO
                    ),
                    UpdateUi::Busy(step) => step,
                    UpdateUi::Answer(crate::update::Status::UpToDate { version }) =>
                        format!("vitrum {version} is the newest release."),
                    UpdateUi::Answer(crate::update::Status::NoReleases) => format!(
                        "No releases published for {} yet.", crate::update::REPO
                    ),
                    UpdateUi::Answer(crate::update::Status::NoAssetForPlatform { version, target }) =>
                        format!(
                            "vitrum {version} is available but published no build for {target}. \
                             Build it from source."
                        ),
                    UpdateUi::Answer(crate::update::Status::Ready(a)) =>
                        format!("vitrum {} is available. You have {current}.", a.version),
                    UpdateUi::Installed(v) =>
                        format!("Updated to {v}. {}", crate::update::AFTER_INSTALL),
                    UpdateUi::Failed(why) => why,
                }
            }
        }

        div { class: "rg-field",
            span { class: "rg-field__label", "From a terminal" }
            span { class: "rg-field__hint",
                "vitrum update --check   reports what is available and installs nothing. \
                 vitrum update           installs it. Same code as the button above."
            }
        }
    }
}
