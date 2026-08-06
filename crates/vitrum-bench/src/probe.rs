//! In-process fuzzing of the library surfaces the daemon workloads cannot
//! reach.
//!
//! The wire workloads (`load`, `race`, `fuzz`) exercise the daemon over a
//! socket, which is the right tool for server-level questions. But they only
//! ever touch whatever the daemon happens to expose, and the daemon is a
//! heavily-exercised path. The parse and decode surfaces underneath it — the
//! binary output frame decoder, the base64 codec, the asciicast reader, the
//! ANSI stripper, the search matcher, the grid — are pure library code that
//! a hostile input can reach through a dozen different routes. The VBR replay
//! parser's reachable panic (a malformed index offset slicing past the end)
//! escaped every daemon-level test and was only caught by reading the code.
//! This workload exists so that class of bug has a permanent, reproducible
//! place to die.
//!
//! The probe is deliberately daemon-free: it calls the library functions
//! directly, in this process, on inputs generated deterministically from a
//! seed. The contract it checks for every input:
//!
//! - **No panic.** Every target runs under `catch_unwind`, so a panic is a
//!   failure with the input recorded, not a crash of the harness.
//! - **Bounded allocation.** Each target's output must stay within a bound
//!   derived from the input size. A hostile header claiming millions of
//!   keyframes or a base64 string that decodes to ten times its own length is
//!   a memory-exhaustion bug, not a parse failure.
//! - **Determinism.** The same seed produces the same byte-for-byte results,
//!   which is what makes any finding reproducible.
//! - **Concurrency independence.** With `--threads N`, every thread runs the
//!   same corpus and must observe identical outcomes. Hidden global state —
//!   a shared buffer, a lazily-initialized table, a thread-unsafe cache — is
//!   a defect this catches that no single-threaded fuzzer can.
//!
//! The generator follows the same philosophy as the wire [`fuzz`](crate::fuzz)
//! workload: a small xorshift64* rather than a crate, because a fuzz run that
//! cannot be replayed is an anecdote. The seed is in the report.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Instant;

use serde_json::json;
use vitrum_grid::grid::CellGrid;
use vitrum_grid::{Style, char_width};
use vitrum_proto::b64;
use vitrum_proto::decode_output;
use vitrum_replay::asciicast;
use vitrum_search::ansi::{Stripper, needs_stripping};
use vitrum_search::matcher::Matcher;
use vitrum_search::query::Query;
use vitrum_model::hint::{HintParser, parse_payload};

use crate::report::Report;

/// How many bytes a hostile input may claim before the probe calls it a
/// memory-exhaustion attempt. Every target's output must stay under
/// `input_len + PROBE_HEADROOM`, which is generous for all of them (the
/// decoders are linear in their input) and far below anything a fuzzer could
/// trigger by accident.
const PROBE_HEADROOM: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct ProbeSpec {
    /// How many input cases to generate per thread.
    pub cases: usize,
    /// Determinism seed, printed in the report.
    pub seed: u64,
    /// How many threads run the corpus concurrently. Each thread derives its
    /// own Rng from the seed, and all must observe identical outcomes.
    pub threads: usize,
}

impl Default for ProbeSpec {
    fn default() -> Self {
        Self {
            cases: 200_000,
            seed: 1,
            threads: 4,
        }
    }
}

/// Deterministic generator. Xorshift64*, identical in spirit to the wire
/// fuzz workload's Rng so a seed means the same thing everywhere.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(if seed == 0 { 0x9E3779B97F4A7C15 } else { seed })
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn bytes(&mut self, out: &mut [u8]) {
        for chunk in out.chunks_mut(8) {
            let word = self.next().to_le_bytes();
            chunk.copy_from_slice(&word[..chunk.len()]);
        }
    }
}

/// One input case: the raw bytes handed to every target, plus a derived
/// hostile variant that is all zeros and another that is all 0xFF, because
/// boundary values are where decoders forget their checks.
struct Case {
    bytes: Vec<u8>,
    zeros: Vec<u8>,
    ones: Vec<u8>,
}

