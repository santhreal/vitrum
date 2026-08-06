//! Concurrency: many connections doing conflicting things to shared state.
//!
//! The load workload asks what the daemon costs. This one asks whether it stays
//! correct while several windows fight over the same sessions, which is the
//! ordinary case: an operator with three windows open, an editor plugin, and a
//! session list that all of them mutate.
//!
//! Every invariant here is one a single-connection run cannot break:
//!
//! - a session created on one connection is visible to every other
//! - concurrent renames converge, and every connection ends on the same title
//! - a close is seen by every attached connection, exactly once
//! - no connection is left holding a session the registry has forgotten

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use anyhow::bail;
use serde_json::json;
use vitrum_proto::{ClientMsg, SessionId};

use crate::client::Client;
use crate::report::Report;
use crate::stats::Latencies;

#[derive(Debug, Clone)]
pub struct RaceSpec {
    pub server: String,
    /// Connections acting at once. Each is a separate socket, as a real second
    /// window would be.
    pub connections: usize,
    /// Sessions every connection creates.
    pub sessions_per_conn: usize,
    /// Rename attempts each connection makes against every session it can see.
    pub renames: usize,
    /// How long to wait for the broadcast state to settle before reading it.
    pub settle: Duration,
}

/// How long a connection must receive nothing before it counts as caught up.
///
/// Long enough that a scheduling hiccup between two broadcasts does not read as
/// quiet, short enough that a converged run does not pay seconds for the proof.
const QUIET: Duration = Duration::from_millis(300);

const OP_TIMEOUT: Duration = Duration::from_secs(30);

