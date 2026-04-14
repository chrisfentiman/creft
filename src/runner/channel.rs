use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::sync::Arc;

/// File descriptor number for the block → creft message channel.
pub(super) const CONTROL_FD: i32 = 3;
/// File descriptor number for the creft → block response channel.
pub(super) const RESPONSE_FD: i32 = 4;

/// A side channel connecting a child block process to the creft parent.
///
/// Creates two OS pipes:
/// - **control pipe**: child writes (fd 3) → parent reads
/// - **response pipe**: parent writes → child reads (fd 4)
///
/// The parent holds the read end of the control pipe and the write end
/// of the response pipe. The child inherits the opposite ends via
/// `dup2` in the `pre_exec` hook.
///
/// Drop closes the parent's ends. The child's ends are closed when the
/// child process exits (they are not marked close-on-exec — the
/// `pre_exec` hook clears that flag after `dup2`).
pub(crate) struct SideChannel {
    /// Read end of the control pipe (parent reads block messages).
    /// `Option` because `take_control_reader` moves it out.
    control_reader: Option<OwnedFd>,
    /// Write end of the control pipe (child writes via fd 3).
    /// Held here only to keep it alive until `pre_exec` dups it.
    /// Dropped after spawn to close the parent's copy.
    control_writer: OwnedFd,
    /// Write end of the response pipe (parent writes prompt responses).
    /// `Option` because `take_response_writer` moves it out.
    response_writer: Option<OwnedFd>,
    /// Read end of the response pipe (child reads via fd 4).
    /// Held here only to keep it alive until `pre_exec` dups it.
    /// Dropped after spawn to close the parent's copy.
    response_reader: OwnedFd,
}

impl SideChannel {
    /// Create a new side channel with two OS pipe pairs.
    pub(crate) fn new() -> std::io::Result<Self> {
        let (ctrl_r, ctrl_w) = os_pipe()?;
        let (resp_r, resp_w) = os_pipe()?;
        Ok(Self {
            control_reader: Some(ctrl_r),
            control_writer: ctrl_w,
            response_writer: Some(resp_w),
            response_reader: resp_r,
        })
    }

    /// Raw fd values the child process needs for `dup2` in `pre_exec`.
    ///
    /// Returns `(control_write_fd, response_read_fd)` — the child's
    /// ends of the two pipes.
    pub(crate) fn child_fds(&self) -> (i32, i32) {
        (
            self.control_writer.as_raw_fd(),
            self.response_reader.as_raw_fd(),
        )
    }

    /// Take the parent's read end of the control pipe.
    ///
    /// Called once after spawn to move the reader into the reader thread.
    /// Consumes the fd — subsequent calls return `None`.
    pub(crate) fn take_control_reader(&mut self) -> Option<OwnedFd> {
        self.control_reader.take()
    }

    /// Take the parent's write end of the response pipe.
    ///
    /// Called once after spawn to move the writer into the writer/prompt thread.
    /// Consumes the fd — subsequent calls return `None`.
    pub(crate) fn take_response_writer(&mut self) -> Option<OwnedFd> {
        self.response_writer.take()
    }

    /// Close the parent's copies of the child's pipe ends.
    ///
    /// Must be called after spawn. If the parent keeps these fds open,
    /// the child's reads on fd 4 will never see EOF (the parent's copy
    /// of the write end keeps the pipe alive), and the parent's reads
    /// on the control pipe may block (the parent's copy of the write
    /// end prevents EOF on the control reader).
    pub(crate) fn close_child_ends(&mut self) {
        // Drop by replacing with a placeholder then immediately dropping.
        // We reconstruct a dummy fd pointing to /dev/null is unnecessary —
        // simply moving out of the fields and dropping achieves the close.
        // Use a private helper to consume the fields.
        drop_fd(&mut self.control_writer);
        drop_fd(&mut self.response_reader);
    }
}

/// Drop the fd held in `slot` by swapping in a fresh dummy.
///
/// `OwnedFd` does not implement `Default`, so we open /dev/null as a
/// harmless placeholder that gets immediately dropped.  The OS reclaims
/// both file descriptions on close.
fn drop_fd(slot: &mut OwnedFd) {
    // SAFETY: open(2) with O_RDONLY on /dev/null always succeeds on any
    // POSIX system and returns a valid file descriptor.  We immediately
    // wrap it in OwnedFd so it is closed on drop.  The old fd in `slot`
    // is closed when the old value is dropped by `std::mem::replace`.
    let devnull = unsafe {
        let fd = libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
        OwnedFd::from_raw_fd(fd)
    };
    let _old = std::mem::replace(slot, devnull);
    // `_old` drops here, closing the original fd.
}

