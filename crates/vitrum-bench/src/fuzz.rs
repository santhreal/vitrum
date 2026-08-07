//! Protocol fuzzing against a live daemon.
//!
//! The contract being tested is narrow and absolute: **no input from a client
//! may take the daemon down or make it stop answering other clients.** A
//! malformed frame is allowed to be rejected, ignored, or answered with an
//! error. It is not allowed to panic the server, wedge the connection, or
//! affect a second connection that is behaving.
//!
//! So every case is checked the same way. Send the hostile input on connection
//! A, then ask connection B for the session list and require a correct answer
//! within the deadline. B is the oracle: if B still works, the daemon survived.
//!
//! The generator is a small xorshift rather than a crate, because a fuzz run
//! that cannot be replayed is an anecdote. The seed is in the report, and the
//! same seed produces the same cases.

use std::time::{Duration, Instant};

use serde_json::json;
use vitrum_proto::{ClientMsg, ServerMsg, SessionId};

use crate::client::Client;
use crate::report::Report;
use crate::stats::Latencies;

#[derive(Debug, Clone)]
pub struct FuzzSpec {
    pub server: String,
    pub cases: usize,
    pub seed: u64,
    /// How long the oracle connection may take to answer before the daemon is
    /// called wedged.
    pub oracle_timeout: Duration,
}

/// Deterministic generator. Xorshift64*, which is enough for choosing shapes and
/// bytes and is reproducible from the seed printed in every report.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        // A zero state is a fixed point for xorshift, so it would emit zero
        // forever and every case would be identical.
        Self(if seed == 0 { 0x9E3779B97F4A7C15 } else { seed })
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

/// Values that have historically broken JSON handlers and terminal servers.
///
/// Not random noise: random bytes almost never parse, so they only ever exercise
/// the outermost reject path. These reach the fields.
const NASTY_STRINGS: &[&str] = &[
    "",
    " ",
    "\u{0}",
    "\u{feff}",
    "\\",
    "\"",
    "\u{1b}[2J\u{1b}[H",
    "../../../../etc/passwd",
    "$(reboot)",
    "`reboot`",
    "%s%s%s%s%n",
    "\u{1f600}",
    "𝕹𝖔𝖙 𝖆𝖘𝖈𝖎𝖎",
    "line\nbreak\r\n",
];

const NASTY_NUMBERS: &[i64] = &[
    0,
    -1,
    1,
    i64::MIN,
    i64::MAX,
    4294967295,
    65535,
    65536,
    -2147483648,
];

