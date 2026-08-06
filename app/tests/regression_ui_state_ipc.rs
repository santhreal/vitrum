//! Integration and regression test suite covering UI state calculations,
//! packed modifier bitfield collision prevention, zero-copy binary buffer frame parsing,
//! and raw waker vtable memory safety invariants.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use std::thread;
use vitrum_proto::{
    FRAME_KIND_OUTPUT, FrameError, OUTPUT_HEADER_LEN, SessionId, decode_output, encode_output,
};

// ============================================================================
// Data Structures for Virtualized Overscan & Packed Key Chords
// ============================================================================

/// Calculates virtualized sidebar rendering bounds with overscan buffers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverscanWindow {
    pub start_index: usize,
    pub end_index: usize,
    pub total_items: usize,
    pub visible_start: usize,
    pub visible_end: usize,
}

/// Compute virtualized overscan indices ensuring viewport stability and boundary bounds.
pub fn calculate_overscan_window(
    scroll_top_px: f64,
    viewport_height_px: f64,
    row_height_px: f64,
    total_items: usize,
    overscan_count: usize,
) -> OverscanWindow {
    if total_items == 0 {
        return OverscanWindow {
            start_index: 0,
            end_index: 0,
            total_items: 0,
            visible_start: 0,
            visible_end: 0,
        };
    }

    let safe_row_height = if row_height_px.is_finite() && row_height_px > 0.0 {
        row_height_px
    } else {
        32.0
    };

    let safe_scroll_top = if scroll_top_px.is_finite() && scroll_top_px > 0.0 {
        scroll_top_px
    } else {
        0.0
    };

    let safe_viewport_height = if viewport_height_px.is_finite() && viewport_height_px > 0.0 {
        viewport_height_px
    } else {
        0.0
    };

    let visible_start = (safe_scroll_top / safe_row_height).floor() as usize;
    let visible_start = visible_start.min(total_items);

    let visible_end = ((safe_scroll_top + safe_viewport_height) / safe_row_height).ceil() as usize;
    let visible_end = visible_end.min(total_items);

    let start_index = visible_start.saturating_sub(overscan_count);
    let end_index = (visible_end + overscan_count).min(total_items);

    OverscanWindow {
        start_index,
        end_index,
        total_items,
        visible_start,
        visible_end,
    }
}

impl OverscanWindow {
    /// Expand the overscan window if necessary to guarantee inclusion of the selected row.
    pub fn force_include_selected(&mut self, selected_index: usize) {
        if selected_index < self.total_items {
            if selected_index < self.start_index {
                self.start_index = selected_index;
            }
            if selected_index >= self.end_index {
                self.end_index = (selected_index + 1).min(self.total_items);
            }
        }
    }
}

/// 32-bit packed representation of keyboard shortcut chords.
///
/// Layout:
/// - Bits 0..8: 8-bit Modifier bitflags (`Ctrl`, `Alt`, `Shift`, `Meta`, `Hyper`)
/// - Bits 8..32: 24-bit key code value
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PackedKeyChord(pub u32);

impl PackedKeyChord {
    pub const MOD_CTRL: u8 = 1 << 0;
    pub const MOD_ALT: u8 = 1 << 1;
    pub const MOD_SHIFT: u8 = 1 << 2;
    pub const MOD_META: u8 = 1 << 3;
    pub const MOD_HYPER: u8 = 1 << 4;

    pub const MOD_MASK: u32 = 0x0000_00FF;
    pub const KEY_MASK: u32 = 0xFFFF_FF00;

    /// Pack key code and modifier bitfield into a 32-bit chord.
    pub fn pack(key_code: u32, modifiers: u8) -> Result<Self, &'static str> {
        if key_code > 0x00FF_FFFF {
            return Err("key code exceeds 24-bit capacity limit");
        }
        let packed = ((key_code & 0x00FF_FFFF) << 8) | (modifiers as u32 & Self::MOD_MASK);
        Ok(Self(packed))
    }

    /// Unpack into `(key_code, modifier_bitflags)`.
    pub fn unpack(self) -> (u32, u8) {
        let key_code = (self.0 & Self::KEY_MASK) >> 8;
        let modifiers = (self.0 & Self::MOD_MASK) as u8;
        (key_code, modifiers)
    }

    /// Check if a specific modifier flag is set.
    pub fn has_modifier(self, flag: u8) -> bool {
        (self.unpack().1 & flag) != 0
    }
}

// ============================================================================
// Group 1: Virtualized Sidebar Overscan Window Calculations
// ============================================================================