/// Create a POSIX pipe and return `(read_end, write_end)` as `OwnedFd`s.
///
/// Sets O_CLOEXEC on both ends via `fcntl` after creation. The
/// `pre_exec` hook in `spawn_block` clears this flag (implicitly) after
/// `dup2` — `dup2` does not inherit `FD_CLOEXEC`, so the duplicated fds
/// remain open across `exec` as intended.
fn os_pipe() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0i32; 2];
    // SAFETY: `fds` is a valid two-element array. pipe(2) writes exactly
    // two file descriptor integers into it and returns -1 on error, 0 on
    // success. Both resulting fds are valid and owned by this scope.
    let ret = unsafe { libc::pipe(fds.as_mut_ptr()) };
    if ret == -1 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: pipe(2) returned 0, so both fds are valid, open, and not
    // shared with any other owner. Wrapping them in OwnedFd transfers
    // ownership and ensures they are closed on drop.
    let r = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let w = unsafe { OwnedFd::from_raw_fd(fds[1]) };

    // Set O_CLOEXEC on both ends so they don't leak into unrelated
    // child processes (only the block that uses dup2 in pre_exec gets them).
    // SAFETY: both fds are valid and owned by r/w; fcntl F_SETFD is safe.
    for fd in [fds[0], fds[1]] {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags == -1 {
            return Err(std::io::Error::last_os_error());
        }
        let ret = unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
        if ret == -1 {
            return Err(std::io::Error::last_os_error());
        }
    }

    Ok((r, w))
}

/// A message from a block to creft via the side channel.
///
/// Each line on fd 3 is a newline-delimited JSON object. The `type` field
/// determines which variant is decoded. Unrecognised types are silently
/// dropped by the reader thread.
#[derive(Debug, serde::Deserialize)]
#[serde(tag = "type")]
// `choices` on the Prompt variant is part of the wire protocol and will
// be consumed by the stage 4 prompt handler. Suppress the dead-code lint
// here rather than removing a protocol field.
#[allow(dead_code)]
pub(crate) enum ChannelMessage {
    #[serde(rename = "print")]
    Print { message: String },
    #[serde(rename = "status")]
    Status { message: String },
    #[serde(rename = "prompt")]
    Prompt {
        id: String,
        question: String,
        choices: String,
    },
}

/// State behind the `TerminalWriter` mutex.
///
/// Combining `has_status` and `stderr` in a single lock means that
/// clearing a status line and writing the next message are atomic.
/// Two separate locks would allow a concurrent thread to write between
/// the status clear and the new content.
struct TerminalWriterInner {
    stderr: std::io::Stderr,
    /// True when a status line has been written and not yet cleared.
    has_status: bool,
}

/// Serialises terminal output from multiple concurrent reader threads.
///
/// In a pipe chain each block runs its own reader thread. Without
/// synchronisation, concurrent writes to stderr interleave. Every public
/// method acquires the inner mutex for the duration of a single write,
/// so each message renders atomically.
pub(crate) struct TerminalWriter {
    inner: std::sync::Mutex<TerminalWriterInner>,
}

impl TerminalWriter {
    pub(crate) fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(TerminalWriterInner {
                stderr: std::io::stderr(),
                has_status: false,
            }),
        }
    }

    /// Write a print message to stderr.
    ///
    /// Clears any active status line first so the print text appears on a
    /// clean line. After writing, `has_status` is false.
    pub(crate) fn print(&self, message: &str) {
        let Ok(mut g) = self.inner.lock() else {
            return;
        };
        if g.has_status {
            // Carriage-return + spaces clears the status line content.
            let _ = write!(g.stderr, "\r{:width$}\r", "", width = 80);
            g.has_status = false;
        }
        let _ = writeln!(g.stderr, "{message}");
    }

    /// Write a status line that will be overwritten by the next status.
    ///
    /// Uses `\r` to move to the start of the current line, clears it with
    /// spaces, then writes the new message without a trailing newline so the
    /// next status can overwrite it. When a print message arrives afterwards,
    /// `print` clears this line first.
    pub(crate) fn status(&self, message: &str) {
        let Ok(mut g) = self.inner.lock() else {
            return;
        };
        // Clear any previous status content before writing the new one.
        let _ = write!(g.stderr, "\r{:width$}\r{message}", "", width = 80);
        g.has_status = true;
    }

    /// Clear the active status line, if any.
    ///
    /// Called when the block exits so no stale status line remains on screen.
    pub(crate) fn clear_status(&self) {
        let Ok(mut g) = self.inner.lock() else {
            return;
        };
        if g.has_status {
            let _ = write!(g.stderr, "\r{:width$}\r", "", width = 80);
            g.has_status = false;
        }
    }
}

