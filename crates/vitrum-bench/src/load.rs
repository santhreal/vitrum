//! Sustained load: many sessions, each producing output, all at once.
//!
//! The shape is the one the product is for. Twenty or a hundred agents each
//! streaming into their own session, a client attached to all of them, and the
//! question is what the daemon costs while that happens and whether any single
//! request stalls behind the firehose.
//!
//! Each session runs a command that writes a known number of bytes, so the
//! run knows what it should have received and can say that it did rather than
//! reporting whatever arrived.

use std::time::{Duration, Instant};

use anyhow::bail;
use serde_json::json;
use vitrum_proto::{ClientMsg, ServerMsg, SessionId};

use crate::client::Client;
use crate::report::Report;
use crate::stats::{Latencies, Throughput};

/// What to run.
#[derive(Debug, Clone)]
pub struct LoadSpec {
    pub server: String,
    pub sessions: usize,
    /// Lines each session writes. Every line is [`LINE_BYTES`] wide.
    pub lines: usize,
    /// How long to wait for the last byte after the last session was created.
    pub drain: Duration,
    pub cols: u16,
    pub rows: u16,
}

/// Bytes each generated line occupies on the wire.
///
/// 63 printed characters plus CR LF. The pty is in its default cooked mode with
/// ONLCR set, so the single newline the generator writes reaches the client as
/// two bytes. Counting 64 here would understate every expected total by one
/// byte per line and turn the delivery check into an inequality that can never
/// fail, which is the same as not checking.
pub const LINE_BYTES: usize = 65;

const OP_TIMEOUT: Duration = Duration::from_secs(30);

