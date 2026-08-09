//! Wire contract between the vitrum session server and its clients.
//!
//! The protocol is deliberately split across two transports on one connection:
//!
//! - **Control plane**: JSON text frames carrying [`ClientMsg`] and [`ServerMsg`].
//!   Low rate, human-debuggable, versioned by [`PROTOCOL_VERSION`].
//! - **Data plane**: binary frames carrying raw PTY bytes, encoded by
//!   [`encode_output`] and read back by [`decode_output`].
//!
//! The split exists because PTY output is a firehose of arbitrary bytes. JSON
//! strings must be valid UTF-8, so routing output through the control plane
//! would force base64 (a 33% size tax plus an encode/decode pass) on the single
//! hottest path in the product, and would additionally corrupt any byte
//! sequence that is not valid UTF-8. A terminal must forward bytes verbatim:
//! partial UTF-8 sequences legitimately straddle read boundaries, and mouse and
//! DEC responses are not text at all.
//!
//! One control-plane message carries arbitrary bytes anyway:
//! [`ServerMsg::ScrollbackChunk`], which answers a deliberate gesture rather
//! than the firehose. It is base64 by [`b64`], not serde's default integer
//! array, because that default measures 3.6 bytes of JSON per payload byte.
//! Avoiding a 33% tax on the hot path while paying 260% on the cold one was
//! backwards.

pub mod b64;
pub mod text;
pub use text::{display_safe, error_text, is_display_safe, MAX_ERROR_CHARS};

use serde::{Deserialize, Serialize};

/// Control-plane schema version. Bump only when old clients and servers must
/// refuse each other; additive fields do not warrant a bump because both sides
/// tolerate unknown fields.
pub const PROTOCOL_VERSION: u32 = 2;

/// Byte length of a data-plane frame header: kind + session + seq.
pub const OUTPUT_HEADER_LEN: usize = 1 + 8 + 8;

/// Data-plane frame kind for PTY output.
pub const FRAME_KIND_OUTPUT: u8 = 1;

/// Identifier for a live or exited terminal session, assigned by the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub u64);

/// Identifier for a project, which is a named working root grouping sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProjectId(pub u64);

/// Lifecycle state of a session's child process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum SessionStatus {
    /// Spawned, no output observed yet.
    Starting,
    /// Child is alive.
    Running,
    /// Child exited. `code` is `None` when the process was signalled.
    Exited { code: Option<i32> },
}

impl SessionStatus {
    /// True while the child process may still produce output.
    ///
    /// Callers use this to decide whether to keep a PTY reader task alive, so
    /// it must be false for exactly the terminal states.
    pub fn is_live(&self) -> bool {
        matches!(self, SessionStatus::Starting | SessionStatus::Running)
    }
}

/// A named working root that groups sessions in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    pub id: ProjectId,
    pub name: String,
    pub root: String,
}

/// Why a session might need the operator, derived entirely from the PTY.
///
/// This is the sidebar's ordering key at scale. With twenty concurrent agents a
/// flat, creation-ordered list is unusable: the one session that is blocked on
/// a question is indistinguishable from nineteen that are happily working.
///
/// Every signal here is harness-agnostic ON PURPOSE. A GUI that reads a
/// harness's structured event stream can only know an agent wants attention for
/// harnesses it has been taught, which is why competing shells support a fixed
/// list of agents. We host a real PTY, so a bell is a bell and an exit code is
/// an exit code for Claude Code, Codex, an unknown agent, or `make`.
///
/// Nothing here guesses. "Blocked reading stdin" is deliberately NOT modelled:
/// detecting it needs per-platform process introspection that does not work
/// uniformly across Linux, macOS and Windows, and a signal that is right on one
/// platform and silently wrong on another is worse than no signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attention {
    /// The child emitted BEL (0x07) or an OSC 777 notification since the client
    /// last focused this session. In-band, universal, and the conventional way
    /// a terminal program asks for a human.
    pub bell: bool,
    /// Milliseconds since this session last produced output THAT THE OPERATOR
    /// HAS NOT SEEN. The server reports 0 once the session has been focused
    /// since its last output.
    ///
    /// The "unseen" qualifier is the whole point. A plain time-since-output
    /// would light up a session you read five seconds ago and never turn off,
    /// and a permanently lit indicator trains people to ignore the indicator.
    /// Qualified this way it means "this agent stopped and you have not looked",
    /// which is exactly the end-of-turn moment an operator wants surfaced, and
    /// unlike a spinner it costs nothing to compute and nothing to draw.
    pub idle_ms: u64,
    /// The child exited with a nonzero code or was signalled.
    pub failed: bool,
    /// Whether the session's FOREGROUND process is blocked reading the
    /// terminal, resolved by asking the operating system rather than the agent.
    ///
    /// `None` means the platform cannot answer, which is NOT the same as
    /// `Some(false)` and must never be rendered as "working". Windows ConPTY
    /// has no equivalent of the Linux `/proc/<pid>/syscall` or macOS `libproc`
    /// query, so it reports `None` and the UI says so.
    ///
    /// This is measured, not guessed. On a live daemon, a shell at a prompt
    /// blocks in `pselect6` and a `read -p` blocks in `read(fd 0)`, while
    /// `clock_nanosleep`, `wait4` and a running process are all plainly
    /// working. Syscall numbers are ABI-stable, unlike `wchan` strings.
    ///
    /// It is also why we beat a shell that parses per-harness event streams:
    /// the operating system answers this for ANY process, including agents
    /// nobody has ever integrated.
    pub waiting: Option<bool>,
}

impl Attention {
    /// Coarse sort weight for the sidebar: higher sorts nearer the top.
    ///
    /// `vitrum-model` owns the richer five-state derivation used for display;
    /// this is the transport-level rank so a client with no model still orders
    /// sensibly. The two must agree on relative order.
    ///
    /// A failure outranks being blocked on the operator, which outranks a bell,
    /// which outranks silence. Blocked beats bell because a bell is often
    /// incidental (a completion beep) whereas a blocked process has genuinely
    /// stopped making progress until a human acts.
    pub fn priority(&self) -> u8 {
        if self.failed {
            4
        } else if self.waiting == Some(true) {
            3
        } else if self.bell {
            2
        } else if self.idle_ms >= IDLE_ATTENTION_MS {
            1
        } else {
            0
        }
    }

    /// True when the sidebar should surface this session above working ones.
    pub fn wants_operator(&self) -> bool {
        self.priority() > 0
    }
}

/// How long a session must be silent before it counts as wanting attention.
pub const IDLE_ATTENTION_MS: u64 = 30_000;

/// OSC number a harness uses to declare its state to the shell.
///
/// Chosen from the private range and documented so any agent author can adopt
/// it. The full sequence is `ESC ] 7373 ; <state> [; <label>] ST`, where `ST`
/// is either BEL (0x07) or `ESC \`.
pub const HINT_OSC: u32 = 7373;

/// A state a harness can declare about itself.
///
/// These exist because two of the states an operator most wants, "waiting for
/// my approval" and "waiting for my input", CANNOT be derived from a PTY byte
/// stream. A shell that guessed them from output cadence would be wrong often
/// and confidently, which is worse than not offering them.
///
/// So the split is deliberate and honest. [`Attention`] carries what we OBSERVE
/// and is always available, for every agent, including ones that have never
/// heard of us. `AgentHint` carries what an agent CHOOSES TO DECLARE and is
/// always optional. A harness that emits nothing still gets a working sidebar;
/// one that opts in gets a better one. Compare a shell that reads per-harness
/// event streams: it cannot show anything at all for an agent it was not built
/// against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HintState {
    /// Blocked asking the operator to approve an action.
    Approval,
    /// Blocked asking the operator a question.
    Input,
    /// Actively working; no operator action needed.
    Working,
    /// Finished a unit of work and idle.
    Ready,
}

