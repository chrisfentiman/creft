use std::borrow::Cow;
use std::io::IsTerminal;

/// Determines whether stdout should receive ANSI formatting.
///
/// Checks (in order):
/// 1. `NO_COLOR` env var set → false (any value, including empty)
/// 2. `TERM=dumb` → false
/// 3. stdout is not a terminal → false
/// 4. Otherwise → true
///
/// Result is computed once per call. Not cached — callers that print
/// multiple lines should call once and pass the bool down.
pub(crate) fn use_ansi() -> bool {
    if std::env::var("NO_COLOR").is_ok() {
        return false;
    }
    if std::env::var("TERM").ok().as_deref() == Some("dumb") {
        return false;
    }
    std::io::stdout().is_terminal()
}

/// Wraps `s` in ANSI bold escape sequences if `ansi` is true.
/// Returns `s` unchanged (borrowed) if `ansi` is false.
///
/// Returns `Cow<'_, str>` to avoid allocation when formatting is disabled.
pub(crate) fn bold<'a>(s: &'a str, ansi: bool) -> Cow<'a, str> {
    if ansi {
        Cow::Owned(format!("\x1b[1m{s}\x1b[0m"))
    } else {
        Cow::Borrowed(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bold_enabled() {
        let result = bold("hello", true);
        assert_eq!(result, "\x1b[1mhello\x1b[0m");
    }

    #[test]
    fn test_bold_disabled_returns_borrowed() {
        let input = "hello";
        let result = bold(input, false);
        assert_eq!(result, "hello");
        // Verify zero allocation: result is borrowed (not owned)
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    #[test]
    fn test_bold_empty_string() {
        // Empty string still gets wrapped — no special case.
        let result = bold("", true);
        assert_eq!(result, "\x1b[1m\x1b[0m");
    }

    #[test]
    fn test_bold_disabled_empty_string() {
        let result = bold("", false);
        assert_eq!(result, "");
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    #[test]
    fn test_bold_enabled_is_owned() {
        let result = bold("text", true);
        assert!(matches!(result, Cow::Owned(_)));
    }
}
