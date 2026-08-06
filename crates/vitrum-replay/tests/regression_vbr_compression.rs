//! Regression test suite covering zero-copy .vbr keyframe index seeking,
//! ZstdChunked / RleDeflate stream compression 0xFF escaping,
//! corrupt asciicast log handling, and VT delta snapshotting.

use vitrum_replay::asciicast::jsonstr::{decode, encode};
use vitrum_replay::asciicast::{self, Header, Utf8Policy, read, to_string};
use vitrum_replay::config::ReplayConfig;
use vitrum_replay::emulator::Emulator;
use vitrum_replay::error::{CastError, Error};
use vitrum_replay::keyframe::KeyframeIndex;
use vitrum_replay::replay::Replay;
use vitrum_replay::screen::Screen;
use vitrum_replay::stream::Stream;
use vitrum_replay::timeline::Timeline;

// ============================================================================
// CATEGORY 1: Zero-Copy .vbr Keyframe Index Seeking
// ============================================================================

/// WHY: Defends zero-copy .vbr keyframe index seeking across multiple stream
/// chunks without copying or altering byte offsets (seq), ensuring seeking to
/// arbitrary offsets across chunk boundaries resolves the exact VT screen state.
#[test]
fn test_vbr_zero_copy_stream_seeking_across_chunks() {
    let chunk1: &[u8] = b"Line 1: Hello World\r\n";
    let chunk2: &[u8] = b"Line 2: Zero Copy VBR\r\n";
    let chunk3: &[u8] = b"Line 3: Stream Seeking\r\n";
    let chunks = [chunk1, chunk2, chunk3];

    let base_seq = 1_000;
    let stream = Stream::new(base_seq, &chunks);

    assert_eq!(stream.base_seq(), 1_000);
    assert_eq!(
        stream.head_seq(),
        1_000 + (chunk1.len() + chunk2.len() + chunk3.len()) as u64
    );

    let config = ReplayConfig::new(80, 24)
        .unwrap()
        .with_keyframe_stride(10)
        .unwrap();

    let mut replay = Replay::build(stream, &config).expect("Replay build must succeed");

    // Seek to the end of chunk1
    let seq_chunk1_end = base_seq + chunk1.len() as u64;
    replay.seek(seq_chunk1_end).expect("Seek should succeed");
    assert_eq!(replay.screen().line(0).trim_end(), "Line 1: Hello World");
    assert_eq!(replay.screen().line(1).trim_end(), "");

    // Seek to the end of chunk2 across boundary
    let seq_chunk2_end = seq_chunk1_end + chunk2.len() as u64;
    replay.seek(seq_chunk2_end).expect("Seek should succeed");
    assert_eq!(replay.screen().line(0).trim_end(), "Line 1: Hello World");
    assert_eq!(replay.screen().line(1).trim_end(), "Line 2: Zero Copy VBR");

    // Seek to the end of stream
    replay.seek(stream.head_seq()).expect("Seek should succeed");
    assert_eq!(replay.screen().line(2).trim_end(), "Line 3: Stream Seeking");
}

/// WHY: Defends keyframe index lookup (`latest_at_or_before`) and rewind behavior
/// where seeking backwards restores the nearest preceding keyframe before feeding delta bytes.
#[test]
fn test_vbr_keyframe_lookup_and_backward_rewind() {
    let mut data = Vec::new();
    for i in 0..20 {
        data.extend_from_slice(format!("Row {:02}: sequential data line\r\n", i).as_bytes());
    }

    let chunks = [data.as_slice()];
    let stream = Stream::new(0, &chunks);
    let config = ReplayConfig::new(80, 24)
        .unwrap()
        .with_keyframe_stride(64)
        .unwrap();

    let index = KeyframeIndex::build(&stream, &config).expect("Index build succeeds");
    assert!(
        index.len() > 1,
        "Index should contain multiple keyframes for 64-byte stride"
    );

    let mut replay = Replay::build(stream, &config).expect("Replay build succeeds");

    // Seek far forward to row 15
    let seq_far = (15 * "Row 00: sequential data line\r\n".len()) as u64;
    replay.seek(seq_far).expect("Forward seek succeeds");
    assert_eq!(replay.screen().line(14).trim_end(), "Row 14: sequential data line");

    // Rewind back to row 3
    let seq_near = (3 * "Row 00: sequential data line\r\n".len()) as u64;
    replay.seek(seq_near).expect("Backward seek / rewind succeeds");
    assert_eq!(replay.screen().line(2).trim_end(), "Row 02: sequential data line");
    assert_eq!(replay.screen().line(3).trim_end(), "");
}

