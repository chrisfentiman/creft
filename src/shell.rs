/// Detect the user's preferred shell.
///
/// Resolution order:
/// 1. `CREFT_SHELL` env var (explicit override)
/// 2. `SHELL` env var (standard Unix convention)
/// 3. None (use block language literally)
///
/// Returns the shell name (e.g., "bash", "zsh", "fish") or None.
pub fn detect() -> Option<String> {
    if let Ok(val) = std::env::var("CREFT_SHELL") {
        if !val.is_empty() && val != "none" {
            return Some(normalize(&val));
        }
        if val == "none" {
            return None;
        }
    }
    std::env::var("SHELL")
        .ok()
        .filter(|v| !v.is_empty())
        .map(|v| normalize(&v))
}

/// Normalize a shell path or name to just the binary name.
///
/// "/bin/zsh" -> "zsh", "/usr/local/bin/bash" -> "bash", "zsh" -> "zsh"
fn normalize(shell: &str) -> String {
    std::path::Path::new(shell)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(shell)
        .to_string()
}

/// The set of shell-family languages where cross-substitution is safe.
///
/// `bash`, `sh`, and `zsh` share enough syntax for the kind of automation
/// scripts creft runs. `fish` is intentionally excluded — its syntax is
/// incompatible with POSIX-family scripts.
const SHELL_FAMILY: &[&str] = &["bash", "sh", "zsh"];

/// Returns true if `lang` is in the shell family.
pub fn is_shell_family(lang: &str) -> bool {
    SHELL_FAMILY.contains(&lang)
}

/// Given a block's language and the user's shell preference, return the
/// interpreter to use.
///
/// If both the block lang and the preference are in the shell family,
/// the preference wins. Otherwise the block lang is used as-is.
///
/// Returns `Some(preference)` if the preference should override the block lang,
/// `None` if the caller should fall through to `runner::interpreter()`.
pub fn resolve_shell<'a>(block_lang: &'a str, preference: Option<&'a str>) -> Option<&'a str> {
    let pref = preference?;
    if is_shell_family(block_lang) && is_shell_family(pref) {
        Some(pref)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::*;

    // ── detect() ─────────────────────────────────────────────────────────────
    //
    // These tests set env vars in-process. nextest runs each test in its own
    // child process so concurrent mutation of the environment is impossible.

    #[test]
    fn detect_returns_zsh_when_shell_is_bin_zsh() {
        // SAFETY: nextest runs each test in its own process; no concurrent
        // env reads or writes happen here.
        unsafe {
            std::env::remove_var("CREFT_SHELL");
            std::env::set_var("SHELL", "/bin/zsh");
        }
        assert_eq!(detect(), Some("zsh".to_string()));
    }

    #[test]
    fn detect_returns_bash_when_shell_is_full_path_bash() {
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::remove_var("CREFT_SHELL");
            std::env::set_var("SHELL", "/usr/local/bin/bash");
        }
        assert_eq!(detect(), Some("bash".to_string()));
    }

    #[test]
    fn detect_returns_none_when_shell_is_unset() {
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::remove_var("CREFT_SHELL");
            std::env::remove_var("SHELL");
        }
        assert_eq!(detect(), None);
    }

    #[test]
    fn detect_returns_none_when_creft_shell_is_none_literal() {
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::set_var("CREFT_SHELL", "none");
            std::env::set_var("SHELL", "/bin/zsh");
        }
        assert_eq!(detect(), None);
    }

    /// CREFT_SHELL takes precedence over SHELL, and full paths are normalized.
    #[test]
    fn detect_returns_fish_when_creft_shell_is_fish_path() {
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::set_var("CREFT_SHELL", "/usr/bin/fish");
            std::env::set_var("SHELL", "/bin/bash");
        }
        assert_eq!(detect(), Some("fish".to_string()));
    }

    // ── is_shell_family() ─────────────────────────────────────────────────────

    #[rstest]
    #[case::bash("bash", true)]
    #[case::sh("sh", true)]
    #[case::zsh("zsh", true)]
    #[case::fish("fish", false)]
    #[case::python("python", false)]
    #[case::node("node", false)]
    fn is_shell_family_classifies_lang_correctly(#[case] lang: &str, #[case] expected: bool) {
        assert_eq!(is_shell_family(lang), expected);
    }

    // ── resolve_shell() ───────────────────────────────────────────────────────

    /// zsh user + bash block → zsh wins.
    #[test]
    fn resolve_shell_zsh_preference_overrides_bash_block() {
        assert_eq!(resolve_shell("bash", Some("zsh")), Some("zsh"));
    }

    /// bash user + zsh block → bash wins.
    #[test]
    fn resolve_shell_bash_preference_overrides_zsh_block() {
        assert_eq!(resolve_shell("zsh", Some("bash")), Some("bash"));
    }

    /// Non-shell block is never overridden by shell preference.
    #[test]
    fn resolve_shell_ignores_preference_for_python_block() {
        assert_eq!(resolve_shell("python", Some("zsh")), None);
    }

    /// No preference → fall through to block lang.
    #[test]
    fn resolve_shell_returns_none_when_preference_is_none() {
        assert_eq!(resolve_shell("bash", None), None);
    }
}
