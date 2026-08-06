use crate::pty::{PtyReader, PtyRingBuffer};
use std::io;

#[test]
fn pty_ring_buffer_basic_ops() {
    let mut ring = PtyRingBuffer::with_capacity(128);
    assert_eq!(ring.capacity(), 128);
    assert_eq!(ring.readable_len(), 0);
    assert_eq!(ring.writable_len(), 128);
    assert!(ring.is_empty());
    assert!(!ring.is_full());

    let (first, second) = ring.prepare_write_slices();
    assert_eq!(first.len(), 128);
    assert_eq!(second.len(), 0);

    first[..5].copy_from_slice(b"hello");
    ring.advance_write(5);

    assert_eq!(ring.readable_len(), 5);
    assert_eq!(ring.writable_len(), 123);

    let (read1, read2) = ring.prepare_read_slices();
    assert_eq!(read1, b"hello");
    assert_eq!(read2, &[] as &[u8]);

    ring.advance_read(5);
    assert_eq!(ring.readable_len(), 0);
    assert!(ring.is_empty());
}

#[test]
fn pty_ring_buffer_wrap_around() {
    let mut ring = PtyRingBuffer::with_capacity(64);
    
    // Fill 50 bytes and consume 40 bytes to shift tail/head near end
    let (w1, _) = ring.prepare_write_slices();
    w1[..50].fill(b'A');
    ring.advance_write(50);
    ring.advance_read(40);
    assert_eq!(ring.readable_len(), 10);

    // Now write 45 bytes, which wraps around
    let (w_first, w_second) = ring.prepare_write_slices();
    assert_eq!(w_first.len(), 14); // 64 - 50 = 14
    assert_eq!(w_second.len(), 40); // 40 available at front

    w_first.fill(b'B');
    w_second[..31].fill(b'C');
    ring.advance_write(45); // 14 + 31

    assert_eq!(ring.readable_len(), 55);

    let (r_first, r_second) = ring.prepare_read_slices();
    assert_eq!(r_first.len(), 24); // 10 'A's + 14 'B's
    assert_eq!(r_second.len(), 31); // 31 'C's

    ring.advance_read(55);
    assert!(ring.is_empty());
}

#[test]
fn pty_reader_direct_syscall_read() {
    let mut reader = PtyReader::with_capacity(256);

    // Simulated syscall read
    let n = reader
        .read_direct(|buf| {
            let data = b"PTY_OUTPUT_LINE_1\n";
            buf[..data.len()].copy_from_slice(data);
            Ok(data.len())
        })
        .expect("syscall failed");

    assert_eq!(n, 18);
    let (peek1, peek2) = reader.peek_slices();
    assert_eq!(peek1, b"PTY_OUTPUT_LINE_1\n");
    assert_eq!(peek2, &[] as &[u8]);

    let stats = reader.stats();
    assert_eq!(stats.bytes_read, 18);
    assert_eq!(stats.syscall_count, 1);
    assert_eq!(stats.zero_copy_reads, 1);

    reader.consume(18);
    assert_eq!(reader.peek_slices().0, &[] as &[u8]);
}

#[test]
fn pty_reader_fill_from_slice() {
    let mut reader = PtyReader::with_capacity(64);
    let data = b"Sample PTY payload that is larger than 16 bytes for testing";
    let count = reader.fill_from_slice(data);
    assert_eq!(count, 59); // Max readable given 64 cap

    let stats = reader.stats();
    assert_eq!(stats.bytes_read, 59);
}