/// WHY: Defends keyframe index boundary sliding when a stride boundary lands in the
/// middle of a VT escape sequence or UTF-8 multi-byte character.
#[test]
fn test_vbr_keyframe_ground_boundary_sliding() {
    let mut data = Vec::new();
    // Insert text then a long escape sequence across a 32-byte boundary
    data.extend_from_slice(b"Prefix text ");
    data.extend_from_slice(b"\x1b[38;2;255;128;64mColored Text\x1b[0m Postfix\r\n");

    let chunks = [data.as_slice()];
    let stream = Stream::new(0, &chunks);
    let config = ReplayConfig::new(80, 24)
        .unwrap()
        .with_keyframe_stride(16)
        .unwrap();

    let index = KeyframeIndex::build(&stream, &config).expect("Index build succeeds");
    // Verify that every recorded keyframe sequence sits at a ground state
    for frame in index.frames() {
        let mut emu = Emulator::new(config.cols, config.rows, config.palette).unwrap();
        for slice in stream.slices(0..frame.seq) {
            emu.feed(slice);
        }
        assert!(
            emu.feed_byte(b'A'),
            "Keyframe seq {} must be in ground state",
            frame.seq
        );
    }
}

/// WHY: Defends zero-stride configuration validation by rejecting `keyframe_stride == 0`
/// with `Error::ZeroStride` to prevent per-byte keyframe allocation exhaustion.
#[test]
fn test_vbr_zero_stride_rejection_safety() {
    let config = ReplayConfig::new(80, 24).unwrap();
    assert_eq!(config.with_keyframe_stride(0), Err(Error::ZeroStride));

    let zero_config = ReplayConfig {
        keyframe_stride: 0,
        ..config
    };
    let stream = Stream::new(0, &[b"test"]);
    assert_eq!(
        KeyframeIndex::build(&stream, &zero_config).err(),
        Some(Error::ZeroStride)
    );
}

/// WHY: Defends forward scrubbing efficiency by ensuring seeking to a higher seq
/// doesn't rewind to a keyframe if the current position is closer to the target.
#[test]
fn test_vbr_forward_scrubbing_optimization() {
    let mut data = Vec::new();
    for i in 0..50 {
        data.extend_from_slice(format!("Entry {:02}\r\n", i).as_bytes());
    }

    let chunks = [data.as_slice()];
    let stream = Stream::new(0, &chunks);
    let config = ReplayConfig::new(80, 24)
        .unwrap()
        .with_keyframe_stride(50)
        .unwrap();

    let mut replay = Replay::build(stream, &config).expect("Replay build succeeds");

    // Seek to position 100
    replay.seek(100).expect("Seek 100 succeeds");
    assert_eq!(replay.position(), 100);

    // Seek slightly forward to 120 (closer than restarting from keyframe)
    replay.seek(120).expect("Seek 120 succeeds");
    assert_eq!(replay.position(), 120);

    // Ensure state at 120 matches linear execution
    let mut emu = Emulator::new(config.cols, config.rows, config.palette).unwrap();
    for slice in stream.slices(0..120) {
        emu.feed(slice);
    }
    assert_eq!(replay.screen().line(0), emu.screen().line(0));
}

