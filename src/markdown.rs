use crate::model::CodeBlock;

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
        } else if !lang_tag.is_empty() {
            let deps = parse_deps(&content, &lang_tag);
            blocks.push(CodeBlock {
                lang: lang_tag,
                code: content,
                deps,
            });
        }
    }

    (docs, blocks)
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
}
