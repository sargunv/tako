//! PTY bridge: spawn a child shell on a pseudo-terminal and shuttle bytes
//! between it and a [`Terminal`](crate::terminal::Terminal).
//!
//! [`StreamingPty`] is the live-render PTY: a background reader thread buffers
//! output for the GUI thread to drain each frame. The reader only touches its
//! own mutex buffer, never the terminal.

use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;

use portable_pty::cmdbuilder::CommandBuilder;
use portable_pty::{Child, MasterPty, PtySize};

/// A long-lived PTY session with a **background reader thread** that buffers
/// output for a GUI thread to drain. The owner thread (which also owns the
/// [`Terminal`](crate::terminal::Terminal)) calls [`drain`](Self::drain) each
/// frame; the reader thread only touches the shared buffer, never the terminal.
///
/// Drop kills the child (so the reader observes EOF) and joins the reader.
pub struct StreamingPty {
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    /// Held for the lifetime of the session so the master fd stays open.
    master: Box<dyn MasterPty + Send>,
    pending: Arc<Mutex<Vec<u8>>>,
    reader: Option<thread::JoinHandle<()>>,
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
