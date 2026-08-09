//! A child that writes a burst and exits immediately must lose nothing.

#[cfg(not(windows))]
use crate::SessionManager;
#[cfg(not(windows))]
use crate::tests::helpers::{collect, settled, shell_spec, wait_exit};

/// Every byte a child wrote before exiting must be published, whatever the
/// volume and however fast the exit followed it.
///
/// WHY: the end of a session's output used to be decided by a stopwatch. Once
/// the child was reaped, the coalescer allowed the reader one flush window of
/// silence and then quit, dropping its end of the byte channel. The reader was
/// still holding a read it had already taken from the kernel and was feeding to
/// the terminal engine, and when it finished parsing there was nobody left to
/// hand the bytes to, so it stopped too. A shell that printed a few kilobytes
/// and exited therefore published a truncated stream, or nothing at all.
///
/// The volume at which that broke was the volume at which parsing one read
/// outlasted the window, so this is a race rather than a threshold: the sizes
/// here straddle the cliff that was observed instead of claiming to define it.
/// Equality is exact at every size, so losing one byte or repeating one fails
/// where a length floor would pass.
///
/// Both surfaces a client can read are checked, because they answer different
/// questions: the live broadcast is what an attached pane paints as the burst
/// happens, and the retained ring is what a client gets after the fact. One
/// `publish` fills both, so a stream that is whole in one and short in the
/// other would mean the publish path itself had grown a second opinion.
///
/// This does NOT catch: eviction, since the ring here is far larger than any
/// burst; backlog on attach, which is deliberately not offered; or Windows,
/// where the reader cannot reach end of stream on its own and quiet after the
/// exit is still the only end of output there is.
#[cfg(not(windows))]
#[tokio::test]
async fn a_burst_followed_by_an_immediate_exit_is_published_whole() {
    for n in [50usize, 100, 300, 1000] {
        let mgr = SessionManager::new(1 << 20);
        let id = mgr
            .spawn(shell_spec(&format!(
                "i=0; while [ $i -lt {n} ]; do printf '\\033[32mburst %s\\033[0m\\n' $i; i=$((i+1)); done"
            )))
            .expect("spawn");
        let mut live = collect(&mgr, id);

        let expected: Vec<u8> = (0..n)
            .flat_map(|i| format!("\x1b[32mburst {i}\x1b[0m\r\n").into_bytes())
            .collect();
        let want = expected.len();

        live.until(|b| b.len() >= want).await;
        assert_eq!(
            live.bytes, expected,
            "{n} writes: the live stream was not what the child wrote"
        );
        assert_eq!(wait_exit(&mgr, id).await, Some(0), "{n} writes");

        let (from, retained) = settled(&mgr, id, move |_, b| b.len() >= want).await;
        assert_eq!(from, 0, "{n} writes: a 1 MiB ring evicted something");
        assert_eq!(
            retained, expected,
            "{n} writes: the retained stream was not what the child wrote"
        );
    }
}
