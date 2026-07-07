//! PTY bridge: spawn a child shell on a pseudo-terminal and shuttle bytes
//! between it and a [`Terminal`](crate::terminal::Terminal).
//!
//! Phase 0 §3 uses [`spawn_shell`] for the real-PTY roundtrip test; the GUI
//! render path (Step C) will reuse this against a background read thread.

use std::io::{self, Read, Write};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use portable_pty::cmdbuilder::CommandBuilder;
use portable_pty::{Child, MasterPty, PtySize};

/// An active PTY session: a writer into the master, a reader draining the
/// master, and the spawned child. Drop kills the child.
pub struct PtySession {
    writer: Box<dyn Write + Send>,
    reader: Option<Box<dyn Read + Send>>,
    child: Box<dyn Child + Send + Sync>,
    /// Held for the lifetime of the session so the master fd stays open.
    _master: Box<dyn MasterPty + Send>,
}

impl PtySession {
    /// Send bytes (e.g. a typed command) to the child's stdin.
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    /// Drain the PTY for `dur`, then kill the child and return everything read.
    ///
    /// This consumes the reader. Killing the child after the drain window
    /// ensures the background read thread observes EOF and exits.
    pub fn read_for(&mut self, dur: Duration) -> Vec<u8> {
        let reader = self
            .reader
            .take()
            .expect("PtySession reader already consumed");
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let handle = thread::spawn(move || {
            let mut reader = reader;
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        thread::sleep(dur);
        let out: Vec<u8> = rx.try_iter().flatten().collect();
        let _ = self.child.kill();
        let _ = handle.join();
        out
    }
}

/// Spawn the user's `$SHELL` (or `/bin/sh`) on a PTY of the given cell size.
pub fn spawn_shell(cols: u16, rows: u16) -> io::Result<PtySession> {
    spawn_program(
        cols,
        rows,
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()),
    )
}

/// Spawn `program` on a PTY of the given cell size, with `TERM=xterm-256color`
/// and the current directory.
pub fn spawn_program(cols: u16, rows: u16, program: impl AsRef<str>) -> io::Result<PtySession> {
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

    // Drop the slave in the parent so EOF reaches the reader when the child
    // exits. Keep the master alive via the session.
    drop(pair.slave);

    let writer = pair
        .master
        .take_writer()
        .map_err(|e| io::Error::other(format!("take_writer failed: {e}")))?;
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| io::Error::other(format!("try_clone_reader failed: {e}")))?;

    Ok(PtySession {
        writer,
        reader: Some(reader),
        child,
        _master: pair.master,
    })
}

fn io_other<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::other(format!("pty error: {e}"))
}

/// A long-lived PTY session with a **background reader thread** that buffers
/// output for a GUI thread to drain. Built for the live render path (Step C):
/// the owner thread (which also owns the [`Terminal`](crate::terminal::Terminal))
/// calls [`drain`](Self::drain) each frame; the reader thread only touches the
/// shared buffer, never the terminal.
///
/// Drop kills the child (so the reader observes EOF) and joins the reader.
pub struct StreamingPty {
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    _master: Box<dyn MasterPty + Send>,
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
            _master: pair.master,
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
