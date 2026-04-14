use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};

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
}