/// WHY: Virtualized sidebar rendering must calculate visible and overscan indices accurately
/// so that scrolling does not produce visual flashing or attempt to render items beyond total bounds.
#[test]
fn test_overscan_window_basic_bounds_and_margins() {
    let window = calculate_overscan_window(100.0, 300.0, 50.0, 100, 3);
    // Visible rows: floor(100/50)=2, ceil((100+300)/50)=8
    assert_eq!(window.visible_start, 2);
    assert_eq!(window.visible_end, 8);
    // Overscan: 2 - 3 -> 0, 8 + 3 -> 11
    assert_eq!(window.start_index, 0);
    assert_eq!(window.end_index, 11);
    assert_eq!(window.total_items, 100);

    // Deep scroll test: scroll_top = 2000px (row 40)
    let deep_window = calculate_overscan_window(2000.0, 300.0, 50.0, 45, 5);
    // Visible: 40..45 (capped at total 45)
    assert_eq!(deep_window.visible_start, 40);
    assert_eq!(deep_window.visible_end, 45);
    assert_eq!(deep_window.start_index, 35);
    assert_eq!(deep_window.end_index, 45);
}

/// WHY: Negative scroll offsets, zero viewport heights, NaN/infinite values, or invalid row heights
/// must degrade gracefully without panicking or creating invalid ranges.
#[test]
fn test_overscan_window_zero_viewport_or_invalid_row_height() {
    // Total items = 0
    let empty = calculate_overscan_window(0.0, 500.0, 40.0, 0, 2);
    assert_eq!(empty.start_index, 0);
    assert_eq!(empty.end_index, 0);

    // Zero row height fallback (32.0)
    let zero_row = calculate_overscan_window(64.0, 128.0, 0.0, 10, 1);
    assert_eq!(zero_row.visible_start, 2);
    assert_eq!(zero_row.visible_end, 6);

    // Negative scroll top handling (clamped to 0.0)
    let neg_scroll = calculate_overscan_window(-150.0, 200.0, 50.0, 20, 2);
    assert_eq!(neg_scroll.visible_start, 0);
    assert_eq!(neg_scroll.visible_end, 4);
    assert_eq!(neg_scroll.start_index, 0);
    assert_eq!(neg_scroll.end_index, 6);

    // NaN / Infinity inputs
    let nan_scroll = calculate_overscan_window(f64::NAN, f64::INFINITY, -10.0, 15, 3);
    assert!(nan_scroll.start_index <= nan_scroll.end_index);
    assert!(nan_scroll.end_index <= 15);
}

/// WHY: When an active selection moves outside the rendered overscan region (e.g. keyboard navigation),
/// the overscan window calculation must force-include the selected index while preserving range validity.
#[test]
fn test_overscan_window_pin_selected_item_extension() {
    let mut window = calculate_overscan_window(500.0, 200.0, 50.0, 100, 2);
    // visible_start=10, visible_end=14, start_index=8, end_index=16
    assert_eq!(window.start_index, 8);
    assert_eq!(window.end_index, 16);

    // Force include item 3 (above start_index 8)
    window.force_include_selected(3);
    assert_eq!(window.start_index, 3);
    assert_eq!(window.end_index, 16);

    // Force include item 25 (below end_index 16)
    window.force_include_selected(25);
    assert_eq!(window.start_index, 3);
    assert_eq!(window.end_index, 26);

    // Out-of-bounds selection index must be ignored safely
    window.force_include_selected(999);
    assert_eq!(window.end_index, 26);
}

// ============================================================================
// Group 2: 32-bit PackedKeyChord Modifier Bitfield Collision Prevention
// ============================================================================

/// WHY: Modifier bitfields must use non-overlapping bit positions to ensure that any combination of
/// Ctrl, Alt, Shift, Meta, and Hyper flags yields a unique 32-bit representation without bitwise collisions.
#[test]
fn test_packed_key_chord_modifier_bitfield_no_collisions() {
    let modifiers = [
        PackedKeyChord::MOD_CTRL,
        PackedKeyChord::MOD_ALT,
        PackedKeyChord::MOD_SHIFT,
        PackedKeyChord::MOD_META,
        PackedKeyChord::MOD_HYPER,
    ];

    // Ensure all 32 combinations (2^5) produce distinct bitfield masks
    let mut seen_masks = std::collections::HashSet::new();
    for i in 0..32u8 {
        let mut mask = 0u8;
        for (idx, &flag) in modifiers.iter().enumerate() {
            if (i & (1 << idx)) != 0 {
                mask |= flag;
            }
        }
        assert!(
            seen_masks.insert(mask),
            "duplicate modifier mask generated: 0x{mask:02X}"
        );

        let chord = PackedKeyChord::pack(0x41, mask).expect("packing key chord failed");
        let (unpacked_key, unpacked_mod) = chord.unpack();
        assert_eq!(unpacked_key, 0x41);
        assert_eq!(unpacked_mod, mask);
    }
}

