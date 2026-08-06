//! Walking lines across a ring buffer's two halves without stitching it back
//! together.
//!
//! # The shape of the input
//!
//! A session's scrollback is a fixed-size ring. Reading it out gives two
//! contiguous runs — the older tail and the newer head — and the join between
//! them falls wherever the write cursor happens to be, which is to say in the
//! middle of a line, in the middle of a word, and quite often in the middle of
//! a UTF-8 character or an escape sequence.
//!
//! The obvious fix is to `concat()` the halves and search the result. For
//! twenty sessions of ten megabytes that is 200 MB of copying and 200 MB of
//! peak memory to answer a query the user expects to be instant, and vitrum's
//! whole idle-cost budget is smaller than that allocation.
//!
//! So nothing is copied. [`Lines`] walks the chunks in order and yields a
//! [`LineSpan`], which is a *locator* — offset and length in the haystack's own
//! coordinate space — rather than bytes. A span is `Copy` and 24 bytes, so the
//! before-context ring is a fixed array of them and holding one costs nothing.
//! Bytes are produced only for the handful of lines that end up in a result.
//!
//! # Straddling
//!
//! A line that crosses the join is the only case that needs a copy, and there
//! is at most one such line per chunk boundary — two per ring, not one per
//! line. [`Chunked::materialize`] copies it into a caller-owned scratch buffer
//! that is reused for the whole scan, so the copy costs no allocation.
//!
//! A UTF-8 character split across the join needs nothing special *because* of
//! this: the line is reassembled before anything looks at it as text, so the
//! two halves of the character are adjacent again by the time it matters.

use memchr::memchr;

/// A session's scrollback, as the ring hands it over.
///
/// `chunks` are in stream order, oldest first. A ring gives two; a contiguous
/// buffer gives one; the type does not care how many.
///
/// # Single-buffer callers
///
/// ```
/// # use vitrum_search::Haystack;
/// let scrollback: &[u8] = b"line one\nline two\n";
/// let haystack = Haystack {
///     session: 7,
///     base_seq: 0,
///     chunks: std::slice::from_ref(&scrollback),
/// };
/// assert_eq!(haystack.len(), 18);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Haystack<'a> {
    /// Which session this scrollback belongs to.
    pub session: u64,
    /// Cumulative stream offset of the first byte of `chunks[0]`.
    ///
    /// This is the session's `seq`: the ring has already discarded `base_seq`
    /// bytes, so a hit at haystack offset `n` is at `base_seq + n` in the
    /// stream, and that is what the protocol's data plane numbers by.
    pub base_seq: u64,
    /// Contiguous runs of scrollback, oldest first.
    pub chunks: &'a [&'a [u8]],
}

impl Haystack<'_> {
    /// Total bytes across every chunk.
    pub fn len(&self) -> u64 {
        self.chunks.iter().map(|chunk| chunk.len() as u64).sum()
    }

    /// Is there anything to search?
    pub fn is_empty(&self) -> bool {
        self.chunks.iter().all(|chunk| chunk.is_empty())
    }
}

/// Where one line lives, in haystack coordinates.
///
/// The newline itself is not part of the span, so a `"a\n"` haystack yields one
/// span of length 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineSpan {
    /// Offset of the first byte, relative to the start of the haystack.
    pub offset: u64,
    /// Length in bytes, excluding the terminating newline.
    pub len: usize,
    /// Zero-based position of this line within the haystack.
    pub index: u64,
}

/// Random access over a chunk list.
#[derive(Debug, Clone, Copy)]
pub struct Chunked<'a> {
    chunks: &'a [&'a [u8]],
}

