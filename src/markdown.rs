use crate::model::{CodeBlock, LlmConfig};

/// Extract fenced code blocks from markdown body.
///
/// Returns `(docs, executable_blocks)` where docs is the content of any
/// ````docs` block, and executable_blocks are all other fenced code blocks.
pub fn extract_blocks(body: &str) -> (Option<String>, Vec<CodeBlock>) {
    let mut docs: Option<String> = None;
    let mut blocks: Vec<CodeBlock> = Vec::new();

    let mut lines = body.lines().peekable();

    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("```") {
            continue;
        }

        let backtick_count = trimmed.chars().take_while(|c| *c == '`').count();
        if backtick_count < 3 {
            continue;
        }

        let lang_tag = trimmed[backtick_count..].trim().to_string();
        let closing = "`".repeat(backtick_count);

        let mut content = String::new();
        let mut found_close = false;
        for line in lines.by_ref() {
            let trimmed_inner = line.trim_start();
            if trimmed_inner.starts_with(&closing)
                && trimmed_inner[closing.len()..].trim().is_empty()
            {
                found_close = true;
                break;
            }
            content.push_str(line);
            content.push('\n');
        }

        if !found_close {
            continue;
        }

        if content.ends_with('\n') {
            content.pop();
        }

        if lang_tag == "docs" {
            docs = Some(content);
        } else if lang_tag == "llm" {
            let block = parse_llm_block(content);
            blocks.push(block);
        } else if !lang_tag.is_empty() {
            let (stripped, dirs) = parse_block_directives(&content, &lang_tag);
            blocks.push(CodeBlock {
                lang: lang_tag,
                code: stripped,
                deps: dirs.deps,
                extension: dirs.extension,
                flags: dirs.flags,
                llm_config: None,
                llm_parse_error: None,
            });
        }
    }

    (docs, blocks)
}

/// Parse an `llm` block's content, splitting on the first `---` separator.
///
/// Everything before `---` is a YAML config header. Everything after is the
/// prompt. If there is no `---`, the full content is the prompt and
/// `LlmConfig::default()` is used.
fn parse_llm_block(content: String) -> CodeBlock {
    // Find the first line that is exactly "---" (possibly followed by whitespace).
    let separator_pos = content
        .lines()
        .enumerate()
        .find(|(_, line)| line.trim() == "---")
        .map(|(idx, _)| idx);

    match separator_pos {
        None => {
            // No separator: full content is the prompt, use default config.
            CodeBlock {
                lang: "llm".to_string(),
                code: content,
                deps: Vec::new(),
                extension: None,
                flags: None,
                llm_config: Some(LlmConfig::default()),
                llm_parse_error: None,
            }
        }
        Some(sep_idx) => {
            // Collect lines before and after the separator.
            let all_lines: Vec<&str> = content.lines().collect();
            let yaml_lines = &all_lines[..sep_idx];
            let prompt_lines = &all_lines[sep_idx + 1..];

            let yaml_section = yaml_lines.join("\n");
            let prompt = prompt_lines.join("\n");

            if yaml_section.trim().is_empty() {
                // Empty YAML section: use defaults.
                CodeBlock {
                    lang: "llm".to_string(),
                    code: prompt,
                    deps: Vec::new(),
                    extension: None,
                    flags: None,
                    llm_config: Some(LlmConfig::default()),
                    llm_parse_error: None,
                }
            } else {
                match crate::yaml::from_str::<LlmConfig>(&yaml_section) {
                    Ok(config) => CodeBlock {
                        lang: "llm".to_string(),
                        code: prompt,
                        deps: Vec::new(),
                        extension: None,
                        flags: None,
                        llm_config: Some(config),
                        llm_parse_error: None,
                    },
                    Err(e) => {
                        // YAML parse failed: store prompt after separator, record error.
                        // Validation will detect lang == "llm" + llm_parse_error.is_some()
                        // and emit a diagnostic.
                        CodeBlock {
                            lang: "llm".to_string(),
                            code: prompt,
                            deps: Vec::new(),
                            extension: None,
                            flags: None,
                            llm_config: None,
                            llm_parse_error: Some(e.to_string()),
                        }
                    }
                }
            }
        }
    }
}

