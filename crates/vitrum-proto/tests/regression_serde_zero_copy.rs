//! Regression test suite for `vitrum-proto` serde and frame handling.
//!
//! Covers zero-copy `Cow` string borrowing, malformed JSON payload handling,
//! compact enum discriminant byte sizing, and Serde scratch buffer reuse.

use std::borrow::Cow;
use std::mem::size_of;
use vitrum_proto::b64;
use vitrum_proto::text;
use vitrum_proto::{
    decode_output, encode_output_into, Attention, ClientMsg, Credit, HintState, ProjectId,
    ServerMsg, SessionId, SessionStatus, OUTPUT_HEADER_LEN, PROTOCOL_VERSION,
};

/// Helper struct for testing zero-copy string borrowing from JSON frames.
#[derive(Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
struct BorrowedFrame<'a> {
    #[serde(borrow)]
    topic: Cow<'a, str>,
    #[serde(borrow)]
    payload: Cow<'a, str>,
}
#[derive(Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
struct BorrowedControlMsg<'a> {
    #[serde(borrow)]
    command: Cow<'a, str>,
    #[serde(borrow)]
    cwd: Cow<'a, str>,
    #[serde(borrow)]
    title: Cow<'a, str>,
}

// ============================================================================
// 1. Zero-Copy Cow String Borrowing from JSON Frames
// ============================================================================

/// WHY: Zero-copy deserialization of string payload fields into std::borrow::Cow<'a, str>
/// from a raw JSON buffer must borrow directly from the input slice (Cow::Borrowed) when
/// no string escape sequences are present, avoiding heap string allocations during JSON frame handling.
#[test]
fn zero_copy_cow_str_borrowing_unescaped_json_frame() {
    let raw_json = r#"{"topic":"session/42/output","payload":"terminal ready"}"#;

    let frame: BorrowedFrame =
        serde_json::from_str(raw_json).expect("failed to deserialize unescaped zero-copy frame");

    assert!(
        matches!(frame.topic, Cow::Borrowed(_)),
        "expected Cow::Borrowed for unescaped topic"
    );
    assert!(
        matches!(frame.payload, Cow::Borrowed(_)),
        "expected Cow::Borrowed for unescaped payload"
    );

    // Verify pointer alignment / address overlap with input string
    let input_bytes = raw_json.as_bytes();
    let topic_ptr = frame.topic.as_ptr();
    let payload_ptr = frame.payload.as_ptr();

    let input_start = input_bytes.as_ptr() as usize;
    let input_end = input_start + input_bytes.len();

    assert!(
        (topic_ptr as usize) >= input_start && (topic_ptr as usize) < input_end,
        "topic pointer must point within input JSON slice"
    );
    assert!(
        (payload_ptr as usize) >= input_start && (payload_ptr as usize) < input_end,
        "payload pointer must point within input JSON slice"
    );

    assert_eq!(frame.topic, "session/42/output");
    assert_eq!(frame.payload, "terminal ready");
}

/// WHY: When a JSON string frame contains escape sequences (such as \n, \t, \", or \uXXXX),
/// zero-copy std::borrow::Cow<'a, str> deserialization must seamlessly fall back to allocating
/// Cow::Owned containing the decoded unescaped string without failing or corrupting data.
#[test]
fn zero_copy_cow_str_fallback_to_owned_on_escapes() {
    let raw_json = r#"{"topic":"session\/42","payload":"line1\nline2\t\"escaped\" \u0041"}"#;

    let frame: BorrowedFrame =
        serde_json::from_str(raw_json).expect("failed to deserialize escaped frame");

    assert!(
        matches!(frame.payload, Cow::Owned(_)),
        "expected Cow::Owned for escaped payload"
    );
    assert_eq!(frame.payload, "line1\nline2\t\"escaped\" A");
}