/// Run the load and return its report. A delivery shortfall is a failure
/// recorded in the report, not an error: the numbers around it are still worth
/// having, and a run that returns nothing teaches nothing.
pub async fn run(spec: &LoadSpec) -> anyhow::Result<Report> {
    let mut report = Report::new(
        "load",
        &spec.server,
        json!({
            "sessions": spec.sessions,
            "lines_per_session": spec.lines,
            "line_bytes": LINE_BYTES,
            "drain_secs": spec.drain.as_secs_f64(),
            "cols": spec.cols,
            "rows": spec.rows,
        }),
    );
    if spec.sessions == 0 {
        bail!("a load run needs at least one session");
    }

    let started = Instant::now();
    let mut client = Client::connect(&spec.server).await?;

    let mut create = Latencies::new();
    let mut attach = Latencies::new();
    let mut ids: Vec<SessionId> = Vec::with_capacity(spec.sessions);
    // The throughput window opens before the first session exists. A session
    // starts streaming the moment it is created, so plenty of output arrives
    // while later sessions are still being set up. Timing only the drain loop
    // would divide every byte by whatever was left over after that, which on a
    // fast host is a millisecond and reports a rate nothing achieved.
    let stream_start = Instant::now();

    for n in 0..spec.sessions {
        let (id, d) = client
            .create_session(
                &format!("load-{n}"),
                "/tmp",
                "/bin/sh",
                &["-c".to_string(), generator(spec.lines)],
                spec.cols,
                spec.rows,
                OP_TIMEOUT,
            )
            .await?;
        create.record(d);
        ids.push(id);
        client.drain_ready().await?;

        let (_, d) = client
            .round_trip(
                &ClientMsg::Attach {
                    session: id,
                    cols: spec.cols,
                    rows: spec.rows,
                },
                OP_TIMEOUT,
                |m| match m {
                    // The daemon acknowledges an attach by updating the session
                    // it now has a viewer on.
                    ServerMsg::SessionUpdated(info) if info.id == id => Ok(()),
                    other => Err(other),
                },
            )
            .await?;
        attach.record(d);
    }

    // Every session writes the same amount, and LINE_BYTES already accounts for
    // the pty's newline translation.
    let expected = (spec.lines * LINE_BYTES * spec.sessions) as u64;
    let deadline = Instant::now() + spec.drain;

    // Exits are counted by the client as frames arrive, so any that landed
    // during session setup are already recorded and this loop only waits for
    // the rest.
    while client.exits.len() < ids.len() && Instant::now() < deadline {
        let left = deadline.saturating_duration_since(Instant::now());
        if client.next(left).await?.is_none() {
            break;
        }
    }
    let exited = ids.iter().filter(|id| client.exits.contains_key(id)).count();
    let streamed = stream_start.elapsed();

    // A request issued after the firehose, answered while it is still draining:
    // this is the number that says whether one slow session blocks the rest.
    let (_, list_after) = client
        .round_trip(&ClientMsg::List, OP_TIMEOUT, |m| match m {
            ServerMsg::Sessions { .. } => Ok(()),
            other => Err(other),
        })
        .await?;
    let mut list = Latencies::new();
    list.record(list_after);

    let mut close = Latencies::new();
    for id in &ids {
        // A session that already exited is still closed, which is what removes
        // it from the registry.
        if let Ok(d) = client.close_session(*id, OP_TIMEOUT).await {
            close.record(d);
        }
    }

    report.duration_secs = started.elapsed().as_secs_f64();
    report.throughput = Some(Throughput::new(client.bytes_in, client.frames_in, streamed));
    for (name, l) in [
        ("create_session", &create),
        ("attach", &attach),
        ("list_under_load", &list),
        ("close_session", &close),
    ] {
        if let Some(s) = l.summary() {
            report.latencies.push((name.to_string(), s));
        }
    }
    // Per-session accounting, from the byte offsets the protocol carries. The
    // expected total is per session, so a shortfall is attributable rather than
    // a single number that says only "something is missing".
    let per_session = spec.lines as u64 * LINE_BYTES as u64;
    let mut lost_in_gaps = 0u64;
    let mut skipped_before_attach = 0u64;
    let mut short_tails = Vec::new();
    for id in &ids {
        let Some(s) = client.streams.get(id) else {
            report
                .failures
                .push(format!("session {} produced no output at all", id.0));
            continue;
        };
        lost_in_gaps += s.gap_bytes;
        skipped_before_attach += s.first_seq;
        // `next_seq` is where the stream ended. Anything short of the generated
        // total that is not a counted gap was truncated at the end, which a gap
        // count alone would miss.
        if s.next_seq < per_session {
            short_tails.push(json!({
                "session": id.0,
                "ended_at": s.next_seq,
                "expected": per_session,
            }));
        }
    }

    report.extra = json!({
        "expected_bytes": expected,
        "received_bytes": client.bytes_in,
        "sessions_exited": exited,
        "bytes_skipped_before_attach": skipped_before_attach,
        "bytes_lost_in_gaps": lost_in_gaps,
        "streams_ending_short": short_tails,
    });

    if exited == ids.len() {
        report
            .checks_passed
            .push(format!("all {} sessions reached exit", ids.len()));
    } else {
        report.failures.push(format!(
            "{} of {} sessions never reported exit within {:?}",
            ids.len() - exited,
            ids.len(),
            spec.drain
        ));
    }

    // A gap is loss the protocol itself proves: a frame arrived at a higher
    // offset than the previous frame ended. Nothing else can explain it.
    if lost_in_gaps == 0 {
        report.checks_passed.push(format!(
            "no session lost a byte mid-stream, across {} bytes delivered",
            client.bytes_in
        ));
    } else {
        report.failures.push(format!(
            "{lost_in_gaps} bytes lost mid-stream across {} sessions",
            client.streams.values().filter(|s| s.gaps > 0).count()
        ));
    }
    if short_tails.is_empty() {
        report.checks_passed.push(format!(
            "every session's stream reached the full {per_session} bytes it generated"
        ));
    } else {
        report.failures.push(format!(
            "{} sessions stopped short of the {per_session} bytes they generated",
            short_tails.len()
        ));
    }
    // The ledger has to close: everything generated was either delivered to this
    // client, produced before it attached, or lost. Checking the identity rather
    // than each term separately is what makes an unexplained byte impossible to
    // report as a pass.
    let accounted = client.bytes_in + skipped_before_attach + lost_in_gaps;
    if accounted == expected {
        report.checks_passed.push(format!(
            "every one of the {expected} generated bytes is accounted for: \
             {} delivered, {skipped_before_attach} produced before this client attached, \
             {lost_in_gaps} lost",
            client.bytes_in
        ));
    } else {
        report.failures.push(format!(
            "the byte ledger does not close: {accounted} accounted for against {expected} \
             generated ({} delivered, {skipped_before_attach} pre-attach, {lost_in_gaps} lost)",
            client.bytes_in
        ));
    }
    Ok(report)
}

/// A shell command emitting `lines` lines of exactly [`LINE_BYTES`] bytes.
///
/// `seq` and `printf` rather than `yes` or a loop of `echo`: the count has to be
/// exact for the delivery check, and the width has to be fixed for the total to
/// be arithmetic.
pub fn generator(lines: usize) -> String {
    // 63 payload characters plus the newline printf writes.
    format!("i=1; while [ $i -le {lines} ]; do printf '%063d\\n' $i; i=$((i+1)); done")
}