pub async fn run(spec: &FuzzSpec) -> anyhow::Result<Report> {
    let mut report = Report::new(
        "fuzz",
        &spec.server,
        json!({
            "cases": spec.cases,
            "seed": spec.seed,
            "oracle_timeout_secs": spec.oracle_timeout.as_secs_f64(),
        }),
    );
    let started = Instant::now();
    let mut rng = Rng::new(spec.seed);

    // The oracle stays connected for the whole run and never sends anything
    // hostile, so any failure it reports is the daemon's, not its own.
    let mut oracle = Client::connect(&spec.server).await?;
    // One real session, so cases that name a live id exercise the paths that
    // touch a session rather than only the "no such session" branch.
    let (live, _) = oracle
        .create_session("fuzz-oracle", "sleep 600", 80, 24, spec.oracle_timeout)
        .await?;

    let mut oracle_latency = Latencies::new();
    let mut rejected = 0usize;
    let mut accepted = 0usize;
    let mut connection_dropped = 0usize;
    let mut interesting: Vec<serde_json::Value> = Vec::new();

    for case in 0..spec.cases {
        let payload = generate(&mut rng, live);

        // A fresh connection per case. A daemon is entitled to close a
        // connection that sent nonsense, and reusing one would report every
        // later case as a failure of the first.
        let outcome = match Client::connect(&spec.server).await {
            Ok(mut victim) => match victim.send_raw(payload.clone()).await {
                Ok(()) => match victim.next_control(Duration::from_millis(500)).await {
                    Ok(ServerMsg::Error { .. }) => {
                        rejected += 1;
                        "error"
                    }
                    Ok(_) => {
                        accepted += 1;
                        "accepted"
                    }
                    // Silence and a dropped connection are both allowed
                    // answers to garbage. Only the oracle decides failure.
                    Err(_) => {
                        connection_dropped += 1;
                        "dropped"
                    }
                },
                Err(_) => {
                    connection_dropped += 1;
                    "send-failed"
                }
            },
            Err(e) => {
                report.failures.push(format!(
                    "case {case}: the daemon stopped accepting connections: {e:#} \
                     [repro: fuzz-{case:04}-connect.bin]"
                ));
                report.artifacts.push((
                    format!("fuzz-{case:04}-connect.bin"),
                    payload.into_bytes(),
                ));
                break;
            }
        };

        // The oracle check, which is the actual assertion.
        oracle.drain_ready().await?;
        match oracle
            .round_trip(&ClientMsg::List, spec.oracle_timeout, |m| match m {
                ServerMsg::Sessions { sessions } => Some(sessions),
                _ => None,
            })
            .await
        {
            Ok((sessions, d)) => {
                oracle_latency.record(d);
                if !sessions.iter().any(|s| s.id == live) {
                    let name = format!("fuzz-{case:04}-forgot-session.bin");
                    report.failures.push(format!(
                        "case {case} ({outcome}) made the daemon forget a live \
                         session: {payload:.400} [repro: {name}]"
                    ));
                    report.artifacts.push((name, payload.as_bytes().to_vec()));
                    interesting.push(json!({ "case": case, "payload": payload }));
                }
            }
            Err(e) => {
                let name = format!("fuzz-{case:04}-oracle-wedge.bin");
                report.failures.push(format!(
                    "case {case} ({outcome}) left the daemon unable to answer: \
                     {e:#}; payload: {payload:.400} [repro: {name}]"
                ));
                interesting.push(json!({ "case": case, "payload": payload }));
                report.artifacts.push((name, payload.into_bytes()));
                // A wedged daemon fails every later case for the same reason,
                // so stop rather than filling the report with echoes.
                break;
            }
        }
    }

    let _ = oracle.close_session(live, spec.oracle_timeout).await;

    report.duration_secs = started.elapsed().as_secs_f64();
    if let Some(s) = oracle_latency.summary() {
        report.latencies.push(("oracle_list".to_string(), s));
    }
    report.extra = json!({
        "answered_with_error": rejected,
        "accepted": accepted,
        "connection_dropped_or_silent": connection_dropped,
        "reproduce_with_seed": spec.seed,
        "interesting": interesting,
    });
    if !report.failed() {
        report.checks_passed.push(format!(
            "the daemon answered a healthy connection correctly after all {} hostile inputs",
            oracle_latency.len()
        ));
    }
    Ok(report)
}