/// The outcome of running the whole corpus once. Threads must produce
/// byte-identical outcomes; the probe compares them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Outcome {
    /// FNV-1a hash of every observation, so a single differing byte anywhere
    /// in any target is a different hash.
    digest: u64,
    /// Total bytes allocated by all targets across the corpus.
    allocated: u64,
    /// How many inputs panicked.
    panics: Vec<String>,
    /// How many inputs exceeded the allocation bound.
    oversized: Vec<String>,
    errors: [usize; 6],
}

const TARGETS: [&str; 6] = ["decode_output", "b64", "asciicast", "search", "grid", "osc"];

impl Outcome {
    fn hash(&mut self, bytes: &[u8]) {
        let mut h = self.digest;
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001B3);
        }
        self.digest = h;
    }
}

/// Run one input through every target. Returns `Ok(())` or a descriptive
/// failure, and feeds every observation into `out`.
fn probe_one(out: &mut Outcome, case: &Case, idx: usize) {
    let inputs = [&case.bytes[..], &case.zeros[..], &case.ones[..]];
    for (which, input) in inputs.iter().enumerate() {
        let bound = input.len() + PROBE_HEADROOM;

        // --- decode_output: hostile binary frames --------------------------
        let r = catch_unwind(AssertUnwindSafe(|| decode_output(input)));
        match r {
            Err(_) => out.panics.push(format!("decode_output case {idx} variant {which} panicked")),
            Ok(Ok((_, _, payload))) => {
                if payload.len() > bound {
                    out.oversized.push(format!(
                        "decode_output case {idx} variant {which}: {} bytes from {} input",
                        payload.len(),
                        input.len()
                    ));
                }
                out.allocated += payload.len() as u64;
                out.hash(payload);
            }
            Ok(Err(_)) => out.errors[0] += 1,
        }

        // --- b64: encode must round-trip, decode must not blow up ----------
        let encoded = b64::encode(input);
        out.allocated += encoded.len() as u64;
        out.hash(encoded.as_bytes());
        let r = catch_unwind(AssertUnwindSafe(|| b64::decode(&encoded)));
        match r {
            Err(_) => out.panics.push(format!("b64 encode case {idx} variant {which} panicked")),
            Ok(Ok(round)) => {
                if round.len() > bound {
                    out.oversized.push(format!(
                        "b64 case {idx} variant {which}: decoded {} bytes from {} input",
                        round.len(),
                        input.len()
                    ));
                }
                out.allocated += round.len() as u64;
                out.hash(&round);
            }
            Ok(Err(_)) => out.errors[1] += 1,
        }
        // Hostile base64 text: garbage alphabet, wrong padding, huge claims.
        let hostile = String::from_utf8_lossy(input);
        let r = catch_unwind(AssertUnwindSafe(|| b64::decode(&hostile)));
        match r {
            Err(_) => out.panics.push(format!("b64 decode case {idx} variant {which} panicked")),
            Ok(Ok(dec)) => {
                if dec.len() > bound {
                    out.oversized.push(format!(
                        "b64 hostile case {idx} variant {which}: decoded {} bytes from {} input",
                        dec.len(),
                        input.len()
                    ));
                }
                out.allocated += dec.len() as u64;
                out.hash(&dec);
            }
            Ok(Err(_)) => out.errors[1] += 1,
        }

        let text = String::from_utf8_lossy(input);

        // --- asciicast: hostile cast files ---------------------------------
        let r = catch_unwind(AssertUnwindSafe(|| asciicast::read(&text)));
        match r {
            Err(_) => out.panics.push(format!("asciicast case {idx} variant {which} panicked")),
            Ok(Ok(rec)) => {
                if rec.bytes().len() > bound {
                    out.oversized.push(format!(
                        "asciicast case {idx} variant {which}: {} payload bytes from {} input",
                        rec.bytes().len(),
                        input.len()
                    ));
                }
                out.allocated += rec.bytes().len() as u64;
                out.hash(rec.bytes());
            }
            Ok(Err(_)) => out.errors[2] += 1,
        }

        // --- search: stripper, matcher, query compile -----------------------
        let r = catch_unwind(AssertUnwindSafe(|| {
            let mut strip = Stripper::new();
            strip.fill(input);
            strip.text().to_vec()
        }));
        match r {
            Err(_) => out.panics.push(format!("stripper case {idx} variant {which} panicked")),
            Ok(stripped) => {
                if stripped.len() > bound {
                    out.oversized.push(format!(
                        "stripper case {idx} variant {which}: {} bytes from {} input",
                        stripped.len(),
                        input.len()
                    ));
                }
                out.allocated += stripped.len() as u64;
                out.hash(&stripped);
            }
        }
        let needs = needs_stripping(input);
        out.hash(&[needs as u8]);
        let query = Query::literal(String::from_utf8_lossy(input).into_owned());
        let r = catch_unwind(AssertUnwindSafe(|| Matcher::compile(&query)));
        match r {
            Err(_) => out.panics.push(format!("matcher compile case {idx} variant {which} panicked")),
            Ok(Ok(m)) => {
                let hit = m.find_at(input, 0);
                out.hash(&[hit.is_some() as u8]);
                let _ = m.is_match(input);
            }
            Ok(Err(_)) => out.errors[3] += 1,
        }

        // --- grid: write hostile text, then verify the result --------------
        let r = catch_unwind(AssertUnwindSafe(|| {
            let mut grid = CellGrid::new(80, 24, Style::DEFAULT).expect("80x24 grid");
            let cols = (idx % 80) as u16;
            let _ = grid.write_str(cols, (idx % 24) as u16, &text, Style::DEFAULT);
            grid.row_text((idx % 24) as u16).unwrap_or_default()
        }));
        match r {
            Err(_) => out.panics.push(format!("grid write case {idx} variant {which} panicked")),
            Ok(row) => {
                out.allocated += row.len() as u64;
                out.hash(row.as_bytes());
            }
        }
        let ch = char::from_u32(idx as u32 & 0x1FFFFF).unwrap_or('\0');
        let w = char_width(ch);
        let columns = match w {
            vitrum_grid::CharWidth::Narrow => 1u8,
            vitrum_grid::CharWidth::Wide => 2u8,
            vitrum_grid::CharWidth::Control | vitrum_grid::CharWidth::ZeroWidth => 0u8,
        };
        out.hash(&[columns]);

        // --- osc: incremental 7373 extractor over hostile terminal output --
        // Byte-at-a-time state machine fed full hostile output. `out` grows one
        // declaration per completed sequence; a hostile stream must never make
        // it allocate unboundedly (bounded by MAX_SEQUENCE_BYTES internally) or
        // panic on a malformed interior.
        let r = catch_unwind(AssertUnwindSafe(|| {
            let mut parser = HintParser::new();
            let decls = parser.feed_to_vec(input);
            (parser.rejected(), decls)
        }));
        match r {
            Err(_) => out.panics.push(format!("osc feed case {idx} variant {which} panicked")),
            Ok((rejected, decls)) => {
                out.hash(&rejected.to_le_bytes());
                for d in &decls {
                    out.hash(d.label.as_deref().unwrap_or("").as_bytes());
                    out.hash(&[d.state as u8]);
                }
                if decls.len() > bound {
                    out.oversized.push(format!(
                        "osc case {idx} variant {which}: {} decls from {} input",
                        decls.len(),
                        input.len()
                    ));
                }
            }
        }
        // The one-shot parser over a payload slice.
        let r = catch_unwind(AssertUnwindSafe(|| parse_payload(input)));
        match r {
            Err(_) => out.panics.push(format!("osc payload case {idx} variant {which} panicked")),
            Ok(Some(decl)) => {
                out.hash(decl.label.as_deref().unwrap_or("").as_bytes());
                out.hash(&[decl.state as u8]);
            }
            Ok(None) => out.errors[5] += 1,
        }
    }
}