impl HintState {
    /// Parse the state token from a hint sequence.
    ///
    /// Unknown tokens return `None` rather than a default, because a future
    /// agent emitting a state we do not know must be ignored, not silently
    /// misreported as one we do.
    pub fn parse(token: &str) -> Option<Self> {
        match token {
            "approval" => Some(Self::Approval),
            "input" => Some(Self::Input),
            "working" => Some(Self::Working),
            "ready" => Some(Self::Ready),
            _ => None,
        }
    }

    /// Every state, in the order the documentation introduces them.
    ///
    /// Enumerated so a gate can walk the set rather than repeat it. Adding a
    /// variant lands here and then fails whatever reads this until the new
    /// state has been decided about: `hint.rs::token` stops compiling, and the
    /// integration tests go red until each shipped harness either wires the
    /// state up or says in writing why it does not.
    pub const ALL: [HintState; 4] = [
        HintState::Approval,
        HintState::Input,
        HintState::Working,
        HintState::Ready,
    ];

    /// True when the agent has declared it is blocked on the operator.
    pub fn blocks_on_operator(&self) -> bool {
        matches!(self, Self::Approval | Self::Input)
    }
}

/// The most recent state a harness declared about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentHint {
    pub state: HintState,
    /// Short operator-facing label, for example the question being asked.
    pub label: Option<String>,
    pub received_at_ms: u64,
}

/// Everything the sidebar draws for one session.
///
/// This is a projection, not the server's internal session struct: it carries
/// what a client renders and nothing else. Scrollback, the PTY handle, and the
/// child process live only on the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: SessionId,
    pub project_id: ProjectId,
    pub title: String,
    pub cwd: String,
    pub command: String,
    pub args: Vec<String>,
    pub status: SessionStatus,
    pub created_at_ms: u64,
    pub last_activity_ms: u64,
    pub cols: u16,
    pub rows: u16,
    /// Current git branch of `cwd`, when the server has resolved one.
    pub git_branch: Option<String>,
    /// Output arrived since the client last had this session focused.
    pub unread: bool,
    /// Why this session may need the operator. Drives sidebar ordering.
    pub attention: Attention,
    /// The last state this agent declared about itself, when it opted in.
    /// `None` for every agent that has never heard of us, which must stay a
    /// fully supported case rather than a degraded one.
    pub hint: Option<AgentHint>,
    /// The last title the program itself announced, verbatim.
    ///
    /// Separate from [`SessionInfo::title`] because the two are different
    /// facts. `title` is the session's name, which a shell is allowed to write
    /// and an operator is allowed to pin. This is whatever the program last put
    /// in the terminal title bar, which for an agent TUI is a status line
    /// rather than a name: Gemini writes `Ready (kernel-notes)` and Codex
    /// writes `[ ! ] Action Required`. Reading one as the other put an agent's
    /// status in the sidebar twice, once as the row's name, truncated.
    ///
    /// `None` until the program announces something.
    #[serde(default)]
    pub term_title: Option<String>,
}

/// Client to server, control plane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ClientMsg {
    /// First message on a connection. The server replies [`ServerMsg::Welcome`].
    Hello { protocol: u32 },
    /// Request the current project and session lists.
    List,
    /// Create and spawn a new session.
    CreateSession {
        project_id: ProjectId,
        cwd: String,
        command: String,
        args: Vec<String>,
        cols: u16,
        rows: u16,
        title: Option<String>,
    },
    /// Begin receiving live output frames for `session`.
    Attach {
        session: SessionId,
        cols: u16,
        rows: u16,
    },
    /// Stop receiving live output frames for `session`. The session keeps running.
    Detach { session: SessionId },
    /// Write bytes to the session's PTY. Base64 is avoided here because input is
    /// low rate and always originates from a keyboard or paste buffer.
    Input { session: SessionId, data: Vec<u8> },
    /// Resize the session's PTY.
    Resize {
        session: SessionId,
        cols: u16,
        rows: u16,
    },
    /// Set an operator-chosen title for the session.
    ///
    /// This lives in the protocol rather than in one client's local state
    /// because a title the daemon does not know is a title that vanishes on
    /// restart and is invisible to a second window. The server owns session
    /// identity, so it owns the name, and it echoes [`ServerMsg::SessionUpdated`]
    /// so every connected client renames at once.
    Rename { session: SessionId, title: String },
    /// Terminate the child process and drop the session.
    Close { session: SessionId },
    /// Ask for buffered history older than `before_seq`.
    Scrollback {
        session: SessionId,
        before_seq: u64,
        max_bytes: u32,
    },
    /// Search every retained scrollback buffer at once.
    ///
    /// Only the daemon can answer this, because only the daemon holds every
    /// session's bytes. A client has the focused viewport and nothing else.
    /// That asymmetry is the whole feature: "which of my twenty agents hit an
    /// OOM" is one server-side sweep here and impossible anywhere else.
    Search {
        /// Restrict to these sessions, or every session when empty.
        sessions: Vec<SessionId>,
        pattern: String,
        /// Treat `pattern` as a regular expression rather than a literal.
        regex: bool,
        case_insensitive: bool,
        whole_word: bool,
        context_lines: u16,
        max_hits: u32,
    },
    /// Turn same-file collision detection on or off, for the whole daemon.
    ///
    /// A subscription rather than a standing feature, and that is a cost
    /// decision. Detection means one inotify watch per directory under every
    /// session root, and this daemon's headline is that it performs no
    /// syscalls at all while nothing is happening. With nobody subscribed it
    /// holds no watcher, no thread and no watch descriptors, so that claim
    /// survives verbatim for every operator who never turns this on.
    ///
    /// Daemon-wide, not per connection: a collision is between two SESSIONS,
    /// which belong to the daemon, so a second window must see the same
    /// answer as the first. The daemon watches while at least one client asks
    /// it to.
    WatchCollisions { enabled: bool },
    /// Ask for the current report without changing the subscription.
    ///
    /// A window that has just connected renders the contested set immediately
    /// rather than waiting for a change that, in a quiet repository, never
    /// comes.
    Collisions,
}

/// One search hit, projected for rendering.
///
/// `line_seq` is the byte offset of the matched line's first byte within that
/// session's cumulative output, so clicking a hit can scroll the terminal to
/// exactly that point rather than approximately.
///
/// `visible` is the line with escape sequences stripped, because a search for
/// `error` must match a line printed as `\x1b[31merror\x1b[0m` and the operator
/// must be shown text rather than escapes. It is bytes rather than a string on
/// purpose: lossy UTF-8 decoding turns one invalid byte into a three-byte
/// replacement character, which shifts every offset after it and makes the
/// highlight range point at the wrong substring, on exactly the lines that are
/// hardest to debug.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub session: SessionId,
    pub line_seq: u64,
    #[serde(with = "crate::b64::bytes")]
    pub visible: Vec<u8>,
    /// Byte range of the match within `visible`.
    pub match_start: u32,
    pub match_end: u32,
    #[serde(with = "crate::b64::bytes_seq")]
    pub before: Vec<Vec<u8>>,
    #[serde(with = "crate::b64::bytes_seq")]
    pub after: Vec<Vec<u8>>,
}

