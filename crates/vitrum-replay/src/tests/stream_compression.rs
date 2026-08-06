use crate::stream::{CompressionAlgorithm, Stream};

#[test]
fn test_stream_rle_deflate_compression_roundtrip() {
    let mut data = Vec::new();
    data.extend_from_slice(b"Hello world!");
    data.resize(data.len() + 100, b' ');
    data.extend_from_slice(b"\x1b[32mLOG ENTRY\x1b[0m");
    data.resize(data.len() + 200, 0x00);

    let chunks = [data.as_slice()];
    let stream = Stream::new(1000, &chunks);

    let archive = stream.compress_archive(CompressionAlgorithm::RleDeflate, 64);
    assert!(archive.verify_checksums());
    assert!(archive.compression_ratio() > 1.0);

    let (base, decompressed) = archive.decompress().expect("decompression failed");
    assert_eq!(base, 1000);
    assert_eq!(decompressed, data);

    let range_data = archive
        .decompress_range(1000..1012)
        .expect("range decompression failed");
    assert_eq!(range_data, b"Hello world!");
}

#[test]
fn test_stream_zstd_chunked_compression_roundtrip() {
    let mut data = Vec::new();
    data.extend_from_slice(b"System log output line 1\r\n");
    data.resize(data.len() + 500, b'A');
    data.extend_from_slice(b"System log output line 2\r\n");

    let chunks = [data.as_slice()];
    let stream = Stream::new(5000, &chunks);

    let archive = stream.compress_archive(CompressionAlgorithm::ZstdChunked, 128);
    assert!(archive.verify_checksums());
    assert!(archive.compression_ratio() > 1.0);

    let (base, decompressed) = archive.decompress().expect("decompression failed");
    assert_eq!(base, 5000);
    assert_eq!(decompressed, data);
}
