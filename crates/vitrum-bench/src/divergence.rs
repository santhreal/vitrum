//! Differential and concurrent fuzzing of the parser and the grid, with a
//! committed artefact for anything it finds.
//!
//! Two failure classes live here, and they have the same cure: a failing input
//! is worthless as a sentence in a report and load-bearing as a file a test can
//! replay. An intermittent failure with no artefact is not a finding.
//!
//! # Chunking divergence
//!
//! The same bytes reach a screen along two paths. Live, the daemon hands over
//! whatever a `read` returned, so the engine is fed in arbitrary pieces and the
//! grid is synced after each one. Replayed, a whole recording is fed at once
//! and synced once at the end. Those two must produce the same screen, because
//! that claim is what lets one parser serve both; a stream's meaning cannot
//! depend on where a reader's buffer happened to end.
//!
//! [`chunking`] feeds one input both ways and compares the resulting screens
//! cell for cell. Splits fall anywhere, including through a UTF-8 sequence and
//! through an escape sequence, which is where a resumable parser is either
//! resumable or not.
//!
//! # Schedule divergence
//!
//! A session belongs to the thread that drives it, and a window drives several
//! at once. So the question is whether a grid built while other threads are
//! building theirs is the grid that input produces alone. Anything shared and
//! unnoticed — a lazily-built table, a scratch buffer, a thread-unsafe cache in
//! the engine — shows up as a screen that depends on what other threads were
//! doing.
//!
//! [`schedules`] runs that under a deterministic scheduler rather than hoping
//! to catch it free-running: threads take steps in an order drawn from the
//! seed, one at a time, so a failing interleaving is a list of integers that
//! replays exactly. Free-running is also tried, because a schedule search can
//! only explore orderings it can enforce, and what it finds there is reported
//! as a signal without an artefact rather than as a result.
//!
//! What neither covers: an instruction-level data race between two threads
//! genuinely inside the same function at the same instant. Step-granular
//! interleaving cannot express that, and this says so rather than implying a
//! clean run rules it out.
//!
//! # Artefacts
//!
//! Anything found is minimised and written to `crates/vitrum-bench/artifacts/`
//! as JSON, and the corpus there is replayed by this crate's own suite on every
//! run. Each artefact carries a status: `open` must still reproduce, `fixed`
//! must not. Both directions are checked, so neither a fix that is never
//! recorded nor a regression that undoes one can pass in silence.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;
use vitrum_grid::cell::{Cell, Cursor};
use vitrum_grid::{CellGrid, Style};
use vitrum_vt::{Vt, VtOptions};

use crate::report::Report;
use crate::rng::Rng;

/// Grid every differential case runs at. Small on purpose: a divergence is a
/// cell, and a smaller screen makes the minimised artefact smaller too.
const COLS: u16 = 80;
const ROWS: u16 = 24;

/// Scrollback for the cases. Enough that a scroll leaves history, small enough
/// that a hostile input cannot make the corpus run out of memory.
const SCROLLBACK: usize = 64 * 1024;

/// Where the committed corpus lives, relative to the workspace root.
pub const CORPUS_DIR: &str = "crates/vitrum-bench/artifacts";

/// What a run was asked to do.
#[derive(Debug, Clone)]
pub struct DivergenceSpec {
    /// Differential cases: one input fed both ways.
    pub cases: usize,
    /// Schedules explored, each over its own inputs.
    pub schedules: usize,
    /// Threads in a scheduled run.
    pub threads: usize,
    /// Seed for every generator in the run.
    pub seed: u64,
    /// Where to write artefacts. The committed corpus by default.
    pub corpus: PathBuf,
}

impl Default for DivergenceSpec {
    fn default() -> Self {
        Self {
            cases: 20_000,
            schedules: 2_000,
            threads: 4,
            seed: 1,
            corpus: PathBuf::from(CORPUS_DIR),
        }
    }
}

// ---------------------------------------------------------------------------
// Screens
// ---------------------------------------------------------------------------

/// A screen as the two paths must agree on it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Screen {
    cols: u16,
    rows: u16,
    cells: Vec<Cell>,
    cursor: Option<Cursor>,
}