/// WHY: Key codes up to 24 bits (0x00FF_FFFF) must be completely isolated in the upper 24 bits
/// so that high-valued key codes never bleed into or corrupt lower modifier bitfield flags.
#[test]
fn test_packed_key_chord_key_code_mask_isolation() {
    let max_key = 0x00FF_FFFF;
    let all_mods = PackedKeyChord::MOD_CTRL
        | PackedKeyChord::MOD_ALT
        | PackedKeyChord::MOD_SHIFT
        | PackedKeyChord::MOD_META;

    let chord = PackedKeyChord::pack(max_key, all_mods).expect("max key chord packing failed");
    let (key, mods) = chord.unpack();
    assert_eq!(key, max_key);
    assert_eq!(mods, all_mods);

    // Attempting to pack a key code exceeding 24 bits must fail cleanly
    assert!(PackedKeyChord::pack(0x0100_0000, all_mods).is_err());

    // Verify individual modifier checks against high key code
    assert!(chord.has_modifier(PackedKeyChord::MOD_CTRL));
    assert!(chord.has_modifier(PackedKeyChord::MOD_ALT));
    assert!(chord.has_modifier(PackedKeyChord::MOD_SHIFT));
    assert!(chord.has_modifier(PackedKeyChord::MOD_META));
    assert!(!chord.has_modifier(PackedKeyChord::MOD_HYPER));
}

/// WHY: The packing and unpacking operations for `PackedKeyChord` must be bijective for all boundary
/// key codes (0, 1, ASCII, Unicode codepoints, 24-bit limit) across all modifier bitmask combinations.
#[test]
fn test_packed_key_chord_round_trip_equality() {
    let test_keys = [0u32, 1, 27, 65, 255, 1000, 65535, 0x00FF_FFFF];
    for &key in &test_keys {
        for mod_mask in 0..=31u8 {
            let chord = PackedKeyChord::pack(key, mod_mask).expect("round trip pack");
            let (unpacked_key, unpacked_mod) = chord.unpack();
            assert_eq!(unpacked_key, key, "key mismatch for key {key}");
            assert_eq!(
                unpacked_mod, mod_mask,
                "mod mismatch for key {key}, mod {mod_mask}"
            );
        }
    }
}

// ============================================================================
// Group 3: Zero-Copy Binary Buffer Frame Parsing
// ============================================================================

/// WHY: The data-plane binary output decoder `decode_output` must perform zero memory allocations
/// and return a payload slice pointing directly into the input byte buffer offset by `OUTPUT_HEADER_LEN`.
#[test]
fn test_zero_copy_binary_frame_parsing_pointer_identity() {
    let session = SessionId(42);
    let seq = 1024;
    let payload = b"Hello zero-copy PTY frame payload!";

    let encoded = encode_output(session, seq, payload);
    assert_eq!(encoded.len(), OUTPUT_HEADER_LEN + payload.len());

    let (decoded_session, decoded_seq, decoded_payload) =
        decode_output(&encoded).expect("decoding valid output frame failed");

    assert_eq!(decoded_session, session);
    assert_eq!(decoded_seq, seq);
    assert_eq!(decoded_payload, payload);

    // Verify zero-copy pointer identity: payload slice address must match encoded buffer offset 17
    let expected_ptr = unsafe { encoded.as_ptr().add(OUTPUT_HEADER_LEN) };
    assert_eq!(
        decoded_payload.as_ptr(),
        expected_ptr,
        "decoded payload slice is not borrowing directly from input frame buffer"
    );
}

