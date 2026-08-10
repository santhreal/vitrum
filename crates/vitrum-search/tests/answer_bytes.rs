//! The size of an answer is bounded, not just the number of rows in it.
//!
//! # The class
//!
//! Every cap this crate had counted rows: `max_hits`, `max_hits_per_session`,
//! `MAX_CONTEXT`. None of them bounded what a row weighs, and a row weighs
//! whatever the session wrote. A client picks the pattern, the hit cap and the
//! context depth; the session picks the line length; the product of the four
//! is the heap. Measured against a 10 MiB ring of one-kilobyte lines with a
//! pattern matching every line, ten thousand hits at sixty-four lines of
//! context each side came to 1.27 GB of `Vec<u8>` — and the daemon then
//! projects that onto the wire, which copies it again.
//!
//! The class is "a cap on the count of something whose size the input
//! chooses". It is closed by pricing the answer in bytes at the one place
//! every hit and every context line is appended, and by checking the returned
//! `SearchResults` of every public entry point rather than of one of them.
//!
//! # What this does not catch
//!
//! - Peak memory inside the parallel path. Each worker enforces the budget
//!   over its own partition, so the concatenation before truncation may hold
//!   one budget per worker. Only the returned answer is asserted.
//! - The stripper's per-line scratch, the ring itself, and anything a caller
//!   does with the answer after it is handed over.
//! - A single line longer than the whole budget. Such a hit is refused
//!   outright and the search reports `truncated` with nothing in it, which is
//!   correct for the bound and poor as an answer.

use vitrum_search::{
    DEFAULT_MAX_ANSWER_BYTES, Haystack, MAX_CONTEXT, Matcher, Query, SearchResults, Sweep, search,
    search_parallel, search_with, search_with_parallel,
};

/// One session's worth of kilobyte lines, every one of which matches `E`.
fn corpus(bytes: usize) -> Vec<u8> {
    let mut line = vec![b'x'; 1023];
    line[0] = b'E';
    let mut out = Vec::with_capacity(bytes + 1024);
    while out.len() < bytes {
        out.extend_from_slice(&line);
        out.push(b'\n');
    }
    out
}

/// The worst shape there is: every cap turned up as far as it goes.
fn greedy(budget: usize) -> Query {
    Query::literal("E")
        .context(MAX_CONTEXT)
        .max_hits(10_000)
        .max_hits_per_session(10_000)
        .max_answer_bytes(budget)
}

const BUDGET: usize = 512 * 1024;

/// Every public entry point that produces a [`SearchResults`], bounded.
///
/// The assertion is on the returned value, not on any internal counter, so it
/// holds however the scan is arranged inside. Both the serial and the
/// threaded arrangement are covered: `search` picks the threaded one once the
/// corpus passes its two-megabyte threshold, which four three-megabyte
/// sessions do.
#[test]
fn no_entry_point_returns_more_line_text_than_the_budget() {
    let body = corpus(3 * 1024 * 1024);
    let chunks: [&[u8]; 1] = [&body];
    let stacks: Vec<Haystack<'_>> = (1..=4u64)
        .map(|session| Haystack {
            session,
            base_seq: 0,
            chunks: &chunks,
        })
        .collect();
    let query = greedy(BUDGET);
    let matcher = Matcher::compile(&query).expect("a literal always compiles");

    let mut sweep = Sweep::with_matcher(&matcher, &query);
    for stack in &stacks {
        if !sweep.push(stack) {
            break;
        }
    }

    let answers: [(&str, SearchResults); 5] = [
        ("search", search(&query, &stacks).expect("valid pattern")),
        (
            "search_with",
            search_with(&matcher, &query, &stacks).expect("valid pattern"),
        ),
        (
            "search_parallel",
            search_parallel(&query, &stacks).expect("valid pattern"),
        ),
        (
            "search_with_parallel",
            search_with_parallel(&matcher, &query, &stacks).expect("valid pattern"),
        ),
        ("finish", sweep.finish()),
    ];

    for (name, results) in &answers {
        assert!(
            results.answer_bytes() <= BUDGET,
            "{name} returned {} bytes of line text against a {BUDGET} byte budget",
            results.answer_bytes()
        );
        assert!(
            results.truncated,
            "{name} spent the whole budget and did not say the answer was cut"
        );
        assert!(
            !results.is_empty(),
            "{name} returned nothing at all, so the bound is being met by refusing to answer"
        );
    }
}

