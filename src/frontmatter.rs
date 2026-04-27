use crate::error::CreftError;
use crate::model::CommandDef;

/// Split a frontmatter document into its YAML header and body slices.
///
/// Returns `(yaml, body)` where `yaml` is the text between the opening and
/// closing `---` delimiters and `body` is everything after the closing
/// delimiter (including any leading newline before the body's first byte).
///
/// Errors with `CreftError::MissingFrontmatterDelimiter` when either delimiter
/// is missing. Accepts both `\n` and `\r\n` line endings.
///
/// This is the substring-extraction step of [`parse`]. Callers that need a
/// custom YAML schema (i.e., not [`CommandDef`]) use this directly.
pub fn split(content: &str) -> Result<(&str, &str), CreftError> {
    let after_open = if let Some(rest) = content.strip_prefix("---\r\n") {
        rest
    } else if let Some(rest) = content.strip_prefix("---\n") {
        rest
    } else {
        return Err(CreftError::MissingFrontmatterDelimiter);
    };

    let close_pos = after_open
        .find("\n---\n")
        .or_else(|| after_open.find("\n---\r\n"))
        .ok_or(CreftError::MissingFrontmatterDelimiter)?;

    let yaml = &after_open[..close_pos];
    let body_start = close_pos
        + if after_open[close_pos..].starts_with("\n---\r\n") {
            6
        } else {
            5
        };
    let body = &after_open[body_start..];

    Ok((yaml, body))
}

/// Parse a markdown file with YAML frontmatter into metadata and body.
///
/// Format:
/// ```text
/// ---
/// name: hello
/// description: greet someone
/// ---
///
/// body content here
/// ```
pub fn parse(content: &str) -> Result<(CommandDef, String), CreftError> {
    let (yaml, body) = split(content)?;

    let def: CommandDef =
        crate::yaml::from_str(yaml).map_err(|e| CreftError::Frontmatter(e.to_string()))?;

    if def.name.is_empty() {
        return Err(CreftError::InvalidName("name cannot be empty".into()));
    }

    Ok((def, body.to_string()))
}

/// Serialize a CommandDef back to frontmatter + body markdown.
pub fn serialize(def: &CommandDef, body: &str) -> Result<String, CreftError> {
    let yaml = crate::yaml::to_string(def);
    Ok(format!("---\n{}---\n{}", yaml, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use pretty_assertions::{assert_eq, assert_ne};

    #[test]
    fn test_parse_basic() {
        let input = "---\nname: hello\ndescription: greet someone\n---\n\n```bash\necho hi\n```\n";
        let (def, body) = parse(input).unwrap();
        assert_eq!(def.name, "hello");
        assert_eq!(def.description, "greet someone");
        assert!(body.contains("```bash"));
    }

    #[test]
    fn test_parse_with_args() {
        let input = "---\nname: hello\ndescription: greet\nargs:\n  - name: who\n    description: name\n---\nbody\n";
        let (def, body) = parse(input).unwrap();
        assert_eq!(def.args.len(), 1);
        assert_eq!(def.args[0].name, "who");
        assert_eq!(body, "body\n");
    }

    #[test]
    fn test_parse_with_env() {
        let input = "---\nname: test\ndescription: test\nenv:\n  - name: TOKEN\n    required: true\n---\nbody\n";
        let (def, _) = parse(input).unwrap();
        assert_eq!(def.env.len(), 1);
        assert!(def.env[0].required);
    }

    #[test]
    fn test_parse_missing_delimiter() {
        let input = "no frontmatter here";
        assert!(parse(input).is_err());
    }

    #[test]
    fn test_parse_missing_close() {
        let input = "---\nname: test\n";
        assert!(parse(input).is_err());
    }

    #[test]
    fn test_parse_empty_name() {
        let input = "---\nname: \"\"\ndescription: test\n---\nbody\n";
        assert!(parse(input).is_err());
    }

    #[test]
    fn test_roundtrip() {
        let input = "---\nname: hello\ndescription: greet someone\n---\n\n```bash\necho hi\n```\n";
        let (def, body) = parse(input).unwrap();
        let output = serialize(&def, &body).unwrap();
        let (def2, body2) = parse(&output).unwrap();
        assert_eq!(def.name, def2.name);
        assert_eq!(body, body2);
    }

    #[test]
    fn test_parse_with_supports() {
        let input =
            "---\nname: deploy\ndescription: deploy stuff\nsupports:\n  - dry-run\n---\nbody\n";
        let (def, _) = parse(input).unwrap();
        assert_eq!(def.supports, vec!["dry-run"]);
    }

    #[test]
    fn test_parse_without_supports() {
        // Commands without a supports field should get an empty vec by serde default
        let input = "---\nname: hello\ndescription: greet someone\n---\nbody\n";
        let (def, _) = parse(input).unwrap();
        assert!(def.supports.is_empty());
    }

    #[test]
    fn test_roundtrip_with_supports() {
        let input =
            "---\nname: deploy\ndescription: deploy stuff\nsupports:\n- dry-run\n---\nbody\n";
        let (def, body) = parse(input).unwrap();
        assert_eq!(def.supports, vec!["dry-run"]);
        let serialized = serialize(&def, &body).unwrap();
        let (def2, body2) = parse(&serialized).unwrap();
        assert_eq!(def2.supports, vec!["dry-run"]);
        assert_eq!(body, body2);
    }

    #[test]
    fn test_roundtrip_ignores_legacy_pipe_field() {
        // YAML with pipe: true deserializes without error. On roundtrip,
        // pipe does not appear in the serialized output (field is gone from the struct).
        let input = "---\nname: hello\ndescription: greet someone\npipe: true\n---\nbody\n";
        let (def, body) = parse(input).unwrap();
        assert_eq!(def.name, "hello");
        let serialized = serialize(&def, &body).unwrap();
        assert!(
            !serialized.contains("pipe"),
            "serialized output must not contain pipe after roundtrip; got:\n{serialized}"
        );
        let (def2, body2) = parse(&serialized).unwrap();
        assert_eq!(def2.name, "hello");
        assert_eq!(body, body2);
    }

    // ── split ─────────────────────────────────────────────────────────────────

    #[test]
    fn split_returns_yaml_and_body_for_unix_endings() {
        let input = "---\nname: hello\n---\nbody text here\n";
        let (yaml, body) = split(input).unwrap();
        assert_eq!(yaml, "name: hello");
        assert_eq!(body, "body text here\n");
    }

    #[test]
    fn split_returns_yaml_and_body_for_windows_endings() {
        let input = "---\r\nname: hello\r\n---\r\nbody text here\r\n";
        let (yaml, body) = split(input).unwrap();
        assert_eq!(yaml, "name: hello\r");
        assert_eq!(body, "body text here\r\n");
    }

    #[test]
    fn split_returns_error_for_missing_open() {
        let input = "name: hello\n---\nbody\n";
        assert!(
            matches!(split(input), Err(CreftError::MissingFrontmatterDelimiter)),
            "expected MissingFrontmatterDelimiter for missing opening ---"
        );
    }

    #[test]
    fn split_returns_error_for_missing_close() {
        let input = "---\nname: hello\n";
        assert!(
            matches!(split(input), Err(CreftError::MissingFrontmatterDelimiter)),
            "expected MissingFrontmatterDelimiter for missing closing ---"
        );
    }
}
