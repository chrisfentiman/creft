use crate::error::CreftError;
use crate::model::PLACEHOLDER_RE;

/// Returns `true` if the language tag uses a shell interpreter that performs
/// word splitting and metacharacter expansion on substituted values.
fn should_shell_escape(lang: &str) -> bool {
    matches!(lang, "bash" | "sh" | "zsh")
}

/// Substitute `{{placeholder}}` values in a template string.
///
/// Named args from the command definition are matched positionally
/// to the provided values. Supports `{{name|default}}` syntax.
///
/// # Shell escaping
///
/// When `lang` is a shell language (`bash`, `sh`, `zsh`), values from the
/// `args` slice are single-quote escaped via `shell_escape::escape` so that
/// shell metacharacters (`$()`, backticks, semicolons, etc.) are treated as
/// literal characters. Author-supplied default values in `{{name|default}}`
/// syntax are NOT escaped — they are considered intentional shell code.
///
/// Non-shell languages (`python`, `node`, etc.) receive raw values.
///
/// # Edge cases
///
/// - Empty string: produces `''` under escaping. This is the correct shell
///   representation of an empty argument and makes intent unambiguous.
/// - `{{prev}}` (previous block output) is in the `args` slice and IS escaped
///   for shell blocks, since it may contain user-influenced content.
pub(crate) fn substitute(
    template: &str,
    args: &[(&str, &str)],
    lang: &str,
) -> Result<String, CreftError> {
    let re = &*PLACEHOLDER_RE;
    let escape = should_shell_escape(lang);

    let result = re.replace_all(template, |caps: &regex::Captures| {
        let name = &caps[1];
        let default_val = caps.get(2).map(|m| m.as_str());

        if let Some((_, val)) = args.iter().find(|(n, _)| *n == name) {
            if escape {
                shell_escape::escape((*val).into()).to_string()
            } else {
                val.to_string()
            }
        } else if let Some(d) = default_val {
            // Author-controlled default — no escaping regardless of language
            d.to_string()
        } else {
            // No matching arg/flag — leave as literal text.
            caps[0].to_string()
        }
    });

    Ok(result.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_substitute_basic() {
        // Safe value: no shell escaping changes the output for plain alphanumeric
        let result = substitute("Hello, {{name}}!", &[("name", "World")], "bash").unwrap();
        assert_eq!(result, "Hello, World!");
    }

    #[test]
    fn test_substitute_multiple() {
        let result = substitute("{{a}} and {{b}}", &[("a", "foo"), ("b", "bar")], "bash").unwrap();
        assert_eq!(result, "foo and bar");
    }

    #[test]
    fn test_substitute_default() {
        // Default value — author-controlled, not escaped
        let result = substitute("Hello, {{name|World}}!", &[], "bash").unwrap();
        assert_eq!(result, "Hello, World!");
    }

    #[test]
    fn test_substitute_default_overridden() {
        // User-supplied value in bash: "Chris" has no metacharacters so output is unchanged
        let result = substitute("Hello, {{name|World}}!", &[("name", "Chris")], "bash").unwrap();
        assert_eq!(result, "Hello, Chris!");
    }

    #[test]
    fn test_substitute_unmatched_passes_through() {
        let result = substitute("Hello, {{name}}!", &[], "bash").unwrap();
        assert_eq!(result, "Hello, {{name}}!");
    }

    #[test]
    fn test_substitute_no_double_replace() {
        // In bash mode, the substituted value '{{b}}' gets shell-escaped to `'{{b}}'`
        // which is fine — the point is that the inner {{b}} is NOT re-expanded
        let result = substitute("{{a}}", &[("a", "{{b}}"), ("b", "NOPE")], "bash").unwrap();
        // shell_escape wraps in single quotes: '{{b}}'
        assert_eq!(result, "'{{b}}'");
    }

    // ---- shell escaping tests ----

    #[test]
    fn test_shell_escape_subshell_injection_bash() {
        // Command injection attempt must be neutralized for bash
        let result = substitute("echo {{name}}", &[("name", "$(whoami)")], "bash").unwrap();
        // shell_escape produces single-quoted literal: '$(whoami)'
        assert_eq!(result, "echo '$(whoami)'");
    }

    #[test]
    fn test_shell_escape_subshell_injection_sh() {
        let result = substitute("echo {{name}}", &[("name", "$(whoami)")], "sh").unwrap();
        assert_eq!(result, "echo '$(whoami)'");
    }

    #[test]
    fn test_shell_escape_subshell_injection_zsh() {
        let result = substitute("echo {{name}}", &[("name", "$(whoami)")], "zsh").unwrap();
        assert_eq!(result, "echo '$(whoami)'");
    }

    #[test]
    fn test_no_shell_escape_python() {
        // Non-shell language: raw value, no escaping
        let result = substitute("print('{{name}}')", &[("name", "$(whoami)")], "python").unwrap();
        assert_eq!(result, "print('$(whoami)')");
    }

    #[test]
    fn test_no_shell_escape_node() {
        let result =
            substitute("console.log('{{name}}')", &[("name", "$(whoami)")], "node").unwrap();
        assert_eq!(result, "console.log('$(whoami)')");
    }

    #[test]
    fn test_no_shell_escape_python3() {
        let result = substitute("print('{{name}}')", &[("name", "$(whoami)")], "python3").unwrap();
        assert_eq!(result, "print('$(whoami)')");
    }

    #[test]
    fn test_shell_escape_default_not_escaped() {
        // Author-supplied default value is NOT escaped even in bash
        let result = substitute("echo {{name|default_val}}", &[], "bash").unwrap();
        assert_eq!(result, "echo default_val");
    }

    #[test]
    fn test_shell_escape_default_with_metachar_not_escaped() {
        // Author can put shell code in defaults — not escaped
        let result = substitute("echo {{name|$(date)}}", &[], "bash").unwrap();
        assert_eq!(result, "echo $(date)");
    }

    #[test]
    fn test_shell_escape_single_quote_in_value() {
        // Embedded single quote: O'Brien -> 'O'\''Brien'
        let result = substitute("echo {{name}}", &[("name", "O'Brien")], "bash").unwrap();
        // shell_escape handles embedded single quotes
        assert_eq!(result, "echo 'O'\\''Brien'");
    }

    #[test]
    fn test_shell_escape_empty_string() {
        // Empty string -> '' (documented behavior change: unambiguous empty arg)
        let result = substitute("echo {{name}}", &[("name", "")], "bash").unwrap();
        assert_eq!(result, "echo ''");
    }

    #[test]
    fn test_shell_escape_semicolon_injection() {
        // Semicolon would allow command chaining — must be escaped
        let result = substitute("echo {{name}}", &[("name", "hello; rm -rf /")], "bash").unwrap();
        assert_eq!(result, "echo 'hello; rm -rf /'");
    }

    #[test]
    fn test_shell_no_escape_for_unknown_lang() {
        // Unknown language: no escaping (treated as non-shell)
        let result = substitute("echo {{name}}", &[("name", "$(whoami)")], "ruby").unwrap();
        assert_eq!(result, "echo $(whoami)");
    }

    // ---- should_shell_escape ----

    #[test]
    fn test_should_shell_escape_langs() {
        assert!(should_shell_escape("bash"));
        assert!(should_shell_escape("sh"));
        assert!(should_shell_escape("zsh"));
        assert!(!should_shell_escape("python"));
        assert!(!should_shell_escape("node"));
        assert!(!should_shell_escape("ruby"));
    }

    #[test]
    fn test_sponge_substitute_prev() {
        // {{prev}} in an llm template must be replaced with upstream content.
        // The "llm" language tag must NOT shell-escape (no single-quoting of values).
        let result = substitute(
            "Process this: {{prev}}",
            &[("prev", "upstream content")],
            "llm",
        )
        .unwrap();
        assert_eq!(result, "Process this: upstream content");
    }

    #[test]
    fn test_sponge_substitute_prev_no_shell_escape() {
        // Shell metacharacters in prev must pass through unescaped in llm templates.
        let result =
            substitute("{{prev}}", &[("prev", "$(echo injected) `whoami`")], "llm").unwrap();
        assert_eq!(result, "$(echo injected) `whoami`");
    }
}
