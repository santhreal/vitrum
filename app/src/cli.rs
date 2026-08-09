//! Command line: what `vitrum` accepts and what it prints when it does not
//! understand you.

use super::*;

use vitrum_proto::HintState;

/// Which xterm.js renderer to mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Renderer {
    /// The WebGL addon. Faster at redraw, and opt-in only.
    ///
    /// NOT the default, and this comment used to say it was. Measured against
    /// DOM at twenty windows it is heavier, not lighter: a compositing layer
    /// per window costs both memory and idle CPU, which are the two axes this
    /// product is built to win. `--renderer webgl` exists for the one case
    /// that wants it, tailing a very large build log in a single pane.
    Webgl,
    /// xterm.js's DOM renderer. The default; see [`Options::parse`], which is
    /// pinned by a test that says so.
    Dom,
}

impl Renderer {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Renderer::Webgl => "webgl",
            Renderer::Dom => "dom",
        }
    }
}

/// Command-line options.
///
/// `Copy`, and deliberately so: every event handler in the shell captures it,
/// and a clone per handler would be noise. That is why `server` is a
/// `&'static str` rather than a `String`.
// No `Eq`: `ui_scale` is a float, and a total-equality bound on a scale factor
// would be a promise this type cannot keep.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Options {
    /// Run against an in-memory fixture instead of a session server.
    ///
    /// Explicit and loud: fixture mode never opens a socket and paints a
    /// "FIXTURE DATA" banner. It is not a fallback for a failed connection,
    /// because a fallback that looks like success hides an outage.
    pub(crate) fixture: bool,
    /// Renderer for the terminal grid.
    ///
    /// The DOM renderer by default, because the choice is measurable rather
    /// than a preference: on this machine WebKitGTK 2.52 composites a live
    /// WebGL layer at a steady 0.24% CPU and ~80 MB more PSS, with nothing on
    /// screen changing and no JS timer scheduled, while the DOM renderer idles
    /// at 0.00%. Throughput is not the deciding factor at 20 agents: they
    /// produce ~0.4 MB/s combined. WebGL stays available for the one case that
    /// does want it, tailing a very large build log in a single pane.
    pub(crate) renderer: Renderer,
    /// WebSocket URL of the session daemon.
    ///
    /// Leaked on purpose. It is parsed once at startup and read for the life
    /// of the process, so a single small allocation that never returns is the
    /// honest trade for keeping [`Options`] `Copy`; there is no path that
    /// reparses arguments, so this cannot leak twice.
    pub(crate) server: &'static str,
    /// Magnification override for the whole document.
    ///
    /// `None` means "work it out from the panel", which is what
    /// [`Density::ui_scale`] does and what every normal launch wants. The
    /// override exists because a physical measurement can only ever be as
    /// good as the EDID behind it, and a user staring at a monitor that lies
    /// about its size needs a way to say so that does not involve recompiling.
    pub(crate) ui_scale: Option<f64>,
    /// Refuse to join, or hand off to, the running instance.
    ///
    /// Forced on by `--fixture` and by a non-default `--server`: those two
    /// flags say "connect this window to something specific", and handing
    /// them to an instance that is already pointed somewhere else silently
    /// ignores them. A developer who types `--fixture` while the real app is
    /// running must get fixture data, not a second window onto the daemon.
    pub(crate) standalone: bool,
    /// Never start the session daemon; assume somebody else did.
    ///
    /// The default is to start it, because the alternative is what shipped
    /// until now: a first launch on a clean machine paints a red banner with a
    /// Retry button that can never succeed, since nothing is listening and
    /// nothing is ever going to be. This flag is for the people running the
    /// daemon under a supervisor or a debugger, who want a failure to connect
    /// to stay a failure rather than be papered over with a second copy.
    pub(crate) autostart: bool,
}

/// Is this argument a `vitrum://` URL rather than an option?
///
/// The `://` is required. Matching the bare scheme would swallow `vitrumish`
/// as a deep link and start a window instead of reporting a typo, which the
/// first version of this did.
pub(crate) fn is_deep_link_arg(arg: &str) -> bool {
    let scheme = vitrum_os::branding::URL_SCHEME;
    arg.len() > scheme.len()
        && arg[..scheme.len()].eq_ignore_ascii_case(scheme)
        && arg[scheme.len()..].starts_with("://")
}