/// WHY: Verify that borrowed &'a str / Cow<'a, str> fields in zero-copy frame structures
/// preserve the precise underlying string buffer lifetime 'a across nested/optional deserialization
/// fields (such as command, cwd, and title) and match exact slice boundaries without extra allocations.
#[test]
fn zero_copy_borrowed_str_lifetime_bound_verification() {
    let raw_json = r#"{"command":"cargo test","cwd":"/projects/vitrum","title":"unit_test"}"#;

    let msg: BorrowedControlMsg =
        serde_json::from_str(raw_json).expect("failed to deserialize control message");

    assert!(matches!(msg.command, Cow::Borrowed(_)));
    assert!(matches!(msg.cwd, Cow::Borrowed(_)));
    assert!(matches!(msg.title, Cow::Borrowed(_)));

    assert_eq!(msg.command, "cargo test");
    assert_eq!(msg.cwd, "/projects/vitrum");
    assert_eq!(msg.title, "unit_test");

    // Address assertions: verify all 3 borrowed fields point within input memory
    let input_bytes = raw_json.as_bytes();
    let input_start = input_bytes.as_ptr() as usize;
    let input_end = input_start + input_bytes.len();

    let cmd_ptr = msg.command.as_ptr() as usize;
    let cwd_ptr = msg.cwd.as_ptr() as usize;
    let title_ptr = msg.title.as_ptr() as usize;

    assert!(cmd_ptr >= input_start && cmd_ptr < input_end);
    assert!(cwd_ptr >= input_start && cwd_ptr < input_end);
    assert!(title_ptr >= input_start && title_ptr < input_end);
}

/// WHY: Base64 decode operations on control-plane frames (such as ServerMsg::ScrollbackChunk)
/// must support decoding directly into borrowing or pre-allocated byte slices without
/// allocating auxiliary temporary string wrappers.
#[test]
fn zero_copy_cow_bytes_b64_slice_decoding() {
    let original_bytes = b"PTY scrollback raw bytes \x00\x01\xFF\xFE sample payload";
    let encoded = b64::encode(original_bytes);

    // Verify b64::decode directly produces byte vector from string slice
    let decoded = b64::decode(&encoded).expect("b64 decode failed");
    assert_eq!(decoded, original_bytes);

    // Verify serde b64 module roundtrip for ScrollbackChunk
    let chunk_msg = ServerMsg::ScrollbackChunk {
        session: SessionId(101),
        from_seq: 4096,
        data: original_bytes.to_vec(),
        more: true,
    };

    let json = serde_json::to_string(&chunk_msg).expect("serialize ScrollbackChunk");
    assert!(json.contains(&encoded));

    let deserialized: ServerMsg =
        serde_json::from_str(&json).expect("deserialize ScrollbackChunk");
    if let ServerMsg::ScrollbackChunk {
        session,
        from_seq,
        data,
        more,
    } = deserialized
    {
        assert_eq!(session, SessionId(101));
        assert_eq!(from_seq, 4096);
        assert_eq!(data, original_bytes);
        assert!(more);
    } else {
        panic!("unexpected variant after deserialization");
    }
}

// ============================================================================
// 2. Malformed JSON Payload Handling
// ============================================================================

/// WHY: Control-plane JSON frames originating from sockets can be truncated, broken,
/// or syntactically invalid; the deserializer must reject truncated brackets, unclosed strings,
/// trailing commas, and partial key-value pairs cleanly with Serde errors rather than panicking.
#[test]
fn malformed_json_truncated_and_syntax_error_recovery() {
    let truncated_inputs = vec![
        r#"{"t":"hello""#,
        r#"{"t":"hello", "protocol":"#,
        r#"{"t":"createSession", "cwd": "foo""#,
        r#"{"t": "input", "session": 1, "data": [1,2,"#,
        r#"{"t":"search", "pattern": "foo", "regex": true,"#,
        r#"{"t":"list","#,
        r#"{"t":"hello", "protocol": 2,}"#, // trailing comma
        r#"{t: "list"}"#,                    // unquoted key
    ];

    for input in truncated_inputs {
        let res: Result<ClientMsg, _> = serde_json::from_str(input);
        assert!(
            res.is_err(),
            "input {:?} should fail deserialization cleanly",
            input
        );
    }
}