/// Spawn a reader thread that drains the control pipe and renders messages.
///
/// The thread reads NDJSON lines from `control_reader` until EOF (the child
/// has exited and all write ends are closed). Each parseable line is matched
/// against `ChannelMessage` variants:
///
/// - `Print` — rendered immediately via `writer.print`.
/// - `Status` — rendered via `writer.status` (overwrites the previous status).
/// - `Prompt` — in stage 3, `prompt_tx` is always `None`; the reader writes
///   an empty response to `response_writer` so the child does not deadlock on
///   its `read(&4)` call, and logs the question as a print message.
///
/// Malformed JSON lines are silently skipped.
///
/// Returns the join handle. Call `.join()` after `wait_with_output()` to ensure
/// all messages are rendered before the parent process exits.
pub(crate) fn spawn_reader(
    control_reader: OwnedFd,
    writer: Arc<TerminalWriter>,
    prompt_tx: Option<std::sync::mpsc::Sender<ChannelMessage>>,
    mut response_writer: Option<File>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("creft-channel-reader".to_owned())
        .spawn(move || {
            // SAFETY: control_reader is valid and exclusively owned by this
            // thread. Converting to File gives buffered line-by-line reading.
            let file = unsafe { File::from_raw_fd(control_reader.into_raw_fd()) };
            let reader = BufReader::new(file);

            for line in reader.lines() {
                let line = match line {
                    Ok(l) => l,
                    // Read error (e.g. pipe broken): stop processing.
                    Err(_) => break,
                };

                let msg = match serde_json::from_str::<ChannelMessage>(&line) {
                    Ok(m) => m,
                    // Unknown type or malformed JSON: skip silently.
                    Err(_) => continue,
                };

                match msg {
                    ChannelMessage::Print { message } => {
                        writer.print(&message);
                    }
                    ChannelMessage::Status { message } => {
                        writer.status(&message);
                    }
                    ChannelMessage::Prompt { id, question, .. } => {
                        if let Some(tx) = &prompt_tx {
                            // Stage 4: forward to the prompt handler thread.
                            let _ = tx.send(ChannelMessage::Prompt {
                                id,
                                question,
                                choices: String::new(),
                            });
                        } else {
                            // Stage 3 / pipe-chain context: log the question and
                            // write an empty response so the child doesn't hang.
                            writer.print(&question);
                            if let Some(ref mut rw) = response_writer {
                                let response = format!("{{\"id\":\"{id}\",\"value\":\"\"}}\n");
                                let _ = rw.write_all(response.as_bytes());
                                let _ = rw.flush();
                            }
                        }
                    }
                }
            }

            // Clear any lingering status line when the block exits.
            writer.clear_status();
        })
        .expect("failed to spawn channel reader thread")
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;
    use std::os::fd::AsRawFd as _;
    use std::os::unix::io::{FromRawFd as _, IntoRawFd as _};

    use pretty_assertions::assert_eq;

    use super::SideChannel;

    /// All four file descriptors in a fresh SideChannel are distinct.
    #[test]
    fn new_produces_four_distinct_fds() {
        let ch = SideChannel::new().unwrap();
        let ctrl_r = ch.control_reader.as_ref().unwrap().as_raw_fd();
        let ctrl_w = ch.control_writer.as_raw_fd();
        let resp_w = ch.response_writer.as_ref().unwrap().as_raw_fd();
        let resp_r = ch.response_reader.as_raw_fd();

        let fds = [ctrl_r, ctrl_w, resp_w, resp_r];
        let unique: std::collections::HashSet<i32> = fds.iter().copied().collect();
        assert_eq!(
            unique.len(),
            4,
            "all four fds must be distinct; got {fds:?}"
        );
    }

    /// child_fds returns control_writer and response_reader raw fds.
    #[test]
    fn child_fds_returns_correct_ends() {
        let ch = SideChannel::new().unwrap();
        let (cfd, rfd) = ch.child_fds();
        assert_eq!(cfd, ch.control_writer.as_raw_fd());
        assert_eq!(rfd, ch.response_reader.as_raw_fd());
    }

    /// take_control_reader moves the fd out; subsequent calls return None.
    #[test]
    fn take_control_reader_is_idempotent() {
        let mut ch = SideChannel::new().unwrap();
        let first = ch.take_control_reader();
        assert!(first.is_some(), "first take should return the fd");
        let second = ch.take_control_reader();
        assert!(second.is_none(), "second take should return None");
    }

    /// take_response_writer moves the fd out; subsequent calls return None.
    #[test]
    fn take_response_writer_is_idempotent() {
        let mut ch = SideChannel::new().unwrap();
        let first = ch.take_response_writer();
        assert!(first.is_some(), "first take should return the fd");
        let second = ch.take_response_writer();
        assert!(second.is_none(), "second take should return None");
    }

    /// A spawned bash process that writes to fd 3 produces bytes on the
    /// parent's control reader.
    #[test]
    fn bash_writes_to_fd3_are_readable_by_parent() {
        let mut ch = SideChannel::new().unwrap();
        let (ctrl_w_fd, _resp_r_fd) = ch.child_fds();

        // Spawn bash, inherit fd 3 (ctrl_w_fd) as fd 3.
        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c");
        cmd.arg("echo -n hello >&3");
        // SAFETY: dup2 and close are async-signal-safe POSIX calls.  The
        // captured fd value is Copy (i32). No Rust allocations occur.
        unsafe {
            use std::os::unix::process::CommandExt as _;
            cmd.pre_exec(move || {
                if libc::dup2(ctrl_w_fd, 3) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if ctrl_w_fd != 3 {
                    libc::close(ctrl_w_fd);
                }
                Ok(())
            });
        }

        let ctrl_reader_fd = ch.take_control_reader().unwrap();

        let mut child = cmd.spawn().unwrap();

        // Close the parent's write end so we see EOF after the child exits.
        ch.close_child_ends();

        child.wait().unwrap();

        // Read what the child wrote.
        let mut reader = unsafe { std::fs::File::from_raw_fd(ctrl_reader_fd.into_raw_fd()) };
        let mut buf = String::new();
        reader.read_to_string(&mut buf).unwrap();
        assert_eq!(buf, "hello");
    }

    /// A bash process that never writes to fd 3 exits cleanly — no hang.
    #[test]
    fn bash_ignoring_fd3_exits_cleanly() {
        let mut ch = SideChannel::new().unwrap();
        let (ctrl_w_fd, _resp_r_fd) = ch.child_fds();

        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c");
        cmd.arg("exit 0");
        unsafe {
            use std::os::unix::process::CommandExt as _;
            cmd.pre_exec(move || {
                if libc::dup2(ctrl_w_fd, 3) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if ctrl_w_fd != 3 {
                    libc::close(ctrl_w_fd);
                }
                Ok(())
            });
        }

        let ctrl_reader_fd = ch.take_control_reader().unwrap();

        let mut child = cmd.spawn().unwrap();
        ch.close_child_ends();
        let status = child.wait().unwrap();
        assert!(status.success());

        // Drain the reader — should see EOF immediately.
        let mut reader = unsafe { std::fs::File::from_raw_fd(ctrl_reader_fd.into_raw_fd()) };
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).unwrap();
        assert!(buf.is_empty(), "no bytes expected from a silent child");
    }

    /// After close_child_ends, the parent no longer holds the child's pipe
    /// write end, so the control reader reaches EOF after the child exits.
    #[test]
    fn close_child_ends_prevents_reader_hang() {
        let mut ch = SideChannel::new().unwrap();
        let (ctrl_w_fd, _) = ch.child_fds();

        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c");
        cmd.arg("echo -n ping >&3");
        unsafe {
            use std::os::unix::process::CommandExt as _;
            cmd.pre_exec(move || {
                if libc::dup2(ctrl_w_fd, 3) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if ctrl_w_fd != 3 {
                    libc::close(ctrl_w_fd);
                }
                Ok(())
            });
        }

        let ctrl_reader_fd = ch.take_control_reader().unwrap();
        let mut child = cmd.spawn().unwrap();

        // This is the critical call: without it the reader would hang
        // because the parent still holds the write end of the control pipe.
        ch.close_child_ends();

        child.wait().unwrap();

        let mut reader = unsafe { std::fs::File::from_raw_fd(ctrl_reader_fd.into_raw_fd()) };
        let mut buf = String::new();
        // read_to_string would block indefinitely if the parent still held
        // the write end open.
        reader.read_to_string(&mut buf).unwrap();
        assert_eq!(buf, "ping");
    }

    // ---- ChannelMessage deserialization ----

    /// A well-formed print message deserializes to the Print variant.
    #[test]
    fn channel_message_print_deserializes() {
        let json = r#"{"type":"print","message":"hello world"}"#;
        let msg: super::ChannelMessage = serde_json::from_str(json).unwrap();
        match msg {
            super::ChannelMessage::Print { message } => {
                assert_eq!(message, "hello world");
            }
            other => panic!("expected Print, got {other:?}"),
        }
    }

    /// A well-formed status message deserializes to the Status variant.
    #[test]
    fn channel_message_status_deserializes() {
        let json = r#"{"type":"status","message":"Loading..."}"#;
        let msg: super::ChannelMessage = serde_json::from_str(json).unwrap();
        match msg {
            super::ChannelMessage::Status { message } => {
                assert_eq!(message, "Loading...");
            }
            other => panic!("expected Status, got {other:?}"),
        }
    }

    /// A well-formed prompt message deserializes to the Prompt variant.
    #[test]
    fn channel_message_prompt_deserializes() {
        let json = r#"{"type":"prompt","id":"p1","question":"Continue?","choices":"yes,no"}"#;
        let msg: super::ChannelMessage = serde_json::from_str(json).unwrap();
        match msg {
            super::ChannelMessage::Prompt {
                id,
                question,
                choices,
            } => {
                assert_eq!(id, "p1");
                assert_eq!(question, "Continue?");
                assert_eq!(choices, "yes,no");
            }
            other => panic!("expected Prompt, got {other:?}"),
        }
    }

    /// An unknown message type returns a deserialization error rather than panicking.
    #[test]
    fn channel_message_unknown_type_returns_error() {
        let json = r#"{"type":"unknown","data":"whatever"}"#;
        let result = serde_json::from_str::<super::ChannelMessage>(json);
        assert!(
            result.is_err(),
            "unknown type must produce a deserialization error"
        );
    }

    /// Completely malformed JSON returns a deserialization error.
    #[test]
    fn channel_message_malformed_json_returns_error() {
        let result = serde_json::from_str::<super::ChannelMessage>("not json at all");
        assert!(result.is_err(), "malformed JSON must produce an error");
    }

    // ---- TerminalWriter ----

    /// TerminalWriter::print writes the message followed by a newline to stderr.
    ///
    /// We can't easily intercept stderr in a unit test, but we can verify the
    /// call completes without panicking and that has_status is false afterwards.
    #[test]
    fn terminal_writer_print_does_not_panic() {
        let tw = super::TerminalWriter::new();
        tw.print("test message");
        // has_status should be false after a print.
        let g = tw.inner.lock().unwrap();
        assert!(!g.has_status, "print clears has_status");
    }

    /// TerminalWriter::status sets has_status to true.
    #[test]
    fn terminal_writer_status_sets_has_status() {
        let tw = super::TerminalWriter::new();
        tw.status("Working...");
        let g = tw.inner.lock().unwrap();
        assert!(g.has_status, "status must set has_status");
    }

    /// TerminalWriter::clear_status resets has_status after a status call.
    #[test]
    fn terminal_writer_clear_status_resets_flag() {
        let tw = super::TerminalWriter::new();
        tw.status("Running");
        tw.clear_status();
        let g = tw.inner.lock().unwrap();
        assert!(!g.has_status, "clear_status must reset has_status");
    }

    /// clear_status is a no-op when no status is active (must not panic).
    #[test]
    fn terminal_writer_clear_status_noop_when_no_status() {
        let tw = super::TerminalWriter::new();
        tw.clear_status(); // should not panic
        let g = tw.inner.lock().unwrap();
        assert!(!g.has_status, "has_status must remain false");
    }

    /// print clears an active status before writing.
    #[test]
    fn terminal_writer_print_clears_active_status() {
        let tw = super::TerminalWriter::new();
        tw.status("Pending");
        tw.print("Done"); // must clear status first
        let g = tw.inner.lock().unwrap();
        assert!(!g.has_status, "print must clear has_status");
    }

    // ---- spawn_reader integration ----

    /// A bash block writing a print message to fd 3 via the preamble produces
    /// that message through spawn_reader without hanging.
    ///
    /// We cannot capture what spawn_reader writes to stderr in a unit test, but
    /// we verify the reader thread exits cleanly (join returns Ok) and the
    /// child process exits successfully.
    #[test]
    fn spawn_reader_drains_print_message_cleanly() {
        use std::sync::Arc;

        let mut ch = SideChannel::new().unwrap();
        let (ctrl_w_fd, resp_r_fd) = ch.child_fds();

        // Spawn bash that writes a JSON print message to fd 3.
        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c");
        cmd.arg(r#"printf '{"type":"print","message":"hello"}\n' >&3"#);
        unsafe {
            use std::os::unix::process::CommandExt as _;
            cmd.pre_exec(move || {
                if libc::dup2(ctrl_w_fd, 3) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if ctrl_w_fd != 3 {
                    libc::close(ctrl_w_fd);
                }
                // Dup the response reader to fd 4 even though it won't be used.
                if libc::dup2(resp_r_fd, 4) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if resp_r_fd != 4 {
                    libc::close(resp_r_fd);
                }
                Ok(())
            });
        }

        let ctrl_reader = ch.take_control_reader().unwrap();
        let resp_writer = ch
            .take_response_writer()
            .map(|fd| unsafe { std::fs::File::from_raw_fd(fd.into_raw_fd()) });
        let mut child = cmd.spawn().unwrap();
        ch.close_child_ends();

        let writer = Arc::new(super::TerminalWriter::new());
        let handle = super::spawn_reader(ctrl_reader, writer, None, resp_writer);

        let status = child.wait().unwrap();
        assert!(status.success(), "bash must exit successfully");

        // join must complete — if spawn_reader hangs on EOF, this blocks.
        handle.join().expect("reader thread must not panic");
    }

    /// A bash block writing an unknown message type to fd 3 is silently ignored
    /// and the reader thread exits cleanly.
    #[test]
    fn spawn_reader_ignores_unknown_message_type() {
        use std::sync::Arc;

        let mut ch = SideChannel::new().unwrap();
        let (ctrl_w_fd, resp_r_fd) = ch.child_fds();

        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c");
        cmd.arg(r#"printf '{"type":"unknown","data":"whatever"}\n' >&3"#);
        unsafe {
            use std::os::unix::process::CommandExt as _;
            cmd.pre_exec(move || {
                if libc::dup2(ctrl_w_fd, 3) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if ctrl_w_fd != 3 {
                    libc::close(ctrl_w_fd);
                }
                if libc::dup2(resp_r_fd, 4) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if resp_r_fd != 4 {
                    libc::close(resp_r_fd);
                }
                Ok(())
            });
        }

        let ctrl_reader = ch.take_control_reader().unwrap();
        let resp_writer = ch
            .take_response_writer()
            .map(|fd| unsafe { std::fs::File::from_raw_fd(fd.into_raw_fd()) });
        let mut child = cmd.spawn().unwrap();
        ch.close_child_ends();

        let writer = Arc::new(super::TerminalWriter::new());
        let handle = super::spawn_reader(ctrl_reader, writer, None, resp_writer);

        child.wait().unwrap();
        handle
            .join()
            .expect("reader thread must not panic on unknown type");
    }

    /// A bash block that never writes to fd 3 causes spawn_reader to exit
    /// cleanly when the pipe reaches EOF.
    #[test]
    fn spawn_reader_exits_on_eof_with_no_messages() {
        use std::sync::Arc;

        let mut ch = SideChannel::new().unwrap();
        let (ctrl_w_fd, resp_r_fd) = ch.child_fds();

        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c");
        cmd.arg("exit 0");
        unsafe {
            use std::os::unix::process::CommandExt as _;
            cmd.pre_exec(move || {
                if libc::dup2(ctrl_w_fd, 3) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if ctrl_w_fd != 3 {
                    libc::close(ctrl_w_fd);
                }
                if libc::dup2(resp_r_fd, 4) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if resp_r_fd != 4 {
                    libc::close(resp_r_fd);
                }
                Ok(())
            });
        }

        let ctrl_reader = ch.take_control_reader().unwrap();
        let resp_writer = ch
            .take_response_writer()
            .map(|fd| unsafe { std::fs::File::from_raw_fd(fd.into_raw_fd()) });
        let mut child = cmd.spawn().unwrap();
        ch.close_child_ends();

        let writer = Arc::new(super::TerminalWriter::new());
        let handle = super::spawn_reader(ctrl_reader, writer, None, resp_writer);

        let status = child.wait().unwrap();
        assert!(status.success());
        handle.join().expect("reader thread must not panic");
    }
}