impl Screen {
    fn of(grid: &CellGrid) -> Self {
        let mut cells = Vec::with_capacity(grid.len());
        for row in 0..grid.rows() {
            cells.extend_from_slice(grid.row(row).unwrap_or(&[]));
        }
        Screen {
            cols: grid.cols(),
            rows: grid.rows(),
            cells,
            cursor: grid.cursor(),
        }
    }

    /// How the two differ, named precisely enough to start debugging from, or
    /// `None` when they agree.
    fn differs(&self, other: &Screen) -> Option<String> {
        if self.cols != other.cols || self.rows != other.rows {
            return Some(format!(
                "geometry {}x{} against {}x{}",
                self.cols, self.rows, other.cols, other.rows
            ));
        }
        if self.cursor != other.cursor {
            return Some(format!(
                "cursor {:?} against {:?}",
                self.cursor, other.cursor
            ));
        }
        for (i, (a, b)) in self.cells.iter().zip(&other.cells).enumerate() {
            if a != b {
                let col = i % self.cols.max(1) as usize;
                let row = i / self.cols.max(1) as usize;
                return Some(format!("cell ({col}, {row}) is {a:?} against {b:?}"));
            }
        }
        None
    }
}

fn engine() -> anyhow::Result<(Vt, CellGrid)> {
    let vt = Vt::new(VtOptions {
        cols: COLS,
        rows: ROWS,
        max_scrollback: SCROLLBACK,
    })
    .map_err(|e| anyhow::anyhow!("building the terminal engine: {e}"))?;
    let grid = CellGrid::new(COLS, ROWS, Style::DEFAULT)
        .map_err(|e| anyhow::anyhow!("building the cell grid: {e}"))?;
    Ok((vt, grid))
}

/// Feed `input` in one call and sync once: the replay path.
fn whole(input: &[u8]) -> anyhow::Result<Screen> {
    let (mut vt, mut grid) = engine()?;
    vt.feed(input);
    vt.sync(&mut grid)
        .map_err(|e| anyhow::anyhow!("syncing after a whole feed: {e}"))?;
    Ok(Screen::of(&grid))
}

/// Feed `input` in the pieces `splits` names, syncing after each: the live path.
///
/// `splits` are byte offsets, not lengths, so a minimiser can drop one without
/// having to fix up the rest.
fn chunked(input: &[u8], splits: &[usize]) -> anyhow::Result<Screen> {
    let (mut vt, mut grid) = engine()?;
    let mut at = 0usize;
    for &split in splits {
        let end = split.min(input.len());
        if end > at {
            vt.feed(&input[at..end]);
            vt.sync(&mut grid)
                .map_err(|e| anyhow::anyhow!("syncing after a chunk: {e}"))?;
            at = end;
        }
    }
    if at < input.len() {
        vt.feed(&input[at..]);
    }
    vt.sync(&mut grid)
        .map_err(|e| anyhow::anyhow!("syncing after the last chunk: {e}"))?;
    Ok(Screen::of(&grid))
}

/// Feed one input whole, another in pieces, and say how the screens differ.
///
/// Two inputs rather than one so the detector itself can be gated: called with
/// the same bytes twice it is the invariant [`chunking`] checks, and called
/// with bytes that differ by one it must report a divergence. A comparison
/// that cannot fail on a screen that really is different would make every
/// clean run of this workload meaningless.
pub fn differential(
    whole_input: &[u8],
    chunk_input: &[u8],
    splits: &[usize],
) -> anyhow::Result<Option<String>> {
    let a = whole(whole_input)?;
    let b = chunked(chunk_input, splits)?;
    Ok(a.differs(&b))
}

/// Whether this input and split diverge, and how.
pub fn chunking(input: &[u8], splits: &[usize]) -> anyhow::Result<Option<String>> {
    differential(input, input, splits)
}

// ---------------------------------------------------------------------------
// Corpus generation
// ---------------------------------------------------------------------------