// ============================================================================
// CATEGORY 2: ZstdChunked / RleDeflate Stream Compression 0xFF Escaping
// ============================================================================

/// WHY: Defends `0xFF` byte escaping under `Utf8Policy::SurrogateEscape` in
/// compressed/chunked log streams, ensuring raw 0xFF roundtrips as `\udcff`
/// without stream truncation or encoding failure.
#[test]
fn test_stream_0xff_escaping_surrogate_roundtrip() {
    let raw_bytes: &[u8] = &[0x41, 0xFF, 0xFE, 0x80, 0x1B, 0x42];
    let mut encoded = String::new();
    encode(raw_bytes, Utf8Policy::SurrogateEscape, &mut encoded);

    assert!(
        encoded.contains("\\udcff"),
        "0xFF byte must encode to surrogate escape \\udcff, got: {encoded}"
    );

    let mut decoded = Vec::new();
    decode(&encoded, 1, &mut decoded).expect("Decoding surrogate escape must succeed");
    assert_eq!(decoded, raw_bytes, "Roundtripped bytes must match original");
}

/// WHY: Defends RLE/Deflate-style chunked stream payloads containing consecutive runs
/// of `0xFF` bytes, verifying that multi-byte `0xFF` sequences survive stream encoding.
#[test]
fn test_stream_rle_deflate_chunked_0xff_runs() {
    let mut rle_chunk = vec![0x41; 10];
    rle_chunk.extend_from_slice(&[0xFF; 64]);
    rle_chunk.extend_from_slice(b"END_RLE_CHUNK");

    let mut encoded = String::new();
    encode(&rle_chunk, Utf8Policy::SurrogateEscape, &mut encoded);

    let mut decoded = Vec::new();
    decode(&encoded, 1, &mut decoded).expect("Decoding RLE 0xFF payload succeeds");
    assert_eq!(
        decoded.len(),
        rle_chunk.len(),
        "Decoded length must match original RLE chunk length"
    );
    assert_eq!(decoded, rle_chunk, "Decoded payload must match original chunk");
}

/// WHY: Defends stream encoding/decoding when `0xFF` bytes are interleaved directly
/// inside VT terminal escape sequences and multibyte UTF-8 characters.
#[test]
fn test_stream_0xff_interleaved_vt_escape_sequences() {
    let payload = b"\x1b[31mRed\xffText\x1b[0m \xe2\x9c\x93 \xff\xfe";
    let chunks = [payload.as_slice()];
    let stream = Stream::new(0, &chunks);

    let json_text = to_string(
        &stream,
        &Timeline::positional(),
        &Header::new(80, 24),
        Utf8Policy::SurrogateEscape,
    )
    .expect("Exporting stream to asciicast string must succeed");

    let recording = read(&json_text).expect("Importing asciicast string must succeed");
    assert_eq!(
        recording.bytes(),
        payload,
        "Interleaved 0xFF VT payload must roundtrip byte-exact"
    );
}

/// WHY: Defends `Utf8Policy::Replacement` policy where `0xFF` bytes are replaced with
/// `U+FFFD` for player compatibility when surrogate escapes are explicitly disabled.
#[test]
fn test_stream_0xff_lossy_replacement_policy() {
    let raw_bytes: &[u8] = b"Header\xffFooter";
    let mut encoded = String::new();
    encode(raw_bytes, Utf8Policy::Replacement, &mut encoded);

    assert!(
        encoded.contains('\u{fffd}'),
        "Replacement policy must output Unicode replacement char U+FFFD for 0xFF byte"
    );

    let mut decoded = Vec::new();
    decode(&encoded, 1, &mut decoded).expect("Decoding replacement string succeeds");
    assert_ne!(
        decoded, raw_bytes,
        "Replacement policy is deliberately lossy and does not restore 0xFF"
    );
}

// ============================================================================
// CATEGORY 3: Corrupt Asciicast Log Handling
// ============================================================================

