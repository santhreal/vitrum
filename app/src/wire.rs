//! Control-plane encoding and the Rust <-> JavaScript bridge vocabulary.
//!
//! Two transports, deliberately split, matching `vitrum-proto`:
//!
//! - Control plane: JSON text frames. Encoded here, in Rust, so there is one
//!   tested definition of every message the client sends.
//! - Data plane: binary frames of raw PTY bytes. Those never enter this
//!   process's Rust half at all. The bridge strips the 17-byte header in
//!   JavaScript and hands the payload straight to xterm.js, because routing a
//!   firehose of arbitrary bytes through a JSON IPC channel would cost a
//!   base64 pass, a parse, and a copy per chunk.
//!
//! The two exceptions are keystrokes and resizes, which the bridge builds
//! itself to keep them off the two-hop round trip through Rust. Their exact
//! JSON shape is pinned by tests here so the JavaScript cannot drift from
//! `vitrum-proto`.

use vitrum_proto::{ClientMsg, ServerMsg};
use serde::{Deserialize, Serialize};

/// Where the session daemon listens unless `--server` says otherwise.
///
/// Loopback, because the daemon owns PTYs and scrollback for this machine's
/// agents; exposing that on a network interface would be a remote shell with
/// no authentication. `--server` exists for running a second daemon beside the
/// first, which is how the disconnect and reconnect paths are tested without
/// taking down the one everybody else is using.
pub const DEFAULT_WS_URL: &str = "ws://127.0.0.1:7737";

/// Assumed bytes of PTY output behind one line of terminal buffer.
///
/// The operator picks a line count and the protocol takes a byte count, so one
/// of the two has to be assumed. 64 bytes is a line of roughly fifty printable
/// columns, its `\r\n`, and one SGR colour pair, which is what tool and agent
/// output actually looks like. Bracketed by two measurements rather than
/// guessed: a full `cargo test --color=always` stream is 69.5 bytes a line and
/// this repo's own 289 source files average 39.2, so 64 sits between the
/// chattiest realistic producer and the plainest.
///
/// Deliberately far below the ~850 bytes a dense, fully wrapped, heavily
/// coloured row costs. Sizing for that at the largest offered setting would be
/// 85 MB per attach, and the honest guarantee is "more history than the step
/// below", not "exactly N lines whatever the content". Undershooting leaves a
/// deep buffer partly unfilled from history; overshooting makes every attach
/// pay for bytes xterm.js drops on arrival.
///
/// At the shipped 1,000-line default this asks for 64,000 bytes, within 2.3%
/// of the 65,536 the previous fixed constant sent, so the default path does
/// not change behaviour.
pub const BACKFILL_BYTES_PER_LINE: u32 = 64;

/// Smallest backfill worth asking for, whatever the setting says.
///
/// 16 KiB is 256 lines at [`BACKFILL_BYTES_PER_LINE`], more rows than any
/// window shows. `scrollbackLines` is deserialized from the state file with no
/// clamp on it, so a hand-edited `0` would otherwise compute a zero-byte
/// budget and attach to a blank grid instead of repainting the screen the
/// daemon already has.
pub const BACKFILL_MIN_BYTES: u32 = 16 * 1024;

/// Hard ceiling on one backfill, in bytes.
///
/// This budget crosses the Rust -> JavaScript bridge as a JSON array of
/// integers, not as binary: measured at 3.6 bytes of JSON per payload byte on
/// real output, so 2 MiB of history is a 7.5 MB string to serialize, ship and
/// `JSON.parse`, plus a transient array of two million JS numbers before
/// `Uint8Array.from` copies it into the grid. Twenty windows share one
/// `WebKitWebProcess`, which is what puts twenty live sessions at 398 MB, so
/// an attach may not spike that process without bound. The latency matters on
/// its own too: live frames queue in `bootstrap.js` against a 1 MiB
/// `PENDING_CAP` while the backfill is in flight, and a backfill slow enough
/// to overflow that queue is discarded outright, costing the operator all
/// history rather than some.
///
/// 2 MiB is above 20,000 lines x [`BACKFILL_BYTES_PER_LINE`] and below
/// 100,000 x it, so every step the settings sheet offers still asks for
/// strictly more than the step beneath it. It is also well under the daemon's
/// 10 MiB per-session ring (`DEFAULT_SCROLLBACK_BYTES`), so the ceiling never
/// asks for bytes the server does not hold.
pub const BACKFILL_CEILING_BYTES: u32 = 2 * 1024 * 1024;