impl Options {
    pub(crate) fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Options, String> {
        let mut opts = Options {
            fixture: false,
            renderer: Renderer::Dom,
            server: wire::DEFAULT_WS_URL,
            ui_scale: None,
            standalone: false,
            autostart: true,
        };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--fixture" => opts.fixture = true,
                "--server" => {
                    let v = args
                        .next()
                        .ok_or_else(|| format!("--server needs a URL\n\n{}", usage()))?;
                    if !v.starts_with("ws://") && !v.starts_with("wss://") {
                        return Err(format!(
                            "--server {v} is not a WebSocket URL; expected ws:// or wss://\n\n{}",
                            usage()
                        ));
                    }
                    opts.server = v.leak();
                }
                "--renderer" => {
                    let v = args.next().ok_or_else(|| {
                        format!("--renderer needs a value: webgl or dom\n\n{}", usage())
                    })?;
                    opts.renderer = match v.as_str() {
                        "webgl" => Renderer::Webgl,
                        "dom" => Renderer::Dom,
                        other => {
                            return Err(format!(
                                "unknown renderer {other}, expected webgl or dom\n\n{}",
                                usage()
                            ));
                        }
                    };
                }
                "--ui-scale" => {
                    let v = args.next().ok_or_else(|| {
                        format!("--ui-scale needs a value: auto or a number\n\n{}", usage())
                    })?;
                    opts.ui_scale = match v.as_str() {
                        "auto" => None,
                        other => {
                            let n: f64 = other.parse().map_err(|_| {
                                format!(
                                    "--ui-scale {other} is not a number; expected auto or a \
                                     value between {MIN_UI_SCALE} and {MAX_UI_SCALE}\n\n{}",
                                    usage()
                                )
                            })?;
                            if !(MIN_UI_SCALE..=MAX_UI_SCALE).contains(&n) {
                                return Err(format!(
                                    "--ui-scale {n} is outside {MIN_UI_SCALE}..={MAX_UI_SCALE}\
                                     \n\n{}",
                                    usage()
                                ));
                            }
                            Some(n)
                        }
                    };
                }
                "--standalone" => opts.standalone = true,
                "--no-autostart" => opts.autostart = false,
                "-h" | "--help" => return Err(usage()),
                // Every shipped binary answers this. It is how an operator
                // filing a report says which build they are on, and how they
                // tell an installed copy from one they just rebuilt. Taken
                // from the crate version at compile time, so it cannot drift
                // from the tag the release was cut at.
                "-V" | "--version" => {
                    return Err(format!("vitrum {}", env!("CARGO_PKG_VERSION")));
                }
                // A `vitrum://` URL is not an option, it is the thing the OS
                // hands this binary when it opens a registered link, and
                // `Activation::from_args` reads it a few lines later. Rejecting
                // it here made the whole deep-link feature unreachable: the app
                // registers itself as the handler, the desktop launches
                // `vitrum vitrum://session/3`, and the process exits with
                // "unknown argument" before the activation is ever looked at.
                // Consumed, not stored, because this parser's job is options.
                other if is_deep_link_arg(other) => {
                    // A malformed one is NOT swallowed here: it reaches
                    // `Activation::from_args`, which reports what is wrong with
                    // the URL. "unknown argument" would name the wrong problem.
                }
                other => return Err(format!("unknown argument {other}\n\n{}", usage())),
            }
        }
        // A window pointed at something specific must never be swallowed by
        // an instance pointed somewhere else.
        opts.standalone |= opts.fixture || opts.server != wire::DEFAULT_WS_URL;
        Ok(opts)
    }
}

pub(crate) fn usage() -> String {
    format!(
        "vitrum - a terminal shell for coding agents\n\n\
         usage: vitrum [--server URL] [--fixture] [--renderer webgl|dom]\n                \
         [--ui-scale auto|N] [--standalone]\n\n\
         Launching again while vitrum is running opens another window in the\n\
         running process rather than a second copy of the program.\n\n\
         options:\n  \
         --server <url>       session daemon to connect to. Default {}.\n  \
         --fixture            render an in-memory fixture instead of connecting\n                       \
         to the session server. Development only; the\n                       \
         sidebar says so. Implies --standalone.\n  \
         --renderer <r>       terminal renderer: dom (default) or webgl. WebGL\n                       \
         redraws faster but keeps a compositing layer awake:\n                       \
         0.24% idle CPU, and 23 MB more across twenty\n                       \
         windows, both measured.\n  \
         --ui-scale <s>       magnification: auto (default) reads the panel's\n                       \
         physical size, or a number from {MIN_UI_SCALE} to {MAX_UI_SCALE} to\n                       \
         override a monitor that misreports it.\n  \
         --standalone         do not join or hand off to a running instance.\n  \
         -V, --version        print the version and exit.\n  \
         --no-autostart       do not start vitrum-server. By default the client\n                       \
         starts the session daemon if nothing is listening,\n                       \
         reuses one that already is, and never kills it on\n                       \
         exit: your agents outlive the window.\n  \
         -h, --help           show this message\n\n\
         commands:\n  \
         update               install the newest published release\n  \
         hint                 declare what a session is doing, so the sidebar\n                       \
         can show Approval and Input. `vitrum hint --help`.\n  \
         icons                write the launcher, Windows and macOS icons into\n                       \
         a data directory. `vitrum icons --help`.\n",
        wire::DEFAULT_WS_URL
    )
}

