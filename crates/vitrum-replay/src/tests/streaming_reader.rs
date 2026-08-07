use crate::asciicast::{EventRef, StreamingReader, read};

#[test]
fn streaming_reader_emits_borrowed_event_refs() {
    let text = "{\"version\":2,\"width\":80,\"height\":24}\n\
                [0.000000, \"o\", \"hello\"]\n\
                [0.500000, \"m\", \"chapter 1\"]\n\
                [1.000000, \"r\", \"100x30\"]\n\
                [1.500000, \"i\", \"ls\\r\"]\n\
                [2.000000, \"x\", \"ignored\"]\n";

    let reader = StreamingReader::new(text).expect("Failed to create StreamingReader");
    assert_eq!(reader.header().width, 80);
    assert_eq!(reader.header().height, 24);

    let events: Vec<_> = reader.map(|e| e.expect("valid event")).collect();
    assert_eq!(events.len(), 5);

    assert_eq!(
        events[0],
        EventRef::Output {
            line: 2,
            micros: 0,
            raw_data: "hello"
        }
    );
    assert_eq!(
        events[1],
        EventRef::Marker {
            line: 3,
            micros: 500_000,
            raw_label: "chapter 1"
        }
    );
    assert_eq!(
        events[2],
        EventRef::Resize {
            micros: 1_000_000,
            cols: 100,
            rows: 30
        }
    );
    assert_eq!(events[3], EventRef::Input { micros: 1_500_000 });

    match events[4] {
        EventRef::Skipped { line: 6, code: b'x' } => {}
        _ => panic!("Expected custom code skipped"),
    }
}

#[test]
fn read_uses_streaming_reader_for_escaped_output() {
    let text = "{\"version\":2,\"width\":80,\"height\":24}\n\
                [0.000000, \"o\", \"hello \\u001b[32mworld\\u001b[0m\\r\\n\"]\n\
                [0.500000, \"m\", \"start\"]\n";

    let recording = read(text).expect("read failed");
    assert_eq!(recording.bytes(), b"hello \x1b[32mworld\x1b[0m\r\n");
    assert_eq!(recording.markers().len(), 1);
    assert_eq!(recording.markers()[0].label, "start");
}

/// Every scalar written as `\uXXXX`, which no reader can take the raw path for.
fn fully_escaped(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        let mut buf = [0u16; 2];
        for unit in ch.encode_utf16(&mut buf) {
            out.push_str(&format!("\\u{unit:04x}"));
        }
    }
    out
}

fn one_event(code: char, body: &str) -> String {
    format!("{{\"version\":2,\"width\":80,\"height\":24}}\n[0.000000, \"{code}\", \"{body}\"]\n")
}

/// The bodies below contain no `\` and no `"`, so they are legal JSON written
/// literally, which is what sends them down the borrowed path.
const RAW_SAFE: &[&str] = &[
    "",
    "hello",
    "plain ascii with spaces and 0123456789",
    "punctuation !#$%&'()*+,-./:;<=>?@[]^_`{|}~",
    "accented caf\u{e9} na\u{ef}ve",
    "arrows \u{2192}\u{2190} and box \u{2500}\u{2502}",
    "cjk \u{4f60}\u{597d}\u{4e16}\u{754c}",
    "emoji \u{1f600}\u{1f680}",
    "tab-looking\tand ascii control-free",
];

#[test]
fn the_borrowed_output_path_agrees_with_the_decoding_one() {
    // `read` stopped decoding every output body: a body with no backslash is
    // pushed as its own bytes. That is only correct because a JSON escape
    // always begins with a backslash, so the two paths must agree scalar for
    // scalar, and this walks a body through both of them.
    for text in RAW_SAFE {
        let borrowed = read(&one_event('o', text)).expect("raw body reads");
        let decoded = read(&one_event('o', &fully_escaped(text))).expect("escaped body reads");
        assert_eq!(
            borrowed.bytes(),
            text.as_bytes(),
            "the borrowed path changed the bytes of {text:?}"
        );
        assert_eq!(
            borrowed.bytes(),
            decoded.bytes(),
            "the two paths disagree on {text:?}"
        );
        assert_eq!(borrowed.stamps().len(), decoded.stamps().len());
    }
}

#[test]
fn the_borrowed_marker_path_agrees_with_the_decoding_one() {
    // A marker label takes the same split, and a label is what an operator
    // reads, so a divergence here is visible in the scrubber.
    for text in RAW_SAFE {
        let borrowed = read(&one_event('m', text)).expect("raw label reads");
        let decoded = read(&one_event('m', &fully_escaped(text))).expect("escaped label reads");
        assert_eq!(borrowed.markers()[0].label, *text);
        assert_eq!(borrowed.markers()[0].label, decoded.markers()[0].label);
    }
}

#[test]
fn an_escape_that_decodes_to_ordinary_text_still_decodes() {
    // The choice of path is made on the written form, not on what it means. A
    // body that is all escapes decodes to text a raw body could have carried.
    let recording = read(&one_event('o', "\\u0068\\u0069")).expect("reads");
    assert_eq!(recording.bytes(), b"hi");
}