/// WHY: Any binary frame containing fewer than `OUTPUT_HEADER_LEN` (17 bytes) must be rejected
/// immediately with `FrameError::TooShort` specifying the exact truncated length.
#[test]
fn test_zero_copy_binary_frame_parsing_header_truncation() {
    let empty_frame: &[u8] = &[];
    assert_eq!(
        decode_output(empty_frame),
        Err(FrameError::TooShort { len: 0 })
    );

    let partial_header = [FRAME_KIND_OUTPUT, 1, 2, 3, 4, 5, 6, 7, 8];
    assert_eq!(
        decode_output(&partial_header),
        Err(FrameError::TooShort { len: 9 })
    );

    let exact_16_bytes = [0u8; 16];
    assert_eq!(
        decode_output(&exact_16_bytes),
        Err(FrameError::TooShort { len: 16 })
    );

    // Exactly 17 bytes (header with empty payload) must succeed with empty payload slice
    let exact_17_bytes = encode_output(SessionId(1), 0, &[]);
    let (s, q, p) = decode_output(&exact_17_bytes).expect("17-byte empty payload frame");
    assert_eq!(s, SessionId(1));
    assert_eq!(q, 0);
    assert!(p.is_empty());
}

/// WHY: Binary frames with unknown header kind bytes must be rejected with `FrameError::UnknownKind`
/// to prevent misinterpreting incompatible frame types or corrupted IPC stream data.
#[test]
fn test_zero_copy_binary_frame_parsing_unknown_kind_rejection() {
    let mut invalid_kind_frame = encode_output(SessionId(99), 500, b"data");
    invalid_kind_frame[0] = 255; // Replace FRAME_KIND_OUTPUT (1) with 255

    assert_eq!(
        decode_output(&invalid_kind_frame),
        Err(FrameError::UnknownKind(255))
    );

    invalid_kind_frame[0] = 0;
    assert_eq!(
        decode_output(&invalid_kind_frame),
        Err(FrameError::UnknownKind(0))
    );
}

/// WHY: Iterating over a stream buffer containing multiple concatenated binary output frames
/// must extract each frame zero-copy without allocating intermediate buffers or corrupting slice offsets.
#[test]
fn test_zero_copy_concatenated_stream_frame_iterator() {
    let mut stream_buffer = Vec::new();
    let frame_data = [
        (SessionId(1), 100u64, &b"chunk-1"[..]),
        (SessionId(2), 200u64, &b"chunk-2-longer"[..]),
        (SessionId(3), 300u64, &b"chunk-3-final"[..]),
    ];

    for &(session, seq, payload) in &frame_data {
        stream_buffer.extend(encode_output(session, seq, payload));
    }

    let mut offset = 0;
    for (idx, &(session, seq, payload)) in frame_data.iter().enumerate() {
        let frame_len = OUTPUT_HEADER_LEN + payload.len();
        let frame_slice = &stream_buffer[offset..offset + frame_len];

        let (decoded_session, decoded_seq, decoded_payload) =
            decode_output(frame_slice).expect("stream parsing frame");

        assert_eq!(decoded_session, session);
        assert_eq!(decoded_seq, seq);
        assert_eq!(decoded_payload, payload);

        // Verify zero-copy slice pointer
        let expected_ptr = unsafe { frame_slice.as_ptr().add(OUTPUT_HEADER_LEN) };
        assert_eq!(
            decoded_payload.as_ptr(),
            expected_ptr,
            "frame {idx} payload slice pointer mismatch"
        );

        offset += frame_len;
    }

    assert_eq!(offset, stream_buffer.len());
}

// ============================================================================
// Group 4: Raw Waker Vtable Safety
// ============================================================================

/// WHY: No-op raw waker vtables must safely tolerate null data pointers across clone, wake,
/// wake_by_ref, and drop calls without dereferencing null pointers or causing memory corruption.
#[test]
fn test_raw_waker_vtable_null_pointer_safety() {
    static NOOP_VTABLE: RawWakerVTable = RawWakerVTable::new(
        |_| RawWaker::new(std::ptr::null(), &NOOP_VTABLE),
        |_| {},
        |_| {},
        |_| {},
    );

    let raw_waker = RawWaker::new(std::ptr::null(), &NOOP_VTABLE);
    let waker = unsafe { Waker::from_raw(raw_waker) };

    // Invoke clone, wake_by_ref, and drop on the waker backed by a null pointer
    let cloned_waker = waker.clone();
    waker.wake_by_ref();
    cloned_waker.wake_by_ref();

    let mut cx = Context::from_waker(&waker);
    assert!(cx.waker().will_wake(&cloned_waker));

    // Explicit drop calls
    drop(waker);
    drop(cloned_waker);
}

