/// Detect the user's preferred shell.
///
/// Resolution order:
/// 1. `CREFT_SHELL` env var (highest priority — one-off overrides)
/// 2. `settings_shell` — the persistent `shell` setting from settings.json
/// 3. `SHELL` env var (system default)
/// 4. None (use block language literally)
///
/// The caller loads settings and passes `settings.get("shell")`. This keeps
/// `shell.rs` free of filesystem dependencies and fully testable with pure values.
///
/// Returns the shell name (e.g., "bash", "zsh", "fish") or `None`.
pub fn detect(settings_shell: Option<&str>) -> Option<String> {
    if let Ok(val) = std::env::var("CREFT_SHELL") {
        if val == "none" {
            return None;
        }
        if !val.is_empty() {
            return Some(normalize(&val));
        }
    }
    if let Some(val) = settings_shell {
        if val == "none" {
            return None;
        }
        if !val.is_empty() {
            return Some(normalize(val));
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
        assert_eq!(detect(None), Some("zsh".to_string()));
    }

    #[test]
    fn detect_returns_bash_when_shell_is_full_path_bash() {
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::remove_var("CREFT_SHELL");
            std::env::set_var("SHELL", "/usr/local/bin/bash");
        }
        assert_eq!(detect(None), Some("bash".to_string()));
    }

    #[test]
    fn detect_returns_none_when_shell_is_unset() {
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::remove_var("CREFT_SHELL");
            std::env::remove_var("SHELL");
        }
        assert_eq!(detect(None), None);
    }

    #[test]
    fn detect_returns_none_when_creft_shell_is_none_literal() {
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::set_var("CREFT_SHELL", "none");
            std::env::set_var("SHELL", "/bin/zsh");
        }
        assert_eq!(detect(None), None);
    }

    /// CREFT_SHELL takes precedence over SHELL, and full paths are normalized.
    #[test]
    fn detect_returns_fish_when_creft_shell_is_fish_path() {
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::set_var("CREFT_SHELL", "/usr/bin/fish");
            std::env::set_var("SHELL", "/bin/bash");
        }
        assert_eq!(detect(None), Some("fish".to_string()));
    }

    /// Settings value fills the gap between CREFT_SHELL and $SHELL.
    #[test]
    fn detect_returns_settings_shell_when_creft_shell_absent() {
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::remove_var("CREFT_SHELL");
            std::env::set_var("SHELL", "/bin/bash");
        }
        assert_eq!(detect(Some("zsh")), Some("zsh".to_string()));
    }

    /// CREFT_SHELL beats settings when both are set.
    #[test]
    fn detect_creft_shell_overrides_settings() {
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::set_var("CREFT_SHELL", "fish");
            std::env::set_var("SHELL", "/bin/bash");
        }
        assert_eq!(detect(Some("zsh")), Some("fish".to_string()));
    }

    /// Settings value "none" disables shell detection even when $SHELL is set.
    #[test]
    fn detect_returns_none_when_settings_shell_is_none_literal() {
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::remove_var("CREFT_SHELL");
            std::env::set_var("SHELL", "/bin/zsh");
        }
        assert_eq!(detect(Some("none")), None);
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

    #[rstest]
    #[case::zsh_preference_overrides_bash_block("bash", Some("zsh"), Some("zsh"))]
    #[case::bash_preference_overrides_zsh_block("zsh", Some("bash"), Some("bash"))]
    #[case::ignores_preference_for_non_shell_block("python", Some("zsh"), None)]
    #[case::returns_none_when_no_preference("bash", None, None)]
    fn resolve_shell_applies_preference_to_shell_blocks_only(
        #[case] lang: &str,
        #[case] preference: Option<&str>,
        #[case] expected: Option<&str>,
    ) {
        assert_eq!(resolve_shell(lang, preference), expected);
    }
}
