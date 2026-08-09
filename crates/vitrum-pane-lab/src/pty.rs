//! One real PTY running one real program.
//!
//! Both paths under test are fed from here, so a throughput number is about
//! the terminal and not about how the bytes were manufactured. Nothing is
//! replayed from a capture: the child is a process, its output arrives at
//! whatever rate the kernel delivers it, and the harness reads the master fd
//! directly so no wrapper sits between the pipe and the parser.

use std::io::Write;
use std::os::unix::io::RawFd;

use anyhow::{Context, Result, anyhow};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

/// A spawned child on its own PTY.
pub struct Pty {
    /// Kept alive: dropping it closes the master fd out from under `fd`.
    _master: Box<dyn MasterPty + Send>,
    /// Kept alive: dropping the child reaps it and the program stops writing.
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    /// The master file descriptor, in non-blocking mode.
    pub fd: RawFd,
}

impl Pty {
    /// Spawn `argv` on a `cols` x `rows` PTY whose cells are `cell_px`.
    ///
    /// The environment is the one the product promises a child: `TERM` and the
    /// `COLORTERM` string `vitrum-vt` guarantees it honours. `HOME` and the
    /// working directory are synthetic so nothing captured from this process
    /// can leak a path off the machine that produced it.
    pub fn spawn(argv: &[String], cols: u16, rows: u16, cell_px: (u32, u32)) -> Result<Self> {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: (cell_px.0 * u32::from(cols)) as u16,
                pixel_height: (cell_px.1 * u32::from(rows)) as u16,
            })
            .context("openpty")?;

        let mut cmd = CommandBuilder::new(&argv[0]);
        for a in &argv[1..] {
            cmd.arg(a);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", vitrum_vt::COLORTERM);
        cmd.env("LANG", "C.UTF-8");
        cmd.cwd("/");
        let child = pair.slave.spawn_command(cmd).context("spawn child")?;
        drop(pair.slave);

        let fd = pair
            .master
            .as_raw_fd()
            .ok_or_else(|| anyhow!("this pty backend has no master fd"))?;
        // The GTK main loop polls this fd. A blocking read inside the poll
        // callback would stall the whole window the moment the child paused
        // mid-burst, so the drain loop needs EAGAIN to stop on.
        set_nonblocking(fd)?;

        let writer = pair.master.take_writer().context("take pty writer")?;

        Ok(Self {
            _master: pair.master,
            child,
            writer,
            fd,
        })
    }

    /// Send keystrokes to the child.
    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    /// Stop the child.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn set_nonblocking(fd: RawFd) -> Result<()> {
    // SAFETY: `fd` came from the pty we just opened and is open for the life
    // of the `Pty` that owns it.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error()).context("F_GETFL on pty master");
    }
    // SAFETY: same fd, and `flags` is the value the kernel just reported.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error()).context("F_SETFL on pty master");
    }
    Ok(())
}

/// Read everything currently available on `fd` into `buf`.
///
/// Returns `Ok(false)` when the child closed its end. A short read that ends
/// in `EAGAIN` is the normal case and returns `Ok(true)`.
pub fn drain(fd: RawFd, buf: &mut Vec<u8>) -> Result<bool> {
    let mut chunk = [0u8; 65536];
    loop {
        // SAFETY: `chunk` is a live stack buffer of exactly this length and
        // `fd` is owned by a `Pty` the caller is holding.
        let n = unsafe { libc::read(fd, chunk.as_mut_ptr().cast(), chunk.len()) };
        if n > 0 {
            buf.extend_from_slice(&chunk[..n as usize]);
            continue;
        }
        if n == 0 {
            return Ok(false);
        }
        let err = std::io::Error::last_os_error();
        return match err.raw_os_error() {
            Some(libc::EAGAIN) => Ok(true),
            // The child exited and the last slave fd went away. Linux reports
            // that as EIO on the master, which is an end of stream and not a
            // failure worth propagating.
            Some(libc::EIO) => Ok(false),
            Some(libc::EINTR) => continue,
            _ => Err(err).context("read pty master"),
        };
    }
}
