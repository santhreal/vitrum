//! A named mutex plus a named pipe.
//!
//! `CreateMutexW` in the `Local\` namespace is the race-free decision: the
//! kernel guarantees exactly one creator, and it releases the name when the
//! owning process dies, so a crash leaves nothing to clean up. The named pipe
//! beside it carries the handoff.
//!
//! No lock file: on Windows a file lock adds nothing a kernel object does not
//! already do, and it would leave a file behind that a user could delete while
//! the app is running.

use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GENERIC_WRITE, GetLastError, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_MODE, OPEN_EXISTING, PIPE_ACCESS_INBOUND,
    ReadFile, WriteFile,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE,
    PIPE_WAIT,
};
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::PCWSTR;

use crate::single_instance::{
    Acquisition, Activation, ActivationSink, InstanceGuard, MAX_ACTIVATION_LEN,
    SingleInstanceError, decode_activation, encode_activation, windows_mutex_name,
    windows_pipe_name,
};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(core::iter::once(0)).collect()
}

fn io<E: core::fmt::Display>(context: &str) -> impl FnOnce(E) -> SingleInstanceError + '_ {
    move |e| SingleInstanceError::Io { context: context.to_string(), detail: e.to_string() }
}

/// Current user name, used to scope the machine-global pipe name.
fn current_user() -> String {
    std::env::var("USERNAME").unwrap_or_else(|_| "default".to_string())
}

pub(crate) struct WindowsGuard {
    /// Held for the process lifetime; closing it frees the name.
    mutex: HANDLE,
    pipe_name: String,
    listening: AtomicBool,
    shutdown: Arc<AtomicBool>,
}

// SAFETY: a `HANDLE` is an opaque kernel handle usable from any thread, and the
// remaining fields are atomics or immutable.
unsafe impl Send for WindowsGuard {}
unsafe impl Sync for WindowsGuard {}

pub fn acquire(
    _lock_path: &Path,
    activation: &Activation,
) -> Result<Acquisition, SingleInstanceError> {
    let name = wide(&windows_mutex_name());
    // SAFETY: `name` is NUL-terminated and outlives the call.
    let mutex = unsafe { CreateMutexW(None, true, PCWSTR(name.as_ptr())) }
        .map_err(io("creating the instance mutex"))?;
    // SAFETY: reading the calling thread's last error immediately after the
    // call that set it.
    let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;

    let pipe_name = windows_pipe_name(&current_user());
    if already {
        // SAFETY: our own handle, not used again.
        unsafe {
            let _ = CloseHandle(mutex);
        }
        hand_off(&pipe_name, activation)?;
        return Ok(Acquisition::HandedOff);
    }

    Ok(Acquisition::Primary(InstanceGuard {
        inner: WindowsGuard {
            mutex,
            pipe_name,
            listening: AtomicBool::new(false),
            shutdown: Arc::new(AtomicBool::new(false)),
        },
    }))
}

/// Write the activation into the primary's pipe.
///
/// `std::fs::File` opens `\\.\pipe\...` directly on Windows, so there is no
/// reason to hand-roll `CreateFileW` for the client side.
fn hand_off(pipe_name: &str, activation: &Activation) -> Result<(), SingleInstanceError> {
    let mut pipe = std::fs::OpenOptions::new().write(true).open(pipe_name).map_err(|e| {
        SingleInstanceError::PrimaryUnreachable {
            detail: format!("opening {pipe_name}: {e}"),
        }
    })?;
    pipe.write_all(&encode_activation(activation)).map_err(|e| {
        SingleInstanceError::PrimaryUnreachable { detail: format!("writing the activation: {e}") }
    })?;
    pipe.flush().map_err(|e| SingleInstanceError::PrimaryUnreachable {
        detail: format!("flushing the activation: {e}"),
    })
}

impl WindowsGuard {
    pub fn listen(&self, sink: ActivationSink) -> Result<(), SingleInstanceError> {
        if self.listening.swap(true, Ordering::SeqCst) {
            return Err(SingleInstanceError::Io {
                context: "starting the activation listener".to_string(),
                detail: "already listening".to_string(),
            });
        }
        let pipe_name = self.pipe_name.clone();
        let shutdown = Arc::clone(&self.shutdown);
        std::thread::Builder::new()
            .name("vitrum-instance-listener".to_string())
            .spawn(move || {
                let name = wide(&pipe_name);
                while !shutdown.load(Ordering::SeqCst) {
                    // One instance at a time: the server serialises handoffs,
                    // which is fine because a handoff is one short line.
                    // SAFETY: `name` is NUL-terminated and outlives the call.
                    let pipe = unsafe {
                        CreateNamedPipeW(
                            PCWSTR(name.as_ptr()),
                            PIPE_ACCESS_INBOUND,
                            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                            1,
                            0,
                            MAX_ACTIVATION_LEN as u32,
                            0,
                            None,
                        )
                    };
                    if pipe.is_invalid() {
                        return;
                    }
                    // SAFETY: `pipe` is a valid server handle; blocks until a
                    // client connects.
                    let connected = unsafe { ConnectNamedPipe(pipe, None) }.is_ok();
                    if connected && !shutdown.load(Ordering::SeqCst) {
                        let mut buf = [0u8; MAX_ACTIVATION_LEN];
                        let mut read = 0u32;
                        // SAFETY: `buf` is a live buffer of the stated length.
                        let ok = unsafe {
                            ReadFile(pipe, Some(&mut buf), Some(&raw mut read), None)
                        }
                        .is_ok();
                        if ok && read > 0 {
                            if let Ok(activation) = decode_activation(&buf[..read as usize]) {
                                sink(activation);
                            }
                        }
                    }
                    // SAFETY: our own handles, released in order.
                    unsafe {
                        let _ = DisconnectNamedPipe(pipe);
                        let _ = CloseHandle(pipe);
                    }
                }
            })
            .map_err(io("spawning the activation listener"))?;
        Ok(())
    }
}

impl Drop for WindowsGuard {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        // Wake a thread blocked in ConnectNamedPipe so it sees the flag.
        let name = wide(&self.pipe_name);
        // SAFETY: opening our own pipe by name; failure is fine, it only means
        // the server was not waiting.
        let client = unsafe {
            CreateFileW(
                PCWSTR(name.as_ptr()),
                GENERIC_WRITE.0,
                FILE_SHARE_MODE(0),
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        };
        if let Ok(client) = client {
            // SAFETY: a valid handle we just opened.
            unsafe {
                let mut written = 0u32;
                let _ = WriteFile(client, Some(b"\n"), Some(&raw mut written), None);
                let _ = CloseHandle(client);
            }
        }
        // SAFETY: our own mutex handle, not used again.
        unsafe {
            let _ = CloseHandle(self.mutex);
        }
    }
}
