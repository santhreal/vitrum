//! What the control plane and the history budgets owe the daemon.
//!
//! Two claims live here and nowhere else. The first is the exact JSON the
//! daemon reads: a casing slip in any of these messages is a client that types
//! into a server that ignores it, with no error anywhere. The second is that
//! the Scrollback setting moves the number of bytes an attach asks for, which
//! is what its caption promises and what the caption has twice been wrong
//! about.

use super::*;
use crate::state::TerminalPrefs;
use crate::ui::settings::SCROLLBACK_STEPS;
use vitrum_proto::{PROTOCOL_VERSION, ProjectId, SessionId};

/// Pins the exact JSON the client sends for a keystroke.
///
/// WHY: the pane reports captured bytes with no session on them, and
/// [`crate::sync`] addresses them to whatever is actually attached. What is
/// pinned here is the wire shape the daemon reads. A renamed field is a
/// keystroke that vanishes with no error at either end.
///
/// What this does NOT catch: the daemon reading a field this client never
/// sends, which is `vitrum-proto`'s round-trip suite.
#[test]
fn input_frame_matches_the_shape_the_daemon_reads() {
    let got = encode(&ClientMsg::Input {
        session: SessionId(7),
        data: vec![104, 105],
    });
    assert_eq!(got, r#"{"t":"input","session":7,"data":[104,105]}"#);
}

/// Pins the resize shape, for the same reason and by the same route: the pane
/// measures its surface and reports cols and rows, and this side decides which
/// session they belong to.
#[test]
fn resize_frame_matches_the_shape_the_daemon_reads() {
    let got = encode(&ClientMsg::Resize {
        session: SessionId(7),
        cols: 120,
        rows: 40,
    });
    assert_eq!(got, r#"{"t":"resize","session":7,"cols":120,"rows":40}"#);
}

/// Non-UTF-8 input must survive encoding.
///
/// WHY: a terminal sends mouse reports and DEC responses that are not text. If
/// these were routed through a JSON string instead of a byte array they would
/// be mangled into replacement characters, and a mouse drag would report a
/// column the child never sees.
#[test]
fn input_carries_arbitrary_bytes_not_text() {
    let got = encode(&ClientMsg::Input {
        session: SessionId(1),
        data: vec![0x1b, 0x5b, 0x4d, 0x20, 0xff, 0x80, 0x00],
    });
    assert_eq!(
        got,
        r#"{"t":"input","session":1,"data":[27,91,77,32,255,128,0]}"#
    );
}

/// Pins the handshake, token included.
///
/// WHY: a wrong field name means the server never replies `Welcome` and the
/// window sits on "connecting" with no explanation. A client that omitted the
/// token, or spelled it differently, would be refused by every daemon and the
/// operator would see an authentication failure they could do nothing about.
#[test]
fn hello_and_list_encode_exactly() {
    assert_eq!(
        encode(&ClientMsg::Hello {
            protocol: PROTOCOL_VERSION,
            token: "a".repeat(vitrum_proto::token::TOKEN_HEX_LEN),
        }),
        format!(
            r#"{{"t":"hello","protocol":3,"token":"{}"}}"#,
            "a".repeat(vitrum_proto::token::TOKEN_HEX_LEN)
        )
    );
    assert_eq!(encode(&ClientMsg::List), r#"{"t":"list"}"#);
}

/// Pins attach, detach, close and the backfill request.
///
/// WHY: these four drive every tab switch. A casing slip in any of them breaks
/// switching panes while leaving the rest of the client apparently fine.
#[test]
fn session_lifecycle_messages_encode_exactly() {
    assert_eq!(
        encode(&ClientMsg::Attach {
            session: SessionId(3),
            cols: 80,
            rows: 24
        }),
        r#"{"t":"attach","session":3,"cols":80,"rows":24}"#
    );
    assert_eq!(
        encode(&ClientMsg::Detach {
            session: SessionId(3)
        }),
        r#"{"t":"detach","session":3}"#
    );
    assert_eq!(
        encode(&ClientMsg::Close {
            session: SessionId(3)
        }),
        r#"{"t":"close","session":3}"#
    );
    assert_eq!(
        encode(&ClientMsg::Scrollback {
            session: SessionId(3),
            before_seq: BEFORE_SEQ_HEAD,
            max_bytes: backfill_max_bytes(1_000),
        }),
        r#"{"t":"scrollback","session":3,"beforeSeq":18446744073709551615,"maxBytes":64000}"#
    );
}

/// The head sentinel must serialize as a full u64.
///
/// WHY: truncated to an f64 it becomes 18446744073709552000, which the server
/// would clamp differently, silently changing what history arrives.
#[test]
fn head_sentinel_survives_json_round_trip() {
    let text = encode(&ClientMsg::Scrollback {
        session: SessionId(1),
        before_seq: BEFORE_SEQ_HEAD,
        max_bytes: 1,
    });
    let back: ClientMsg = serde_json::from_str(&text).expect("what we just encoded");
    let ClientMsg::Scrollback { before_seq, .. } = back else {
        panic!("wrong variant");
    };
    assert_eq!(before_seq, u64::MAX);
}

/// Pins `CreateSession`, which the launcher sends.
#[test]
fn create_session_encodes_exactly() {
    assert_eq!(
        encode(&ClientMsg::CreateSession {
            project_id: ProjectId(2),
            cwd: "/src/app".into(),
            command: "/usr/bin/claude".into(),
            args: vec!["--resume".into()],
            cols: 100,
            rows: 30,
            title: Some("claude".into()),
        }),
        r#"{"t":"createSession","projectId":2,"cwd":"/src/app","command":"/usr/bin/claude","args":["--resume"],"cols":100,"rows":30,"title":"claude"}"#
    );
}

/// Every scrollback step the settings sheet offers must ask the daemon for
/// strictly more pre-attach history than the step below it.
///
/// WHY: the backfill was a hard-coded 64 KiB, so picking "100,000 lines" grew
/// the local buffer a hundredfold and retrieved not one extra byte of the
/// history the daemon was already holding. Raising the setting was advertised
/// as the only way to see further back, and for everything written before the
/// attach it did nothing whatsoever.
///
/// **That caption has now been wrong twice.** Its first version described a
/// fetch no code made. The correction deleted the phrase and went on claiming
/// that raising the number was how you see further back, which was false for a
/// different reason, and the guard left behind only checked that the retired
/// phrase was absent. A word-absence test cannot catch this class of defect at
/// all, which is why it survived. This one asserts the relationship the
/// caption claims.
///
/// What this does NOT catch: a send site that computes the budget from a
/// literal, which the case below owns.
#[test]
fn every_scrollback_step_fetches_strictly_more_history_than_the_one_below() {
    let offered: Vec<u32> = SCROLLBACK_STEPS.iter().map(|(lines, _)| *lines).collect();
    let budgets: Vec<u32> = offered.iter().copied().map(backfill_max_bytes).collect();

    assert_eq!(offered, vec![1_000, 5_000, 20_000, 100_000]);
    assert_eq!(
        budgets,
        vec![64_000, 320_000, 1_280_000, BACKFILL_CEILING_BYTES]
    );
    for (pair, lines) in budgets.windows(2).zip(offered.windows(2)) {
        assert!(
            pair[1] > pair[0],
            "{} lines and {} lines both ask for {} bytes, so one of them shows \
             no more history than the other and the caption is overstating the \
             product for the third time",
            lines[0],
            lines[1],
            pair[0]
        );
    }
}

/// The budget must be clamped at both ends.
///
/// WHY: `scrollbackLines` is deserialized from the state file with nothing
/// validating it. A hand-edited or truncated `0` computes a zero-byte backfill,
/// and an attach that asks for zero bytes paints a blank screen for a session
/// whose output the daemon still has, which reads as lost history rather than
/// as a setting. A huge value asks the daemon for tens of megabytes on a tab
/// switch. And a multiply that wraps instead of saturating turns the largest
/// settings into the smallest budget.
///
/// The wrap probe is `1 << 26` and not `u32::MAX`, and that is the whole point
/// of it. `u32::MAX` cannot discriminate: it wraps to 4294967232, still far
/// above the ceiling, so both spellings clamp to the same answer. `1 << 26` is
/// the input where they diverge: 67108864 x 64 is exactly 2^32, which saturates
/// to the ceiling and wraps to zero.
#[test]
fn the_backfill_budget_is_clamped_at_both_ends() {
    assert_eq!(backfill_max_bytes(0), BACKFILL_MIN_BYTES);
    assert_eq!(backfill_max_bytes(1), BACKFILL_MIN_BYTES);
    // 256 lines x 64 bytes is the floor exactly; 257 is the first step off it.
    assert_eq!(backfill_max_bytes(256), 16_384);
    assert_eq!(backfill_max_bytes(257), 16_448);

    assert_eq!(BACKFILL_CEILING_BYTES, 2 * 1024 * 1024);
    assert_eq!(backfill_max_bytes(32_769), BACKFILL_CEILING_BYTES);
    assert_eq!(backfill_max_bytes(u32::MAX), BACKFILL_CEILING_BYTES);
    assert_eq!(
        backfill_max_bytes(1 << 26),
        BACKFILL_CEILING_BYTES,
        "67108864 lines x 64 bytes is exactly 2^32; a wrapping multiply makes \
         that 0 and hands the deepest scrollback the shallowest backfill"
    );
}

/// A page-back grows by one attach-sized window and then stops.
///
/// WHY: a gesture that grew without bound would let one pane hold the whole of
/// a day's output, and a gesture that did not grow at all would re-request the
/// same window forever and look inert. `None` at the ceiling is what
/// [`crate::sync::plan_page_back`] turns into a sentence rather than a silent
/// repaint of bytes already on screen.
///
/// What this does NOT catch: whether the refusal is stated once, which
/// `sync::a_refusal_speaks_once` owns.
#[test]
fn a_page_back_grows_by_one_window_and_stops_at_the_ceiling() {
    let step = u64::from(backfill_max_bytes(1_000));
    assert_eq!(page_back_max_bytes(0, 1_000), Some(step as u32));
    assert_eq!(page_back_max_bytes(step, 1_000), Some((step * 2) as u32));
    assert_eq!(
        page_back_max_bytes(u64::from(PAGE_CEILING_BYTES) - 1, 1_000),
        Some(PAGE_CEILING_BYTES),
        "the last grant is clamped to the ceiling, not refused"
    );
    assert_eq!(page_back_max_bytes(u64::from(PAGE_CEILING_BYTES), 1_000), None);
    assert_eq!(page_back_max_bytes(u64::MAX, 100_000), None);
}

/// The operator's setting must actually reach the wire.
///
/// WHY: every other guard here exercises `backfill_max_bytes` with a literal or
/// with `SCROLLBACK_STEPS`, so all of them stay green if `reconcile` calls it
/// with a constant. That restores the original defect one layer up: the
/// function is correct, the caption is correct, the four steps are distinct,
/// and the number the operator picked still never leaves the settings sheet.
///
/// Reads the shipped source because the send site is inside a component path
/// with no seam to call. Anchored on tokens rather than on quote pairing.
#[test]
fn the_send_site_computes_the_budget_from_the_operators_setting() {
    let shell = crate::testkit::shell();
    let shell = shell.as_str();

    let at = shell
        .find("ClientMsg::Scrollback")
        .expect("the client no longer asks the daemon for scrollback at all");
    let end = shell[at..]
        .find("});")
        .map(|i| at + i)
        .expect("the Scrollback send site is no longer a struct literal");
    let site = &shell[at..end];

    let key = site
        .find("max_bytes:")
        .expect("the Scrollback send site has no max_bytes field");
    let arg = site[key + "max_bytes:".len()..]
        .lines()
        .next()
        .expect("max_bytes has no value")
        .trim()
        .trim_end_matches(',');

    let inner = arg
        .strip_prefix("backfill_max_bytes(")
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or_else(|| panic!("max_bytes is `{arg}`, not a call to `backfill_max_bytes`"))
        .trim();

    assert!(
        !inner.is_empty() && !inner.starts_with(|c: char| c.is_ascii_digit()),
        "the budget is computed from the literal `{inner}`, so the setting \
         never reaches the wire and the Scrollback caption is false again"
    );

    // `expect`, never `unwrap_or(0)`. Falling back to 0 would silently widen
    // the scan to the whole file above the send site, so the clause below would
    // keep passing by finding the binding somewhere unrelated instead of
    // reporting that the function it scopes to is gone.
    let from = shell[..at]
        .rfind("fn reconcile")
        .expect("`reconcile` was renamed, so this guard no longer knows what it is reading");
    assert!(
        shell[from..end].contains("settings.terminal.scrollback_lines"),
        "`{inner}` is not bound from `settings.terminal.scrollback_lines`, so \
         the backfill is a function of something other than the setting whose \
         caption promises it"
    );
}

/// The shipped default must cross the wire under the name the server reads,
/// and it must be enough to fill the screen it lands on.
///
/// WHY: the protocol already types `max_bytes` as a u32 and Rust enforces that,
/// so a bare comparison proves nothing. What can go wrong is the field name
/// drifting, or the number being set so small that a tab switch shows a
/// fragment of a screen.
#[test]
fn the_backfill_request_crosses_the_wire_as_max_bytes() {
    /// One screen of dense output, measured on the corpus.
    const DENSE_SCREEN: u64 = 20 * 1024;

    let text = encode(&ClientMsg::Scrollback {
        session: SessionId(5),
        before_seq: BEFORE_SEQ_HEAD,
        max_bytes: backfill_max_bytes(TerminalPrefs::default().scrollback_lines),
    });
    let sent: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    let budget = sent["maxBytes"]
        .as_u64()
        .expect("the server reads the budget from `maxBytes`");

    assert_eq!(budget, 64_000, "the shipped default asks for 1,000 lines");
    assert!(
        budget >= 3 * DENSE_SCREEN,
        "a backfill that cannot fill three screens leaves the pane looking truncated"
    );
}
