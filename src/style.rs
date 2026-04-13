use yansi::Condition;

/// Initialize yansi's global color condition.
///
/// Call once at startup (in main). Configures yansi to respect:
/// - `NO_COLOR` env var (any value disables color)
/// - `CLICOLOR` / `CLICOLOR_FORCE` env vars
/// - `TERM=dumb` (disables color)
/// - stdout/stderr TTY detection (non-TTY disables color)
pub(crate) fn init_color() {
    yansi::whenever(Condition::cached(color_enabled()));
}

/// Computes whether color output should be enabled.
///
/// Checks `TERM=dumb` (which yansi doesn't check natively) in addition
/// to yansi's built-in NO_COLOR, CLICOLOR, and TTY detection.
fn color_enabled() -> bool {
    if std::env::var("TERM").ok().as_deref() == Some("dumb") {
        return false;
    }
    // yansi's built-in: NO_COLOR disables, CLICOLOR/CLICOLOR_FORCE override,
    // and stdout+stderr must both be TTYs.
    Condition::stdouterr_are_tty() && Condition::no_color() && Condition::clicolor()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::{assert_eq, assert_ne};
    use yansi::Paint;

    use super::*;

    #[test]
    fn no_color_env_disables_color() {
        // SAFETY: nextest runs each test in a separate process, so env mutation
        // cannot race with other tests.
        unsafe { std::env::set_var("NO_COLOR", "1") };
        let enabled = color_enabled();
        unsafe { std::env::remove_var("NO_COLOR") };
        assert!(!enabled, "NO_COLOR=1 must disable color");
    }

    #[test]
    fn term_dumb_disables_color() {
        let prev = std::env::var("TERM").ok();
        // SAFETY: nextest runs each test in a separate process, so env mutation
        // cannot race with other tests.
        unsafe { std::env::set_var("TERM", "dumb") };
        let enabled = color_enabled();
        match prev {
            // SAFETY: same process-isolation guarantee as above.
            Some(v) => unsafe { std::env::set_var("TERM", v) },
            None => unsafe { std::env::remove_var("TERM") },
        }
        assert!(!enabled, "TERM=dumb must disable color");
    }

    #[test]
    fn init_color_no_color_disables_yansi() {
        // SAFETY: nextest runs each test in a separate process, so env mutation
        // cannot race with other tests.
        unsafe { std::env::set_var("NO_COLOR", "1") };
        init_color();
        let enabled = yansi::is_enabled();
        unsafe { std::env::remove_var("NO_COLOR") };
        yansi::enable();
        assert!(!enabled, "init_color with NO_COLOR=1 must disable yansi");
    }

    #[test]
    fn init_color_term_dumb_disables_yansi() {
        let prev = std::env::var("TERM").ok();
        // SAFETY: nextest runs each test in a separate process, so env mutation
        // cannot race with other tests.
        unsafe { std::env::set_var("TERM", "dumb") };
        init_color();
        let enabled = yansi::is_enabled();
        match prev {
            // SAFETY: same process-isolation guarantee as above.
            Some(v) => unsafe { std::env::set_var("TERM", v) },
            None => unsafe { std::env::remove_var("TERM") },
        }
        yansi::enable();
        assert!(!enabled, "init_color with TERM=dumb must disable yansi");
    }

    #[test]
    fn paint_disabled_produces_no_ansi() {
        yansi::disable();
        let s = "hello".bold().to_string();
        yansi::enable();
        assert_eq!(s, "hello");
    }

    #[test]
    fn paint_enabled_produces_ansi_bold() {
        yansi::enable();
        let s = "hello".bold().to_string();
        assert_ne!(
            s, "hello",
            "bold output must differ from plain text when enabled"
        );
        assert!(s.contains("\x1b["), "bold output must contain ANSI escape");
    }
}