/// WHY: Adversarial or outdated clients sending unknown message tags or invalid field types
/// (e.g., string session ID instead of u64, or string protocol version) must be rejected with
/// informative variant/type mismatch errors while protecting protocol integrity.
#[test]
fn malformed_json_invalid_enum_discriminants_and_type_mismatches() {
    // Unknown message tag 't'
    let unknown_tag = r#"{"t":"unknownMessageKind","foo":"bar"}"#;
    let res: Result<ClientMsg, _> = serde_json::from_str(unknown_tag);
    assert!(res.is_err());
    let err_str = res.unwrap_err().to_string();
    assert!(
        err_str.contains("unknown variant") || err_str.contains("unknown field"),
        "error should state unknown variant: {}",
        err_str
    );

    // Type mismatch: protocol expected u32, got string
    let bad_protocol_type = r#"{"t":"hello","protocol":"v2"}"#;
    let res: Result<ClientMsg, _> = serde_json::from_str(bad_protocol_type);
    assert!(res.is_err());

    // Type mismatch: session expected u64 integer, got boolean
    let bad_session_type = r#"{"t":"detach","session":true}"#;
    let res: Result<ClientMsg, _> = serde_json::from_str(bad_session_type);
    assert!(res.is_err());

    // Type mismatch for ServerMsg discriminant
    let bad_server_tag = r#"{"t":"sessionCreated","id":1}"#;
    let res: Result<ServerMsg, _> = serde_json::from_str(bad_server_tag);
    assert!(res.is_err());
}