/// Bytes of history to request when a pane gains focus, for an operator who
/// asked for `scrollback_lines` of local buffer.
///
/// A function of the setting rather than a constant, because the setting's
/// caption promises that raising it is how you see further back and for two
/// separate corrections of that caption the promise was false: the backfill
/// was a fixed 64 KiB, so choosing 100,000 lines grew the xterm buffer and
/// retrieved not one extra byte of the history the daemon already held.
///
/// Deeper history than this still stays on the server, which is the point of
/// the split. What changed is that the operator moves where the line falls.
#[must_use]
pub const fn backfill_max_bytes(scrollback_lines: u32) -> u32 {
    // `Ord::clamp` is not const, and the multiply saturates rather than
    // wrapping: the setting is a u32 read from a file nobody validates.
    let want = scrollback_lines.saturating_mul(BACKFILL_BYTES_PER_LINE);
    if want < BACKFILL_MIN_BYTES {
        BACKFILL_MIN_BYTES
    } else if want > BACKFILL_CEILING_BYTES {
        BACKFILL_CEILING_BYTES
    } else {
        want
    }
}

/// Sentinel `before_seq` meaning "everything up to the current head".
///
/// The server clamps it to `head_seq`. Sending the head we last saw instead
/// would race: the child can emit bytes between the `Attach` and the
/// `Scrollback` being processed, and those bytes would never be painted.
pub const BEFORE_SEQ_HEAD: u64 = u64::MAX;

/// Most history one pane will hold after paging back.
///
/// Paging back is a repaint: the daemon is asked for a bigger window ending at
/// the same head, the grid is reset and the whole thing is written again. That
/// is the only exact way to prepend to xterm.js, which has no prepend, and it
/// is affordable because it happens on a deliberate gesture rather than on
/// output. It is not affordable without a ceiling, so this is one.
///
/// 8 MiB is four full [`BACKFILL_CEILING_BYTES`] windows, roughly 128,000
/// lines at the 64-byte estimate. Past it the operator is told the client will
/// not hold more rather than being given a window that takes seconds to
/// repaint.
pub const PAGE_CEILING_BYTES: u32 = 8 * 1024 * 1024;

/// Budget for the next page-back, given what is already painted.
///
/// Grows by one attach-sized window per gesture, so each page-back shows about
/// as much new history as the operator saw when they arrived. Returns `None`
/// once the ceiling is reached, which is the caller's cue to say so instead of
/// silently repainting the same bytes.
#[must_use]
pub const fn page_back_max_bytes(painted: u64, scrollback_lines: u32) -> Option<u32> {
    if painted >= PAGE_CEILING_BYTES as u64 {
        return None;
    }
    let step = backfill_max_bytes(scrollback_lines) as u64;
    let want = painted + step;
    if want >= PAGE_CEILING_BYTES as u64 {
        Some(PAGE_CEILING_BYTES)
    } else {
        // `want` is below a u32 constant, so this cannot truncate.
        Some(want as u32)
    }
}

/// Bytes of context to keep after a search hit when jumping to it.
///
/// The daemon answers "the last N bytes before this offset", so landing on a
/// hit means asking for a window that ENDS past it. Without this slack the hit
/// would be the last byte painted, at the very bottom of the grid, with no
/// sight of what the agent said next, which is usually the reason the operator
/// searched for it.
pub const JUMP_TAIL_BYTES: u64 = 8 * 1024;

/// Encode a control-plane message as the JSON text frame the server expects.
pub fn encode(msg: &ClientMsg) -> String {
    // ClientMsg is a closed enum of plain data with no map keys that can fail
    // to serialize, so this cannot error in practice; a panic here would mean
    // vitrum-proto changed shape underneath us.
    serde_json::to_string(msg).expect("ClientMsg is always serializable")
}

/// Socket lifecycle as the bridge observes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnEvent {
    Open,
    Closed,
    Error,
}

