//! Command line: what `vitrum` accepts, what it prints when it does not
//! understand you, and what it exits with either way.

use super::*;

use vitrum_proto::exit::{self, Exit};
use vitrum_proto::{HintState, token};

/// The parser stopped, and this is what the process says on the way out.
///
/// One type for the whole command-line boundary, carrying the three things a
/// caller needs: the text, the stream it belongs on, and the code.
///
/// It covers help and `--version` as well as mistakes, and that is the point.
/// Those used to travel as `Err(String)` beside real errors, and `main`
/// printed every one of them to stdout and returned normally, so `vitrum
/// --bogus` wrote its usage where a script was reading output and exited 0.
/// A wrong flag looked exactly like a successful launch. Here the difference
/// is a field rather than a convention, and [`CliExit::report`] is the only
/// place that chooses a stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CliExit {
    /// What to print, without a trailing newline.
    pub(crate) message: String,
    /// What the process exits with.
    pub(crate) exit: Exit,
}

impl CliExit {
    /// Output the operator asked for: help, or the version.
    pub(crate) fn asked(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            exit: Exit::Ok,
        }
    }

    /// A command line this program cannot act on.
    pub(crate) fn misuse(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            exit: Exit::Usage,
        }
    }

    /// Print to the right stream and return the process exit code.
    ///
    /// Asked-for output goes to stdout, because a caller doing `vitrum --help
    /// | less` is asking for it. Everything else goes to stderr, because a
    /// caller reading stdout is not asking for a diagnostic and a usage dump
    /// mixed into their data is worse than none.
    pub(crate) fn report(&self) -> i32 {
        if self.exit == Exit::Ok {
            println!("{}", self.message);
        } else {
            eprintln!("{}", self.message);
        }
        self.exit.code()
    }
}

impl std::fmt::Display for CliExit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// How to call `vitrum`, in three lines.
///
/// One owner, read by [`usage`] and by every diagnostic, so a caller told how
/// to call this program is told the same thing whichever way they arrived.
pub(crate) const SYNOPSIS: &str = "\
usage: vitrum [--server URL] [--fixture] [--renderer webgl|dom]\n              \
[--ui-scale auto|N] [--standalone] [--no-autostart]\n              \
[--token-file PATH]\n       \
vitrum update|hint|icons";

/// A diagnostic, in the shape a Unix tool writes one.
///
/// Three lines: what went wrong, how the command is called, and where the
/// rest of the manual is. `command` is the program as the operator typed it,
/// including the subcommand, so a line on a shared stderr says who wrote it.
///
/// The whole manual used to follow the first line instead. Forty options went
/// to stderr for one mistyped flag, and on a short terminal the sentence
/// naming the mistake had scrolled off by the time the shell prompt came
/// back. The four surfaces also disagreed about whether to name themselves at
/// all: `icons` did, `update` and the option parser did not.
pub(crate) fn diagnostic(command: &str, problem: &str, synopsis: &str) -> String {
    format!("{command}: {problem}\n{synopsis}\nRun '{command} --help' for the options.")
}

/// [`diagnostic`] for the option parser, which has no subcommand name.
fn misuse(problem: impl AsRef<str>) -> CliExit {
    CliExit::misuse(diagnostic("vitrum", problem.as_ref(), SYNOPSIS))
}

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
    /// Where to read the daemon token from, overriding the default file.
    ///
    /// Leaked for the same reason `server` is. `None` means the file
    /// [`vitrum_proto::token::path`] resolves, which is where a daemon on this
    /// machine writes it.
    pub(crate) token_file: Option<&'static str>,
}

/// The environment variable that outranks every token file.
pub(crate) const TOKEN_VAR: &str = "VITRUM_TOKEN";

/// What a handshake presents, and whose problem a missing token is.
#[derive(Debug)]
pub(crate) enum Token {
    /// Found and well formed.
    Present(String),
    /// Nothing named a token, and the file a daemon on this machine would have
    /// written could not be used.
    ///
    /// The handshake still goes ahead, with an empty token, because this
    /// client only guessed at the path and the daemon knows it. An older
    /// daemon wants no token at all and answers with the version skew; a
    /// current one answers by naming the file it actually wrote and who can
    /// read it. Refusing here instead put a guess on the screen in place of
    /// either answer, and against a daemon from an earlier release it reported
    /// a missing token when the real problem was that the daemon predated
    /// tokens entirely.
    Unnamed(token::TokenError),
    /// `VITRUM_TOKEN` or `--token-file` named a token that cannot be used.
    ///
    /// Nothing is sent. A named source is this client's own configuration, and
    /// a typo in it is not a question for the daemon.
    Named(token::TokenError),
}

