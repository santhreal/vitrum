//! `vitrum-replay`: scrub a captured session from the command line.
//!
//! The library takes a byte stream and answers "what did the screen look like at byte
//! N". This binary is that, for an operator holding a file: a raw scrollback capture or
//! an asciicast v2 recording goes in, and a screen, a chapter list, a size report, or a
//! recording comes out.
//!
//! # Exit codes
//!
//! - `0`: the command did what it said.
//! - `1`: the file could not be read, parsed, or replayed. The reason goes to stderr.
//! - `2`: the command line was wrong. The usage goes to stderr.
//!
//! Nothing is written to stdout on a failure, so a pipeline never receives a half
//! recording.

use std::io::{self, Read, Write};
use std::process::ExitCode;

use vitrum_replay::asciicast::{self, Header, Utf8Policy};
use vitrum_replay::{Replay, ReplayConfig, Stream, Timeline};

const USAGE: &str = "\
vitrum-replay: scrub a captured terminal session.

Usage:
  vitrum-replay info    <FILE> [options]
  vitrum-replay screen  <FILE> [options]
  vitrum-replay markers <FILE> [options]
  vitrum-replay export  <FILE> [options]

FILE is a raw scrollback capture, or an asciicast v2 recording, or `-` for stdin.
A file whose first non-blank byte is `{` is read as asciicast; anything else is
read as raw bytes.

Commands:
  info      size, geometry, chapter count, replay memory
  screen    print the screen as it stood at one position
  markers   list the OSC 7373 chapters, with the position of each
  export    write the input as an asciicast v2 recording

Options:
  --cols N          screen width  (default 80; an asciicast header wins)
  --rows N          screen height (default 24; an asciicast header wins)
  --at SEQ          screen: byte position to show (default: the end)
  --micros N        screen: time position to show, in microseconds
  --title TEXT      export: title to record in the header
  -o, --output F    export: where to write (default stdout)
  -h, --help        print this and exit 0
  -V, --version     print the version and exit 0

Exit codes: 0 success, 1 the file could not be read or replayed, 2 bad usage.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Usage(message)) => {
            eprintln!("vitrum-replay: {message}");
            eprint!("\n{USAGE}");
            ExitCode::from(2)
        }
        Err(Failure::Runtime(message)) => {
            eprintln!("vitrum-replay: {message}");
            ExitCode::FAILURE
        }
        Err(Failure::Handled) => ExitCode::SUCCESS,
    }
}

/// Why the run stopped, and which exit code that is.
enum Failure {
    /// The command line was wrong. Exit 2, with the usage.
    Usage(String),
    /// The work failed. Exit 1.
    Runtime(String),
    /// `--help` or `--version` already printed. Exit 0.
    Handled,
}

/// Everything the command line asked for.
struct Options {
    command: Command,
    path: String,
    cols: u16,
    rows: u16,
    at: Option<u64>,
    micros: Option<u64>,
    title: Option<String>,
    output: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Command {
    Info,
    Screen,
    Markers,
    Export,
}

fn run(args: &[String]) -> Result<(), Failure> {
    let options = parse(args)?;
    let source = load(&options.path)?;
    let input = Input::classify(&source, &options)?;

    let chunks = [input.bytes.as_slice()];
    let stream = Stream::new(0, &chunks);
    let config = ReplayConfig::new(input.cols, input.rows)
        .map_err(|error| Failure::Runtime(error.to_string()))?;

    let mut replay =
        Replay::build(stream, &config).map_err(|error| Failure::Runtime(error.to_string()))?;
    if let Some(recorded) = input.timeline.clone() {
        // The build pass already found the OSC 7373 chapters in the bytes. A recording
        // that carries its own markers has the better list; one that does not would
        // otherwise lose the chapters by taking the recorded timeline whole.
        let markers = if recorded.markers().is_empty() {
            replay.timeline().markers().to_vec()
        } else {
            recorded.markers().to_vec()
        };
        replay.set_timeline(recorded.with_markers(markers));
    }

    let mut out = io::stdout().lock();
    match options.command {
        Command::Info => info(&mut out, &replay, &input),
        Command::Screen => screen(&mut out, &mut replay, &options),
        Command::Markers => markers(&mut out, &replay),
        Command::Export => export(&replay, &options),
    }
}

/// The bytes to replay, the geometry to replay them at, and any recorded times.
struct Input {
    bytes: Vec<u8>,
    cols: u16,
    rows: u16,
    timeline: Option<Timeline>,
    /// What the file was, for the `info` report.
    kind: &'static str,
    /// Non-zero only for an asciicast that recorded keystrokes.
    inputs: usize,
    /// Events whose type code this reader does not implement.
    skipped: usize,
    /// Resizes the recording asked for, which a replay reports and does not apply.
    resizes: Vec<(u64, u16, u16)>,
}

impl Input {
    fn classify(source: &[u8], options: &Options) -> Result<Self, Failure> {
        if source.iter().find(|byte| !byte.is_ascii_whitespace()) != Some(&b'{') {
            return Ok(Self {
                bytes: source.to_vec(),
                cols: options.cols,
                rows: options.rows,
                timeline: None,
                kind: "raw scrollback",
                inputs: 0,
                skipped: 0,
                resizes: Vec::new(),
            });
        }

        let text = core::str::from_utf8(source).map_err(|error| {
            Failure::Runtime(format!(
                "this looks like an asciicast file but it is not valid UTF-8: {error}"
            ))
        })?;
        let recording = asciicast::read(text).map_err(|error| Failure::Runtime(error.to_string()))?;
        Ok(Self {
            bytes: recording.bytes().to_vec(),
            cols: recording.header.width,
            rows: recording.header.height,
            timeline: Some(recording.timeline()),
            kind: "asciicast v2",
            inputs: recording.input_events(),
            skipped: recording.skipped_events(),
            resizes: recording
                .resizes()
                .iter()
                .map(|resize| (resize.seq, resize.cols, resize.rows))
                .collect(),
        })
    }
}

fn parse(args: &[String]) -> Result<Options, Failure> {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        print!("{USAGE}");
        return Err(Failure::Handled);
    }
    if args.iter().any(|arg| arg == "-V" || arg == "--version") {
        println!("vitrum-replay {}", env!("CARGO_PKG_VERSION"));
        return Err(Failure::Handled);
    }