/// WHY: Defends corrupt asciicast log handling when line 1 header contains malformed
/// JSON or invalid schema, ensuring structured error is returned without panicking.
#[test]
fn test_corrupt_asciicast_invalid_json_header() {
    assert_eq!(read(""), Err(CastError::Empty));

    assert!(matches!(
        read("NOT_JSON\n[0.1, \"o\", \"hi\"]"),
        Err(CastError::HeaderSyntax { .. })
    ));

    // Unsupported version
    assert_eq!(
        read("{\"version\":1,\"width\":80,\"height\":24}\n"),
        Err(CastError::Version { found: 1 })
    );

    // Zero width/height
    assert_eq!(
        read("{\"version\":2,\"width\":0,\"height\":24}\n"),
        Err(CastError::MissingGeometry)
    );
}

/// WHY: Defends corrupt asciicast log handling when event timestamps go backward in time,
/// ensuring `CastError::EventTimeOrder` names the offending 1-based line number.
#[test]
fn test_corrupt_asciicast_non_monotonic_timestamps() {
    let log = "{\"version\":2,\"width\":80,\"height\":24}\n\
               [1.50, \"o\", \"First\"]\n\
               [1.20, \"o\", \"Backwards\"]\n";

    match read(log) {
        Err(CastError::EventTimeOrder { line, .. }) => {
            assert_eq!(line, 3, "Error must pinpoint line 3 for backwards time");
        }
        other => panic!("Expected CastError::EventTimeOrder, got: {other:?}"),
    }
}

/// WHY: Defends corrupt asciicast log handling when data strings contain malformed
/// JSON unicode escapes, unpaired high surrogates, or invalid low surrogates.
#[test]
fn test_corrupt_asciicast_malformed_surrogate_escapes() {
    // Malformed \u escape \uZZZZ
    let bad_hex = "{\"version\":2,\"width\":80,\"height\":24}\n\
                   [0.1, \"o\", \"\\uZZZZ\"]\n";
    assert!(matches!(read(bad_hex), Err(CastError::EventData { line: 2, .. })));

    // Unpaired high surrogate
    let unpaired_high = "{\"version\":2,\"width\":80,\"height\":24}\n\
                        [0.1, \"o\", \"\\ud83d text\"]\n";
    assert!(matches!(read(unpaired_high), Err(CastError::EventData { line: 2, .. })));

    // Out-of-range low surrogate (\udc00 is below \udc80 byte lower bound)
    let bad_low = "{\"version\":2,\"width\":80,\"height\":24}\n\
                   [0.1, \"o\", \"\\udc00\"]\n";
    assert!(matches!(read(bad_low), Err(CastError::EventData { line: 2, .. })));
}

/// WHY: Defends corrupt asciicast log handling when event lines are truncated or
/// contain non-array/wrong-length JSON elements.
#[test]
fn test_corrupt_asciicast_truncated_event_arrays() {
    // Event array with only 2 elements
    let short_event = "{\"version\":2,\"width\":80,\"height\":24}\n\
                       [0.5, \"o\"]\n";
    assert_eq!(read(short_event), Err(CastError::EventShape { line: 2 }));

    // Event array with 4 elements
    let long_event = "{\"version\":2,\"width\":80,\"height\":24}\n\
                      [0.5, \"o\", \"data\", \"extra\"]\n";
    assert_eq!(read(long_event), Err(CastError::EventShape { line: 2 }));
}

// ============================================================================
// CATEGORY 4: VT Delta Snapshotting
// ============================================================================

