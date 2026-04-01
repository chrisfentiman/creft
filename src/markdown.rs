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
            let deps = parse_deps(&content, &lang_tag);
            blocks.push(CodeBlock {
                lang: lang_tag,
                code: content,
                deps,
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
                    llm_config: Some(LlmConfig::default()),
                    llm_parse_error: None,
                }
            } else {
                match serde_yaml_ng::from_str::<LlmConfig>(&yaml_section) {
                    Ok(config) => CodeBlock {
                        lang: "llm".to_string(),
                        code: prompt,
                        deps: Vec::new(),
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
                            llm_config: None,
                            llm_parse_error: Some(e.to_string()),
                        }
                    }
                }
            }
        }
    }
}

/// Parse inline dependency declarations from the first comment line of a code block.
///
/// Supported formats:
/// - `# deps: requests, beautifulsoup4` (Python/Bash)
/// - `// deps: lodash, chalk` (Node/JS/TS)
fn parse_deps(code: &str, lang: &str) -> Vec<String> {
    let first_line = match code.lines().next() {
        Some(l) => l.trim(),
        None => return Vec::new(),
    };

    let comment_prefix = match lang {
        "bash" | "sh" | "zsh" | "python" | "ruby" => "#",
        "node" | "javascript" | "typescript" | "js" | "ts" | "go" | "rust" => "//",
        _ => return Vec::new(),
    };

    let pattern = format!("{} deps:", comment_prefix);
    if !first_line.starts_with(&pattern) {
        return Vec::new();
    }

    first_line[pattern.len()..]
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

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
    }

    #[test]
    fn test_deps_node() {
        let body = "\n```node\n// deps: lodash, chalk\nconst _ = require('lodash');\n```\n";
        let (_, blocks) = extract_blocks(body);
        assert_eq!(blocks[0].deps, vec!["lodash", "chalk"]);
    }

    #[test]
    fn test_no_deps() {
        let body = "\n```bash\necho no deps here\n```\n";
        let (_, blocks) = extract_blocks(body);
        assert!(blocks[0].deps.is_empty());
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
}
