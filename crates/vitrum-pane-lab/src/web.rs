//! The baseline: the pane as it ships today, isolated so it can be timed.
//!
//! A real `WebKitWebView` in a real GTK 3 toplevel, running the byte-identical
//! vendored `xterm.js` and WebGL addon the app inlines, fed from a real PTY
//! over a binary WebSocket — the same transport shape `bootstrap.js` reads,
//! down to writing the payload straight into `term.write` with no decode on
//! the way.
//!
//! It is a reproduction of the pane rather than the whole client because the
//! shipping client cannot be instrumented without editing `app/src`, which
//! this pass may not touch. What is reproduced is everything the pane's frame
//! ceiling could plausibly depend on: the same WebKitGTK, the same JavaScript
//! engine, the same parser, the same renderer choice, the same window system.
//! What is left out is the Dioxus virtual DOM and the sidebar, which add cost
//! to the shipping path rather than removing it, so this baseline is a
//! best case for xterm.js and not a strawman.

use std::cell::RefCell;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::rc::Rc;
use std::sync::mpsc;

use anyhow::{Context, Result, anyhow};
use gtk::prelude::*;
use webkit2gtk::WebViewExt;

use crate::pty::Pty;

/// The vendored bundle the app inlines. Same bytes, same version, no CDN.
const XTERM_JS: &str = include_str!("../../../app/src/vendor/xterm.js");
const XTERM_CSS: &str = include_str!("../../../app/src/vendor/xterm.css");
const ADDON_WEBGL_JS: &str = include_str!("../../../app/src/vendor/addon-webgl.js");

struct Args {
    cols: u16,
    rows: u16,
    seconds: u64,
    webgl: bool,
    stats: Option<String>,
    argv: Vec<String>,
}

fn parse(args: &[String]) -> Result<Args> {
    let mut out = Args {
        cols: 100,
        rows: 30,
        seconds: 20,
        webgl: false,
        stats: None,
        argv: Vec::new(),
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--cols" => {
                out.cols = args[i + 1].parse()?;
                i += 2;
            }
            "--rows" => {
                out.rows = args[i + 1].parse()?;
                i += 2;
            }
            "--seconds" => {
                out.seconds = args[i + 1].parse()?;
                i += 2;
            }
            "--stats" => {
                out.stats = Some(args[i + 1].clone());
                i += 2;
            }
            "--webgl" => {
                out.webgl = true;
                i += 1;
            }
            "--" => {
                out.argv = args[i + 1..].to_vec();
                i = args.len();
            }
            other => return Err(anyhow!("unknown flag {other}")),
        }
    }
    if out.argv.is_empty() {
        out.argv = vec!["/usr/bin/python3".into(), "-q".into()];
    }
    Ok(out)
}

/// Run the xterm.js baseline.
pub fn run(args: &[String]) -> Result<()> {
    let args = parse(args)?;
    gtk::init().context("gtk_init: is DISPLAY set?")?;

    // Bound before the page loads so the port can be baked into the script.
    let listener = TcpListener::bind("127.0.0.1:0").context("bind ws listener")?;
    let port = listener.local_addr()?.port();

    let cols = args.cols;
    let rows = args.rows;
    let argv = args.argv.clone();
    let seconds = args.seconds;
    let (started_tx, started_rx) = mpsc::channel::<()>();
    let feeder = std::thread::spawn(move || {
        if let Err(err) = feed(listener, &argv, cols, rows, seconds, &started_tx) {
            eprintln!("feeder: {err}");
        }
    });

    let webview = webkit2gtk::WebView::new();
    if let Some(settings) = WebViewExt::settings(&webview) {
        // The app runs the vendored bundle on default settings; the only
        // thing forced here is what the WebGL comparison needs.
        webkit2gtk::SettingsExt::set_enable_webgl(&settings, true);
    }

    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("vitrum pane lab (xterm.js in WebKitGTK)");
    window.set_default_size(1280, 760);
    window.add(&webview);

    // The report comes back through `document.title`.
    //
    // A script message handler would be the tidy channel, but it lives behind
    // a `webkit2gtk` feature flag, and this workspace shares one lockfile with
    // the shipping app: turning that feature on here turns it on for tao's
    // copy too. A lab crate does not get to change what the product links.
    let report: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    {
        let report = Rc::clone(&report);
        webview.connect_title_notify(move |wv| {
            let Some(title) = wv.title() else { return };
            let Some(json) = title.strip_prefix("REPORT:") else {
                return;
            };
            *report.borrow_mut() = Some(json.to_string());
            gtk::main_quit();
        });
    }

    webview.load_html(&page(port, cols, rows, args.webgl), None);
    window.show_all();
    window.connect_delete_event(|_, _| {
        gtk::main_quit();
        glib::Propagation::Proceed
    });

    // A hard stop, so a page that never reports cannot hang the harness.
    glib::timeout_add_seconds_local((seconds + 25) as u32, || {
        eprintln!("web baseline: page never reported, giving up");
        gtk::main_quit();
        glib::ControlFlow::Break
    });

    gtk::main();
    let _ = started_rx.try_recv();
    let _ = feeder.join();

    let text = report
        .borrow()
        .clone()
        .ok_or_else(|| anyhow!("no report came back from the page"))?;
    let mut value: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|_| serde_json::json!({ "raw": text }));
    value["label"] = serde_json::json!(if args.webgl {
        "webkitgtk-xterm.js-webgl"
    } else {
        "webkitgtk-xterm.js-dom"
    });
    value["cols"] = serde_json::json!(cols);
    value["rows"] = serde_json::json!(rows);
    let out = serde_json::to_string_pretty(&value)?;
    if let Some(path) = &args.stats {
        std::fs::write(path, &out)?;
    }
    println!("{out}");
    Ok(())
}