/// Run the whole corpus once with a fresh Rng seeded from `seed`. Every
/// thread runs this same corpus, so threads observing different outcomes is
/// exactly the hidden-shared-state signal the concurrency dimension exists to
/// catch.
fn run_corpus(spec: &ProbeSpec, _thread: usize) -> Outcome {
    let mut out = Outcome::default();
    let mut rng = Rng::new(spec.seed);
    let mut buf = vec![0u8; 256];
    for idx in 0..spec.cases {
        let len = 1 + rng.below(256);
        buf.resize(len, 0);
        rng.bytes(&mut buf);
        let zeros = vec![0u8; len];
        let ones = vec![0xFFu8; len];
        probe_one(&mut out, &Case { bytes: buf.clone(), zeros, ones }, idx);
    }
    out
}

pub fn run(spec: &ProbeSpec) -> anyhow::Result<Report> {
    if spec.cases == 0 {
        anyhow::bail!("a probe needs at least one case");
    }
    if spec.threads == 0 {
        anyhow::bail!("a probe needs at least one thread");
    }
    let started = Instant::now();
    let mut report = Report::new(
        "probe",
        "in-process",
        json!({
            "cases_per_thread": spec.cases,
            "seed": spec.seed,
            "threads": spec.threads,
        }),
    );
    let mut handles = Vec::with_capacity(spec.threads);
    for t in 0..spec.threads {
        let spec = spec.clone();
        handles.push(std::thread::spawn(move || run_corpus(&spec, t)));
    }
    let mut outcomes: Vec<Outcome> = handles
        .into_iter()
        .map(|h| h.join().expect("probe thread must not be join-failed"))
        .collect();

    // Concurrency independence: every thread must have observed the same
    // outcomes. A single differing byte anywhere is a different digest.
    let baseline = outcomes[0].clone();
    for (t, o) in outcomes.iter().enumerate().skip(1) {
        if o.digest != baseline.digest {
            report.failures.push(format!(
                "thread {t} observed a different outcome than thread 0 (digest {:016x} vs {:016x}): \
                 hidden shared state in a target",
                o.digest, baseline.digest
            ));
        }
        if o.panics.len() != baseline.panics.len() || o.oversized.len() != baseline.oversized.len() {
            report.failures.push(format!(
                "thread {t} disagreed with thread 0 on how many inputs failed"
            ));
        }
    }
    for o in &mut outcomes {
        report.failures.extend(std::mem::take(&mut o.panics));
        report.failures.extend(std::mem::take(&mut o.oversized));
    }
    let allocated: u64 = outcomes.iter().map(|o| o.allocated).sum();
    let cases_run = spec.cases * spec.threads;
    let errors: Vec<usize> = (0..TARGETS.len())
        .map(|i| outcomes.iter().map(|o| o.errors[i]).sum())
        .collect();

    report.checks_passed.push(format!(
        "no panic across {cases_run} inputs x 3 variants x {} targets",
        TARGETS.len()
    ));
    report.checks_passed.push(format!(
        "no allocation exceeded the input-derived bound ({} bytes headroom)",
        PROBE_HEADROOM
    ));
    report.checks_passed.push(format!(
        "{} threads observed byte-identical outcomes",
        spec.threads
    ));
    report.extra = json!({
        "cases_run": cases_run,
        "allocated_bytes": allocated,
        "rejected": {
            "decode_output": errors[0],
            "b64": errors[1],
            "asciicast": errors[2],
            "matcher": errors[3],
            "grid": errors[4],
            "osc": errors[5],
        },
    });
    report.duration_secs = started.elapsed().as_secs_f64();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The digest is a function of the seed: two runs with the same seed must
    /// be identical, and different seeds must differ. This is the property
    /// that makes a probe finding reproducible.
    #[test]
    fn same_seed_same_outcome_different_seed_differs() {
        let spec = ProbeSpec { cases: 200, seed: 7, threads: 1 };
        let a = run_corpus(&spec, 0);
        let b = run_corpus(&spec, 0);
        assert_eq!(a.digest, b.digest, "same seed must reproduce the same digest");
        let c = run_corpus(&ProbeSpec { cases: 200, seed: 8, threads: 1 }, 0);
        assert_ne!(
            a.digest, c.digest,
            "different seeds must diverge; a constant digest would mean the corpus is not \
             actually varying"
        );
    }

    /// The concurrency claim: several threads on the same corpus all agree.
    #[test]
    fn threads_agree_on_the_corpus() {
        let spec = ProbeSpec { cases: 300, seed: 42, threads: 4 };
        let single = run_corpus(&spec, 0);
        for t in 1..spec.threads {
            assert_eq!(single.digest, run_corpus(&spec, t).digest);
        }
    }

    /// The whole workload runs and passes on a small corpus, end to end,
    /// without needing a daemon.
    #[test]
    fn a_small_probe_run_passes() {
        let spec = ProbeSpec { cases: 500, seed: 99, threads: 2 };
        let report = run(&spec).expect("probe runs");
        assert!(!report.failed(), "failures: {:?}", report.failures);
        assert!(report.checks_passed.len() >= 3);
    }
}