// ---------------------------------------------------------------------------
// The `hint` subcommand
// ---------------------------------------------------------------------------

/// What `vitrum hint` was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HintRequest {
    /// Write the sequence for this state, with this label.
    Declare {
        state: HintState,
        label: Option<String>,
    },
    /// Print the usage text.
    Help,
}

/// Read the arguments of `vitrum hint`.
///
/// Separate from writing the bytes because the two failures are different:
/// this one is the operator's typo, reported on stderr with an exit code a
/// script can branch on, and the other is a broken pipe. An unknown state is
/// an error rather than a default, the same rule the parser follows, because
/// silently declaring `ready` when the caller asked for `approvel` would put a
/// wrong badge on a row and never mention it.
pub(crate) fn parse_hint(args: &[String]) -> Result<HintRequest, String> {
    let mut state: Option<HintState> = None;
    let mut label: Option<String> = None;
    let mut cleared = false;
    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => return Ok(HintRequest::Help),
            // There is no erase token, and there should not be one: a stray
            // sequence could then blank a real approval gate. `working` is the
            // one state the resolver retires by itself, so declaring it hands
            // the row back to observation once the session goes quiet.
            "--clear" if state.is_none() && !cleared => {
                cleared = true;
                state = Some(HintState::Working);
            }
            // A label may well start with a dash, so only the words before a
            // state are read as options. After one, everything is text.
            other if state.is_none() && other.starts_with('-') => {
                return Err(format!("unknown option {other}\n\n{}", hint_usage()));
            }
            other if state.is_none() => {
                state = Some(HintState::parse(other).ok_or_else(|| {
                    format!(
                        "unknown state {other}; expected approval, input, working or ready\
                         \n\n{}",
                        hint_usage()
                    )
                })?);
            }
            other if cleared => {
                return Err(format!(
                    "--clear takes no other arguments, and {other} is one\n\n{}",
                    hint_usage()
                ));
            }
            other if label.is_none() => label = Some(other.to_string()),
            other => {
                return Err(format!(
                    "unexpected argument {other}; the label is one argument, so quote it\
                     \n\n{}",
                    hint_usage()
                ));
            }
        }
    }
    match state {
        Some(state) => Ok(HintRequest::Declare { state, label }),
        None => Err(format!("vitrum hint needs a state\n\n{}", hint_usage())),
    }
}

pub(crate) fn hint_usage() -> String {
    format!(
        "vitrum hint - tell the sidebar what this session is doing\n\n\
         usage: vitrum hint <state> [label]\n       \
         vitrum hint --clear\n\n\
         Writes an OSC {} sequence to stdout. Any terminal that does not know\n\
         it ignores it, so a harness can emit it unconditionally.\n\n\
         Approval and Input exist ONLY here. They cannot be observed from a\n\
         PTY, because an agent asking to force-push and a shell sitting at a\n\
         prompt block in the same read. Without this command every row falls\n\
         back to the status vitrum observes.\n\n\
         states:\n  \
         approval             blocked asking you to approve an action\n  \
         input                blocked asking you a question\n  \
         working              running; needs nothing from you\n  \
         ready                finished a unit of work\n\n\
         options:\n  \
         --clear              hand the row back to the observed status. Declares\n                       \
         working, which is the one state that retires itself\n                       \
         once the session goes quiet.\n  \
         -h, --help           show this message\n\n\
         The label is one argument and is shown beside the row. Control\n\
         characters in it become spaces and it is truncated to fit.\n\n\
         example:\n  \
         vitrum hint approval 'run `rm -rf build/`?'\n\n\
         exit status:\n  \
         0                    the sequence was written\n  \
         2                    no state, or a state that does not exist\n",
        vitrum_proto::HINT_OSC
    )
}

#[cfg(test)]
mod what_the_command_line_accepts;