    let command = match args.first().map(String::as_str) {
        Some("info") => Command::Info,
        Some("screen") => Command::Screen,
        Some("markers") => Command::Markers,
        Some("export") => Command::Export,
        Some(other) => return Err(Failure::Usage(format!("unknown command `{other}`"))),
        None => return Err(Failure::Usage(String::from("no command given"))),
    };

    let mut path = None;
    let mut cols = 80u16;
    let mut rows = 24u16;
    let mut at = None;
    let mut micros = None;
    let mut title = None;
    let mut output = None;

    let mut index = 1usize;
    while index < args.len() {
        let arg = args[index].as_str();
        // Every option below takes one value, so the fetch is the same for all of them.
        let mut value = |name: &str| -> Result<String, Failure> {
            index += 1;
            args.get(index)
                .cloned()
                .ok_or_else(|| Failure::Usage(format!("{name} needs a value")))
        };
        match arg {
            "--cols" => cols = number(&value("--cols")?, "--cols")?,
            "--rows" => rows = number(&value("--rows")?, "--rows")?,
            "--at" => at = Some(number(&value("--at")?, "--at")?),
            "--micros" => micros = Some(number(&value("--micros")?, "--micros")?),
            "--title" => title = Some(value("--title")?),
            "-o" | "--output" => output = Some(value("--output")?),
            other if other.starts_with('-') && other != "-" => {
                return Err(Failure::Usage(format!("unknown option `{other}`")));
            }
            other => {
                if path.replace(other.to_owned()).is_some() {
                    return Err(Failure::Usage(String::from("more than one file was given")));
                }
            }
        }
        index += 1;
    }

    let path = path.ok_or_else(|| Failure::Usage(String::from("no file given")))?;
    if at.is_some() && micros.is_some() {
        return Err(Failure::Usage(String::from(
            "--at and --micros name two different positions; pass one",
        )));
    }
    if command != Command::Screen && (at.is_some() || micros.is_some()) {
        return Err(Failure::Usage(format!(
            "--at and --micros only apply to `screen`, not to `{}`",
            name_of(command)
        )));
    }
    if command != Command::Export && (title.is_some() || output.is_some()) {
        return Err(Failure::Usage(format!(
            "--title and --output only apply to `export`, not to `{}`",
            name_of(command)
        )));
    }

    Ok(Options { command, path, cols, rows, at, micros, title, output })
}

const fn name_of(command: Command) -> &'static str {
    match command {
        Command::Info => "info",
        Command::Screen => "screen",
        Command::Markers => "markers",
        Command::Export => "export",
    }
}

/// Parse an unsigned option value, naming the option when it is not one.
fn number<T: core::str::FromStr>(text: &str, name: &str) -> Result<T, Failure> {
    text.parse()
        .map_err(|_| Failure::Usage(format!("{name} wants a whole number, not `{text}`")))
}