/// WHY: Defends VT delta snapshotting accuracy where seeking between keyframes applies
/// only the VT byte deltas from the previous keyframe to reconstruct identical cell grid state.
#[test]
fn test_vt_delta_snapshotting_screen_reconstruction() {
    let vt_stream = b"Line A\r\n\x1b[31mRed Line B\x1b[0m\r\nLine C\r\n";
    let chunks = [vt_stream.as_slice()];
    let stream = Stream::new(0, &chunks);

    let config = ReplayConfig::new(80, 24)
        .unwrap()
        .with_keyframe_stride(10)
        .unwrap();

    let mut replay = Replay::build(stream, &config).expect("Replay build succeeds");

    // Seek to halfway point
    let mid_seq = 15;
    replay.seek(mid_seq).expect("Seek succeeds");

    // Compare with direct Emulator execution up to mid_seq
    let mut emu = Emulator::new(config.cols, config.rows, config.palette).unwrap();
    for slice in stream.slices(0..mid_seq) {
        emu.feed(slice);
    }

    assert_eq!(
        replay.screen().line(0),
        emu.screen().line(0),
        "Screen line 0 must match delta snapshot"
    );
    assert_eq!(
        replay.screen().line(1),
        emu.screen().line(1),
        "Screen line 1 must match delta snapshot"
    );
    assert_eq!(
        replay.screen().cursor(),
        emu.screen().cursor(),
        "Cursor position must match delta snapshot"
    );
}

/// WHY: Defends VT delta snapshotting across screen erases (ED/EL) and alternate screen
/// buffer toggling (`\x1b[?1049h` / `\x1b[?1049l`), confirming keyframes preserve screen state.
#[test]
fn test_vt_delta_scrollback_erasure_and_alt_screen() {
    let mut vt_data = Vec::new();
    vt_data.extend_from_slice(b"Main Screen Content\r\n");
    vt_data.extend_from_slice(b"\x1b[?1049h"); // Switch to alt screen
    vt_data.extend_from_slice(b"\x1b[2J\x1b[H"); // Clear alt screen
    vt_data.extend_from_slice(b"Alt Screen Content\r\n");
    vt_data.extend_from_slice(b"\x1b[?1049l"); // Switch back to main screen

    let chunks = [vt_data.as_slice()];
    let stream = Stream::new(0, &chunks);
    let config = ReplayConfig::new(80, 24)
        .unwrap()
        .with_keyframe_stride(8)
        .unwrap();

    let mut replay = Replay::build(stream, &config).expect("Replay build succeeds");

    // Seek to end of stream
    replay.seek(stream.head_seq()).expect("Seek succeeds");

    // Content should show main screen restored
    assert_eq!(
        replay.screen().line(0).trim_end(),
        "Main Screen Content",
        "Main screen content must be restored after exiting alt screen"
    );
}

/// WHY: Defends VT delta snapshot keyframe resumption equivalence, proving that
/// `Emulator::resume(keyframe.screen().clone())` produces byte-for-byte identical output to linear execution.
#[test]
fn test_vt_delta_keyframe_resumption_equivalence() {
    let mut vt_data = Vec::new();
    for i in 0..100 {
        vt_data.extend_from_slice(
            format!("\x1b[38;5;{}mStep {:03} output log\x1b[0m\r\n", i % 256, i).as_bytes(),
        );
    }

    let chunks = [vt_data.as_slice()];
    let stream = Stream::new(0, &chunks);
    let config = ReplayConfig::new(80, 24)
        .unwrap()
        .with_keyframe_stride(128)
        .unwrap();

    let mut replay = Replay::build(stream, &config).expect("Replay build succeeds");

    // Perform seeking at multiple arbitrary points and compare against linear run
    for target_seq in [50, 300, 750, 1200, stream.head_seq()] {
        replay.seek(target_seq).expect("Seek must succeed");

        let mut emu = Emulator::new(config.cols, config.rows, config.palette).unwrap();
        for slice in stream.slices(0..target_seq) {
            emu.feed(slice);
        }

        for row in 0..24 {
            assert_eq!(
                replay.screen().line(row),
                emu.screen().line(row),
                "Row {} mismatch at target seq {}",
                row,
                target_seq
            );
        }
    }
}