/// Directives parsed from a block's leading `#` (or `//`) comment lines.
///
/// Each field is empty/`None` when the directive is absent. The parser
/// scans consecutive directive comment lines from the top of the block
/// and stops at the first non-directive line.
#[derive(Debug, Default, Clone)]
pub(crate) struct BlockDirectives {
    pub deps: Vec<String>,
    pub extension: Option<String>,
    pub flags: Option<String>,
}

/// The set of lang tags that use `//` as their comment prefix for `deps:`.
///
/// Every tag NOT in this set is treated as using `#` as its comment prefix.
fn uses_slash_prefix(lang: &str) -> bool {
    matches!(
        lang,
        "node" | "javascript" | "typescript" | "js" | "ts" | "go" | "rust"
    )
}

/// Whether `line` is a recognised directive comment line for the given `lang`.
///
/// `# extension:` and `# flags:` are recognised with the `#` prefix for every
/// lang tag — including slash-prefix langs like `node`, `typescript`, and `go`.
/// The `deps:` directive discriminates on prefix: slash-prefix langs use
/// `// deps:` and all others use `# deps:`.
fn is_directive_line(line: &str, lang: &str) -> bool {
    let trimmed = line.trim();
    let deps_match = if uses_slash_prefix(lang) {
        trimmed.starts_with("// deps:")
    } else {
        trimmed.starts_with("# deps:")
    };
    deps_match || trimmed.starts_with("# extension:") || trimmed.starts_with("# flags:")
}

/// Parse and strip the directive comment block at the top of `code`.
///
/// Returns `(stripped_body, directives)`. The stripped body is every line
/// from the first non-directive line onward; directive lines are removed so
/// the interpreter never sees them. This is required for languages where `#`
/// is not a comment (zx, node, typescript).
///
/// Recognised directives:
/// - `# deps: <comma-separated>` for any tag whose comment prefix is `#`,
///   i.e. every tag that is NOT in the `//`-prefix set. The `//`-prefix set
///   is `node | javascript | typescript | js | ts | go | rust`; those tags
///   use `// deps: <comma-separated>` instead.
/// - `# extension: <ext>` (recognised universally with the `#` prefix).
/// - `# flags: <args...>` (recognised universally with the `#` prefix).
///
/// The scan terminates at the first non-directive line. If the first line is
/// not a recognised directive, the returned body equals the input verbatim
/// and all directive fields are empty/`None`.
pub(crate) fn parse_block_directives(code: &str, lang: &str) -> (String, BlockDirectives) {
    let mut dirs = BlockDirectives::default();
    let mut lines = code.lines().peekable();
    let mut skip_count: usize = 0;

    // Walk the contiguous leading run of directive lines.
    while let Some(&line) = lines.peek() {
        if !is_directive_line(line, lang) {
            break;
        }
        lines.next();
        skip_count += 1;

        let trimmed = line.trim();

        // `# extension:` and `# flags:` are universal — dispatch them first so
        // the slash-prefix branch below only ever sees `// deps:` lines.
        if let Some(rest) = trimmed.strip_prefix("# extension:") {
            dirs.extension = Some(rest.trim().to_string());
        } else if let Some(rest) = trimmed.strip_prefix("# flags:") {
            dirs.flags = Some(rest.trim().to_string());
        } else if uses_slash_prefix(lang) {
            // Only `// deps:` reaches here for slash-prefix langs.
            let val = trimmed["// deps:".len()..].trim();
            let parsed: Vec<String> = val
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            dirs.deps.extend(parsed);
        } else if let Some(rest) = trimmed.strip_prefix("# deps:") {
            let parsed: Vec<String> = rest
                .trim()
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            dirs.deps.extend(parsed);
        }
    }

    if skip_count == 0 {
        // No directives: return verbatim. Avoid an allocation if nothing changed.
        return (code.to_string(), dirs);
    }

    // Collect the remaining (non-directive) lines into the stripped body.
    // Preserve the same trailing-newline behaviour as extract_blocks: the
    // outer loop already stripped a trailing newline from `content` before
    // calling us, so we just join with "\n" — no extra trailing newline.
    let remaining: Vec<&str> = lines.collect();
    let stripped = remaining.join("\n");

    (stripped, dirs)
}