/// Sequence shapes the generator draws from.
///
/// Each is a place a resumable parser has state to carry across a chunk
/// boundary: a partial UTF-8 sequence, a partial CSI, an unterminated OSC, a
/// scroll region that changes what a later newline means.
const SHAPES: [&str; 18] = [
    "\x1b[H",
    "\x1b[2J",
    "\x1b[K",
    "\x1b[1;1H",
    "\x1b[10;30H",
    "\x1b[38;5;200m",
    "\x1b[38;2;10;200;30m",
    "\x1b[0m",
    "\x1b[1m",
    "\x1b[4m",
    "\x1b[7m",
    "\x1b[2;20r",
    "\x1b[?1049h",
    "\x1b[?1049l",
    "\x1b[?25l",
    "\x1bM",
    "\x1bD",
    "\x1b]0;a title\x07",
];

/// Multi-byte and multi-cell text, which is where a split through a character
/// is either handled or not.
const TEXT: [&str; 10] = [
    "hello",
    "ambiguous ~",
    "日本語のテキスト",
    "e\u{0301}composed",
    "🙂🙃",
    "\u{200b}zero width",
    "tabs\there",
    "box ─┼─",
    "combining a\u{0300}\u{0301}",
    "mixed 日a本b",
];

/// One generated input.
fn case(rng: &mut Rng) -> Vec<u8> {
    let mut out = Vec::with_capacity(256);
    let parts = 1 + rng.below(24);
    for _ in 0..parts {
        match rng.below(10) {
            0..=3 => out.extend_from_slice(rng.pick(&TEXT).as_bytes()),
            4..=7 => out.extend_from_slice(rng.pick(&SHAPES).as_bytes()),
            8 => {
                out.extend_from_slice(b"\r\n");
            }
            _ => {
                // Raw bytes, including invalid UTF-8 and stray escapes. A
                // parser that only survives well-formed input is a parser one
                // corrupt read away from a wrong screen.
                let n = 1 + rng.below(8);
                for _ in 0..n {
                    out.push((rng.next() & 0xFF) as u8);
                }
            }
        }
    }
    out
}

/// Byte offsets to split `input` at, in increasing order.
fn splits_for(input: &[u8], rng: &mut Rng) -> Vec<usize> {
    if input.len() < 2 {
        return Vec::new();
    }
    let count = 1 + rng.below(6);
    let mut splits: Vec<usize> = (0..count).map(|_| 1 + rng.below(input.len() - 1)).collect();
    splits.sort_unstable();
    splits.dedup();
    splits
}

// ---------------------------------------------------------------------------
// Minimisation
// ---------------------------------------------------------------------------

/// Shrink a failing case while it keeps failing.
///
/// Delta debugging on two axes: drop a run of bytes, then drop a split. Bytes
/// first, because a shorter input usually makes several splits redundant, and
/// the loop repeats until a pass changes nothing. The result is what gets
/// committed, so it is worth the passes: an artefact nobody can read is an
/// artefact nobody fixes.
fn minimise(input: &[u8], splits: &[usize]) -> anyhow::Result<(Vec<u8>, Vec<usize>)> {
    let mut input = input.to_vec();
    let mut splits = splits.to_vec();
    let mut progress = true;
    while progress {
        progress = false;

        let mut window = input.len() / 2;
        while window >= 1 {
            let mut at = 0;
            while at < input.len() {
                let end = (at + window).min(input.len());
                let mut candidate = input.clone();
                candidate.drain(at..end);
                let cut = end - at;
                let trimmed: Vec<usize> = splits
                    .iter()
                    .filter_map(|&s| {
                        if s <= at {
                            Some(s)
                        } else if s >= end {
                            Some(s - cut)
                        } else {
                            None
                        }
                    })
                    .filter(|&s| s > 0 && s < candidate.len())
                    .collect();
                if !candidate.is_empty() && chunking(&candidate, &trimmed)?.is_some() {
                    input = candidate;
                    splits = trimmed;
                    progress = true;
                } else {
                    at = end;
                }
            }
            window /= 2;
        }

        let mut i = 0;
        while i < splits.len() {
            let mut candidate = splits.clone();
            candidate.remove(i);
            if chunking(&input, &candidate)?.is_some() {
                splits = candidate;
                progress = true;
            } else {
                i += 1;
            }
        }
    }
    Ok((input, splits))
}

// ---------------------------------------------------------------------------
// Schedules
// ---------------------------------------------------------------------------