/// How a change to a file was pinned on a session.
///
/// Carried rather than flattened to a boolean because the operator is about
/// to decide whether to interrupt an agent, and "we watched it hold the file
/// open" and "we think it was probably that one" are different claims. The
/// daemon never guesses silently: a change it cannot pin on anybody is
/// counted in [`CollisionSession::unattributed`] and appears in no
/// participant list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Credit {
    /// The session's process tree was seen holding the file open at the
    /// moment the change landed. The strongest evidence available.
    Observed,
    /// No process had it open by the time we looked, and this session is the
    /// only one that has written this file recently. An inference, labelled.
    Inferred,
}

impl Credit {
    /// Whether this credit is a guess rather than an observation.
    ///
    /// The UI hedges on it. A row that reads the same for "we watched it
    /// write the file" and "we think it did" will eventually be used to
    /// interrupt the wrong agent.
    #[must_use]
    pub fn is_inferred(self) -> bool {
        matches!(self, Credit::Inferred)
    }
}

/// One session's history with one contested file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollisionParticipant {
    pub session: SessionId,
    /// Unix millisecond of this session's first recorded change to the file.
    pub first_ms: u64,
    /// Unix millisecond of its most recent one.
    pub last_ms: u64,
    /// Changes recorded for this session on this file.
    pub writes: u32,
    pub credit: Credit,
}

/// One file that two or more live sessions have both changed.
///
/// The whole point of the feature is in that sentence. Not the same
/// repository, not the same directory, not "both ran cargo". Ten agents in a
/// large checkout usually do not conflict, so a warning that fires on shared
/// repositories fires constantly and means nothing. This fires on the failure
/// that actually costs work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Collision {
    /// Absolute path of the contested file.
    pub path: String,
    /// Every live session that changed it, ordered by session id.
    ///
    /// Always two or more. A single-participant entry is not a collision and
    /// the daemon never sends one.
    pub participants: Vec<CollisionParticipant>,
}

/// What detection knows about one session, including what it does not know.
///
/// The counters exist so a client can never render a confident "nothing is
/// colliding". A session with a large `unattributed` count is one whose
/// writes were mostly too short to catch, and claiming its files are
/// uncontested would be a claim the daemon cannot support.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollisionSession {
    pub session: SessionId,
    /// Directory being watched for this session.
    pub root: String,
    /// Files currently retained for it. Bounded, so this is a window rather
    /// than a total.
    pub tracked_paths: u32,
    /// Changes under this root that could not be pinned on anybody.
    ///
    /// Not "changes we missed from this session": we do not know whose they
    /// were. It is the honest denominator beside the attributed count.
    pub unattributed: u64,
}


/// Server to client, control plane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ServerMsg {
    Welcome {
        protocol: u32,
        server_version: String,
    },
    /// Struct variants, not newtype variants. Serde's internally tagged
    /// representation must merge the variant's content into the same JSON map
    /// as the `t` discriminator, and a sequence has no keys to merge, so
    /// `Projects(Vec<_>)` fails at RUNTIME with "cannot serialize tagged
    /// newtype variant containing a sequence". Naming the field gives the
    /// sequence a key and makes it representable.
    Projects { projects: Vec<ProjectInfo> },
    Sessions { sessions: Vec<SessionInfo> },
    SessionCreated(SessionInfo),
    SessionUpdated(SessionInfo),
    /// A session left the registry and must be removed from the sidebar.
    ///
    /// Distinct from [`ServerMsg::Exited`], which says the CHILD died while the
    /// session stays listed with its scrollback intact. This says the session
    /// itself is gone. Without it a client can only learn of a removal by
    /// re-listing, so a closed session lingers forever and clicking it reports
    /// a session the daemon has never heard of.
    SessionRemoved { session: SessionId },
    /// A replay chunk answering [`ClientMsg::Scrollback`]. `more` is false once
    /// the client has reached the oldest retained byte.
    ///
    /// `data` is base64, not serde's default integer array: this is the one
    /// control-plane field carrying raw PTY bytes, and at the 2 MiB backfill
    /// ceiling the array form is a 7.5 MB JSON string that `JSON.parse` turns
    /// into two million transient JavaScript numbers.
    ScrollbackChunk {
        session: SessionId,
        from_seq: u64,
        #[serde(with = "crate::b64::bytes")]
        data: Vec<u8>,
        more: bool,
    },
    /// Answer to [`ClientMsg::Search`]. `truncated` is true when the hit cap
    /// stopped the sweep, so the UI can say "first N" rather than imply these
    /// are all of them.
    SearchResults {
        pattern: String,
        hits: Vec<SearchHit>,
        truncated: bool,
        /// Bytes swept, so the UI can show what was actually searched.
        bytes_scanned: u64,
    },
    /// Which files two or more live sessions are both changing.
    ///
    /// Published to every client whenever the contested set changes, and also
    /// sent in answer to [`ClientMsg::Collisions`] and to a
    /// [`ClientMsg::WatchCollisions`] that flips the subscription.
    CollisionReport {
        /// Whether the daemon is watching at all.
        ///
        /// False means the lists below are empty because nobody looked, not
        /// because nothing collides. A client that renders those two the same
        /// way is telling the operator their agents are safe when nothing has
        /// checked, which is the one answer this feature must never give.
        watching: bool,
        collisions: Vec<Collision>,
        /// Per-session counters, including the unattributed count that
        /// qualifies an empty `collisions` list.
        sessions: Vec<CollisionSession>,
        /// Ways detection is currently incomplete, each a finished sentence.
        ///
        /// Non-empty means absence of collisions is not evidence of absence.
        /// A tree that could not be watched, a kernel queue that overflowed,
        /// and a platform with no per-process open-file query all land here
        /// rather than being swallowed.
        degraded: Vec<String>,
    },
    Exited {
        session: SessionId,
        code: Option<i32>,
    },
    /// Marked `#[non_exhaustive]` so no other crate can build one directly.
    ///
    /// This is the enforcement, not a convention: the sanitising bound in
    /// [`ServerMsg::error`] is worth exactly as much as the guarantee that
    /// there is no way around it. Every one of these was once built inline at
    /// five call sites, and the sixth would have been written the same way.
    #[non_exhaustive]
    Error {
        session: Option<SessionId>,
        message: String,
    },
}

impl ServerMsg {
    /// The only way an error should reach a client.
    ///
    /// Every one of these strings is built by formatting untrusted input back
    /// at the operator: a path, a command, a branch, a reason from the OS
    /// carrying any of them. Constructing the variant directly skips both the
    /// sanitiser and the bound, so callers go through here and the guarantee
    /// holds for errors that do not exist yet.
    pub fn error(session: Option<SessionId>, message: impl AsRef<str>) -> Self {
        ServerMsg::Error {
            session,
            message: text::error_text(message.as_ref()),
        }
    }
}

/// Errors from decoding a data-plane frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    /// Fewer bytes than [`OUTPUT_HEADER_LEN`].
    TooShort { len: usize },
    /// Leading kind byte is not a kind this version understands.
    UnknownKind(u8),
}

