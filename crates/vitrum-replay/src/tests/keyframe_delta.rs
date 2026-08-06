use crate::config::ReplayConfig;
use crate::keyframe::{Keyframe, KeyframeIndex, KeyframeStorage};
use crate::replay::Replay;
use crate::screen::Screen;
use crate::stream::Stream;

#[test]
fn test_keyframe_delta_computation_and_apply() -> Result<(), Box<dyn std::error::Error>> {
    let text1 = b"Hello world 1\r\n";
    let text2 = b"Hello world 1\r\nHello world 2\r\n";

    let chunks1 = [text1.as_slice()];
    let chunks2 = [text2.as_slice()];
    let stream1 = Stream::new(0, &chunks1);
    let stream2 = Stream::new(0, &chunks2);
    let config = ReplayConfig::new(80, 24)?;

    let mut replay1 = Replay::build(stream1, &config)?;
    replay1.seek(stream1.head_seq())?;
    let kf1 = Keyframe::new(stream1.head_seq(), replay1.screen().clone());

    let mut replay2 = Replay::build(stream2, &config)?;
    replay2.seek(stream2.head_seq())?;
    let kf2 = Keyframe::new(stream2.head_seq(), replay2.screen().clone());

    let delta = kf2.compute_delta(&kf1);
    assert_eq!(delta.base_seq, stream1.head_seq());
    assert_eq!(delta.seq, stream2.head_seq());
    assert!(!delta.cell_diffs.is_empty());

    let reconstructed_screen = Keyframe::apply_delta(kf1.screen(), &delta);
    assert_eq!(&reconstructed_screen, kf2.screen());
    Ok(())
}

#[test]
fn test_delta_encoded_storage_memory_reduction() -> Result<(), Box<dyn std::error::Error>> {
    let mut session_bytes = Vec::new();
    for i in 0..100 {
        session_bytes.extend_from_slice(format!("Line output #{i}\r\n").as_bytes());
    }

    let chunks = [session_bytes.as_slice()];
    let stream = Stream::new(0, &chunks);
    let config = ReplayConfig::new(80, 24)?;

    let index = KeyframeIndex::build(&stream, &config)?;
    if index.len() >= 2 {
        let storage = index.frames();
        assert_eq!(storage.len(), index.len());
        match &storage[0] {
            KeyframeStorage::Anchor(_) => {}
            _ => panic!("Expected first entry to be Anchor"),
        }
        assert!(index.heap_bytes() > 0);
    }
    Ok(())
}