/// Everything the bridge reports back to Rust.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "ev", rename_all = "camelCase")]
pub enum BridgeEvent {
    /// A control-plane text frame, already parsed from JSON by the bridge.
    Server { msg: ServerMsg },
    /// Socket opened, closed, or refused.
    Conn {
        state: ConnEvent,
        #[serde(default)]
        detail: Option<String>,
    },
    /// Terminal geometry after the fit addon measured the container.
    Resize { cols: u16, rows: u16 },
    /// A chord the shell handles. Raw string; validated by
    /// [`crate::keymap::KeyAction::parse`].
    Key { action: String },
    /// Result of a [`BridgeCmd::Clipboard`] write.
    ///
    /// Reported rather than assumed: a webview can refuse a clipboard write,
    /// and a "Copied" notice for a copy that did not happen is a lie the user
    /// only discovers when they paste.
    Copied { ok: bool, text: String },
    /// The operator scrolled to the top of the painted history and there is
    /// more behind it.
    ///
    /// Sent once per arrival at the top, not per wheel tick: paging is a
    /// deliberate act and a repeat while a request is already in flight would
    /// stack repaints. The bridge suppresses the repeat; Rust does not have to
    /// debounce.
    PageBack { session: u64 },
    /// The bridge could not make sense of something: a malformed frame, a
    /// missing library, a lost WebGL context. Surfaced, never swallowed.
    Bad { detail: String },
}

