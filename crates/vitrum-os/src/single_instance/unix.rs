//! `flock` plus a Unix domain socket.
//!
//! The lock and the socket are two different jobs and both are needed. The lock
//! answers "am I first?" atomically and is released by the kernel on process
//! death, so a crash leaves no stale claim. The socket carries the second
//! launch's intent to the first, which a lock cannot do.
//!
//! Order matters: the lock is taken first, and only the lock holder binds the
//! socket. A stale socket file from a previous crash is removed by the new
//! lock holder, which is safe precisely because holding the lock proves no
//! other instance is using it.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rustix::fs::{FlockOperation, flock};

use crate::paths::Platform;
use crate::single_instance::{
    Acquisition, ActivationSink, InstanceGuard, MAX_ACTIVATION_LEN, SingleInstanceError,
    check_socket_path, decode_activation, encode_activation,
};

fn io<E: core::fmt::Display>(context: &str) -> impl FnOnce(E) -> SingleInstanceError + '_ {
    move |e| SingleInstanceError::Io { context: context.to_string(), detail: e.to_string() }
}

pub(crate) struct UnixGuard {
    /// Held open for the process lifetime; closing it releases the `flock`.
    _lock: std::fs::File,
    socket_path: PathBuf,
    listener: std::sync::Mutex<Option<UnixListener>>,
    shutdown: Arc<AtomicBool>,
}

pub fn acquire(
    lock_path: &Path,
    socket_path: &Path,
    activation: &crate::single_instance::Activation,
) -> Result<Acquisition, SingleInstanceError> {
    check_socket_path(Platform::current(), socket_path)?;

    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent).map_err(io("creating the runtime directory"))?;
    }

    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .map_err(io("opening the instance lock"))?;

    match flock(&lock, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => {}
        Err(rustix::io::Errno::WOULDBLOCK) => {
            // Someone else is primary. Hand off and exit.
            hand_off(socket_path, activation)?;
            return Ok(Acquisition::HandedOff);
        }
        Err(e) => return Err(io("locking the instance lock")(e)),
    }

    // We hold the lock, so any socket file here is a corpse from a crash.
    match std::fs::remove_file(socket_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io("removing a stale instance socket")(e)),
    }
    let listener = UnixListener::bind(socket_path).map_err(io("binding the instance socket"))?;

    Ok(Acquisition::Primary(InstanceGuard {
        inner: UnixGuard {
            _lock: lock,
            socket_path: socket_path.to_path_buf(),
            listener: std::sync::Mutex::new(Some(listener)),
            shutdown: Arc::new(AtomicBool::new(false)),
        },
    }))
}

fn hand_off(
    socket_path: &Path,
    activation: &crate::single_instance::Activation,
) -> Result<(), SingleInstanceError> {
    let mut stream = UnixStream::connect(socket_path).map_err(|e| {
        SingleInstanceError::PrimaryUnreachable {
            detail: format!("connecting to {}: {e}", socket_path.display()),
        }
    })?;
    stream.write_all(&encode_activation(activation)).map_err(|e| {
        SingleInstanceError::PrimaryUnreachable { detail: format!("writing the activation: {e}") }
    })?;
    // Half-close so the reader sees EOF instead of waiting for more.
    stream
        .shutdown(std::net::Shutdown::Write)
        .map_err(|e| SingleInstanceError::PrimaryUnreachable {
            detail: format!("closing the activation stream: {e}"),
        })
}

impl UnixGuard {
    pub fn listen(&self, sink: ActivationSink) -> Result<(), SingleInstanceError> {
        let listener = self
            .listener
            .lock()
            .expect("listener slot is never held across a panic")
            .take()
            .ok_or_else(|| SingleInstanceError::Io {
                context: "starting the activation listener".to_string(),
                detail: "already listening".to_string(),
            })?;
        let shutdown = Arc::clone(&self.shutdown);
        std::thread::Builder::new()
            .name("vitrum-instance-listener".to_string())
            .spawn(move || {
                for stream in listener.incoming() {
                    if shutdown.load(Ordering::SeqCst) {
                        return;
                    }
                    let Ok(mut stream) = stream else { continue };
                    let mut buf = Vec::new();
                    // Bounded read: a peer that never closes must not be able
                    // to grow this without limit.
                    if std::io::Read::by_ref(&mut stream)
                        .take(MAX_ACTIVATION_LEN as u64 + 1)
                        .read_to_end(&mut buf)
                        .is_err()
                    {
                        continue;
                    }
                    if let Ok(activation) = decode_activation(&buf) {
                        sink(activation);
                    }
                }
            })
            .map_err(io("spawning the activation listener"))?;
        Ok(())
    }
}

impl Drop for UnixGuard {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        // Wake the accept loop so the thread observes the flag and returns,
        // rather than sitting on a socket whose file is about to vanish.
        if let Ok(stream) = UnixStream::connect(&self.socket_path) {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
        let _ = std::fs::remove_file(&self.socket_path);
    }
}