fn load(path: &str) -> Result<Vec<u8>, Failure> {
    if path == "-" {
        let mut buffer = Vec::new();
        return io::stdin()
            .read_to_end(&mut buffer)
            .map(|_| buffer)
            .map_err(|error| Failure::Runtime(format!("cannot read stdin: {error}")));
    }
    std::fs::read(path).map_err(|error| Failure::Runtime(format!("cannot read {path}: {error}")))
}

fn info<W: Write>(out: &mut W, replay: &Replay<'_>, input: &Input) -> Result<(), Failure> {
    let timeline = replay.timeline();
    let mut report = String::new();

    report.push_str(&format!("source        {}\n", input.kind));
    report.push_str(&format!("bytes         {}\n", replay.stream().len()));
    report.push_str(&format!(
        "geometry      {}x{}\n",
        replay.config().cols,
        replay.config().rows
    ));
    let micros = timeline.duration_micros();
    if !timeline.has_real_time() {
        report.push_str("duration      not recorded; scrub by byte position\n");
    } else if micros == 0 {
        // An export taken from a live session carries no clock, so every event in the
        // file is stamped zero. Printing "0.000000 s" alone would read as a bug.
        report.push_str(&format!(
            "duration      every one of the {} events is stamped zero; the source had no clock\n",
            timeline.stamps().len()
        ));
    } else {
        report.push_str(&format!(
            "duration      {}.{:06} s over {} recorded chunks\n",
            micros / 1_000_000,
            micros % 1_000_000,
            timeline.stamps().len()
        ));
    }
    report.push_str(&format!("chapters      {}\n", timeline.markers().len()));
    report.push_str(&format!("replay memory {} bytes\n", replay.heap_bytes()));
    if input.inputs > 0 {
        report.push_str(&format!(
            "keystrokes    {} input events, not replayed\n",
            input.inputs
        ));
    }
    if input.skipped > 0 {
        report.push_str(&format!(
            "unknown       {} events with an unrecognised type code\n",
            input.skipped
        ));
    }
    for (seq, cols, rows) in &input.resizes {
        report.push_str(&format!(
            "resize        at byte {seq} to {cols}x{rows}, reported not applied\n"
        ));
    }

    write_all(out, report.as_bytes())
}

fn screen<W: Write>(
    out: &mut W,
    replay: &mut Replay<'_>,
    options: &Options,
) -> Result<(), Failure> {
    if let Some(micros) = options.micros {
        if !replay.timeline().has_real_time() {
            return Err(Failure::Runtime(String::from(
                "this input recorded no times, so --micros has nothing to seek along; use --at",
            )));
        }
        replay
            .seek_micros(micros)
            .map_err(|error| Failure::Runtime(error.to_string()))?;
    } else {
        let target = options.at.unwrap_or_else(|| replay.stream().head_seq());
        replay
            .seek(target)
            .map_err(|error| Failure::Runtime(error.to_string()))?;
    }

    let mut text = replay.screen().text();
    text.push('\n');
    write_all(out, text.as_bytes())
}

fn markers<W: Write>(out: &mut W, replay: &Replay<'_>) -> Result<(), Failure> {
    let timeline = replay.timeline();
    if timeline.markers().is_empty() {
        return Err(Failure::Runtime(String::from(
            "this input carries no chapters: no OSC 7373 hints and no asciicast markers",
        )));
    }

    let mut report = String::new();
    for marker in timeline.markers() {
        let state = marker
            .hint
            .map_or_else(|| String::from("-"), |hint| format!("{hint:?}").to_lowercase());
        report.push_str(&format!("{:>12}  {:<10}  {}\n", marker.seq, state, marker.label));
    }
    write_all(out, report.as_bytes())
}

fn export(replay: &Replay<'_>, options: &Options) -> Result<(), Failure> {
    let mut header = Header::new(replay.config().cols, replay.config().rows);
    if let Some(title) = &options.title {
        header.title = Some(title.clone());
    }

    let text = asciicast::to_string(
        replay.stream(),
        replay.timeline(),
        &header,
        Utf8Policy::SurrogateEscape,
    )
    .map_err(|error| Failure::Runtime(format!("cannot encode the recording: {error}")))?;

    match &options.output {
        Some(path) => std::fs::write(path, text)
            .map_err(|error| Failure::Runtime(format!("cannot write {path}: {error}"))),
        None => write_all(&mut io::stdout().lock(), text.as_bytes()),
    }
}

/// Write and report a broken pipe as the ordinary thing it is.
fn write_all<W: Write>(out: &mut W, bytes: &[u8]) -> Result<(), Failure> {
    match out.write_all(bytes).and_then(|()| out.flush()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(Failure::Runtime(format!("cannot write output: {error}"))),
    }
}
