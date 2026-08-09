//! The scrollback corpus both integration suites scan.
//!
//! One generator, because the two suites compare against each other: the
//! allocation proof and the offset proof are claims about scanning *the same*
//! bytes, and a corpus that drifted between them would leave the allocation
//! bound pinned on a shape the correctness suite no longer covers.
//!
//! A directory module rather than a sibling file, because cargo compiles every
//! top-level `tests/*.rs` as its own test binary and `tests/corpus.rs` would
//! become an empty one.

/// Scrollback with the awkward shapes mixed in, deterministic for a line count.
///
/// Colour on one line in three, an OSC title, a blank line, a very long line.
/// A corpus of plain ASCII would exercise neither the stripping path nor the
/// straddle path and would prove nothing about either. Colour matters twice
/// over for the allocation proof: a coloured line takes the stripping path,
/// which has its own scratch buffers, and those are exactly what a naive
/// implementation would reallocate every line.
pub fn mixed_scrollback(lines: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(lines * 72);
    for index in 0..lines {
        match index % 8 {
            0 => {
                out.extend_from_slice(b"   Compiling vitrum-search v0.1.0 (crates/vitrum-search)\n")
            }
            1 => out.extend_from_slice(
                b"\x1b[1;32m    Finished\x1b[0m dev profile in 1.79s, no diagnostics\n",
            ),
            2 => {
                out.extend_from_slice(b"\x1b[2mdebug\x1b[0m ring wrote 4096 bytes at seq 918273\n")
            }
            3 => out.extend_from_slice(b"\n"),
            4 => out.extend_from_slice(b"\x1b]0;vitrum - session 7\x07plain line after a title\n"),
            5 => out.extend_from_slice(
                b"a much longer line than the others, carrying a stack frame: \
                  at core::iter::adapters::map::Map<I,F> as core::iter::traits\n",
            ),
            6 => out.extend_from_slice(b"warning: unused variable `index`\n"),
            _ => out.extend_from_slice(b"test chunks::tests::empty_chunks_are_skipped ... ok\n"),
        }
    }
    out
}