impl<'a> Chunked<'a> {
    pub fn new(chunks: &'a [&'a [u8]]) -> Self {
        Self { chunks }
    }

    /// The bytes of `span`, borrowed when it lies inside one chunk.
    ///
    /// `scratch` is only touched for a span that crosses a chunk boundary, and
    /// is cleared first. Passing the same scratch for every line is what makes
    /// straddling free of allocation after the first one.
    pub fn materialize<'s>(&self, span: LineSpan, scratch: &'s mut Vec<u8>) -> &'s [u8]
    where
        'a: 's,
    {
        if let Some(slice) = self.contiguous(span) {
            return slice;
        }
        scratch.clear();
        self.copy_into(span, scratch);
        scratch.as_slice()
    }

    /// The bytes of `span` when it lies inside a single chunk.
    pub fn contiguous(&self, span: LineSpan) -> Option<&'a [u8]> {
        let (chunk, offset) = self.locate(span.offset)?;
        let bytes = self.chunks.get(chunk)?;
        if offset + span.len <= bytes.len() {
            Some(&bytes[offset..offset + span.len])
        } else {
            None
        }
    }

    /// Append the bytes of `span` to `out`.
    pub fn copy_into(&self, span: LineSpan, out: &mut Vec<u8>) {
        let Some((mut chunk, mut offset)) = self.locate(span.offset) else {
            return;
        };
        let mut remaining = span.len;
        while remaining > 0 && chunk < self.chunks.len() {
            let bytes = self.chunks[chunk];
            let available = bytes.len().saturating_sub(offset);
            let take = available.min(remaining);
            out.extend_from_slice(&bytes[offset..offset + take]);
            remaining -= take;
            chunk += 1;
            offset = 0;
        }
    }

    /// Chunk index and within-chunk offset for a haystack offset.
    ///
    /// An offset exactly at the end of the data has no byte, and returns
    /// `None`; this is what makes a zero-length final line safe.
    pub(crate) fn locate(&self, offset: u64) -> Option<(usize, usize)> {
        let mut remaining = offset;
        for (index, chunk) in self.chunks.iter().enumerate() {
            let len = chunk.len() as u64;
            if remaining < len {
                return Some((index, remaining as usize));
            }
            remaining -= len;
        }
        None
    }
}

/// Newline-delimited lines across a chunk list.
///
/// Allocates nothing, ever. The final line is yielded even without a trailing
/// newline, because a ring's newest bytes are routinely a partial line the
/// agent is still writing.
#[derive(Debug)]
pub struct Lines<'a> {
    chunks: &'a [&'a [u8]],
    chunk: usize,
    pos: usize,
    offset: u64,
    index: u64,
}

impl<'a> Lines<'a> {
    pub fn new(chunks: &'a [&'a [u8]]) -> Self {
        Self {
            chunks,
            chunk: 0,
            pos: 0,
            offset: 0,
            index: 0,
        }
    }
}