pub async fn run(spec: &RaceSpec) -> anyhow::Result<Report> {
    if spec.connections < 2 {
        bail!("a concurrency run needs at least two connections; one cannot race");
    }
    let mut report = Report::new(
        "race",
        &spec.server,
        json!({
            "connections": spec.connections,
            "sessions_per_conn": spec.sessions_per_conn,
            "renames": spec.renames,
            "settle_secs": spec.settle.as_secs_f64(),
        }),
    );
    let started = Instant::now();

    let mut conns = Vec::with_capacity(spec.connections);
    for _ in 0..spec.connections {
        conns.push(Client::connect(&spec.server).await?);
    }

    // Phase one: every connection creates its own sessions at the same time.
    // Created concurrently on purpose: serialising them would test a queue, not
    // a race.
    let mut create = Latencies::new();
    let mut mine: Vec<Vec<SessionId>> = Vec::with_capacity(conns.len());
    let creates = futures_util::future::join_all(conns.iter_mut().enumerate().map(|(i, c)| {
        let n = spec.sessions_per_conn;
        async move {
            let mut ids = Vec::with_capacity(n);
            let mut ds = Vec::with_capacity(n);
            for k in 0..n {
                let (id, d) = c
                    .create_session(
                        &format!("race-{i}-{k}"),
                        "/tmp",
                        "/bin/sh",
                        &["-c".to_string(), format!("printf 'conn{i}-{k}\\n'")],
                        80,
                        24,
                        OP_TIMEOUT,
                    )
                    .await?;
                ids.push(id);
                ds.push(d);
            }
            Ok::<_, anyhow::Error>((ids, ds))
        }
    }))
    .await;
    for r in creates {
        let (ids, ds) = r?;
        for d in ds {
            create.record(d);
        }
        mine.push(ids);
    }
    let all: HashSet<SessionId> = mine.iter().flatten().copied().collect();
    if all.len() != spec.connections * spec.sessions_per_conn {
        report.failures.push(format!(
            "session ids collided: {} distinct ids for {} creates",
            all.len(),
            spec.connections * spec.sessions_per_conn
        ));
    } else {
        report
            .checks_passed
            .push(format!("{} concurrent creates got distinct ids", all.len()));
    }

    // Phase two: every connection renames every session, including the ones it
    // did not create. Last write wins is fine; disagreement is not.
    let ordered: Vec<SessionId> = {
        let mut v: Vec<SessionId> = all.iter().copied().collect();
        v.sort_by_key(|s| s.0);
        v
    };
    let mut rename = Latencies::new();
    let renames = futures_util::future::join_all(conns.iter_mut().enumerate().map(|(i, c)| {
        let ids = ordered.clone();
        let rounds = spec.renames;
        async move {
            let mut ds = Vec::new();
            for r in 0..rounds {
                for id in &ids {
                    let start = Instant::now();
                    c.send(&ClientMsg::Rename {
                        session: *id,
                        title: format!("c{i}-r{r}"),
                    })
                    .await?;
                    ds.push(start.elapsed());
                }
            }
            Ok::<_, anyhow::Error>(ds)
        }
    }))
    .await;
    for r in renames {
        for d in r? {
            rename.record(d);
        }
    }

    // Phase three: after the storm stops, every connection's view must be the
    // same. This is the invariant a broadcast bug breaks and a single client can
    // never notice.
    //
    // Convergence is waited for, not slept through: every rename is published to
    // every connection, so the traffic scales with connections times renames and
    // a connection that has not caught up would report a stale title.
    let mut titles: Vec<HashMap<SessionId, String>> = Vec::with_capacity(conns.len());
    let mut list = Latencies::new();
    for (i, c) in conns.iter_mut().enumerate() {
        let read = c.list_now(QUIET, spec.settle).await?;
        if !read.converged {
            report.failures.push(format!(
                "connection {i} was still receiving after {:?}: {} control frames and \
                 {} list snapshots arrived without a {QUIET:?} pause, so its view never settled",
                spec.settle, read.control_frames, read.snapshots
            ));
        }
        list.record(read.elapsed);
        let Some(sessions) = read.sessions else {
            report
                .failures
                .push(format!("connection {i} was never sent a session list"));
            continue;
        };
        titles.push(sessions.into_iter().map(|s| (s.id, s.title)).collect());
    }
    let mut disagreements = Vec::new();
    for id in &ordered {
        let seen: HashSet<Option<&String>> = titles.iter().map(|t| t.get(id)).collect();
        if seen.len() > 1 {
            disagreements.push(format!(
                "session {} has {} different views across connections",
                id.0,
                seen.len()
            ));
        }
    }
    if disagreements.is_empty() {
        report.checks_passed.push(format!(
            "every connection agrees on all {} sessions after {} concurrent renames",
            ordered.len(),
            spec.renames * spec.connections * ordered.len()
        ));
    } else {
        report.failures.extend(disagreements);
    }

    // Phase four: one connection closes everything, and no other connection may
    // still believe a closed session exists.
    //
    // The check is on the resulting view, not on the delta. A connection that
    // lags the broadcast bus is deliberately sent a full snapshot instead of the
    // deltas it missed, so counting `SessionRemoved` messages would fail a
    // client the daemon repaired correctly. What must hold either way is that
    // the session is gone from its picture.
    let (closer, watchers) = conns.split_at_mut(1);
    let closer = &mut closer[0];
    let mut close = Latencies::new();
    for id in &ordered {
        if let Ok(d) = closer.close_session(*id, OP_TIMEOUT).await {
            close.record(d);
        }
    }
    for (i, w) in watchers.iter_mut().enumerate() {
        let read = w.list_now(QUIET, spec.settle).await?;
        if !read.converged {
            report.failures.push(format!(
                "connection {} was still receiving {:?} after the last close",
                i + 1,
                spec.settle
            ));
        }
        let Some(sessions) = read.sessions else {
            report.failures.push(format!(
                "connection {} was never sent a session list after the closes",
                i + 1
            ));
            continue;
        };
        let stale: Vec<u64> = sessions
            .iter()
            .filter(|s| ordered.contains(&s.id))
            .map(|s| s.id.0)
            .collect();
        if !stale.is_empty() {
            report.failures.push(format!(
                "connection {} still lists closed sessions {stale:?}",
                i + 1
            ));
        }
    }
    if !report.failed() {
        report.checks_passed.push(format!(
            "all {} watching connections dropped every one of {} closed sessions",
            watchers.len(),
            ordered.len()
        ));
    }

    report.duration_secs = started.elapsed().as_secs_f64();
    for (name, l) in [
        ("create_session", &create),
        ("rename_send", &rename),
        ("list_after_settle", &list),
        ("close_session", &close),
    ] {
        if let Some(s) = l.summary() {
            report.latencies.push((name.to_string(), s));
        }
    }
    Ok(report)
}