/// Accept one WebSocket client and stream the PTY into it as binary frames.
///
/// A hand-written server rather than a dependency: the protocol needed here is
/// one accept, one handshake, and unmasked binary frames out. Pulling an async
/// runtime and a WebSocket crate into a throwaway lab to send frames in one
/// direction would be more code, not less.
fn feed(
    listener: TcpListener,
    argv: &[String],
    cols: u16,
    rows: u16,
    seconds: u64,
    started: &mpsc::Sender<()>,
) -> Result<()> {
    let (mut sock, _) = listener.accept().context("accept ws client")?;
    handshake(&mut sock)?;
    let _ = started.send(());

    let mut pty = Pty::spawn(argv, cols, rows, (8, 17))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    let mut buf = Vec::with_capacity(1 << 16);
    loop {
        if std::time::Instant::now() >= deadline {
            break;
        }
        buf.clear();
        // Block in `poll` rather than sleeping between drains. The idle
        // measurement samples this whole process tree, and a feeder thread
        // waking a thousand times a second would show up as the web path
        // burning CPU that xterm.js never asked for.
        let mut pfd = libc::pollfd {
            fd: pty.fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one live pollfd naming an fd the `Pty` still owns.
        unsafe { libc::poll(&mut pfd, 1, 200) };
        let open = crate::pty::drain(pty.fd, &mut buf)?;
        if !buf.is_empty() {
            send_binary(&mut sock, &buf)?;
        }
        if !open {
            break;
        }
    }
    send_text(&mut sock, "done")?;
    pty.kill();
    Ok(())
}

fn handshake(sock: &mut std::net::TcpStream) -> Result<()> {
    let mut req = Vec::new();
    let mut byte = [0u8; 1];
    while !req.ends_with(b"\r\n\r\n") {
        if sock.read(&mut byte)? == 0 {
            return Err(anyhow!("client closed during handshake"));
        }
        req.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&req);
    let key = text
        .lines()
        .find_map(|l| l.strip_prefix("Sec-WebSocket-Key: "))
        .ok_or_else(|| anyhow!("no Sec-WebSocket-Key"))?
        .trim();
    let accept = ws_accept(key);
    let resp = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    sock.write_all(resp.as_bytes())?;
    Ok(())
}

/// RFC 6455 accept value: SHA-1 of key + GUID, base64.
fn ws_accept(key: &str) -> String {
    let mut input = key.as_bytes().to_vec();
    input.extend_from_slice(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    base64(&sha1(&input))
}

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476, 0xc3d2_e1f0];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for (i, c) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5a82_7999),
                20..=39 => (b ^ c ^ d, 0x6ed9_eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
                _ => (b ^ c ^ d, 0xca62_c1d6),
            };
            let tmp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