/// A turn-taking gate: thread `steps[i]` may take its `i`th step and no other
/// thread may move until it has.
///
/// This is what makes an interleaving a value rather than an accident. A
/// free-running failure cannot be replayed; a scheduled one is a list of
/// integers that reproduces on any machine.
struct Baton {
    steps: Vec<usize>,
    at: Mutex<usize>,
    bell: Condvar,
}

impl Baton {
    fn new(steps: Vec<usize>) -> Self {
        Self {
            steps,
            at: Mutex::new(0),
            bell: Condvar::new(),
        }
    }

    /// Block until it is `thread`'s turn. `false` once the schedule is spent.
    fn wait(&self, thread: usize) -> bool {
        let mut at = self.at.lock().expect("baton mutex poisoned");
        loop {
            if *at >= self.steps.len() {
                return false;
            }
            if self.steps[*at] == thread {
                return true;
            }
            at = self.bell.wait(at).expect("baton mutex poisoned");
        }
    }

    /// Hand the turn on.
    fn pass(&self) {
        let mut at = self.at.lock().expect("baton mutex poisoned");
        *at += 1;
        self.bell.notify_all();
    }
}

/// One thread's work, as a list of chunks it feeds one per step.
type Work = Vec<Vec<u8>>;

/// The screen each thread's input produces when nothing else is running.
fn alone(work: &Work) -> anyhow::Result<Screen> {
    let (mut vt, mut grid) = engine()?;
    for chunk in work {
        vt.feed(chunk);
        vt.sync(&mut grid)
            .map_err(|e| anyhow::anyhow!("syncing a solo step: {e}"))?;
    }
    Ok(Screen::of(&grid))
}

/// Run `work` concurrently, under `steps` when it is given and free when it is
/// not, and return the first thread whose screen is not the one it produces
/// alone.
pub(crate) fn concurrent(work: &[Work], steps: Option<&[usize]>) -> anyhow::Result<Option<String>> {
    let expected: Vec<Screen> = work.iter().map(alone).collect::<anyhow::Result<_>>()?;
    let baton = steps.map(|s| Arc::new(Baton::new(s.to_vec())));
    let mut got: Vec<Option<Screen>> = vec![None; work.len()];

    std::thread::scope(|scope| -> anyhow::Result<()> {
        let mut handles = Vec::with_capacity(work.len());
        for (t, w) in work.iter().enumerate() {
            let baton = baton.clone();
            handles.push(scope.spawn(move || -> anyhow::Result<Screen> {
                let (mut vt, mut grid) = engine()?;
                for chunk in w {
                    if let Some(b) = &baton {
                        if !b.wait(t) {
                            break;
                        }
                    }
                    vt.feed(chunk);
                    vt.sync(&mut grid)
                        .map_err(|e| anyhow::anyhow!("syncing a concurrent step: {e}"))?;
                    if let Some(b) = &baton {
                        b.pass();
                    }
                }
                Ok(Screen::of(&grid))
            }));
        }
        for (t, h) in handles.into_iter().enumerate() {
            got[t] = Some(
                h.join()
                    .map_err(|_| anyhow::anyhow!("thread {t} panicked during a scheduled run"))??,
            );
        }
        Ok(())
    })?;

    for (t, (want, have)) in expected.iter().zip(&got).enumerate() {
        let Some(have) = have else { continue };
        if let Some(how) = want.differs(have) {
            return Ok(Some(format!("thread {t}: {how}")));
        }
    }
    Ok(None)
}

/// A schedule that visits every thread's every step, in a seeded order.
fn schedule_for(work: &[Work], rng: &mut Rng) -> Vec<usize> {
    let mut left: Vec<usize> = work.iter().map(|w| w.len()).collect();
    let total: usize = left.iter().sum();
    let mut steps = Vec::with_capacity(total);
    for _ in 0..total {
        // Pick among threads that still have a step, so the schedule is always
        // complete and a replay never deadlocks on a thread with nothing left.
        let ready: Vec<usize> = left
            .iter()
            .enumerate()
            .filter(|(_, n)| **n > 0)
            .map(|(t, _)| t)
            .collect();
        let t = *rng.pick(&ready);
        left[t] -= 1;
        steps.push(t);
    }
    steps
}