/// One hostile frame.
///
/// The mix is weighted towards inputs that parse, because those reach the
/// handlers. Pure garbage is included but is the cheap case.
fn generate(rng: &mut Rng, live: SessionId) -> String {
    match rng.below(10) {
        // Structurally invalid JSON.
        0 => {
            let bad = [
                "{", "}", "[]", "null", "\"\"", "{\"t\":}", "{\"t\": \"hello\"",
                "{\"t\":\"hello\",\"protocol\":}",
            ];
            (*rng.pick(&bad)).to_string()
        }
        // Valid JSON, unknown or missing tag.
        1 => {
            let t = rng.pick(NASTY_STRINGS);
            json!({ "t": t, "protocol": 2 }).to_string()
        }
        // A known tag with the wrong field types.
        2 => json!({
            "t": "createSession",
            "projectId": rng.pick(NASTY_STRINGS),
            "cwd": rng.next(),
            "command": rng.pick(NASTY_NUMBERS),
            "args": "not-an-array",
            "cols": -1,
            "rows": rng.pick(NASTY_NUMBERS),
            "title": [1, 2, 3],
        })
        .to_string(),
        // Out-of-range geometry against a real session. Zero and u16::MAX are
        // the sizes that produce a zero-area or enormous grid allocation.
        3 => json!({
            "t": "resize",
            "session": live.0,
            "cols": rng.pick(NASTY_NUMBERS),
            "rows": rng.pick(NASTY_NUMBERS),
        })
        .to_string(),
        // Operations on a session that does not exist.
        4 => {
            let id = rng.next();
            let t = *rng.pick(&["attach", "detach", "close", "input", "rename"]);
            json!({
                "t": t,
                "session": id,
                "cols": 80,
                "rows": 24,
                "data": [0, 27, 91, 50, 74, 255],
                "title": rng.pick(NASTY_STRINGS),
            })
            .to_string()
        }
        // A scrollback request with an absurd budget, which is the one message
        // whose parameters directly size a server-side allocation.
        5 => json!({
            "t": "scrollback",
            "session": live.0,
            "beforeSeq": rng.next(),
            "maxBytes": u32::MAX,
        })
        .to_string(),
        // Search, which compiles an operator-supplied regular expression.
        6 => {
            let patterns = [
                "(",
                "(((((((((((((((((((((",
                "a{1000000}",
                "(a+)+$",
                "[",
                "\\",
                ".*.*.*.*.*.*.*.*.*b",
            ];
            json!({
                "t": "search",
                "sessions": [],
                "pattern": rng.pick(&patterns),
                "regex": true,
                "caseInsensitive": true,
                "wholeWord": rng.next().is_multiple_of(2),
                "contextLines": u16::MAX,
                "maxHits": u32::MAX,
            })
            .to_string()
        }
        // Input carrying bytes no terminal should choke on but many do.
        7 => {
            let mut data = Vec::with_capacity(64);
            for _ in 0..rng.below(64) + 1 {
                data.push((rng.next() % 256) as u8);
            }
            json!({ "t": "input", "session": live.0, "data": data }).to_string()
        }
        // A protocol number that is not ours, including versions that do not
        // exist. Rejection is correct; silence is not, and neither is a crash.
        8 => json!({ "t": "hello", "protocol": rng.pick(NASTY_NUMBERS) }).to_string(),
        // Deep nesting, which is where recursive descent parsers overflow.
        _ => {
            let depth = 1 + rng.below(2048);
            let mut s = String::with_capacity(depth * 2 + 16);
            for _ in 0..depth {
                s.push('[');
            }
            for _ in 0..depth {
                s.push(']');
            }
            s
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The seed is the whole reproducibility story, so it has to hold.
    #[test]
    fn the_same_seed_generates_the_same_cases() {
        let cases = |seed| {
            let mut r = Rng::new(seed);
            (0..500)
                .map(|_| generate(&mut r, SessionId(7)))
                .collect::<Vec<_>>()
        };
        assert_eq!(cases(12345), cases(12345));
        assert_ne!(cases(12345), cases(12346));
    }

    /// A generator that emits one shape tests one code path and reports a clean
    /// run for the other nine.
    #[test]
    fn the_generator_covers_every_shape() {
        let mut r = Rng::new(1);
        let cases: Vec<String> = (0..2000).map(|_| generate(&mut r, SessionId(7))).collect();
        for marker in [
            "createSession",
            "resize",
            "scrollback",
            "search",
            "input",
            "hello",
            "[[[",
        ] {
            assert!(
                cases.iter().any(|c| c.contains(marker)),
                "no generated case contained {marker}"
            );
        }
        // Structurally invalid JSON has to appear too, and it is identified by
        // failing to parse rather than by a marker.
        assert!(
            cases
                .iter()
                .any(|c| serde_json::from_str::<serde_json::Value>(c).is_err()),
            "the generator never emitted invalid JSON"
        );
    }

    /// A zero seed must not collapse the generator to a constant.
    #[test]
    fn a_zero_seed_still_varies() {
        let mut r = Rng::new(0);
        let first = generate(&mut r, SessionId(1));
        assert!(
            (0..64).any(|_| generate(&mut r, SessionId(1)) != first),
            "a zero seed produced one repeated case"
        );
    }
}