impl core::fmt::Display for FrameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FrameError::TooShort { len } => {
                write!(f, "frame is {len} bytes, need at least {OUTPUT_HEADER_LEN}")
            }
            FrameError::UnknownKind(k) => write!(f, "unknown frame kind {k}"),
        }
    }
}

impl core::error::Error for FrameError {}

/// Encode one PTY output frame: `[kind][session: u64 LE][seq: u64 LE][payload]`.
///
/// `seq` is the byte offset of `payload[0]` within the session's output stream,
/// which is what lets a reconnecting client ask for exactly the range it missed
/// and lets the server detect a gap rather than silently splicing.
pub fn encode_output(session: SessionId, seq: u64, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(OUTPUT_HEADER_LEN + payload.len());
    encode_output_into(&mut out, session, seq, payload);
    out
}

/// Append a frame to `out` instead of allocating a fresh one.
///
/// The output pump encodes one frame per PTY read, so a per-frame `Vec` is an
/// allocation on the hottest path in the daemon. A pump that keeps one buffer
/// and clears it per frame pays none.
pub fn encode_output_into(out: &mut Vec<u8>, session: SessionId, seq: u64, payload: &[u8]) {
    out.reserve(OUTPUT_HEADER_LEN + payload.len());
    out.push(FRAME_KIND_OUTPUT);
    out.extend_from_slice(&session.0.to_le_bytes());
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(payload);
}

