//! Control-plane encoding for every message the server can actually emit.
//!
//! These exist because a variant that cannot be serialized is a runtime error at
//! the moment a client needs it most, not a compile error. Nothing in the schema
//! crate's own tests exercises the full set, so the gap is covered here.

use vitrum_proto::{
    AgentHint, Attention, HintState, ProjectId, ProjectInfo, ServerMsg, SessionId, SessionInfo,
    SessionStatus,
};

fn sample_session() -> SessionInfo {
    SessionInfo {
        id: SessionId(4),
        project_id: ProjectId(9),
        title: "claude".to_string(),
        cwd: "/src/demo".to_string(),
        command: "claude".to_string(),
        args: vec!["--resume".to_string()],
        status: SessionStatus::Running,
        created_at_ms: 1_700_000_000_000,
        last_activity_ms: 1_700_000_000_500,
        cols: 120,
        rows: 40,
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
            label: Some("write src/main.rs?".to_string()),
            received_at_ms: 1_700_000_000_400,
        }),
        term_title: Some("[ ! ] Action Required - claude".to_string()),
        worktree: Some("review".to_string()),
    }
}

fn sample_project() -> ProjectInfo {
    ProjectInfo {
        id: ProjectId(9),
        name: "demo".to_string(),
        root: "/tmp/demo".to_string(),
    }
}

/// Every server message must survive a JSON round trip.
///
/// A variant that fails to serialize would surface as an error frame in place of
/// the sidebar's session list, i.e. an empty GUI with no explanation. Asserting
/// on the recovered value rather than on the string keeps this honest about
/// semantics instead of formatting.
#[test]
fn every_server_message_round_trips() {
    let cases = vec![
        ServerMsg::Welcome {
            protocol: vitrum_proto::PROTOCOL_VERSION,
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
            session: SessionId(4),
            from_seq: 4_294_967_400,
            data: vec![0, 27, 91, 255],
            more: true,
        },
        ServerMsg::Exited {
            session: SessionId(4),
            code: Some(3),
        },
        ServerMsg::error(None, "unsupported protocol"),
    ];
    for case in cases {
        let text = serde_json::to_string(&case)
            .unwrap_or_else(|e| panic!("serializing {case:?} failed: {e}"));
        let back: ServerMsg = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("deserializing {text} failed: {e}"));
        assert_eq!(back, case);
    }
}

/// The two list-shaped snapshots must carry their payload under a named field.
///
/// serde's internally tagged representation cannot serialize a newtype variant
/// wrapping a sequence, so `Projects(Vec<_>)` would fail at runtime while
/// compiling cleanly. This pins the wire shape both sides agreed on.
#[test]
fn list_snapshots_use_named_fields() {
    let text = serde_json::to_string(&ServerMsg::Sessions {
        sessions: Vec::new(),
    })
    .expect("sessions snapshot must serialize");
    assert_eq!(text, r#"{"t":"sessions","sessions":[]}"#);

    let text = serde_json::to_string(&ServerMsg::Projects {
        projects: Vec::new(),
    })
    .expect("projects snapshot must serialize");
    assert_eq!(text, r#"{"t":"projects","projects":[]}"#);
}

/// Scrollback offsets above `u32::MAX` must survive JSON as exact integers.
///
/// A long-lived agent passes 4 GiB of output, and a lossy round trip through a
/// float would land replayed history at the wrong offset.
#[test]
fn large_seq_survives_json() {
    let msg = ServerMsg::ScrollbackChunk {
        session: SessionId(1),
        from_seq: 9_007_199_254_740_993,
        data: Vec::new(),
        more: false,
    };
    let text = serde_json::to_string(&msg).expect("must serialize");
    assert!(
        text.contains("9007199254740993"),
        "seq must be written as an exact integer, got {text}"
    );
    assert_eq!(
        serde_json::from_str::<ServerMsg>(&text).expect("must parse"),
        msg
    );
}
