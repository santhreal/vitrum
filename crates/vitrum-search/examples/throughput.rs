//! Throughput benchmark: twenty sessions of ten megabytes, the real target.
//!
//! ```sh
//! cargo run --release -p vitrum-search --example throughput
//! ```
//!
//! Twenty concurrent agents is the product's stated load, and a ten-megabyte
//! ring per session is the scrollback budget. So a full cross-session query
//! sweeps 200 MB, and if a client live-searches it sweeps 200 MB per keystroke.
//! That is the number this measures.
//!
//! The corpus is not random bytes. Random data has no newlines, no escape
//! sequences and unrealistic branch behaviour, and would flatter every path
//! here. This generates output shaped like what a coding agent actually
//! produces: cargo diagnostics, coloured status lines, test output, an OSC
//! window title, stack frames, blank lines. Roughly one line in three carries
//! escape sequences, which is what forces the stripping path.
//!
//! Each buffer is handed over as two chunks, because that is what a ring gives
//! you, and the split is placed mid-line so the straddling path is exercised
//! once per session rather than never.

use std::time::{Duration, Instant};

use vitrum_search::matcher::Matcher;
use vitrum_search::{Haystack, Query, SearchResults, search_with};

const SESSIONS: usize = 20;
const BYTES_PER_SESSION: usize = 10 * 1024 * 1024;
const REPEATS: usize = 3;

fn main() {
    println!(
        "building corpus: {SESSIONS} sessions x {} MiB",
        BYTES_PER_SESSION / (1024 * 1024)
    );
    let corpus: Vec<Vec<u8>> = (0..SESSIONS)
        .map(|session| generate(session as u64, BYTES_PER_SESSION))
        .collect();

    let total_bytes: usize = corpus.iter().map(Vec::len).sum();
    let total_lines: usize = corpus
        .iter()
        .map(|body| body.iter().filter(|b| **b == b'\n').count())
        .sum();
    println!(
        "corpus: {:.1} MiB, {} lines, {:.1} bytes/line average\n",
        total_bytes as f64 / (1024.0 * 1024.0),
        total_lines,
        total_bytes as f64 / total_lines as f64
    );

    // Two chunks per session, split mid-line, exactly as a wrapped ring reads.
    let halves: Vec<[&[u8]; 2]> = corpus
        .iter()
        .map(|body| {
            let cut = body.len() / 2;
            [&body[..cut], &body[cut..]]
        })
        .collect();
    let haystacks: Vec<Haystack<'_>> = halves
        .iter()
        .enumerate()
        .map(|(index, chunks)| Haystack {
            session: index as u64,
            base_seq: 0,
            chunks: chunks.as_slice(),
        })
        .collect();

    // Every throughput case lifts both caps and asks for no context. A capped
    // query early-exits, and dividing 200 MiB by the time it took to read the
    // first two megabytes would report a throughput nobody can reproduce.
    // Context is measured separately below, because materialising a hit is a
    // different cost from scanning past a line.
    let uncapped = |query: Query| {
        query
            .context(0)
            .max_hits(usize::MAX)
            .max_hits_per_session(usize::MAX)
    };

    let cases: [(&str, Query); 6] = [
        (
            "literal, SIMD fast path",
            uncapped(Query::literal("OutOfMemory")),
        ),
        (
            "literal, case-insensitive",
            uncapped(Query::literal("outofmemory").case_insensitive(true)),
        ),
        (
            "literal, whole-word",
            uncapped(Query::literal("OutOfMemory").whole_word(true)),
        ),
        (
            "regex, alternation",
            uncapped(Query::regex(r"error|warning|fatal")),
        ),
        (
            "regex, anchored with classes",
            uncapped(Query::regex(r"^\s*Compiling [a-z-]+ v\d+\.\d+\.\d+")),
        ),
        (
            "literal, very common token",
            uncapped(Query::literal("test")),
        ),
    ];

    println!("full sweep of the whole corpus, no caps, no context");
    header();
    for (label, query) in cases {
        let (results, best) = time(&query, &haystacks);
        assert_eq!(
            results.bytes_scanned, total_bytes as u64,
            "{label} did not scan the whole corpus"
        );
        report(label, &results, best);
    }

    println!("\ncost of returning hits: same scan, two lines of context each");
    header();
    for (label, pattern) in [
        ("rare needle (144 hits)", Query::literal("OutOfMemory")),
        ("common token (659k hits)", Query::literal("test")),
    ] {
        let query = pattern
            .context(2)
            .max_hits(usize::MAX)
            .max_hits_per_session(usize::MAX);
        let (results, best) = time(&query, &haystacks);
        report(label, &results, best);
    }

    println!("\nearly exit: the caps a live search box actually uses");
    header();
    for (label, query) in [
        (
            "first 100 hits, context 2",
            Query::literal("test").context(2).max_hits(100),
        ),
        (
            "first 1000 hits, regex",
            Query::regex(r"error|warning|fatal")
                .context(2)
                .max_hits(1_000),
        ),
    ] {
        let (results, best) = time(&query, &haystacks);
        assert!(results.truncated, "{label} should have been truncated");
        report(label, &results, best);
    }

    println!("\nsingle session, 10 MiB, literal fast path");
    header();
    let single = &haystacks[..1];
    let (results, best) = time(&uncapped(Query::literal("OutOfMemory")), single);
    report("one session", &results, best);
}

