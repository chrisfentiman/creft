//! SIGINT forwarding for pipe chain execution.
//!
//! Installs a signal handler that forwards SIGINT to the child process
//! group. The original disposition is restored on guard drop.

/// Atomic storage for the child process group ID, used by the SIGINT
/// forwarding handler. Zero means "no active pipe chain".
pub(super) static PIPE_CHILD_PGID: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0);

/// Signal handler that forwards SIGINT to the child process group.
///
/// # Safety
///
/// Called by the OS as a signal handler; must be async-signal-safe.
/// Only async-signal-safe operations are performed:
/// - `AtomicU32::load` (no locks, no allocation)
/// - `libc::kill` (listed as async-signal-safe in POSIX)
pub(super) extern "C" fn sigint_forward_handler(_sig: libc::c_int) {
    let pgid = PIPE_CHILD_PGID.load(std::sync::atomic::Ordering::SeqCst);
    if pgid != 0 {
        // SAFETY: kill(-pgid, SIGINT) is async-signal-safe per POSIX.
        // pgid is non-zero (checked above). Negative pgid means "process group".
        unsafe {
            libc::kill(-(pgid as libc::pid_t), libc::SIGINT);
        }
    }
}

/// RAII guard that manages SIGINT handling during pipe chain execution.
///
/// Installs a signal handler that forwards SIGINT to the child process
/// group via `kill(-pgid, SIGINT)`. The original SIGINT disposition is
/// restored on drop.
///
/// Children are in their own process group (set up by `spawn_block`).
/// creft stays in the shell's process group and never transfers terminal
/// foreground ownership. This avoids races with shell job control (zsh
/// in particular) that cause SIGTTOU suspension.
pub(super) struct PipeSignalGuard {
    original_handler: libc::sighandler_t,
}

impl PipeSignalGuard {
    pub(super) fn new(child_pgid: u32) -> Self {
        PIPE_CHILD_PGID.store(child_pgid, std::sync::atomic::Ordering::SeqCst);

        // creft ignores SIGINT while waiting for children; the handler
        // forwards any SIGINT to the child process group instead. The
        // previous disposition is saved here and restored in Drop.
        //
        // SAFETY: sigint_forward_handler is an extern "C" fn and is
        // async-signal-safe (only calls atomic load and kill). Casting to
        // sighandler_t is the standard way to install a signal handler via
        // libc::signal.
        let original_handler = unsafe {
            libc::signal(
                libc::SIGINT,
                sigint_forward_handler as *const () as libc::sighandler_t,
            )
        };

        Self { original_handler }
    }
}

impl Drop for PipeSignalGuard {
    fn drop(&mut self) {
        // SAFETY: libc::signal with the previously-saved handler value is
        // the standard way to restore signal disposition.
        unsafe {
            libc::signal(libc::SIGINT, self.original_handler);
        }
        // Clear the atomic so a stale handler (if somehow called after drop)
        // does not forward to a dead process group.
        PIPE_CHILD_PGID.store(0, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use pretty_assertions::assert_eq;

    use super::{PIPE_CHILD_PGID, PipeSignalGuard};

    /// Verifies that `PipeSignalGuard` stores the process group ID on
    /// construction and clears it on drop.
    ///
    /// Each test runs in its own process under `cargo nextest`, so touching
    /// the global atomic does not interfere with other tests.
    #[test]
    fn pipe_signal_guard_stores_and_clears_pgid() {
        assert_eq!(
            PIPE_CHILD_PGID.load(Ordering::SeqCst),
            0,
            "PIPE_CHILD_PGID must be 0 before guard is created"
        );

        {
            let _guard = PipeSignalGuard::new(12345);
            assert_eq!(
                PIPE_CHILD_PGID.load(Ordering::SeqCst),
                12345,
                "PIPE_CHILD_PGID must equal the pgid passed to PipeSignalGuard::new"
            );
        }

        assert_eq!(
            PIPE_CHILD_PGID.load(Ordering::SeqCst),
            0,
            "PIPE_CHILD_PGID must be cleared to 0 when guard is dropped"
        );
    }
}