/// A new public entry point must be added to the test above or turn this red.
///
/// The list of things that can hand a caller a `SearchResults` is the variant
/// space here, and it is read out of the module rather than written down, so
/// adding a fifth way to run a search fails until somebody decides whether it
/// is bounded.
#[test]
fn the_covered_entry_points_are_every_entry_point() {
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/search.rs"))
        .expect("the search module is readable from its own crate");
    // A signature may wrap over several lines, so each one is gathered up to
    // its opening brace before being read. Matching a single line found three
    // of the five and would have declared the space closed.
    let mut found: Vec<String> = Vec::new();
    let mut lines = source.lines().map(str::trim);
    while let Some(line) = lines.next() {
        let Some(rest) = line.strip_prefix("pub fn ") else {
            continue;
        };
        let name = rest
            .split(['(', '<'])
            .next()
            .expect("a split always yields one part")
            .to_string();
        let mut signature = line.to_string();
        while !signature.contains('{') {
            match lines.next() {
                Some(more) => signature.push_str(more),
                None => break,
            }
        }
        let Some((head, _)) = signature.split_once('{') else {
            continue;
        };
        if head.contains("SearchResults") {
            found.push(name);
        }
    }
    found.sort();
    found.dedup();

    // `new` and `with_matcher` build a searcher; they do not answer. Every
    // remaining name is a way to obtain results and is asserted above.
    let covered = [
        "finish",
        "search",
        "search_parallel",
        "search_with",
        "search_with_parallel",
    ];
    assert_eq!(
        found,
        covered,
        "the set of functions returning SearchResults changed; bound the new one \
         in no_entry_point_returns_more_line_text_than_the_budget, then list it here"
    );
}

/// A query nobody configured is already bounded.
///
/// The daemon builds its query from wire fields and sets no byte cap, so the
/// default is the one that runs in production.
#[test]
fn the_default_query_carries_the_byte_cap() {
    assert_eq!(Query::literal("x").max_answer_bytes, DEFAULT_MAX_ANSWER_BYTES);
    assert_eq!(Query::regex("x").max_answer_bytes, DEFAULT_MAX_ANSWER_BYTES);
    assert!(
        DEFAULT_MAX_ANSWER_BYTES < 10 * 1024 * 1024,
        "the default must be smaller than one session's ring, or it bounds nothing"
    );
}

/// An ordinary search must not notice the cap.
///
/// A bound that changes the everyday answer is a regression dressed as a fix,
/// so the shape the shipped window sends is asserted to come back whole.
#[test]
fn an_ordinary_search_is_returned_intact() {
    let body = corpus(64 * 1024);
    let chunks: [&[u8]; 1] = [&body];
    let stack = Haystack {
        session: 1,
        base_seq: 0,
        chunks: &chunks,
    };
    let query = Query::literal("E").context(2).max_hits(500);
    let results = search(&query, std::slice::from_ref(&stack)).expect("valid pattern");
    assert_eq!(results.len(), 64, "every line of a 64 KiB corpus matches");
    assert!(!results.truncated, "a 64 KiB answer is nowhere near the cap");
    assert!(results.answer_bytes() < DEFAULT_MAX_ANSWER_BYTES);
}

/// Spending the budget on context must stop the sweep, not merely stop the
/// hits.
///
/// After-context is attached lines after a hit was recorded, so a budget
/// enforced only where hits are pushed would keep growing the answer for every
/// hit already in flight. The corpus is one enormous hit line followed by
/// context lines, sized so the hit fits and its context does not.
#[test]
fn after_context_cannot_spend_past_the_budget() {
    let mut body = Vec::new();
    body.extend_from_slice(b"E");
    body.push(b'\n');
    for _ in 0..MAX_CONTEXT * 4 {
        body.extend_from_slice(&vec![b'y'; 4096]);
        body.push(b'\n');
    }
    let chunks: [&[u8]; 1] = [&body];
    let stack = Haystack {
        session: 1,
        base_seq: 0,
        chunks: &chunks,
    };
    let budget = 8 * 1024;
    let query = greedy(budget);
    let results = search(&query, std::slice::from_ref(&stack)).expect("valid pattern");
    assert!(
        results.answer_bytes() <= budget,
        "after-context grew the answer to {} bytes against a {budget} byte budget",
        results.answer_bytes()
    );
    assert!(results.truncated);
}
