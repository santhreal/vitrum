//! An unterminated sequence must not hold a session off the fast path.
//!
//! [`crate::scan::OutputScan`] reads every byte every session produces. It
//! stays cheap by skipping to the next ESC or BEL with a vector search, and it
//! can only do that while nothing is in flight. While a sequence is open the
//! scan runs a branch per byte instead.
//!
//! The class this closes: a sequence that is entered and never left. A
//! terminator is one byte, so anything that drops it (a killed writer, a
//! truncated log replayed with `cat`, `printf '\e]x'`) leaves the string open
//! for every byte that session writes afterwards, for the life of the session.
//! Nothing looks wrong: the title still updates, the transcript still renders,
//! and the only symptom is that one session now costs several times what it
//! used to, permanently.
//!
//! The assertion is therefore about the scan's state and not about its output,
//! because the output is identical either way. The property is that entering a
//! sequence is bounded: after enough bytes with no terminator the scan is back
//! on the fast path, whichever way the sequence was entered.
//!
//! The entry points are derived from the `Phase` enum in the source, so a new
//! phase turns this suite red until it is listed with a byte prefix that
//! reaches it.
//!
//! What it does not catch: the abandonment point itself. A stream that emits a
//! terminator just under the bound is never abandoned and is not a defect; a
//! stream that alternates short unterminated strings with real output pays the
//! slow path for those bytes by design.

use crate::scan::OutputScan;

/// Bytes fed after the prefix, with no terminator among them.
///
/// Larger than any bound the scan may reasonably adopt, so the test states
/// "bounded" rather than pinning a constant that is free to change.
const FILLER: usize = 64 * 1024;

/// A way into a sequence, named for the phase it reaches.
struct Entry {
    phase: &'static str,
    prefix: &'static [u8],
}

/// One prefix per phase of the capture.
const ENTRIES: &[Entry] = &[
    Entry { phase: "Ground", prefix: b"" },
    Entry { phase: "Introduced", prefix: b"\x1b" },
    Entry { phase: "Ident", prefix: b"\x1b]" },
    // Two payload phases, one kept and one measured, reached by an identifier
    // the capture wants and one it does not.
    Entry { phase: "Payload", prefix: b"\x1b]2;" },
    Entry { phase: "Discard", prefix: b"\x1b]9;" },
];

/// Feed `prefix` then `FILLER` terminator-free bytes and report whether the
/// scan is still mid-sequence at the end.
fn stuck_after(prefix: &[u8]) -> bool {
    let mut scan = OutputScan::new();
    let mut hints = Vec::new();
    scan.scan(prefix, &mut hints);
    // Fed in small chunks, because a pty read lands wherever the kernel put
    // it and a bound that only holds for one big slice does not hold at all.
    let chunk = vec![b'x'; 512];
    for _ in 0..FILLER / chunk.len() {
        scan.scan(&chunk, &mut hints);
    }
    scan.mid_sequence()
}

#[test]
fn no_entry_point_stays_open_forever() {
    for entry in ENTRIES {
        assert!(
            !stuck_after(entry.prefix),
            "{}: still mid-sequence after {FILLER} bytes with no terminator, \
             so this session is off the fast path for good",
            entry.phase,
        );
    }
}

#[test]
fn the_covered_entry_points_are_every_phase() {
    let source = include_str!("../scan.rs");
    let body = source
        .split_once("enum Phase {")
        .expect("scan.rs declares enum Phase")
        .1
        .split_once('}')
        .expect("the enum is closed")
        .0;
    let declared: Vec<&str> = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("//"))
        .map(|l| l.split(['(', ',']).next().unwrap_or(l))
        .collect();
    assert!(!declared.is_empty(), "no phases parsed out of scan.rs");
    for phase in declared {
        assert!(
            ENTRIES.iter().any(|e| e.phase == phase),
            "Phase::{phase} has no entry in this suite, so nothing proves a \
             stream can leave it",
        );
    }
}

#[test]
fn a_terminated_string_still_reaches_the_fast_path() {
    // The bound must not be the only thing that closes a string: an ordinary
    // title ends on its terminator, well under the bound, and a test that
    // passed by abandoning everything would prove nothing.
    let mut scan = OutputScan::new();
    let mut hints = Vec::new();
    scan.scan(b"\x1b]2;vitrum\x07", &mut hints);
    assert!(!scan.mid_sequence(), "a complete OSC 2 left the scan mid-sequence");
    assert_eq!(scan.take_title().as_deref(), Some("vitrum"));
}

#[test]
fn abandoning_one_string_does_not_cost_the_next_title() {
    // Whatever the bound is, it is per string. A session that loses one
    // terminator keeps reporting its title afterwards.
    let mut scan = OutputScan::new();
    let mut hints = Vec::new();
    scan.scan(b"\x1b]0;", &mut hints);
    scan.scan(&vec![b'x'; FILLER], &mut hints);
    scan.scan(b"\x1b]2;after\x07", &mut hints);
    assert_eq!(scan.take_title().as_deref(), Some("after"));
}

#[test]
fn back_to_back_strings_each_get_the_whole_bound() {
    // A string may end by introducing the next one: a bare ESC terminates the
    // payload and the `]` after it opens the next string, so the stream never
    // passes through Ground. Charging the run rather than the string would
    // abandon a title that is nowhere near long enough to be suspicious.
    let mut scan = OutputScan::new();
    let mut hints = Vec::new();
    let long = "t".repeat(600);
    for _ in 0..64 {
        scan.scan(format!("\x1b]2;{long}").as_bytes(), &mut hints);
    }
    scan.scan(b"\x1b\\", &mut hints);
    assert_eq!(scan.take_title(), Some(long));
}