/// Everything Rust asks the bridge to do.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "cmd", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum BridgeCmd {
    /// Open (or reopen) the WebSocket. Only ever sent on startup or on an
    /// explicit user retry; there is no automatic reconnect loop.
    Connect { url: String },
    /// Send a pre-encoded control-plane text frame.
    Send { text: String },
    /// Point the single terminal at `session`, reset the grid, and start
    /// buffering live frames until the backfill lands. `None` clears the pane.
    Focus { session: Option<u64> },
    /// Paint history, then flush buffered live frames from `resume_seq`.
    ///
    /// `resume_seq` and `from_seq` are decimal strings: a byte offset is a u64
    /// and JSON numbers are f64, which silently rounds above 2^53.
    ///
    /// `from_seq` is the stream offset of the FIRST painted byte. The bridge
    /// needs it to turn a search hit's absolute byte offset into a position
    /// inside the buffer it just painted; without it a hit offset is a number
    /// with no frame of reference.
    ///
    /// `jump_seq` asks the grid to scroll to that absolute offset once the
    /// paint lands, which is what makes a search hit land on its line instead
    /// of wherever the head-anchored history happened to stop.
    ///
    /// `keep_view` is set when this repaint is a page-back: the operator is
    /// looking at a line and asked for more history above it, so the viewport
    /// must stay on that line rather than snapping to the bottom.
    ///
    /// `more` is the daemon saying it still holds bytes older than `from_seq`.
    /// The bridge needs it so reaching the top of the buffer only asks for
    /// more when there IS more; without it every scroll to the top would ask,
    /// and every ask that could not be answered would flash a notice.
    Backfill {
        session: u64,
        from_seq: String,
        resume_seq: String,
        bytes: Vec<u8>,
        jump_seq: Option<String>,
        keep_view: bool,
        more: bool,
    },
    /// Write literal text into the terminal. Used only by fixture mode, which
    /// has no server to produce bytes.
    Banner { lines: Vec<String> },
    /// Move DOM focus to the first element matching `selector`. Focus is a DOM
    /// operation with no virtual-DOM equivalent, so it has to be asked for
    /// explicitly.
    FocusDom { selector: String },
    /// Put `text` on the system clipboard and report the outcome back as
    /// [`BridgeEvent::Copied`].
    Clipboard { text: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::TerminalPrefs;
    use crate::ui::settings::SCROLLBACK_STEPS;
    use vitrum_proto::{PROTOCOL_VERSION, ProjectId, SessionId, SessionStatus};

    /// Pins the exact JSON the bridge builds for a keystroke.
    ///
    /// Keystrokes are the one message JavaScript encodes itself, to keep typing
    /// off a two-hop IPC round trip. That means `bootstrap.js` hardcodes this
    /// shape. If serde's tagging, casing, or field order ever changes, this
    /// test fails and the JavaScript must be updated in the same commit;
    /// without it the client would type into a server that ignores it.
    #[test]
    fn input_frame_matches_the_shape_bootstrap_js_builds() {
        let got = encode(&ClientMsg::Input {
            session: SessionId(7),
            data: vec![104, 105],
        });
        assert_eq!(got, r#"{"t":"input","session":7,"data":[104,105]}"#);
    }

    /// Pins the resize shape for the same reason: the fit addon builds it in
    /// JavaScript, so the Rust definition is the only place it is checked.
    #[test]
    fn resize_frame_matches_the_shape_bootstrap_js_builds() {
        let got = encode(&ClientMsg::Resize {
            session: SessionId(7),
            cols: 120,
            rows: 40,
        });
        assert_eq!(got, r#"{"t":"resize","session":7,"cols":120,"rows":40}"#);
    }

    /// Non-UTF-8 input must survive encoding. A terminal sends mouse reports
    /// and DEC responses that are not text; if these were ever routed through a
    /// JSON string instead of a byte array they would be mangled.
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

    /// Pins the handshake. A wrong field name here means the server never
    /// replies `Welcome` and the app sits on "connecting" with no explanation.
    #[test]
    fn hello_and_list_encode_exactly() {
        assert_eq!(
            encode(&ClientMsg::Hello {
                protocol: PROTOCOL_VERSION
            }),
            r#"{"t":"hello","protocol":1}"#
        );
        assert_eq!(encode(&ClientMsg::List), r#"{"t":"list"}"#);
    }

    /// Pins attach, detach, close, and the backfill request. These four drive
    /// every tab switch; a casing slip in any of them breaks switching panes
    /// while leaving the rest of the app apparently fine.
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

    /// The head sentinel must serialize as a full u64, not lose precision.
    /// Truncated to f64 it becomes 18446744073709552000, which the server would
    /// reject or clamp differently, silently changing what history arrives.
    #[test]
    fn head_sentinel_survives_json_round_trip() {
        let text = encode(&ClientMsg::Scrollback {
            session: SessionId(1),
            before_seq: BEFORE_SEQ_HEAD,
            max_bytes: 1,
        });
        let back: ClientMsg = serde_json::from_str(&text).unwrap();
        let ClientMsg::Scrollback { before_seq, .. } = back else {
            panic!("wrong variant");
        };
        assert_eq!(before_seq, u64::MAX);
    }

    /// Pins `CreateSession`, which the sidebar's "+" button sends.
    #[test]
    fn create_session_encodes_exactly() {
        assert_eq!(
            encode(&ClientMsg::CreateSession {
                project_id: ProjectId(2),
                cwd: "/src/app".into(),
                command: "/bin/bash".into(),
                args: vec!["-l".into()],
                cols: 100,
                rows: 30,
                title: Some("shell".into()),
            }),
            r#"{"t":"createSession","projectId":2,"cwd":"/src/app","command":"/bin/bash","args":["-l"],"cols":100,"rows":30,"title":"shell"}"#
        );
    }

    /// The bridge wraps a parsed `ServerMsg` in an envelope. This locks the
    /// nesting so a server push actually reaches the reducer instead of being
    /// dropped as an unknown event.
    #[test]
    fn server_events_deserialize_through_the_envelope() {
        let raw = r#"{"ev":"server","msg":{"t":"welcome","protocol":1,"serverVersion":"0.1.0"}}"#;
        assert_eq!(
            serde_json::from_str::<BridgeEvent>(raw).unwrap(),
            BridgeEvent::Server {
                msg: ServerMsg::Welcome {
                    protocol: 1,
                    server_version: "0.1.0".into()
                }
            }
        );
    }

    /// A full `Sessions` snapshot must survive the envelope with every field
    /// intact. This is the message the sidebar is built from; a field dropped
    /// here shows as a blank row rather than an error.
    #[test]
    fn sessions_snapshot_deserializes_with_every_field() {
        let raw = r#"{"ev":"server","msg":{"t":"sessions","sessions":[{
            "id":4,"projectId":2,"title":"claude","cwd":"/src/app","command":"claude",
            "args":["--resume"],"status":{"state":"running"},"createdAtMs":1000,
            "lastActivityMs":2000,"cols":80,"rows":24,"gitBranch":"main","unread":true,
            "attention":{"bell":true,"idleMs":45000,"failed":false,"waiting":false},
            "hint":{"state":"approval","label":"run migrations?","receivedAtMs":2500}}]}}"#;
        let BridgeEvent::Server {
            msg: ServerMsg::Sessions { sessions: v },
        } = serde_json::from_str::<BridgeEvent>(raw).unwrap()
        else {
            panic!("wrong variant");
        };
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].id, SessionId(4));
        assert_eq!(v[0].project_id, ProjectId(2));
        assert_eq!(v[0].title, "claude");
        assert_eq!(v[0].args, vec!["--resume".to_string()]);
        assert_eq!(v[0].status, SessionStatus::Running);
        assert_eq!(v[0].created_at_ms, 1000);
        assert_eq!(v[0].last_activity_ms, 2000);
        assert_eq!(v[0].git_branch.as_deref(), Some("main"));
        assert!(v[0].unread);
        assert_eq!(
            v[0].attention,
            vitrum_proto::Attention {
                bell: true,
                idle_ms: 45_000,
                failed: false,
                waiting: Some(false),
            },
            "attention drives sidebar ordering; a dropped field silently \
             demotes every session to priority zero"
        );
        assert_eq!(v[0].attention.priority(), 2);
        let hint = v[0]
            .hint
            .as_ref()
            .expect("the opt-in hint channel must survive");
        assert_eq!(hint.state, vitrum_proto::HintState::Approval);
        assert_eq!(hint.label.as_deref(), Some("run migrations?"));
        assert_eq!(hint.received_at_ms, 2500);
    }

    /// A `Projects` snapshot must survive the envelope too.
    ///
    /// This variant and `Sessions` were unserializable in the first cut of
    /// `vitrum-proto`: serde's internal tagging cannot encode a newtype variant
    /// wrapping a sequence, so both failed at runtime while the type-checker
    /// and the test suite stayed green. They are struct variants now, and this
    /// test is what would catch a revert.
    #[test]
    fn projects_snapshot_deserializes_through_the_envelope() {
        let raw = r#"{"ev":"server","msg":{"t":"projects","projects":[
            {"id":2,"name":"vitrum","root":"/src/vitrum"}]}}"#;
        let BridgeEvent::Server {
            msg: ServerMsg::Projects { projects },
        } = serde_json::from_str::<BridgeEvent>(raw).unwrap()
        else {
            panic!("wrong variant");
        };
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].id, ProjectId(2));
        assert_eq!(projects[0].name, "vitrum");
        assert_eq!(projects[0].root, "/src/vitrum");
    }

    /// Both snapshot variants must round-trip in the sending direction as
    /// well, which is the exact operation that used to return an error.
    #[test]
    fn snapshot_variants_serialize_without_erroring() {
        assert_eq!(
            serde_json::to_string(&ServerMsg::Projects { projects: vec![] }).unwrap(),
            r#"{"t":"projects","projects":[]}"#
        );
        assert_eq!(
            serde_json::to_string(&ServerMsg::Sessions { sessions: vec![] }).unwrap(),
            r#"{"t":"sessions","sessions":[]}"#
        );
    }

    /// A close event without a detail string must still parse. The bridge omits
    /// `detail` on a clean close, and a hard failure there would drop the one
    /// event that tells the UI the server went away.
    #[test]
    fn conn_events_parse_with_and_without_detail() {
        assert_eq!(
            serde_json::from_str::<BridgeEvent>(r#"{"ev":"conn","state":"open"}"#).unwrap(),
            BridgeEvent::Conn {
                state: ConnEvent::Open,
                detail: None
            }
        );
        assert_eq!(
            serde_json::from_str::<BridgeEvent>(
                r#"{"ev":"conn","state":"error","detail":"cannot reach ws://127.0.0.1:7737"}"#
            )
            .unwrap(),
            BridgeEvent::Conn {
                state: ConnEvent::Error,
                detail: Some("cannot reach ws://127.0.0.1:7737".into())
            }
        );
        assert_eq!(
            serde_json::from_str::<BridgeEvent>(r#"{"ev":"conn","state":"closed"}"#).unwrap(),
            BridgeEvent::Conn {
                state: ConnEvent::Closed,
                detail: None
            }
        );
    }

    /// Resize and bad-frame reports must parse from the exact objects the
    /// bridge sends.
    #[test]
    fn resize_and_bad_events_parse() {
        assert_eq!(
            serde_json::from_str::<BridgeEvent>(r#"{"ev":"resize","cols":213,"rows":57}"#).unwrap(),
            BridgeEvent::Resize {
                cols: 213,
                rows: 57
            }
        );
        assert_eq!(
            serde_json::from_str::<BridgeEvent>(r#"{"ev":"bad","detail":"short frame 9"}"#)
                .unwrap(),
            BridgeEvent::Bad {
                detail: "short frame 9".into()
            }
        );
    }

    /// The clipboard outcome must parse in both directions. A refused write
    /// reported as a success would show "Copied" for a clipboard that still
    /// holds whatever was there before, and the user only finds out on paste.
    #[test]
    fn copy_results_parse_for_success_and_failure() {
        assert_eq!(
            serde_json::from_str::<BridgeEvent>(
                r#"{"ev":"copied","ok":true,"text":"/src/vitrum"}"#
            )
            .unwrap(),
            BridgeEvent::Copied {
                ok: true,
                text: "/src/vitrum".into()
            }
        );
        assert_eq!(
            serde_json::from_str::<BridgeEvent>(r#"{"ev":"copied","ok":false,"text":"main"}"#)
                .unwrap(),
            BridgeEvent::Copied {
                ok: false,
                text: "main".into()
            }
        );
    }

    /// Pins the command envelope the bridge switches on. A renamed field here
    /// makes the bridge silently ignore the command, which looks like a dead
    /// click rather than an error.
    #[test]
    fn bridge_commands_serialize_to_the_shape_bootstrap_js_switches_on() {
        assert_eq!(
            serde_json::to_string(&BridgeCmd::Connect {
                url: DEFAULT_WS_URL.into()
            })
            .unwrap(),
            r#"{"cmd":"connect","url":"ws://127.0.0.1:7737"}"#
        );
        assert_eq!(
            serde_json::to_string(&BridgeCmd::Send {
                text: r#"{"t":"list"}"#.into()
            })
            .unwrap(),
            r#"{"cmd":"send","text":"{\"t\":\"list\"}"}"#
        );
        assert_eq!(
            serde_json::to_string(&BridgeCmd::Focus { session: Some(9) }).unwrap(),
            r#"{"cmd":"focus","session":9}"#
        );
        assert_eq!(
            serde_json::to_string(&BridgeCmd::Focus { session: None }).unwrap(),
            r#"{"cmd":"focus","session":null}"#
        );
        assert_eq!(
            serde_json::to_string(&BridgeCmd::Banner {
                lines: vec!["hi".into()]
            })
            .unwrap(),
            r#"{"cmd":"banner","lines":["hi"]}"#
        );
        assert_eq!(
            serde_json::to_string(&BridgeCmd::FocusDom {
                selector: "#rg-filter".into()
            })
            .unwrap(),
            r##"{"cmd":"focusDom","selector":"#rg-filter"}"##
        );
        assert_eq!(
            serde_json::to_string(&BridgeCmd::Clipboard {
                text: "/src/vitrum".into()
            })
            .unwrap(),
            r#"{"cmd":"clipboard","text":"/src/vitrum"}"#
        );
    }

    /// Every byte offset must cross the bridge as a string. As a JSON number a
    /// byte offset past 2^53 rounds, and the splice that decides how many
    /// leading bytes of a live frame to drop would then be wrong by up to a
    /// kilobyte, corrupting the terminal mid-escape-sequence.
    ///
    /// `from_seq` and `jump_seq` carry the same hazard for the same reason:
    /// the bridge subtracts them to locate a search hit inside the bytes it
    /// just painted, and a rounded subtrahend puts the operator on the wrong
    /// line with no sign anything went wrong.
    #[test]
    fn backfill_offsets_cross_as_strings() {
        let cmd = BridgeCmd::Backfill {
            session: 5,
            from_seq: (u64::MAX - 4).to_string(),
            resume_seq: (u64::MAX - 1).to_string(),
            bytes: vec![1, 2, 255],
            jump_seq: Some((u64::MAX - 3).to_string()),
            keep_view: true,
            more: true,
        };
        assert_eq!(
            serde_json::to_string(&cmd).unwrap(),
            r#"{"cmd":"backfill","session":5,"fromSeq":"18446744073709551611","resumeSeq":"18446744073709551614","bytes":[1,2,255],"jumpSeq":"18446744073709551612","keepView":true,"more":true}"#
        );
        // The common case: an attach, which asks for neither.
        let plain = BridgeCmd::Backfill {
            session: 5,
            from_seq: "0".to_string(),
            resume_seq: "3".to_string(),
            bytes: vec![],
            jump_seq: None,
            keep_view: false,
            more: false,
        };
        assert_eq!(
            serde_json::to_string(&plain).unwrap(),
            r#"{"cmd":"backfill","session":5,"fromSeq":"0","resumeSeq":"3","bytes":[],"jumpSeq":null,"keepView":false,"more":false}"#
        );
    }

    /// Every scrollback step the settings sheet offers must ask the daemon for
    /// strictly more pre-attach history than the step below it.
    ///
    /// Locks out the defect where the Scrollback setting could not do what its
    /// caption said. The backfill was a hard-coded `BACKFILL_MAX_BYTES` of
    /// 64 KiB, so picking "100,000 lines" grew the xterm buffer a hundredfold
    /// and retrieved not one extra byte of the history the daemon was already
    /// holding. Raising the setting was advertised as the only way to see
    /// further back, and for everything written before the attach it did
    /// nothing whatsoever.
    ///
    /// **That caption has now been wrong twice.** Its first version told the
    /// operator the daemon "serves it on demand ... before a request",
    /// describing a fetch no code makes. The correction deleted those two
    /// phrases and went on claiming that raising the number was how you see
    /// further back, which was false for a different reason, and the guard
    /// left behind only checked that the two retired phrases were absent. A
    /// word-absence test cannot catch this class of defect at all, which is
    /// why it survived. This one asserts the relationship the caption claims,
    /// so the claim holds only while the budget really moves with the setting.
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
                "{} lines and {} lines both ask for {} bytes, so one of them \
                 shows no more history than the other and the caption is \
                 overstating the product for the third time",
                lines[0],
                lines[1],
                pair[0]
            );
        }
    }

    /// The budget must be clamped at both ends, because `scrollbackLines` is
    /// deserialized from the state file with nothing validating it.
    ///
    /// Locks out the three ways a bare multiply fails. A hand-edited or
    /// truncated `0` computes a zero-byte backfill, and an attach that asks
    /// for zero bytes paints a blank grid for a session whose output the
    /// daemon still has, which reads as lost history rather than as a setting.
    /// A huge value asks a webview to `JSON.parse` tens of megabytes on a tab
    /// switch; twenty windows share one web process, which is the whole reason
    /// twenty live sessions fit in 398 MB, so one unbounded backfill spikes all
    /// twenty. And a multiply that wraps instead of saturating turns the
    /// largest settings into the smallest budget.
    ///
    /// The same caption is downstream of this, and it has been wrong twice, so
    /// the clamp is asserted by value: a ceiling that quietly moved would make
    /// two steps identical again without any test noticing.
    ///
    /// The wrap probe is `1 << 26` and not `u32::MAX`, and that is the whole
    /// point of it. This test used to assert
    /// `u32::MAX.checked_mul(BACKFILL_BYTES_PER_LINE).is_none()`, which is a
    /// fact about the two constants and says nothing about the function:
    /// swapping `saturating_mul` for `wrapping_mul` left it green, found by
    /// hunting for a mutation that escapes rather than confirming one that
    /// does not. `u32::MAX` cannot discriminate either, because it wraps to
    /// 4294967232, still far above the ceiling, so both spellings clamp to the
    /// same answer. `1 << 26` is the input where they diverge: 67108864 x 64
    /// is exactly 2^32, which saturates to the ceiling and wraps to zero, so
    /// the largest scrollback anyone could ask for would quietly return the
    /// smallest backfill the floor allows.
    #[test]
    fn the_backfill_budget_is_clamped_at_both_ends() {
        assert_eq!(backfill_max_bytes(0), BACKFILL_MIN_BYTES);
        assert_eq!(backfill_max_bytes(1), BACKFILL_MIN_BYTES);
        // Those two only prove the floor is APPLIED. Both sides move together,
        // so they say nothing about what it is: a floor of 64 bytes, one single
        // line, satisfies them and every other assertion in this file, which
        // is a hand-edited `scrollbackLines: 0` attaching to a blank grid
        // exactly as if there were no floor at all. Measured: every value from
        // 64 to 16383 escaped before this line existed.
        //
        // So assert what the floor is FOR, in the currency it is denominated
        // in. It has to repaint the visible grid whatever the setting says, and
        // no window shows 256 rows.
        assert!(
            BACKFILL_MIN_BYTES / BACKFILL_BYTES_PER_LINE >= 256,
            "the floor covers {} lines, fewer rows than a tall window shows, so \
             a session whose buffer setting is zero attaches to a grid the \
             backfill cannot fill",
            BACKFILL_MIN_BYTES / BACKFILL_BYTES_PER_LINE
        );
        // 256 lines x 64 bytes is the floor exactly; 257 is the first step off it.
        assert_eq!(backfill_max_bytes(256), 16_384);
        assert_eq!(backfill_max_bytes(257), 16_448);

        assert_eq!(BACKFILL_CEILING_BYTES, 2 * 1024 * 1024);
        assert_eq!(backfill_max_bytes(32_769), BACKFILL_CEILING_BYTES);
        assert_eq!(backfill_max_bytes(u32::MAX), BACKFILL_CEILING_BYTES);
        // Saturating, not wrapping. See the note above on why this input and
        // not `u32::MAX`.
        assert_eq!(
            backfill_max_bytes(1 << 26),
            BACKFILL_CEILING_BYTES,
            "67108864 lines x 64 bytes is exactly 2^32; a wrapping multiply \
             makes that 0 and hands the deepest scrollback the shallowest \
             backfill"
        );
    }

    /// The operator's setting must actually reach the wire.
    ///
    /// Every other guard here exercises `backfill_max_bytes` with a literal or
    /// with `SCROLLBACK_STEPS`, so all of them stay green if `reconcile` calls
    /// it with a constant. That restores this ticket's original defect one
    /// layer up: the function is correct, the caption is correct, the four
    /// steps are distinct, and the number the operator picked still never
    /// leaves the settings sheet. Found by hunting for a mutation that escapes
    /// rather than confirming one that does not, after the same shape turned
    /// up in four other agents' suites in one afternoon.
    ///
    /// This caption has been wrong twice, and both times the failure was a
    /// promise no code kept. A budget computed from a literal would make it
    /// wrong a third time while every test above still passed.
    ///
    /// Reads the shipped source because the send site is inside a Dioxus
    /// component path with no seam to call. Anchored on tokens rather than on
    /// quote pairing, which `GOAL.md` records as the way this codebase's other
    /// source scans go wrong.
    #[test]
    fn the_send_site_computes_the_budget_from_the_operators_setting() {
        let main_src = crate::testkit::shell();
        let main_src = main_src.as_str();

        let at = main_src
            .find("ClientMsg::Scrollback")
            .expect("the client no longer asks the daemon for scrollback at all");
        let end = main_src[at..]
            .find("});")
            .map(|i| at + i)
            .expect("the Scrollback send site is no longer a struct literal");
        let site = &main_src[at..end];

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
            .unwrap_or_else(|| {
                panic!("max_bytes is `{arg}`, not a call to `backfill_max_bytes`")
            })
            .trim();

        assert!(
            !inner.is_empty() && !inner.starts_with(|c: char| c.is_ascii_digit()),
            "the budget is computed from the literal `{inner}`, so the setting \
             never reaches the wire and the Scrollback caption is false again"
        );

        // `expect`, never `unwrap_or(0)`. Falling back to 0 would silently
        // widen the scan to the whole file above the send site, so the clause
        // below would keep passing by finding the binding somewhere unrelated
        // instead of reporting that the function it scopes to is gone. A source
        // scan that degrades quietly is the failure mode this suite exists to
        // catch, and it does not get an exemption for being in the suite.
        let from = main_src[..at]
            .rfind("fn reconcile")
            .expect("`reconcile` was renamed, so this guard no longer knows what it is reading");
        assert!(
            main_src[from..end].contains("settings.terminal.scrollback_lines"),
            "`{inner}` is not bound from `settings.terminal.scrollback_lines`, \
             so the backfill is a function of something other than the setting \
             whose caption promises it"
        );
    }

    /// The default's budget has to survive the wire under the name the server
    /// reads, and the ceiling has to stay affordable at the far end of the
    /// bridge.
    ///
    /// Asserted through the encoder rather than against the constant: the
    /// protocol already types `max_bytes` as u32 and Rust enforces that, so a
    /// bare comparison proves nothing. What can go wrong is the field name
    /// drifting, or the number being set too small (a tab switch shows a
    /// fragment of a screen) or too large (an attach becomes a download into
    /// the shared web process).
    #[test]
    fn the_backfill_request_crosses_the_wire_as_max_bytes() {
        /// One screen of dense output, measured on the corpus.
        const DENSE_SCREEN: u64 = 20 * 1024;
        /// Bytes of JSON per payload byte. `BridgeCmd::Backfill` carries the
        /// history to JavaScript as an array of integers, measured at 3.6x on
        /// a real coloured stream and rounded up.
        const JSON_PER_BYTE: u64 = 4;
        /// What one `JSON.parse` on a tab switch may cost the shared process.
        const JSON_BUDGET: u64 = 8 * 1024 * 1024;

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
        assert!(
            u64::from(BACKFILL_CEILING_BYTES) * JSON_PER_BYTE <= JSON_BUDGET,
            "the largest backfill ships {} MB of JSON into the one web process \
             every window shares",
            u64::from(BACKFILL_CEILING_BYTES) * JSON_PER_BYTE / (1024 * 1024)
        );
    }
}
