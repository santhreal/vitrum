//! The ConPTY cursor handshake: which bytes answer the console host, and which
//! bytes reach the client.

use crate::session::{CONPTY_CURSOR_QUERY, take_cursor_query};

/// A read that leads with the host's own preamble must still be recognised as
/// carrying the handshake.
///
/// This is the whole bug. `PSEUDOCONSOLE_INHERIT_CURSOR` makes conhost withhold
/// the child until something reports the cursor, and on some hosts the query
/// arrives behind a run of mode sets and an OSC 0 naming the shell. A prefix test
/// misses it there, so nothing answers, the query is forwarded to the client as
/// if it were child output, and the session delivers its preamble and then hangs
/// with the child never having run.
#[test]
fn the_query_is_found_behind_a_preamble() {
    let preamble = b"\x1b[?9001h\x1b[?1004h\x1b[m\x1b]0;C:\\Windows\\system32\\cmd.exe\x07\x1b[?25h";
    let mut read = preamble.to_vec();
    read.extend_from_slice(CONPTY_CURSOR_QUERY);

    let rest = take_cursor_query(&read).expect("the query is in this read");
    assert_eq!(rest, preamble, "the preamble is the client's, the query is not");
}

/// The query must be removed from whatever surrounds it, not just truncated at.
///
/// Bytes after the query are the child's and a client that never received them
/// would paint an incomplete screen, so the read is rejoined rather than cut.
#[test]
fn the_query_is_removed_from_the_middle_of_a_read() {
    let mut read = b"before".to_vec();
    read.extend_from_slice(CONPTY_CURSOR_QUERY);
    read.extend_from_slice(b"after");

    let rest = take_cursor_query(&read).expect("the query is in this read");
    assert_eq!(rest, b"beforeafter");
}

/// A read with no query must be left alone, because answering twice invites the
/// host to think a second terminal attached, and stripping a lookalike would eat
/// the child's own bytes.
#[test]
fn a_read_without_the_query_is_untouched() {
    assert_eq!(take_cursor_query(b"vitrum-ok\r\n"), None);
    assert_eq!(take_cursor_query(b"\x1b[6"), None, "a truncated query is not a query");
    assert_eq!(take_cursor_query(b""), None);
}