fn work_for(threads: usize, rng: &mut Rng) -> Vec<Work> {
    (0..threads)
        .map(|_| {
            let steps = 2 + rng.below(6);
            (0..steps).map(|_| case(rng)).collect()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Artefacts
// ---------------------------------------------------------------------------

/// A committed, replayable failure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Artifact {
    /// The same bytes produced different screens depending on where they were
    /// split.
    Chunking {
        /// `open` while it still reproduces, `fixed` once it must not.
        status: Status,
        /// What was seen when it was captured.
        note: String,
        /// The minimised input, as hex.
        input_hex: String,
        /// Byte offsets it is split at.
        splits: Vec<usize>,
    },
    /// A thread's screen depended on what other threads were doing.
    Schedule {
        /// `open` while it still reproduces, `fixed` once it must not.
        status: Status,
        /// What was seen when it was captured.
        note: String,
        /// Each thread's chunks, as hex.
        work_hex: Vec<Vec<String>>,
        /// The interleaving, as thread indices.
        steps: Vec<usize>,
    },
}

/// Whether an artefact is expected to still reproduce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// The defect is live. Replaying it must still diverge; when it stops, the
    /// fix is real and the status is what records it.
    Open,
    /// The defect is fixed. Replaying it must not diverge, which is what keeps
    /// the fix from being undone.
    Fixed,
}

impl Artifact {
    /// The status this artefact claims.
    #[must_use]
    pub const fn status(&self) -> Status {
        match self {
            Artifact::Chunking { status, .. } | Artifact::Schedule { status, .. } => *status,
        }
    }

    /// Run it again. `Some` describes the divergence, `None` means it did not
    /// reproduce.
    pub fn replay(&self) -> anyhow::Result<Option<String>> {
        match self {
            Artifact::Chunking {
                input_hex, splits, ..
            } => chunking(&from_hex(input_hex)?, splits),
            Artifact::Schedule {
                work_hex, steps, ..
            } => {
                let work: Vec<Work> = work_hex
                    .iter()
                    .map(|t| t.iter().map(|c| from_hex(c)).collect())
                    .collect::<anyhow::Result<_>>()?;
                concurrent(&work, Some(steps))
            }
        }
    }

    /// A stable file name: the kind and a digest of the payload, so the same
    /// failure found twice is one file rather than two.
    #[must_use]
    pub fn file_name(&self) -> String {
        let (kind, payload) = match self {
            Artifact::Chunking {
                input_hex, splits, ..
            } => ("chunking", format!("{input_hex}:{splits:?}")),
            Artifact::Schedule {
                work_hex, steps, ..
            } => ("schedule", format!("{work_hex:?}:{steps:?}")),
        };
        format!("{kind}-{:016x}.json", digest(payload.as_bytes()))
    }
}

/// FNV-1a over the payload. Not a security digest: this only has to name a
/// file the same way twice.
fn digest(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn from_hex(s: &str) -> anyhow::Result<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        bail!("hex payload has an odd length");
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16).with_context(|| format!("hex at byte {}", i / 2))
        })
        .collect()
}

/// Every artefact committed under `dir`.
///
/// # Errors
///
/// A file that is not a readable artefact, which is a corpus that has rotted
/// rather than a corpus that is empty.
pub fn corpus(dir: &Path) -> anyhow::Result<Vec<(PathBuf, Artifact)>> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let artifact: Artifact = serde_json::from_str(&text)
            .with_context(|| format!("parsing {} as an artefact", path.display()))?;
        out.push((path, artifact));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn commit(dir: &Path, artifact: &Artifact) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(artifact.file_name());
    let mut text = serde_json::to_string_pretty(artifact)?;
    text.push('\n');
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------