/// A finding from fence nesting analysis.
#[derive(Debug, Clone)]
pub struct FenceWarning {
    /// Line number (1-indexed) of the inner fence that would close the outer fence.
    pub line: usize,
    /// Line number (1-indexed) of the outer fence's opening.
    pub outer_line: usize,
    /// Number of backticks on the outer fence.
    pub outer_backticks: usize,
    /// Human-readable message.
    pub message: String,
}

/// Check raw markdown for inner fences that would prematurely close outer fences.
///
/// Returns a list of warnings. An empty list means no nesting issues were found.
///
/// This function mirrors the fence-matching logic of `extract_blocks` but does not
/// extract content — it only checks for the specific failure mode where an inner
/// fence (intended as example content) matches the closing fence pattern of an
/// outer block.
pub fn check_fence_nesting(body: &str) -> Vec<FenceWarning> {
    let mut warnings = Vec::new();

    let mut in_block = false;
    let mut outer_line: usize = 0;
    let mut outer_n: usize = 0;
    // Tracks whether the outer block has an info string. Bare blocks are
    // consumed to keep the state machine aligned with extract_blocks, but
    // inner-fence tracking only applies to named blocks.
    let mut has_info: bool = false;
    let mut inner_fence_lines: Vec<usize> = Vec::new();

    for (idx, line) in body.lines().enumerate() {
        let line_num = idx + 1;
        let trimmed = line.trim_start();

        if !in_block {
            if !trimmed.starts_with("```") {
                continue;
            }
            let n = trimmed.chars().take_while(|c| *c == '`').count();
            if n < 3 {
                continue;
            }
            let info = trimmed[n..].trim();
            // Enter block state regardless of whether there is an info string,
            // matching extract_blocks which consumes bare fences too.
            in_block = true;
            outer_line = line_num;
            outer_n = n;
            has_info = !info.is_empty();
            inner_fence_lines.clear();
        } else {
            // Use the same closing-fence check as extract_blocks:
            //   starts_with exactly outer_n backticks AND the rest is empty.
            let closing = "`".repeat(outer_n);
            if trimmed.starts_with(&closing) && trimmed[outer_n..].trim().is_empty() {
                // Closing fence: emit warning if we tracked inner fences in a named block.
                if has_info && !inner_fence_lines.is_empty() {
                    let inner_lines_str: Vec<String> =
                        inner_fence_lines.iter().map(|l| l.to_string()).collect();
                    let first_inner = inner_fence_lines[0];
                    let msg = format!(
                        "block at line {} uses {} backticks but contains inner fence syntax at line {}. \
                        Use {} or more backticks on the outer fence to contain inner fence examples.",
                        outer_line,
                        outer_n,
                        inner_lines_str.join(", "),
                        outer_n + 1,
                    );
                    warnings.push(FenceWarning {
                        line: first_inner,
                        outer_line,
                        outer_backticks: outer_n,
                        message: msg,
                    });
                }
                in_block = false;
            } else if has_info && trimmed.starts_with(&closing) {
                // The line starts with at least outer_n backticks but is not a closing fence.
                // Only track it as an inner fence if it carries a real info string — i.e., the
                // full backtick run on this line ends and is followed by non-empty text.  A line
                // that is only backticks (e.g. ``````) has no info string and is plain content
                // in extract_blocks, so it must not be tracked here either.
                let m = trimmed.chars().take_while(|c| *c == '`').count();
                let info = trimmed[m..].trim();
                if !info.is_empty() {
                    inner_fence_lines.push(line_num);
                }
            }
            // All other lines (including shorter backtick runs) are plain content.
        }
    }

    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use pretty_assertions::{assert_eq, assert_ne};

    #[test]
    fn test_single_bash_block() {
        let body = "\n```bash\necho hello\n```\n";
        let (docs, blocks) = extract_blocks(body);
        assert!(docs.is_none());
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].lang, "bash");
        assert_eq!(blocks[0].code, "echo hello");
    }

    #[test]
    fn test_docs_and_code() {
        let body = "\n```docs\nThis is documentation.\n```\n\n```bash\necho hi\n```\n";
        let (docs, blocks) = extract_blocks(body);
        assert_eq!(docs.unwrap(), "This is documentation.");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].lang, "bash");
    }

    #[test]
    fn test_multiple_blocks() {
        let body = "\n```bash\necho step1\n```\n\n```python\nprint('step2')\n```\n";
        let (_, blocks) = extract_blocks(body);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].lang, "bash");
        assert_eq!(blocks[1].lang, "python");
    }

    #[test]
    fn test_deps_python() {
        let body = "\n```python\n# deps: requests, bs4\nimport requests\n```\n";
        let (_, blocks) = extract_blocks(body);
        assert_eq!(blocks[0].deps, vec!["requests", "bs4"]);
        // Directive line must be stripped from the delivered code.
        assert!(
            !blocks[0].code.contains("# deps:"),
            "directive line must not appear in block code"
        );
        assert_eq!(blocks[0].code, "import requests");
    }

    #[test]
    fn test_deps_node() {
        let body = "\n```node\n// deps: lodash, chalk\nconst _ = require('lodash');\n```\n";
        let (_, blocks) = extract_blocks(body);
        assert_eq!(blocks[0].deps, vec!["lodash", "chalk"]);
        // Directive line must be stripped from the delivered code.
        assert!(
            !blocks[0].code.contains("// deps:"),
            "directive line must not appear in block code"
        );
        assert_eq!(blocks[0].code, "const _ = require('lodash');");
    }

    #[test]
    fn test_no_deps() {
        let body = "\n```bash\necho no deps here\n```\n";
        let (_, blocks) = extract_blocks(body);
        assert!(blocks[0].deps.is_empty());
    }

    #[test]
    fn directive_extension_populates_field_and_strips_line() {
        let body = "\n```bash\n# extension: zip\necho hello\n```\n";
        let (_, blocks) = extract_blocks(body);
        assert_eq!(blocks[0].extension, Some("zip".to_string()));
        assert!(!blocks[0].code.contains("# extension:"));
        assert_eq!(blocks[0].code, "echo hello");
    }

    #[test]
    fn directive_flags_populates_field_and_strips_line() {
        let body = "\n```bash\n# flags: -e\necho hello\n```\n";
        let (_, blocks) = extract_blocks(body);
        assert_eq!(blocks[0].flags, Some("-e".to_string()));
        assert!(!blocks[0].code.contains("# flags:"));
        assert_eq!(blocks[0].code, "echo hello");
    }

    #[test]
    fn all_three_directives_flags_first_all_populated_and_stripped() {
        let body = "\n```bash\n# flags: -x\n# deps: jq\n# extension: sh\necho hello\n```\n";
        let (_, blocks) = extract_blocks(body);
        assert_eq!(blocks[0].deps, vec!["jq"]);
        assert_eq!(blocks[0].extension, Some("sh".to_string()));
        assert_eq!(blocks[0].flags, Some("-x".to_string()));
        assert_eq!(blocks[0].code, "echo hello");
    }

    #[test]
    fn non_directive_first_line_leaves_fields_empty_and_code_verbatim() {
        let body = "\n```bash\necho hello\n# deps: jq\n```\n";
        let (_, blocks) = extract_blocks(body);
        assert!(blocks[0].deps.is_empty());
        assert!(blocks[0].extension.is_none());
        assert!(blocks[0].flags.is_none());
        // The directive after a non-directive line is not parsed.
        assert_eq!(blocks[0].code, "echo hello\n# deps: jq");
    }

    #[test]
    fn gap_between_directives_stops_scan_at_gap() {
        // Directive, then non-directive, then another directive: only the first
        // contiguous run is parsed and stripped.
        let body = "\n```bash\n# deps: jq\necho hello\n# flags: -x\n```\n";
        let (_, blocks) = extract_blocks(body);
        assert_eq!(blocks[0].deps, vec!["jq"]);
        assert!(blocks[0].flags.is_none());
        // The second directive line sits after the gap and remains in code.
        assert!(blocks[0].code.contains("# flags:"));
    }

    #[test]
    fn unknown_tag_hash_deps_parsed_and_stripped() {
        // Unknown tags (e.g. "zx") use the # prefix for deps — widened from the
        // previous closed-set recognition (bash|sh|zsh|python|ruby only).
        let body = "\n```zx\n# deps: jq\nawait $`echo hello`\n```\n";
        let (_, blocks) = extract_blocks(body);
        assert_eq!(blocks[0].deps, vec!["jq"]);
        assert!(!blocks[0].code.contains("# deps:"));
        assert_eq!(blocks[0].code, "await $`echo hello`");
    }

    #[test]
    fn zx_block_flags_directive_stripped_so_js_parser_never_sees_hash_line() {
        // The motivating bug: a zx block with `# flags: -` would cause
        // zx's JS parser to fail on the `#` line. Stripping fixes it.
        let body = "\n```zx\n# flags: -\nawait $`echo hello`\n```\n";
        let (_, blocks) = extract_blocks(body);
        assert_eq!(blocks[0].flags, Some("-".to_string()));
        assert_eq!(blocks[0].code, "await $`echo hello`");
    }

    #[test]
    fn extension_empty_value_yields_some_empty_string() {
        // Empty value is preserved as Some("") — validation (Stage 3) rejects it.
        let (stripped, dirs) = parse_block_directives("# extension: \necho hi", "bash");
        assert_eq!(dirs.extension, Some(String::new()));
        assert_eq!(stripped, "echo hi");
    }

    #[test]
    fn trailing_whitespace_on_directive_value_is_trimmed() {
        let (_, dirs) = parse_block_directives("# extension: zip   \necho hi", "bash");
        assert_eq!(dirs.extension, Some("zip".to_string()));
    }

    #[test]
    fn node_block_slash_deps_unchanged() {
        // node/js/ts tags continue to use // deps: (no widening to # deps:).
        let body = "\n```node\n// deps: lodash\nrequire('lodash')\n```\n";
        let (_, blocks) = extract_blocks(body);
        assert_eq!(blocks[0].deps, vec!["lodash"]);
        assert!(!blocks[0].code.contains("// deps:"));
    }

    #[test]
    fn node_block_extension_directive_recognised_universally() {
        // `# extension:` must be stripped and parsed on slash-prefix langs (node)
        // — the spec promises "recognised universally with the # prefix".
        let body = "\n```node\n# extension: mjs\nconst x = 1;\n```\n";
        let (_, blocks) = extract_blocks(body);
        assert_eq!(blocks[0].extension, Some("mjs".to_string()));
        assert!(!blocks[0].code.contains("# extension:"));
        assert_eq!(blocks[0].code, "const x = 1;");
    }

    #[test]
    fn node_block_flags_directive_recognised_universally() {
        // `# flags:` must be stripped and parsed on slash-prefix langs (node).
        // Previously this line would pass through to node as-is, causing a
        // SyntaxError because `#` is not a comment in JavaScript.
        let body = "\n```node\n# flags: --experimental-modules\nconst x = 1;\n```\n";
        let (_, blocks) = extract_blocks(body);
        assert_eq!(blocks[0].flags, Some("--experimental-modules".to_string()));
        assert!(!blocks[0].code.contains("# flags:"));
        assert_eq!(blocks[0].code, "const x = 1;");
    }

    #[test]
    fn node_block_mixed_slash_deps_and_hash_flags_both_parsed_and_stripped() {
        // A contiguous leading run that mixes `// deps:` and `# flags:` —
        // both are directive lines and the scan must not stop between them.
        let body = "\n```node\n// deps: lodash\n# flags: --no-warnings\nrequire('lodash');\n```\n";
        let (_, blocks) = extract_blocks(body);
        assert_eq!(blocks[0].deps, vec!["lodash"]);
        assert_eq!(blocks[0].flags, Some("--no-warnings".to_string()));
        assert_eq!(blocks[0].code, "require('lodash');");
    }

    #[test]
    fn block_with_no_directive_returns_code_verbatim() {
        let code = "echo hello\necho world";
        let (stripped, dirs) = parse_block_directives(code, "bash");
        assert_eq!(stripped, code);
        assert!(dirs.deps.is_empty());
        assert!(dirs.extension.is_none());
        assert!(dirs.flags.is_none());
    }

    #[test]
    fn flags_with_multiple_tokens_preserved_as_single_string() {
        let (_, dirs) = parse_block_directives("# flags: -e --no-warnings\ncode", "bash");
        assert_eq!(dirs.flags, Some("-e --no-warnings".to_string()));
    }

    #[test]
    fn test_empty_body() {
        let (docs, blocks) = extract_blocks("");
        assert!(docs.is_none());
        assert!(blocks.is_empty());
    }

    #[test]
    fn test_no_lang_tag_ignored() {
        let body = "\n```\nno lang tag\n```\n";
        let (_, blocks) = extract_blocks(body);
        assert!(blocks.is_empty());
    }

    #[test]
    fn test_multiline_code() {
        let body = "\n```bash\necho line1\necho line2\necho line3\n```\n";
        let (_, blocks) = extract_blocks(body);
        assert_eq!(blocks[0].code, "echo line1\necho line2\necho line3");
    }

    // ── llm block tests ──

    #[test]
    fn test_llm_block_with_all_fields() {
        let body = "```llm\nprovider: claude\nmodel: haiku\nparams: \"--max-tokens 500\"\n---\nReview this prompt.\n```\n";
        let (_, blocks) = extract_blocks(body);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].lang, "llm");
        assert_eq!(blocks[0].code, "Review this prompt.");
        let config = blocks[0].llm_config.as_ref().unwrap();
        assert_eq!(config.provider, "claude");
        assert_eq!(config.model, "haiku");
        assert_eq!(config.params, "--max-tokens 500");
        assert!(blocks[0].llm_parse_error.is_none());
    }

    #[test]
    fn test_llm_block_no_separator_is_prompt() {
        // No --- separator: entire body is the prompt, defaults apply.
        let body = "```llm\nSummarize the following:\n\n{{prev}}\n```\n";
        let (_, blocks) = extract_blocks(body);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].lang, "llm");
        assert_eq!(blocks[0].code, "Summarize the following:\n\n{{prev}}");
        let config = blocks[0].llm_config.as_ref().unwrap();
        assert_eq!(config.provider, "claude");
        assert!(config.model.is_empty());
        assert!(config.params.is_empty());
        assert!(blocks[0].llm_parse_error.is_none());
    }

    #[test]
    fn test_llm_block_empty_yaml_uses_defaults() {
        // Block starts with --- immediately: empty YAML section, use defaults.
        let body = "```llm\n---\nJust a prompt.\n```\n";
        let (_, blocks) = extract_blocks(body);
        assert_eq!(blocks.len(), 1);
        let config = blocks[0].llm_config.as_ref().unwrap();
        assert_eq!(config.provider, "claude");
        assert_eq!(blocks[0].code, "Just a prompt.");
        assert!(blocks[0].llm_parse_error.is_none());
    }

    #[test]
    fn test_llm_block_provider_only() {
        let body = "```llm\nprovider: gemini\n---\nExplain this code.\n```\n";
        let (_, blocks) = extract_blocks(body);
        let config = blocks[0].llm_config.as_ref().unwrap();
        assert_eq!(config.provider, "gemini");
        assert!(config.model.is_empty());
        assert_eq!(blocks[0].code, "Explain this code.");
    }

    #[test]
    fn test_llm_block_model_only() {
        let body = "```llm\nmodel: opus\n---\nWrite a summary.\n```\n";
        let (_, blocks) = extract_blocks(body);
        let config = blocks[0].llm_config.as_ref().unwrap();
        // provider defaults to "claude" when absent
        assert_eq!(config.provider, "claude");
        assert_eq!(config.model, "opus");
        assert_eq!(blocks[0].code, "Write a summary.");
    }

    #[test]
    fn test_llm_block_params_field() {
        let body = "```llm\nparams: \"--temperature 0.5 --max-tokens 1000\"\n---\nGenerate a story.\n```\n";
        let (_, blocks) = extract_blocks(body);
        let config = blocks[0].llm_config.as_ref().unwrap();
        assert_eq!(config.params, "--temperature 0.5 --max-tokens 1000");
    }

    #[test]
    fn test_llm_block_invalid_yaml_records_error() {
        // A --- separator present but the YAML above it is invalid.
        let body = "```llm\n: invalid: yaml: [\n---\nThe prompt.\n```\n";
        let (_, blocks) = extract_blocks(body);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].lang, "llm");
        // llm_config is None when YAML fails to parse
        assert!(blocks[0].llm_config.is_none());
        // llm_parse_error records the failure
        assert!(blocks[0].llm_parse_error.is_some());
        // code is the prompt (after ---)
        assert_eq!(blocks[0].code, "The prompt.");
    }

    #[test]
    fn test_llm_block_multiple_separators_only_first_splits() {
        // Second --- is part of the prompt (YAML in prompts is normal).
        let body = "```llm\nprovider: claude\n---\nFirst section.\n---\nSecond section.\n```\n";
        let (_, blocks) = extract_blocks(body);
        let config = blocks[0].llm_config.as_ref().unwrap();
        assert_eq!(config.provider, "claude");
        assert_eq!(blocks[0].code, "First section.\n---\nSecond section.");
    }

    #[test]
    fn test_non_llm_blocks_have_no_llm_config() {
        let body = "```bash\necho hello\n```\n";
        let (_, blocks) = extract_blocks(body);
        assert!(blocks[0].llm_config.is_none());
        assert!(blocks[0].llm_parse_error.is_none());
    }

    #[test]
    fn test_llm_block_deps_are_empty() {
        // llm blocks don't parse deps (no supported comment prefix)
        let body = "```llm\n---\nA prompt.\n```\n";
        let (_, blocks) = extract_blocks(body);
        assert!(blocks[0].deps.is_empty());
    }

    #[test]
    fn test_llm_block_among_regular_blocks() {
        // A skill with both a bash block and an llm block: both are extracted correctly.
        let body =
            "```bash\necho hello\n```\n\n```llm\nprovider: claude\n---\nSummarize output.\n```\n";
        let (_, blocks) = extract_blocks(body);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].lang, "bash");
        assert_eq!(blocks[0].code, "echo hello");
        assert!(blocks[0].llm_config.is_none());
        assert_eq!(blocks[1].lang, "llm");
        assert_eq!(blocks[1].code, "Summarize output.");
        let config = blocks[1].llm_config.as_ref().unwrap();
        assert_eq!(config.provider, "claude");
    }

    #[test]
    fn four_backtick_outer_fence_treats_inner_three_backtick_examples_as_literal_content() {
        // A 4-backtick bash block whose content includes 3-backtick examples (as the
        // session-start skill does) must be parsed as a single block. The inner ```llm
        // line must appear verbatim in the extracted code, not be treated as a new block.
        let body = "````bash\necho hello\n```llm\nprompt here\n```\n````\n";
        let (docs, blocks) = extract_blocks(body);
        assert!(docs.is_none());
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].lang, "bash");
        assert!(
            blocks[0].code.contains("```llm"),
            "expected block content to contain literal ```llm, got: {:?}",
            blocks[0].code,
        );
    }

    #[test]
    fn test_non_llm_block_with_triple_dash_in_code() {
        // A bash block whose code body contains "---" must not be mistaken for an llm separator.
        let body = "```bash\necho start\n---\necho end\n```\n";
        let (_, blocks) = extract_blocks(body);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].lang, "bash");
        assert_eq!(blocks[0].code, "echo start\n---\necho end");
        assert!(blocks[0].llm_config.is_none());
        assert!(blocks[0].llm_parse_error.is_none());
    }

    // ── check_fence_nesting tests ─────────────────────────────────────────────

    #[test]
    fn check_fence_nesting_detects_inner_fence_that_would_close_outer() {
        // 3-backtick bash block containing ```python and bare ``` — the inner
        // ``` would close the outer block prematurely.
        let body = "```bash\necho hello\n```python\nprint('hi')\n```\n```\n";
        let warnings = check_fence_nesting(body);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].outer_line, 1);
        assert_eq!(warnings[0].outer_backticks, 3);
        assert_eq!(warnings[0].line, 3);
        assert!(warnings[0].message.contains("line 1"));
        assert!(warnings[0].message.contains("3 backticks"));
        assert!(warnings[0].message.contains("line 3"));
        assert!(warnings[0].message.contains("4 or more"));
    }

    #[test]
    fn check_fence_nesting_no_warning_for_four_backtick_outer_with_inner_three() {
        // 4-backtick outer fence containing inner 3-backtick examples — clean.
        let body = "````bash\necho hello\n```python\nprint('hi')\n```\n````\n";
        let warnings = check_fence_nesting(body);
        assert!(warnings.is_empty());
    }

    #[test]
    fn check_fence_nesting_no_warning_for_block_with_no_inner_fences() {
        // Normal 3-backtick bash block with no fence-like content.
        let body = "```bash\necho hello\necho world\n```\n";
        let warnings = check_fence_nesting(body);
        assert!(warnings.is_empty());
    }

    #[test]
    fn check_fence_nesting_flags_only_problematic_block_in_multi_block_body() {
        // Two blocks: first has inner fence issue, second is clean.
        let body = concat!(
            "```bash\n",
            "echo step1\n",
            "```python\n",
            "print('x')\n",
            "```\n",
            "```\n",
            "\n",
            "````python\n",
            "print('clean')\n",
            "````\n",
        );
        let warnings = check_fence_nesting(body);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].outer_line, 1);
    }

    #[test]
    fn check_fence_nesting_no_false_positive_for_echo_with_backticks_inline() {
        // echo '```python' does NOT start with backticks — not a fence line.
        let body = "```bash\necho '```python'\necho done\n```\n";
        let warnings = check_fence_nesting(body);
        assert!(warnings.is_empty());
    }

    #[test]
    fn check_fence_nesting_no_warning_for_unclosed_outer_block() {
        // An unclosed outer block should not produce a spurious warning.
        let body = "```bash\necho hello\n```python\nprint('hi')\n";
        let warnings = check_fence_nesting(body);
        // The block never closes so no warning is emitted.
        assert!(warnings.is_empty());
    }

    #[test]
    fn check_fence_nesting_four_bare_backticks_inside_three_backtick_block_is_not_a_closing_fence()
    {
        // A line of 4 bare backticks inside a 3-backtick block is NOT a closing fence
        // per extract_blocks (starts_with "```" is true but "````"[3..] is "`", not empty).
        // The block should remain open and no warning should fire.
        let body = "```bash\necho hello\n````\necho still inside\n```\n";
        let warnings = check_fence_nesting(body);
        assert!(
            warnings.is_empty(),
            "4-bare-backtick line inside a 3-backtick block must not trigger a warning; got: {:?}",
            warnings
        );
    }

    #[test]
    fn check_fence_nesting_bare_outer_fence_consumes_content_and_does_not_cause_false_positive() {
        // A bare ``` block (no info string) followed by content including ```bash, then the
        // matching ```, followed by a real ```bash block with no inner fences. The bare block
        // must be consumed so that the real block is not misidentified as being inside it.
        let body = concat!(
            "```\n",
            "some content\n",
            "```bash\n",
            "echo inside bare\n",
            "```\n",
            "\n",
            "```bash\n",
            "echo real block\n",
            "```\n",
        );
        let warnings = check_fence_nesting(body);
        assert!(
            warnings.is_empty(),
            "bare outer fence followed by clean bash block must produce no warnings; got: {:?}",
            warnings
        );
    }
}