/// The token this client presents to the daemon.
///
/// Three inputs, in one order, and every one of them ends in
/// [`vitrum_proto::token::validate`] so there is a single definition of what a
/// token is:
///
/// 1. `VITRUM_TOKEN`, which is how a token reaches a client talking to a
///    daemon on another machine through a tunnel. The value, not a path,
///    because a path is only useful on the machine that has the file.
/// 2. `--token-file`, for a copied file that is not where a local daemon would
///    have written one.
/// 3. The file a daemon on this machine writes.
///
/// The secret is never an argument. `ps` is readable by every account on the
/// machine, so a token on a command line is a token published to the machine
/// the token exists to keep out.
///
/// Read at each handshake rather than once at startup: the daemon writes a
/// fresh token every time it starts, so a client that cached one at launch
/// would fail to reconnect to a daemon that had been restarted, which is the
/// one moment a reconnect matters.
pub(crate) fn resolve_token(opts: Options) -> Token {
    resolve_token_from(std::env::var(TOKEN_VAR).ok().as_deref(), opts.token_file)
}

/// [`resolve_token`] with the environment supplied rather than read.
///
/// The environment is process-global and a test suite is not, so the order of
/// precedence is proved here, against values, instead of against a variable
/// every other test in the binary would see.
pub(crate) fn resolve_token_from(from_env: Option<&str>, from_flag: Option<&str>) -> Token {
    // An empty value is treated as unset. An exported but blank variable is a
    // script that meant to set it and did not, and refusing on the empty
    // string rather than falling through to the file would break a local
    // client over a variable nobody meant to set.
    if let Some(value) = from_env.filter(|v| !v.trim().is_empty()) {
        return match token::validate(value, TOKEN_VAR) {
            Ok(token) => Token::Present(token),
            Err(e) => Token::Named(e),
        };
    }
    match from_flag {
        Some(path) => match token::load_from(std::path::Path::new(path)) {
            Ok(token) => Token::Present(token),
            Err(e) => Token::Named(e),
        },
        None => match token::load() {
            Ok(token) => Token::Present(token),
            Err(e) => Token::Unnamed(e),
        },
    }
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
    pub(crate) fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Options, CliExit> {
        let mut opts = Options {
            fixture: false,
            renderer: Renderer::Dom,
            server: wire::DEFAULT_WS_URL,
            ui_scale: None,
            standalone: false,
            autostart: true,
            token_file: None,
        };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--fixture" => opts.fixture = true,
                "--token-file" => {
                    let v = args
                        .next()
                        .ok_or_else(|| misuse("--token-file needs a path"))?;
                    if v.is_empty() {
                        return Err(misuse("--token-file needs a path, and it was empty"));
                    }
                    opts.token_file = Some(v.leak());
                }
                "--server" => {
                    let v = args
                        .next()
                        .ok_or_else(|| misuse("--server needs a ws:// or wss:// URL"))?;
                    if !v.starts_with("ws://") && !v.starts_with("wss://") {
                        return Err(misuse(format!(
                            "--server {v} is not a WebSocket URL. Use ws:// for a daemon on \
                             this machine, or wss:// for one reached through a tunnel."
                        )));
                    }
                    opts.server = v.leak();
                }
                "--renderer" => {
                    let v = args
                        .next()
                        .ok_or_else(|| misuse("--renderer needs a value: webgl or dom"))?;
                    opts.renderer = match v.as_str() {
                        "webgl" => Renderer::Webgl,
                        "dom" => Renderer::Dom,
                        other => {
                            return Err(misuse(format!(
                                "unknown renderer {other}. The renderers are dom and webgl."
                            )));
                        }
                    };
                }
                "--ui-scale" => {
                    let v = args
                        .next()
                        .ok_or_else(|| misuse("--ui-scale needs a value: auto or a number"))?;
                    opts.ui_scale = match v.as_str() {
                        "auto" => None,
                        other => {
                            let n: f64 = other.parse().map_err(|_| {
                                misuse(format!(
                                    "--ui-scale {other} is not a number. Pass auto, or a value \
                                     between {MIN_UI_SCALE} and {MAX_UI_SCALE}."
                                ))
                            })?;
                            if !(MIN_UI_SCALE..=MAX_UI_SCALE).contains(&n) {
                                return Err(misuse(format!(
                                    "--ui-scale {n} is outside {MIN_UI_SCALE} to {MAX_UI_SCALE}. \
                                     Pass auto to read the panel's physical size."
                                )));
                            }
                            Some(n)
                        }
                    };
                }
                "--standalone" => opts.standalone = true,
                "--no-autostart" => opts.autostart = false,
                "-h" | "--help" => return Err(CliExit::asked(usage())),
                // Every shipped binary answers this. It is how an operator
                // filing a report says which build they are on, and how they
                // tell an installed copy from one they just rebuilt. Taken
                // from the crate version at compile time, so it cannot drift
                // from the tag the release was cut at.
                "-V" | "--version" => {
                    return Err(CliExit::asked(format!(
                        "vitrum {}",
                        env!("CARGO_PKG_VERSION")
                    )));
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
                other => {
                    return Err(misuse(format!("unknown argument {other}")));
                }
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
         {SYNOPSIS}\n\n\
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
         --token-file <path>  read the daemon token from this file instead of\n                       \
         the one a daemon on this machine writes. Set\n                       \
         {TOKEN_VAR} to pass the token itself, which is what\n                       \
         a daemon reached through a tunnel needs. A token is\n                       \
         never taken as an argument: ps is readable by every\n                       \
         account on the machine.\n  \
         -h, --help           show this message\n\n\
         commands:\n  \
         update               install the newest published release\n  \
         hint                 declare what a session is doing, so the sidebar\n                       \
         can show Approval and Input. `vitrum hint --help`.\n  \
         icons                write the launcher, Windows and macOS icons into\n                       \
         a data directory. `vitrum icons --help`.\n\n\
         exit status:\n\
         {}",
        wire::DEFAULT_WS_URL,
        exit::status_lines(EXIT_CODES)
    )
}

/// Every code `vitrum` itself can exit with, before a subcommand takes over.
///
/// The subcommands document their own, which are narrower: `hint` cannot fail
/// the way `update` can. Listing the union here would tell an operator to
/// expect 4 from a window launch, which never happens.
pub(crate) const EXIT_CODES: &[Exit] = &[Exit::Ok, Exit::Usage];

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
    let bad = |problem: String| diagnostic("vitrum hint", &problem, HINT_SYNOPSIS);
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
                return Err(bad(format!("unknown option {other}")));
            }
            other if state.is_none() => {
                state = Some(HintState::parse(other).ok_or_else(|| {
                    bad(format!(
                        "unknown state {other}. The states are approval, input, working \
                         and ready."
                    ))
                })?);
            }
            other if cleared => {
                return Err(bad(format!(
                    "--clear takes no other arguments, and {other} is one"
                )));
            }
            other if label.is_none() => label = Some(other.to_string()),
            other => {
                return Err(bad(format!(
                    "unexpected argument {other}. The label is one argument, so quote it."
                )));
            }
        }
    }
    match state {
        Some(state) => Ok(HintRequest::Declare { state, label }),
        None => Err(bad("no state was given".to_string())),
    }
}

/// How to call `vitrum hint`.
pub(crate) const HINT_SYNOPSIS: &str = "\
usage: vitrum hint <state> [label]\n       \
vitrum hint --clear";

pub(crate) fn hint_usage() -> String {
    format!(
        "vitrum hint - tell the sidebar what this session is doing\n\n\
         {HINT_SYNOPSIS}\n\n\
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
         exit status:\n\
         {}",
        vitrum_proto::HINT_OSC,
        exit::status_lines(HINT_EXIT_CODES)
    )
}

/// Every code `vitrum hint` can exit with.
///
/// [`Exit::Failed`] is here because stdout can be a closed pipe: a prompt
/// command whose reader went away has written nothing, and saying so beats
/// claiming a hint was declared that no terminal ever saw.
pub(crate) const HINT_EXIT_CODES: &[Exit] = &[Exit::Ok, Exit::Failed, Exit::Usage];

#[cfg(test)]
mod what_the_command_line_accepts;

/// The exit-code table is one table, and each command's help matches what it
/// really returns.
#[cfg(test)]
mod what_each_command_exits_with;

/// Where a client's token comes from, in which order, and what each way of
/// getting it wrong says.
#[cfg(test)]
mod how_a_token_reaches_the_daemon;