fn base64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(A[(n >> 18) as usize & 63] as char);
        out.push(A[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            A[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            A[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn frame_header(opcode: u8, len: usize) -> Vec<u8> {
    let mut h = vec![0x80 | opcode];
    if len < 126 {
        h.push(len as u8);
    } else if len <= u16::MAX as usize {
        h.push(126);
        h.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        h.push(127);
        h.extend_from_slice(&(len as u64).to_be_bytes());
    }
    h
}

fn send_binary(sock: &mut std::net::TcpStream, payload: &[u8]) -> Result<()> {
    sock.write_all(&frame_header(0x2, payload.len()))?;
    sock.write_all(payload)?;
    Ok(())
}

fn send_text(sock: &mut std::net::TcpStream, text: &str) -> Result<()> {
    sock.write_all(&frame_header(0x1, text.len()))?;
    sock.write_all(text.as_bytes())?;
    Ok(())
}

/// The page under test.
///
/// The write path mirrors `bootstrap.js`: a binary frame arrives, its bytes go
/// into `term.write` as a `Uint8Array` with no decode. The only addition is the
/// completion callback, which xterm invokes once the chunk has been parsed and
/// the affected rows have been painted; that is the byte-to-pixels interval
/// this baseline reports.
fn page(port: u16, cols: u16, rows: u16, webgl: bool) -> String {
    format!(
        r#"<!doctype html><meta charset=utf-8>
<style>{XTERM_CSS}
html,body{{margin:0;height:100%;background:#101014}}
#t{{position:absolute;inset:0}}</style>
<div id=t></div>
<script>{XTERM_JS}</script>
<script>{ADDON_WEBGL_JS}</script>
<script>
const term = new Terminal({{
  allowProposedApi: true,
  cursorBlink: false,
  scrollback: 10000,
  cols: {cols}, rows: {rows},
  fontFamily: 'DejaVu Sans Mono, monospace',
  fontSize: 13,
}});
term.open(document.getElementById('t'));
if ({webgl}) {{ term.loadAddon(new WebglAddon.WebglAddon()); }}

const lat = [];          // byte-to-pixels, ms, one per chunk
const raf = [];          // frame-to-frame interval while data flows, ms
let bytes = 0, first = 0, last = 0, active = false;
let prev = 0, armed = false;
// A permanent `requestAnimationFrame` loop is a wakeup per frame forever, and
// this page is also the subject of an idle-CPU measurement. So the sampler is
// armed by the first byte and disarms itself the moment the stream stops:
// while nothing is arriving the page schedules nothing at all.
function tick(t) {{
  if (!active) {{ armed = false; prev = 0; return; }}
  if (prev) raf.push(t - prev);
  prev = t;
  requestAnimationFrame(tick);
}}
function arm() {{
  if (armed) return;
  armed = true;
  requestAnimationFrame(tick);
}}

const ws = new WebSocket('ws://127.0.0.1:{port}');
ws.binaryType = 'arraybuffer';
ws.onmessage = (ev) => {{
  if (typeof ev.data === 'string') {{ if (ev.data === 'done') finish(); return; }}
  const u8 = new Uint8Array(ev.data);
  const t0 = performance.now();
  if (!first) first = t0;
  active = true;
  arm();
  bytes += u8.length;
  term.write(u8, () => {{ const t1 = performance.now(); last = t1; lat.push(t1 - t0); }});
}};

function q(a, p) {{
  if (!a.length) return 0;
  const s = a.slice().sort((x, y) => x - y);
  return s[Math.min(s.length - 1, Math.round((s.length - 1) * p))];
}}
function finish() {{
  active = false;
  // One more turn of the event loop, so the last write callback lands before
  // the numbers are read.
  term.write('', () => {{
    const span = (last - first) / 1000;
    document.title = 'REPORT:' + JSON.stringify({{
      frames: lat.length,
      bytes: bytes,
      stream_seconds: span,
      bytes_per_second: span > 0 ? bytes / span : 0,
      frame_ms_p50: q(lat, 0.50),
      frame_ms_p95: q(lat, 0.95),
      frame_ms_p99: q(lat, 0.99),
      frame_ms_max: q(lat, 1.0),
      frame_ms_mean: lat.reduce((a, b) => a + b, 0) / (lat.length || 1),
      raf_ms_p50: q(raf, 0.50),
      raf_ms_p95: q(raf, 0.95),
      raf_ms_max: q(raf, 1.0),
      renderer: {webgl} ? 'webgl' : 'dom',
    }});
  }});
}}
</script>"#
    )
}
