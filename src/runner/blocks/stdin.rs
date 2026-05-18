use std::path::Path;

use crate::error::CreftError;
use crate::model::CodeBlock;

use super::super::interpreter;
use super::{BlockRunner, warn_missing_deps};

/// Runs the block by piping its expanded source to the interpreter's stdin.
///
/// Used for non-family language tags without an `# extension:` directive.
/// The command shape is `<interpreter> [flags...]`. `script_path` is
/// accepted per the trait contract but ignored — the body is delivered to
/// the child's stdin by `execute_block` (or by the sponge stage in a pipe
/// chain) via the existing `stdin_data` plumbing, the same mechanism
/// `LlmRunner` uses today.
pub(super) struct StdinRunner;

impl BlockRunner for StdinRunner {
    fn build_command(
        &self,
        block: &CodeBlock,
        _script_path: &Path,
        flags: &[String],
    ) -> Result<(std::process::Command, Option<tempfile::TempDir>), CreftError> {
        warn_missing_deps(block);

        let interp = interpreter(&block.lang);
        // Split on whitespace to handle multi-token interpreter strings, matching
        // the same approach used by ShellRunner for "npx tsx".
        let parts: Vec<&str> = interp.split_whitespace().collect();
        let mut c = std::process::Command::new(parts[0]);
        for p in &parts[1..] {
            c.arg(p);
        }
        // Flags are placed immediately after the interpreter, before any other args.
        // script_path is intentionally omitted — source is delivered via stdin.
        for f in flags {
            c.arg(f);
        }
        Ok((c, None))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use pretty_assertions::assert_eq;

    use crate::model::CodeBlock;

    use super::super::BlockRunner;
    use super::StdinRunner;

    fn make_block(lang: &str) -> CodeBlock {
        CodeBlock {
            lang: lang.to_string(),
            code: String::new(),
            deps: vec![],
            extension: None,
            flags: None,
            llm_config: None,
            llm_parse_error: None,
        }
    }

    fn make_block_with_flags(lang: &str, flags_str: &str) -> CodeBlock {
        CodeBlock {
            lang: lang.to_string(),
            code: String::new(),
            deps: vec![],
            extension: None,
            flags: Some(flags_str.to_string()),
            llm_config: None,
            llm_parse_error: None,
        }
    }

    /// The interpreter name comes from the lang tag verbatim for unknown tags.
    #[test]
    fn build_command_program_is_lang_tag() {
        let block = make_block("ruby");
        let script = Path::new("/tmp/creft-test.ruby");
        let (cmd, dir) = StdinRunner.build_command(&block, script, &[]).unwrap();
        assert_eq!(cmd.get_program(), "ruby");
        assert!(dir.is_none());
    }

    /// script_path must NOT appear in the args — source is delivered via stdin.
    #[test]
    fn build_command_does_not_include_script_path() {
        let block = make_block("ruby");
        let script = Path::new("/tmp/creft-test.ruby");
        let (cmd, _) = StdinRunner.build_command(&block, script, &[]).unwrap();
        let args: Vec<_> = cmd.get_args().collect();
        assert!(
            args.is_empty(),
            "StdinRunner must not pass the script path as an arg; got: {args:?}"
        );
    }

    /// Flags appear in the args list when provided.
    #[test]
    fn build_command_appends_flags_after_interpreter() {
        let block = make_block_with_flags("ruby", "-e");
        let script = Path::new("/tmp/creft-test.ruby");
        let flags = vec!["-e".to_string()];
        let (cmd, _) = StdinRunner.build_command(&block, script, &flags).unwrap();
        let args: Vec<_> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(args, ["-e"]);
    }

    /// Multiple flags are all passed.
    #[test]
    fn build_command_multiple_flags_in_order() {
        let block = make_block("deno");
        let script = Path::new("/tmp/creft-test.deno");
        let flags = vec!["run".to_string(), "--allow-net".to_string()];
        let (cmd, _) = StdinRunner.build_command(&block, script, &flags).unwrap();
        let args: Vec<_> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(args, ["run", "--allow-net"]);
    }

    /// No flags: args list is empty.
    #[test]
    fn build_command_no_flags_no_args() {
        let block = make_block("ruby");
        let script = Path::new("/tmp/creft-test.ruby");
        let (cmd, _) = StdinRunner.build_command(&block, script, &[]).unwrap();
        let args: Vec<_> = cmd.get_args().collect();
        assert!(args.is_empty());
    }
}