fn header() {
    println!(
        "{:<32} {:>10} {:>12} {:>10} {:>12}",
        "query", "hits", "MB/s", "ms", "ns/line"
    );
    println!("{}", "-".repeat(80));
}

/// Best of [`REPEATS`] runs, after a warm-up pass.
///
/// Best rather than mean: the corpus is 200 MiB and the first touch pays page
/// faults and cache misses that belong to the harness, not to the scan.
fn time(query: &Query, haystacks: &[Haystack<'_>]) -> (SearchResults, Duration) {
    let matcher = Matcher::compile(query).expect("valid pattern");
    let mut results = search_with(&matcher, query, haystacks).expect("search");
    let mut best = Duration::MAX;
    for _ in 0..REPEATS {
        let started = Instant::now();
        results = search_with(&matcher, query, haystacks).expect("search");
        best = best.min(started.elapsed());
    }
    (results, best)
}

fn report(label: &str, results: &SearchResults, elapsed: Duration) {
    let seconds = elapsed.as_secs_f64();
    let megabytes = results.bytes_scanned as f64 / (1024.0 * 1024.0);
    println!(
        "{label:<32} {:>10} {:>12.1} {:>10.1} {:>12.1}",
        results.hits.len(),
        megabytes / seconds,
        seconds * 1000.0,
        seconds * 1e9 / results.lines_scanned as f64,
    );
}

/// Output shaped like a coding agent's, deterministic for a given session.
fn generate(session: u64, target: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(target + 256);
    let mut state = 0x2545_F491_4F6C_DD1Du64 ^ session.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut counter = 0u64;

    while out.len() < target {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        counter += 1;

        match state % 16 {
            0..=2 => {
                out.extend_from_slice(
                    b"   Compiling vitrum-search v0.1.0 (crates/vitrum-search)\n",
                );
            }
            3 | 4 => {
                out.extend_from_slice(b"\x1b[1;32m    Finished\x1b[0m `dev` profile [unoptimized + debuginfo] target(s) in 1.79s\n");
            }
            5 => {
                out.extend_from_slice(b"\x1b[1;33mwarning\x1b[0m: unused variable: `index`\n");
            }
            6 => {
                out.extend_from_slice(
                    b"\x1b[1;31merror[E0277]\x1b[0m: the trait bound is not satisfied\n",
                );
            }
            7 => out.extend_from_slice(b"\n"),
            8 => {
                out.extend_from_slice(b"\x1b]0;vitrum - session ");
                push_number(&mut out, session);
                out.extend_from_slice(b"\x07");
                out.extend_from_slice(b"prompt line follows the title\n");
            }
            9 | 10 => {
                out.extend_from_slice(
                    b"test chunks::tests::empty_chunks_are_skipped_not_terminal ... ok\n",
                );
            }
            11 => {
                out.extend_from_slice(
                    b"          at core::iter::adapters::map::Map<I,F> as core::iter::traits::iterator::Iterator>::next\n",
                );
            }
            12 => {
                out.extend_from_slice(b"\x1b[2mdebug\x1b[0m ring wrote 4096 bytes at seq ");
                push_number(&mut out, counter * 4096);
                out.push(b'\n');
            }
            13 => {
                out.extend_from_slice(
                    b"thread 'main' panicked at src/lib.rs:42:9: assertion failed\n",
                );
            }
            14 => {
                // The needle, roughly once every 1500 lines.
                if counter.is_multiple_of(1500) {
                    out.extend_from_slice(
                        b"memory allocation failed: OutOfMemory { requested: 4194304 }\n",
                    );
                } else {
                    out.extend_from_slice(
                        b"    Running unittests src/lib.rs (target/debug/deps/vitrum)\n",
                    );
                }
            }
            _ => {
                out.extend_from_slice(
                    b"info: downloaded 12 crates in 0.41s, resolving dependencies\n",
                );
            }
        }
    }
    out
}

fn push_number(out: &mut Vec<u8>, mut value: u64) {
    if value == 0 {
        out.push(b'0');
        return;
    }
    let start = out.len();
    while value > 0 {
        out.push(b'0' + (value % 10) as u8);
        value /= 10;
    }
    out[start..].reverse();
}
