//! TEMPORARY measurement harness. Delete before shipping.
//!
//! Runs a daemon on an ephemeral loopback port, spawns one PTY session, and
//! times three things the operator can feel:
//!
//!   echo  - one keystroke written, until the byte the line discipline echoes
//!           back arrives as a data frame at the client.
//!   first - an agent writes a line after silence; time from the write landing
//!           in the pty until the first painted byte reaches the client.
//!   bulk  - a full-screen TUI redraw: bytes/s and frames for a fixed volume.
//!
//! Usage: cargo run -p vitrum-server --release --example echolat

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use vitrum_core::SessionManager;
use vitrum_proto::{ClientMsg, PROTOCOL_VERSION, ProjectId, ServerMsg, SessionId, decode_output};

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn pct(v: &mut Vec<f64>, p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let i = ((v.len() as f64 - 1.0) * p).round() as usize;
    v[i]
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let port = listener.local_addr()?.port();
    let manager = Arc::new(SessionManager::new(10 * 1024 * 1024));
    tokio::spawn(vitrum_server::serve(listener, Arc::clone(&manager), TOKEN.into()));

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}")).await?;
    let say = |m: &ClientMsg| Message::Text(serde_json::to_string(m).unwrap());
    ws.send(say(&ClientMsg::Hello {
        protocol: PROTOCOL_VERSION,
        token: TOKEN.into(),
    }))
    .await?;

    // A shell with echo on is the line-discipline echo path a keystroke takes.
    ws.send(say(&ClientMsg::CreateSession {
        project_id: ProjectId(0),
        cwd: std::env::var("HOME").unwrap_or_else(|_| "/".into()),
        command: "/bin/sh".into(),
        args: vec![],
        cols: 200,
        rows: 50,
        title: None,
    }))
    .await?;

    let mut session = None;
    while session.is_none() {
        if let Some(Ok(Message::Text(t))) = ws.next().await {
            if let Ok(ServerMsg::SessionCreated(info)) = serde_json::from_str(&t) {
                session = Some(info.id);
            }
        }
    }
    let session: SessionId = session.unwrap();
    ws.send(say(&ClientMsg::Attach {
        session,
        cols: 200,
        rows: 50,
    }))
    .await?;

    // Let the shell finish printing its prompt.
    let settle = Instant::now();
    while settle.elapsed() < Duration::from_millis(1500) {
        let _ = tokio::time::timeout(Duration::from_millis(100), ws.next()).await;
    }

    // ---- echo: one keystroke, byte back ----
    let mut echo = Vec::new();
    for _ in 0..300 {
        let t0 = Instant::now();
        ws.send(say(&ClientMsg::Input {
            session,
            data: vec![b'x'],
        }))
        .await?;
        loop {
            match tokio::time::timeout(Duration::from_secs(5), ws.next()).await {
                Ok(Some(Ok(Message::Binary(b)))) => {
                    let (_, _, payload) = decode_output(&b).unwrap();
                    if payload.contains(&b'x') {
                        echo.push(t0.elapsed().as_secs_f64() * 1e3);
                        break;
                    }
                }
                Ok(Some(Ok(_))) => {}
                _ => anyhow::bail!("no echo"),
            }
        }
        // Erase what was typed, and let the run go quiet again.
        ws.send(say(&ClientMsg::Input {
            session,
            data: vec![0x7f],
        }))
        .await?;
        tokio::time::sleep(Duration::from_millis(6)).await;
        while let Ok(Some(Ok(_))) = tokio::time::timeout(Duration::from_millis(2), ws.next()).await
        {
        }
    }

    // ---- first painted byte after silence ----
    ws.send(say(&ClientMsg::Input {
        session,
        data: b"\x15\n".to_vec(),
    }))
    .await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    while let Ok(Some(Ok(_))) = tokio::time::timeout(Duration::from_millis(20), ws.next()).await {}

    let mut first = Vec::new();
    for i in 0..100 {
        let token = format!("mark{i}");
        let cmd = format!("printf '{token}\\n'\n");
        // Type the command, wait for it to be quiet, then press Enter and time
        // from Enter to the first frame carrying the token.
        ws.send(say(&ClientMsg::Input {
            session,
            data: cmd.as_bytes()[..cmd.len() - 1].to_vec(),
        }))
        .await?;
        tokio::time::sleep(Duration::from_millis(30)).await;
        while let Ok(Some(Ok(_))) = tokio::time::timeout(Duration::from_millis(5), ws.next()).await
        {
        }
        let t0 = Instant::now();
        ws.send(say(&ClientMsg::Input {
            session,
            data: b"\n".to_vec(),
        }))
        .await?;
        let mut acc = Vec::new();
        loop {
            match tokio::time::timeout(Duration::from_secs(5), ws.next()).await {
                Ok(Some(Ok(Message::Binary(b)))) => {
                    let (_, _, payload) = decode_output(&b).unwrap();
                    acc.extend_from_slice(payload);
                    if acc.windows(token.len()).any(|w| w == token.as_bytes()) {
                        first.push(t0.elapsed().as_secs_f64() * 1e3);
                        break;
                    }
                }
                Ok(Some(Ok(_))) => {}
                _ => anyhow::bail!("no output"),
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        while let Ok(Some(Ok(_))) = tokio::time::timeout(Duration::from_millis(5), ws.next()).await
        {
        }
    }

    // ---- bulk: a full-screen redraw, repeated ----
    const BULK: usize = 64 * 1024 * 1024;
    let cmd = format!("yes 0123456789abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmn | head -c {BULK}\n");
    let t0 = Instant::now();
    ws.send(say(&ClientMsg::Input {
        session,
        data: cmd.into_bytes(),
    }))
    .await?;
    let mut got = 0usize;
    let mut frames = 0usize;
    let mut firstbyte = None;
    while got < BULK {
        match tokio::time::timeout(Duration::from_secs(20), ws.next()).await {
            Ok(Some(Ok(Message::Binary(b)))) => {
                let (_, _, payload) = decode_output(&b).unwrap();
                if firstbyte.is_none() {
                    firstbyte = Some(t0.elapsed());
                }
                got += payload.len();
                frames += 1;
            }
            Ok(Some(Ok(_))) => {}
            _ => break,
        }
    }
    let bulk = t0.elapsed();

    let counts = manager.pump_counts(session);
    println!("--- echo (keystroke -> echoed byte at client), n={}", echo.len());
    println!(
        "p50 {:.3} ms  p90 {:.3} ms  p99 {:.3} ms  max {:.3} ms",
        pct(&mut echo, 0.50),
        pct(&mut echo, 0.90),
        pct(&mut echo, 0.99),
        pct(&mut echo, 1.0)
    );
    println!("--- first painted byte after silence, n={}", first.len());
    println!(
        "p50 {:.3} ms  p90 {:.3} ms  p99 {:.3} ms  max {:.3} ms",
        pct(&mut first, 0.50),
        pct(&mut first, 0.90),
        pct(&mut first, 0.99),
        pct(&mut first, 1.0)
    );
    println!("--- bulk redraw");
    println!(
        "{:.1} MB in {:.3} s = {:.1} MB/s, {frames} frames, {:.1} KiB/frame, first byte {:.3} ms",
        got as f64 / 1e6,
        bulk.as_secs_f64(),
        got as f64 / 1e6 / bulk.as_secs_f64(),
        got as f64 / frames as f64 / 1024.0,
        firstbyte.unwrap_or_default().as_secs_f64() * 1e3
    );
    println!("pump: {counts:?}");
    Ok(())
}