/// WHY: Dynamic wakers that track reference counts (e.g. atomic wakers) must correctly increment
/// refcounts on `clone` and decrement on `wake` / `drop` without memory leaks or double-frees.
#[test]
fn test_raw_waker_vtable_atomic_refcount_lifecycle() {
    struct CustomWakerState {
        ref_count: AtomicUsize,
        wake_count: AtomicUsize,
    }

    static CUSTOM_VTABLE: RawWakerVTable = RawWakerVTable::new(
        |ptr| {
            let state = unsafe { &*(ptr as *const CustomWakerState) };
            state.ref_count.fetch_add(1, Ordering::SeqCst);
            RawWaker::new(ptr, &CUSTOM_VTABLE)
        },
        |ptr| {
            let state = unsafe { &*(ptr as *const CustomWakerState) };
            state.wake_count.fetch_add(1, Ordering::SeqCst);
            let prev = state.ref_count.fetch_sub(1, Ordering::SeqCst);
            if prev == 1 {
                unsafe { drop(Box::from_raw(ptr as *mut CustomWakerState)) };
            }
        },
        |ptr| {
            let state = unsafe { &*(ptr as *const CustomWakerState) };
            state.wake_count.fetch_add(1, Ordering::SeqCst);
        },
        |ptr| {
            let state = unsafe { &*(ptr as *const CustomWakerState) };
            let prev = state.ref_count.fetch_sub(1, Ordering::SeqCst);
            if prev == 1 {
                unsafe { drop(Box::from_raw(ptr as *mut CustomWakerState)) };
            }
        },
    );

    let state_box = Box::new(CustomWakerState {
        ref_count: AtomicUsize::new(1),
        wake_count: AtomicUsize::new(0),
    });
    let state_ptr = Box::into_raw(state_box);

    let raw = RawWaker::new(state_ptr as *const (), &CUSTOM_VTABLE);
    let waker1 = unsafe { Waker::from_raw(raw) };

    // Clone waker -> ref_count = 2
    let waker2 = waker1.clone();
    unsafe {
        assert_eq!((*state_ptr).ref_count.load(Ordering::SeqCst), 2);
    }

    // Wake by ref -> wake_count = 1, ref_count = 2
    waker2.wake_by_ref();
    unsafe {
        assert_eq!((*state_ptr).wake_count.load(Ordering::SeqCst), 1);
        assert_eq!((*state_ptr).ref_count.load(Ordering::SeqCst), 2);
    }

    // Consume waker2 -> ref_count = 1, wake_count = 2
    waker2.wake();
    unsafe {
        assert_eq!((*state_ptr).wake_count.load(Ordering::SeqCst), 2);
        assert_eq!((*state_ptr).ref_count.load(Ordering::SeqCst), 1);
    }

    // Drop remaining waker1 -> ref_count = 0 (frees memory cleanly)
    drop(waker1);
}

/// WHY: Waker instances constructed from raw vtables must be safe to move across threads,
/// clone concurrently, and wake in multi-threaded task drivers without data races.
#[test]
fn test_raw_waker_vtable_waker_from_raw_concurrency() {
    let wake_counter = Arc::new(AtomicUsize::new(0));

    struct ThreadState {
        counter: Arc<AtomicUsize>,
    }

    static THREAD_VTABLE: RawWakerVTable = RawWakerVTable::new(
        |ptr| {
            let state = unsafe { &*(ptr as *const ThreadState) };
            let new_state = Box::new(ThreadState {
                counter: state.counter.clone(),
            });
            RawWaker::new(Box::into_raw(new_state) as *const (), &THREAD_VTABLE)
        },
        |ptr| {
            let state = unsafe { Box::from_raw(ptr as *mut ThreadState) };
            state.counter.fetch_add(1, Ordering::SeqCst);
        },
        |ptr| {
            let state = unsafe { &*(ptr as *const ThreadState) };
            state.counter.fetch_add(1, Ordering::SeqCst);
        },
        |ptr| {
            unsafe { drop(Box::from_raw(ptr as *mut ThreadState)) };
        },
    );

    let state = Box::new(ThreadState {
        counter: wake_counter.clone(),
    });
    let raw = RawWaker::new(Box::into_raw(state) as *const (), &THREAD_VTABLE);
    let main_waker = unsafe { Waker::from_raw(raw) };

    let mut handles = Vec::new();
    for _ in 0..4 {
        let waker_clone = main_waker.clone();
        handles.push(thread::spawn(move || {
            waker_clone.wake_by_ref();
            waker_clone.wake();
        }));
    }

    for handle in handles {
        handle.join().expect("waker thread join failed");
    }

    drop(main_waker);

    // 4 threads * 2 wakes = 8
    assert_eq!(wake_counter.load(Ordering::SeqCst), 8);
}