/// Explore both classes, minimise anything found, and commit it.
pub fn run(spec: &DivergenceSpec) -> anyhow::Result<Report> {
    let mut report = Report::new(
        "divergence",
        "in-process",
        json!({
            "cases": spec.cases,
            "schedules": spec.schedules,
            "threads": spec.threads,
            "seed": spec.seed,
            "corpus": spec.corpus.display().to_string(),
        }),
    );
    if spec.threads < 2 {
        bail!("a schedule needs at least two threads");
    }
    let started = Instant::now();

    // The committed corpus runs first, every time. A corpus that is only
    // replayed by the test suite is a corpus that rots between test runs.
    let mut corpus_checked = 0usize;
    for (path, artifact) in corpus(&spec.corpus)? {
        corpus_checked += 1;
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        match (artifact.status(), artifact.replay()?) {
            (Status::Open, Some(_)) | (Status::Fixed, None) => {}
            (Status::Open, None) => report.failures.push(format!(
                "artefact {name} is recorded as open but no longer reproduces: the defect is \
                 fixed and the artefact's status has to say so"
            )),
            (Status::Fixed, Some(how)) => report.failures.push(format!(
                "artefact {name} is recorded as fixed but diverged again: {how}"
            )),
        }
    }

    // Differential: one input, two paths.
    let mut rng = Rng::new(spec.seed);
    let mut chunk_bytes = 0u64;
    let mut found = Vec::new();
    let chunk_started = Instant::now();
    for _ in 0..spec.cases {
        let input = case(&mut rng);
        let splits = splits_for(&input, &mut rng);
        chunk_bytes += input.len() as u64;
        if let Some(how) = chunking(&input, &splits)? {
            let (small_input, small_splits) = minimise(&input, &splits)?;
            let artifact = Artifact::Chunking {
                status: Status::Open,
                note: how.clone(),
                input_hex: to_hex(&small_input),
                splits: small_splits,
            };
            let path = commit(&spec.corpus, &artifact)?;
            report.failures.push(format!(
                "chunking divergence: {how} (minimised to {} bytes, committed as {})",
                small_input.len(),
                path.display()
            ));
            found.push(path.display().to_string());
        }
    }
    let chunk_secs = chunk_started.elapsed().as_secs_f64();

    // Concurrent: many threads, one deterministic order at a time.
    let sched_started = Instant::now();
    let mut free_divergences = 0usize;
    for _ in 0..spec.schedules {
        let work = work_for(spec.threads, &mut rng);
        let steps = schedule_for(&work, &mut rng);
        if let Some(how) = concurrent(&work, Some(&steps))? {
            let artifact = Artifact::Schedule {
                status: Status::Open,
                note: how.clone(),
                work_hex: work
                    .iter()
                    .map(|t| t.iter().map(|c| to_hex(c)).collect())
                    .collect(),
                steps: steps.clone(),
            };
            let path = commit(&spec.corpus, &artifact)?;
            report.failures.push(format!(
                "schedule divergence: {how} (committed as {})",
                path.display()
            ));
            found.push(path.display().to_string());
        }
        // Free running explores orderings the baton cannot express. A
        // divergence here is reported without an artefact, because a schedule
        // nobody recorded is not replayable and saying otherwise would be a
        // lie in a file.
        if concurrent(&work, None)?.is_some() {
            free_divergences += 1;
        }
    }
    let sched_secs = sched_started.elapsed().as_secs_f64();

    if free_divergences > 0 {
        report.failures.push(format!(
            "{free_divergences} free-running interleavings produced a screen the same input \
             produces differently alone, and no baton order reproduced them; the schedule search \
             cannot express instruction-level interleaving"
        ));
    }

    if report.failures.is_empty() {
        report.checks_passed.push(format!(
            "{} inputs totalling {chunk_bytes} bytes produced the same screen fed whole and fed \
             in pieces, splits falling inside UTF-8 sequences and inside escape sequences",
            spec.cases
        ));
        report.checks_passed.push(format!(
            "{} interleavings over {} threads each produced the screen its input produces alone, \
             and {} free-running runs of the same work agreed",
            spec.schedules, spec.threads, spec.schedules
        ));
        if corpus_checked > 0 {
            report.checks_passed.push(format!(
                "{corpus_checked} committed artefacts replayed to the status recorded for them"
            ));
        }
    }

    report.duration_secs = started.elapsed().as_secs_f64();
    report.extra = json!({
        "corpus_checked": corpus_checked,
        "artifacts_written": found,
        "input_space": {
            "text_shapes": TEXT.len(),
            "escape_shapes": SHAPES.len(),
            "parts_per_case": "1..=24",
            "splits_per_case": "1..=6, uniform over byte offsets",
            "steps_per_thread": "2..=7",
            "bytes_fed": chunk_bytes,
        },
        "wall_seconds": {
            "chunking": chunk_secs,
            "schedules": sched_secs,
        },
    });
    Ok(report)
}
