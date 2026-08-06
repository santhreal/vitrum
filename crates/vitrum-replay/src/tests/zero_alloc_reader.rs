use crate::asciicast::{EventRef, StreamingReader, read};

#[test]
fn test_streaming_reader_zero_alloc() {
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
        EventRef::Skipped { line: 6, code: Some(b'x') } => {}
        _ => panic!("Expected custom code skipped"),
    }
}

#[test]
fn test_streaming_reader_matches_read_roundtrip() {
    let text = "{\"version\":2,\"width\":80,\"height\":24}\n\
                [0.000000, \"o\", \"hello \\u001b[32mworld\\u001b[0m\\r\\n\"]\n\
                [0.500000, \"m\", \"start\"]\n";

    let recording = read(text).expect("read failed");
    assert_eq!(recording.bytes(), b"hello \x1b[32mworld\x1b[0m\r\n");
    assert_eq!(recording.markers().len(), 1);
    assert_eq!(recording.markers()[0].label, "start");
}