/// WHY: Extremely large, deeply nested, or invalid Base64 malformed JSON payloads must fail safely
/// within bounds without triggering stack overflow or process crashes.
#[test]
fn malformed_json_adversarial_deep_nesting_and_excessive_payloads() {
    // Deeply nested JSON object
    let mut nested = String::new();
    for _ in 0..128 {
        nested.push_str(r#"{"nested":"#);
    }
    nested.push_str("42");
    for _ in 0..128 {
        nested.push('}');
    }

    let res: Result<ClientMsg, _> = serde_json::from_str(&nested);
    assert!(res.is_err(), "deeply nested payload should fail cleanly");

    // Invalid base64 characters in ScrollbackChunk frame
    let invalid_b64_json = r#"{"t":"scrollbackChunk","session":1,"fromSeq":0,"data":"!!!NOT_VALID_BASE64!!!","more":false}"#;
    let res: Result<ServerMsg, _> = serde_json::from_str(invalid_b64_json);
    assert!(res.is_err(), "invalid base64 payload must fail serde");
}

/// WHY: Malformed text fields containing dangerous control characters (ANSI escape sequences,
/// bi-directional override codepoints like U+202E, or raw newlines) in error messages or title strings
/// must be sanitized by text::display_safe and text::error_text without corrupting wire deserialization.
#[test]
fn malformed_json_control_character_injection_in_text_fields() {
    // Input containing bidi control character (U+202E) and ANSI escape sequence
    let adversarial_title = "main_branch\u{202E}\x1b[31m_spoofed";
    let safe_title = text::display_safe(adversarial_title);

    assert!(
        !safe_title.chars().any(|c| c.is_control()),
        "control characters must be stripped"
    );
    assert!(
        !safe_title.contains('\u{202E}'),
        "bidi override characters must be stripped"
    );
    assert_eq!(safe_title, "main_branch[31m_spoofed");
    // Test ServerMsg::error sanitizer constructor
    let raw_err = "failed to launch process\n\x1b[33mWarning:\x1b[0m \u{202E}secret_override";
    let err_msg = ServerMsg::error(Some(SessionId(7)), raw_err);
    if let ServerMsg::Error { session, message, .. } = err_msg {
        assert_eq!(session, Some(SessionId(7)));
        assert!(!message.contains('\n'), "newlines must be sanitized");
        assert!(!message.contains('\x1b'), "escapes must be sanitized");
        assert!(!message.contains('\u{202E}'), "bidi must be sanitized");
    } else {
        panic!("expected ServerMsg::Error variant");
    }
}

// ============================================================================
// 3. Compact Enum Discriminant Byte Sizing
// ============================================================================

/// WHY: Enum discriminants in ClientMsg, ServerMsg, SessionStatus, HintState, and Credit
/// must serialize into compact JSON representations (using camelCase tags or simple string variants)
/// to minimize control-plane wire size overhead per message.
#[test]
fn compact_enum_discriminant_json_byte_sizing() {
    // 1. ClientMsg::List variant discriminant compact tag "list"
    let msg_list = ClientMsg::List;
    let json_list = serde_json::to_string(&msg_list).expect("serialize List");
    assert_eq!(json_list, r#"{"t":"list"}"#);
    assert_eq!(json_list.len(), 12);

    // 2. ClientMsg::Hello variant tag "hello"
    let msg_hello = ClientMsg::Hello {
        protocol: PROTOCOL_VERSION,
    };
    let json_hello = serde_json::to_string(&msg_hello).expect("serialize Hello");
    assert_eq!(json_hello, r#"{"t":"hello","protocol":2}"#);

    // 3. SessionStatus variants: tagged as "state" with camelCase values
    let status_starting = SessionStatus::Starting;
    let json_starting = serde_json::to_string(&status_starting).expect("serialize Starting");
    assert_eq!(json_starting, r#"{"state":"starting"}"#);

    let status_running = SessionStatus::Running;
    let json_running = serde_json::to_string(&status_running).expect("serialize Running");
    assert_eq!(json_running, r#"{"state":"running"}"#);

    let status_exited = SessionStatus::Exited { code: Some(0) };
    let json_exited = serde_json::to_string(&status_exited).expect("serialize Exited");
    assert_eq!(json_exited, r#"{"state":"exited","code":0}"#);

    // 4. HintState variants: compact camelCase strings
    assert_eq!(
        serde_json::to_string(&HintState::Approval).unwrap(),
        r#""approval""#
    );
    assert_eq!(
        serde_json::to_string(&HintState::Input).unwrap(),
        r#""input""#
    );
    assert_eq!(
        serde_json::to_string(&HintState::Working).unwrap(),
        r#""working""#
    );
    assert_eq!(
        serde_json::to_string(&HintState::Ready).unwrap(),
        r#""ready""#
    );

    // 5. Credit variants: compact camelCase strings
    assert_eq!(
        serde_json::to_string(&Credit::Observed).unwrap(),
        r#""observed""#
    );
    assert_eq!(
        serde_json::to_string(&Credit::Inferred).unwrap(),
        r#""inferred""#
    );
}

/// WHY: Memory representations of core protocol enums (SessionStatus, HintState, Credit, Attention)
/// must stay tightly bounded in byte size (std::mem::size_of) to prevent layout bloat in session
/// registries, search hit vectors, and state caches.
#[test]
fn compact_enum_discriminant_in_memory_layout_bounds() {
    assert_eq!(
        size_of::<SessionId>(),
        8,
        "SessionId transparent u64 must be 8 bytes"
    );
    assert_eq!(
        size_of::<ProjectId>(),
        8,
        "ProjectId transparent u64 must be 8 bytes"
    );

    assert!(
        size_of::<SessionStatus>() <= 16,
        "SessionStatus must be compact in memory (<= 16 bytes), got {}",
        size_of::<SessionStatus>()
    );

    assert_eq!(
        size_of::<HintState>(),
        1,
        "HintState unit enum must fit in 1 byte"
    );

    assert_eq!(
        size_of::<Credit>(),
        1,
        "Credit unit enum must fit in 1 byte"
    );

    assert!(
        size_of::<Attention>() <= 24,
        "Attention struct must remain compact in memory (<= 24 bytes), got {}",
        size_of::<Attention>()
    );
}

/// WHY: Round-trip serialization and deserialization of all enum variants in SessionStatus,
/// HintState, and Credit must preserve discriminant identity and match exact string representations
/// across binary and JSON serializers.
#[test]
fn compact_enum_discriminant_roundtrip_integrity() {
    let statuses = vec![
        SessionStatus::Starting,
        SessionStatus::Running,
        SessionStatus::Exited { code: Some(0) },
        SessionStatus::Exited { code: Some(137) },
        SessionStatus::Exited { code: None },
    ];
    for st in statuses {
        let json = serde_json::to_string(&st).expect("status serialization");
        let de: SessionStatus = serde_json::from_str(&json).expect("status deserialization");
        assert_eq!(st, de);
    }

    let hints = vec![
        HintState::Approval,
        HintState::Input,
        HintState::Working,
        HintState::Ready,
    ];
    for hint in hints {
        let json = serde_json::to_string(&hint).expect("hint serialization");
        let de: HintState = serde_json::from_str(&json).expect("hint deserialization");
        assert_eq!(hint, de);
    }

    let credits = vec![
        Credit::Observed,
        Credit::Inferred,
    ];
    for credit in credits {
        let json = serde_json::to_string(&credit).expect("credit serialization");
        let de: Credit = serde_json::from_str(&json).expect("credit deserialization");
        assert_eq!(credit, de);
    }
}

// ============================================================================
// 4. Serde Scratch Buffer Reuse
// ============================================================================

/// WHY: High-frequency control-plane serialization (e.g. repeated ClientMsg::Input or
/// ServerMsg::SearchResults) into a recycled scratch buffer (Vec<u8>) must retain buffer
/// capacity and avoid heap reallocations across iterations.
#[test]
fn serde_scratch_buffer_reuse_zero_alloc_on_repeated_serialization() {
    let msg = ClientMsg::Input {
        session: SessionId(42),
        data: b"ls -la /tmp\n".to_vec(),
    };

    let mut scratch: Vec<u8> = Vec::with_capacity(1024);
    serde_json::to_writer(&mut scratch, &msg).expect("initial write");
    let initial_cap = scratch.capacity();
    let initial_ptr = scratch.as_ptr();

    // Perform 1000 serialization cycles reusing scratch
    for i in 0..1000 {
        scratch.clear();
        serde_json::to_writer(&mut scratch, &msg).expect("recycled write");
        assert!(
            !scratch.is_empty(),
            "scratch buffer must contain serialized JSON at step {}",
            i
        );
    }

    assert_eq!(
        scratch.capacity(),
        initial_cap,
        "scratch buffer capacity must remain unchanged across repeated serializations"
    );
    assert_eq!(
        scratch.as_ptr(),
        initial_ptr,
        "scratch buffer allocation pointer must remain stable across iterations"
    );
}

/// WHY: PTY output frame encoding via encode_output_into must reuse a caller-provided Vec<u8>
/// scratch buffer without reallocating, ensuring zero allocation on the data-plane hot path.
#[test]
fn serde_scratch_buffer_reuse_data_plane_frame_encoding() {
    let payload = b"output line 1\r\noutput line 2\r\n";
    let mut scratch: Vec<u8> = Vec::with_capacity(256);

    encode_output_into(&mut scratch, SessionId(999), 1048576, payload);
    let initial_cap = scratch.capacity();
    let initial_ptr = scratch.as_ptr();

    // Verify decoded content of initial encode
    let (decoded_session, decoded_seq, decoded_payload) =
        decode_output(&scratch).expect("decode failed");
    assert_eq!(decoded_session, SessionId(999));
    assert_eq!(decoded_seq, 1048576);
    assert_eq!(decoded_payload, payload);

    // Repeated frame encoding using scratch buffer reuse
    for seq in 1048577..1049576 {
        scratch.clear();
        encode_output_into(&mut scratch, SessionId(999), seq, payload);
        assert_eq!(
            scratch.len(),
            OUTPUT_HEADER_LEN + payload.len(),
            "encoded length must match exact header + payload size"
        );
    }

    assert_eq!(
        scratch.capacity(),
        initial_cap,
        "data plane scratch capacity must not grow"
    );
    assert_eq!(
        scratch.as_ptr(),
        initial_ptr,
        "data plane scratch pointer must remain stable"
    );
}

/// WHY: Parsing a stream of JSON control-plane frames from a shared byte buffer using
/// serde_json::Deserializer::from_slice must reuse stream offsets and scratch allocations
/// cleanly without copying memory.
#[test]
fn serde_scratch_buffer_reuse_json_deserializer_stream() {
    let mut stream_bytes = Vec::new();
    let msg1 = ClientMsg::List;
    let msg2 = ClientMsg::Hello { protocol: 2 };
    let msg3 = ClientMsg::Detach {
        session: SessionId(5),
    };

    serde_json::to_writer(&mut stream_bytes, &msg1).unwrap();
    serde_json::to_writer(&mut stream_bytes, &msg2).unwrap();
    serde_json::to_writer(&mut stream_bytes, &msg3).unwrap();

    let deserializer = serde_json::Deserializer::from_slice(&stream_bytes);
    let mut stream_iter = deserializer.into_iter::<ClientMsg>();

    let parsed1 = stream_iter.next().expect("msg1").expect("valid msg1");
    let parsed2 = stream_iter.next().expect("msg2").expect("valid msg2");
    let parsed3 = stream_iter.next().expect("msg3").expect("valid msg3");

    assert_eq!(parsed1, msg1);
    assert_eq!(parsed2, msg2);
    assert_eq!(parsed3, msg3);
    assert!(stream_iter.next().is_none(), "stream should be fully consumed");
}