impl Iterator for Lines<'_> {
    type Item = LineSpan;

    fn next(&mut self) -> Option<LineSpan> {
        // Empty chunks are legal — a ring whose head half is empty hands over
        // a zero-length slice — and must not end iteration.
        while self.chunk < self.chunks.len() && self.pos >= self.chunks[self.chunk].len() {
            self.chunk += 1;
            self.pos = 0;
        }
        if self.chunk >= self.chunks.len() {
            return None;
        }

        let start = self.offset;
        let mut len = 0usize;
        let mut chunk = self.chunk;
        let mut pos = self.pos;

        loop {
            let Some(bytes) = self.chunks.get(chunk) else {
                // Ran out of data with no newline: the last line is unterminated.
                self.chunk = chunk;
                self.pos = 0;
                self.offset = start + len as u64;
                let span = LineSpan {
                    offset: start,
                    len,
                    index: self.index,
                };
                self.index += 1;
                return Some(span);
            };
            if pos >= bytes.len() {
                chunk += 1;
                pos = 0;
                continue;
            }
            match memchr(b'\n', &bytes[pos..]) {
                Some(found) => {
                    len += found;
                    self.chunk = chunk;
                    self.pos = pos + found + 1;
                    // +1 for the newline, which belongs to this line's extent
                    // even though it is not part of its bytes.
                    self.offset = start + len as u64 + 1;
                    let span = LineSpan {
                        offset: start,
                        len,
                        index: self.index,
                    };
                    self.index += 1;
                    return Some(span);
                }
                None => {
                    len += bytes.len() - pos;
                    chunk += 1;
                    pos = 0;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(chunks: &[&[u8]]) -> Vec<LineSpan> {
        Lines::new(chunks).collect()
    }

    fn texts(chunks: &[&[u8]]) -> Vec<String> {
        let view = Chunked::new(chunks);
        let mut scratch = Vec::new();
        Lines::new(chunks)
            .map(|span| {
                String::from_utf8(view.materialize(span, &mut scratch).to_vec()).expect("utf8")
            })
            .collect()
    }

    /// Locks out the newline being counted as part of the line, which would put
    /// a `\n` at the end of every returned context line and break `$` anchors.
    #[test]
    fn newline_terminates_but_does_not_belong_to_the_line() {
        let chunks: &[&[u8]] = &[b"abc\ndef\n"];
        assert_eq!(
            spans(chunks),
            vec![
                LineSpan {
                    offset: 0,
                    len: 3,
                    index: 0
                },
                LineSpan {
                    offset: 4,
                    len: 3,
                    index: 1
                },
            ]
        );
        assert_eq!(texts(chunks), vec!["abc", "def"]);
    }

    /// Locks out a trailing newline producing a phantom empty final line, which
    /// would add a blank hit-context row to every result.
    #[test]
    fn trailing_newline_does_not_produce_an_extra_line() {
        assert_eq!(spans(&[b"only\n"]).len(), 1);
        assert_eq!(spans(&[b"only"]).len(), 1);
        assert_eq!(spans(&[b""]).len(), 0);
        assert_eq!(spans(&[]).len(), 0);
    }

    /// Locks out the newest, still-being-written line being skipped. A ring's
    /// head is almost always a partial line and it is the most interesting one.
    #[test]
    fn unterminated_final_line_is_still_yielded() {
        let chunks: &[&[u8]] = &[b"done\npartial"];
        assert_eq!(
            spans(chunks),
            vec![
                LineSpan {
                    offset: 0,
                    len: 4,
                    index: 0
                },
                LineSpan {
                    offset: 5,
                    len: 7,
                    index: 1
                },
            ]
        );
        assert_eq!(texts(chunks), vec!["done", "partial"]);
    }

    /// Locks out empty lines being swallowed, which would shift every
    /// subsequent line index and misreport context.
    #[test]
    fn empty_lines_are_real_lines() {
        let chunks: &[&[u8]] = &[b"a\n\n\nb\n"];
        assert_eq!(
            spans(chunks)
                .iter()
                .map(|s| (s.offset, s.len))
                .collect::<Vec<_>>(),
            vec![(0, 1), (2, 0), (3, 0), (4, 1)]
        );
        assert_eq!(texts(chunks), vec!["a", "", "", "b"]);
    }

    /// Locks out a line split by the ring join being reported as two lines,
    /// which is the single most damaging bug in this module: the match is on
    /// neither half.
    #[test]
    fn a_line_split_across_chunks_is_one_line() {
        let chunks: &[&[u8]] = &[b"first\nsplit-", b"here\nlast\n"];
        assert_eq!(
            spans(chunks),
            vec![
                LineSpan {
                    offset: 0,
                    len: 5,
                    index: 0
                },
                LineSpan {
                    offset: 6,
                    len: 10,
                    index: 1
                },
                LineSpan {
                    offset: 17,
                    len: 4,
                    index: 2
                },
            ]
        );
        assert_eq!(texts(chunks), vec!["first", "split-here", "last"]);
    }

    /// Locks out a line spanning three or more chunks losing its middle. Two
    /// chunks is the ring case, but nothing in the API promises only two.
    #[test]
    fn a_line_spanning_three_chunks_is_reassembled_in_order() {
        let chunks: &[&[u8]] = &[b"one-", b"two-", b"three\ntail"];
        assert_eq!(texts(chunks), vec!["one-two-three", "tail"]);
        assert_eq!(
            spans(chunks)[0],
            LineSpan {
                offset: 0,
                len: 13,
                index: 0
            }
        );
    }

    /// Locks out a UTF-8 character split by the ring join being corrupted.
    /// A three-byte CJK character cut after its first byte must come back whole.
    #[test]
    fn utf8_split_across_the_join_is_reassembled() {
        let full = "prefix \u{4e2d}\u{6587} suffix\n".as_bytes();
        // Cut in the middle of the first CJK character.
        let cut = 8;
        assert!(std::str::from_utf8(&full[..cut]).is_err());
        let chunks: &[&[u8]] = &[&full[..cut], &full[cut..]];
        assert_eq!(texts(chunks), vec!["prefix \u{4e2d}\u{6587} suffix"]);
    }

    /// Locks out an empty chunk ending iteration early. A ring whose head is
    /// empty hands over a zero-length slice and everything after it would be
    /// silently unsearchable.
    #[test]
    fn empty_chunks_are_skipped_not_terminal() {
        let chunks: &[&[u8]] = &[b"", b"a\n", b"", b"", b"b\n", b""];
        assert_eq!(texts(chunks), vec!["a", "b"]);
        assert_eq!(
            spans(chunks).iter().map(|s| s.offset).collect::<Vec<_>>(),
            vec![0, 2]
        );
    }

    /// Locks out offsets restarting per chunk instead of accumulating across
    /// them, which would report every hit in the second half of a ring at the
    /// wrong seq.
    #[test]
    fn offsets_accumulate_across_chunk_boundaries() {
        let chunks: &[&[u8]] = &[b"aaa\n", b"bbb\n", b"ccc\n"];
        assert_eq!(
            spans(chunks).iter().map(|s| s.offset).collect::<Vec<_>>(),
            vec![0, 4, 8]
        );
    }

    /// Locks out line indices being per-chunk rather than per-haystack, which
    /// would make "line 3" ambiguous within one session.
    #[test]
    fn line_indices_are_continuous_across_chunks() {
        let chunks: &[&[u8]] = &[b"a\nb\n", b"c\nd\n"];
        assert_eq!(
            spans(chunks).iter().map(|s| s.index).collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
    }

    /// Locks out `materialize` copying when it does not need to. A borrowed
    /// return is what keeps the scan free of per-line work.
    #[test]
    fn contiguous_lines_are_borrowed_not_copied() {
        let chunks: &[&[u8]] = &[b"inline\nalso\n"];
        let view = Chunked::new(chunks);
        let mut scratch = Vec::new();
        for span in Lines::new(chunks) {
            assert!(
                view.contiguous(span).is_some(),
                "line at {} should be borrowable",
                span.offset
            );
            let bytes = view.materialize(span, &mut scratch);
            assert!(
                bytes.as_ptr() >= chunks[0].as_ptr(),
                "materialize must borrow from the chunk"
            );
        }
        assert!(
            scratch.is_empty(),
            "the scratch must stay untouched when nothing straddles"
        );
    }

    /// Locks out `contiguous` returning a truncated slice for a straddling
    /// line, which would search only the first half and report a short line.
    #[test]
    fn straddling_lines_report_as_non_contiguous() {
        let chunks: &[&[u8]] = &[b"abc", b"def\n"];
        let view = Chunked::new(chunks);
        let span = Lines::new(chunks).next().expect("one line");
        assert_eq!(span.len, 6);
        assert_eq!(view.contiguous(span), None);
        let mut scratch = Vec::new();
        assert_eq!(view.materialize(span, &mut scratch), b"abcdef");
    }

    /// Locks out the scratch accumulating across straddling lines, which would
    /// concatenate them into one enormous bogus line.
    #[test]
    fn scratch_is_cleared_for_each_straddling_line() {
        let chunks: &[&[u8]] = &[b"aa", b"a\nbb", b"b\n"];
        let view = Chunked::new(chunks);
        let mut scratch = Vec::new();
        let collected: Vec<Vec<u8>> = Lines::new(chunks)
            .map(|span| view.materialize(span, &mut scratch).to_vec())
            .collect();
        assert_eq!(collected, vec![b"aaa".to_vec(), b"bbb".to_vec()]);
    }

    /// Locks out `len` summing the wrong thing, which the benchmark's
    /// throughput figure depends on being exact.
    #[test]
    fn haystack_length_is_the_sum_of_its_chunks() {
        let chunks: &[&[u8]] = &[b"abc", b"", b"de"];
        let haystack = Haystack {
            session: 1,
            base_seq: 0,
            chunks,
        };
        assert_eq!(haystack.len(), 5);
        assert!(!haystack.is_empty());

        let empty: &[&[u8]] = &[b"", b""];
        assert!(
            Haystack {
                session: 1,
                base_seq: 0,
                chunks: empty
            }
            .is_empty()
        );
    }

    /// Locks out `locate` walking past the end and panicking on a span whose
    /// offset is exactly the haystack length.
    #[test]
    fn locating_past_the_end_is_none_not_a_panic() {
        let chunks: &[&[u8]] = &[b"abc", b"de"];
        let view = Chunked::new(chunks);
        assert_eq!(view.locate(0), Some((0, 0)));
        assert_eq!(view.locate(2), Some((0, 2)));
        assert_eq!(view.locate(3), Some((1, 0)));
        assert_eq!(view.locate(4), Some((1, 1)));
        assert_eq!(view.locate(5), None);
        assert_eq!(view.locate(9_999), None);
    }

    /// Locks out iteration failing to terminate on a chunk list that is all
    /// newlines, which is the densest input the line walker can see.
    #[test]
    fn dense_newlines_terminate_and_count_correctly() {
        let data = vec![b'\n'; 1000];
        let chunks: &[&[u8]] = &[&data[..500], &data[500..]];
        assert_eq!(Lines::new(chunks).count(), 1000);
    }
}
