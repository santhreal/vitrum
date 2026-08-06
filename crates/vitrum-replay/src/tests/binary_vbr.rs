use crate::binary::{VbrHeader, VbrIndexEntry, VbrView, VbrWriter, VBR_MAGIC, VBR_VERSION};
use crate::replay::Replay;
use crate::config::ReplayConfig;
use crate::stream::Stream;

#[test]
fn test_vbr_header_encode_decode() {
    let header = VbrHeader {
        version: VBR_VERSION,
        header_len: 46,
        cols: 80,
        rows: 24,
        base_seq: 1000,
        head_seq: 2000,
        chunk_count: 5,
        keyframe_count: 2,
        index_offset: 500,
    };

    let encoded = header.encode();
    assert_eq!(&encoded[0..4], VBR_MAGIC);
    let decoded = VbrHeader::decode(&encoded).expect("header decoding failed");
    assert_eq!(header, decoded);
}

#[test]
fn test_vbr_writer_and_view_roundtrip() {
    let mut writer = VbrWriter::new(80, 24);
    writer.add_chunk(0, 100, b"hello ");
    writer.add_chunk(6, 200, b"world\r\n");
    writer.add_keyframe(0, 0, 100);
    writer.add_keyframe(6, 6, 200);

    let serialized = writer.serialize();
    let view = VbrView::parse(&serialized).expect("VbrView parsing failed");

    assert_eq!(view.header().cols, 80);
    assert_eq!(view.header().rows, 24);
    assert_eq!(view.header().chunk_count, 2);
    assert_eq!(view.header().keyframe_count, 2);

    let chunks: Vec<_> = view.chunks().collect();
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].data, b"hello ");
    assert_eq!(chunks[1].data, b"world\r\n");

    let reconstructed = view.reconstruct_bytes();
    assert_eq!(reconstructed, b"hello world\r\n");

    let kf = view.binary_search_keyframe(3);
    assert!(kf.is_some());
    assert_eq!(kf.unwrap().seq, 0);

    let kf_exact = view.binary_search_keyframe(6);
    assert!(kf_exact.is_some());
    assert_eq!(kf_exact.unwrap().seq, 6);
}

#[test]
fn test_replay_export_vbr() -> Result<(), Box<dyn std::error::Error>> {
    let text = b"line 1\r\nline 2\r\nline 3\r\n";
    let chunks = [text.as_slice()];
    let stream = Stream::new(0, &chunks);
    let config = ReplayConfig::new(80, 24)?;
    let replay = Replay::build(stream, &config)?;

    let vbr_bytes = replay.export_vbr();
    let view = VbrView::parse(&vbr_bytes)?;

    assert_eq!(view.header().cols, 80);
    assert_eq!(view.header().rows, 24);
    assert_eq!(view.reconstruct_bytes(), text);
    Ok(())
}