/// Decode a frame produced by [`encode_output`], borrowing the payload.
///
/// Total: every rejected shape has a named error and there is no panicking
/// path, because the bytes arrive from a socket.
pub fn decode_output(frame: &[u8]) -> Result<(SessionId, u64, &[u8]), FrameError> {
    if frame.len() < OUTPUT_HEADER_LEN {
        return Err(FrameError::TooShort { len: frame.len() });
    }
    let Some((&kind, rest)) = frame.split_first() else {
        return Err(FrameError::TooShort { len: frame.len() });
    };
    if kind != FRAME_KIND_OUTPUT {
        return Err(FrameError::UnknownKind(kind));
    }
    // Fixed-size chunks rather than `try_into().expect(..)`: the length check
    // above already proves these succeed, and a proof the compiler checks is
    // worth more here than one in a panic message.
    let Some((session, rest)) = rest.split_first_chunk::<8>() else {
        return Err(FrameError::TooShort { len: frame.len() });
    };
    let Some((seq, payload)) = rest.split_first_chunk::<8>() else {
        return Err(FrameError::TooShort { len: frame.len() });
    };
    Ok((
        SessionId(u64::from_le_bytes(*session)),
        u64::from_le_bytes(*seq),
        payload,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The workspace version and the internal dependency versions must agree.
    ///
    /// `[workspace.dependencies]` gives each internal crate both a path and a
    /// literal `version`, because `cargo package` refuses a path dependency
    /// without one. Cargo has no `version.workspace = true` there, so the
    /// literal is a second copy of a number that already exists ten lines
    /// above it. Bump one and not the other and every crate publishes with a
    /// requirement no published crate satisfies: the upload succeeds and
    /// nothing can depend on the result.
    #[test]
    fn every_internal_dependency_requires_the_version_being_published() {
        let manifest = include_str!("../../../Cargo.toml");
        let declared = manifest
            .split("[workspace.package]")
            .nth(1)
            .and_then(|rest| rest.lines().find_map(|l| l.strip_prefix("version = ")))
            .map(|v| v.trim().trim_matches('"'))
            .expect("the workspace declares a version");

        let mut checked = 0;
        for line in manifest.lines() {
            let Some((name, rest)) = line.split_once(" = { path = \"crates/") else {
                continue;
            };
            let want = format!("version = \"{declared}\"");
            assert!(
                rest.contains(&want),
                "{name} is published as {declared} but requires {rest}"
            );
            checked += 1;
        }
        assert_eq!(checked, 9, "the set of internal dependencies changed");
    }

    /// The publish list must name every crate AFTER the crates it depends on.
    ///
    /// This is the half the count above cannot see. `cargo publish` resolves a
    /// crate's dependencies from the registry and nowhere else, so publishing
    /// `vitrum` before `vitrum-proto` does not fail late or produce a broken
    /// crate: it fails on the first upload, having already released whatever
    /// came before it at a version crates.io will never hand back. The order is
    /// the only part of a release that cannot be corrected afterwards.
    ///
    /// Proven locally as well: `cargo package -p vitrum` fails today with "no
    /// matching package named `vitrum-dioxus-desktop` found", because the fork
    /// is not on the registry yet. That is the same failure, one crate early.
    ///
    /// The graph is read from `path` dependencies rather than from names, so a
    /// dependency renamed on the way in, as `portable-pty` is, still counts.
    #[test]
    fn the_publish_list_names_a_crate_after_everything_it_depends_on() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("the crate sits two levels under the workspace root");
        let manifest = std::fs::read_to_string(root.join("Cargo.toml"))
            .expect("the workspace manifest is readable");

        // Every member directory, and the package name it publishes under.
        let dirs: Vec<String> = manifest
            .split("members = [")
            .nth(1)
            .and_then(|rest| rest.split_once(']'))
            .expect("the workspace lists members")
            .0
            .lines()
            .filter_map(|l| l.split('"').nth(1))
            .map(str::to_string)
            .collect();
        let package_of = |dir: &str| -> String {
            let text = std::fs::read_to_string(root.join(dir).join("Cargo.toml"))
                .unwrap_or_else(|e| panic!("member {dir} has no manifest: {e}"));
            text.lines()
                .find_map(|l| l.trim().strip_prefix("name = "))
                .map(|n| n.trim().trim_matches('"').to_string())
                .unwrap_or_else(|| panic!("member {dir} declares no package name"))
        };

        // `dir -> package` for members, and `path -> package` so a workspace
        // dependency can be resolved to the member it points at.
        let by_dir: Vec<(String, String)> =
            dirs.iter().map(|d| (d.clone(), package_of(d))).collect();

        // The workspace declares every internal dependency once, by path. The
        // key a member writes may differ from the package name.
        let mut key_to_package: Vec<(String, String)> = Vec::new();
        for line in manifest.lines() {
            let Some((key, rest)) = line.split_once(" = {") else {
                continue;
            };
            let Some(path) = rest.split("path = \"").nth(1).and_then(|r| r.split('"').next())
            else {
                continue;
            };
            if let Some((_, package)) = by_dir.iter().find(|(dir, _)| dir == path) {
                key_to_package.push((key.trim().to_string(), package.clone()));
            }
        }
        assert!(
            !key_to_package.is_empty(),
            "no internal path dependencies were found, so this test proves nothing"
        );

        // What each member depends on, as package names.
        let mut edges: Vec<(String, String)> = Vec::new();
        for (dir, package) in &by_dir {
            let text = std::fs::read_to_string(root.join(dir).join("Cargo.toml"))
                .expect("a member manifest just read is still readable");
            for line in text.lines() {
                let key = line
                    .trim()
                    .split_once(".workspace")
                    .or_else(|| line.trim().split_once(" = {"))
                    .map(|(k, _)| k.trim());
                let Some(key) = key else { continue };
                if let Some((_, dep)) = key_to_package.iter().find(|(k, _)| k == key) {
                    edges.push((package.clone(), dep.clone()));
                }
            }
        }

        let workflow = std::fs::read_to_string(root.join(".github/workflows/publish.yml"))
            .expect("the publish workflow is readable");
        let order: Vec<String> = workflow
            .split("for c in ")
            .nth(1)
            .and_then(|rest| rest.split_once("; do"))
            .expect("the first publish loop closes")
            .0
            .split_whitespace()
            .filter(|w| *w != "\\")
            .map(str::to_string)
            .collect();
        let position = |name: &str| order.iter().position(|c| c == name);

        for (crate_name, dep) in &edges {
            let Some(at) = position(crate_name) else {
                // The count test owns coverage; a member that opts out of
                // publishing has no position and nothing to order.
                continue;
            };
            let Some(dep_at) = position(dep) else {
                panic!(
                    "{crate_name} depends on {dep}, which publish.yml never publishes, \
                     so {crate_name} cannot resolve it from the registry"
                );
            };
            assert!(
                dep_at < at,
                "publish.yml publishes {crate_name} at position {at} but its dependency \
                 {dep} at {dep_at}; the release fails on {crate_name} with the earlier \
                 crates already uploaded and their versions gone: {order:?}"
            );
        }
    }

    /// The measurement harness speaks the version this crate defines.
    ///
    /// `harness/remote/sessions.py` opens a real connection to a real daemon,
    /// which is the point of it, and the daemon refuses any protocol number but
    /// its own. Bumping `PROTOCOL_VERSION` here without bumping it there breaks
    /// every measurement run at the handshake, and it did: after the scrollback
    /// frames moved to base64 the harness failed with "unsupported protocol 1;
    /// this server speaks 2" and stayed broken, because nothing in the build
    /// reads that file.
    #[test]
    fn the_harness_speaks_this_protocol_version() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("the crate sits two levels under the workspace root");
        let sessions = std::fs::read_to_string(root.join("harness/remote/sessions.py"))
            .expect("the harness client is readable");
        let declared = sessions
            .lines()
            .find_map(|l| l.strip_prefix("PROTOCOL_VERSION = "))
            .expect("the harness declares PROTOCOL_VERSION")
            .trim()
            .parse::<u32>()
            .expect("it declares a number");
        assert_eq!(
            declared, PROTOCOL_VERSION,
            "harness/remote/sessions.py speaks protocol {declared}, this crate speaks \
             {PROTOCOL_VERSION}; the daemon will refuse the handshake"
        );
    }

    /// Every workspace member must be in both publish lists, in one order.
    ///
    /// `cargo publish` resolves each crate's dependencies from the registry, so
    /// the list in `publish.yml` is a topological sort and a crate missing from
    /// it is not a missing upload: it is the next crate in the list failing to
    /// resolve, halfway through a release that cannot be rolled back because
    /// crates.io does not free a version.
    ///
    /// The count is the check, not the names, because the failure being caught
    /// is adding a member and not thinking about publishing at all. Names would
    /// need this test edited by the same person in the same commit, which is
    /// exactly the step that gets skipped.
    ///
    /// A member that opts out with `publish = false` is not counted. Not every
    /// member is a product: a test harness belongs in the workspace and not on
    /// crates.io, and the opt-out is the deliberate decision this test wants,
    /// so it accepts either answer and only rejects silence.
    ///
    /// The two lists are compared to each other as well. A dry run that
    /// packages a different set from what the publish step uploads verifies
    /// nothing about the release it is supposed to be rehearsing.
    #[test]
    fn the_publish_workflow_covers_every_workspace_member() {
        let manifest = include_str!("../../../Cargo.toml");
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("the crate sits two levels under the workspace root");
        let members = manifest
            .split("members = [")
            .nth(1)
            .and_then(|rest| rest.split_once(']'))
            .expect("the workspace lists members")
            .0
            .lines()
            .filter_map(|l| l.split('"').nth(1))
            .filter(|dir| {
                let path = root.join(dir).join("Cargo.toml");
                let text = std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("member {dir} has no manifest at {path:?}: {e}"));
                !text.contains("publish = false")
            })
            .count();

        let workflow = include_str!("../../../.github/workflows/publish.yml");
        let lists: Vec<Vec<&str>> = workflow
            .split("for c in ")
            .skip(1)
            .map(|rest| {
                rest.split_once("; do")
                    .expect("each loop closes")
                    .0
                    .split_whitespace()
                    .filter(|w| *w != "\\")
                    .collect()
            })
            .collect();

        assert_eq!(lists.len(), 2, "publish.yml no longer has a dry run and a publish");
        assert_eq!(
            lists[0], lists[1],
            "the dry run packages a different set of crates from the publish step"
        );
        assert_eq!(
            lists[0].len(),
            members,
            "the workspace has {members} members and publish.yml names {}: {:?}",
            lists[0].len(),
            lists[0]
        );
    }

    /// A frame must survive the round trip byte for byte.
    ///
    /// This is the contract the whole data plane rests on: the terminal renders
    /// exactly the bytes the child wrote. Any mangling here shows up as garbled
    /// escape sequences that are extremely hard to trace back to the transport.
    #[test]
    fn output_frame_round_trips_payload_verbatim() {
        let payload = b"\x1b[31mred\x1b[0m \xe2\x9c\x93";
        let frame = encode_output(SessionId(7), 4096, payload);
        let (session, seq, got) = decode_output(&frame).expect("valid frame decodes");
        assert_eq!(session, SessionId(7));
        assert_eq!(seq, 4096);
        assert_eq!(got, payload);
    }

    /// Non-UTF-8 payloads must pass through untouched.
    ///
    /// This test exists because the obvious implementation (put output in the
    /// JSON control plane as a String) silently corrupts these bytes. A lone
    /// 0x80 continuation byte is exactly what a UTF-8 sequence split across two
    /// PTY reads looks like, and it occurs constantly under load.
    #[test]
    fn output_frame_preserves_invalid_utf8() {
        let payload = &[0x80, 0xff, 0xfe, 0x00, 0x1b];
        let frame = encode_output(SessionId(1), 0, payload);
        let (_, _, got) = decode_output(&frame).expect("valid frame decodes");
        assert_eq!(got, payload);
        // Asserted on what came BACK, not on the literal. Two reasons: the
        // round trip having preserved the invalidity is the actual claim, and
        // `from_utf8` over a literal is a compile-time constant that rustc
        // warns about because it can never fail. The guard still does its job,
        // which is to stop someone replacing this payload with something
        // valid and leaving a test that proves nothing.
        assert!(
            std::str::from_utf8(got).is_err(),
            "the decoded payload is valid UTF-8, so this test no longer covers \
             the corruption it exists for"
        );
    }

    /// An empty payload is a legal frame, not an error.
    ///
    /// Readers can legitimately produce a zero-length read, and treating that as
    /// a decode failure would tear down a healthy session.
    #[test]
    fn empty_payload_is_a_valid_frame() {
        let frame = encode_output(SessionId(3), 12, b"");
        let (session, seq, got) = decode_output(&frame).expect("empty payload is valid");
        assert_eq!((session, seq), (SessionId(3), 12));
        assert!(got.is_empty());
    }

    /// WHY: `encode_output_into` exists so the output pump can keep one buffer
    /// instead of allocating a `Vec` per PTY read. A second encoder is a second
    /// place the wire format can drift, and it must also append rather than
    /// overwrite, or a reused buffer silently corrupts the previous frame.
    #[test]
    fn the_reusable_encoder_writes_the_same_bytes_and_appends() {
        let payload = b"\x1b[31mred\x1b[0m \xe2\x9c\x93\x00\xff";
        let expected = encode_output(SessionId(7), 4096, payload);

        let mut buffer = Vec::new();
        encode_output_into(&mut buffer, SessionId(7), 4096, payload);
        assert_eq!(buffer, expected, "the two encoders must agree byte for byte");

        // A second frame appended to the same buffer must leave the first
        // intact and sit immediately after it.
        let second = encode_output(SessionId(8), 0, b"more");
        encode_output_into(&mut buffer, SessionId(8), 0, b"more");
        assert_eq!(&buffer[..expected.len()], &expected[..]);
        assert_eq!(&buffer[expected.len()..], &second[..]);

        // And the reused buffer still decodes, once cleared, exactly as before.
        buffer.clear();
        encode_output_into(&mut buffer, SessionId(2), u64::MAX, b"");
        assert_eq!(
            decode_output(&buffer),
            Ok((SessionId(2), u64::MAX, &b""[..]))
        );
    }

    /// A truncated frame must be rejected rather than read out of bounds.
    ///
    /// Guards the slice indexing in `decode_output` against a short or partial
    /// frame delivered by a misbehaving or truncating transport.
    #[test]
    fn truncated_frame_is_rejected_at_every_length() {
        let full = encode_output(SessionId(9), 1, b"x");
        for len in 0..OUTPUT_HEADER_LEN {
            assert_eq!(
                decode_output(&full[..len]),
                Err(FrameError::TooShort { len }),
                "length {len} must be rejected"
            );
        }
    }

    /// An unrecognized kind byte must be reported, not misparsed as output.
    ///
    /// Locks the forward-compatibility boundary: when a future server adds a
    /// second frame kind, an old client must say so instead of rendering the
    /// header of an unrelated frame into the user's terminal.
    #[test]
    fn unknown_frame_kind_is_rejected() {
        let mut frame = encode_output(SessionId(1), 0, b"payload");
        frame[0] = 0xAB;
        assert_eq!(decode_output(&frame), Err(FrameError::UnknownKind(0xAB)));
    }

    /// Sequence numbers must survive beyond 32 bits.
    ///
    /// A long-lived agent session streaming output for days will exceed 4 GiB of
    /// cumulative output; truncating the offset to u32 would silently alias old
    /// scrollback onto new and corrupt replay after reconnect.
    #[test]
    fn sequence_numbers_survive_past_u32() {
        let seq = u64::from(u32::MAX) + 12_345;
        let frame = encode_output(SessionId(1), seq, b"z");
        let (_, got, _) = decode_output(&frame).expect("valid frame decodes");
        assert_eq!(got, seq);
    }

    /// Control-plane messages must round trip through JSON with their tag.
    ///
    /// The `t` discriminator is what lets both ends switch on message type and
    /// keep a tolerant default arm; losing it turns an unknown future variant
    /// into a hard parse failure instead of an ignorable message.
    #[test]
    fn client_msg_round_trips_as_tagged_json() {
        let msg = ClientMsg::Resize {
            session: SessionId(42),
            cols: 200,
            rows: 46,
        };
        let json = serde_json::to_string(&msg).expect("serializes");
        assert!(json.contains("\"t\":\"resize\""), "tagged: {json}");
        let back: ClientMsg = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, msg);
    }

    /// A session projection must round trip with its status payload intact.
    ///
    /// `Exited { code: None }` (signalled) and `Exited { code: Some(0) }` (clean)
    /// are different facts the sidebar renders differently, so the encoding must
    /// not collapse them.
    #[test]
    fn session_status_distinguishes_signalled_from_clean_exit() {
        for status in [
            SessionStatus::Exited { code: None },
            SessionStatus::Exited { code: Some(0) },
            SessionStatus::Running,
        ] {
            let json = serde_json::to_string(&status).expect("serializes");
            let back: SessionStatus = serde_json::from_str(&json).expect("deserializes");
            assert_eq!(back, status, "round trip changed {status:?}");
        }
        assert_ne!(
            SessionStatus::Exited { code: None },
            SessionStatus::Exited { code: Some(0) }
        );
    }

    /// Only the two pre-exit states count as live.
    ///
    /// The server keys PTY reader-task teardown off this predicate, so a wrong
    /// answer either leaks a task per session or truncates output at exit.
    #[test]
    fn only_pre_exit_states_are_live() {
        assert!(SessionStatus::Starting.is_live());
        assert!(SessionStatus::Running.is_live());
        assert!(!SessionStatus::Exited { code: Some(0) }.is_live());
        assert!(!SessionStatus::Exited { code: None }.is_live());
    }

    /// Unknown fields on an incoming message must not break parsing.
    ///
    /// A newer server will add fields to projections; an older client must keep
    /// working rather than dropping the connection on first contact.
    #[test]
    fn unknown_fields_are_tolerated() {
        let json = r#"{"t":"welcome","protocol":1,"serverVersion":"0.1.0","futureField":true}"#;
        let msg: ServerMsg = serde_json::from_str(json).expect("tolerates unknown fields");
        assert_eq!(
            msg,
            ServerMsg::Welcome {
                protocol: 1,
                server_version: "0.1.0".to_string()
            }
        );
    }

    fn sample_project() -> ProjectInfo {
        ProjectInfo {
            id: ProjectId(1),
            name: "santh".to_string(),
            root: "/srv/santh".to_string(),
        }
    }

    fn sample_session() -> SessionInfo {
        SessionInfo {
            id: SessionId(5),
            project_id: ProjectId(1),
            title: "claude".to_string(),
            cwd: "/srv/santh".to_string(),
            command: "claude".to_string(),
            args: vec!["--resume".to_string()],
            status: SessionStatus::Running,
            created_at_ms: 1_700_000_000_000,
            last_activity_ms: 1_700_000_000_500,
            cols: 200,
            rows: 46,
            git_branch: Some("main".to_string()),
            unread: true,
            attention: Attention {
                bell: true,
                idle_ms: 45_000,
                failed: false,
                waiting: Some(true),
            },
            hint: Some(AgentHint {
                state: HintState::Approval,
                label: Some("run `rm -rf build/`?".to_string()),
                received_at_ms: 1_700_000_000_400,
            }),
            term_title: Some("[ ! ] Action Required - claude".to_string()),
        }
    }

    /// EVERY `ServerMsg` variant must survive a JSON round trip.
    ///
    /// This test exists because `Projects(Vec<_>)` and `Sessions(Vec<_>)` were
    /// originally newtype variants, which serde's INTERNALLY TAGGED
    /// representation cannot serialize at all: the variant content has to merge
    /// into the same map as the `t` discriminator, and a sequence has no keys
    /// to merge. `to_string` returned Err at runtime, and because the original
    /// suite only round-tripped `Welcome`, the two messages that populate the
    /// entire sidebar were broken with a green test run. Enumerating every
    /// variant is the only coverage that makes that class of bug impossible.
    #[test]
    fn every_server_msg_variant_round_trips() {
        let all = vec![
            ServerMsg::Welcome {
                protocol: PROTOCOL_VERSION,
                server_version: "0.1.0".to_string(),
            },
            ServerMsg::Projects {
                projects: vec![sample_project()],
            },
            ServerMsg::Sessions {
                sessions: vec![sample_session()],
            },
            ServerMsg::SessionCreated(sample_session()),
            ServerMsg::SessionUpdated(sample_session()),
            ServerMsg::ScrollbackChunk {
                session: SessionId(5),
                from_seq: 1024,
                data: vec![0x1b, 0x5b, 0x30, 0x6d],
                more: true,
            },
            ServerMsg::Exited {
                session: SessionId(5),
                code: Some(130),
            },
            ServerMsg::Error {
                session: None,
                message: "boom".to_string(),
            },
        ];
        for msg in all {
            let json = serde_json::to_string(&msg)
                .unwrap_or_else(|e| panic!("{msg:?} must serialize, got {e}"));
            let back: ServerMsg = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("{msg:?} must deserialize from {json}, got {e}"));
            assert_eq!(back, msg, "round trip changed the value");
        }
    }

    /// EVERY `ClientMsg` variant must survive a JSON round trip.
    ///
    /// Same reasoning as the server side: a variant whose content cannot be
    /// represented under an internal tag fails only at runtime, so the suite
    /// must construct every one of them rather than a representative sample.
    #[test]
    fn every_client_msg_variant_round_trips() {
        let all = vec![
            ClientMsg::Hello {
                protocol: PROTOCOL_VERSION,
            },
            ClientMsg::List,
            ClientMsg::CreateSession {
                project_id: ProjectId(1),
                cwd: "/srv/santh".to_string(),
                command: "bash".to_string(),
                args: vec!["-l".to_string()],
                cols: 200,
                rows: 46,
                title: Some("shell".to_string()),
            },
            ClientMsg::Attach {
                session: SessionId(5),
                cols: 200,
                rows: 46,
            },
            ClientMsg::Detach {
                session: SessionId(5),
            },
            ClientMsg::Input {
                session: SessionId(5),
                data: vec![0x03],
            },
            ClientMsg::Resize {
                session: SessionId(5),
                cols: 100,
                rows: 30,
            },
            ClientMsg::Rename {
                session: SessionId(5),
                title: "review auth refactor".to_string(),
            },
            ClientMsg::Close {
                session: SessionId(5),
            },
            ClientMsg::Scrollback {
                session: SessionId(5),
                before_seq: 4096,
                max_bytes: 65536,
            },
        ];
        for msg in all {
            let json = serde_json::to_string(&msg)
                .unwrap_or_else(|e| panic!("{msg:?} must serialize, got {e}"));
            let back: ClientMsg = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("{msg:?} must deserialize from {json}, got {e}"));
            assert_eq!(back, msg, "round trip changed the value");
        }
    }

    /// A unit variant must still carry its discriminator.
    ///
    /// `List` has no payload, and a representation that dropped the `t` key
    /// would leave the server unable to tell it from an empty object.
    #[test]
    fn unit_variant_keeps_its_tag() {
        let json = serde_json::to_string(&ClientMsg::List).expect("serializes");
        assert_eq!(json, r#"{"t":"list"}"#);
    }

    /// The sidebar payload must keep every projected field across the wire.
    ///
    /// The sidebar renders status, branch and unread state; silently losing one
    /// of them shows a stale or wrong row rather than failing loudly, so assert
    /// the whole struct rather than that it merely parsed.
    #[test]
    fn session_projection_survives_the_wire_intact() {
        let msg = ServerMsg::Sessions {
            sessions: vec![sample_session()],
        };
        let json = serde_json::to_string(&msg).expect("serializes");
        assert!(json.contains(r#""t":"sessions""#), "tagged: {json}");
        assert!(json.contains(r#""gitBranch":"main""#), "camelCase: {json}");
        let ServerMsg::Sessions { sessions } =
            serde_json::from_str::<ServerMsg>(&json).expect("deserializes")
        else {
            panic!("variant changed");
        };
        assert_eq!(sessions, vec![sample_session()]);
    }

    /// Attention priority must rank failure above bell above silence.
    ///
    /// This ordering IS the sidebar's value proposition at twenty concurrent
    /// agents: the row that needs a human has to float above the nineteen that
    /// are working. Getting the order wrong buries the one session the operator
    /// actually has to act on, which is the exact failure of a flat list.
    #[test]
    fn attention_ranks_failure_above_bell_above_silence() {
        let working = Attention::default();
        let silent = Attention {
            idle_ms: IDLE_ATTENTION_MS,
            ..Attention::default()
        };
        let belled = Attention {
            bell: true,
            ..Attention::default()
        };
        let failed = Attention {
            failed: true,
            ..Attention::default()
        };

        assert_eq!(working.priority(), 0);
        assert!(silent.priority() < belled.priority());
        assert!(belled.priority() < failed.priority());
        assert!(!working.wants_operator());
        for a in [silent, belled, failed] {
            assert!(a.wants_operator(), "{a:?} must surface");
        }
    }

    /// A session that just produced output must never demand attention.
    ///
    /// The idle threshold is a boundary, and an off-by-one here would mark every
    /// actively streaming agent as needing a human, which inverts the feature
    /// and makes the sidebar useless noise.
    #[test]
    fn idle_attention_threshold_is_exclusive_below_and_inclusive_at() {
        let just_under = Attention {
            idle_ms: IDLE_ATTENTION_MS - 1,
            ..Attention::default()
        };
        let exactly_at = Attention {
            idle_ms: IDLE_ATTENTION_MS,
            ..Attention::default()
        };
        assert!(!just_under.wants_operator(), "1ms under must stay quiet");
        assert!(exactly_at.wants_operator(), "at the threshold must surface");
    }

    /// A failing session outranks a belled one even when both signals are set.
    ///
    /// Signals co-occur constantly: an agent that dies often rings the bell on
    /// its way out. Priority must be a total order over the combination, not a
    /// first-match-wins scan whose result depends on field order.
    #[test]
    fn combined_signals_resolve_to_the_most_urgent() {
        let both = Attention {
            bell: true,
            idle_ms: 10 * IDLE_ATTENTION_MS,
            failed: true,
            waiting: Some(true),
        };
        let only_failed = Attention {
            failed: true,
            ..Attention::default()
        };
        assert_eq!(both.priority(), only_failed.priority());
        assert_eq!(both.priority(), 4);
    }

    /// Being blocked on the operator must outrank a bell.
    ///
    /// A bell is frequently incidental, a completion beep or a shell mistake. A
    /// foreground process blocked in `read()` has genuinely stopped making
    /// progress until a human acts. Ranking the beep higher would push the one
    /// session that is actually stuck below noise.
    #[test]
    fn a_blocked_session_outranks_a_bell() {
        let blocked = Attention {
            waiting: Some(true),
            ..Attention::default()
        };
        let belled = Attention {
            bell: true,
            ..Attention::default()
        };
        assert!(blocked.priority() > belled.priority());
        assert!(blocked.wants_operator());
    }

    /// Unknown must never be treated as "not waiting".
    ///
    /// Windows ConPTY cannot answer this question, so it reports `None`. If the
    /// code compared against `false` instead of `Some(true)`, or defaulted the
    /// Option, every Windows session would silently claim it is definitely not
    /// blocked. That is the exact class of per-platform lie we refuse: a signal
    /// that is right on Linux and confidently wrong on Windows.
    #[test]
    fn unknown_waiting_is_not_the_same_as_not_waiting() {
        let unknown = Attention {
            waiting: None,
            ..Attention::default()
        };
        let known_idle = Attention {
            waiting: Some(false),
            ..Attention::default()
        };
        let known_blocked = Attention {
            waiting: Some(true),
            ..Attention::default()
        };

        assert_ne!(unknown, known_idle, "None and Some(false) are distinct");
        assert_eq!(unknown.priority(), known_idle.priority());
        assert!(known_blocked.priority() > unknown.priority());
        assert!(!unknown.wants_operator(), "unknown must not fabricate demand");
        assert_eq!(Attention::default().waiting, None, "default is unknown");
    }

    /// The waiting signal must survive the wire including its unknown state.
    ///
    /// `Option<bool>` has three values and JSON must preserve all three. A
    /// codec that collapsed `null` to `false` would erase the platform-cannot-
    /// answer case on the way to the client.
    #[test]
    fn waiting_round_trips_all_three_states() {
        for state in [None, Some(false), Some(true)] {
            let a = Attention {
                waiting: state,
                ..Attention::default()
            };
            let json = serde_json::to_string(&a).expect("serializes");
            let back: Attention = serde_json::from_str(&json).expect("deserializes");
            assert_eq!(back.waiting, state, "round trip changed {state:?}: {json}");
        }
    }

    /// Attention must survive the wire as camelCase alongside the projection.
    ///
    /// The client sorts on these fields, so a serialization mismatch silently
    /// degrades every session to priority zero and the sidebar stops ordering.
    #[test]
    fn attention_round_trips_inside_session_info() {
        let json = serde_json::to_string(&sample_session()).expect("serializes");
        assert!(json.contains(r#""idleMs":45000"#), "camelCase: {json}");
        let back: SessionInfo = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back.attention, sample_session().attention);
        assert!(back.attention.wants_operator());
    }
    /// An acknowledged failure must be representable: dead process, no demand.
    ///
    /// `SessionStatus` and `Attention` are two independent axes. The left status
    /// dot says what the PROCESS did; the attention marker says whether the
    /// operator still needs to look. This state, exited nonzero but already
    /// read, is the one that proves they cannot be collapsed into a single
    /// signal: collapsing them would make an unacknowledged death
    /// indistinguishable from an acknowledged one, which at twenty agents is the
    /// difference between triage and re-reading everything.
    #[test]
    fn a_read_failure_stays_dead_without_demanding_attention() {
        let acknowledged = SessionInfo {
            status: SessionStatus::Exited { code: Some(101) },
            unread: false,
            attention: Attention::default(),
            ..sample_session()
        };
        assert!(!acknowledged.status.is_live(), "process is dead");
        assert!(
            !acknowledged.attention.wants_operator(),
            "already read, must not demand the operator"
        );

        let unacknowledged = SessionInfo {
            attention: Attention {
                failed: true,
                ..Attention::default()
            },
            ..acknowledged.clone()
        };
        assert!(unacknowledged.attention.wants_operator());
        assert_ne!(acknowledged.attention, unacknowledged.attention);
    }

    /// Every declared state token must parse, and nothing else may.
    ///
    /// An unknown token must return None rather than defaulting. A future agent
    /// emitting a state we do not understand has to be ignored, because
    /// silently reporting it as `Working` would tell the operator an agent is
    /// busy when it is actually blocked asking them a question.
    #[test]
    fn hint_state_parses_exactly_the_declared_tokens() {
        assert_eq!(HintState::parse("approval"), Some(HintState::Approval));
        assert_eq!(HintState::parse("input"), Some(HintState::Input));
        assert_eq!(HintState::parse("working"), Some(HintState::Working));
        assert_eq!(HintState::parse("ready"), Some(HintState::Ready));
        for bad in ["", "Approval", "APPROVAL", "approve", "app", "ready ", " ready", "42"] {
            assert_eq!(HintState::parse(bad), None, "{bad:?} must not parse");
        }
    }

    /// Only approval and input mean the agent is blocked on a human.
    ///
    /// The sidebar promotes blocked sessions above working ones, so a wrong
    /// answer here either buries a session waiting on the operator or floods
    /// the top of the list with agents that are perfectly fine.
    #[test]
    fn only_approval_and_input_block_on_the_operator() {
        assert!(HintState::Approval.blocks_on_operator());
        assert!(HintState::Input.blocks_on_operator());
        assert!(!HintState::Working.blocks_on_operator());
        assert!(!HintState::Ready.blocks_on_operator());
    }

    /// A session with no hint must stay fully representable.
    ///
    /// Every agent that has never heard of us reports `hint: None`, and that is
    /// the COMMON case, not a degraded one. If the projection could not encode
    /// it, unknown agents would break the sidebar entirely, which is the exact
    /// failure mode we exist to avoid.
    #[test]
    fn a_session_from_an_unaware_agent_round_trips_without_a_hint() {
        let plain = SessionInfo {
            hint: None,
            ..sample_session()
        };
        let json = serde_json::to_string(&plain).expect("serializes");
        let back: SessionInfo = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back.hint, None);
        assert_eq!(back, plain);
    }

    /// A hint must survive the wire with its label and camelCase field names.
    ///
    /// The label is what the operator reads to decide whether to act without
    /// opening the session, so losing it silently turns a useful row into a
    /// bare "needs you" with no way to triage.
    #[test]
    fn an_agent_hint_round_trips_with_its_label() {
        let json = serde_json::to_string(&sample_session()).expect("serializes");
        assert!(json.contains(r#""receivedAtMs":"#), "camelCase: {json}");
        assert!(json.contains(r#""state":"approval""#), "state token: {json}");
        let back: SessionInfo = serde_json::from_str(&json).expect("deserializes");
        let hint = back.hint.expect("hint present");
        assert_eq!(hint.state, HintState::Approval);
        assert_eq!(hint.label.as_deref(), Some("run `rm -rf build/`?"));
        assert!(hint.state.blocks_on_operator());
    }

    /// WHY: every field on the control plane that carries raw PTY bytes must
    /// ride as base64, not as serde's default array of decimal integers. The
    /// default costs about 3.5 bytes of JSON per payload byte and, on the
    /// receiving side, one boxed number per byte before anything can use it.
    ///
    /// Pinned as a shape rather than a round trip because a round trip passes
    /// either way: `Vec<u8>` deserializes happily from the array form, so only
    /// asserting on the emitted JSON can catch the attribute being dropped.
    #[test]
    fn every_byte_carrying_field_rides_as_base64_not_an_integer_array() {
        let hit = SearchHit {
            session: SessionId(1),
            line_seq: 7,
            // Deliberately not valid UTF-8: these are terminal bytes.
            visible: vec![b'h', b'i', 0xff],
            match_start: 0,
            match_end: 2,
            before: vec![vec![b'a'], vec![0x00, 0x1b]],
            after: vec![],
        };
        let json = serde_json::to_string(&hit).expect("a hit serializes");
        assert!(json.contains(r#""visible":"aGn/""#), "visible: {json}");
        assert!(json.contains(r#""before":["YQ==","ABs="]"#), "before: {json}");
        assert!(json.contains(r#""after":[]"#), "after: {json}");
        assert_eq!(
            serde_json::from_str::<SearchHit>(&json).expect("round trips"),
            hit
        );

        let chunk = ServerMsg::ScrollbackChunk {
            session: SessionId(2),
            from_seq: 0,
            data: vec![0x1b, b'[', b'0', b'm', 0xfe],
            more: false,
        };
        let json = serde_json::to_string(&chunk).expect("a chunk serializes");
        assert!(json.contains(r#""data":"G1swbf4=""#), "data: {json}");
        assert_eq!(
            serde_json::from_str::<ServerMsg>(&json).expect("round trips"),
            chunk
        );
    }
}
