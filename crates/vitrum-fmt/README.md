# vitrum-fmt

Formatting, and only formatting. Every string a person reads in a terminal
multiplexer's sidebar, tab strip, or status line: relative timestamps, elapsed
durations, shortened paths, truncated titles, byte sizes, counted nouns, exit
statuses, and git heads.

This crate never decides anything. It takes values you already have and returns
a string. It does not read the clock, the environment, or the filesystem, so
`now`, the UTC offset, and `$HOME` are all parameters and every output is
reproducible.

```rust
use std::time::Duration;
use vitrum_fmt::{TimeFormat, Timestamp, bytes, count, duration, path};

let clock = TimeFormat::new(Timestamp::from_secs(1_700_000_000), 0);
assert_eq!(clock.relative(Timestamp::from_secs(1_699_999_748)), "4m");
assert_eq!(duration::compact(Duration::from_secs(252)), "4m 12s");
assert_eq!(bytes::binary(1_572_864), "1.5 MiB");
assert_eq!(count::count_s(2, "session"), "2 sessions");
assert_eq!(
    path::shorten_home_relative("/home/mk/src/vitrum/crates/vitrum-fmt", "/home/mk", 24),
    "~/\u{2026}/crates/vitrum-fmt",
);
```

Widths are terminal columns measured over grapheme clusters, so CJK, emoji, and
combining marks lay out correctly and truncation never splits a glyph.

Part of [vitrum](https://github.com/santhreal/vitrum). MIT licensed.
