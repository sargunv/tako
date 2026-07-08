//! PTY bridge: spawn a child shell on a pseudo-terminal and shuttle bytes
//! between it and a [`Terminal`](crate::terminal::Terminal).
//!
//! [`StreamingPty`] is the live-render PTY: a background reader thread buffers
//! output for the GUI thread to drain each frame. The reader only touches its
//! own mutex buffer, never the terminal.

use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, OwnedFd, RawFd};
use std::sync::{Arc, Mutex};
use std::thread;

use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::unistd::{pipe, read as pipe_read, write as pipe_write};
use portable_pty::cmdbuilder::CommandBuilder;
use portable_pty::{Child, MasterPty, PtySize};

/// A long-lived PTY session with a **background reader thread** that buffers
/// output for a GUI thread to drain. The owner thread (which also owns the
/// [`Terminal`](crate::terminal::Terminal)) calls [`drain`](Self::drain) each
/// frame; the reader thread only touches the shared buffer, never the terminal.
///
/// A **readiness pipe** lets the GUI thread wake on output instead of polling:
/// the reader thread writes one byte after each successful PTY read, and the
/// embedder watches the read end with a fd-based event loop notifier (e.g.
/// Qt's `QSocketNotifier`). If the pipe couldn't be created, [`notify_fd`]
/// returns `None` and the embedder falls back to a timer.
///
/// Drop kills the child (so the reader observes EOF) and joins the reader. The
/// write end of the notify pipe is held here for the lifetime of the session so
/// the read end never sees EOF (which would busy-loop a level-triggered
/// notifier) until the session tears down.
///
/// [`notify_fd`]: StreamingPty::notify_fd
pub struct StreamingPty {
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    /// Held for the lifetime of the session so the master fd stays open.
    master: Box<dyn MasterPty + Send>,
    pending: Arc<Mutex<Vec<u8>>>,
    reader: Option<thread::JoinHandle<()>>,
    /// Read end of the readiness pipe, handed to the embedder's event loop.
    notify_read: Option<OwnedFd>,
    /// Write end of the readiness pipe. Held (not moved into the reader
    /// thread) so the read end doesn't see EOF until [`StreamingPty`] drops.
    /// Never read — it exists purely for its `Drop` to keep the fd open.
    #[allow(dead_code)]
    notify_write: Option<OwnedFd>,
}

impl StreamingPty {
    /// Spawn `$SHELL` (or `/bin/sh`) with a background reader.
    pub fn spawn_shell(cols: u16, rows: u16) -> io::Result<Self> {
        Self::spawn_program(
            cols,
            rows,
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()),
        )
    }

    /// Spawn `program` with `TERM=xterm-256color` and a background reader.
    pub fn spawn_program(cols: u16, rows: u16, program: impl AsRef<str>) -> io::Result<Self> {
        let pty = portable_pty::native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(io_other)?;

        let mut cmd = CommandBuilder::new(program.as_ref());
        cmd.env("TERM", "xterm-256color");
        cmd.cwd(std::env::current_dir()?);

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| io::Error::other(format!("spawn failed: {e}")))?;
        drop(pair.slave);

        let writer = pair
            .master
            .take_writer()
            .map_err(|e| io::Error::other(format!("take_writer failed: {e}")))?;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| io::Error::other(format!("try_clone_reader failed: {e}")))?;

        // Readiness pipe: reader thread writes 1 byte after each PTY read so the
        // GUI event loop can wake on output instead of polling. Both ends
        // non-blocking: the write must not stall the reader if the pipe fills
        // (the wake is already pending), and the GUI drain must not block when
        // the pipe is empty. Failure is non-fatal — the embedder falls back to
        // a timer wake.
        let (notify_read, notify_write) = match pipe() {
            Ok((r, w)) => {
                let _ = fcntl(r.as_raw_fd(), FcntlArg::F_SETFL(OFlag::O_NONBLOCK));
                let _ = fcntl(w.as_raw_fd(), FcntlArg::F_SETFL(OFlag::O_NONBLOCK));
                (Some(r), Some(w))
            }
            Err(e) => {
                log::warn!("notify pipe creation failed ({e}); falling back to timer wake");
                (None, None)
            }
        };
        // The reader thread gets a dup'd write end so the original (kept here)
        // keeps the pipe's write side open — preventing a premature read-EOF
        // that would busy-loop a level-triggered notifier when the reader exits.
        let notify_write_for_thread = notify_write.as_ref().and_then(|w| w.try_clone().ok());

        let pending = Arc::new(Mutex::new(Vec::<u8>::new()));
        let pending_for_thread = Arc::clone(&pending);
        let reader_handle = thread::spawn(move || {
            let mut reader = reader;
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Ok(mut g) = pending_for_thread.lock() {
                            g.extend_from_slice(&buf[..n]);
                        } else {
                            break;
                        }
                        // Wake the GUI: one byte is enough; if the pipe fills
                        // (GUI stalled) the non-blocking write returns EAGAIN
                        // and we drop the byte — the pending wake already set.
                        if let Some(w) = notify_write_for_thread.as_ref() {
                            let _ = pipe_write(w, b"\x01");
                        }
                    }
                }
            }
        });

        Ok(Self {
            writer,
            child,
            master: pair.master,
            pending,
            reader: Some(reader_handle),
            notify_read,
            notify_write,
        })
    }

    /// Take all PTY output buffered so far. Called once per frame from the
    /// terminal owner thread.
    pub fn drain(&self) -> Vec<u8> {
        match self.pending.lock() {
            Ok(mut g) => std::mem::take(&mut *g),
            Err(_) => Vec::new(),
        }
    }

    /// Send bytes (typed input) to the child's stdin.
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    /// Resize the PTY window to the given grid size. Idempotent; safe to call
    /// with the current dimensions. The child sees a `SIGWINCH` and
    /// re-queries the window size via `ioctl(TIOCGWINSZ)`.
    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| io::Error::other(format!("pty resize failed: {e}")))
    }

    /// Raw fd of the readiness-pipe read end, for an event-loop notifier.
    /// `None` if the pipe couldn't be created (fall back to a timer). The fd
    /// is valid for the lifetime of this [`StreamingPty`]; the caller must not
    /// close it.
    pub fn notify_fd(&self) -> Option<RawFd> {
        self.notify_read.as_ref().map(|r| r.as_raw_fd())
    }

    /// Drain all pending wake bytes from the readiness pipe. Non-blocking;
    /// safe to call speculatively. Call this when the notifier fires, before
    /// [`drain`](Self::drain), so the level-triggered source is cleared.
    pub fn drain_notify(&self) {
        let Some(r) = self.notify_read.as_ref() else {
            return;
        };
        let mut buf = [0u8; 64];
        // Non-blocking: loops until the pipe is empty (EAGAIN) or closes.
        while let Ok(n) = pipe_read(r.as_raw_fd(), &mut buf) {
            if n == 0 {
                break;
            }
        }
    }
}

impl Drop for StreamingPty {
    fn drop(&mut self) {
        // Kill the child so the reader sees EOF; then join the reader thread.
        let _ = self.child.kill();
        if let Some(handle) = self.reader.take() {
            let _ = handle.join();
        }
    }
}

fn io_other<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::other(format!("pty error: {e}"))
}
